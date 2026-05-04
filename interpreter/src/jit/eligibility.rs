//! Walks the AST starting from `main` and collects every function that the
//! JIT can compile. A function is eligible when its signature uses only
//! `i64`/`u64`/`bool`/`Unit` and its body uses only the supported expression
//! and statement kinds (literals, locals, arithmetic, comparison, logical,
//! bitwise, unary, if/elif/else, while, for-range, val/var, assignment to
//! locals, return, calls to other eligible functions). Anything else makes
//! the entire reachable set ineligible — the caller silently falls back to
//! the tree-walking interpreter.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use frontend::ast::{BuiltinFunction, Expr, ExprRef, Function, MethodFunction, Operator, Pattern, Program, Stmt, StmtRef, UnaryOp};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::jit::runtime::HelperKind;

/// How the JIT codegen should lower a call to a particular `extern fn`.
/// `Helper` routes through one of the existing runtime helpers (used
/// for sin/cos/tan/log/log2/exp/pow which cranelift has no native
/// instructions for). `Native*` variants emit the corresponding
/// cranelift instruction inline (sqrt/floor/ceil/abs).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExternDispatch {
    Helper(HelperKind),
    NativeSqrtF64,
    NativeFloorF64,
    NativeCeilF64,
    NativeAbsF64,
    /// `__extern_abs_i64` — `wrapping_abs` for i64, used by the
    /// prelude's `impl Abs for i64`. Lowered to a `select(x < 0,
    /// -x, x)` sequence; for `i64::MIN` the negation wraps and the
    /// result stays at `i64::MIN`, matching the legacy
    /// `BuiltinMethod::I64Abs` semantics.
    NativeAbsI64,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct ExternDispatchEntry {
    pub dispatch: ExternDispatch,
    pub params: &'static [ScalarTy],
    pub ret: ScalarTy,
}

/// Catalogue of extern fn names the JIT knows how to lower.
/// First field is the source-level identifier (matches the interpreter
/// extern registry in `evaluation/extern_math.rs`); the rest is the
/// codegen-side recipe.
const JIT_EXTERN_DISPATCH: &[(&str, ExternDispatchEntry)] = {
    use ExternDispatch::*;
    use ScalarTy::{F64, I64};
    &[
        ("__extern_sin_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::SinF64),  params: &[F64], ret: F64 }),
        ("__extern_cos_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::CosF64),  params: &[F64], ret: F64 }),
        ("__extern_tan_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::TanF64),  params: &[F64], ret: F64 }),
        ("__extern_log_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::LogF64),  params: &[F64], ret: F64 }),
        ("__extern_log2_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::Log2F64), params: &[F64], ret: F64 }),
        ("__extern_exp_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::ExpF64),  params: &[F64], ret: F64 }),
        ("__extern_pow_f64", ExternDispatchEntry { dispatch: Helper(HelperKind::Pow),     params: &[F64, F64], ret: F64 }),
        ("__extern_sqrt_f64", ExternDispatchEntry { dispatch: NativeSqrtF64,  params: &[F64], ret: F64 }),
        ("__extern_floor_f64", ExternDispatchEntry { dispatch: NativeFloorF64, params: &[F64], ret: F64 }),
        ("__extern_ceil_f64", ExternDispatchEntry { dispatch: NativeCeilF64,  params: &[F64], ret: F64 }),
        ("__extern_abs_f64", ExternDispatchEntry { dispatch: NativeAbsF64,    params: &[F64], ret: F64 }),
        ("__extern_abs_i64", ExternDispatchEntry { dispatch: NativeAbsI64,    params: &[I64], ret: I64 }),
        // Phase 1/2 test aliases.
        ("extern_sin", ExternDispatchEntry { dispatch: Helper(HelperKind::SinF64), params: &[F64], ret: F64 }),
        ("extern_cos", ExternDispatchEntry { dispatch: Helper(HelperKind::CosF64), params: &[F64], ret: F64 }),
    ]
};

thread_local! {
    /// Per-thread map from interned extern fn name → dispatch recipe,
    /// installed by `analyze` for the lifetime of one JIT compile.
    /// Threading the map as a regular argument would touch every
    /// `check_expr` recursion site; the call frequency is low enough
    /// that a thread-local is acceptable.
    static EXTERN_DISPATCH_MAP: RefCell<HashMap<DefaultSymbol, ExternDispatchEntry>>
        = RefCell::new(HashMap::new());

    /// Per-thread map from primitive `ScalarTy` → canonical-name
    /// `DefaultSymbol`, populated for those primitives whose
    /// `impl Trait for <PrimitiveType> { ... }` block target name has
    /// been interned (i.e. extension traits exist for them in this
    /// program). Same justification as the extern dispatch map for
    /// using a thread-local — `check_expr` doesn't carry the
    /// interner reference.
    static PRIMITIVE_TARGET_SYMBOLS: RefCell<HashMap<ScalarTy, DefaultSymbol>>
        = RefCell::new(HashMap::new());

    /// Layout map for non-generic, unit-variant-only enums (Phase
    /// JE-1). Same justification as PRIMITIVE_TARGET_SYMBOLS for the
    /// thread-local: `check_expr` already takes a long parameter
    /// list, and threading a per-program `&HashMap` through every
    /// recursive call would cascade into many arms. Set in
    /// `analyze` after `collect_enum_layouts`. Looked up by
    /// `enum_layout_for(name)` from the enum-related arms.
    static ENUM_LAYOUTS: RefCell<HashMap<DefaultSymbol, EnumLayout>>
        = RefCell::new(HashMap::new());
}

/// Install the extern dispatch map for the current thread. Run inside
/// `analyze` so that `check_plain_call` can resolve extern callees by
/// symbol.
fn install_extern_dispatch(interner: &DefaultStringInterner) {
    EXTERN_DISPATCH_MAP.with(|cell| {
        let mut m = cell.borrow_mut();
        m.clear();
        for (name, entry) in JIT_EXTERN_DISPATCH {
            // `interner.get` rather than `get_or_intern`: if the user's
            // program never mentioned the extern, leave it out so the
            // map stays small. Functions only get an `is_extern: true`
            // entry by being declared with the same source-level name,
            // so an unmentioned extern can never trigger a call.
            if let Some(sym) = interner.get(name) {
                m.insert(sym, *entry);
            }
        }
    });
}

/// Look up the dispatch recipe for an extern fn by its interned name.
/// Returns `None` if the symbol isn't in the table — caller falls back
/// to the interpreter dispatch path.
pub(crate) fn jit_extern_dispatch_for(name: DefaultSymbol) -> Option<ExternDispatchEntry> {
    EXTERN_DISPATCH_MAP.with(|cell| cell.borrow().get(&name).copied())
}

/// Install the primitive ScalarTy → canonical-name symbol map for the
/// current thread. Run inside `analyze`. Only inserts entries for
/// primitives whose canonical name has actually been interned — a
/// program with no extension-trait impl on that primitive returns
/// `None` from `primitive_target_sym_for_scalar`, and the eligibility
/// analyzer falls through to its non-extension path.
fn install_primitive_target_symbols(interner: &DefaultStringInterner) {
    PRIMITIVE_TARGET_SYMBOLS.with(|cell| {
        let mut m = cell.borrow_mut();
        m.clear();
        for (sty, name) in [
            (ScalarTy::Bool, "bool"),
            (ScalarTy::I64, "i64"),
            (ScalarTy::U64, "u64"),
            (ScalarTy::F64, "f64"),
            (ScalarTy::Ptr, "ptr"),
        ] {
            if let Some(sym) = interner.get(name) {
                m.insert(sty, sym);
            }
        }
    });
}

/// Look up the canonical-name symbol used as the impl target for
/// extension-trait methods on `sty`. Returns `None` when the program
/// has no impl block for that primitive (canonical name was never
/// interned).
fn primitive_target_sym_for_scalar(sty: ScalarTy) -> Option<DefaultSymbol> {
    PRIMITIVE_TARGET_SYMBOLS.with(|cell| cell.borrow().get(&sty).copied())
}

/// Install the per-program enum layout map for the current thread.
/// Called by `analyze` once `collect_enum_layouts` has run; the
/// thread-local is consulted by `enum_layout_for` from
/// `check_expr`'s enum-related arms.
fn install_enum_layouts(layouts: &HashMap<DefaultSymbol, EnumLayout>) {
    ENUM_LAYOUTS.with(|cell| {
        let mut m = cell.borrow_mut();
        m.clear();
        for (name, layout) in layouts {
            m.insert(*name, layout.clone());
        }
    });
}

/// Look up the JIT-side `EnumLayout` for `name`. Returns `None` when
/// the enum either doesn't exist or was filtered out by
/// `collect_enum_layouts` (generic, has tuple variants, etc.).
fn enum_layout_for(name: DefaultSymbol) -> Option<EnumLayout> {
    ENUM_LAYOUTS.with(|cell| cell.borrow().get(&name).cloned())
}

/// `enum_layout_for` re-exported for the codegen sibling module.
/// Same thread-local lookup; the rename underscores that codegen
/// reads — never writes — the layout map.
pub(crate) fn enum_layout_for_codegen(name: DefaultSymbol) -> Option<EnumLayout> {
    enum_layout_for(name)
}

/// Phase JE-1b: when a `val` / `var` annotation refers to a
/// JIT-eligible enum, return `ScalarTy::U64` (the tag is the entire
/// representation for unit-only enums). The parser emits enum
/// annotations as `TypeDecl::Identifier(name)`; we look the name up
/// in `enum_layouts` and accept any hit. Anything else falls back
/// to `None` so the existing reject path runs.
fn scalar_ty_for_enum_decl(td: &TypeDecl) -> Option<ScalarTy> {
    let name = match td {
        TypeDecl::Identifier(s) => *s,
        TypeDecl::Enum(s, args) if args.is_empty() => *s,
        _ => return None,
    };
    enum_layout_for(name).map(|_| ScalarTy::U64)
}

/// Reverse lookup: when an impl-target symbol corresponds to a
/// primitive (extension-trait impl), return the matching `TypeDecl`
/// so `Self_` resolution in `callable_signature` produces a scalar
/// `TypeDecl` instead of `TypeDecl::Identifier(prim_sym)` (which
/// `resolve_param_ty` rejects).
fn primitive_type_decl_for_target_sym(sym: DefaultSymbol) -> Option<TypeDecl> {
    PRIMITIVE_TARGET_SYMBOLS.with(|cell| {
        cell.borrow().iter().find_map(|(sty, &s)| {
            if s != sym {
                return None;
            }
            Some(match sty {
                ScalarTy::Bool => TypeDecl::Bool,
                ScalarTy::I64 => TypeDecl::Int64,
                ScalarTy::U64 => TypeDecl::UInt64,
                ScalarTy::F64 => TypeDecl::Float64,
                ScalarTy::Ptr => TypeDecl::Ptr,
                _ => return None,
            })
        })
    })
}

/// Records the *first* reason eligibility analysis rejected the program.
/// Subsequent rejections deeper in the recursion are ignored — the user
/// only needs the closest hint to the surface.
fn note(reason: &mut Option<String>, msg: impl FnOnce() -> String) {
    if reason.is_none() {
        *reason = Some(msg());
    }
}

/// JIT-supported scalar types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarTy {
    I64,
    U64,
    F64,
    Bool,
    Unit,
    /// Heap pointer. Internally a u64 / cranelift I64 — distinct from
    /// `U64` for type checking but ABI-compatible.
    Ptr,
    /// Allocator handle. Internally a u64 index into the JIT runtime's
    /// allocator registry; `with allocator = expr { … }` pushes / pops
    /// the corresponding allocator on the active stack.
    Allocator,
    /// NUM-W: narrow integer types. Each maps to a cranelift `I8` /
    /// `I16` / `I32` value type. ABI-level argument extension (sign
    /// for the I-prefixed variants, zero for the U-prefixed) is added
    /// in `make_signature` so calls across the function boundary
    /// match the platform's calling convention.
    I8,
    I16,
    I32,
    U8,
    U16,
    U32,
    /// Bottom type for diverging expressions (currently only `panic`).
    /// Compatible with any other `ScalarTy` in branch unification, since
    /// a diverging branch never produces a value at runtime. No
    /// `TypeDecl` maps to `Never`; it arises only from `panic` returning
    /// from `check_expr`.
    Never,
}

impl ScalarTy {
    /// Branch-type unification used by `if-elif-else`. `Never` acts as a
    /// wildcard so `if cond { panic("...") } else { 5i64 }` types as I64
    /// (the panicking branch never produces a value at runtime, so the
    /// other branch determines the if-expression's value type). Returns
    /// `None` when two concrete types disagree.
    pub fn unify_branch(a: ScalarTy, b: ScalarTy) -> Option<ScalarTy> {
        match (a, b) {
            (ScalarTy::Never, t) | (t, ScalarTy::Never) => Some(t),
            (x, y) if x == y => Some(x),
            _ => None,
        }
    }

    pub fn from_type_decl(td: &TypeDecl) -> Option<Self> {
        match td {
            TypeDecl::Int64 => Some(ScalarTy::I64),
            TypeDecl::UInt64 => Some(ScalarTy::U64),
            TypeDecl::Float64 => Some(ScalarTy::F64),
            TypeDecl::Bool => Some(ScalarTy::Bool),
            TypeDecl::Unit => Some(ScalarTy::Unit),
            TypeDecl::Ptr => Some(ScalarTy::Ptr),
            TypeDecl::Allocator => Some(ScalarTy::Allocator),
            // NUM-W: narrow integer types. Mirror the same shape as
            // U64 / I64 so call sites that construct a `ParamTy::Scalar`
            // for any of them flow through the existing eligibility
            // path.
            TypeDecl::Int8 => Some(ScalarTy::I8),
            TypeDecl::Int16 => Some(ScalarTy::I16),
            TypeDecl::Int32 => Some(ScalarTy::I32),
            TypeDecl::UInt8 => Some(ScalarTy::U8),
            TypeDecl::UInt16 => Some(ScalarTy::U16),
            TypeDecl::UInt32 => Some(ScalarTy::U32),
            _ => None,
        }
    }

    /// `true` for the narrow integer widths (NUM-W). Used at codegen
    /// boundaries that need per-width logic (printer dispatch, ABI
    /// extension, cast lowering).
    pub fn is_narrow_int(self) -> bool {
        matches!(
            self,
            ScalarTy::I8 | ScalarTy::I16 | ScalarTy::I32
                | ScalarTy::U8 | ScalarTy::U16 | ScalarTy::U32
        )
    }

    /// `true` for the signed integer scalar types (i8/i16/i32/i64).
    /// Drives `Sext` vs `Zext` ABI extension and signed/unsigned
    /// cmp predicate selection.
    pub fn is_signed_int(self) -> bool {
        matches!(
            self,
            ScalarTy::I8 | ScalarTy::I16 | ScalarTy::I32 | ScalarTy::I64
        )
    }
}

/// JIT-supported parameter / argument types. A `ParamTy::Struct` expands
/// into one cranelift parameter per scalar field at the ABI level; a
/// `ParamTy::Tuple` likewise expands into one parameter per element.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ParamTy {
    Scalar(ScalarTy),
    /// Struct value, identified by its declared type name. Field layout
    /// is looked up via `EligibleSet::struct_layouts`.
    Struct(DefaultSymbol),
    /// Tuple value with the listed element types. Tuples are
    /// structural, so the element types alone identify the shape; no
    /// separate layout map is needed.
    Tuple(Vec<ScalarTy>),
}

/// Signature of an eligible function in JIT-friendly form.
#[derive(Debug, Clone)]
pub struct FuncSignature {
    pub params: Vec<(DefaultSymbol, ParamTy)>,
    pub ret: ParamTy,
}

/// Identifies *what* gets compiled: either a free function by name, or
/// a struct method by `(struct_name, method_name)`. Methods get a
/// distinct discriminator because the same method name may exist on
/// multiple structs and the cranelift display name needs to disambiguate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MonoTarget {
    Function(DefaultSymbol),
    Method(DefaultSymbol, DefaultSymbol),
}

/// Identifies a single monomorphization. The `Vec<ScalarTy>` is the
/// substitution list ordered by the source's `generic_params` (always
/// empty for non-generic targets in the current iteration; methods
/// don't carry independent generics yet).
pub type MonoKey = (MonoTarget, Vec<ScalarTy>);

/// Source unit a monomorphization compiles from. `Function` and
/// `MethodFunction` share enough of a shape (parameters, return type,
/// generic params, body StmtRef) that we expose a small read-only view
/// in `CallableInfo` so codegen / signature-building can stay generic.
#[derive(Clone)]
pub enum MonomorphSource {
    Function(Rc<Function>),
    Method(Rc<MethodFunction>),
}

impl MonomorphSource {
    pub fn parameter(&self) -> &[(DefaultSymbol, TypeDecl)] {
        match self {
            MonomorphSource::Function(f) => &f.parameter,
            MonomorphSource::Method(m) => &m.parameter,
        }
    }

    pub fn return_type(&self) -> Option<&TypeDecl> {
        match self {
            MonomorphSource::Function(f) => f.return_type.as_ref(),
            MonomorphSource::Method(m) => m.return_type.as_ref(),
        }
    }

    pub fn generic_params(&self) -> &[DefaultSymbol] {
        match self {
            MonomorphSource::Function(f) => &f.generic_params,
            MonomorphSource::Method(m) => &m.generic_params,
        }
    }

    pub fn generic_bounds(
        &self,
    ) -> &std::collections::HashMap<DefaultSymbol, TypeDecl> {
        match self {
            MonomorphSource::Function(f) => &f.generic_bounds,
            MonomorphSource::Method(m) => &m.generic_bounds,
        }
    }

    pub fn code(&self) -> StmtRef {
        match self {
            MonomorphSource::Function(f) => f.code,
            MonomorphSource::Method(m) => m.code,
        }
    }

    /// Whether this source has any DbC clauses (`requires` / `ensures`).
    /// Eligibility uses this to silent-fallback contract-bearing functions
    /// rather than try to lower the predicates into cranelift IR.
    pub fn has_contracts(&self) -> bool {
        match self {
            MonomorphSource::Function(f) => !f.requires.is_empty() || !f.ensures.is_empty(),
            MonomorphSource::Method(m) => !m.requires.is_empty() || !m.ensures.is_empty(),
        }
    }
}

/// Field layout for a JIT-compatible struct: every field must be a JIT
/// scalar type (no nested structs in this iteration). Field names are
/// stored as `DefaultSymbol`s so they can be matched directly against
/// the symbols carried by `Expr::StructLiteral` and `Expr::FieldAccess`.
#[derive(Debug, Clone)]
pub struct StructLayout {
    pub fields: Vec<(DefaultSymbol, ScalarTy)>,
}

impl StructLayout {
    pub fn field(&self, name: DefaultSymbol) -> Option<ScalarTy> {
        self.fields
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, t)| *t)
    }
}

/// Layout for a JIT-compatible enum (Phase JE-1: unit variants only,
/// no payloads, no generics). The variant index in the `Vec` doubles
/// as the cranelift tag value, so eligibility / codegen agree on the
/// dispatch order via the source-declaration order.
///
/// Future phases will widen this:
///   - JE-1b: actual constructor + match codegen (this commit only
///     wires the data structure and uses it for a precise skip
///     diagnostic).
///   - JE-2: tuple-variant payloads (`PayloadSlot::Scalar` per slot,
///     plus per-variant payload-type vec).
///   - JE-3: generic enum monomorphisation (separate layout entries
///     per `(name, type_args)` instantiation).
///   - JE-4: enum-typed function parameters / returns (cranelift
///     boundary expansion).
///
/// `base_name` / `variants` / `variant_tag` are read from JE-1b
/// onward; the `#[allow(dead_code)]` keeps the build clean while
/// only the diagnostic in `enum_layout_for` consumes the layout.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct EnumLayout {
    pub base_name: DefaultSymbol,
    /// Variant names in declaration order. Index = tag value.
    pub variants: Vec<DefaultSymbol>,
}

