use std::collections::HashMap;
use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::type_decl::TypeDecl;
use crate::source_map::SourceMap;
use crate::type_checker::SourceLocation;
use crate::ast::MemStat;
use super::{StmtRef, ExprRef, StmtPool, ExprPool, LocationPool, Expr};

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct File {
    /// Identity that survives the address being reused.
    ///
    /// The JIT keyed its compiled-`main` cache on the *pointer* of the
    /// `File`, on the reasoning that a freshly parsed program always
    /// misses. It does not: parse a program, drop it, parse another,
    /// and the allocator can hand back the same address — at which
    /// point the second program runs the first one's compiled code.
    /// Harmless when a process runs one program; wrong for anything
    /// that runs several (a benchmark harness, an embedding, the
    /// example sweep in `compiler/tests/example_consistency.rs`, which
    /// is what surfaced it).
    pub id: u64,
    pub node: Node,
    pub package_decl: Option<PackageDecl>,
    pub imports: Vec<ImportDecl>,
    pub function: Vec<Rc<Function>>,
    /// Module origin per function entry (parallel to `function`). For each
    /// `function[i]`, this holds:
    ///   - `None` if the function was authored in the user's source file.
    ///   - `Some(path)` if it came in via integration; `path` is the
    ///     dotted module path (`["std", "math"]` for `core/std/math.t`).
    ///
    /// Used to disambiguate same-name `pub fn`s across modules at the IR
    /// `function_index` level (see compiler todo #193). Empty before
    /// integration; `module_integration` pushes one entry per integrated
    /// function — entries already in `function` at integration time get
    /// `None` retroactively if they don't already have an entry.
    pub function_module_paths: Vec<Option<Vec<DefaultSymbol>>>,
    /// Which module root each function came from, parallel to
    /// `function`. Higher wins.
    ///
    /// BUILD-TOOL B0 gave a later `--core-modules` root the win over
    /// an earlier one for a module *path*; this is the same rule one
    /// level down, for the bare name a call may use without a
    /// qualifier. Without it a package's own `fn parse` is merely a
    /// third candidate against `std::json::parse` and every bare call
    /// becomes `[E0010] ambiguous module path` -- so a private helper
    /// breaks the day the stdlib grows a function of the same name
    /// (BUILD_TOOL.md §1, hole 2). `0` is the stdlib and user-authored
    /// top-level functions do not appear here at all: they win
    /// outright, as they always did.
    pub function_module_ranks: Vec<u32>,
    /// Top-level `const NAME: Type = expr` declarations. Evaluated once
    /// at program startup and bound as immutable globals so any function
    /// body (including `main`) can reference them.
    pub consts: Vec<ConstDecl>,
    /// `test "name" { ... }` blocks (LLM-LOOP P4).
    ///
    /// Each one is lowered to an ordinary zero-argument function pushed
    /// onto `function`, so type checking and every backend handle test
    /// bodies with no special cases; this list only records which of
    /// those functions are tests and what the author called them.
    /// Normal execution never calls them.
    pub tests: Vec<TestCase>,

    /// Declaration statements of bindings whose value was handed to
    /// something that outlives them (BOX-T phase C/D).
    ///
    /// Filled by `type_checker::check_moves` after the bodies are
    /// checked, and read by every backend's auto-drop registration: a
    /// binding in this set must *not* drop, because whatever it was
    /// given to now holds the resource. Keyed by the `val` / `var`
    /// statement rather than by name, since a name means different
    /// values in different scopes.
    ///
    /// Empty until the checker runs, which is the right default — an
    /// empty set is exactly the pre-ownership behaviour.
    pub transferred_bindings: std::collections::HashSet<StmtRef>,
    /// CONCURRENCY A1: the `for` statements written `parallel for`.
    ///
    /// A parallel loop *is* a `Stmt::For` — the modifier says the
    /// iterations may run in any order and in parallel, not that the
    /// loop has a different shape. Recording it beside the tree
    /// rather than as a variant means every pass that does not care
    /// (and most do not) is unchanged, and the sequential lanes are
    /// correct implementations by construction.
    /// The location is the `parallel` keyword's own: a `Stmt::For`
    /// records where the parser *finished* the loop, which is past
    /// the closing brace and no use to a reader.
    pub parallel_loops: std::collections::HashMap<StmtRef, crate::type_checker::SourceLocation>,
    /// MODULE-SYSTEM P3: the full qualifier of every call written with
    /// more than one module segment. See `Parser::call_paths`.
    pub call_paths: std::collections::HashMap<ExprRef, Vec<DefaultSymbol>>,

    pub statement: StmtPool,
    pub expression: ExprPool,
    pub location_pool: LocationPool,
    /// Every file this program was built from (DEBUG-OBS D2).
    ///
    /// Empty as the parser leaves it — a parser is handed text, not a
    /// path. The driver fills in [`FileId::ENTRY`] once it knows what
    /// the user called the file, and `module_integration` adds one
    /// entry per module it copies in. Anything drawing an excerpt
    /// resolves a location's `file` here.
    pub source_map: SourceMap,
}

