use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::environment::Environment;
use crate::object::{Object, RcObject};
use crate::value::Value;
use crate::error::InterpreterError;
use crate::heap::{Allocator, GlobalAllocator, HeapManager};

pub mod extern_math;
use extern_math::ExternFn;

/// Per-enum entry registered with the evaluation context. Carries
/// enough info both for variant lookup at construction sites and for
/// deriving `type_args` on the resulting `Object::EnumVariant`.
#[derive(Debug, Clone)]
pub struct EnumRegistryEntry {
    pub generic_params: Vec<DefaultSymbol>,
    pub variants: Vec<EnumRegistryVariant>,
}

#[derive(Debug, Clone)]
pub struct EnumRegistryVariant {
    pub name: DefaultSymbol,
    pub payload_types: Vec<TypeDecl>,
}

/// One impl-block specialisation registered for a `(struct, method)`
/// pair. CONCRETE-IMPL Phase 2: a single `(struct_name, method_name)`
/// can have multiple specs distinguished by `target_type_args` (e.g.
/// `impl FromStr for Vec<u8>` registers under
/// `(Vec, from_str)` with `target_type_args = [TypeDecl::UInt8]`,
/// while `impl FromStr for Vec<i64>` registers separately under
/// the same pair with `[TypeDecl::Int64]`). Empty `target_type_args`
/// means a generic-parameterised impl (`impl<T> Foo<T>`).
#[derive(Debug, Clone)]
pub struct MethodSpec {
    pub target_type_args: Vec<TypeDecl>,
    pub method: Rc<MethodFunction>,
}

/// Per-struct entry registered with the evaluation context. Used
/// only for deriving `type_args` on `Object::Struct` so generic
/// instances print like the compiler.
#[derive(Debug, Clone)]
pub struct StructRegistryEntry {
    pub generic_params: Vec<DefaultSymbol>,
    pub fields: Vec<(DefaultSymbol, TypeDecl)>,
}

mod operators;
mod expression;
mod statement;
mod call;
mod slice;
mod builtin;

/// Whether `requires` and `ensures` clauses are evaluated at runtime. The
/// fields default to "both on" so the interpreter has the same semantics
/// it had before the env-var gate was introduced. `INTERPRETER_CONTRACTS`
/// flips one or both off; see `ContractMode::from_env`.
#[derive(Debug, Clone, Copy)]
pub struct ContractMode {
    pub check_pre: bool,
    pub check_post: bool,
}

impl Default for ContractMode {
    fn default() -> Self {
        Self { check_pre: true, check_post: true }
    }
}

impl ContractMode {
    /// Parse the active mode from the `INTERPRETER_CONTRACTS` environment
    /// variable. Recognised values (case-insensitive):
    ///   - `all` (or unset): both `requires` and `ensures` are evaluated
    ///   - `pre`: only `requires` runs; `ensures` is skipped
    ///   - `post`: only `ensures` runs; `requires` is skipped
    ///   - `off`: neither runs (D's `-release` equivalent)
    /// Any other value falls back to `all` and prints a warning to stderr,
    /// matching the philosophy of `INTERPRETER_JIT` (typos shouldn't
    /// silently disable safety).
    pub fn from_env() -> Self {
        let raw = match std::env::var("INTERPRETER_CONTRACTS") {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "all" | "on" | "1" | "true" => {
                Self { check_pre: true, check_post: true }
            }
            "pre" => Self { check_pre: true, check_post: false },
            "post" => Self { check_pre: false, check_post: true },
            "off" | "0" | "false" => Self { check_pre: false, check_post: false },
            other => {
                eprintln!(
                    "warning: INTERPRETER_CONTRACTS={other:?} not recognised; using `all`. \
                     Valid: all|pre|post|off"
                );
                Self::default()
            }
        }
    }
}

