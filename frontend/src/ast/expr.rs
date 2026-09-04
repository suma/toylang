use string_interner::{DefaultSymbol, DefaultStringInterner};
use std::rc::Rc;
use crate::type_decl::TypeDecl;
use super::{ExprRef, StmtRef, StructField, Visibility, MethodFunction, TraitMethodSignature, ParameterList};

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SliceType {
    SingleElement,    // a[index]
    RangeSlice,       // a[start..end], a[start..], a[..end], a[..]
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SliceInfo {
    pub start: Option<ExprRef>,
    pub end: Option<ExprRef>,
    pub has_dotdot: bool,  // Whether DotDot syntax was used
    pub slice_type: SliceType,
}

impl SliceInfo {
    pub fn single_element(index: ExprRef) -> Self {
        SliceInfo {
            start: Some(index),
            end: None,
            has_dotdot: false,
            slice_type: SliceType::SingleElement,
        }
    }

    pub fn range_slice(start: Option<ExprRef>, end: Option<ExprRef>) -> Self {
        SliceInfo {
            start,
            end,
            has_dotdot: true,
            slice_type: SliceType::RangeSlice,
        }
    }

    pub fn is_valid_for_dict(&self) -> bool {
        match self.slice_type {
            SliceType::SingleElement => true,  // dict[key] is OK
            SliceType::RangeSlice => false,    // dict[start..end] is not supported
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Stmt {
    Expression(ExprRef),
    Val(DefaultSymbol, Option<TypeDecl>, ExprRef),
    Var(DefaultSymbol, Option<TypeDecl>, Option<ExprRef>),
    Return(Option<ExprRef>),
    /// Optional `Some(label_sym)` for `break @label` (LABEL feature),
    /// `None` for plain `break` which targets the innermost loop.
    Break(Option<DefaultSymbol>),
    /// Optional `Some(label_sym)` for `continue @label`.
    Continue(Option<DefaultSymbol>),
    /// Optional leading label for `@label: for ...` (LABEL feature).
    For(Option<DefaultSymbol>, DefaultSymbol, ExprRef, ExprRef, ExprRef), // label?, var, start, end, block
    /// Optional leading label for `@label: while ...`.
    While(Option<DefaultSymbol>, ExprRef, ExprRef), // label?, cond, block
    StructDecl {
        name: DefaultSymbol,
        generic_params: Vec<DefaultSymbol>,  // Generic type parameters like <T>
        // Optional bounds on each generic parameter (e.g. `<A: Allocator>`).
        // Missing entries mean unbounded.
        generic_bounds: std::collections::HashMap<DefaultSymbol, TypeDecl>,
        fields: Vec<StructField>,
        visibility: Visibility,
    },
    ImplBlock {
        target_type: DefaultSymbol,
        /// Concrete type arguments on the impl target (e.g. `<u8>` in
        /// `impl FromStr for Vec<u8>`). Empty for both inherent impls
        /// `impl Foo` and generic-parameterised impls `impl<T> Foo<T>`
        /// where `T` is a generic parameter, not a concrete type.
        /// Disambiguates multiple `impl Trait for Generic<T>` blocks
        /// with different `T` so the registry can store them as
        /// separate specialisations.
        target_type_args: Vec<TypeDecl>,
        methods: Vec<Rc<MethodFunction>>,
        /// `Some(trait_name)` for `impl <Trait> for <Type>`, `None` for an
        /// inherent `impl <Type>`. Trait conformance is recorded by the
        /// type-checker; runtime dispatch sees the methods either way.
        trait_name: Option<DefaultSymbol>,
        /// ITER-PROTOCOL-TRAIT: concrete type arguments supplied to a
        /// **generic** trait at this impl site (e.g. `<i64>` in
        /// `impl Iterator<i64> for Counter`). Empty for non-generic
        /// trait impls (`impl Greet for Dog`) and inherent impls
        /// (`trait_name` is None). The type-checker substitutes the
        /// trait's generic parameters with these args before verifying
        /// each impl method matches the trait signature.
        trait_type_args: Vec<TypeDecl>,
    },
    /// `trait Name { fn m(self: Self, ...) -> T; ... }` — declares a set of
    /// method signatures that conforming structs must provide. Trait methods
    /// have no body. ITER-PROTOCOL-TRAIT added `generic_params`; default
    /// methods, multi-bound (`<T: A + B>`), trait inheritance, and `dyn Trait`
    /// remain out of scope.
    TraitDecl {
        name: DefaultSymbol,
        /// ITER-PROTOCOL-TRAIT: parameter names introduced by
        /// `trait Foo<T, U, ...>`. Each appears as `TypeDecl::Generic(P)`
        /// inside method signatures and is substituted with the
        /// matching `trait_type_args` entry from the impl site at
        /// conformance-check time. Empty for non-generic traits.
        generic_params: Vec<DefaultSymbol>,
        methods: Vec<TraitMethodSignature>,
        visibility: Visibility,
    },
    EnumDecl {
        name: DefaultSymbol,
        generic_params: Vec<DefaultSymbol>,  // empty for non-generic enums
        variants: Vec<EnumVariantDef>,
        visibility: Visibility,
    },
    /// `type Name = Type` or `type Name<T1, T2> = Type` —
    /// top-level type alias. The defining parser eagerly
    /// substitutes uses of `name` *within the same file*; this
    /// Stmt also drives a post-integration cross-module
    /// substitution pass that resolves alias references in any
    /// auto-loaded / imported module. `generic_params` is empty
    /// for non-generic aliases; otherwise it lists the
    /// parameter symbols that appear as `Generic(P)` markers
    /// in `target`.
    TypeAlias {
        name: DefaultSymbol,
        generic_params: Vec<DefaultSymbol>,
        target: TypeDecl,
        visibility: Visibility,
    },
}

/// Phase 2 enum variant: a name plus an optional tuple-style payload. An empty
/// `payload_types` vector is a unit variant.
#[derive(Debug, PartialEq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EnumVariantDef {
    pub name: DefaultSymbol,
    pub payload_types: Vec<TypeDecl>,
}

/// Patterns for `match` arms. Patterns compose recursively — tuple-variant
/// sub-patterns can themselves be any Pattern, enabling nested matches such
/// as `Some(Some(x))` or `Some(Color::Red)`.
#[derive(Debug, PartialEq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Pattern {
    /// `Enum::Variant` for unit variants, or `Enum::Variant(p, q, r)` for
    /// tuple variants. The sub-pattern vector is empty for unit variants.
    EnumVariant(DefaultSymbol, DefaultSymbol, Vec<Pattern>),
    /// Integer / bool literal pattern such as `0i64`, `42u64`, or `true`.
    /// The stored `ExprRef` points at a literal expression in the pool.
    Literal(ExprRef),
    /// Identifier pattern — binds the matched value to `name` in the arm
    /// body's scope. Only legal as a sub-pattern of a tuple variant.
    Name(DefaultSymbol),
    /// Tuple pattern, e.g. `(x, y)` or `(_, 0i64)`. Sub-patterns may be
    /// any `Pattern`, including nested tuples. Currently irrefutable —
    /// the scrutinee's tuple length and element types must match.
    Tuple(Vec<Pattern>),
    /// PATTERN-STRUCT: `Point { x: 0i64, y }` — match on a struct's
    /// fields by name. The shorthand `{ x }` is stored as
    /// `(x, Pattern::Name(x))`, so every entry pairs a field with the
    /// pattern its value must match.
    ///
    /// The `bool` is whether the pattern ended in `..`: with it the
    /// unlisted fields are ignored, without it every field must be
    /// named. Field order in the pattern need not match the
    /// declaration.
    Struct(DefaultSymbol, Vec<(DefaultSymbol, Pattern)>, bool),
    /// PATTERN-EXTEND: `lo..hi` — a **half-open** integer range,
    /// matching the `..` expression form, so `0i64..5i64` covers 0
    /// through 4. Both `ExprRef`s point at integer literals in the
    /// pool; an empty range (`hi <= lo`) is a type error rather than
    /// an arm that can never run.
    Range(ExprRef, ExprRef),
    /// PATTERN-EXTEND: `n @ pat` — bind `n` to the whole matched
    /// value while `pat` still decides whether the arm runs.
    ///
    /// The inner pattern is any pattern, so `x @ Color::Red` and
    /// `p @ Point { x: 0i64, .. }` both work. The binding itself never
    /// rejects a value, so refutability, exhaustiveness and the shape
    /// checks every backend emits are all the inner pattern's — a
    /// `Binding` is transparent to them and is peeled before they run.
    Binding(DefaultSymbol, Box<Pattern>),
    Wildcard, // _
}

/// One arm of a `match` expression. The optional `guard` is a boolean
/// expression evaluated **after** the pattern matches and the pattern's
/// bindings are in scope; an arm with a `false` guard is skipped, so
/// guarded arms count as refutable for exhaustiveness regardless of
/// pattern shape.
#[derive(Debug, PartialEq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<ExprRef>,
    pub body: ExprRef,
}

#[derive(Debug, PartialEq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Expr {
    Assign(ExprRef, ExprRef),   // lhs = rhs
    IfElifElse(ExprRef, ExprRef, Vec<(ExprRef, ExprRef)>, ExprRef), // if_cond, if_block, elif_pairs, else_block
    Binary(Operator, ExprRef, ExprRef),
    Unary(UnaryOp, ExprRef),     // unary operations like ~expr
    Block(Vec<StmtRef>),
    True,
    False,
    Int64(i64),
    UInt64(u64),
    // NUM-W narrow integer literals. Same shape as Int64 / UInt64;
    // the parser produces these when the lexer hands back a typed
    // literal token (`42u8` / `0xFFi32` / `7i16` ...). The
    // values are pre-validated to fit by the lexer.
    Int8(i8),
    Int16(i16),
    Int32(i32),
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    /// A char literal — `'a'` / `'\n'` / `'\u{1F600}'`.
    ///
    /// Its type is `u32` (the `char` alias): 32 bits is what a code
    /// point is held in, and `val c = 'a'` infers `u32`. It is a
    /// variant of its own rather than a `UInt32` because it is the
    /// one integer literal a position naming a *different* integer
    /// type may take without an `as` cast — `val b: u8 = '0'`,
    /// `byte == 'h'` — as long as the value fits. A suffixed literal
    /// keeps the strict NUM-W rule, since its suffix already named
    /// its type; telling the two apart needs the distinction to
    /// survive parsing. The type checker rewrites the node to the
    /// concrete width when a position asks for one, so a backend
    /// that sees this node is looking at a plain `u32` literal.
    CharLiteral(u32),
    Float64(f64),
    Float32(f32),
    Number(DefaultSymbol),
    Identifier(DefaultSymbol),
    Null,
    ExprList(Vec<ExprRef>),
    Call(DefaultSymbol, ExprRef), // apply, function call, etc
    String(DefaultSymbol),
    ArrayLiteral(Vec<ExprRef>),  // [1, 2, 3, 4, 5]
    FieldAccess(ExprRef, DefaultSymbol),  // obj.field
    MethodCall(ExprRef, DefaultSymbol, Vec<ExprRef>),  // obj.method(args)
    StructLiteral(DefaultSymbol, Vec<(DefaultSymbol, ExprRef)>),  // Point { x: 10, y: 20 }
    QualifiedIdentifier(Vec<DefaultSymbol>),  // math::add
    BuiltinMethodCall(ExprRef, BuiltinMethod, Vec<ExprRef>),  // "hello".len(), str.concat("world")
    BuiltinCall(BuiltinFunction, Vec<ExprRef>),  // __builtin_heap_alloc(), __builtin_print_ln(), etc.
    SliceAccess(ExprRef, SliceInfo),  // arr[start..end] - slice access, arr[i] as single element access
    SliceAssign(ExprRef, Option<ExprRef>, Option<ExprRef>, ExprRef),  // arr[start..end] = value, arr[i] = value
    AssociatedFunctionCall(DefaultSymbol, DefaultSymbol, Vec<ExprRef>),  // Container::new(args) - struct_name, function_name, args
    DictLiteral(Vec<(ExprRef, ExprRef)>),  // {key1: value1, key2: value2}
    TupleLiteral(Vec<ExprRef>),  // (expr1, expr2, ...) - tuple literal
    TupleAccess(ExprRef, usize),  // tuple.0, tuple.1, etc - tuple element access
    Cast(ExprRef, TypeDecl),  // expr as type - type cast expression
    With(ExprRef, ExprRef),  // with allocator = allocator_expr { body } - scoped allocator binding
    Match(ExprRef, Vec<MatchArm>),  // match scrutinee { pat [if guard] => body, ... }
    Range(ExprRef, ExprRef),  // start..end — half-open integer range literal
    /// Closure / lambda literal: `fn(x: T, y: U) -> R { body }`. Phase 1
    /// (frontend-only) — parses + lives in the AST + (Phase 2) the type
    /// checker reports a function type. Interpreter / JIT / AOT execution
    /// is wired up in subsequent phases. The `body` is an `ExprRef`
    /// pointing at an `Expr::Block`.
    Closure {
        params: ParameterList,
        return_type: Option<TypeDecl>,
        body: ExprRef,
        /// CLOSURE-CAPTURE E3: does this closure share its captured
        /// bindings with the scope that owns them, rather than taking
        /// a copy of each?
        ///
        /// The parser always writes `false`; the type checker sets it
        /// after deciding the closure cannot outlive those bindings
        /// (`closure_escape`). Backends read it rather than repeating
        /// the analysis — the three independent copies of the *capture
        /// scan* are what let the engines disagree about writes in the
        /// first place.
        captures_by_ref: bool,
    },
    /// `expr?` — postfix early-return operator. The parser emits this
    /// node; the type checker rewrites it to a `match` over the
    /// inner expression's type (`Result<T, E>` or `Option<T>`) using
    /// `expr_pool.update`. Backends therefore never see `Try` —
    /// they only see the rewritten `Match` at the same `ExprRef`.
    ///
    /// The synthetic symbols are pre-interned by the parser so the
    /// type checker (which holds an immutable `&DefaultStringInterner`)
    /// can build the desugared AST without mutable access:
    ///
    /// - `success_binding`: the success-arm pattern binding
    ///   (`__try_v_<n>`) bound to the unwrapped value.
    /// - `error_binding`: the error-arm pattern binding
    ///   (`__try_e_<n>`) bound to the error value; unused for
    ///   `Option::None` (no payload).
    /// - `panic_msg`: pre-interned `"?-unreachable"` symbol that
    ///   drives the dead `panic` after the early `return` —
    ///   pinning the arm's static type to `Unknown` so the two
    ///   arms unify into `T`.
    ///
    /// From/Into cross-error conversion (`?`): when the enclosed
    /// `Result<T, E1>` is `?`-propagated through a function
    /// returning `Result<T, E2>` with `E2: From<E1>`, the error
    /// arm becomes:
    ///
    /// ```text
    /// Result::Err(__try_e_N) => {
    ///     val __try_conv_N = E2::from(__try_e_N)
    ///     val __try_err_N = Result::Err(__try_conv_N)
    ///     return __try_err_N
    ///     panic("?-unreachable")
    /// }
    /// ```
    ///
    /// `converted_binding` (`__try_conv_<n>`) and `result_binding`
    /// (`__try_err_<n>`) are the two extra pre-interned temporaries;
    /// `result_binding` keeps the AOT's `return <ident>` constraint
    /// satisfied (a bare identifier) while carrying the *converted*
    /// error. Unused (and unallocated in the desugar) when no
    /// conversion applies.
    Try {
        inner: ExprRef,
        /// Outer-scope binding for the evaluated inner value
        /// (the `Result` / `Option`). The desugar emits
        /// `val <scrutinee_binding> = <inner>` and matches on it.
        /// Used as the return value in the error arm so the AOT
        /// compiler's MVP `return <ident>` constraint is satisfied
        /// without manually re-constructing `Result::Err` /
        /// `Option::None`.
        scrutinee_binding: DefaultSymbol,
        success_binding: DefaultSymbol,
        error_binding: DefaultSymbol,
        panic_msg: DefaultSymbol,
        /// From/Into cross-error conversion temporary:
        /// `E2::from(__try_e_N)` result (`__try_conv_<n>`).
        converted_binding: DefaultSymbol,
        /// From/Into cross-error conversion temporary:
        /// reconstructed `Result::Err(E2)` (`__try_err_<n>`).
        result_binding: DefaultSymbol,
    },
    /// `a ?? b` — null-coalesce. The parser pre-interns three synthetic
    /// symbols (scrutinee / success / error bindings) because the type
    /// checker holds an immutable `&DefaultStringInterner` and can't
    /// intern new strings itself. The type checker rewrites the node
    /// in place to `Block { val t = a; match t { Some(v) => v, None => b } }`
    /// (analogously `Ok` / `Err` for `Result`), so backends never
    /// observe the NullCoalesce node and the default operand stays
    /// lazy — it is only evaluated on the `None` / `Err` path.
    NullCoalesce {
        lhs: ExprRef,
        rhs: ExprRef,
        scrutinee_binding: DefaultSymbol,
        success_binding: DefaultSymbol,
        error_binding: DefaultSymbol,
    },
    /// `P { x: 1i64, ..base }` — struct update syntax. The parser emits
    /// this node; the type checker rewrites it in place (it is the only
    /// place that knows `P`'s full field list) into
    ///
    /// ```text
    /// {
    ///     val __su_N = base
    ///     P { x: 1i64, y: __su_N.y, z: __su_N.z }
    /// }
    /// ```
    ///
    /// so backends only ever observe a `Block` holding an ordinary
    /// `StructLiteral`. The binding exists so a base with side effects
    /// (`P { x: 1i64, ..make() }`) is evaluated exactly once; like
    /// `Try`, its symbol is pre-interned by the parser because the type
    /// checker holds an immutable `&DefaultStringInterner`.
    StructUpdate {
        type_name: DefaultSymbol,
        /// Explicitly written fields, in source order. Fields absent
        /// here are taken from `base`.
        fields: Vec<(DefaultSymbol, ExprRef)>,
        base: ExprRef,
        /// Synthetic `val` binding for `base` (`__su_<n>`).
        base_binding: DefaultSymbol,
    },
}

