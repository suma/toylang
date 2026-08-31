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

pub mod extern_io;
pub mod extern_net;
pub mod extern_math;
pub mod extern_ffi;
use extern_io::ExternBufFn;
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
/// DATA-ORIENTED Phase 1: this engine's column windows (`ps.mass`).
mod column;
mod slice;
mod builtin;
pub(crate) mod simd;

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
    ///
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
    /// LABEL: optional target label for `break @label`. Loops match by
    /// `Some(sym) == loop_label_sym`; bare `break` (`None`) exits the
    /// innermost loop. The signal is forwarded up the block stack
    /// until the matching loop consumes it.
    Break(Option<DefaultSymbol>),
    /// LABEL: same dispatch convention as `Break`.
    Continue(Option<DefaultSymbol>),
}

pub struct EvaluationContext<'a> {
    pub(super) stmt_pool: &'a StmtPool,
    pub(super) expr_pool: &'a ExprPool,
    pub string_interner: &'a mut DefaultStringInterner,
    /// `Rc` so property-check trials (TEST-PERF) can share one
    /// program-derived map across many fresh evaluation contexts; the
    /// tree-walker only reads it while running.
    pub(crate) function: Rc<HashMap<DefaultSymbol, Rc<Function>>>,
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
    pub(crate) function_qualified: Rc<HashMap<(Option<DefaultSymbol>, DefaultSymbol), Rc<Function>>>,
    pub environment: Environment,
    /// `Rc` for the same reason as `function` (shared across trials).
    pub(crate) method_registry: Rc<HashMap<DefaultSymbol, HashMap<DefaultSymbol, Vec<MethodSpec>>>>, // struct_name -> method_name -> [specs by target_type_args]
    pub(super) null_object: RcObject, // Pre-created null object for reuse
    /// Source locations for expressions, when the caller supplied them.
    /// LLM-LOOP P6: the interpreter had no access to positions at all,
    /// so a runtime failure could not say where it happened.
    pub location_pool: Option<&'a frontend::ast::LocationPool>,
    /// The program's source map, when the caller supplied it. The
    /// memory profile wants the *file* an allocation site is in, and a
    /// `SourceLocation` carries a `FileId`, not a name (DEBUG-OBS D2).
    pub source_map: Option<&'a frontend::source_map::SourceMap>,
    /// Toylang call stack, innermost last. Used to build panic
    /// backtraces; empty when nothing is running.
    pub(super) call_stack: Vec<crate::error::CallFrame>,
    pub(super) recursion_depth: u32,
    pub(super) max_recursion_depth: u32,
    // Nesting depth of `evaluate_function_with_values_writeback`
    // frames. The tree-walker consumes a host stack frame per toylang
    // call, so a deeply recursive program (or one whose recursion the
    // IR VM could not lift) would overflow the host stack *before* the
    // `recursion_depth` guard above (which counts expression nesting)
    // ever fires. This guard trips first and turns a fatal stack
    // overflow (exit 134) into a plain error.
    pub(super) call_depth: u32,
    pub(super) max_call_depth: u32,
    /// Loop iterations executed so far, and the ceiling the caller
    /// imposed (CHECK-NONTERMINATION).
    ///
    /// `None` — the default, and what every ordinary run uses — means
    /// no ceiling: a program the user asked to run is allowed to loop
    /// as long as it likes. `--check` sets one, because a property
    /// trial calls a function with inputs nobody wrote it for, and
    /// `while i <= n` with a sampled `n = u64::MAX` would otherwise
    /// hang the checker instead of reporting anything.
    ///
    /// The budget counts loop back-edges rather than wall-clock time
    /// on purpose: `--check --seed=0x99` promises to replay, and a
    /// clock would make the same seed pass on a fast machine and get
    /// cut on a slow one. Back-edges are also the complete set of
    /// places a toylang run can diverge — recursion is already bounded
    /// by `max_call_depth`, and the language has no other backward
    /// jump — so counting them costs one increment per iteration and
    /// nothing at all on straight-line code.
    pub(super) loop_steps: u64,
    pub(super) max_loop_steps: Option<u64>,
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
    pub(crate) enum_definitions: Rc<HashMap<DefaultSymbol, EnumRegistryEntry>>,
    // Registered struct types: struct_name -> entry. Same shape as
    // `enum_definitions` — used purely for deriving `type_args` on
    // `Object::Struct` so generic instances print like the compiler
    // (`Y<i64> { b: 2 }`).
    pub(crate) struct_definitions: Rc<HashMap<DefaultSymbol, StructRegistryEntry>>,
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
    /// EXTERN-BUF: the externs that need to reach toylang memory.
    ///
    /// `ExternFn` takes only the argument values, which is enough for
    /// everything that crosses the boundary as a scalar or a `str`.
    /// An extern handed a `(ptr, len)` buffer cannot work from those
    /// alone: in this engine a `ptr` is an index into `HeapManager`,
    /// not a host address, so the implementation needs the context to
    /// resolve it. Rather than widen every entry, buffer externs get
    /// their own table; the compiled lanes need nothing extra, since
    /// there the pointer already *is* the address.
    pub(super) extern_buf_registry: HashMap<&'static str, ExternBufFn>,
    /// Phase 5 (汎用 RAII): set of struct symbols that have an
    /// `impl Drop for <Struct>` block. Populated at startup from
    /// `build_method_registry` (we record the trait_name field of
    /// each impl block and pick out the ones whose trait resolves
    /// to "Drop"). Consumed by `register_drop_if_needed` at val /
    /// var binding time — bindings whose runtime value is a
    /// struct in this set get pushed onto `drop_scopes` and
    /// `Drop::drop` is auto-called when the scope exits.
    pub(crate) drop_trait_structs: std::rc::Rc<std::collections::HashSet<DefaultSymbol>>,
    /// BOX-T: `val` / `var` statements whose value was handed to
    /// something that outlives them. A binding listed here does not
    /// register a drop — the receiver owns the resource now. Copied
    /// from `File::transferred_bindings` at startup.
    pub(crate) transferred_bindings: std::rc::Rc<std::collections::HashSet<frontend::ast::StmtRef>>,
    /// Phase 5 (汎用 RAII): per-active-scope LIFO list of bindings
    /// awaiting auto-drop. Each `enter_drop_scope` pushes a fresh
    /// Vec, `register_drop` appends, `exit_drop_scope` runs the
    /// entries in reverse declaration order before popping the
    /// scope. The depth mirrors `Environment::var` so block enter
    /// / exit and function call boundaries stay in lock-step.
    pub(super) drop_scopes: Vec<Vec<DropEntry>>,
    /// POINTER P1: active generic type-argument scopes, innermost
    /// last. `__builtin_sizeof::<T>()` needs to know what `T` was
    /// instantiated with, and the tree-walker — unlike the compiled
    /// backends, which monomorphise with a substitution table — has
    /// no such table. Call boundaries that *do* know the arguments
    /// push a scope:
    ///   - a method call maps the receiver's runtime `type_args`
    ///     onto the declaring struct / enum's `generic_params`;
    ///   - an associated call / free function call maps the
    ///     callee's generic params from the argument values, and —
    ///     for a `Self`-returning associated call — from the
    ///     pending `val` / `var` annotation.
    ///
    /// `sizeof::<T>()` reads the merged stack; an unbound parameter
    /// is a runtime error rather than a silent guess.
    pub(super) generic_type_scopes: Vec<HashMap<DefaultSymbol, TypeDecl>>,
    /// POINTER P1: the annotation of the `val` / `var` currently
    /// being bound, visible while its rhs evaluates. `val h:
    /// Holder<u64> = Holder::make(n)` puts `T` only in the return
    /// type, so the annotation is the one place that knows it — the
    /// same information the compiled lanes' let-lowering uses when
    /// it instantiates the call. Cleared when the binding finishes.
    pub(super) pending_annotation: Option<TypeDecl>,
}

