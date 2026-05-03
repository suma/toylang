use string_interner::{DefaultSymbol, DefaultStringInterner};
use std::rc::Rc;
use crate::type_decl::TypeDecl;
use super::{ExprRef, StmtRef, StructField, Visibility, MethodFunction, TraitMethodSignature};

#[derive(Debug, Clone, PartialEq)]
pub enum SliceType {
    SingleElement,    // a[index]
    RangeSlice,       // a[start..end], a[start..], a[..end], a[..]
}

#[derive(Debug, Clone, PartialEq)]
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
pub enum Stmt {
    Expression(ExprRef),
    Val(DefaultSymbol, Option<TypeDecl>, ExprRef),
    Var(DefaultSymbol, Option<TypeDecl>, Option<ExprRef>),
    Return(Option<ExprRef>),
    Break,
    Continue,
    For(DefaultSymbol, ExprRef, ExprRef, ExprRef), // str, start, end, block
    While(ExprRef, ExprRef), // cond, block
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
        methods: Vec<Rc<MethodFunction>>,
        /// `Some(trait_name)` for `impl <Trait> for <Type>`, `None` for an
        /// inherent `impl <Type>`. Trait conformance is recorded by the
        /// type-checker; runtime dispatch sees the methods either way.
        trait_name: Option<DefaultSymbol>,
    },
    /// `trait Name { fn m(self: Self, ...) -> T; ... }` — declares a set of
    /// method signatures that conforming structs must provide. Trait methods
    /// have no body. Generics on the trait itself, default methods, and trait
    /// inheritance are out of scope for the initial implementation.
    TraitDecl {
        name: DefaultSymbol,
        methods: Vec<TraitMethodSignature>,
        visibility: Visibility,
    },
    EnumDecl {
        name: DefaultSymbol,
        generic_params: Vec<DefaultSymbol>,  // empty for non-generic enums
        variants: Vec<EnumVariantDef>,
        visibility: Visibility,
    },
}

/// Phase 2 enum variant: a name plus an optional tuple-style payload. An empty
/// `payload_types` vector is a unit variant.
#[derive(Debug, PartialEq, Clone)]
pub struct EnumVariantDef {
    pub name: DefaultSymbol,
    pub payload_types: Vec<TypeDecl>,
}

/// Patterns for `match` arms. Patterns compose recursively — tuple-variant
/// sub-patterns can themselves be any Pattern, enabling nested matches such
/// as `Some(Some(x))` or `Some(Color::Red)`.
#[derive(Debug, PartialEq, Clone)]
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
    Wildcard, // _
}

/// One arm of a `match` expression. The optional `guard` is a boolean
/// expression evaluated **after** the pattern matches and the pattern's
/// bindings are in scope; an arm with a `false` guard is skipped, so
/// guarded arms count as refutable for exhaustiveness regardless of
/// pattern shape.
#[derive(Debug, PartialEq, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<ExprRef>,
    pub body: ExprRef,
}

#[derive(Debug, PartialEq, Clone)]
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
}