impl Expr {
    pub fn is_block(&self) -> bool {
        matches!(self, Expr::Block(_))
    }
}

/// Which of the running program's allocation counters to read
/// (MEMORY_PROFILING M4).
///
/// One variant per `MemoryStats` field a program can hold an opinion
/// about, under exactly the name the report prints — a second
/// vocabulary for the same numbers would be a thing to get wrong.
/// `peak_at_request` is deliberately absent: it is the report's
/// reproducible stand-in for "when", not a quantity to assert on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MemStat {
    AllocCount,
    FreeCount,
    ReallocCount,
    CumulativeBytes,
    LiveBytes,
    PeakLiveBytes,
}

impl MemStat {
    pub const ALL: [MemStat; 6] = [
        MemStat::AllocCount,
        MemStat::FreeCount,
        MemStat::ReallocCount,
        MemStat::CumulativeBytes,
        MemStat::LiveBytes,
        MemStat::PeakLiveBytes,
    ];

    /// Stable selector passed to the runtime helpers, so a counter is
    /// one call with a constant argument rather than one entry point
    /// per field. Shared with `toy_prof_stat` in the `toylang_rt` crate,
    /// is why the numbering must not be reshuffled.
    pub fn code(self) -> u64 {
        match self {
            MemStat::AllocCount => 0,
            MemStat::FreeCount => 1,
            MemStat::ReallocCount => 2,
            MemStat::CumulativeBytes => 3,
            MemStat::LiveBytes => 4,
            MemStat::PeakLiveBytes => 5,
        }
    }