#[derive(Debug)]
pub enum EvaluationResult {
    None,
    /// Phase 3: carry a `Value`. Primitive variants stay inline; heap
    /// values keep their existing shared `Rc<RefCell<HeapObject>>`
    /// cell so mutation and aliasing semantics are unchanged.
    Value(Value),
    /// Function return propagation. `None` means a bare `return`
    /// without a value (Unit return); `Some(Value)` carries the
    /// returned value back through the call boundary.
    Return(Option<Value>),
    Break,
    Continue,
}

pub struct EvaluationContext<'a> {
    pub(super) stmt_pool: &'a StmtPool,
    pub(super) expr_pool: &'a ExprPool,
    pub string_interner: &'a mut DefaultStringInterner,
    pub(super) function: HashMap<DefaultSymbol, Rc<Function>>,
    /// Module-aware mirror of `function` keyed by
    /// `(module_qualifier, fn_name)`. The qualifier is the **last
    /// segment** of the originating module's dotted path
    /// (`Some("math")` for `core/std/math.t`) or `None` for
    /// user-authored top-level functions. Mirrors the IR-level
    /// `function_index` keying introduced for #193 and the
    /// type-checker `context.functions` keying for #193b. Bare
    /// `Expr::Call("add", ...)` resolves via
    /// `lookup_function_qualified(None, "add")`; qualified
    /// `Expr::AssociatedFunctionCall("math", "add", ...)` resolves
    /// via `lookup_function_qualified(Some("math"), "add")`. The
    /// flat `function` map above is kept for backwards-compatibility
    /// at sites that don't yet thread the qualifier.
    pub(super) function_qualified: HashMap<(Option<DefaultSymbol>, DefaultSymbol), Rc<Function>>,
    pub environment: Environment,
    pub(super) method_registry: HashMap<DefaultSymbol, HashMap<DefaultSymbol, Vec<MethodSpec>>>, // struct_name -> method_name -> [specs by target_type_args]
    pub(super) null_object: RcObject, // Pre-created null object for reuse
    pub(super) recursion_depth: u32,
    pub(super) max_recursion_depth: u32,
    // Shared heap state. The GlobalAllocator holds an Rc to this same cell so
    // pointer-based builtins (ptr_read/write, mem_copy, ...) can access memory
    // regardless of which allocator is active on the stack.
    pub(super) heap_manager: Rc<RefCell<HeapManager>>,
    // Process-wide default allocator. Always present at the bottom of
    // `allocator_stack` and returned by `__builtin_default_allocator()`.
    pub(super) global_allocator: Rc<dyn Allocator>,
    // Lexically-scoped allocator binding stack. `with allocator = expr { ... }`
    // pushes on entry and pops on exit. `allocator_stack.last()` is always
    // non-None because the global allocator sits at the bottom.
    pub(super) allocator_stack: Vec<Rc<dyn Allocator>>,
    // Registered enum types: enum_name -> entry. The entry records the
    // generic parameter symbols (empty for non-generic enums) and the
    // ordered variant definitions (variant name + payload type list).
    // Used both for variant lookup at construction sites and for
    // deriving `type_args` on `Object::EnumVariant` for display.
    pub(super) enum_definitions: HashMap<DefaultSymbol, EnumRegistryEntry>,
    // Registered struct types: struct_name -> entry. Same shape as
    // `enum_definitions` — used purely for deriving `type_args` on
    // `Object::Struct` so generic instances print like the compiler
    // (`Y<i64> { b: 2 }`).
    pub(super) struct_definitions: HashMap<DefaultSymbol, StructRegistryEntry>,
    /// Runtime gate for Design-by-Contract evaluation. Read once from
    /// `INTERPRETER_CONTRACTS` at construction; `call.rs` consults
    /// `check_pre` / `check_post` to decide whether to evaluate each clause.
    pub(super) contract_mode: ContractMode,
    /// Pre-interned symbol for the `result` keyword bound inside `ensures`
    /// clauses. Cached at construction so contract evaluation doesn't
    /// re-intern the same string on every call.
    pub(super) result_symbol: DefaultSymbol,
    /// Registry of extern fn implementations. Populated at construction
    /// from `extern_math::build_default_registry`. Look-up is by the
    /// extern fn's declared name (the user-visible identifier in source).
    /// Phase 2 of the math externalisation work — replaces the
    /// hardcoded `BuiltinFunction::{Sin, Cos, ...}` dispatch in
    /// `evaluation/builtin.rs` for any function the user declares as
    /// `extern fn`.
    pub(super) extern_registry: HashMap<&'static str, ExternFn>,
}