/// A `test "name" { ... }` block, paired with the synthesized function
/// that holds its body.
#[derive(Debug, PartialEq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TestCase {
    /// What the author wrote between the quotes; used in the report.
    pub name: String,
    /// The generated zero-argument function in `File::function`.
    pub function: DefaultSymbol,
    /// Where the `test` keyword is, so a failure can cite the block.
    pub line: u32,
    /// TEST-TOOL T4: the block is expected to **panic**.
    ///
    /// `test "..." panics { }` inverts the outcome — the block passes
    /// by stopping the program. Written because there was no way at
    /// all to check that a contract holds: VEC-CONTRACTS turned
    /// `Vec`'s bounds into `requires` clauses and nothing could
    /// confirm one ever fires.
    ///
    /// `Some(None)` accepts any panic; `Some(Some(text))` requires the
    /// message to contain `text`, because "something died" is not a
    /// test of *which* contract was broken.
    pub expect_panic: Option<Option<String>>,
    /// Which file the block is in, when it is not the entry.
    ///
    /// TEST-TOOL T0: a module's tests are carried into the program
    /// being run, so `line` alone names a position in the wrong file
    /// -- the report said `main.t:8` for a block in `src/mathx.t`.
    /// `None` means the entry, which the runner already knows the
    /// name of.
    pub file: Option<String>,
}

/// Top-level `const NAME: Type = expression` declaration. The `value`
/// expression lives in the same `ExprPool` as everything else; the
/// interpreter evaluates it once at startup with no parameters in scope.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ConstDecl {
    pub node: Node,
    pub name: DefaultSymbol,
    pub type_decl: TypeDecl,
    pub value: ExprRef,
    pub visibility: Visibility,
}

