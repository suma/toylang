use string_interner::{DefaultSymbol, DefaultStringInterner};
use std::rc::Rc;
use crate::type_decl::TypeDecl;
use super::{ExprRef, StmtRef, StructField, Visibility, MethodFunction};

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
    Wildcard, // _
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
    Match(ExprRef, Vec<(Pattern, ExprRef)>),  // match scrutinee { pat => body, ... }
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