/// Phase 5 (汎用 RAII): one auto-drop record. `name` is just for
/// diagnostics; `value` is the `Rc<RefCell<Object>>` that backs
/// the binding — the auto-drop call goes against this Rc so
/// mutations the body made via field access (`s.field = ...`)
/// are visible inside the synthesized `drop(&mut self)` body.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct DropEntry {
    pub(super) name: DefaultSymbol,
    pub(super) value: RcObject,
}

impl<'a> EvaluationContext<'a> {
    /// Source location of an expression, when locations are available.
    pub fn expr_location(&self, expr: &ExprRef) -> Option<frontend::type_checker::SourceLocation> {
        self.location_pool
            .and_then(|pool| pool.get_expr_location(expr).cloned())
    }

    /// Build a `Panic` carrying where it happened and how it got there.
    ///
    /// The backtrace is reversed so the innermost call comes first —
    /// the frame nearest the failure is the one a reader wants at the
    /// top.
    pub fn panic_error(
        &self,
        message: String,
        location: Option<frontend::type_checker::SourceLocation>,
    ) -> InterpreterError {
        let mut backtrace = self.call_stack.clone();
        backtrace.reverse();
        InterpreterError::Panic { message, location, backtrace }
    }

    /// Enter a toylang call frame (DEBUG-OBS D1).
    ///
    /// `function` is what the user wrote at the call site, qualified
    /// by the receiver's runtime type for methods (`S::boom`), and
    /// `call_site` is where the call was written. Every path that
    /// enters user code pairs this with [`Self::pop_frame`] — a bare
    /// `panic:` line with no backtrace was the interpreter's answer
    /// for method / associated / closure / `dyn` calls until this
    /// existed, because the one push site lived on the `Expr::Call`
    /// branch alone (`DEBUG_OBSERVABILITY.md` 実測 3).
    pub(crate) fn push_frame(
        &mut self,
        function: String,
        call_site: Option<frontend::type_checker::SourceLocation>,
    ) {
        self.call_stack.push(crate::error::CallFrame { function, call_site });
    }