/// Source of [`File::id`]. Monotonic for the life of the process.
static NEXT_FILE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Allocate a fresh [`File::id`].
pub fn next_file_id() -> u64 {
    NEXT_FILE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl File {
    pub fn get(&self, expr_ref: &ExprRef) -> Option<Expr> {
        self.expression.get(expr_ref)
    }

    pub fn len(&self) -> usize {
        self.expression.len()
    }

    pub fn is_empty(&self) -> bool {
        self.expression.is_empty()
    }
}

#[derive(Debug, PartialEq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Function {
    pub node: Node,
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,  // Generic type parameters like <T, U>
    // Optional bounds for each generic parameter, e.g. `<A: Allocator>`. Present only
    // when the programmer wrote a `: Type` after the parameter name. Lookup by symbol;
    // missing entries mean the parameter is unbounded.
    pub generic_bounds: HashMap<DefaultSymbol, TypeDecl>,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    /// `requires` clauses (preconditions). Each entry is a bool-typed
    /// expression evaluated on function entry; AND-composed.
    pub requires: Vec<ExprRef>,
    /// `ensures` clauses (postconditions). Each entry is a bool-typed
    /// expression evaluated just before return; the special identifier
    /// `result` is in scope and refers to the return value.
    pub ensures: Vec<ExprRef>,
    /// ALLOC-CONTRACT: the expressions written inside `old(...)` in
    /// this function's `ensures` clauses, in the order the parser
    /// met them. Each is evaluated once on entry — after `requires`,
    /// before the body — and bound to the synthetic name `__old_<i>`
    /// that the parser left in the clause's place. A postcondition
    /// can therefore compare a value against the pre-state it had,
    /// which is what makes an allocation contract expressible:
    ///
    /// ```text
    /// ensures __builtin_live_bytes() == old(__builtin_live_bytes())
    /// ```
    ///
    /// Empty for the overwhelming majority of functions, and never
    /// evaluated when postconditions are switched off.
    pub old_exprs: Vec<ExprRef>,
    /// ALLOC-CONTRACT-SUGAR: one entry per `ensures` clause, in the
    /// same order. `Plain` for anything a user wrote by hand.
    pub ensures_kinds: Vec<EnsuresKind>,
    /// NEVER-ALLOCATES: the function was declared `never_allocates`,
    /// so the type checker refuses it if any path from here can reach
    /// `__builtin_heap_alloc` / `__builtin_heap_realloc`.
    ///
    /// The static counterpart to `ensures allocates(0u64)`: that one
    /// measures a call, this one rules the possibility out. On an
    /// `extern fn` it is a *declaration* rather than a check — the
    /// implementation lives outside the language and cannot be walked.
    pub never_allocates: bool,
    /// COMPILE-TIME-EVAL C1: the function was declared `const fn`, so
    /// it may be evaluated at compile time when every argument is a
    /// constant. The type checker refuses the declaration if any path
    /// from here reaches something that cannot run in the compiler
    /// (the allocator, `print`, an `extern fn`, a closure call).
    ///
    /// The spelling is C++'s `constexpr` rather than `consteval`: it
    /// says the function *can* be folded, never that it must be. What
    /// forces the fold is the use site — a `const NAME = f(1u64)`
    /// initialiser, or (later) an array length.
    pub const_fn: bool,
    /// POINTER P6: the function was declared `unsafe fn` — it may
    /// perform raw memory access (`__builtin_ptr_read` /
    /// `__builtin_ptr_write` / the mem-move family) in its own body.
    /// The checker refuses a body that reaches a `RawRead` / `RawWrite`
    /// builtin without the declaration; going through the stdlib's
    /// `Ptr<T>` / `Span<T>` keeps the caller safe. Direct calls only —
    /// calling an `unsafe fn` does not make the caller unsafe.
    pub is_unsafe: bool,
    /// Body block. For `extern fn` declarations this points at a
    /// placeholder `Stmt::Break`; backends look at `is_extern`
    /// before walking the body.
    pub code: StmtRef,
    /// `extern fn name(args) -> T` — declared signature only, with
    /// the implementation provided by the runtime / linker. Each
    /// backend resolves `name` against its own dispatch:
    /// the interpreter consults a Rust-side registry, the JIT looks
    /// up a same-named helper, and the AOT compiler emits an
    /// import that the linker resolves (e.g. against libm). Used
    /// to keep math intrinsics (`sin`, `cos`, ...) out of the
    /// frontend's `BuiltinFunction` enum and inside a stdlib
    /// `.t` file instead.
    pub is_extern: bool,
    /// FFI_PLAN P1: `extern fn ... from "lib" [as "sym"]`. When
    /// present, the symbol name comes from the declaration instead
    /// of the backend's built-in dispatch, and the AOT linker gets
    /// `-l<lib>`. `None` for every non-extern function.
    pub extern_link: Option<ExternLink>,
    pub visibility: Visibility,
}

/// `from "lib" as "sym"` on an `extern fn` declaration (FFI_PLAN 論点 1).
/// `lib` is the linker `-l` name (no `lib` prefix / extension);
/// `symbol` defaults to the function's own name when `as` is absent.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ExternLink {
    pub lib: DefaultSymbol,
    pub symbol: Option<DefaultSymbol>,
}

pub type Parameter = (DefaultSymbol, TypeDecl);
pub type ParameterList = Vec<Parameter>;

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StructField {
    pub name: String,
    pub type_decl: TypeDecl,
    pub visibility: Visibility,
}