impl Expr {
    pub fn is_block(&self) -> bool {
        matches!(self, Expr::Block(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BuiltinFunction {
    // Memory management
    HeapAlloc,    // __builtin_heap_alloc(size: u64) -> ptr
    HeapFree,     // __builtin_heap_free(pointer: ptr) -> unit
    HeapRealloc,  // __builtin_heap_realloc(pointer: ptr, new_size: u64) -> ptr

    // Pointer operations
    PtrRead,      // __builtin_ptr_read(pointer: ptr, offset: u64) -> u64
    PtrWrite,     // __builtin_ptr_write(pointer: ptr, offset: u64, value: u64) -> unit
    PtrIsNull,    // __builtin_ptr_is_null(pointer: ptr) -> bool

    // Memory operations
    MemCopy,      // __builtin_mem_copy(src: ptr, dest: ptr, size: u64) -> unit
    MemMove,      // __builtin_mem_move(src: ptr, dest: ptr, size: u64) -> unit
    MemSet,       // __builtin_mem_set(pointer: ptr, value: u8, size: u64) -> unit

    // Allocator context
    CurrentAllocator,      // __builtin_current_allocator() -> Allocator on top of stack (default handle when unset)
    DefaultAllocator,      // __builtin_default_allocator() -> Allocator referring to the global/default allocator
    ArenaAllocator,        // __builtin_arena_allocator() -> Allocator backed by an arena (bulk-free on drop)
    FixedBufferAllocator,  // __builtin_fixed_buffer_allocator(capacity: u64) -> Allocator that fails when quota is exceeded

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

    // Memory operations
    pub mem_copy: DefaultSymbol,
    pub mem_move: DefaultSymbol,
    pub mem_set: DefaultSymbol,

    // Allocator context
    pub current_allocator: DefaultSymbol,
    pub default_allocator: DefaultSymbol,
    pub arena_allocator: DefaultSymbol,
    pub fixed_buffer_allocator: DefaultSymbol,

    // Output
    pub print: DefaultSymbol,
    pub println: DefaultSymbol,

    // Termination
    pub panic: DefaultSymbol,
    pub assert: DefaultSymbol,

    // Type introspection
    pub sizeof: DefaultSymbol,

    // Integer math (user-facing names).
    pub abs: DefaultSymbol,
    pub min: DefaultSymbol,
    pub max: DefaultSymbol,
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
            mem_copy: interner.get_or_intern("__builtin_mem_copy"),
            mem_move: interner.get_or_intern("__builtin_mem_move"),
            mem_set: interner.get_or_intern("__builtin_mem_set"),
            current_allocator: interner.get_or_intern("__builtin_current_allocator"),
            default_allocator: interner.get_or_intern("__builtin_default_allocator"),
            arena_allocator: interner.get_or_intern("__builtin_arena_allocator"),
            fixed_buffer_allocator: interner.get_or_intern("__builtin_fixed_buffer_allocator"),
            // I/O builtins are user-facing, so they keep the plain names
            // `print` and `println` instead of the `__builtin_` prefix used
            // for low-level memory primitives.
            print: interner.get_or_intern("print"),
            println: interner.get_or_intern("println"),
            panic: interner.get_or_intern("panic"),
            assert: interner.get_or_intern("assert"),
            sizeof: interner.get_or_intern("__builtin_sizeof"),
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
        }
    }

    pub fn symbol_to_builtin(&self, symbol: DefaultSymbol) -> Option<BuiltinFunction> {
        if symbol == self.heap_alloc { Some(BuiltinFunction::HeapAlloc) }
        else if symbol == self.heap_free { Some(BuiltinFunction::HeapFree) }
        else if symbol == self.heap_realloc { Some(BuiltinFunction::HeapRealloc) }
        else if symbol == self.ptr_read { Some(BuiltinFunction::PtrRead) }
        else if symbol == self.ptr_write { Some(BuiltinFunction::PtrWrite) }
        else if symbol == self.ptr_is_null { Some(BuiltinFunction::PtrIsNull) }
        else if symbol == self.mem_copy { Some(BuiltinFunction::MemCopy) }
        else if symbol == self.mem_move { Some(BuiltinFunction::MemMove) }
        else if symbol == self.mem_set { Some(BuiltinFunction::MemSet) }
        else if symbol == self.current_allocator { Some(BuiltinFunction::CurrentAllocator) }
        else if symbol == self.default_allocator { Some(BuiltinFunction::DefaultAllocator) }
        else if symbol == self.arena_allocator { Some(BuiltinFunction::ArenaAllocator) }
        else if symbol == self.fixed_buffer_allocator { Some(BuiltinFunction::FixedBufferAllocator) }
        else if symbol == self.print { Some(BuiltinFunction::Print) }
        else if symbol == self.println { Some(BuiltinFunction::Println) }
        else if symbol == self.panic { Some(BuiltinFunction::Panic) }
        else if symbol == self.assert { Some(BuiltinFunction::Assert) }
        else if symbol == self.sizeof { Some(BuiltinFunction::SizeOf) }
        else if symbol == self.abs { Some(BuiltinFunction::Abs) }
        else if symbol == self.min { Some(BuiltinFunction::Min) }
        else if symbol == self.max { Some(BuiltinFunction::Max) }
        else { None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
pub enum UnaryOp {
    BitwiseNot,  // ~
    LogicalNot,  // !
    Negate,      // -expr (sign flip for signed integer types)
}

#[derive(Debug, Clone, PartialEq)]
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