impl EnumLayout {
    #[allow(dead_code)]
    pub fn variant_tag(&self, name: DefaultSymbol) -> Option<u64> {
        self.variants
            .iter()
            .position(|n| *n == name)
            .map(|i| i as u64)
    }
}

/// Result of eligibility analysis. Each MonoKey corresponds to one
/// cranelift function the runtime will compile.
pub struct EligibleSet {
    /// Each monomorphization key -> its source (free function or
    /// struct method).
    pub monomorphs: HashMap<MonoKey, MonomorphSource>,
    /// Each monomorphization key -> its concrete (substituted) signature.
    pub signatures: HashMap<MonoKey, FuncSignature>,
    /// For every user-defined function call expression, the
    /// monomorphization the JIT should target. Different call sites of
    /// the same generic function with different arg types get different
    /// MonoKeys here.
    pub call_targets: HashMap<ExprRef, MonoKey>,
    /// `__builtin_ptr_read(...)` is type-polymorphic at the language level —
    /// the interpreter picks the return type from the typed-slot store at
    /// runtime. The JIT instead requires the expected scalar at compile
    /// time, so eligibility records the type for every supported PtrRead
    /// position (Val/Var/Assign with a typed identifier on the LHS).
    /// Codegen reads back from this map to pick the right helper.
    /// Generic functions cannot use PtrRead: the same ExprRef would need
    /// distinct types per monomorph.
    pub ptr_read_hints: HashMap<ExprRef, ScalarTy>,
    /// Layout of every struct type the JIT understands. Built in a
    /// pre-pass over top-level `Stmt::StructDecl` declarations.
    pub struct_layouts: HashMap<DefaultSymbol, StructLayout>,
    /// Layout of every enum type the JIT understands. Phase JE-1
    /// only fills this for non-generic, unit-only enums (no payload
    /// variants); anything else is silently omitted and references to
    /// it stay on the interpreter fallback path. Currently only
    /// consulted via the `ENUM_LAYOUTS` thread-local from
    /// `check_expr`'s skip diagnostic; JE-1b will read it directly
    /// for codegen.
    #[allow(dead_code)]
    pub enum_layouts: HashMap<DefaultSymbol, EnumLayout>,
}

/// Per-callsite monomorphization record. `call_expr` identifies the
/// `Expr::Call` / `Expr::MethodCall` AST node so codegen can map back
/// to the right callee FuncRef; `target` and `mono_args` build the
/// MonoKey.
#[derive(Debug, Clone)]
pub(crate) struct MonoCall {
    pub call_expr: ExprRef,
    pub target: MonoTarget,
    pub mono_args: Vec<ScalarTy>,
}

pub fn analyze(
    program: &Program,
    main: &Rc<Function>,
    interner: &DefaultStringInterner,
) -> Result<EligibleSet, String> {
    install_extern_dispatch(interner);
    install_primitive_target_symbols(interner);
    // Phase 5 (汎用 RAII): the JIT codegen doesn't model the
    // user-Drop scope-bound auto-call. When the program has a
    // **user-defined** `impl Drop for ...` (i.e. excluding the
    // stdlib `Arena` / `FixedBuffer` impls, which use a
    // syntactic-sniff path the interpreter / AOT both wire
    // without a Drop trait dispatch), fall back to the tree-
    // walking interpreter so the auto-drop machinery runs.
    // Cheap pre-check before the main eligibility walk.
    if let Some(drop_sym) = interner.get("Drop") {
        let arena_sym = interner.get("Arena");
        let fixed_buffer_sym = interner.get("FixedBuffer");
        for i in 0..program.statement.len() {
            let stmt_ref = frontend::ast::StmtRef(i as u32);
            if let Some(frontend::ast::Stmt::ImplBlock {
                target_type,
                trait_name: Some(t),
                ..
            }) = program.statement.get(&stmt_ref)
            {
                if t == drop_sym
                    && Some(target_type) != arena_sym
                    && Some(target_type) != fixed_buffer_sym
                {
                    return Err(
                        "program has a user-defined `impl Drop for ...` block (JIT delegates to interpreter for auto-drop)"
                            .to_string(),
                    );
                }
            }
        }
    }
    let mut function_map: HashMap<DefaultSymbol, Rc<Function>> = HashMap::new();
    for f in &program.function {
        function_map.insert(f.name, f.clone());
    }
    // method_map: (struct_name, method_name) -> MethodFunction. Built
    // by walking top-level `Stmt::ImplBlock` declarations.
    let method_map = collect_method_map(program);

    // Pre-pass: build layouts for every struct whose fields are all
    // JIT-supported scalars. Anything else (nested struct fields,
    // generic structs, struct with arrays / strings, …) is silently
    // omitted; reads from such types would later reject anyway.
    let struct_layouts = collect_struct_layouts(program, interner);
    // Pre-pass: enum layouts for non-generic, unit-only enums (Phase JE-1).
    let enum_layouts = collect_enum_layouts(program);
    install_enum_layouts(&enum_layouts);

    let mut visited: HashSet<MonoKey> = HashSet::new();
    let mut signatures: HashMap<MonoKey, FuncSignature> = HashMap::new();
    let mut monomorphs: HashMap<MonoKey, MonomorphSource> = HashMap::new();
    let mut call_targets: HashMap<ExprRef, MonoKey> = HashMap::new();
    let mut ptr_read_hints: HashMap<ExprRef, ScalarTy> = HashMap::new();
    // Work item: (source, target, substitution-vec ordered by source.generic_params).
    let mut stack: Vec<(MonomorphSource, MonoTarget, Vec<ScalarTy>)> =
        vec![(MonomorphSource::Function(main.clone()), MonoTarget::Function(main.name), Vec::new())];

    while let Some((source, target, subs_vec)) = stack.pop() {
        let key: MonoKey = (target.clone(), subs_vec.clone());
        if !visited.insert(key.clone()) {
            continue;
        }

        let fname_disp = display_mono(interner, &target, &subs_vec);

        // Generic bound checks (e.g. `<A: Allocator>`) require runtime
        // allocator handles which the JIT can't represent. Keep things
        // simple by rejecting bound generics.
        if !source.generic_bounds().is_empty() {
            return Err(format!(
                "function `{fname_disp}` has generic bounds (not supported in JIT)"
            ));
        }

        // Design-by-Contract clauses (`requires` / `ensures`) are evaluated
        // by the tree-walking interpreter to keep JIT codegen simple. Lowering
        // the predicates to cranelift IR is feasible (they're just bool exprs
        // over locals + `result`) but not required for correctness; silent
        // fallback preserves the contract-violation error message and stack.
        if source.has_contracts() {
            return Err(format!(
                "function `{fname_disp}` has DbC contracts (not supported in JIT)"
            ));
        }

        // The substitution list must agree with the source's generic
        // parameter count.
        if subs_vec.len() != source.generic_params().len() {
            return Err(format!(
                "internal: monomorph for `{fname_disp}` expected {} substitutions, got {}",
                source.generic_params().len(),
                subs_vec.len()
            ));
        }
        let substitutions: HashMap<DefaultSymbol, ScalarTy> = source
            .generic_params()
            .iter()
            .copied()
            .zip(subs_vec.iter().copied())
            .collect();

        let receiver_struct = match &target {
            MonoTarget::Method(struct_name, _) => Some(*struct_name),
            MonoTarget::Function(_) => None,
        };
        let mut sig_reason: Option<String> = None;
        let sig = match callable_signature(
            &source,
            receiver_struct,
            &substitutions,
            &struct_layouts,
            &mut sig_reason,
        ) {
            Some(s) => s,
            None => {
                let detail = sig_reason.unwrap_or_else(|| "unsupported signature".into());
                return Err(format!("function `{fname_disp}`: {detail}"));
            }
        };

        let mut callees: Vec<MonoCall> = Vec::new();
        let mut body_reason: Option<String> = None;
        if !check_callable_body(
            program,
            &source,
            &sig,
            &substitutions,
            &struct_layouts,
            &mut callees,
            &mut ptr_read_hints,
            &mut body_reason,
        ) {
            let detail = body_reason.unwrap_or_else(|| "unsupported feature".into());
            return Err(format!("function `{fname_disp}`: {detail}"));
        }

        signatures.insert(key.clone(), sig);
        monomorphs.insert(key.clone(), source.clone());

        for call in callees {
            // Resolve the callee. Methods come from method_map, free
            // functions from function_map.
            let (callee_source, callee_target) = match call.target {
                MonoTarget::Function(name) => match function_map.get(&name) {
                    Some(f) => (
                        MonomorphSource::Function(f.clone()),
                        MonoTarget::Function(name),
                    ),
                    None => {
                        let cname = interner.resolve(name).unwrap_or("<anon>");
                        return Err(format!(
                            "function `{fname_disp}` calls unknown / non-eligible function `{cname}`"
                        ));
                    }
                },
                MonoTarget::Method(struct_name, method_name) => {
                    match method_map.get(&(struct_name, method_name)) {
                        Some(m) => (
                            MonomorphSource::Method(m.clone()),
                            MonoTarget::Method(struct_name, method_name),
                        ),
                        None => {
                            let mname = interner.resolve(method_name).unwrap_or("<anon>");
                            return Err(format!(
                                "function `{fname_disp}` calls unknown method `{mname}`"
                            ));
                        }
                    }
                }
            };
            let callee_key: MonoKey = (callee_target, call.mono_args);
            call_targets.insert(call.call_expr, callee_key.clone());
            stack.push((callee_source, callee_key.0.clone(), callee_key.1));
        }
    }

    Ok(EligibleSet {
        monomorphs,
        signatures,
        call_targets,
        ptr_read_hints,
        struct_layouts,
        enum_layouts,
    })
}