impl<'a> EvaluationContext<'a> {
    pub fn new(stmt_pool: &'a StmtPool, expr_pool: &'a ExprPool, string_interner: &'a mut DefaultStringInterner, function: HashMap<DefaultSymbol, Rc<Function>>) -> Self {
        Self::new_with_qualified(stmt_pool, expr_pool, string_interner, function, HashMap::new())
    }

    /// Construct with both the legacy bare-name map (`function`) and
    /// the module-qualified map (`function_qualified`). Callers that
    /// have access to `program.function_module_paths` should populate
    /// the latter so #193b's qualified lookups reach the right
    /// function.
    pub fn new_with_qualified(
        stmt_pool: &'a StmtPool,
        expr_pool: &'a ExprPool,
        string_interner: &'a mut DefaultStringInterner,
        function: HashMap<DefaultSymbol, Rc<Function>>,
        function_qualified: HashMap<(Option<DefaultSymbol>, DefaultSymbol), Rc<Function>>,
    ) -> Self {
        let heap_manager = Rc::new(RefCell::new(HeapManager::new()));
        let global_allocator: Rc<dyn Allocator> = Rc::new(GlobalAllocator::new(heap_manager.clone()));
        let allocator_stack: Vec<Rc<dyn Allocator>> = vec![global_allocator.clone()];
        let result_symbol = string_interner.get_or_intern("result");
        Self {
            stmt_pool,
            expr_pool,
            string_interner,
            function,
            function_qualified,
            environment: Environment::new(),
            method_registry: HashMap::new(),
            null_object: Rc::new(RefCell::new(Object::null_unknown())),
            recursion_depth: 0,
            max_recursion_depth: 1000, // Increased to support deeper recursion like fib(20)
            heap_manager,
            global_allocator,
            allocator_stack,
            enum_definitions: HashMap::new(),
            struct_definitions: HashMap::new(),
            contract_mode: ContractMode::from_env(),
            result_symbol,
            extern_registry: extern_math::build_default_registry(),
        }
    }

    /// Module-aware function resolver. Mirrors the type-checker's
    /// `TypeCheckContext::lookup_fn`:
    /// - `Some(qualifier)` looks up `(Some(q), name)` directly.
    /// - `None` (bare call) prefers `(None, name)`, then falls back to
    ///   the unique `(Some(_), name)` entry; ambiguous bare calls
    ///   return `None` so the caller can surface a clean error.
    /// Returns `None` if `function_qualified` is empty (legacy
    /// constructor path) — in that case callers fall back to the
    /// flat `function` map.
    pub(super) fn lookup_function_qualified(
        &self,
        qualifier: Option<DefaultSymbol>,
        name: DefaultSymbol,
    ) -> Option<Rc<Function>> {
        if self.function_qualified.is_empty() {
            return None;
        }
        if let Some(q) = qualifier {
            return self.function_qualified.get(&(Some(q), name)).cloned();
        }
        if let Some(f) = self.function_qualified.get(&(None, name)).cloned() {
            return Some(f);
        }
        let candidates: Vec<_> = self
            .function_qualified
            .iter()
            .filter(|((_, n), _)| *n == name)
            .collect();
        if candidates.len() == 1 {
            Some(candidates[0].1.clone())
        } else {
            None
        }
    }

    /// Override the contract mode after construction. Tests use this to
    /// exercise specific modes deterministically without process-level
    /// env mutation.
    pub fn set_contract_mode(&mut self, mode: ContractMode) {
        self.contract_mode = mode;
    }

