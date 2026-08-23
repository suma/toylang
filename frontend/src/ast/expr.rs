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
    Float64(f64),
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BuiltinFunction {
    // Memory management
    HeapAlloc,    // __builtin_heap_alloc(size: u64) -> ptr
    HeapFree,     // __builtin_heap_free(pointer: ptr) -> unit
    HeapRealloc,  // __builtin_heap_realloc(pointer: ptr, new_size: u64) -> ptr

    // Pointer operations
    PtrRead,      // __builtin_ptr_read(pointer: ptr, offset: u64) -> u64
    PtrWrite,     // __builtin_ptr_write(pointer: ptr, offset: u64, value: u64) -> unit
    PtrIsNull,    // __builtin_ptr_is_null(pointer: ptr) -> bool
    PtrEq,        // __builtin_ptr_eq(a: ptr, b: ptr) -> bool
    NullPtr,      // __builtin_null_ptr() -> ptr — portable null pointer constant
    PtrOffset,    // __builtin_ptr_offset(base: ptr, offset: u64) -> ptr — interior pointer

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

    // Type introspection
    SizeOf,  // __builtin_sizeof(value) -> u64 — size in bytes of the argument's type

    // Display formatting. `__builtin_to_string(value) -> str`
    // produces the same display string `print` / `println` would
    // emit for `value` (via `Object::to_display_string` in the
    // interpreter). Primary user is the parser-level desugaring of
    // string interpolation: `"hello {x}"` lowers to
    // `"hello ".concat(__builtin_to_string(x))`. Any value is
    // accepted; type-check side reports `str` regardless of the
    // argument's type.
    ToString,

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

    // Output
    pub print: DefaultSymbol,
    pub println: DefaultSymbol,

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
            // I/O builtins are user-facing, so they keep the plain names
            // `print` and `println` instead of the `__builtin_` prefix used
            // for low-level memory primitives.
            print: interner.get_or_intern("print"),
            println: interner.get_or_intern("println"),
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
        else if symbol == self.panic { Some(BuiltinFunction::Panic) }
        else if symbol == self.assert { Some(BuiltinFunction::Assert) }
        else if symbol == self.sizeof { Some(BuiltinFunction::SizeOf) }
        else if symbol == self.to_string { Some(BuiltinFunction::ToString) }
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