/// Look up a method on a struct by linear scanning ImplBlock decls.
fn find_method(
    program: &Program,
    struct_name: DefaultSymbol,
    method_name: DefaultSymbol,
) -> Option<Rc<MethodFunction>> {
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(Stmt::ImplBlock { target_type, methods, .. }) = program.statement.get(&stmt_ref) {
            if target_type == struct_name {
                if let Some(m) = methods.iter().find(|m| m.name == method_name) {
                    return Some(m.clone());
                }
            }
        }
    }
    None
}

/// Build a `(struct_name, method_name) -> MethodFunction` map from every
/// top-level `Stmt::ImplBlock` in the program.
fn collect_method_map(
    program: &Program,
) -> HashMap<(DefaultSymbol, DefaultSymbol), Rc<MethodFunction>> {
    let mut out: HashMap<(DefaultSymbol, DefaultSymbol), Rc<MethodFunction>> =
        HashMap::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(Stmt::ImplBlock { target_type, methods, .. }) = program.statement.get(&stmt_ref) {
            for m in &methods {
                out.insert((target_type, m.name), m.clone());
            }
        }
    }
    out
}

fn collect_struct_layouts(
    program: &Program,
    interner: &DefaultStringInterner,
) -> HashMap<DefaultSymbol, StructLayout> {
    let mut out: HashMap<DefaultSymbol, StructLayout> = HashMap::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(Stmt::StructDecl {
            name,
            generic_params,
            fields,
            ..
        }) = program.statement.get(&stmt_ref)
        {
            // Generic structs aren't supported in this iteration — the
            // JIT would need per-monomorph layouts.
            if !generic_params.is_empty() {
                continue;
            }
            let mut scalar_fields: Vec<(DefaultSymbol, ScalarTy)> = Vec::with_capacity(fields.len());
            let mut all_scalar = true;
            for f in &fields {
                match ScalarTy::from_type_decl(&f.type_decl) {
                    Some(t) if t != ScalarTy::Unit => {
                        // Resolving the field name to its symbol via the
                        // interner avoids an extra string lookup at every
                        // FieldAccess site.
                        let sym = interner
                            .get(f.name.as_str())
                            .unwrap_or_else(|| {
                                // Fall back: insert into a clone of the
                                // interner. This shouldn't happen in
                                // practice because the parser interned
                                // every identifier already.
                                let mut tmp = interner.clone();
                                tmp.get_or_intern(f.name.as_str())
                            });
                        scalar_fields.push((sym, t));
                    }
                    _ => {
                        all_scalar = false;
                        break;
                    }
                }
            }
            if all_scalar {
                out.insert(
                    name,
                    StructLayout {
                        fields: scalar_fields,
                    },
                );
            }
        }
    }
    out
}

/// Pre-pass over `Stmt::EnumDecl` declarations: build a layout for
/// every JIT-compatible enum (Phase JE-1: non-generic, unit-only).
/// Anything with a tuple variant or generic param is silently
/// omitted; eligibility checks downstream will reject references to
/// it via the regular "JIT does not yet model enum values" path.
fn collect_enum_layouts(program: &Program) -> HashMap<DefaultSymbol, EnumLayout> {
    let mut out: HashMap<DefaultSymbol, EnumLayout> = HashMap::new();
    for i in 0..program.statement.len() {
        if let Some(Stmt::EnumDecl {
            name,
            generic_params,
            variants,
            ..
        }) = program.statement.get(&StmtRef(i as u32))
        {
            if !generic_params.is_empty() {
                continue;
            }
            let mut variant_names: Vec<DefaultSymbol> = Vec::with_capacity(variants.len());
            let mut all_unit = true;
            for v in &variants {
                if !v.payload_types.is_empty() {
                    all_unit = false;
                    break;
                }
                variant_names.push(v.name);
            }
            if all_unit {
                out.insert(
                    name,
                    EnumLayout {
                        base_name: name,
                        variants: variant_names,
                    },
                );
            }
        }
    }
    out
}

/// Format a monomorphization for diagnostic output, e.g. `id<i64>` or
/// `Point::dist`.
fn display_mono(
    interner: &DefaultStringInterner,
    target: &MonoTarget,
    mono_args: &[ScalarTy],
) -> String {
    let base = match target {
        MonoTarget::Function(s) => interner.resolve(*s).unwrap_or("<anon>").to_string(),
        MonoTarget::Method(struct_sym, method_sym) => format!(
            "{}::{}",
            interner.resolve(*struct_sym).unwrap_or("<anon>"),
            interner.resolve(*method_sym).unwrap_or("<anon>"),
        ),
    };
    if mono_args.is_empty() {
        base
    } else {
        let parts: Vec<String> = mono_args.iter().map(|t| format!("{t:?}")).collect();
        format!("{base}<{}>", parts.join(", "))
    }
}

/// Resolve a TypeDecl to its concrete ScalarTy after applying any active
/// generic substitutions. Returns None if the type cannot be represented
/// in the JIT (or if a referenced generic is unbound in this monomorph).
pub(crate) fn substitute_to_scalar(
    td: &TypeDecl,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
) -> Option<ScalarTy> {
    match td {
        TypeDecl::Generic(g) => substitutions.get(g).copied(),
        _ => ScalarTy::from_type_decl(td),
    }
}