    pub fn register_enum(&mut self, name: DefaultSymbol, entry: EnumRegistryEntry) {
        self.enum_definitions.insert(name, entry);
    }

    pub fn register_struct(
        &mut self,
        name: DefaultSymbol,
        entry: StructRegistryEntry,
    ) {
        self.struct_definitions.insert(name, entry);
    }

    /// Register an impl-block method. CONCRETE-IMPL Phase 2:
    /// `target_type_args` distinguishes multiple impls of the same
    /// `(struct, method)` pair under different concrete type args;
    /// pass an empty Vec for inherent / generic-parameterised impls.
    pub fn register_method(
        &mut self,
        struct_name: DefaultSymbol,
        method_name: DefaultSymbol,
        target_type_args: Vec<TypeDecl>,
        method: Rc<MethodFunction>,
    ) {
        let specs = self
            .method_registry
            .entry(struct_name)
            .or_default()
            .entry(method_name)
            .or_default();
        // Replace an existing spec with the same target_type_args
        // (later registration wins for the same exact args; this
        // matches the legacy single-entry HashMap semantics).
        if let Some(existing) = specs
            .iter_mut()
            .find(|s| s.target_type_args == target_type_args)
        {
            existing.method = method;
        } else {
            specs.push(MethodSpec { target_type_args, method });
        }
    }

    /// Resolve a method by struct + method name + receiver's concrete
    /// type args. Lookup priority:
    /// 1. exact match on `target_type_args`;
    /// 2. generic-parameterised impl with empty args;
    /// 3. if only one spec exists for this `(struct, method)` pair,
    ///    return it regardless of args mismatch — this preserves
    ///    legacy behaviour for associated function calls
    ///    (`Vec::from_str(...)`) where the call site has no
    ///    receiver and no annotation hint to feed concrete args
    ///    into the lookup. Phase 2b will thread annotation hints
    ///    through so this fallback can become stricter.
    /// Pass `&[]` when the receiver has no type args (inherent impls,
    /// non-generic structs, primitive receivers).
    pub fn get_method(
        &self,
        struct_name: DefaultSymbol,
        method_name: DefaultSymbol,
        receiver_type_args: &[TypeDecl],
    ) -> Option<Rc<MethodFunction>> {
        let specs = self.method_registry.get(&struct_name)?.get(&method_name)?;
        if let Some(spec) = specs
            .iter()
            .find(|s| s.target_type_args.as_slice() == receiver_type_args)
        {
            return Some(spec.method.clone());
        }
        if let Some(spec) = specs.iter().find(|s| s.target_type_args.is_empty()) {
            return Some(spec.method.clone());
        }
        if specs.len() == 1 {
            return Some(specs[0].method.clone());
        }
        None
    }

    /// Drop the `EvaluationResult` envelope of a successful evaluation,
    /// returning the produced value. **Pre-condition**: the caller has
    /// already separated control-flow signals (Return / Break / Continue)
    /// from values via `try_value!`. If a control-flow variant reaches
    /// here, that's an interpreter bug — flag it as InternalError rather
    /// than silently turning it into an error message the user sees.
    pub(super) fn unwrap_value(
        &self,
        result: EvaluationResult,
    ) -> Result<Rc<RefCell<Object>>, InterpreterError> {
        match result {
            EvaluationResult::Value(v) => Ok(v.into_rc()),
            EvaluationResult::Return(_)
            | EvaluationResult::Break
            | EvaluationResult::Continue
            | EvaluationResult::None => Err(InterpreterError::InternalError(
                "control-flow signal reached unwrap_value (use try_value! to extract values from positions where flow may occur)".to_string(),
            )),
        }
    }
}

