use std::rc::Rc;
use string_interner::{DefaultSymbol, DefaultStringInterner};
use crate::type_checker::{Acceptable, TypeCheckError, SourceLocation};
use crate::type_decl::TypeDecl;
use crate::visitor::AstVisitor;

#[derive (Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExprRef(pub u32);
#[derive(Debug, PartialEq, Clone)]
pub struct ExprPool(pub Vec<Expr>);

/// Location information storage for AST nodes
#[derive(Debug, PartialEq, Clone)]
pub struct LocationPool {
    pub expr_locations: Vec<Option<SourceLocation>>,
    pub stmt_locations: Vec<Option<SourceLocation>>,
}

impl Default for LocationPool {
    fn default() -> Self {
        Self::new()
    }
}

impl LocationPool {
    pub fn new() -> Self {
        Self {
            expr_locations: Vec::new(),
            stmt_locations: Vec::new(),
        }
    }
    
    pub fn with_capacity(expr_cap: usize, stmt_cap: usize) -> Self {
        Self {
            expr_locations: Vec::with_capacity(expr_cap),
            stmt_locations: Vec::with_capacity(stmt_cap),
        }
    }
    
    pub fn add_expr_location(&mut self, location: Option<SourceLocation>) {
        self.expr_locations.push(location);
    }
    
    pub fn add_stmt_location(&mut self, location: Option<SourceLocation>) {
        self.stmt_locations.push(location);
    }
    
    pub fn get_expr_location(&self, expr_ref: &ExprRef) -> Option<&SourceLocation> {
        self.expr_locations.get(expr_ref.to_index())?.as_ref()
    }
    
    pub fn get_stmt_location(&self, stmt_ref: &StmtRef) -> Option<&SourceLocation> {
        self.stmt_locations.get(stmt_ref.to_index())?.as_ref()
    }
    
    pub fn set_expr_location(&mut self, expr_ref: &ExprRef, location: SourceLocation) {
        if let Some(loc) = self.expr_locations.get_mut(expr_ref.to_index()) {
            *loc = Some(location);
        }
    }
    
    pub fn set_stmt_location(&mut self, stmt_ref: &StmtRef, location: SourceLocation) {
        if let Some(loc) = self.stmt_locations.get_mut(stmt_ref.to_index()) {
            *loc = Some(location);
        }
    }
}

#[derive (Clone, Copy, Debug, PartialEq)]
pub struct StmtRef(pub u32);
#[derive(Debug, PartialEq, Clone)]
pub struct StmtPool(pub Vec<Stmt>);

#[derive(Debug, PartialEq, Clone)]
pub struct Node {
    pub start: usize,
    pub end: usize,
}

impl Node {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
    
    pub fn to_source_location(&self, line: u32, column: u32) -> SourceLocation {
        SourceLocation {
            line,
            column,
            offset: self.start as u32,
        }
    }
}

/// AST node with location information
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

impl ExprRef {
    pub fn to_index(&self) -> usize {
        self.0 as usize
    }
}

impl StmtRef {
    pub fn to_index(&self) -> usize {
        self.0 as usize
    }
}

impl Default for ExprPool {
    fn default() -> Self {
        Self::new()
    }
}

impl ExprPool {
    pub fn new() -> ExprPool {
        ExprPool(Vec::new())
    }
    pub fn with_capacity(cap: usize) -> ExprPool {
        ExprPool(Vec::with_capacity(cap))
    }

    pub fn push(&mut self, expr: Expr) {
        self.0.push(expr);
    }

    pub fn add(&mut self, expr: Expr) -> ExprRef {
        let len = self.0.len();
        self.0.push(expr);
        ExprRef(len as u32)
    }

    pub fn get(&self, i: usize) -> Option<&Expr> {
        self.0.get(i)
    }

    pub fn get_mut(&mut self, i: usize) -> Option<&mut Expr> {
        self.0.get_mut(i)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn accept_expr(&self, expr_ref: &ExprRef, visitor: &mut dyn AstVisitor)
                       -> Result<TypeDecl, TypeCheckError> {
        match self.get(expr_ref.to_index()) {
            Some(expr) => expr.clone().accept(visitor),
            None => Err(TypeCheckError::new(format!("Expression not found at index: {:?}", expr_ref.to_index()))),
        }
    }


}

impl Default for StmtPool {
    fn default() -> Self {
        Self::new()
    }
}

impl StmtPool {
    pub fn new() -> StmtPool {
        StmtPool(Vec::new())
    }
    pub fn with_capacity(cap: usize) -> StmtPool {
        StmtPool(Vec::with_capacity(cap))
    }