impl StructField {
    /// Whether this field came from a tuple struct (`struct Meters(i64)`),
    /// whose fields the parser names by their index -- `"0"`, `"1"`, ...
    ///
    /// A named field can never collide with one: the parser only accepts
    /// an `Identifier` in field position, and identifiers cannot start
    /// with a digit. So the leading byte is a sound discriminator and no
    /// extra flag has to be threaded through the pool, the module
    /// interface, and the AST cache.
    pub fn is_positional(&self) -> bool {
        self.name.as_bytes().first().is_some_and(u8::is_ascii_digit)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ImplBlock {
    pub target_type: String,
    pub methods: Vec<Rc<MethodFunction>>,
    /// Some(name) when this block is `impl <Trait> for <Type>`,
    /// None for inherent `impl <Type>`.
    pub trait_name: Option<DefaultSymbol>,
}

/// ALLOC-CONTRACT-SUGAR: what an `ensures` clause is, when that is
/// more than "a bool expression".
///
/// Carried alongside `ensures` (same length, same order) rather than
/// replacing it, so every reader that only needs the predicate — the
/// type checker, the lowering's contract emitter — is untouched, and
/// only the code that reports a violation looks at the kind.
///
/// Deliberately holds no `ExprRef`: the pieces a diagnostic needs are
/// recoverable from the clause expression itself (which is
/// `counter() <= __old_N + budget` by construction), so module
/// integration has nothing extra to remap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EnsuresKind {
    /// An ordinary predicate. Violation reports the clause index.
    Plain,
    /// An allocation budget written as `allocates(N)` / `retains(N)` /
    /// `allocations(N)`. `old_index` is the entry snapshot's position
    /// in `old_exprs`, which is what turns the two absolute counter
    /// readings into the delta a reader wants to see.
    AllocBudget { stat: MemStat, old_index: usize },
}

/// A method signature appearing in a `trait` declaration. The body is absent;
/// only the contract (parameters, return type, optional `requires` / `ensures`)
/// participates in conformance checking. This intentionally mirrors the
/// non-body portion of `MethodFunction` so registering a trait impl as an
/// inherent method is straightforward.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TraitMethodSignature {
    pub node: Node,
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,
    pub generic_bounds: HashMap<DefaultSymbol, TypeDecl>,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub requires: Vec<ExprRef>,
    pub ensures: Vec<ExprRef>,
    /// ALLOC-CONTRACT: the expressions written inside `old(...)` in
    /// this function's `ensures` clauses, in the order the parser
    /// met them. Each is evaluated once on entry — after `requires`,
    /// before the body — and bound to the synthetic name `__old_<i>`
    /// that the parser left in the clause's place. A postcondition
    /// can therefore compare a value against the pre-state it had,
    /// which is what makes an allocation contract expressible:
    ///
    /// ```text
    /// ensures __builtin_live_bytes() == old(__builtin_live_bytes())
    /// ```
    ///
    /// Empty for the overwhelming majority of functions, and never
    /// evaluated when postconditions are switched off.
    pub old_exprs: Vec<ExprRef>,
    /// ALLOC-CONTRACT-SUGAR: one entry per `ensures` clause, in the
    /// same order. `Plain` for anything a user wrote by hand.
    pub ensures_kinds: Vec<EnsuresKind>,
    /// NEVER-ALLOCATES: the function was declared `never_allocates`,
    /// so the type checker refuses it if any path from here can reach
    /// `__builtin_heap_alloc` / `__builtin_heap_realloc`.
    ///
    /// The static counterpart to `ensures allocates(0u64)`: that one
    /// measures a call, this one rules the possibility out. On an
    /// `extern fn` it is a *declaration* rather than a check — the
    /// implementation lives outside the language and cannot be walked.
    pub never_allocates: bool,
    /// POINTER P6: the signature was declared `unsafe fn`. A trait
    /// signature has no body of its own, so this only travels: a
    /// default body inherits it, and an impl that omits the method
    /// gets a method declared the same way. An impl writing its own
    /// override declares `unsafe` itself — conformance does not
    /// compare the two, because the check is about a body, not a
    /// signature.
    pub is_unsafe: bool,
    pub has_self_param: bool,
    /// `true` when the receiver was written `&mut self` (mutable
    /// reference). Only meaningful when `has_self_param == true`.
    /// Stage 1 of the `&` references work — used by trait
    /// conformance to require matching kinds and by the AOT
    /// codegen to emit a Self-out-parameter writeback.
    pub self_is_mut: bool,
    /// Optional default body. When `Some(stmt_ref)`, an
    /// `impl <Trait> for <T>` that omits this method gets the
    /// default body installed as an inherent method on `T`. When
    /// `None`, every impl must provide the method explicitly.
    /// Inside the body, `Self` and `self` resolve to the impl's
    /// target type at type-check time.
    pub body: Option<StmtRef>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MethodFunction {
    pub node: Node,
    pub name: DefaultSymbol,
    pub generic_params: Vec<DefaultSymbol>,  // Generic type parameters like <T, U>
    // Bounds inherited from the enclosing `impl<A: Allocator>` block plus any
    // method-level bounds. Missing entries mean the parameter is unbounded.
    pub generic_bounds: HashMap<DefaultSymbol, TypeDecl>,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub requires: Vec<ExprRef>,
    pub ensures: Vec<ExprRef>,
    /// ALLOC-CONTRACT: the expressions written inside `old(...)` in
    /// this function's `ensures` clauses, in the order the parser
    /// met them. Each is evaluated once on entry — after `requires`,
    /// before the body — and bound to the synthetic name `__old_<i>`
    /// that the parser left in the clause's place. A postcondition
    /// can therefore compare a value against the pre-state it had,
    /// which is what makes an allocation contract expressible:
    ///
    /// ```text
    /// ensures __builtin_live_bytes() == old(__builtin_live_bytes())
    /// ```
    ///
    /// Empty for the overwhelming majority of functions, and never
    /// evaluated when postconditions are switched off.
    pub old_exprs: Vec<ExprRef>,
    /// ALLOC-CONTRACT-SUGAR: one entry per `ensures` clause, in the
    /// same order. `Plain` for anything a user wrote by hand.
    pub ensures_kinds: Vec<EnsuresKind>,
    /// NEVER-ALLOCATES: the function was declared `never_allocates`,
    /// so the type checker refuses it if any path from here can reach
    /// `__builtin_heap_alloc` / `__builtin_heap_realloc`.
    ///
    /// The static counterpart to `ensures allocates(0u64)`: that one
    /// measures a call, this one rules the possibility out. On an
    /// `extern fn` it is a *declaration* rather than a check — the
    /// implementation lives outside the language and cannot be walked.
    pub never_allocates: bool,
    /// POINTER P6: the method was declared `unsafe fn` — its own body
    /// may reach a raw-memory builtin. Enforced like the free-function
    /// form; see [`Function::is_unsafe`].
    pub is_unsafe: bool,
    pub code: StmtRef,
    pub has_self_param: bool, // true if first parameter is &self
    /// `true` when the receiver was written `&mut self` (mutable
    /// reference). Only meaningful when `has_self_param == true`.
    /// Drives the AOT Self-out-parameter writeback path that lets
    /// `core/std/dict.t::insert` mutations propagate to the caller.
    /// Interpreter ignores this (RefCell already gives reference
    /// semantics on every receiver kind).
    pub self_is_mut: bool,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PackageDecl {
    pub name: Vec<DefaultSymbol>,  // package path components: [math_symbol, basic_symbol]
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ImportDecl {
    pub module_path: Vec<DefaultSymbol>,  // module path: [math_symbol, basic_symbol]
    pub alias: Option<DefaultSymbol>,     // alias from "as" clause
}

#[derive(Debug, PartialEq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Node {
    pub start: usize,
    pub end: usize,
}

impl Node {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn to_source_location(&self, line: u32, column: u32) -> SourceLocation {
        SourceLocation::new(line, column, self.start as u32, self.end as u32)
    }
}

/// AST node with optional source location metadata.
#[derive(Debug, PartialEq, Clone)]
pub struct NodeWithLocation<T> {
    pub node: T,
    pub location: Option<SourceLocation>,
}

impl<T> NodeWithLocation<T> {
    pub fn new(node: T) -> Self {
        Self {
            node,
            location: None,
        }
    }

    pub fn with_location(node: T, location: SourceLocation) -> Self {
        Self {
            node,
            location: Some(location),
        }
    }
}