/// Extract a `Value` from an `evaluate*` result, propagating any
/// control-flow signal (Return / Break / Continue) to the caller's
/// caller via early `return Ok(flow)`. Errors propagate via `?`.
///
/// Replaces the old `extract_value`, which converted flow into
/// `Err(InterpreterError::PropagateFlow(...))` and relied on no one
/// catching it — a latent bug because flow then leaked out as a
/// "Propagate flow:" message whenever `return` appeared in a value
/// position (e.g. `val y = if cond { return X } else { Y }`).
///
/// **Caller contract**: must return
/// `Result<EvaluationResult, InterpreterError>` so the macro can
/// `return Ok(flow)` cleanly. For functions returning
/// `Result<RcObject, InterpreterError>` (function-call boundaries,
/// contract evaluation), handle flow inline instead.
/// Bridging variant — extract the inner `Value` and immediately
/// convert it to a legacy `Rc<RefCell<Object>>`. Existing consumer
/// code that does `val.borrow()` keeps working unchanged. The cost
/// is one `Rc` allocation per primitive (matching the pre-Phase 3
/// behaviour). Hot paths can opt into `try_value_v!` to skip this
/// allocation.
#[macro_export]
macro_rules! try_value {
    ($result:expr) => {
        match $result {
            Ok($crate::evaluation::EvaluationResult::Value(v)) => v.into_rc(),
            Ok($crate::evaluation::EvaluationResult::Return(opt)) => {
                return Ok($crate::evaluation::EvaluationResult::Return(opt));
            }
            Ok(flow @ $crate::evaluation::EvaluationResult::Break) => return Ok(flow),
            Ok(flow @ $crate::evaluation::EvaluationResult::Continue) => return Ok(flow),
            Ok($crate::evaluation::EvaluationResult::None) => {
                return Err($crate::error::InterpreterError::InternalError(
                    "unexpected None evaluation result".to_string(),
                ));
            }
            Err(e) => return Err(e),
        }
    };
}

/// Phase 3 variant of `try_value!`: extract the primitive-friendly
/// `Value` directly. Use this in hot paths that benefit from inline
/// primitives.
#[macro_export]
macro_rules! try_value_v {
    ($result:expr) => {
        match $result {
            Ok($crate::evaluation::EvaluationResult::Value(v)) => v,
            Ok($crate::evaluation::EvaluationResult::Return(opt)) => {
                return Ok($crate::evaluation::EvaluationResult::Return(opt));
            }
            Ok(flow @ $crate::evaluation::EvaluationResult::Break) => return Ok(flow),
            Ok(flow @ $crate::evaluation::EvaluationResult::Continue) => return Ok(flow),
            Ok($crate::evaluation::EvaluationResult::None) => {
                return Err($crate::error::InterpreterError::InternalError(
                    "unexpected None evaluation result".to_string(),
                ));
            }
            Err(e) => return Err(e),
        }
    };
}

pub fn convert_object(e: &Expr) -> Result<Object, InterpreterError> {
    match e {
        Expr::True => Ok(Object::Bool(true)),
        Expr::False => Ok(Object::Bool(false)),
        Expr::Int64(v) => Ok(Object::Int64(*v)),
        Expr::UInt64(v) => Ok(Object::UInt64(*v)),
        Expr::Int8(v) => Ok(Object::Int8(*v)),
        Expr::Int16(v) => Ok(Object::Int16(*v)),
        Expr::Int32(v) => Ok(Object::Int32(*v)),
        Expr::UInt8(v) => Ok(Object::UInt8(*v)),
        Expr::UInt16(v) => Ok(Object::UInt16(*v)),
        Expr::UInt32(v) => Ok(Object::UInt32(*v)),
        Expr::Float64(v) => Ok(Object::Float64(*v)),
        Expr::String(v) => Ok(Object::ConstString(*v)),
        Expr::Number(_v) => {
            // Type-unspecified numbers should be resolved during type checking
            Err(InterpreterError::InternalError(format!(
                "Expr::Number should be transformed to concrete type during type checking: {e:?}"
            )))
        },
        _ => Err(InterpreterError::InternalError(format!(
            "Expression type not handled in convert_object: {e:?}"
        ))),
    }
}