    pub fn from_code(code: u64) -> Option<MemStat> {
        MemStat::ALL.into_iter().find(|s| s.code() == code)
    }

    /// The source spelling. `__builtin_` prefixed: these are
    /// introspection on the runtime, not everyday I/O.
    pub fn builtin_name(self) -> &'static str {
        match self {
            MemStat::AllocCount => "__builtin_alloc_count",
            MemStat::FreeCount => "__builtin_free_count",
            MemStat::ReallocCount => "__builtin_realloc_count",
            MemStat::CumulativeBytes => "__builtin_cumulative_bytes",
            MemStat::LiveBytes => "__builtin_live_bytes",
            MemStat::PeakLiveBytes => "__builtin_peak_live_bytes",
        }
    }
}

/// SIMD intrinsics (SIMD.md Phase 2) — the operations a vector type
/// and an ordinary operator cannot express.
///
/// Lane-wise arithmetic (`a * b + a`), comparison, and the bitwise
/// operators are *not* here: they go through the regular binary /
/// unary operator paths, which is the whole point of making vectors a
/// type. What is left is construction, memory traffic, lane
/// addressing, horizontal reduction, and lane permutation —
/// seventeen names, so the AST cache schema is bumped once rather
/// than once per lane type.
///
/// Spelling: no lane-type suffix. `__simd_splat` / `__simd_load`
/// take their result type from the annotation at the call site, the
/// way `__builtin_ptr_read` already does; every other intrinsic reads
/// it off an argument that is already a vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SimdOp {
    /// `__simd_splat(x: E) -> V` — scalar into every lane.
    Splat,
    /// `__simd_load(p: ptr, i: u64) -> V` — the 128 bits starting at
    /// *element* `i`, i.e. byte offset `i * lane_bytes`. `ptr` rather
    /// than the slice SIMD.md assumed, because `&[T]` is not
    /// implemented and the stdlib's `Vec<T>` / `String` already hold
    /// their bytes behind a `ptr`.
    Load,
    /// `__simd_store(p: ptr, i: u64, v: V) -> ()` — the inverse.
    Store,
    /// `__simd_extract(v: V, k: u64) -> E` — lane `k`. `k` must be a
    /// literal in range; a non-constant index is a type error rather
    /// than a runtime bounds check.
    Extract,
    /// `__simd_insert(v: V, k: u64, x: E) -> V` — `v` with lane `k`
    /// replaced. Same constant-`k` rule as `Extract`.
    Insert,
    /// `__simd_select(mask: M, a: V, b: V) -> V` — lane-wise
    /// branch-free choice: `a` where the mask lane is all-ones, `b`
    /// where it is all-zeros.
    Select,
    /// `__simd_reduce_add(v: V) -> E` — lane 0 through n in order.
    /// The order is part of the language definition, not an
    /// implementation detail: a pairwise tree would make the float
    /// answers differ between the tree-walker and cranelift.
    ReduceAdd,
    /// `__simd_reduce_min(v: V) -> E`, same left-to-right fold.
    ReduceMin,
    /// `__simd_reduce_max(v: V) -> E`, same left-to-right fold.
    ReduceMax,
    /// `__simd_reduce_and(v: V) -> E` — integer lanes only.
    ReduceAnd,
    /// `__simd_reduce_or(v: V) -> E` — integer lanes only.
    ReduceOr,
    /// `__simd_any(mask: M) -> bool` — any lane non-zero.
    Any,
    /// `__simd_all(mask: M) -> bool` — every lane non-zero.
    All,
    /// `__simd_bitmask(v: V) -> u64` — bit `k` is the **most
    /// significant bit** of lane `k`; the bits above the lane count
    /// are zero. `__simd_any` answers *whether* a lane matched;
    /// this answers *which*, so a byte search can jump straight to
    /// the hit with `trailing_zeros` instead of re-scanning the
    /// chunk one lane at a time.
    ///
    /// The MSB rather than "non-zero" because that is the one
    /// definition every ISA implements in a single instruction
    /// (`pmovmskb` / the NEON shift-and-add sequence). On the
    /// all-ones / all-zeros masks a comparison produces — the only
    /// input this is meant for — the two definitions agree.
    Bitmask,
    /// `__simd_swizzle(a: u8x16, idx: u8x16) -> u8x16` — a runtime
    /// table lookup: result lane `i` is `a[idx[i]]`, or zero when
    /// `idx[i] >= 16`. The indices are *values*, not literals, which
    /// is what separates this from a shuffle and what makes a
    /// 16-entry lookup table (hex digits, the base64 alphabet)
    /// vectorisable.
    ///
    /// Byte lanes only, because that is the shape of `pshufb` and
    /// `tbl`. Wider lanes go through `__simd_bitcast`.
    Swizzle,
    /// `__simd_bitcast(v: V) -> W` — the same 16 bytes read as
    /// another vector type, equivalent to `__simd_store` followed by
    /// `__simd_load` at the new type. Like `__simd_splat`, the
    /// result type comes from the call site's annotation.
    Bitcast,
    /// `__simd_shuffle(a: V, b: V, [k...]) -> V` — result lane `j`
    /// is lane `k[j]` of `a` followed by `b`: an index below the
    /// lane count selects from `a`, one at or above it selects
    /// `b[k - lanes]`. The mask is a **compile-time constant** with
    /// exactly one index per lane, and an out-of-range index is a
    /// compile error rather than a zero lane — the whole point of a
    /// constant mask is that the compiler can check it.
    ///
    /// The user writes the mask as an array literal; the type
    /// checker validates it and **replaces it with two packed `u64`
    /// words** ([`SimdOp::pack_shuffle_mask`]), so no backend ever
    /// sees an array. That keeps the mask out of the value graph,
    /// where it would otherwise look like an allocation.
    ///
    /// The runtime-indexed cousin is [`SimdOp::Swizzle`].
    Shuffle,
}