/// Given a callee function and the resolved argument types at a call
/// site, derive the substitution map for the callee's generic params.
/// `caller_subs` is used to resolve `Generic(_)` references that appear
/// in non-generic param positions (e.g. when a generic function calls
/// another with one of its own generics as the arg type — though that
/// path is uncommon in our current scope).
fn infer_substitutions(
    callee: &Function,
    arg_tys: &[ScalarTy],
    caller_subs: &HashMap<DefaultSymbol, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<HashMap<DefaultSymbol, ScalarTy>> {
    if callee.parameter.len() != arg_tys.len() {
        note(reject_reason, || {
            format!(
                "call has {} arg(s), callee expects {}",
                arg_tys.len(),
                callee.parameter.len()
            )
        });
        return None;
    }
    let mut subs: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
    for ((_, param_td), &arg_ty) in callee.parameter.iter().zip(arg_tys.iter()) {
        // Struct / tuple param positions skip generic inference and
        // scalar matching — the caller has already validated that the
        // arg's struct/tuple type lines up with the callee's declared
        // type.
        if matches!(
            param_td,
            TypeDecl::Identifier(_) | TypeDecl::Struct(_, _) | TypeDecl::Tuple(_)
        ) {
            continue;
        }
        match param_td {
            TypeDecl::Generic(g) => {
                if let Some(prev) = subs.insert(*g, arg_ty) {
                    if prev != arg_ty {
                        note(reject_reason, || {
                            format!(
                                "generic parameter bound to conflicting types {prev:?} and {arg_ty:?}"
                            )
                        });
                        return None;
                    }
                }
            }
            other => {
                let resolved = substitute_to_scalar(other, caller_subs);
                match resolved {
                    Some(r) if r == arg_ty => {}
                    _ => {
                        note(reject_reason, || {
                            format!(
                                "callee parameter type {other:?} does not match arg type {arg_ty:?}"
                            )
                        });
                        return None;
                    }
                }
            }
        }
    }
    // Every generic_param must be bound by now.
    for g in &callee.generic_params {
        if !subs.contains_key(g) {
            note(reject_reason, || {
                "could not infer all generic type arguments from call site".to_string()
            });
            return None;
        }
    }
    Some(subs)
}

/// Compute a JIT signature for either a free function or a method.
/// `receiver_struct` is `Some(struct)` when `source` is a method bound
/// to that struct; the function uses it to resolve any `TypeDecl::Self_`
/// references in the parameter list / return type.
fn callable_signature(
    source: &MonomorphSource,
    receiver_struct: Option<DefaultSymbol>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    reject_reason: &mut Option<String>,
) -> Option<FuncSignature> {
    let parameters = source.parameter();
    let mut params = Vec::with_capacity(parameters.len());
    let self_struct = receiver_struct;
    for (_, td) in parameters {
        // Map `Self_` to the receiver's type for methods. For a
        // primitive impl target (extension trait — Step C onward),
        // expand `Self_` directly to the matching primitive
        // `TypeDecl` so `resolve_param_ty` can reduce it to a
        // `ParamTy::Scalar`. Struct receivers fall back to
        // `TypeDecl::Identifier` as before.
        let resolved_td = match (td, self_struct) {
            (TypeDecl::Self_, Some(s)) => primitive_type_decl_for_target_sym(s)
                .unwrap_or_else(|| TypeDecl::Identifier(s)),
            (other, _) => other.clone(),
        };
        let pt = match resolve_param_ty(&resolved_td, substitutions, struct_layouts) {
            Some(p) => p,
            None => {
                note(reject_reason, || {
                    format!("parameter has unsupported type {resolved_td:?}")
                });
                return None;
            }
        };
        if matches!(pt, ParamTy::Scalar(ScalarTy::Unit)) {
            note(reject_reason, || {
                "parameter type Unit is not supported".to_string()
            });
            return None;
        }
        params.push((parameters[params.len()].0, pt));
    }
    // Return type. Scalars and structs are both allowed; struct returns
    // expand into cranelift multi-returns (one cranelift return per
    // field) at the ABI layer.
    let ret = match source.return_type() {
        Some(td) => {
            // Map `Self_` similarly for methods. Primitive impl
            // targets resolve to the matching primitive `TypeDecl`
            // so the return is a `ParamTy::Scalar`.
            let resolved_td = match (td, self_struct) {
                (TypeDecl::Self_, Some(s)) => primitive_type_decl_for_target_sym(s)
                    .unwrap_or_else(|| TypeDecl::Identifier(s)),
                (other, _) => other.clone(),
            };
            match resolve_param_ty(&resolved_td, substitutions, struct_layouts) {
                Some(p) => p,
                None => {
                    note(reject_reason, || {
                        format!("return type {resolved_td:?} is not supported")
                    });
                    return None;
                }
            }
        }
        None => ParamTy::Scalar(ScalarTy::Unit),
    };
    Some(FuncSignature { params, ret })
}

/// Resolve a TypeDecl into a JIT parameter type, considering both scalar
/// substitutions (for generic monomorphs) and known struct layouts.
/// Tuples whose elements all resolve to scalars become `ParamTy::Tuple`.
pub(crate) fn resolve_param_ty(
    td: &TypeDecl,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
) -> Option<ParamTy> {
    if let Some(s) = substitute_to_scalar(td, substitutions) {
        return Some(ParamTy::Scalar(s));
    }
    match td {
        TypeDecl::Identifier(s) | TypeDecl::Struct(s, _)
            if struct_layouts.contains_key(s) =>
        {
            Some(ParamTy::Struct(*s))
        }
        TypeDecl::Tuple(elements) => {
            // Tuples are scalar-only at the JIT layer; any non-scalar
            // element (a nested tuple `((a,b),c)` or a struct element
            // `(Point, i64)`) drops us back to the interpreter. todo
            // #160 tracks lifting this by extending `ParamTy::Tuple`
            // to a tree of element shapes — large enough to defer.
            let mut scalars: Vec<ScalarTy> = Vec::with_capacity(elements.len());
            for e in elements {
                let s = substitute_to_scalar(e, substitutions)?;
                if s == ScalarTy::Unit {
                    return None;
                }
                scalars.push(s);
            }
            if scalars.len() < 2 {
                return None;
            }
            Some(ParamTy::Tuple(scalars))
        }
        _ => None,
    }
}

/// Walks a function or method body to confirm it only uses supported
/// constructs and reports every callee found via `callees`. Returns
/// false on the first unsupported construct.
fn check_callable_body(
    program: &Program,
    source: &MonomorphSource,
    sig: &FuncSignature,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let code = source.code();
    // Generic sources are forbidden from using `__builtin_ptr_read`
    // because the hint table is keyed by ExprRef, which is shared across
    // monomorphs of the same body. Reject early so the diagnostic is
    // clearer than a per-arm rejection deep inside the body.
    if !source.generic_params().is_empty() && body_has_ptr_read(program, &code) {
        note(reject_reason, || {
            "generic functions cannot use __builtin_ptr_read in JIT".to_string()
        });
        return false;
    }
    let mut locals: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
    let mut struct_locals: HashMap<DefaultSymbol, DefaultSymbol> = HashMap::new();
    let mut tuple_locals: HashMap<DefaultSymbol, Vec<ScalarTy>> = HashMap::new();
    for (n, t) in &sig.params {
        match t {
            ParamTy::Scalar(s) => {
                locals.insert(*n, *s);
            }
            ParamTy::Struct(struct_name) => {
                struct_locals.insert(*n, *struct_name);
            }
            ParamTy::Tuple(elements) => {
                tuple_locals.insert(*n, elements.clone());
            }
        }
    }

    // For struct-returning functions, the body's terminal expression
    // must produce a struct value (Identifier of a struct local, or a
    // StructLiteral). check_expr rejects struct literals in arbitrary
    // positions, so we process the leading statements normally and then
    // validate the trailing expression by hand.
    if let ParamTy::Struct(struct_name) = &sig.ret {
        return check_struct_returning_body(
            program,
            &code,
            *struct_name,
            &mut locals,
            &mut struct_locals,
            &mut tuple_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        );
    }
    // Tuple-returning functions follow a similar shape — last expression
    // must produce a tuple value, fields are gathered for multi-return.
    if let ParamTy::Tuple(element_tys) = &sig.ret {
        return check_tuple_returning_body(
            program,
            &code,
            element_tys,
            &mut locals,
            &mut struct_locals,
            &mut tuple_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        );
    }

    check_stmt(
        program,
        &code,
        &mut locals,
        &mut struct_locals,
        &mut tuple_locals,
        substitutions,
        struct_layouts,
        callees,
        ptr_read_hints,
        reject_reason,
    )
}

#[allow(clippy::too_many_arguments)]
fn check_struct_returning_body(
    program: &Program,
    body_stmt_ref: &StmtRef,
    struct_name: DefaultSymbol,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
    tuple_locals: &mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let body_stmt = match program.statement.get(body_stmt_ref) {
        Some(s) => s,
        None => return false,
    };
    let body_expr_ref = match body_stmt {
        Stmt::Expression(e) => e,
        _ => {
            note(reject_reason, || {
                "struct-returning function body must be an expression".to_string()
            });
            return false;
        }
    };
    let body_expr = match program.expression.get(&body_expr_ref) {
        Some(e) => e,
        None => return false,
    };
    let block_stmts = match body_expr {
        Expr::Block(stmts) => stmts,
        _ => {
            note(reject_reason, || {
                "struct-returning function body must be a block".to_string()
            });
            return false;
        }
    };
    if block_stmts.is_empty() {
        note(reject_reason, || {
            "struct-returning function body cannot be empty".to_string()
        });
        return false;
    }
    let (last_ref, leading) = block_stmts.split_last().unwrap();
    for s in leading {
        if !check_stmt(
            program,
            s,
            locals,
            struct_locals,
            tuple_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        ) {
            return false;
        }
    }
    // The trailing statement must produce the declared struct value.
    let last_stmt = match program.statement.get(last_ref) {
        Some(s) => s,
        None => return false,
    };
    let result_expr_ref = match last_stmt {
        Stmt::Expression(e) => e,
        _ => {
            note(reject_reason, || {
                "struct-returning function body must end in an expression".to_string()
            });
            return false;
        }
    };
    let result_expr = match program.expression.get(&result_expr_ref) {
        Some(e) => e,
        None => return false,
    };
    match result_expr {
        Expr::Identifier(name) => {
            match struct_locals.get(&name).copied() {
                Some(s) if s == struct_name => true,
                Some(_) => {
                    note(reject_reason, || {
                        "returned struct local has a different type than declared"
                            .to_string()
                    });
                    false
                }
                None => {
                    note(reject_reason, || {
                        "returned identifier is not a known struct local".to_string()
                    });
                    false
                }
            }
        }
        Expr::StructLiteral(lit_name, _) => {
            if lit_name != struct_name {
                note(reject_reason, || {
                    "returned struct literal does not match declared return type"
                        .to_string()
                });
                return false;
            }
            // Validate fields against the layout. Use a temporary
            // variable name so check_struct_literal_fields can be reused
            // — we don't need to actually keep the struct local around.
            check_struct_literal_fields(
                program,
                &result_expr_ref,
                struct_name,
                locals,
                struct_locals,
                tuple_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            )
        }
        _ => {
            note(reject_reason, || {
                "struct return value must be an identifier or struct literal"
                    .to_string()
            });
            false
        }
    }
}

/// Tuple-returning analog of `check_struct_returning_body`. The body's
/// last expression must be either a TupleLiteral with the declared
/// element types or an Identifier of a tuple local with the same shape.
#[allow(clippy::too_many_arguments)]
fn check_tuple_returning_body(
    program: &Program,
    body_stmt_ref: &StmtRef,
    element_tys: &[ScalarTy],
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
    tuple_locals: &mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let body_stmt = match program.statement.get(body_stmt_ref) {
        Some(s) => s,
        None => return false,
    };
    let body_expr_ref = match body_stmt {
        Stmt::Expression(e) => e,
        _ => {
            note(reject_reason, || {
                "tuple-returning function body must be an expression".to_string()
            });
            return false;
        }
    };
    let body_expr = match program.expression.get(&body_expr_ref) {
        Some(e) => e,
        None => return false,
    };
    let block_stmts = match body_expr {
        Expr::Block(stmts) => stmts,
        _ => {
            note(reject_reason, || {
                "tuple-returning function body must be a block".to_string()
            });
            return false;
        }
    };
    if block_stmts.is_empty() {
        note(reject_reason, || {
            "tuple-returning function body cannot be empty".to_string()
        });
        return false;
    }
    let (last_ref, leading) = block_stmts.split_last().unwrap();
    for s in leading {
        if !check_stmt(
            program,
            s,
            locals,
            struct_locals,
            tuple_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        ) {
            return false;
        }
    }
    let last_stmt = match program.statement.get(last_ref) {
        Some(s) => s,
        None => return false,
    };
    let result_expr_ref = match last_stmt {
        Stmt::Expression(e) => e,
        _ => {
            note(reject_reason, || {
                "tuple-returning function body must end in an expression".to_string()
            });
            return false;
        }
    };
    let result_expr = match program.expression.get(&result_expr_ref) {
        Some(e) => e,
        None => return false,
    };
    match result_expr {
        Expr::Identifier(name) => match tuple_locals.get(&name).cloned() {
            Some(shape) if shape.as_slice() == element_tys => true,
            Some(_) => {
                note(reject_reason, || {
                    "returned tuple local has a different shape than declared".to_string()
                });
                false
            }
            None => {
                note(reject_reason, || {
                    "returned identifier is not a known tuple local".to_string()
                });
                false
            }
        },
        Expr::TupleLiteral(elems) => check_tuple_literal_fields(
            program,
            &elems,
            element_tys,
            locals,
            struct_locals,
            tuple_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        ),
        _ => {
            note(reject_reason, || {
                "tuple return value must be an identifier or tuple literal".to_string()
            });
            false
        }
    }
}

/// Validate every element of a tuple literal against the expected
/// element types. Records callees / ptr_read hints encountered while
/// typing the individual element initializers.
#[allow(clippy::too_many_arguments)]
fn check_tuple_literal_fields(
    program: &Program,
    elements: &[ExprRef],
    expected: &[ScalarTy],
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
    tuple_locals: &mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    if elements.len() != expected.len() {
        note(reject_reason, || {
            format!(
                "tuple literal has {} element(s), expected {}",
                elements.len(),
                expected.len()
            )
        });
        return false;
    }
    for (e, want) in elements.iter().zip(expected.iter()) {
        let actual = match check_expr(
            program,
            e,
            locals,
            struct_locals,
            tuple_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        ) {
            Some(t) => t,
            None => return false,
        };
        if actual != *want {
            note(reject_reason, || {
                format!(
                    "tuple literal element type {actual:?} does not match expected {want:?}"
                )
            });
            return false;
        }
    }
    true
}

/// If `value_ref` is a `TupleLiteral`, derive its element types by
/// type-checking each child expression. Returns the shape only when all
/// elements are JIT scalars.
#[allow(clippy::too_many_arguments)]
fn tuple_literal_target(
    program: &Program,
    value_ref: &ExprRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
    tuple_locals: &mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<Vec<ScalarTy>> {
    let expr = program.expression.get(value_ref)?;
    let elems = match expr {
        Expr::TupleLiteral(es) => es,
        _ => return None,
    };
    if elems.len() < 2 {
        return None;
    }
    let mut shape: Vec<ScalarTy> = Vec::with_capacity(elems.len());
    for e in &elems {
        let t = check_expr(
            program,
            e,
            locals,
            struct_locals,
            tuple_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        )?;
        if t == ScalarTy::Unit {
            note(reject_reason, || {
                "tuple element of Unit type is not supported in JIT".to_string()
            });
            return None;
        }
        shape.push(t);
    }
    Some(shape)
}

/// If `value_ref` is a `Call(callee, args)` whose callee returns a
/// scalar tuple, validate args and record the call site, returning the
/// tuple's element-type vector for the caller to register as a tuple
/// local.
#[allow(clippy::too_many_arguments)]
fn check_tuple_returning_call(
    program: &Program,
    value_ref: &ExprRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
    tuple_locals: &mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<Vec<ScalarTy>> {
    let expr = program.expression.get(value_ref)?;
    let callee_name = match expr {
        Expr::Call(n, _) => n,
        _ => return None,
    };
    let callee = program
        .function
        .iter()
        .find(|f| f.name == callee_name)
        .cloned()?;
    // Only proceed when the callee's return is a tuple of scalars.
    let element_tys = match &callee.return_type {
        Some(td) => match resolve_param_ty(td, substitutions, struct_layouts) {
            Some(ParamTy::Tuple(t)) => t,
            _ => return None,
        },
        None => return None,
    };
    // Reuse the regular Call analysis for argument validation and
    // monomorph recording.
    let saved_callees_len = callees.len();
    let _ = check_expr(
        program,
        value_ref,
        locals,
        struct_locals,
        tuple_locals,
        substitutions,
        struct_layouts,
        callees,
        ptr_read_hints,
        reject_reason,
    );
    if callees.len() == saved_callees_len {
        return None;
    }
    Some(element_tys)
}

/// If the value-position expression is a `StructLiteral` whose struct
/// name has a registered scalar layout, return that struct name. Used to
/// special-case `val p = Point { … }` / `var p = Point { … }`.
fn struct_literal_target(
    program: &Program,
    value_ref: &ExprRef,
    type_decl: Option<&TypeDecl>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    reject_reason: &mut Option<String>,
) -> Option<DefaultSymbol> {
    let expr = program.expression.get(value_ref)?;
    let lit_name = match expr {
        Expr::StructLiteral(name, _) => name,
        _ => return None,
    };
    if !struct_layouts.contains_key(&lit_name) {
        // Distinguish generic struct (todo #159 / T6) from
        // other reject reasons (non-scalar field, etc.) so the
        // diagnostic points users at the right todo entry.
        let is_generic = (0..program.statement.len()).any(|i| {
            if let Some(Stmt::StructDecl { name, generic_params, .. }) =
                program.statement.get(&StmtRef(i as u32))
            {
                name == lit_name && !generic_params.is_empty()
            } else {
                false
            }
        });
        note(reject_reason, || {
            if is_generic {
                "struct literal references a generic struct (JIT does not yet \
                 model generic struct values; see #159)".to_string()
            } else {
                "struct literal references a struct without a JIT-eligible scalar layout".to_string()
            }
        });
        return None;
    }
    // If a type annotation is present, it must agree with the literal's
    // struct name. Unknown is the parser's placeholder for "no annotation"
    // (the type checker leaves it in place for many shapes), so accept it
    // as if it weren't there. Generic struct annotations (`Point<T>`) and
    // unrelated names are rejected.
    if let Some(td) = type_decl {
        match td {
            // The parser leaves Unknown when the user writes
            // `var p = Point { … }` without an annotation, so accept it.
            TypeDecl::Unknown => {}
            TypeDecl::Identifier(s) | TypeDecl::Struct(s, _) if *s == lit_name => {}
            _ => {
                note(reject_reason, || {
                    "struct literal type annotation does not match literal name".to_string()
                });
                return None;
            }
        }
    }
    Some(lit_name)
}

/// If `value_ref` is a `Call(callee, args)` whose callee returns a known
/// struct type, validate each argument against the callee's parameters
/// (Identifier-of-struct-local for struct params; ScalarTy for scalar
/// params), record the monomorphization, and return the resulting
/// struct's name. Caller registers the struct local.
#[allow(clippy::too_many_arguments)]
fn check_struct_returning_call(
    program: &Program,
    value_ref: &ExprRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
    tuple_locals: &mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<DefaultSymbol> {
    let expr = program.expression.get(value_ref)?;
    let (callee_name, args_ref) = match expr {
        Expr::Call(n, a) => (n, a),
        _ => return None,
    };
    let callee = program
        .function
        .iter()
        .find(|f| f.name == callee_name)
        .cloned()?;
    // Only proceed when the callee returns a known struct.
    let ret_struct = match &callee.return_type {
        Some(td) => match td {
            TypeDecl::Identifier(s) | TypeDecl::Struct(s, _)
                if struct_layouts.contains_key(s) =>
            {
                *s
            }
            _ => return None,
        },
        None => return None,
    };
    // Reuse the regular Call analysis by delegating to check_expr; it
    // populates callees/call_targets and validates arguments. The
    // expected return type from check_expr will be None (struct returns
    // aren't representable as ScalarTy), but the side effects we need
    // already happened.
    //
    // We re-run the call's argument validation manually here so the
    // overall eligibility analysis stays in sync. The check_expr's
    // existing Call branch handles struct-typed parameters, generic
    // inference, and call_targets registration.
    let saved_callees_len = callees.len();
    let result = check_expr(
        program,
        value_ref,
        locals,
        struct_locals,
        tuple_locals,
        substitutions,
        struct_layouts,
        callees,
        ptr_read_hints,
        reject_reason,
    );
    // For struct-returning calls, check_expr returns None (since its
    // result type isn't a ScalarTy). That's fine — we only care that
    // the side-effects (call recording, argument validation) succeeded.
    // If check_expr failed before recording the call, treat that as a
    // genuine eligibility failure; otherwise propagate the struct
    // return type.
    if result.is_none() && callees.len() == saved_callees_len {
        return None;
    }
    let _ = args_ref; // suppress unused warning
    Some(ret_struct)
}

/// Validate every field of a struct literal against the registered
/// layout. Records callees / ptr_read hints encountered while typing the
/// individual field initializers.
#[allow(clippy::too_many_arguments)]
fn check_struct_literal_fields(
    program: &Program,
    value_ref: &ExprRef,
    struct_name: DefaultSymbol,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
    tuple_locals: &mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let layout = match struct_layouts.get(&struct_name) {
        Some(l) => l.clone(),
        None => {
            // Distinguish generic vs other reasons so the
            // diagnostic points users at the right todo entry.
            // `collect_struct_layouts` skips structs whose
            // `generic_params` is non-empty (no per-monomorph
            // layout yet — todo #159 / T6); a missing layout
            // for a generic struct should say so.
            let is_generic = (0..program.statement.len()).any(|i| {
                if let Some(Stmt::StructDecl { name, generic_params, .. }) =
                    program.statement.get(&StmtRef(i as u32))
                {
                    name == struct_name && !generic_params.is_empty()
                } else {
                    false
                }
            });
            note(reject_reason, || {
                if is_generic {
                    "JIT does not yet model generic struct values \
                     (would need per-monomorph struct_layouts; see #159)"
                        .to_string()
                } else {
                    "struct layout missing in JIT analysis".to_string()
                }
            });
            return false;
        }
    };
    let expr = match program.expression.get(value_ref) {
        Some(e) => e,
        None => return false,
    };
    let lit_fields = match expr {
        Expr::StructLiteral(_, fields) => fields,
        _ => return false,
    };
    if lit_fields.len() != layout.fields.len() {
        note(reject_reason, || {
            format!(
                "struct literal has {} field(s), layout expects {}",
                lit_fields.len(),
                layout.fields.len()
            )
        });
        return false;
    }
    for (field_sym, field_expr) in &lit_fields {
        let want = match layout.field(*field_sym) {
            Some(t) => t,
            None => {
                note(reject_reason, || "unknown field in struct literal".to_string());
                return false;
            }
        };
        let actual = match check_expr(
            program,
            field_expr,
            locals,
            struct_locals,
            tuple_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        ) {
            Some(t) => t,
            None => return false,
        };
        if actual != want {
            note(reject_reason, || {
                format!("struct literal field type {actual:?} does not match layout {want:?}")
            });
            return false;
        }
    }
    true
}

/// Quick syntactic walk to detect any PtrRead within a function body.
fn body_has_ptr_read(program: &Program, stmt_ref: &StmtRef) -> bool {
    let mut found = false;
    walk_stmt_for_ptr_read(program, stmt_ref, &mut found);
    found
}

/// Phase JE-1b: validate that a `Pattern` is supported by the JIT
/// match codegen for the given scrutinee scalar type. Patterns
/// reduce to one of:
///   - `Wildcard` — accepted for any scrutinee type
///   - `Literal(ExprRef)` — accepted when the literal's scalar type
///     matches the scrutinee
///   - `EnumVariant(enum, variant, [])` — accepted when the enum is
///     in `enum_layouts`, the variant is unit, and the scrutinee is
///     a U64 tag (the JIT representation of unit-only enums)
/// Tuple patterns and named bindings (which require payload
/// extraction) are rejected.
fn check_match_pattern(
    program: &Program,
    pat: &Pattern,
    scrut_ty: ScalarTy,
    reject_reason: &mut Option<String>,
) -> bool {
    match pat {
        Pattern::Wildcard => true,
        Pattern::Literal(eref) => {
            let lit_ty = match program.expression.get(eref) {
                Some(Expr::Int64(_)) => ScalarTy::I64,
                Some(Expr::UInt64(_)) => ScalarTy::U64,
                Some(Expr::True) | Some(Expr::False) => ScalarTy::Bool,
                _ => {
                    note(reject_reason, || {
                        "JIT match: unsupported literal pattern shape".to_string()
                    });
                    return false;
                }
            };
            if lit_ty != scrut_ty {
                note(reject_reason, || {
                    format!(
                        "JIT match: literal pattern type {lit_ty:?} does not match scrutinee {scrut_ty:?}"
                    )
                });
                return false;
            }
            true
        }
        Pattern::EnumVariant(enum_sym, variant_sym, sub_pats) => {
            if !sub_pats.is_empty() {
                note(reject_reason, || {
                    "JIT match: enum variant with payload sub-patterns not yet supported \
                     (JE-2+; see JIT-enum-1)".to_string()
                });
                return false;
            }
            if scrut_ty != ScalarTy::U64 {
                note(reject_reason, || {
                    format!(
                        "JIT match: enum variant pattern but scrutinee is {scrut_ty:?}, expected U64 tag"
                    )
                });
                return false;
            }
            let layout = match enum_layout_for(*enum_sym) {
                Some(l) => l,
                None => {
                    note(reject_reason, || {
                        "JIT match: enum is not JIT-eligible (generic / has payloads; \
                     see JIT-enum-1)".to_string()
                    });
                    return false;
                }
            };
            if layout.variant_tag(*variant_sym).is_none() {
                note(reject_reason, || {
                    "JIT match: variant not declared on this enum".to_string()
                });
                return false;
            }
            true
        }
        Pattern::Tuple(_) | Pattern::Name(_) => {
            note(reject_reason, || {
                "JIT match: tuple / name patterns not yet supported".to_string()
            });
            false
        }
    }
}

/// Returns true if `name` matches any top-level `enum` declaration
/// in the program. Used by the AssociatedFunctionCall reject path so
/// it can distinguish enum constructors (`Option::Some(...)`) from
/// other unsupported associated calls and report a precise reason.
fn enum_decl_lookup_by_name(
    program: &Program,
    name: DefaultSymbol,
) -> Option<()> {
    for i in 0..program.statement.len() {
        if let Some(Stmt::EnumDecl { name: n, .. }) = program.statement.get(&StmtRef(i as u32)) {
            if n == name {
                return Some(());
            }
        }
    }
    None
}

fn walk_stmt_for_ptr_read(program: &Program, stmt_ref: &StmtRef, found: &mut bool) {
    if *found {
        return;
    }
    let Some(stmt) = program.statement.get(stmt_ref) else {
        return;
    };
    match stmt {
        Stmt::Expression(e) => walk_expr_for_ptr_read(program, &e, found),
        Stmt::Val(_, _, e) => walk_expr_for_ptr_read(program, &e, found),
        Stmt::Var(_, _, Some(e)) => walk_expr_for_ptr_read(program, &e, found),
        Stmt::Return(Some(e)) => walk_expr_for_ptr_read(program, &e, found),
        Stmt::For(_, s, e, body) => {
            walk_expr_for_ptr_read(program, &s, found);
            walk_expr_for_ptr_read(program, &e, found);
            walk_expr_for_ptr_read(program, &body, found);
        }
        Stmt::While(c, body) => {
            walk_expr_for_ptr_read(program, &c, found);
            walk_expr_for_ptr_read(program, &body, found);
        }
        _ => {}
    }
}

fn walk_expr_for_ptr_read(program: &Program, expr_ref: &ExprRef, found: &mut bool) {
    if *found {
        return;
    }
    let Some(expr) = program.expression.get(expr_ref) else {
        return;
    };
    match expr {
        Expr::BuiltinCall(BuiltinFunction::PtrRead, _) => *found = true,
        Expr::Block(stmts) => {
            for s in &stmts {
                walk_stmt_for_ptr_read(program, s, found);
            }
        }
        Expr::Binary(_, l, r) | Expr::Assign(l, r) | Expr::Range(l, r) => {
            walk_expr_for_ptr_read(program, &l, found);
            walk_expr_for_ptr_read(program, &r, found);
        }
        Expr::Unary(_, e) | Expr::Cast(e, _) => {
            walk_expr_for_ptr_read(program, &e, found);
        }
        Expr::IfElifElse(c, t, elifs, el) => {
            walk_expr_for_ptr_read(program, &c, found);
            walk_expr_for_ptr_read(program, &t, found);
            for (ec, eb) in &elifs {
                walk_expr_for_ptr_read(program, ec, found);
                walk_expr_for_ptr_read(program, eb, found);
            }
            walk_expr_for_ptr_read(program, &el, found);
        }
        Expr::Call(_, args) => walk_expr_for_ptr_read(program, &args, found),
        Expr::ExprList(es) | Expr::ArrayLiteral(es) | Expr::TupleLiteral(es) => {
            for e in &es {
                walk_expr_for_ptr_read(program, e, found);
            }
        }
        Expr::BuiltinCall(_, args) => {
            for a in &args {
                walk_expr_for_ptr_read(program, a, found);
            }
        }
        _ => {}
    }
}

fn check_stmt(
    program: &Program,
    stmt_ref: &StmtRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
    tuple_locals: &mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let stmt = match program.statement.get(stmt_ref) {
        Some(s) => s,
        None => return false,
    };
    match stmt {
        Stmt::Expression(e) => {
            check_expr(program, &e, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason).is_some()
        }
        Stmt::Val(name, type_decl, value) => {
            // Special-case: a struct-literal RHS registers `name` as a
            // struct local. Field-by-field types are validated against the
            // struct's known layout; everything else falls through to the
            // scalar path.
            if let Some(struct_name) =
                struct_literal_target(program, &value, type_decl.as_ref(), struct_layouts, reject_reason)
            {
                if !check_struct_literal_fields(
                    program,
                    &value,
                    struct_name,
                    locals,
                    struct_locals,
                    tuple_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                ) {
                    return false;
                }
                struct_locals.insert(name, struct_name);
                return true;
            }
            // Special-case: a struct-returning function call also lands as
            // a fresh struct local. Validate the call site (and its args)
            // through the normal Call eligibility path.
            if let Some(struct_name) = check_struct_returning_call(
                program,
                &value,
                locals,
                struct_locals,
                tuple_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            ) {
                struct_locals.insert(name, struct_name);
                return true;
            }
            // Tuple literal RHS — `val pair = (1i64, 2u64)` — registers
            // `name` as a tuple local with the inferred element shape.
            if let Some(shape) = tuple_literal_target(
                program,
                &value,
                locals,
                struct_locals,
                tuple_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            ) {
                tuple_locals.insert(name, shape);
                return true;
            }
            // Tuple-returning call — `val pair = make_pair()`.
            if let Some(shape) = check_tuple_returning_call(
                program,
                &value,
                locals,
                struct_locals,
                tuple_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            ) {
                tuple_locals.insert(name, shape);
                return true;
            }
            // Tuple alias — `val q = pair` where `pair` is already a
            // known tuple local.
            if let Some(Expr::Identifier(rhs_name)) = program.expression.get(&value) {
                if let Some(shape) = tuple_locals.get(&rhs_name).cloned() {
                    tuple_locals.insert(name, shape);
                    return true;
                }
            }
            let declared_hint = type_decl.as_ref().and_then(ScalarTy::from_type_decl);
            // If both the annotation and the RHS are PtrRead-shaped, record
            // the expected return type before recursing so check_expr can
            // accept the otherwise type-polymorphic builtin.
            if let Some(t) = declared_hint {
                register_ptr_read_hint(program, &value, t, ptr_read_hints);
            }
            let val_ty = match check_expr(program, &value, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            let declared = match type_decl {
                // The parser leaves Unknown when the user wrote no
                // annotation; treat it as "infer from rhs".
                Some(TypeDecl::Unknown) | None => val_ty,
                Some(td) => match ScalarTy::from_type_decl(&td) {
                    Some(t) => t,
                    None => match scalar_ty_for_enum_decl(&td) {
                        // Phase JE-1b: a JIT-eligible enum
                        // annotation (`val c: Color = ...`)
                        // resolves to ScalarTy::U64 because the
                        // enum's representation at the JIT layer
                        // is just its tag.
                        Some(t) => t,
                        None => return false,
                    },
                },
            };
            if declared != val_ty {
                return false;
            }
            // Reject Unit and Never RHS: there is no value to bind, and
            // recording `Never` in `locals` would poison subsequent
            // expressions that read `name`. `val x = panic(...)` is the
            // typical Never case — silent-fallback is fine since the
            // expression does the same observable thing in the
            // interpreter.
            if matches!(declared, ScalarTy::Unit | ScalarTy::Never) {
                return false;
            }
            locals.insert(name, declared);
            true
        }
        Stmt::Var(name, type_decl, value) => {
            // Mirror the Val struct-literal special case — `var p = Point { ... }`
            // also registers a struct local.
            if let Some(v) = value {
                if let Some(struct_name) =
                    struct_literal_target(program, &v, type_decl.as_ref(), struct_layouts, reject_reason)
                {
                    if !check_struct_literal_fields(
                        program,
                        &v,
                        struct_name,
                        locals,
                        struct_locals,
                        tuple_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    ) {
                        return false;
                    }
                    struct_locals.insert(name, struct_name);
                    return true;
                }
                if let Some(struct_name) = check_struct_returning_call(
                    program,
                    &v,
                    locals,
                    struct_locals,
                    tuple_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                ) {
                    struct_locals.insert(name, struct_name);
                    return true;
                }
                if let Some(shape) = tuple_literal_target(
                    program,
                    &v,
                    locals,
                    struct_locals,
                    tuple_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                ) {
                    tuple_locals.insert(name, shape);
                    return true;
                }
                if let Some(shape) = check_tuple_returning_call(
                    program,
                    &v,
                    locals,
                    struct_locals,
                    tuple_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                ) {
                    tuple_locals.insert(name, shape);
                    return true;
                }
                if let Some(Expr::Identifier(rhs_name)) = program.expression.get(&v) {
                    if let Some(shape) = tuple_locals.get(&rhs_name).cloned() {
                        tuple_locals.insert(name, shape);
                        return true;
                    }
                }
            }
            let declared = match (type_decl.as_ref(), value) {
                // Treat `Some(Unknown)` like `None` — the parser inserts
                // it when the user wrote no annotation.
                (Some(TypeDecl::Unknown), Some(v)) | (None, Some(v)) => {
                    match check_expr(program, &v, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                        Some(t) => t,
                        None => return false,
                    }
                }
                (Some(td), _) => match ScalarTy::from_type_decl(td) {
                    Some(t) => t,
                    None => return false,
                },
                (None, None) => return false,
            };
            if let Some(v) = value {
                if type_decl.is_some() {
                    register_ptr_read_hint(program, &v, declared, ptr_read_hints);
                }
                let val_ty = match check_expr(program, &v, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                    Some(t) => t,
                    None => return false,
                };
                if val_ty != declared {
                    return false;
                }
            }
            if matches!(declared, ScalarTy::Unit | ScalarTy::Never) {
                return false;
            }
            locals.insert(name, declared);
            true
        }
        Stmt::Return(value) => {
            if let Some(v) = value {
                check_expr(program, &v, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason).is_some()
            } else {
                true
            }
        }
        Stmt::Break | Stmt::Continue => true,
        Stmt::For(var, start, end, block) => {
            let start_ty = match check_expr(program, &start, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            let end_ty = match check_expr(program, &end, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            if start_ty != end_ty {
                return false;
            }
            if !matches!(start_ty, ScalarTy::I64 | ScalarTy::U64) {
                return false;
            }
            let prev = locals.insert(var, start_ty);
            let body_ok =
                check_expr(program, &block, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason).is_some();
            match prev {
                Some(t) => {
                    locals.insert(var, t);
                }
                None => {
                    locals.remove(&var);
                }
            }
            body_ok
        }
        Stmt::While(cond, block) => {
            let cond_ty = match check_expr(program, &cond, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            if cond_ty != ScalarTy::Bool {
                return false;
            }
            check_expr(program, &block, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason).is_some()
        }
        // No struct / impl / enum declarations are tolerated inside an
        // eligible function body. Top-level decls live outside of any
        // function so they don't affect us here.
        Stmt::StructDecl { .. } | Stmt::ImplBlock { .. } | Stmt::EnumDecl { .. } | Stmt::TraitDecl { .. } => false,
        // Type aliases are resolved at parse time; their presence inside
        // a function body (which the parser doesn't actually allow) is
        // a no-op and would not disqualify the body either way.
        Stmt::TypeAlias { .. } => true,
    }
}

/// If `value_ref` is a direct `__builtin_ptr_read(...)` call, register
/// `expected` as the read's return type so check_expr can accept it. The
/// JIT only supports PtrRead in positions where the expected type is
/// statically known (val/var with annotation, assignment to a typed
/// identifier).
fn register_ptr_read_hint(
    program: &Program,
    value_ref: &ExprRef,
    expected: ScalarTy,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
) {
    if let Some(Expr::BuiltinCall(BuiltinFunction::PtrRead, _)) =
        program.expression.get(value_ref)
    {
        ptr_read_hints.insert(*value_ref, expected);
    }
}

/// Returns the type produced by the expression, or `None` if the expression
/// uses an unsupported construct. As a side effect, populates `callees` with
/// names of user-defined functions invoked by this expression and
/// `ptr_read_hints` with PtrRead expected return types where statically
/// derivable from context.
pub(crate) fn check_expr(
    program: &Program,
    expr_ref: &ExprRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
    tuple_locals: &mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<ScalarTy> {
    let expr = program.expression.get(expr_ref)?;
    match expr {
        Expr::Int64(_) => Some(ScalarTy::I64),
        Expr::UInt64(_) => Some(ScalarTy::U64),
        // NUM-W narrow integer literals.
        Expr::Int8(_) => Some(ScalarTy::I8),
        Expr::Int16(_) => Some(ScalarTy::I16),
        Expr::Int32(_) => Some(ScalarTy::I32),
        Expr::UInt8(_) => Some(ScalarTy::U8),
        Expr::UInt16(_) => Some(ScalarTy::U16),
        Expr::UInt32(_) => Some(ScalarTy::U32),
        Expr::Float64(_) => Some(ScalarTy::F64),
        Expr::True | Expr::False => Some(ScalarTy::Bool),
        Expr::Identifier(sym) => locals.get(&sym).copied(),
        // Phase JE-1b: unit-variant constructor (`Color::Red`).
        // Reduce to `ScalarTy::U64` so the rest of eligibility +
        // codegen treats the value as just the tag — that's the
        // entire representation for unit variants. Generic enums
        // and enums with payload variants miss the layout map and
        // fall through to the catch-all reject.
        Expr::QualifiedIdentifier(path)
            if path.len() == 2
                && enum_layout_for(path[0])
                    .and_then(|l| l.variant_tag(path[1]))
                    .is_some() =>
        {
            Some(ScalarTy::U64)
        }
        // Phase JE-1b: `match scrutinee { ... }` for scalar / unit-
        // enum-tag scrutinees. Each arm's body must produce a
        // value-typed result; all arms must agree on the result
        // type. Variant patterns over a JIT-eligible enum reduce
        // to a u64 tag comparison, just like the constructor
        // returns the tag.
        Expr::Match(scrutinee, arms) => {
            let scrut_ty = check_expr(
                program, &scrutinee, locals, struct_locals, tuple_locals,
                substitutions, struct_layouts, callees, ptr_read_hints,
                reject_reason,
            )?;
            // All arms unify to a single type. Walk each arm's
            // pattern (rejecting unsupported shapes) and body.
            let mut result_ty: Option<ScalarTy> = None;
            for arm in &arms {
                if !check_match_pattern(
                    program, &arm.pattern, scrut_ty, reject_reason,
                ) {
                    return None;
                }
                if arm.guard.is_some() {
                    note(reject_reason, || {
                        "JIT match arm guards are not yet supported".to_string()
                    });
                    return None;
                }
                let body_ty = check_expr(
                    program, &arm.body, locals, struct_locals, tuple_locals,
                    substitutions, struct_layouts, callees, ptr_read_hints,
                    reject_reason,
                )?;
                match result_ty {
                    None => result_ty = Some(body_ty),
                    Some(prev) if prev == body_ty => {}
                    Some(prev) => {
                        note(reject_reason, || {
                            format!(
                                "match arms disagree on result type: {prev:?} vs {body_ty:?}"
                            )
                        });
                        return None;
                    }
                }
            }
            result_ty
        }
        Expr::Binary(op, lhs, rhs) => {
            let lt = check_expr(program, &lhs, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            let rt = check_expr(program, &rhs, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            if lt != rt {
                return None;
            }
            // NUM-W: narrow integers (I8/I16/I32/U8/U16/U32) accept
            // the same operator set as their I64/U64 cousins. cranelift's
            // `iadd` / `isub` / `imul` / `udiv` / `sdiv` / `urem` /
            // `srem` are all width-polymorphic on operand type, and
            // `icmp` likewise picks the right width from the operands.
            let int_like = lt.is_narrow_int()
                || matches!(lt, ScalarTy::I64 | ScalarTy::U64);
            match op {
                Operator::IAdd | Operator::ISub | Operator::IMul | Operator::IDiv => {
                    if int_like || lt == ScalarTy::F64 {
                        Some(lt)
                    } else {
                        None
                    }
                }
                Operator::IMod => {
                    // Cranelift exposes srem/urem for ints but no native f64
                    // remainder; reject f64 mod here so codegen never sees it.
                    if int_like {
                        Some(lt)
                    } else {
                        None
                    }
                }
                Operator::EQ | Operator::NE => {
                    if lt == ScalarTy::Unit {
                        None
                    } else {
                        Some(ScalarTy::Bool)
                    }
                }
                Operator::LT | Operator::LE | Operator::GT | Operator::GE => {
                    if int_like || lt == ScalarTy::F64 {
                        Some(ScalarTy::Bool)
                    } else {
                        None
                    }
                }
                Operator::LogicalAnd | Operator::LogicalOr => {
                    if lt == ScalarTy::Bool {
                        Some(ScalarTy::Bool)
                    } else {
                        None
                    }
                }
                Operator::BitwiseAnd | Operator::BitwiseOr | Operator::BitwiseXor => {
                    if int_like || lt == ScalarTy::Bool {
                        Some(lt)
                    } else {
                        None
                    }
                }
                Operator::LeftShift | Operator::RightShift => {
                    if int_like {
                        Some(lt)
                    } else {
                        None
                    }
                }
            }
        }
        Expr::Unary(op, operand) => {
            let t = check_expr(program, &operand, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            match op {
                UnaryOp::BitwiseNot => {
                    if matches!(t, ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Bool) {
                        Some(t)
                    } else {
                        None
                    }
                }
                UnaryOp::LogicalNot => {
                    if t == ScalarTy::Bool {
                        Some(ScalarTy::Bool)
                    } else {
                        None
                    }
                }
                UnaryOp::Negate => {
                    // Negation of u64 is rejected at the type-check phase
                    // already. Allow i64 and f64 (cranelift `fneg`).
                    if matches!(t, ScalarTy::I64 | ScalarTy::F64) {
                        Some(t)
                    } else {
                        None
                    }
                }
                // REF-Stage-2: borrow ops are erased — eligibility
                // simply forwards the operand type, codegen emits
                // the operand value.
                UnaryOp::Borrow | UnaryOp::BorrowMut => Some(t),
            }
        }
        Expr::Block(stmts) => {
            let mut last_ty = ScalarTy::Unit;
            let mut snapshot = locals.clone();
            for s in &stmts {
                let stmt = program.statement.get(s)?;
                if let Stmt::Expression(e) = &stmt {
                    last_ty = check_expr(program, e, &mut snapshot, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                } else {
                    if !check_stmt(program, s, &mut snapshot, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    last_ty = ScalarTy::Unit;
                }
            }
            Some(last_ty)
        }
        Expr::IfElifElse(cond, if_block, elif_pairs, else_block) => {
            let ct = check_expr(program, &cond, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            if ct != ScalarTy::Bool {
                return None;
            }
            // Unify each branch's type via `ScalarTy::unify_branch`, which
            // treats `Never` (panic / divergence) as a wildcard — so
            // `if cond { panic("...") } else { 5i64 }` types as I64.
            let then_ty = check_expr(program, &if_block, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            let mut unified = then_ty;
            for (ec, eb) in &elif_pairs {
                let et = check_expr(program, ec, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                if et != ScalarTy::Bool {
                    return None;
                }
                let bt = check_expr(program, eb, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                unified = ScalarTy::unify_branch(unified, bt)?;
            }
            let else_ty = check_expr(program, &else_block, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            ScalarTy::unify_branch(unified, else_ty)
        }
        Expr::Assign(lhs, rhs) => {
            // Two assignment shapes are supported:
            //   1) `name = value` for a previously declared scalar local
            //   2) `name.field = value` for a struct local's field
            let lhs_expr = program.expression.get(&lhs)?;
            match lhs_expr {
                Expr::Identifier(name) => {
                    let lhs_ty = locals.get(&name).copied()?;
                    let rhs_ty = check_expr(
                        program,
                        &rhs,
                        locals,
                        struct_locals,
                        tuple_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    if rhs_ty != lhs_ty {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                Expr::FieldAccess(receiver, field_name) => {
                    let receiver_expr = program.expression.get(&receiver)?;
                    let recv_name = match receiver_expr {
                        Expr::Identifier(s) => s,
                        _ => {
                            note(reject_reason, || {
                                "field-assign receiver must be a struct local".to_string()
                            });
                            return None;
                        }
                    };
                    let struct_name = match struct_locals.get(&recv_name).copied() {
                        Some(s) => s,
                        None => {
                            note(reject_reason, || {
                                "field-assign target is not a struct local".to_string()
                            });
                            return None;
                        }
                    };
                    let field_ty = struct_layouts
                        .get(&struct_name)
                        .and_then(|l| l.field(field_name))?;
                    let rhs_ty = check_expr(
                        program,
                        &rhs,
                        locals,
                        struct_locals,
                        tuple_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    if rhs_ty != field_ty {
                        note(reject_reason, || {
                            format!(
                                "field assign rhs type {rhs_ty:?} does not match field type {field_ty:?}"
                            )
                        });
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                _ => {
                    note(reject_reason, || {
                        "assignment target must be an identifier or struct field".to_string()
                    });
                    None
                }
            }
        }
        Expr::AssociatedFunctionCall(struct_name, function_name, args) => {
            // Module-qualified call (`math::add(args)`): when the
            // qualifier doesn't refer to a struct / enum but the
            // function name lives in the (post-import) flat function
            // table, treat it as a plain `Call(function_name, args)`
            // and reuse the same eligibility logic. Real associated
            // function calls (`Container::new(args)` with `Container`
            // a struct) keep the unsupported reject path because the
            // JIT doesn't lower instance methods yet.
            if struct_layouts.contains_key(&struct_name)
                || !program.function.iter().any(|f| f.name == function_name)
            {
                // Differentiate the common "enum constructor" case
                // (`Option::Some(...)`, `Result::Err(...)`, etc.)
                // from the generic struct-associated-function reject
                // so the verbose JIT log points at the actual blocker.
                // Enum values aren't represented in the JIT yet —
                // adding them would mean a `ParamTy::Enum`,
                // `enum_locals` map, tag-dispatch codegen, etc.
                // (essentially the AOT compiler's enum phases). The
                // interpreter handles the call correctly via
                // fallback; this just makes the reason precise.
                let is_enum_qualifier = enum_decl_lookup_by_name(program, struct_name).is_some();
                // Phase JE-1a: a JIT-eligible enum (non-generic,
                // unit-variant-only) shows up in `enum_layouts`. The
                // architecture for tag-based dispatch is in place
                // (EnumLayout + ENUM_LAYOUTS thread-local), but
                // constructor / match codegen hasn't landed yet —
                // use a precise "infrastructure ready, codegen
                // pending" message so a later JE-1b commit knows
                // which programs to enable.
                note(reject_reason, || {
                    if is_enum_qualifier {
                        if enum_layout_for(struct_name).is_some() {
                            "JIT enum support pending: unit-variant constructor codegen \
                             (Phase JE-1b will lower this via the existing tag layout)"
                                .to_string()
                        } else {
                            "JIT does not yet model enum values \
                             (constructors / match / methods; see JIT-enum-1)"
                                .to_string()
                        }
                    } else {
                        "uses unsupported expression associated function call".to_string()
                    }
                });
                return None;
            }
            return check_plain_call(
                program, expr_ref, function_name, &args, locals, struct_locals,
                tuple_locals, substitutions, struct_layouts, callees,
                ptr_read_hints, reject_reason,
            );
        }
        Expr::Call(name, args_ref) => {
            let args_expr = program.expression.get(&args_ref)?;
            let arg_list = match args_expr {
                Expr::ExprList(v) => v,
                _ => return None,
            };

            check_plain_call(
                program, expr_ref, name, &arg_list, locals, struct_locals,
                tuple_locals, substitutions, struct_layouts, callees,
                ptr_read_hints, reject_reason,
            )
        }
        Expr::BuiltinCall(func, args) => {
            // Type-check each argument against an expected ScalarTy.
            let check_args = |expected: &[ScalarTy],
                              args: &Vec<ExprRef>,
                              locals: &mut HashMap<DefaultSymbol, ScalarTy>,
                              struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
    tuple_locals: &mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
                              callees: &mut Vec<MonoCall>,
                              ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
                              reject_reason: &mut Option<String>|
             -> bool {
                if args.len() != expected.len() {
                    note(reject_reason, || {
                        format!(
                            "builtin called with {} arg(s), expected {}",
                            args.len(),
                            expected.len()
                        )
                    });
                    return false;
                }
                for (a, want) in args.iter().zip(expected.iter()) {
                    match check_expr(
                        program,
                        a,
                        locals,
                        struct_locals,
                        tuple_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    ) {
                        Some(t) if t == *want => {}
                        _ => return false,
                    }
                }
                true
            };
            match func {
                BuiltinFunction::Panic => {
                    // `panic("literal")` is the only form the JIT can lower:
                    // the message has to be a parse-time `Expr::String(sym)`
                    // so codegen can pass the symbol id as a u64 immediate
                    // to `jit_panic`. Anything dynamic (a const, a runtime
                    // str, etc.) falls back to the interpreter where the
                    // value is already a real Object::ConstString / String.
                    //
                    // Returns `Never` (the bottom type) so that an
                    // expression-position panic — e.g. the then-branch of
                    // `if cond { panic("...") } else { 5i64 }` — unifies
                    // with the other branch's value type instead of
                    // forcing the if-expression to be Unit.
                    if args.len() != 1 {
                        note(reject_reason, || "panic takes 1 argument".to_string());
                        return None;
                    }
                    let arg = program.expression.get(&args[0])?;
                    if !matches!(arg, Expr::String(_)) {
                        note(reject_reason, || {
                            "panic argument must be a string literal in JIT".to_string()
                        });
                        return None;
                    }
                    Some(ScalarTy::Never)
                }
                BuiltinFunction::Assert => {
                    // `assert(cond, "literal")` — same constraint on the
                    // message as `panic` (literal only). The condition is a
                    // regular bool expression and is checked recursively.
                    if args.len() != 2 {
                        note(reject_reason, || {
                            "assert takes 2 arguments (cond, msg)".to_string()
                        });
                        return None;
                    }
                    let cond_ty = check_expr(
                        program, &args[0], locals, struct_locals, tuple_locals,
                        substitutions, struct_layouts, callees, ptr_read_hints,
                        reject_reason,
                    )?;
                    if cond_ty != ScalarTy::Bool {
                        note(reject_reason, || {
                            "assert condition must be bool".to_string()
                        });
                        return None;
                    }
                    let msg_arg = program.expression.get(&args[1])?;
                    if !matches!(msg_arg, Expr::String(_)) {
                        note(reject_reason, || {
                            "assert message must be a string literal in JIT".to_string()
                        });
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::Print | BuiltinFunction::Println => {
                    if args.len() != 1 {
                        return None;
                    }
                    let t = check_expr(program, &args[0], locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                    if !matches!(t, ScalarTy::I64 | ScalarTy::U64 | ScalarTy::F64 | ScalarTy::Bool)
                        && !t.is_narrow_int()
                    {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::HeapAlloc => {
                    if !check_args(&[ScalarTy::U64], &args, locals, struct_locals, tuple_locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Ptr)
                }
                BuiltinFunction::HeapFree => {
                    if !check_args(&[ScalarTy::Ptr], &args, locals, struct_locals, tuple_locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::HeapRealloc => {
                    if !check_args(&[ScalarTy::Ptr, ScalarTy::U64], &args, locals, struct_locals, tuple_locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Ptr)
                }
                BuiltinFunction::PtrIsNull => {
                    if !check_args(&[ScalarTy::Ptr], &args, locals, struct_locals, tuple_locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Bool)
                }
                BuiltinFunction::StrToPtr => {
                    // `__builtin_str_to_ptr(s: str) -> ptr` is not yet
                    // hot-path JIT-eligible: the JIT has no `ScalarTy::Str`
                    // yet (string values aren't modelled as scalars in the
                    // JIT IR), so the call falls back to the interpreter
                    // path where the helper does its work.
                    *reject_reason = Some(
                        "__builtin_str_to_ptr (JIT does not yet model str scalar values)".to_string(),
                    );
                    None
                }
                BuiltinFunction::StrLen => {
                    *reject_reason = Some(
                        "__builtin_str_len (JIT does not yet model str scalar values)".to_string(),
                    );
                    None
                }
                BuiltinFunction::MemCopy | BuiltinFunction::MemMove => {
                    if !check_args(
                        &[ScalarTy::Ptr, ScalarTy::Ptr, ScalarTy::U64],
                        &args,
                        locals,
                    struct_locals,
                    tuple_locals,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    ) {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::MemSet => {
                    if !check_args(
                        &[ScalarTy::Ptr, ScalarTy::U64, ScalarTy::U64],
                        &args,
                        locals,
                    struct_locals,
                    tuple_locals,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    ) {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::PtrRead => {
                    // Args must be (ptr, u64). Return type is decided at the
                    // call site context — we look it up in the hint map. If
                    // the read appears in a position where eligibility never
                    // got to register a hint, fail.
                    if !check_args(
                        &[ScalarTy::Ptr, ScalarTy::U64],
                        &args,
                        locals,
                    struct_locals,
                    tuple_locals,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    ) {
                        return None;
                    }
                    let resolved = ptr_read_hints.get(expr_ref).copied();
                    if resolved.is_none() {
                        note(reject_reason, || {
                            "ptr_read used outside a typed val/var/assign — JIT \
                             needs the result type to be statically known"
                                .to_string()
                        });
                    }
                    resolved
                }
                BuiltinFunction::SizeOf => {
                    if args.len() != 1 {
                        note(reject_reason, || {
                            format!(
                                "__builtin_sizeof takes 1 argument, got {}",
                                args.len()
                            )
                        });
                        return None;
                    }
                    let t = check_expr(
                        program,
                        &args[0],
                        locals,
                        struct_locals,
                        tuple_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    if !matches!(
                        t,
                        ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Bool | ScalarTy::Ptr
                            | ScalarTy::F64
                    ) && !t.is_narrow_int()
                    {
                        note(reject_reason, || {
                            format!("__builtin_sizeof of {t:?} is not supported in JIT")
                        });
                        return None;
                    }
                    Some(ScalarTy::U64)
                }
                BuiltinFunction::PtrWrite => {
                    if args.len() != 3 {
                        return None;
                    }
                    let p = check_expr(program, &args[0], locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                    let off = check_expr(program, &args[1], locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                    let v = check_expr(program, &args[2], locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                    if p != ScalarTy::Ptr || off != ScalarTy::U64 {
                        return None;
                    }
                    if !matches!(
                        v,
                        ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Bool | ScalarTy::Ptr
                    ) {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::DefaultAllocator
                | BuiltinFunction::ArenaAllocator
                | BuiltinFunction::CurrentAllocator => {
                    if !args.is_empty() {
                        note(reject_reason, || {
                            format!(
                                "{:?} expects no arguments, got {}",
                                func,
                                args.len()
                            )
                        });
                        return None;
                    }
                    Some(ScalarTy::Allocator)
                }
                BuiltinFunction::FixedBufferAllocator => {
                    if args.len() != 1 {
                        note(reject_reason, || {
                            format!(
                                "fixed_buffer_allocator expects 1 argument, got {}",
                                args.len()
                            )
                        });
                        return None;
                    }
                    let cap_ty = check_expr(
                        program,
                        &args[0],
                        locals,
                        struct_locals,
                        tuple_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    if cap_ty != ScalarTy::U64 {
                        note(reject_reason, || {
                            "fixed_buffer_allocator capacity must be u64".to_string()
                        });
                        return None;
                    }
                    Some(ScalarTy::Allocator)
                }
                BuiltinFunction::ArenaDrop => {
                    // #121 Phase B-rest Item 2 follow-up: explicit
                    // arena drop. JIT path falls back to interpreter
                    // since the JIT lowering for the user-callable
                    // form isn't wired (the AOT path is). Reject
                    // here so callers fall back cleanly.
                    note(reject_reason, || {
                        "JIT does not yet model __builtin_arena_drop".to_string()
                    });
                    None
                }
                BuiltinFunction::FixedBufferDrop => {
                    // Phase 5: explicit fixed_buffer drop, same JIT
                    // story as ArenaDrop — silent fallback to
                    // interpreter (the AOT path is wired).
                    note(reject_reason, || {
                        "JIT does not yet model __builtin_fixed_buffer_drop".to_string()
                    });
                    None
                }
                BuiltinFunction::Abs => {
                    if args.len() != 1 {
                        note(reject_reason, || {
                            format!("abs expects 1 argument, got {}", args.len())
                        });
                        return None;
                    }
                    let t = check_expr(
                        program,
                        &args[0],
                        locals,
                        struct_locals,
                        tuple_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    // Polymorphic: i64 / f64 both produce same-type
                    // result. f64 lowers to cranelift's `fabs`
                    // instruction, i64 to `select(x < 0, -x, x)`.
                    match t {
                        ScalarTy::I64 => Some(ScalarTy::I64),
                        ScalarTy::F64 => Some(ScalarTy::F64),
                        _ => {
                            note(reject_reason, || {
                                "abs expects an i64 or f64 argument".to_string()
                            });
                            None
                        }
                    }
                }
                // NOTE: f64 math arms (Sqrt/Pow and Sin..=Ceil) lived
                // here before Phase 4. Each is now an `extern fn
                // __extern_*_f64` declaration whose JIT lowering goes
                // through `try_gen_extern_call` (codegen) +
                // `JIT_EXTERN_DISPATCH` (eligibility's extern table).
                BuiltinFunction::Min | BuiltinFunction::Max => {
                    if args.len() != 2 {
                        let name = if matches!(func, BuiltinFunction::Min) { "min" } else { "max" };
                        note(reject_reason, || {
                            format!("{name} expects 2 arguments, got {}", args.len())
                        });
                        return None;
                    }
                    let a = check_expr(
                        program,
                        &args[0],
                        locals,
                        struct_locals,
                        tuple_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    let b = check_expr(
                        program,
                        &args[1],
                        locals,
                        struct_locals,
                        tuple_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    if !matches!(a, ScalarTy::I64 | ScalarTy::U64) || a != b {
                        let name = if matches!(func, BuiltinFunction::Min) { "min" } else { "max" };
                        note(reject_reason, || {
                            format!("{name} expects matching i64 or u64 operands")
                        });
                        return None;
                    }
                    Some(a)
                }
            }
        }
        Expr::With(allocator_expr, body_expr) => {
            // Validate the allocator producer first; it must yield an
            // Allocator handle.
            let alloc_ty = check_expr(
                program,
                &allocator_expr,
                locals,
                struct_locals,
                tuple_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            )?;
            if alloc_ty != ScalarTy::Allocator {
                note(reject_reason, || {
                    "`with allocator =` requires an Allocator-typed expression".to_string()
                });
                return None;
            }
            // Early exits inside the body (`return` / `break` /
            // `continue`) are now supported: the codegen tracks the
            // active `with` depth and emits the matching pops before
            // each early-exit terminator.
            check_expr(
                program,
                &body_expr,
                locals,
                struct_locals,
                tuple_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            )
        }
        Expr::MethodCall(receiver, method_name, args) => {
            // The receiver may be:
            //   1. A struct local (existing struct-method dispatch).
            //   2. A primitive scalar local (Step C extension-trait
            //      dispatch — `i64.neg()` etc.).
            //   3. An arbitrary primitive-scalar-typed expression
            //      (#194 chained-call relaxation — `x.abs().abs()`,
            //      `make_i64().abs()`, etc.) where the receiver is
            //      not a bare identifier but type-checks to a known
            //      primitive scalar.
            // Eligibility rejects anything that doesn't reduce to
            // one of these.
            let recv_expr = program.expression.get(&receiver)?;
            let recv_name_opt = match recv_expr {
                Expr::Identifier(s) => Some(s),
                _ => None,
            };

            // Resolve the receiver's primitive type. Identifier
            // receivers consult `locals` directly; non-identifier
            // receivers re-enter `check_expr` so a chained
            // `MethodCall` / `Call` / `BinaryOp` etc. that returns a
            // scalar gets its scalar type back.
            let recv_prim_ty: Option<ScalarTy> = if let Some(name) = recv_name_opt {
                locals.get(&name).copied()
            } else {
                check_expr(
                    program,
                    &receiver,
                    locals,
                    struct_locals,
                    tuple_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                )
            };

            // Step C / #194: extension-trait dispatch on a primitive
            // receiver. The receiver scalar type keys into the
            // same `find_method` lookup the struct path uses. On
            // success we register the call as a `MonoTarget::Method`
            // so the analyzer queues the method body for compilation,
            // mirroring the struct path.
            if let Some(prim_ty) = recv_prim_ty {
                let target_sym = match primitive_target_sym_for_scalar(prim_ty) {
                    Some(s) => s,
                    None => {
                        note(reject_reason, || {
                            "method receiver primitive has no extension impls".to_string()
                        });
                        return None;
                    }
                };
                let method = match find_method(program, target_sym, method_name) {
                    Some(m) => m,
                    None => {
                        note(reject_reason, || {
                            "method not found on primitive type".to_string()
                        });
                        return None;
                    }
                };
                if !method.generic_params.is_empty() {
                    note(reject_reason, || {
                        "generic methods are not yet JIT-compatible".to_string()
                    });
                    return None;
                }
                if method.parameter.is_empty() {
                    note(reject_reason, || {
                        "method has no parameters; expected `self`".to_string()
                    });
                    return None;
                }
                let expected_param_count = method.parameter.len() - 1;
                if args.len() != expected_param_count {
                    note(reject_reason, || {
                        format!(
                            "primitive method call has {} arg(s), expects {}",
                            args.len(),
                            expected_param_count
                        )
                    });
                    return None;
                }
                // Type-check each arg against the parameter, with
                // `Self_` resolved to the receiver's primitive type.
                for (i, arg) in args.iter().enumerate() {
                    let raw_param_td = &method.parameter[i + 1].1;
                    let resolved_param_td = match raw_param_td {
                        TypeDecl::Self_ => primitive_type_decl_for_target_sym(target_sym)
                            .unwrap_or_else(|| raw_param_td.clone()),
                        other => other.clone(),
                    };
                    let actual = check_expr(
                        program, arg, locals, struct_locals, tuple_locals,
                        substitutions, struct_layouts, callees, ptr_read_hints,
                        reject_reason,
                    )?;
                    let want = match resolve_param_ty(&resolved_param_td, substitutions, struct_layouts) {
                        Some(ParamTy::Scalar(s)) => s,
                        _ => {
                            note(reject_reason, || {
                                "primitive method parameter type unsupported".to_string()
                            });
                            return None;
                        }
                    };
                    if actual != want {
                        note(reject_reason, || {
                            format!("primitive method arg type mismatch: got {actual:?}, want {want:?}")
                        });
                        return None;
                    }
                }
                callees.push(MonoCall {
                    call_expr: *expr_ref,
                    target: MonoTarget::Method(target_sym, method_name),
                    mono_args: Vec::new(),
                });
                // Resolve the return type, with `Self_` mapping to the
                // receiver's primitive type.
                let ret_td = match &method.return_type {
                    Some(td) => match td {
                        TypeDecl::Self_ => primitive_type_decl_for_target_sym(target_sym)
                            .unwrap_or_else(|| td.clone()),
                        other => other.clone(),
                    },
                    None => TypeDecl::Unit,
                };
                return match resolve_param_ty(&ret_td, substitutions, struct_layouts) {
                    Some(ParamTy::Scalar(s)) => Some(s),
                    _ => {
                        note(reject_reason, || {
                            "primitive method return type unsupported".to_string()
                        });
                        None
                    }
                };
            }

            // Struct-method dispatch: the existing path requires a
            // bare `Identifier` receiver because struct values flow
            // through `struct_locals` (per-field SSA Variables) and
            // there's no machinery to materialise a chained struct
            // value back into that representation. Reject anything
            // that didn't come through the Identifier shortcut.
            let recv_name = match recv_name_opt {
                Some(s) => s,
                None => {
                    note(reject_reason, || {
                        "non-primitive method receiver must be a local identifier".to_string()
                    });
                    return None;
                }
            };
            let struct_name = match struct_locals.get(&recv_name).copied() {
                Some(s) => s,
                None => {
                    note(reject_reason, || {
                        "method receiver is not a known struct local".to_string()
                    });
                    return None;
                }
            };
            // Validate each argument's type against the corresponding
            // method parameter (skipping `self`). Methods don't yet
            // support generics, so callee_subs is always empty.
            // Linear scan over top-level ImplBlock decls is fine — only
            // run once per call site, and the analyzer already pre-built
            // a method_map for the work-stack pass.
            let method = match find_method(program, struct_name, method_name) {
                Some(m) => m,
                None => {
                    note(reject_reason, || {
                        "method not found on struct".to_string()
                    });
                    return None;
                }
            };
            if !method.generic_params.is_empty() {
                note(reject_reason, || {
                    "generic methods are not yet JIT-compatible".to_string()
                });
                return None;
            }
            // The first parameter is the receiver (`self: Self` in the
            // language's preferred style). Remaining parameters must
            // line up with the explicit arguments at the call site.
            if method.parameter.is_empty() {
                note(reject_reason, || {
                    "method has no parameters; expected `self`".to_string()
                });
                return None;
            }
            let expected_param_count = method.parameter.len() - 1;
            if args.len() != expected_param_count {
                note(reject_reason, || {
                    format!(
                        "method call has {} arg(s), expects {}",
                        args.len(),
                        expected_param_count
                    )
                });
                return None;
            }
            for (i, arg) in args.iter().enumerate() {
                let param_td = &method.parameter[i + 1].1;
                let arg_expr = program.expression.get(arg)?;
                if let Expr::Identifier(id) = arg_expr {
                    if let Some(arg_struct) = struct_locals.get(&id).copied() {
                        match param_td {
                            TypeDecl::Identifier(s) | TypeDecl::Struct(s, _)
                                if *s == arg_struct
                                    && struct_layouts.contains_key(s) =>
                            {
                                continue;
                            }
                            _ => {
                                note(reject_reason, || {
                                    "method struct argument type mismatch".to_string()
                                });
                                return None;
                            }
                        }
                    }
                }
                // Scalar arg path
                let actual = check_expr(
                    program,
                    arg,
                    locals,
                    struct_locals,
                    tuple_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                )?;
                let want = match resolve_param_ty(param_td, substitutions, struct_layouts) {
                    Some(ParamTy::Scalar(s)) => s,
                    _ => {
                        note(reject_reason, || {
                            "method parameter type unsupported".to_string()
                        });
                        return None;
                    }
                };
                if actual != want {
                    note(reject_reason, || {
                        format!("method arg type mismatch: got {actual:?}, want {want:?}")
                    });
                    return None;
                }
            }
            callees.push(MonoCall {
                call_expr: *expr_ref,
                target: MonoTarget::Method(struct_name, method_name),
                mono_args: Vec::new(),
            });
            // Compute method's return type.
            match &method.return_type {
                Some(td) => {
                    let resolved = match td {
                        TypeDecl::Self_ => TypeDecl::Identifier(struct_name),
                        other => other.clone(),
                    };
                    match resolve_param_ty(&resolved, substitutions, struct_layouts) {
                        Some(ParamTy::Scalar(s)) => Some(s),
                        Some(ParamTy::Struct(_)) => {
                            // Struct-returning methods only flow through
                            // val/var rhs, similar to free-function struct
                            // returns. Reject in arbitrary positions.
                            note(reject_reason, || {
                                "struct-returning method must be the rhs of a val/var"
                                    .to_string()
                            });
                            None
                        }
                        Some(ParamTy::Tuple(_)) => {
                            note(reject_reason, || {
                                "tuple-returning method must be the rhs of a val/var"
                                    .to_string()
                            });
                            None
                        }
                        None => None,
                    }
                }
                None => Some(ScalarTy::Unit),
            }
        }
        Expr::FieldAccess(receiver, field_name) => {
            // Read access on a struct local: returns the field's scalar
            // type. Anything else (FieldAccess on a function call result,
            // nested FieldAccess, etc.) falls through to ineligible.
            let receiver_expr = program.expression.get(&receiver)?;
            let recv_name = match receiver_expr {
                Expr::Identifier(s) => s,
                _ => {
                    note(reject_reason, || {
                        "field access receiver must be a struct local".to_string()
                    });
                    return None;
                }
            };
            let struct_name = match struct_locals.get(&recv_name).copied() {
                Some(s) => s,
                None => {
                    note(reject_reason, || {
                        "field access on a non-struct local".to_string()
                    });
                    return None;
                }
            };
            let field_ty = struct_layouts
                .get(&struct_name)
                .and_then(|l| l.field(field_name));
            if field_ty.is_none() {
                note(reject_reason, || "unknown field on struct".to_string());
            }
            field_ty
        }
        Expr::TupleAccess(tuple, idx) => {
            // Read access on a tuple local. The receiver must be an
            // identifier already bound to a known tuple shape; the
            // index must be in-range.
            let recv_expr = program.expression.get(&tuple)?;
            let recv_name = match recv_expr {
                Expr::Identifier(s) => s,
                _ => {
                    note(reject_reason, || {
                        "tuple access receiver must be a tuple local".to_string()
                    });
                    return None;
                }
            };
            let shape = match tuple_locals.get(&recv_name).cloned() {
                Some(v) => v,
                None => {
                    note(reject_reason, || {
                        "tuple access on a non-tuple local".to_string()
                    });
                    return None;
                }
            };
            if idx >= shape.len() {
                note(reject_reason, || {
                    format!("tuple index {idx} out of bounds for shape {shape:?}")
                });
                return None;
            }
            Some(shape[idx])
        }
        Expr::Cast(inner, target) => {
            // Casts allowed: any-width int ↔ any-width int (sextend /
            // uextend / ireduce in codegen), and i64/u64 ↔ f64 (real
            // fcvt instructions). bool casts are intentionally
            // excluded.
            let inner_ty = check_expr(program, &inner, locals, struct_locals, tuple_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            let target_ty = ScalarTy::from_type_decl(&target)?;
            let int_or_float = |t: ScalarTy| {
                matches!(t, ScalarTy::I64 | ScalarTy::U64 | ScalarTy::F64) || t.is_narrow_int()
            };
            if !int_or_float(inner_ty) {
                return None;
            }
            if !int_or_float(target_ty) {
                return None;
            }
            // Reject narrow-int <-> f64 for now: those require fcvt
            // pre-extending the narrow side, which the cast codegen
            // doesn't yet handle. Stay safe and fall back.
            let narrow_to_float = inner_ty.is_narrow_int() && target_ty == ScalarTy::F64;
            let float_to_narrow = inner_ty == ScalarTy::F64 && target_ty.is_narrow_int();
            if narrow_to_float || float_to_narrow {
                note(reject_reason, || {
                    "JIT cast between narrow int and f64 is not yet supported".to_string()
                });
                return None;
            }
            Some(target_ty)
        }
        // Everything else is unsupported in this iteration.
        other => {
            // Phase JE-1a: a `QualifiedIdentifier` whose head is a
            // JIT-eligible enum (non-generic, unit-only) corresponds
            // to a unit-variant constructor like `Color::Red`. The
            // tag layout is already in `enum_layouts`; the missing
            // piece is constructor + match codegen (Phase JE-1b).
            // Surface a precise "infra ready, codegen pending"
            // message instead of the generic "qualified identifier"
            // catch-all so the next phase knows which programs to
            // enable.
            let precise = match &other {
                Expr::QualifiedIdentifier(path)
                    if path.len() == 2
                        && enum_layout_for(path[0])
                            .and_then(|l| l.variant_tag(path[1]))
                            .is_some() =>
                {
                    "JIT enum support pending: unit-variant constructor codegen \
                     (Phase JE-1b will lower this via the existing tag layout)"
                        .to_string()
                }
                Expr::QualifiedIdentifier(path)
                    if !path.is_empty()
                        && enum_decl_lookup_by_name(program, path[0]).is_some() =>
                {
                    "JIT does not yet model enum values \
                     (constructors / match / methods)"
                        .to_string()
                }
                _ => format!("uses unsupported expression {}", expr_kind_name(&other)),
            };
            note(reject_reason, move || precise);
            None
        }
    }
}

/// Shared eligibility check for plain function calls. Used by both
/// `Expr::Call(name, ExprList(args))` (the bare-name form) and
/// `Expr::AssociatedFunctionCall(module, name, args)` (the
/// module-qualified form, after the module-alias guard has confirmed
/// the qualifier doesn't refer to a struct). Locates the callee in
/// the program's function table, type-checks each argument against
/// the matching parameter (handling identifier-of-struct, identifier-
/// of-tuple-local, inline tuple literal, and scalar fall-throughs),
/// records the monomorphisation key, and returns the substituted
/// callee return type.
#[allow(clippy::too_many_arguments)]
fn check_plain_call(
    program: &Program,
    expr_ref: &ExprRef,
    name: DefaultSymbol,
    arg_list: &Vec<ExprRef>,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
    tuple_locals: &mut HashMap<DefaultSymbol, Vec<ScalarTy>>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<ScalarTy> {
    // Locate the callee in the program's function table.
    let callee = program.function.iter().find(|f| f.name == name).cloned();
    let callee = match callee {
        Some(f) => f,
        None => {
            note(reject_reason, || "calls an unknown function".to_string());
            return None;
        }
    };

    // `extern fn` callees: validate against the JIT extern dispatch
    // table. The table is keyed by interned symbol via the
    // thread-local set up by `analyze` (see `with_extern_dispatch`).
    // Names that aren't in the table fall back to the interpreter
    // call path.
    if callee.is_extern {
        let entry = match jit_extern_dispatch_for(name) {
            Some(e) => e,
            None => {
                note(reject_reason, || {
                    "calls extern fn not registered with the JIT".to_string()
                });
                return None;
            }
        };
        if arg_list.len() != entry.params.len() {
            note(reject_reason, || {
                format!(
                    "extern fn called with {} arg(s), expected {}",
                    arg_list.len(),
                    entry.params.len()
                )
            });
            return None;
        }
        for (a, want) in arg_list.iter().zip(entry.params.iter()) {
            match check_expr(
                program, a, locals, struct_locals, tuple_locals,
                substitutions, struct_layouts, callees, ptr_read_hints,
                reject_reason,
            ) {
                Some(t) if t == *want => {}
                _ => {
                    note(reject_reason, || {
                        "extern fn argument type mismatch".to_string()
                    });
                    return None;
                }
            }
        }
        // Extern fns skip the monomorphisation pipeline; codegen
        // recognises the `is_extern` flag and emits the matching
        // helper / native op.
        return Some(entry.ret);
    }

    // Resolve each argument's type, allowing struct identifiers
    // when the callee's parameter at that position is a struct.
    // Generic substitutions are inferred only from scalar args;
    // generic-over-struct functions aren't supported in this
    // iteration.
    if arg_list.len() != callee.parameter.len() {
        note(reject_reason, || {
            format!(
                "call has {} arg(s), callee expects {}",
                arg_list.len(),
                callee.parameter.len()
            )
        });
        return None;
    }
    let mut scalar_arg_tys: Vec<ScalarTy> = Vec::with_capacity(arg_list.len());
    let mut callee_param_tys: Vec<ParamTy> = Vec::with_capacity(arg_list.len());
    for (a, (_, param_td)) in arg_list.iter().zip(callee.parameter.iter()) {
        let arg_expr = program.expression.get(a)?;
        if let Expr::Identifier(id) = arg_expr {
            if let Some(struct_name) = struct_locals.get(&id).copied() {
                match param_td {
                    TypeDecl::Identifier(s) | TypeDecl::Struct(s, _)
                        if *s == struct_name && struct_layouts.contains_key(s) =>
                    {
                        callee_param_tys.push(ParamTy::Struct(struct_name));
                        scalar_arg_tys.push(ScalarTy::Unit);
                        continue;
                    }
                    _ => {
                        note(reject_reason, || {
                            "struct argument's type does not match callee parameter"
                                .to_string()
                        });
                        return None;
                    }
                }
            }
            if let Some(shape) = tuple_locals.get(&id).cloned() {
                let want = match resolve_param_ty(param_td, substitutions, struct_layouts) {
                    Some(ParamTy::Tuple(ts)) => ts,
                    _ => {
                        note(reject_reason, || {
                            "tuple argument's type does not match callee parameter".to_string()
                        });
                        return None;
                    }
                };
                if want != shape {
                    note(reject_reason, || {
                        "tuple argument shape does not match callee parameter".to_string()
                    });
                    return None;
                }
                callee_param_tys.push(ParamTy::Tuple(shape));
                scalar_arg_tys.push(ScalarTy::Unit);
                continue;
            }
        }
        if let Expr::TupleLiteral(elements) = arg_expr {
            let want = match resolve_param_ty(param_td, substitutions, struct_layouts) {
                Some(ParamTy::Tuple(ts)) => ts,
                _ => {
                    note(reject_reason, || {
                        "inline tuple literal argument needs a tuple parameter".to_string()
                    });
                    return None;
                }
            };
            if elements.len() != want.len() {
                note(reject_reason, || {
                    "inline tuple literal argument arity does not match callee parameter".to_string()
                });
                return None;
            }
            let mut shape: Vec<ScalarTy> = Vec::with_capacity(elements.len());
            for e in &elements {
                let t = check_expr(
                    program, e, locals, struct_locals, tuple_locals, substitutions,
                    struct_layouts, callees, ptr_read_hints, reject_reason,
                )?;
                shape.push(t);
            }
            if shape != want {
                note(reject_reason, || {
                    "inline tuple literal argument element types do not match callee parameter"
                        .to_string()
                });
                return None;
            }
            callee_param_tys.push(ParamTy::Tuple(shape));
            scalar_arg_tys.push(ScalarTy::Unit);
            continue;
        }
        let t = check_expr(
            program, a, locals, struct_locals, tuple_locals, substitutions,
            struct_layouts, callees, ptr_read_hints, reject_reason,
        )?;
        scalar_arg_tys.push(t);
        callee_param_tys.push(ParamTy::Scalar(t));
    }

    let callee_subs = match infer_substitutions(
        &callee, &scalar_arg_tys, substitutions, reject_reason,
    ) {
        Some(s) => s,
        None => return None,
    };

    let mono_args: Vec<ScalarTy> = callee
        .generic_params
        .iter()
        .map(|g| callee_subs.get(g).copied().unwrap_or(ScalarTy::Unit))
        .collect();
    callees.push(MonoCall {
        call_expr: *expr_ref,
        target: MonoTarget::Function(name),
        mono_args,
    });

    let _ = callee_param_tys;

    match &callee.return_type {
        Some(td) => substitute_to_scalar(td, &callee_subs),
        None => Some(ScalarTy::Unit),
    }
}

/// Short human-readable name of an Expr variant for reject messages.
fn expr_kind_name(e: &Expr) -> &'static str {
    match e {
        Expr::Number(_) => "untyped numeric literal",
        Expr::Null => "null",
        Expr::ExprList(_) => "expression list",
        Expr::String(_) => "string literal",
        Expr::ArrayLiteral(_) => "array literal",
        Expr::FieldAccess(_, _) => "field access",
        Expr::MethodCall(_, _, _) => "method call",
        Expr::StructLiteral(_, _) => "struct literal",
        Expr::QualifiedIdentifier(_) => "qualified identifier",
        Expr::BuiltinMethodCall(_, _, _) => "builtin method call",
        Expr::SliceAccess(_, _) => "slice access",
        Expr::SliceAssign(_, _, _, _) => "slice assign",
        Expr::AssociatedFunctionCall(_, _, _) => "associated function call",
        Expr::DictLiteral(_) => "dict literal",
        Expr::TupleLiteral(_) => "tuple literal",
        Expr::TupleAccess(_, _) => "tuple access",
        Expr::With(_, _) => "`with allocator` block",
        Expr::Match(_, _) => "match expression",
        Expr::Range(_, _) => "range value",
        _ => "expression",
    }
}
