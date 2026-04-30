use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::environment::Environment;
use crate::object::{Object, RcObject};
use crate::value::Value;
use crate::error::InterpreterError;
use crate::heap::{Allocator, GlobalAllocator, HeapManager};

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
    pub environment: Environment,
    pub(super) method_registry: HashMap<DefaultSymbol, HashMap<DefaultSymbol, Rc<MethodFunction>>>, // struct_name -> method_name -> method
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
    // Registered enum types: enum_name -> ordered variant definitions. Each
    // variant records the payload arity so the interpreter can pull the
    // right number of argument values when building an EnumVariant object.
    pub(super) enum_definitions: HashMap<DefaultSymbol, Vec<(DefaultSymbol, usize)>>,
    /// Runtime gate for Design-by-Contract evaluation. Read once from
    /// `INTERPRETER_CONTRACTS` at construction; `call.rs` consults
    /// `check_pre` / `check_post` to decide whether to evaluate each clause.
    pub(super) contract_mode: ContractMode,
    /// Pre-interned symbol for the `result` keyword bound inside `ensures`
    /// clauses. Cached at construction so contract evaluation doesn't
    /// re-intern the same string on every call.
    pub(super) result_symbol: DefaultSymbol,
}

impl<'a> EvaluationContext<'a> {
    pub fn new(stmt_pool: &'a StmtPool, expr_pool: &'a ExprPool, string_interner: &'a mut DefaultStringInterner, function: HashMap<DefaultSymbol, Rc<Function>>) -> Self {
        let heap_manager = Rc::new(RefCell::new(HeapManager::new()));
        let global_allocator: Rc<dyn Allocator> = Rc::new(GlobalAllocator::new(heap_manager.clone()));
        let allocator_stack: Vec<Rc<dyn Allocator>> = vec![global_allocator.clone()];
        let result_symbol = string_interner.get_or_intern("result");
        Self {
            stmt_pool,
            expr_pool,
            string_interner,
            function,
            environment: Environment::new(),
            method_registry: HashMap::new(),
            null_object: Rc::new(RefCell::new(Object::null_unknown())),
            recursion_depth: 0,
            max_recursion_depth: 1000, // Increased to support deeper recursion like fib(20)
            heap_manager,
            global_allocator,
            allocator_stack,
            enum_definitions: HashMap::new(),
            contract_mode: ContractMode::from_env(),
            result_symbol,
        }
    }

    /// Override the contract mode after construction. Tests use this to
    /// exercise specific modes deterministically without process-level
    /// env mutation.
    pub fn set_contract_mode(&mut self, mode: ContractMode) {
        self.contract_mode = mode;
    }

    pub fn register_enum(&mut self, name: DefaultSymbol, variants: Vec<(DefaultSymbol, usize)>) {
        self.enum_definitions.insert(name, variants);
    }

    pub fn register_method(&mut self, struct_name: DefaultSymbol, method_name: DefaultSymbol, method: Rc<MethodFunction>) {
        self.method_registry
            .entry(struct_name)
            .or_default()
            .insert(method_name, method);
    }

    pub fn get_method(&self, struct_name: DefaultSymbol, method_name: DefaultSymbol) -> Option<Rc<MethodFunction>> {
        self.method_registry
            .get(&struct_name)?
            .get(&method_name)
            .cloned()
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