impl SimdOp {
    pub const ALL: [SimdOp; 17] = [
        SimdOp::Splat,
        SimdOp::Load,
        SimdOp::Store,
        SimdOp::Extract,
        SimdOp::Insert,
        SimdOp::Select,
        SimdOp::ReduceAdd,
        SimdOp::ReduceMin,
        SimdOp::ReduceMax,
        SimdOp::ReduceAnd,
        SimdOp::ReduceOr,
        SimdOp::Any,
        SimdOp::All,
        SimdOp::Bitmask,
        SimdOp::Swizzle,
        SimdOp::Bitcast,
        SimdOp::Shuffle,
    ];

    /// The source spelling.
    pub fn builtin_name(self) -> &'static str {
        match self {
            SimdOp::Splat => "__simd_splat",
            SimdOp::Load => "__simd_load",
            SimdOp::Store => "__simd_store",
            SimdOp::Extract => "__simd_extract",
            SimdOp::Insert => "__simd_insert",
            SimdOp::Select => "__simd_select",
            SimdOp::ReduceAdd => "__simd_reduce_add",
            SimdOp::ReduceMin => "__simd_reduce_min",
            SimdOp::ReduceMax => "__simd_reduce_max",
            SimdOp::ReduceAnd => "__simd_reduce_and",
            SimdOp::ReduceOr => "__simd_reduce_or",
            SimdOp::Any => "__simd_any",
            SimdOp::All => "__simd_all",
            SimdOp::Bitmask => "__simd_bitmask",
            SimdOp::Swizzle => "__simd_swizzle",
            SimdOp::Bitcast => "__simd_bitcast",
            SimdOp::Shuffle => "__simd_shuffle",
        }
    }

    /// How many arguments the intrinsic takes.
    pub fn arity(self) -> usize {
        match self {
            SimdOp::Splat
            | SimdOp::ReduceAdd
            | SimdOp::ReduceMin
            | SimdOp::ReduceMax
            | SimdOp::ReduceAnd
            | SimdOp::ReduceOr
            | SimdOp::Any
            | SimdOp::All
            | SimdOp::Bitmask
            | SimdOp::Bitcast => 1,
            SimdOp::Load | SimdOp::Extract | SimdOp::Swizzle => 2,
            SimdOp::Store | SimdOp::Insert | SimdOp::Select | SimdOp::Shuffle => 3,
        }
    }

    /// Whether the result type comes from the call site's annotation
    /// rather than from an argument.
    pub fn needs_result_annotation(self) -> bool {
        matches!(self, SimdOp::Splat | SimdOp::Load | SimdOp::Bitcast)
    }

    /// How many arguments the node holds once the type checker has
    /// stamped in what the call site could not spell. It is
    /// [`SimdOp::arity`] for every intrinsic but `__simd_shuffle`,
    /// whose one array-literal mask becomes two packed `u64` words.
    /// (The suffix-less three grow too, but their prologue reads the
    /// stamp off and drops it before this is consulted.)
    pub fn consumed_args(self) -> usize {
        match self {
            SimdOp::Shuffle => self.arity() + 1,
            _ => self.arity(),
        }
    }

    /// Pack a `__simd_shuffle` mask into the two `u64` words the
    /// stamped call carries.
    ///
    /// Each index is stored **biased by one** in its own byte, low
    /// word first, so an unwritten byte reads back as zero and the
    /// *length* of the mask survives the round trip. Without the
    /// bias a trailing `0u64` index would be indistinguishable from
    /// padding, and the type checker could not tell a two-lane mask
    /// from a four-lane one whose last two indices are zero.
    ///
    /// `None` when the mask cannot be represented at all (empty,
    /// longer than sixteen, or an index too large for a biased
    /// byte); the caller reports that against the vector's actual
    /// lane count, which it knows and this does not.
    pub fn pack_shuffle_mask(indices: &[u64]) -> Option<(u64, u64)> {
        if indices.is_empty() || indices.len() > 16 {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (slot, index) in bytes.iter_mut().zip(indices) {
            *slot = u8::try_from(index.checked_add(1)?).ok()?;
        }
        let word = |half: &[u8]| {
            half.iter()
                .enumerate()
                .fold(0u64, |acc, (i, b)| acc | (*b as u64) << (8 * i))
        };
        Some((word(&bytes[..8]), word(&bytes[8..])))
    }

    /// The inverse of [`SimdOp::pack_shuffle_mask`]: the indices back
    /// in order, stopping at the first unwritten byte.
    pub fn unpack_shuffle_mask(lo: u64, hi: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(16);
        for j in 0..16 {
            let word = if j < 8 { lo } else { hi };
            let byte = (word >> (8 * (j % 8))) as u8;
            if byte == 0 {
                break;
            }
            out.push(byte - 1);
        }
        out
    }

    /// Whether the intrinsic only makes sense on integer lanes.
    pub fn integer_lanes_only(self) -> bool {
        matches!(self, SimdOp::ReduceAnd | SimdOp::ReduceOr)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BuiltinFunction {
    // Memory management
    HeapAlloc,    // __builtin_heap_alloc(size: u64) -> ptr
    HeapFree,     // __builtin_heap_free(pointer: ptr) -> unit
    HeapRealloc,  // __builtin_heap_realloc(pointer: ptr, new_size: u64) -> ptr

    // Pointer operations
    PtrRead,      // __builtin_ptr_read(pointer: ptr, offset: u64) -> u64
    // MEMORY-ACCESS M1: the type-argument form,
    // `__builtin_ptr_read::<T>(p, offset) -> T`.
    //
    // `PtrRead` above takes the read's width from the *surrounding
    // context* -- the annotation of the `val v: T = ...` it must be
    // bound by. That makes the read a statement rather than an
    // expression, forces a side channel through every lane
    // (`pending_annotation` in the tree-walker, `ptr_read_hints` in
    // the interpreter JIT, a syntactic special case in
    // `let_lowering.rs`), and leaves the lanes free to disagree when
    // no annotation is in reach. Naming the type at the call settles
    // all three: the width is in the operation, where the IR has
    // always carried it (`InstKind::PtrRead { elem_ty }`).
    //
    // A generic parameter arrives as `TypeDecl::Identifier(T)` and
    // resolves through the backend's active substitution, exactly as
    // for `SizeOfType`. See design-docs/MEMORY_ACCESS.md.
    PtrReadTyped(TypeDecl),
    PtrWrite,     // __builtin_ptr_write(pointer: ptr, offset: u64, value: u64) -> unit
    PtrIsNull,    // __builtin_ptr_is_null(pointer: ptr) -> bool
    PtrEq,        // __builtin_ptr_eq(a: ptr, b: ptr) -> bool
    NullPtr,      // __builtin_null_ptr() -> ptr — portable null pointer constant
    PtrOffset,    // __builtin_ptr_offset(base: ptr, offset: u64) -> ptr — interior pointer

    // DATA-ORIENTED Phase 2: the column-split heap buffer behind
    // `SoaVec<T>` (`core/std/collections/soa_vec.t`). One allocation
    // is divided into one column per leaf scalar of `T`, so leaf `j`
    // of element `i` lives at
    //
    //     byte_off = prefix_j * cap + i * stride_j
    //
    // where `stride_j` is the leaf's own width and `prefix_j` the sum
    // of the widths before it — both constants once `T` is
    // monomorphised, with only `cap` arriving at runtime. That keeps
    // leaf *selection* a compile-time decision, which is what the
    // rejected reflection builtins (`__builtin_field_offset(T, i)`)
    // could not promise; see `design-docs/DATA_ORIENTED.md`.
    //
    // Both expand into the same per-leaf `PtrRead` / `PtrWrite` the
    // compound `__builtin_ptr_read` / `__builtin_ptr_write` already
    // emit, so the IR, codegen and the IR VM learn nothing about SoA.
    SoaRead,  // __builtin_soa_read(base: ptr, index: u64, cap: u64) -> T
              // `T` comes from the annotation, exactly as for
              // `__builtin_ptr_read`.
    SoaWrite, // __builtin_soa_write(base: ptr, index: u64, cap: u64, value: T) -> unit

    // String → pointer conversion. Returns a pointer to the string's
    // UTF-8 bytes (NUL-terminated). The pointer is valid for the
    // lifetime of the string. Backend semantics:
    //   - AOT: identity — `Type::Str` is already an i64-sized pointer
    //     to a NUL-terminated `.rodata` blob (or heap copy).
    //   - JIT: identity, same as AOT.
    //   - Interpreter: allocates a heap buffer via `heap_manager`,
    //     stores each byte as `Object::U8` in `typed_slots` so
    //     `__builtin_ptr_read(p, i)` with a `val: u8` annotation
    //     returns the byte at offset i. NUL terminator at index `len`.
    //
    // Use case: low-level FFI / interop where the user needs to walk
    // the bytes of a string with `__builtin_ptr_read` (and counterpart
    // `__builtin_ptr_write` / `mem_copy` for buffers built from
    // `__builtin_heap_alloc`).
    StrToPtr,     // __builtin_str_to_ptr(s: str) -> ptr

    // String → byte length. Returns the number of UTF-8 bytes in
    // the string (not the character count). O(1) at every backend:
    //   - AOT: loads the 8-byte length header that
    //     `declare_print_string` lays out immediately before the
    //     string's UTF-8 bytes (str values point at the byte start;
    //     the length lives at offset -8).
    //   - JIT: silent fallback to interpreter (no `ScalarTy::Str`
    //     in the JIT IR yet).
    //   - Interpreter: returns `s.bytes().len() as u64` from the
    //     underlying `Object::String` / `Object::ConstString`.
    StrLen,       // __builtin_str_len(s: str) -> u64

    // Bytes → string. The inverse of `StrToPtr`: copies `len` bytes
    // from `p` into a fresh `str`. Nothing else in the language can
    // produce a `str` from data computed at runtime, which is what
    // kept the stdlib's own `String` from being printable — it holds
    // its bytes in a heap buffer and had no way to hand them back as
    // the type interpolation consumes. Backend semantics:
    //   - AOT / compiler JIT: `toy_str_alloc`, the same helper that
    //     already builds the results of `concat` and `to_string`.
    //   - Interpreter: reads the bytes back out of the heap (typed
    //     slots first, raw memory otherwise) into an `Object::String`.
    //   - Interpreter JIT: rejected, like `StrToPtr`.
    //
    // The bytes are copied, so the `str` outlives any later write to
    // the buffer. UTF-8 is not validated: the buffer is the program's
    // to get right, exactly as with `__builtin_ptr_write`.
    StrFromBytes, // __builtin_str_from_bytes(p: ptr, len: u64) -> str

    // Memory operations
    MemCopy,      // __builtin_mem_copy(src: ptr, dest: ptr, size: u64) -> unit
    MemMove,      // __builtin_mem_move(src: ptr, dest: ptr, size: u64) -> unit
    MemSet,       // __builtin_mem_set(pointer: ptr, value: u8, size: u64) -> unit

    // Allocator context
    CurrentAllocator,      // __builtin_current_allocator() -> Allocator on top of stack (default handle when unset)
    DefaultAllocator,      // __builtin_default_allocator() -> Allocator referring to the global/default allocator

    // Allocation counters, readable mid-run (MEMORY_PROFILING M4).
    // `__builtin_live_bytes()` and friends, all `() -> u64`. The point
    // is that `requires` / `ensures` and `test` blocks can assert on
    // memory, so these must answer truthfully in an ordinary run —
    // no profiling flag involved. See `MemStatEnable` in the IR for
    // how the compiled runtime is told to keep counting.
    MemStat(MemStat),

    // Allocator layout registry (MEMORY_PROFILING M3 residual).
    // `__builtin_record_allocator_layout(name: str, managed: u64,
    // live: u64, free_blocks: u64, largest: u64) -> unit` registers a
    // region-owning allocator's final layout with the runtime, so
    // `--profile=mem` can fold fragmentation into its report without
    // the runtime reaching back into a toylang object. The stdlib's
    // region allocator (`SlotRegion`) calls it from its `Drop`, which
    // is what makes the report automatic.
    RecordAllocatorLayout,

    // Output (exposed without the `__builtin_` prefix since they are
    // everyday user-facing operations, not low-level intrinsics).
    Print,   // print(value) -> unit (no trailing newline)
    Println, // println(value) -> unit (trailing newline)
    // RUNTIME-LIB P0-A: the same two on the error stream. Same
    // formatting (`Display` / `to_str` dispatch included) and the same
    // any-type argument — only the stream differs, which is why they
    // are builtins beside `print` rather than `str`-taking functions
    // in `core/std/io.t`.
    EPrint,   // eprint(value) -> unit (stderr, no trailing newline)
    EPrintln, // eprintln(value) -> unit (stderr, trailing newline)

    // Abrupt termination. `panic(msg: str)` aborts the current run with
    // the supplied message; the type-checker pretends the call returns a
    // type compatible with any context (Unknown), so it can appear in
    // value positions like `if c { 5i64 } else { panic("bad") }`.
    Panic,

    // Conditional abort. `assert(cond: bool, msg: str)` panics with `msg`
    // when `cond` is false and is a no-op otherwise; the return type is
    // Unit. Sugar for `if !cond { panic(msg) }` but with a clearer
    // intent at call sites and a single point to disable in the future.
    Assert,

    /// SIMD intrinsics (SIMD.md Phase 2). See [`SimdOp`] for the
    /// list and for why lane-wise arithmetic is *not* in it.
    Simd(SimdOp),

    // Type introspection
    SizeOf,  // __builtin_sizeof(value) -> u64 — size in bytes of the argument's type
    // POINTER P1: the type-argument form, `__builtin_sizeof::<T>() -> u64`.
    // Carries the written type instead of a probe value, so an allocator
    // can size a slot without a representative value in hand
    // (`__builtin_heap_alloc(__builtin_sizeof::<T>() * n)`). A generic
    // parameter arrives as `TypeDecl::Identifier(T)` (the turbofish type
    // is parsed without generic context) and resolves through the
    // backend's active substitution; a named type resolves through the
    // struct / enum tables. The value form stays for probe-style reads.
    SizeOfType(TypeDecl),

    // Display formatting. `__builtin_to_string(value) -> str`
    // produces the same display string `print` / `println` would
    // emit for `value` (via `Object::to_display_string` in the
    // interpreter). Primary user is the parser-level desugaring of
    // string interpolation: `"hello {x}"` lowers to
    // `"hello ".concat(__builtin_to_string(x))`. Any value is
    // accepted; type-check side reports `str` regardless of the
    // argument's type.
    ToString,
    /// `__builtin_backtrace() -> str` — how the program got here
    /// (DEBUG-OBS D5).
    ///
    /// The same text a panic prints, available without dying. Each
    /// engine reads its own call stack — the tree-walker's frames, the
    /// IR VM's, or the shadow stack the compiled backends keep — and
    /// renders through the one shared formatter.
    Backtrace,

    // STR-INTERP-FMT: `__builtin_format(value, spec: u64) -> str`.
    // Same rendering as `ToString`, plus width / alignment /
    // zero-padding / precision / radix taken from `spec` — a
    // compile-time constant packed by
    // `frontend::format_spec::FormatSpec::pack`, never a runtime
    // value. Emitted only by the interpolation desugaring for
    // `"{x:.2}"`-style segments; a spec that asks for nothing lowers
    // to a plain `ToString` instead. The value must be a primitive
    // (see `docs/language.md`): a struct / tuple / enum with a spec
    // is a type error rather than a silently ignored spec.
    Format,

    // Integer math (user-facing; same shape as `print`/`println`/`panic`/
    // `assert` — everyday operations rather than low-level intrinsics).
    // `abs(x)` accepts `i64` and returns `i64` (matches Rust's
    // `i64::wrapping_abs` for `i64::MIN`). `min(a, b)` / `max(a, b)`
    // accept either `i64` or `u64` and return the shared input type.
    Abs,
    Min,
    Max,

    // NOTE: f64 math intrinsics (sin/cos/tan/log/log2/exp/floor/ceil
    // /pow/sqrt) used to live here as `BuiltinFunction::*` variants
    // dispatched by the parser-recognised `__builtin_*_f64` names.
    // Phase 4 of the math externalisation work removed them — the
    // `math` module now declares each as `extern fn __extern_*_f64`
    // and every backend dispatches through the extern path
    // (`evaluation/extern_math` registry / JIT extern dispatch table /
    // AOT libm import). User code calls `math::sin(x)` etc. as
    // before; the `__builtin_*_f64` names are no longer recognised.
    //
    // `Abs` / `Min` / `Max` are still here because integer math
    // doesn't have an extern dispatch path yet — Phase 5 moves
    // those onto the same machinery.
}

#[derive(Debug, Clone)]
pub struct BuiltinFunctionSymbols {
    // Memory management
    pub heap_alloc: DefaultSymbol,
    pub heap_free: DefaultSymbol,
    pub heap_realloc: DefaultSymbol,

    // Pointer operations
    pub ptr_read: DefaultSymbol,
    pub ptr_write: DefaultSymbol,
    pub ptr_is_null: DefaultSymbol,
    pub ptr_eq: DefaultSymbol,
    pub null_ptr: DefaultSymbol,
    pub ptr_offset: DefaultSymbol,

    // DATA-ORIENTED Phase 2: the `SoaVec<T>` column accessors.
    pub soa_read: DefaultSymbol,
    pub soa_write: DefaultSymbol,

    // String → pointer conversion (interop with raw byte access).
    pub str_to_ptr: DefaultSymbol,
    pub str_len: DefaultSymbol,
    pub str_from_bytes: DefaultSymbol,

    // Memory operations
    pub mem_copy: DefaultSymbol,
    pub mem_move: DefaultSymbol,
    pub mem_set: DefaultSymbol,

    // Allocator context
    pub current_allocator: DefaultSymbol,
    pub default_allocator: DefaultSymbol,

    /// Allocation counters, in `MemStat::ALL` order.
    pub mem_stats: Vec<DefaultSymbol>,

    /// Allocator layout registry (MEMORY_PROFILING M3 residual).
    pub record_allocator_layout: DefaultSymbol,

    /// SIMD intrinsics, in `SimdOp::ALL` order.
    pub simd_ops: Vec<DefaultSymbol>,

    // Output
    pub print: DefaultSymbol,
    pub println: DefaultSymbol,
    pub eprint: DefaultSymbol,
    pub eprintln: DefaultSymbol,

    // Termination
    pub panic: DefaultSymbol,
    pub assert: DefaultSymbol,

    // Type introspection
    pub sizeof: DefaultSymbol,

    // Display formatting (powers string interpolation).
    pub to_string: DefaultSymbol,
    // Display formatting with a packed format spec (STR-INTERP-FMT).
    pub format: DefaultSymbol,

    // Integer math (user-facing names).
    pub abs: DefaultSymbol,
    pub min: DefaultSymbol,
    pub max: DefaultSymbol,

    // Source-location introspection. Each of these is recognised at
    // parser time and substituted in-place with the corresponding
    // literal (line / column as `u64`, file as `str`); they never
    // reach `symbol_to_builtin` or any backend. Powers `__builtin_dbg`
    // and `assert_eq` / `assert_ne`.
    pub source_file: DefaultSymbol,
    pub source_line: DefaultSymbol,
    pub source_column: DefaultSymbol,
    /// DEBUG-OBS D5: the enclosing function's name, substituted at
    /// parse time like the three above. Costs nothing at run time —
    /// the parser already knows which body it is inside, and the
    /// answer is a string literal by the time any backend sees it.
    pub function_name: DefaultSymbol,
    /// DEBUG-OBS D5: `__builtin_backtrace()`. Unlike the three above
    /// this one is a real builtin — the answer is only known while the
    /// program runs.
    pub backtrace: DefaultSymbol,

    // `__builtin_dbg(expr)` — parser-level macro that captures the
    // source text of `expr` and lowers to a print + return-value
    // block. Recognised by symbol equality in the parser.
    pub dbg: DefaultSymbol,

    // `assert_eq(a, b)` / `assert_ne(a, b)` — parser-level desugar
    // emitting an inequality check + formatted `panic` on failure.
    pub assert_eq: DefaultSymbol,
    pub assert_ne: DefaultSymbol,
    // NOTE: f64 math symbol fields (`pow` / `sqrt` / `sin` / `cos` /
    // `tan` / `log` / `log2` / `exp` / `floor` / `ceil`) lived here
    // before Phase 4. They were the parser-side recogniser for the
    // legacy `__builtin_*_f64` names. After Phase 4, the math
    // module declares each as `extern fn __extern_*_f64` so the
    // recognition happens through the regular function table —
    // these dedicated symbol fields are no longer needed.
}

impl BuiltinFunctionSymbols {
    pub fn new(interner: &mut DefaultStringInterner) -> Self {
        Self {
            heap_alloc: interner.get_or_intern("__builtin_heap_alloc"),
            heap_free: interner.get_or_intern("__builtin_heap_free"),
            heap_realloc: interner.get_or_intern("__builtin_heap_realloc"),
            ptr_read: interner.get_or_intern("__builtin_ptr_read"),
            ptr_write: interner.get_or_intern("__builtin_ptr_write"),
            ptr_is_null: interner.get_or_intern("__builtin_ptr_is_null"),
            ptr_eq: interner.get_or_intern("__builtin_ptr_eq"),
            null_ptr: interner.get_or_intern("__builtin_null_ptr"),
            ptr_offset: interner.get_or_intern("__builtin_ptr_offset"),
            soa_read: interner.get_or_intern("__builtin_soa_read"),
            soa_write: interner.get_or_intern("__builtin_soa_write"),
            str_to_ptr: interner.get_or_intern("__builtin_str_to_ptr"),
            str_len: interner.get_or_intern("__builtin_str_len"),
            str_from_bytes: interner.get_or_intern("__builtin_str_from_bytes"),
            mem_copy: interner.get_or_intern("__builtin_mem_copy"),
            mem_move: interner.get_or_intern("__builtin_mem_move"),
            mem_set: interner.get_or_intern("__builtin_mem_set"),
            current_allocator: interner.get_or_intern("__builtin_current_allocator"),
            default_allocator: interner.get_or_intern("__builtin_default_allocator"),
            mem_stats: MemStat::ALL
                .iter()
                .map(|s| interner.get_or_intern(s.builtin_name()))
                .collect(),
            record_allocator_layout: interner.get_or_intern("__builtin_record_allocator_layout"),
            simd_ops: SimdOp::ALL
                .iter()
                .map(|op| interner.get_or_intern(op.builtin_name()))
                .collect(),
            // I/O builtins are user-facing, so they keep the plain names
            // `print` and `println` instead of the `__builtin_` prefix used
            // for low-level memory primitives.
            print: interner.get_or_intern("print"),
            println: interner.get_or_intern("println"),
            eprint: interner.get_or_intern("eprint"),
            eprintln: interner.get_or_intern("eprintln"),
            panic: interner.get_or_intern("panic"),
            assert: interner.get_or_intern("assert"),
            sizeof: interner.get_or_intern("__builtin_sizeof"),
            to_string: interner.get_or_intern("__builtin_to_string"),
            format: interner.get_or_intern("__builtin_format"),
            // Integer math intrinsics. The user-facing entry points
            // are `math::abs` / `math::min_*` / `math::max_*` in
            // `interpreter/modules/math/math.t`; the wrappers forward
            // to these symbols. The f64 family (sin/cos/tan/log/log2
            // /exp/floor/ceil/pow/sqrt) used to live here too — Phase 4
            // moved them onto `extern fn __extern_*_f64` declarations
            // in math.t so they no longer need a parser-level symbol.
            abs: interner.get_or_intern("__builtin_abs"),
            min: interner.get_or_intern("__builtin_min"),
            max: interner.get_or_intern("__builtin_max"),
            source_file: interner.get_or_intern("__builtin_source_file"),
            source_line: interner.get_or_intern("__builtin_source_line"),
            source_column: interner.get_or_intern("__builtin_source_column"),
            function_name: interner.get_or_intern("__builtin_function_name"),
            backtrace: interner.get_or_intern("__builtin_backtrace"),
            dbg: interner.get_or_intern("__builtin_dbg"),
            assert_eq: interner.get_or_intern("assert_eq"),
            assert_ne: interner.get_or_intern("assert_ne"),
        }
    }

    pub fn symbol_to_builtin(&self, symbol: DefaultSymbol) -> Option<BuiltinFunction> {
        if symbol == self.heap_alloc { Some(BuiltinFunction::HeapAlloc) }
        else if symbol == self.heap_free { Some(BuiltinFunction::HeapFree) }
        else if symbol == self.heap_realloc { Some(BuiltinFunction::HeapRealloc) }
        else if symbol == self.ptr_read { Some(BuiltinFunction::PtrRead) }
        else if symbol == self.ptr_write { Some(BuiltinFunction::PtrWrite) }
        else if symbol == self.ptr_is_null { Some(BuiltinFunction::PtrIsNull) }
        else if symbol == self.ptr_eq { Some(BuiltinFunction::PtrEq) }
        else if symbol == self.null_ptr { Some(BuiltinFunction::NullPtr) }
        else if symbol == self.ptr_offset { Some(BuiltinFunction::PtrOffset) }
        else if symbol == self.soa_read { Some(BuiltinFunction::SoaRead) }
        else if symbol == self.soa_write { Some(BuiltinFunction::SoaWrite) }
        else if symbol == self.str_to_ptr { Some(BuiltinFunction::StrToPtr) }
        else if symbol == self.str_len { Some(BuiltinFunction::StrLen) }
        else if symbol == self.str_from_bytes { Some(BuiltinFunction::StrFromBytes) }
        else if symbol == self.mem_copy { Some(BuiltinFunction::MemCopy) }
        else if symbol == self.mem_move { Some(BuiltinFunction::MemMove) }
        else if symbol == self.mem_set { Some(BuiltinFunction::MemSet) }
        else if symbol == self.current_allocator { Some(BuiltinFunction::CurrentAllocator) }
        else if symbol == self.default_allocator { Some(BuiltinFunction::DefaultAllocator) }
        else if symbol == self.print { Some(BuiltinFunction::Print) }
        else if symbol == self.println { Some(BuiltinFunction::Println) }
        else if symbol == self.eprint { Some(BuiltinFunction::EPrint) }
        else if symbol == self.eprintln { Some(BuiltinFunction::EPrintln) }
        else if symbol == self.panic { Some(BuiltinFunction::Panic) }
        else if symbol == self.assert { Some(BuiltinFunction::Assert) }
        else if symbol == self.sizeof { Some(BuiltinFunction::SizeOf) }
        else if symbol == self.to_string { Some(BuiltinFunction::ToString) }
        else if symbol == self.backtrace { Some(BuiltinFunction::Backtrace) }
        else if symbol == self.format { Some(BuiltinFunction::Format) }
        else if symbol == self.record_allocator_layout { Some(BuiltinFunction::RecordAllocatorLayout) }
        else if symbol == self.abs { Some(BuiltinFunction::Abs) }
        else if symbol == self.min { Some(BuiltinFunction::Min) }
        else if symbol == self.max { Some(BuiltinFunction::Max) }
        else {
            self.mem_stats
                .iter()
                .position(|s| *s == symbol)
                .map(|i| BuiltinFunction::MemStat(MemStat::ALL[i]))
                .or_else(|| {
                    self.simd_ops
                        .iter()
                        .position(|s| *s == symbol)
                        .map(|i| BuiltinFunction::Simd(SimdOp::ALL[i]))
                })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BuiltinMethod {
    // Universal methods (available for all types)
    IsNull,       // any.is_null() -> bool

    // String methods
    StrLen,       // str.len() -> u64
    StrConcat,    // str.concat(str) -> str
    StrSubstring, // str.substring(u64, u64) -> str
    StrContains,  // str.contains(str) -> bool
    StrSplit,     // str.split(str) -> [str]
    StrTrim,      // str.trim() -> str
    StrToUpper,   // str.to_upper() -> str
    StrToLower,   // str.to_lower() -> str

    // NOTE: `I64Abs` / `F64Abs` / `F64Sqrt` lived here as hardcoded
    // numeric value-method dispatchers. Step E (extension-trait
    // migration) replaced them with regular `impl Abs for {i64,f64}`
    // / `impl Sqrt for f64` blocks in the always-loaded prelude
    // (`interpreter/src/prelude.t`); `x.abs()` / `x.sqrt()` now
    // resolve through the same `method_registry` user-defined
    // extension traits go through. Step F removed the variants.
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UnaryOp {
    BitwiseNot,  // ~
    LogicalNot,  // !
    Negate,      // -expr (sign flip for signed integer types)
    // REF-Stage-2: explicit borrow expressions. `&expr` produces
    // `&T`, `&mut expr` produces `&mut T`. Both are erased to the
    // inner expression at lower level (interpreter / AOT) — they
    // exist purely to satisfy the type-checker when call sites
    // don't want to rely on auto-borrow.
    Borrow,      // &expr
    BorrowMut,   // &mut expr
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Operator {
    IAdd,
    ISub,
    IMul,
    IDiv,
    IMod,

    // Comparison operator
    EQ, // ==
    NE, // !=
    LT, // <
    LE, // <=
    GT, // >
    GE, // >=

    LogicalAnd,
    LogicalOr,

    // Bitwise operators
    BitwiseAnd,    // &
    BitwiseOr,     // |
    BitwiseXor,    // ^
    LeftShift,     // <<
    RightShift,    // >>
}

#[derive(Debug)]
pub struct BinaryExpr {
    pub op: Operator,
    pub lhs: ExprRef,
    pub rhs: ExprRef,
}