    /// Push the entry function's own frame (DEBUG-OBS D1).
    ///
    /// `main` never appeared in a backtrace because nothing *calls*
    /// it — the frames were pushed by the call evaluator, and the
    /// entry is entered directly. A backtrace that stops one frame
    /// short of the bottom reads as if it were truncated.
    pub(crate) fn push_entry_frame(&mut self, function: DefaultSymbol) {
        let name = self
            .string_interner
            .resolve(function)
            .unwrap_or("<entry>")
            .to_string();
        self.push_frame(name, None);
    }

    /// Leave a call frame that returned normally.
    ///
    /// Deliberately *not* called on the error path: a panic unwinds to
    /// the top, and the stack as it stood at the failure is exactly
    /// what the report needs.
    pub(crate) fn pop_frame(&mut self) {
        self.call_stack.pop();
    }

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
            function: Rc::new(function),
            function_qualified: Rc::new(function_qualified),
            environment: Environment::new(),
            method_registry: Rc::new(HashMap::new()),
            null_object: Rc::new(RefCell::new(Object::null_unknown())),
            location_pool: None,
            source_map: None,
            call_stack: Vec::new(),
            recursion_depth: 0,
            max_recursion_depth: 1000, // Increased to support deeper recursion like fib(20)
            call_depth: 0,
            // The tree-walker burns a large host stack frame per
            // toylang call (function body + nested expression
            // evaluation). In a debug build a default 2 MiB test
            // thread overflows around 40 frames — well before the
            // expression-nesting guard (1000) fires. Count call
            // frames explicitly and trip first, so a deep recursion
            // on the tree-walker fallback path stops with a plain
            // error instead of `fatal runtime error: stack overflow`
            // (exit 134). The IR VM runs deep recursion on the heap,
            // so the guard only constrains the fallback path.
            max_call_depth: 30,
            loop_steps: 0,
            max_loop_steps: None,
            heap_manager,
            global_allocator,
            allocator_stack,
            enum_definitions: Rc::new(HashMap::new()),
            struct_definitions: Rc::new(HashMap::new()),
            contract_mode: ContractMode::from_env(),
            result_symbol,
            extern_registry: {
                let mut registry = extern_math::build_default_registry();
                registry.extend(extern_io::build_io_registry());
                registry.extend(extern_net::build_net_registry());
                registry
            },
            extern_buf_registry: {
                let mut registry = extern_io::build_io_buf_registry();
                registry.extend(extern_net::build_net_buf_registry());
                registry
            },
            drop_trait_structs: Rc::new(std::collections::HashSet::new()),
            transferred_bindings: Rc::new(std::collections::HashSet::new()),
            drop_scopes: vec![Vec::new()],
            generic_type_scopes: Vec::new(),
            pending_annotation: None,
        }
    }

    /// Construct from [`crate::SharedRunData`] (TEST-PERF): program-
    /// derived maps and registries are shared `Rc`s, so a fresh
    /// evaluation context for a property-check trial only pays for the
    /// per-run state (heap, allocator stack, environment, drop scopes).
    /// The maps are read-only during execution — any code that needs to
    /// mutate them goes through `Rc::make_mut`, which clones on first
    /// write and only when this context is the sole owner.
    pub fn new_with_shared(
        stmt_pool: &'a StmtPool,
        expr_pool: &'a ExprPool,
        string_interner: &'a mut DefaultStringInterner,
        shared: &crate::SharedRunData,
    ) -> Self {
        let heap_manager = Rc::new(RefCell::new(HeapManager::new()));
        let global_allocator: Rc<dyn Allocator> = Rc::new(GlobalAllocator::new(heap_manager.clone()));
        let allocator_stack: Vec<Rc<dyn Allocator>> = vec![global_allocator.clone()];
        let result_symbol = string_interner.get_or_intern("result");
        Self {
            stmt_pool,
            expr_pool,
            string_interner,
            function: shared.func_map.clone(),
            function_qualified: shared.func_qualified.clone(),
            environment: Environment::new(),
            method_registry: shared.method_registry.clone(),
            null_object: Rc::new(RefCell::new(Object::null_unknown())),
            location_pool: None,
            source_map: None,
            call_stack: Vec::new(),
            recursion_depth: 0,
            max_recursion_depth: 1000,
            call_depth: 0,
            max_call_depth: 30,
            loop_steps: 0,
            max_loop_steps: None,
            heap_manager,
            global_allocator,
            allocator_stack,
            enum_definitions: shared.enum_definitions.clone(),
            struct_definitions: shared.struct_definitions.clone(),
            contract_mode: ContractMode::from_env(),
            result_symbol,
            extern_registry: {
                let mut registry = extern_math::build_default_registry();
                registry.extend(extern_io::build_io_registry());
                registry.extend(extern_net::build_net_registry());
                registry
            },
            extern_buf_registry: {
                let mut registry = extern_io::build_io_buf_registry();
                registry.extend(extern_net::build_net_buf_registry());
                registry
            },
            drop_trait_structs: shared.drop_trait_structs.clone(),
            transferred_bindings: shared.transferred_bindings.clone(),
            drop_scopes: vec![Vec::new()],
            generic_type_scopes: Vec::new(),
            pending_annotation: None,
        }
    }

    /// Module-aware function resolver. Mirrors the type-checker's
    /// `TypeCheckContext::lookup_fn`:
    /// - `Some(qualifier)` looks up `(Some(q), name)` directly.
    /// - `None` (bare call) prefers `(None, name)`, then falls back to
    ///   the unique `(Some(_), name)` entry; ambiguous bare calls
    ///   return `None` so the caller can surface a clean error.
    ///
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

    /// Cap how many loop iterations this context will execute
    /// (CHECK-NONTERMINATION). `None` removes the cap, which is what
    /// every path except `--check` wants.
    pub fn set_step_budget(&mut self, budget: Option<u64>) {
        self.loop_steps = 0;
        self.max_loop_steps = budget;
    }

    /// Account for one loop back-edge, failing the run once the
    /// budget set by [`Self::set_step_budget`] is spent.
    ///
    /// Called from the two places a toylang run can jump backwards:
    /// `handle_while_loop` and `execute_for_loop`. With no budget set
    /// this is a load, a compare, and nothing else.
    pub(super) fn charge_loop_step(&mut self) -> Result<(), InterpreterError> {
        let Some(max) = self.max_loop_steps else {
            return Ok(());
        };
        self.loop_steps += 1;
        if self.loop_steps > max {
            return Err(InterpreterError::StepBudgetExceeded { steps: max });
        }
        Ok(())
    }

    pub fn register_enum(&mut self, name: DefaultSymbol, entry: EnumRegistryEntry) {
        Rc::make_mut(&mut self.enum_definitions).insert(name, entry);
    }

    // -------------------------------------------------------------
    // POINTER P1: generic type-argument scopes for `sizeof::<T>()`.
    // -------------------------------------------------------------

    /// Push a generic type-argument scope for the duration of one
    /// call body. Paired with [`Self::pop_generic_type_scope`] on
    /// every exit path of the call site that pushed it.
    pub(super) fn push_generic_type_scope(&mut self, scope: HashMap<DefaultSymbol, TypeDecl>) {
        self.generic_type_scopes.push(scope);
    }

    pub(super) fn pop_generic_type_scope(&mut self) {
        self.generic_type_scopes.pop();
    }

    /// Merged view of every active scope, innermost winning. The
    /// per-frame maps are small (one entry per generic parameter),
    /// so a fresh merge per `sizeof::<T>()` is cheaper than keeping
    /// the merged form incrementally in sync.
    pub(super) fn merged_generic_scope(&self) -> HashMap<DefaultSymbol, TypeDecl> {
        let mut merged = HashMap::new();
        for scope in &self.generic_type_scopes {
            for (param, ty) in scope {
                merged.insert(*param, ty.clone());
            }
        }
        merged
    }

    /// The generic-parameter scope a *receiver* determines: the
    /// declaring struct / enum's `generic_params` zipped with the
    /// runtime `type_args` the value carries. A `Ptr<u64>` receiver
    /// therefore speaks for `T -> UInt64` inside every method body
    /// `impl<T> Ptr<T>` runs on it.
    pub(super) fn receiver_generic_scope(&self, obj: &Object) -> HashMap<DefaultSymbol, TypeDecl> {
        let (type_args, generic_params) = match obj {
            Object::Struct { type_name, type_args, .. } => (
                type_args,
                self.struct_definitions.get(type_name).map(|e| e.generic_params.clone()),
            ),
            Object::EnumVariant { enum_name, type_args, .. } => (
                type_args,
                self.enum_definitions.get(enum_name).map(|e| e.generic_params.clone()),
            ),
            _ => return HashMap::new(),
        };
        let Some(generic_params) = generic_params else {
            return HashMap::new();
        };
        generic_params
            .into_iter()
            .zip(type_args.iter().cloned())
            .collect()
    }

    /// The generic-parameter scope a `val` / `var` annotation
    /// determines for an associated call on `owner`:
    /// `val h: Holder<u64> = Holder::make(n)` puts `T` only in the
    /// return type, so the annotation is what the callee's body
    /// needs. Mirrors the compiled lanes' let-lowering, which
    /// resolves the same call's instantiation from the same
    /// annotation.
    pub(super) fn annotation_generic_scope(
        &self,
        owner: DefaultSymbol,
        annotation: Option<&TypeDecl>,
    ) -> HashMap<DefaultSymbol, TypeDecl> {
        let Some(anno) = annotation else {
            return HashMap::new();
        };
        // The annotation names the owner at its top level in the
        // common case (`val p: Ptr<u64> = Ptr::alloc(2u64)`), but it
        // also reaches the binding wrapped in another type
        // (`val p: Option<Ptr<u64>> = Ptr::try_from_raw(raw)`). Look
        // inside before giving up, or the callee builds its value with
        // no type arguments and a later `__builtin_sizeof::<T>()` in a
        // method body fails on an unbound `T`
        // (GENERIC-IN-ENUM-PAYLOAD).
        let nested;
        let anno_args: &[TypeDecl] = match anno {
            TypeDecl::Struct(name, args) | TypeDecl::Enum(name, args) if *name == owner => args,
            other => match other.nested_type_args(owner) {
                Some(args) => {
                    nested = args;
                    &nested
                }
                None => return HashMap::new(),
            },
        };
        // The owner is normally a struct (`impl<T> Ptr<T>`); an
        // `impl ... for <Enum>` with a `Self`-returning associated
        // fn takes the same path through the enum table.
        let generic_params = if let Some(entry) = self.struct_definitions.get(&owner) {
            &entry.generic_params
        } else if let Some(entry) = self.enum_definitions.get(&owner) {
            &entry.generic_params
        } else {
            return HashMap::new();
        };
        // The annotation is written in the *caller's* vocabulary, so
        // its arguments can themselves be generic parameters:
        // `val w: Option<Ptr<T>> = Ptr::try_from_raw(p)` inside
        // `impl<T> Span<T>` says "Ptr's T is my T". Binding that
        // literally would shadow the caller's own binding for `T`
        // with `T` itself, and the value built inside would carry no
        // usable type argument. Resolve through the active scope
        // first; a parameter the caller cannot name either stays as
        // it is, and the construction sites treat it as unknown.
        let active = self.merged_generic_scope();
        generic_params
            .iter()
            .copied()
            .zip(anno_args.iter().map(|a| match a {
                TypeDecl::Generic(g) | TypeDecl::Identifier(g) => {
                    active.get(g).cloned().unwrap_or_else(|| a.clone())
                }
                _ => a.clone(),
            }))
            .collect()
    }

    /// Fill the generic parameters a call's own evidence left
    /// unbound from the *caller's* active scope (`fn outer<T>() {
    /// inner() }` — the callee's `T` is the caller's `T`, exactly
    /// what the compiled lanes' monomorph substitution does).
    /// Parameters the caller also cannot name stay unbound; reading
    /// `sizeof::<T>()` with one of those is a runtime error, not a
    /// silent guess.
    pub(super) fn fill_scope_from_caller(
        &mut self,
        scope: &mut HashMap<DefaultSymbol, TypeDecl>,
        generic_params: &[DefaultSymbol],
    ) {
        if generic_params.iter().all(|p| scope.contains_key(p)) {
            return;
        }
        let caller = self.merged_generic_scope();
        for p in generic_params {
            if let Some(ty) = caller.get(p) {
                scope.entry(*p).or_insert_with(|| ty.clone());
            }
        }
    }

    pub fn register_struct(
        &mut self,
        name: DefaultSymbol,
        entry: StructRegistryEntry,
    ) {
        Rc::make_mut(&mut self.struct_definitions).insert(name, entry);
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
        let specs = Rc::make_mut(&mut self.method_registry)
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
    ///
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
        // CONCRETE-IMPL-Phase-2c: a wildcard spec — empty args or
        // all-symbolic args (`impl<T> C<T>` registers
        // `[Generic(T)]`) — matches any receiver. This is the tier
        // that lets `impl C<u8>` override the generic impl for `u8`
        // receivers while every other receiver falls back to it.
        if let Some(spec) = specs
            .iter()
            .find(|s| frontend::type_checker::is_wildcard_spec(&s.target_type_args))
        {
            return Some(spec.method.clone());
        }
        if specs.len() == 1 {
            return Some(specs[0].method.clone());
        }
        None
    }

    // -------------------------------------------------------------
    // Phase 5 (汎用 RAII): scope-bound auto-drop.
    // -------------------------------------------------------------

    /// Push a fresh auto-drop scope on entry to a `{ ... }` block.
    /// Mirrors `Environment::enter_block` — every block that
    /// introduces bindings gets a paired drop scope so its
    /// `Drop`-impling values can be cleaned up at exit.
    pub(super) fn enter_drop_scope(&mut self) {
        self.drop_scopes.push(Vec::new());
    }

    /// Pop the current auto-drop scope and run each binding's drop
    /// glue in reverse declaration order (LIFO — last-bound drops
    /// first). Errors from any drop call abort the unwind and
    /// surface to the caller. Called on every successful exit
    /// path of a block (linear / `Return` / `Break` / `Continue`);
    /// errors from the body itself skip the drop calls (the
    /// process is going to die anyway, similar to a panic in
    /// Rust where unwind = no second pass on `Drop`).
    pub(super) fn run_and_pop_drop_scope(&mut self) -> Result<(), InterpreterError> {
        let scope = self.drop_scopes.pop().unwrap_or_default();
        for entry in scope.into_iter().rev() {
            self.glue_drop(&entry)?;
        }
        Ok(())
    }

    /// Drop without running — used by error-path bailouts where
    /// we want to discard the pending drops without executing
    /// them (the process is exiting via panic / a parser-side
    /// error / an IR codegen error, etc.).
    pub(super) fn discard_drop_scope(&mut self) {
        self.drop_scopes.pop();
    }

    /// Inspect a freshly bound value and, if it (transitively)
    /// contains a type with an `impl Drop`, append a matching
    /// `DropEntry` to the current top scope. The Rc captured here
    /// is the same one the binding holds, so mutations through the
    /// binding (`s.field = ...`) are visible inside the
    /// synthesized `drop(&mut self)` call.
    ///
    /// DROP-GLUE: this registers far more than the old direct
    /// membership check — an enum carrying a `Box` payload, a
    /// struct holding a `Box` field, a `Vec<Box<T>>`: anything
    /// whose death can free something. The probe is value-driven
    /// (the runtime shape is the only reliable source) and
    /// iterative so a deep value cannot overflow the host stack.
    pub(super) fn register_drop_if_needed(
        &mut self,
        stmt_ref: frontend::ast::StmtRef,
        name: DefaultSymbol,
        value: &crate::value::Value,
    ) {
        if self.drop_trait_structs.is_empty() {
            return;
        }
        // BOX-T: a binding that handed its value to something outliving
        // it must not free the resource — the receiver owns it now.
        if self.transferred_bindings.contains(&stmt_ref) {
            return;
        }
        let rc = match value {
            crate::value::Value::Heap(rc) => rc.clone(),
            _ => return, // Primitives have no Drop impl by definition.
        };
        if !self.value_contains_drop(&rc) {
            return;
        }
        if let Some(scope) = self.drop_scopes.last_mut() {
            scope.push(DropEntry { name, value: rc });
        }
    }

    /// Whether the value (transitively) contains a type with a
    /// `Drop` impl. Walks the runtime shape: struct fields, enum
    /// payloads, tuple / array elements, and — for the stdlib
    /// containers `Box` / `Vec` — the heap slots they own (the
    /// typed-slot map holds the exact Rc, so the walk reaches
    /// boxed values the field walk cannot see). Iterative, with a
    /// visited set, so deep and shared values are both safe.
    pub(super) fn value_contains_drop(&self, value: &RcObject) -> bool {
        let mut work: Vec<RcObject> = vec![value.clone()];
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        while let Some(v) = work.pop() {
            let ptr = Rc::as_ptr(&v) as usize;
            if !seen.insert(ptr) {
                continue;
            }
            let obj = v.borrow();
            match &*obj {
                Object::Struct { type_name, .. }
                    if self.drop_trait_structs.contains(type_name) =>
                {
                    return true;
                }
                Object::Struct { type_name, fields, .. } => {
                    if self.struct_has_name(*type_name, "Box") {
                        let addr = self
                            .string_interner
                            .get("data")
                            .and_then(|s| Self::struct_pointer_field(fields, s));
                        if let Some(addr) = addr {
                            if let Some(inner) = self.heap_manager.borrow().typed_read(addr, 0) {
                                work.push(inner);
                            }
                        }
                    } else if self.struct_has_name(*type_name, "Vec") {
                        let addr = self
                            .string_interner
                            .get("data")
                            .and_then(|s| Self::struct_pointer_field(fields, s));
                        let len = self
                            .string_interner
                            .get("len")
                            .and_then(|s| Self::struct_uint_field(fields, s));
                        let elem_size = self
                            .string_interner
                            .get("elem_size")
                            .and_then(|s| Self::struct_uint_field(fields, s));
                        if let (Some(addr), Some(len), Some(elem_size)) = (addr, len, elem_size) {
                            let mut i = 0u64;
                            while i < len {
                                if let Some(e) = self
                                    .heap_manager
                                    .borrow()
                                    .typed_read(addr, (i * elem_size) as usize)
                                {
                                    work.push(e);
                                }
                                i += 1;
                            }
                        }
                    } else {
                        work.extend(fields.values().cloned());
                    }
                }
                Object::EnumVariant { values, .. } => work.extend(values.iter().cloned()),
                Object::Tuple(elems) | Object::Array(elems) => work.extend(elems.iter().cloned()),
                _ => {}
            }
        }
        false
    }

    /// The recursive drop of a binding whose value owns resources
    /// (DROP-GLUE). Visits the value's owned sub-values — for the
    /// stdlib containers `Box` / `Vec` the heap slots themselves —
    /// and then runs the type's user `drop()` body, which for the
    /// containers frees the storage the contents lived in.
    ///
    /// Iterative (an explicit worklist, not recursion): a long
    /// boxed linked list must not exhaust the host stack. `free`
    /// is idempotent on every backend, so a value reachable from
    /// two bindings (aliasing, a `get()` copy, a shared boxed
    /// node) is freed once and later visits are no-ops.
    pub(super) fn glue_drop(&mut self, entry: &DropEntry) -> Result<(), InterpreterError> {
        enum Phase {
            Contents,
            UserDrop,
        }
        let mut work: Vec<(Phase, RcObject)> = vec![(Phase::Contents, entry.value.clone())];
        while let Some((phase, v)) = work.pop() {
            if matches!(phase, Phase::UserDrop) {
                self.invoke_drop(&v)?;
                continue;
            }
            let obj = v.borrow();
            match &*obj {
                // Containers: the user drop frees the storage, so
                // the contents must be glued first.
                Object::Struct { type_name, fields, .. }
                    if self.drop_trait_structs.contains(type_name)
                        && self.struct_has_name(*type_name, "Box") =>
                {
                    let addr = self
                        .string_interner
                        .get("data")
                        .and_then(|s| Self::struct_pointer_field(fields, s));
                    if let Some(addr) = addr {
                        if let Some(inner) = self.heap_manager.borrow().typed_read(addr, 0) {
                            work.push((Phase::Contents, inner));
                        }
                    }
                    work.push((Phase::UserDrop, v.clone()));
                }
                Object::Struct { type_name, fields, .. }
                    if self.drop_trait_structs.contains(type_name)
                        && self.struct_has_name(*type_name, "Vec") =>
                {
                    let addr = self
                        .string_interner
                        .get("data")
                        .and_then(|s| Self::struct_pointer_field(fields, s));
                    let len = self
                        .string_interner
                        .get("len")
                        .and_then(|s| Self::struct_uint_field(fields, s));
                    let elem_size = self
                        .string_interner
                        .get("elem_size")
                        .and_then(|s| Self::struct_uint_field(fields, s));
                    if let (Some(addr), Some(len), Some(elem_size)) = (addr, len, elem_size) {
                        let mut i = 0u64;
                        while i < len {
                            if let Some(e) = self
                                .heap_manager
                                .borrow()
                                .typed_read(addr, (i * elem_size) as usize)
                            {
                                work.push((Phase::Contents, e));
                            }
                            i += 1;
                        }
                    }
                    work.push((Phase::UserDrop, v.clone()));
                }
                // A user Drop impl runs first (Rust order — the body
                // may read its fields), then the fields are glued.
                Object::Struct { type_name, fields, .. }
                    if self.drop_trait_structs.contains(type_name) =>
                {
                    work.extend(fields.values().cloned().map(|f| (Phase::Contents, f)));
                    work.push((Phase::UserDrop, v.clone()));
                }
                Object::Struct { fields, .. } => {
                    work.extend(fields.values().cloned().map(|f| (Phase::Contents, f)));
                }
                Object::EnumVariant { values, .. } => {
                    work.extend(values.iter().cloned().map(|p| (Phase::Contents, p)));
                }
                Object::Tuple(elems) | Object::Array(elems) => {
                    work.extend(elems.iter().cloned().map(|e| (Phase::Contents, e)));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Extract the `ptr`-typed field of a struct value, by name.
    fn struct_pointer_field(
        fields: &std::collections::HashMap<DefaultSymbol, RcObject>,
        sym: DefaultSymbol,
    ) -> Option<usize> {
        fields
            .get(&sym)
            .and_then(|v| v.borrow().try_unwrap_pointer().ok())
    }

    /// Extract the `u64`-typed field of a struct value, by name.
    fn struct_uint_field(
        fields: &std::collections::HashMap<DefaultSymbol, RcObject>,
        sym: DefaultSymbol,
    ) -> Option<u64> {
        fields
            .get(&sym)
            .and_then(|v| v.borrow().try_unwrap_uint64().ok())
    }

    /// Whether `type_name` is the base name of the stdlib
    /// container `Box` / `Vec` (checked by `which`).
    fn struct_has_name(&self, type_name: DefaultSymbol, which: &str) -> bool {
        self.string_interner.resolve(type_name) == Some(which)
    }

    /// Synthesize the equivalent of `value.drop()` and invoke it
    /// via the regular method-dispatch path. The receiver is
    /// `&mut`, but the interpreter's value model is Rc-shared so
    /// mutations against the cell are visible without any
    /// out-parameter writeback dance. The value's type must have
    /// a registered `drop` method (the glue only emits
    /// `UserDrop` phases for `drop_trait_structs` members).
    pub(super) fn invoke_drop(&mut self, value: &RcObject) -> Result<(), InterpreterError> {
        let struct_sym = match &*value.borrow() {
            Object::Struct { type_name, .. } => *type_name,
            _ => {
                return Err(InterpreterError::InternalError(
                    "auto-drop: drop target is not a struct".to_string(),
                ));
            }
        };
        let drop_sym = self.string_interner.get_or_intern("drop");
        let method = match self.get_method(struct_sym, drop_sym, &[]) {
            Some(m) => m,
            None => {
                // The struct was registered as Drop-impl-bearing
                // at startup but the method went missing — flag
                // it as an internal error rather than silently
                // skipping (which could mask a registry bug).
                let s = self.string_interner.resolve(struct_sym).unwrap_or("?");
                return Err(InterpreterError::InternalError(format!(
                    "auto-drop: no `drop` method registered for struct `{s}`"
                )));
            }
        };
        // call_method takes (method, self_obj, args). No extra args
        // for `Drop::drop`. Result envelope is discarded — drop is
        // unit-returning by convention.
        // Auto-drop is not written anywhere in the source, so the
        // frame carries no call site (DEBUG-OBS D1).
        self.call_method(method, value.clone(), Vec::new(), None)?;
        Ok(())
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
            | EvaluationResult::Break(_)
            | EvaluationResult::Continue(_)
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
            Ok(flow @ $crate::evaluation::EvaluationResult::Break(_)) => return Ok(flow),
            Ok(flow @ $crate::evaluation::EvaluationResult::Continue(_)) => return Ok(flow),
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
            Ok(flow @ $crate::evaluation::EvaluationResult::Break(_)) => return Ok(flow),
            Ok(flow @ $crate::evaluation::EvaluationResult::Continue(_)) => return Ok(flow),
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
        Expr::UInt32(v) | Expr::CharLiteral(v) => Ok(Object::UInt32(*v)),
        Expr::Float64(v) => Ok(Object::Float64(*v)),
        Expr::Float32(v) => Ok(Object::Float32(*v)),
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