    pub fn push(&mut self, stmt: Stmt) {
        self.0.push(stmt);
    }

    pub fn add(&mut self, stmt: Stmt) -> StmtRef {
        let len = self.0.len();
        self.0.push(stmt);
        StmtRef(len as u32)
    }

    pub fn get(&self, i: usize) -> Option<&Stmt> {
        self.0.get(i)
    }

    pub fn get_mut(&mut self, i: usize) -> Option<&mut Stmt> {
        self.0.get_mut(i)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

}

pub struct AstBuilder {
    pub expr_pool: ExprPool,
    pub stmt_pool: StmtPool,
    pub location_pool: LocationPool,
}

impl AstBuilder {
    pub fn new() -> Self {
        AstBuilder {
            expr_pool: ExprPool::new(),
            stmt_pool: StmtPool::new(),
            location_pool: LocationPool::new(),
        }
    }

    pub fn with_capacity(expr_cap: usize, stmt_cap: usize) -> Self {
        AstBuilder {
            expr_pool: ExprPool::with_capacity(expr_cap),
            stmt_pool: StmtPool::with_capacity(stmt_cap),
            location_pool: LocationPool::with_capacity(expr_cap, stmt_cap),
        }
    }

    // Legacy methods for compatibility
    pub fn add_expr(&mut self, expr: Expr) -> ExprRef {
        let expr_ref = self.expr_pool.add(expr);
        self.location_pool.add_expr_location(None);
        expr_ref
    }

    pub fn add_stmt(&mut self, stmt: Stmt) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(stmt);
        self.location_pool.add_stmt_location(None);
        stmt_ref
    }
    
    // New methods with location support
    pub fn add_expr_with_location(&mut self, expr: Expr, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(expr);
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn add_stmt_with_location(&mut self, stmt: Stmt, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(stmt);
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn get_expr_pool(&self) -> &ExprPool {
        &self.expr_pool
    }

    pub fn get_stmt_pool(&self) -> &StmtPool {
        &self.stmt_pool
    }

    pub fn get_expr_pool_mut(&mut self) -> &mut ExprPool {
        &mut self.expr_pool
    }

    pub fn get_stmt_pool_mut(&mut self) -> &mut StmtPool {
        &mut self.stmt_pool
    }
    
    pub fn get_location_pool(&self) -> &LocationPool {
        &self.location_pool
    }
    
    pub fn get_location_pool_mut(&mut self) -> &mut LocationPool {
        &mut self.location_pool
    }

    pub fn extract_pools(self) -> (ExprPool, StmtPool, LocationPool) {
        (self.expr_pool, self.stmt_pool, self.location_pool)
    }

    // New Builder Pattern API
    
    // Expression builders
    pub fn uint64_expr(&mut self, value: u64, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::UInt64(value));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn int64_expr(&mut self, value: i64, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Int64(value));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn bool_true_expr(&mut self, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::True);
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn bool_false_expr(&mut self, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::False);
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn null_expr(&mut self, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Null);
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn identifier_expr(&mut self, symbol: DefaultSymbol, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Identifier(symbol));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn string_expr(&mut self, symbol: DefaultSymbol, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::String(symbol));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn number_expr(&mut self, symbol: DefaultSymbol, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Number(symbol));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn binary_expr(&mut self, op: Operator, lhs: ExprRef, rhs: ExprRef, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Binary(op, lhs, rhs));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn assign_expr(&mut self, lhs: ExprRef, rhs: ExprRef, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Assign(lhs, rhs));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn if_elif_else_expr(&mut self, cond: ExprRef, if_block: ExprRef, elif_pairs: Vec<(ExprRef, ExprRef)>, else_block: ExprRef, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::IfElifElse(cond, if_block, elif_pairs, else_block));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn block_expr(&mut self, statements: Vec<StmtRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Block(statements));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn call_expr(&mut self, fn_name: DefaultSymbol, args: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let args_ref = self.expr_pool.add(Expr::ExprList(args));
        self.location_pool.add_expr_location(None); // args_ref location
        let expr_ref = self.expr_pool.add(Expr::Call(fn_name, args_ref));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn expr_list(&mut self, exprs: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::ExprList(exprs));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn array_literal_expr(&mut self, elements: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::ArrayLiteral(elements));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn array_access_expr(&mut self, array: ExprRef, index: ExprRef, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::ArrayAccess(array, index));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn index_access_expr(&mut self, object: ExprRef, index: ExprRef, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::IndexAccess(object, index));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn index_assign_expr(&mut self, object: ExprRef, index: ExprRef, value: ExprRef, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::IndexAssign(object, index, value));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn dict_literal_expr(&mut self, entries: Vec<(ExprRef, ExprRef)>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::DictLiteral(entries));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn tuple_literal_expr(&mut self, elements: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::TupleLiteral(elements));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn tuple_access_expr(&mut self, tuple: ExprRef, index: usize, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::TupleAccess(tuple, index));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn field_access_expr(&mut self, object: ExprRef, field: DefaultSymbol, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::FieldAccess(object, field));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn method_call_expr(&mut self, object: ExprRef, method: DefaultSymbol, args: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::MethodCall(object, method, args));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn struct_literal_expr(&mut self, type_name: DefaultSymbol, fields: Vec<(DefaultSymbol, ExprRef)>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::StructLiteral(type_name, fields));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn qualified_identifier_expr(&mut self, path: Vec<DefaultSymbol>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::QualifiedIdentifier(path));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn builtin_method_call_expr(&mut self, receiver: ExprRef, method: BuiltinMethod, args: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::BuiltinMethodCall(receiver, method, args));
        self.location_pool.add_expr_location(location);
        expr_ref
    }
    
    pub fn builtin_call_expr(&mut self, func: BuiltinFunction, args: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::BuiltinCall(func, args));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    // Statement builders
    pub fn expression_stmt(&mut self, expr: ExprRef, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::Expression(expr));
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
    
    pub fn val_stmt(&mut self, name: DefaultSymbol, type_decl: Option<TypeDecl>, value: ExprRef, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::Val(name, type_decl, value));
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
    
    pub fn var_stmt(&mut self, name: DefaultSymbol, type_decl: Option<TypeDecl>, value: Option<ExprRef>, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::Var(name, type_decl, value));
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
    
    pub fn return_stmt(&mut self, value: Option<ExprRef>, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::Return(value));
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
    
    pub fn break_stmt(&mut self, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::Break);
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
    
    pub fn continue_stmt(&mut self, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::Continue);
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
    
    pub fn for_stmt(&mut self, var: DefaultSymbol, start: ExprRef, end: ExprRef, block: ExprRef, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::For(var, start, end, block));
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
    
    pub fn while_stmt(&mut self, cond: ExprRef, block: ExprRef, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::While(cond, block));
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
    
    pub fn struct_decl_stmt(&mut self, name: String, fields: Vec<StructField>, visibility: Visibility, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::StructDecl { name, fields, visibility });
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
    
    pub fn impl_block_stmt(&mut self, target_type: String, methods: Vec<Rc<MethodFunction>>, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::ImplBlock { target_type, methods });
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
}


#[derive(Debug, Clone)]
pub struct Program {
    pub node: Node,
    pub package_decl: Option<PackageDecl>,
    pub imports: Vec<ImportDecl>,
    pub function: Vec<Rc<Function>>,

    pub statement: StmtPool,
    pub expression: ExprPool,
    pub location_pool: LocationPool,
}

impl Program {
    pub fn get(&self, i: u32) -> Option<&crate::ast::Expr> {
        self.expression.0.get(i as usize)
    }

    pub fn len(&self) -> usize {
        self.expression.0.len()
    }

}

#[derive(Debug, PartialEq, Clone)]
pub struct Function {
    pub node: Node,
    pub name: DefaultSymbol,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub code: StmtRef,
    pub visibility: Visibility,
}

pub type Parameter = (DefaultSymbol, TypeDecl);
pub type ParameterList = Vec<Parameter>;

#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub name: String,
    pub type_decl: TypeDecl,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImplBlock {
    pub target_type: String,
    pub methods: Vec<Rc<MethodFunction>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodFunction {
    pub node: Node,
    pub name: DefaultSymbol,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub code: StmtRef,
    pub has_self_param: bool, // true if first parameter is &self
    pub visibility: Visibility,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackageDecl {
    pub name: Vec<DefaultSymbol>,  // package path components: [math_symbol, basic_symbol]
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    pub module_path: Vec<DefaultSymbol>,  // module path: [math_symbol, basic_symbol]
    pub alias: Option<DefaultSymbol>,     // alias from "as" clause
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
        name: String,
        fields: Vec<StructField>,
        visibility: Visibility,
    },
    ImplBlock {
        target_type: String,
        methods: Vec<Rc<MethodFunction>>,
    },
}

#[derive(Debug, PartialEq, Clone)]
pub enum Expr {
    Assign(ExprRef, ExprRef),   // lhs = rhs
    IfElifElse(ExprRef, ExprRef, Vec<(ExprRef, ExprRef)>, ExprRef), // if_cond, if_block, elif_pairs, else_block
    Binary(Operator, ExprRef, ExprRef),
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
    ArrayAccess(ExprRef, ExprRef),  // a[0]
    FieldAccess(ExprRef, DefaultSymbol),  // obj.field
    MethodCall(ExprRef, DefaultSymbol, Vec<ExprRef>),  // obj.method(args)
    StructLiteral(DefaultSymbol, Vec<(DefaultSymbol, ExprRef)>),  // Point { x: 10, y: 20 }
    QualifiedIdentifier(Vec<DefaultSymbol>),  // math::add
    BuiltinMethodCall(ExprRef, BuiltinMethod, Vec<ExprRef>),  // "hello".len(), str.concat("world")
    BuiltinCall(BuiltinFunction, Vec<ExprRef>),  // __builtin_heap_alloc(), __builtin_print_ln(), etc.
    IndexAccess(ExprRef, ExprRef),  // x[key] - generic index access
    IndexAssign(ExprRef, ExprRef, ExprRef),  // x[key] = value - index assignment
    DictLiteral(Vec<(ExprRef, ExprRef)>),  // {key1: value1, key2: value2}
    TupleLiteral(Vec<ExprRef>),  // (expr1, expr2, ...) - tuple literal
    TupleAccess(ExprRef, usize),  // tuple.0, tuple.1, etc - tuple element access
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
    
    // I/O operations (disabled for now)
    // Print,        // __builtin_print(value: any) -> unit
    // PrintLn,      // __builtin_print_ln(value: any) -> unit
    
    // Math operations (disabled for now)
    // AbsI64,       // __builtin_abs_i64(x: i64) -> i64
    // AbsU64,       // __builtin_abs_u64(x: u64) -> u64
    // MinI64,       // __builtin_min_i64(a: i64, b: i64) -> i64
    // MaxI64,       // __builtin_max_i64(a: i64, b: i64) -> i64
    // MinU64,       // __builtin_min_u64(a: u64, b: u64) -> u64
    // MaxU64,       // __builtin_max_u64(a: u64, b: u64) -> u64
    
    // String operations (disabled for now)
    // StrLen,       // __builtin_str_len(s: str) -> u64
    // StrConcat,    // __builtin_str_concat(a: str, b: str) -> str
    // StrSubstring, // __builtin_str_substring(s: str, start: u64, end: u64) -> str
    // StrContains,  // __builtin_str_contains(haystack: str, needle: str) -> bool
    
    // Array operations (disabled for now)
    // ArrayLen,     // __builtin_array_len(arr: [T]) -> u64
    // ArrayGet,     // __builtin_array_get(arr: [T], index: u64) -> T
    // ArraySet,     // __builtin_array_set(arr: [T], index: u64, value: T) -> [T]
    
    // Type conversion (disabled for now)
    // I64ToString,  // __builtin_i64_to_string(x: i64) -> str
    // U64ToString,  // __builtin_u64_to_string(x: u64) -> str
    // BoolToString, // __builtin_bool_to_string(x: bool) -> str
    // StringToI64,  // __builtin_string_to_i64(s: str) -> i64
    // StringToU64,  // __builtin_string_to_u64(s: str) -> u64
    // StringToBool, // __builtin_string_to_bool(s: str) -> bool
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
}

impl BuiltinFunctionSymbols {
    pub fn new(interner: &mut DefaultStringInterner) -> Self {
        Self {
            // Memory management
            heap_alloc: interner.get_or_intern("__builtin_heap_alloc"),
            heap_free: interner.get_or_intern("__builtin_heap_free"),
            heap_realloc: interner.get_or_intern("__builtin_heap_realloc"),
            
            // Pointer operations
            ptr_read: interner.get_or_intern("__builtin_ptr_read"),
            ptr_write: interner.get_or_intern("__builtin_ptr_write"),
            ptr_is_null: interner.get_or_intern("__builtin_ptr_is_null"),
            
            // Memory operations
            mem_copy: interner.get_or_intern("__builtin_mem_copy"),
            mem_move: interner.get_or_intern("__builtin_mem_move"),
            mem_set: interner.get_or_intern("__builtin_mem_set"),
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

impl Expr {
    pub fn is_block(&self) -> bool {
        match self {
            Expr::Block(_) => true,
            _ => false,
        }
    }
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
}

#[derive(Debug)]
pub struct BinaryExpr {
    pub op: Operator,
    pub lhs: ExprRef,
    pub rhs: ExprRef,
}
