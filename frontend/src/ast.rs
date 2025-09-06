use std::rc::Rc;
use string_interner::{DefaultSymbol, DefaultStringInterner};
use crate::type_checker::{Acceptable, TypeCheckError, SourceLocation};
use crate::type_decl::TypeDecl;
use crate::visitor::AstVisitor;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExprRef(pub u32);

#[derive(Debug, PartialEq, Clone)]
pub struct ExprPool {
    // Multiarray list - each field has its own Vec
    // Expression type discriminant
    pub expr_types: Vec<ExprType>,
    
    // Common fields used across multiple expression types
    pub lhs: Vec<Option<ExprRef>>,           // Left-hand side for binary, assign, etc.
    pub rhs: Vec<Option<ExprRef>>,           // Right-hand side for binary, assign, etc.
    pub operand: Vec<Option<ExprRef>>,       // Operand for unary operations
    pub operator: Vec<Option<Operator>>,     // Binary operators
    pub unary_op: Vec<Option<UnaryOp>>,      // Unary operators
    
    // Value fields
    pub int64_val: Vec<Option<i64>>,
    pub uint64_val: Vec<Option<u64>>,
    pub symbol_val: Vec<Option<DefaultSymbol>>,    // For identifiers, strings, numbers, function names, etc.
    pub boolean_val: Vec<Option<bool>>,            // For true/false
    
    // Collection fields
    pub expr_list: Vec<Option<Vec<ExprRef>>>,      // For expression lists, array literals, etc.
    pub stmt_list: Vec<Option<Vec<StmtRef>>>,      // For blocks
    pub symbol_list: Vec<Option<Vec<DefaultSymbol>>>,  // For qualified identifiers
    pub field_list: Vec<Option<Vec<(DefaultSymbol, ExprRef)>>>,  // For struct literals
    pub entry_list: Vec<Option<Vec<(ExprRef, ExprRef)>>>,  // For dict literals, elif pairs
    
    // Special fields
    pub builtin_method: Vec<Option<BuiltinMethod>>,
    pub builtin_function: Vec<Option<BuiltinFunction>>,
    pub index_val: Vec<Option<usize>>,             // For tuple access
    pub third_operand: Vec<Option<ExprRef>>,       // For index assign (value), if-elif-else (else block)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExprType {
    Assign = 0,
    IfElifElse = 1, 
    Binary = 2,
    Unary = 3,
    Block = 4,
    True = 5,
    False = 6,
    Int64 = 7,
    UInt64 = 8,
    Number = 9,
    Identifier = 10,
    Null = 11,
    ExprList = 12,
    Call = 13,
    String = 14,
    ArrayLiteral = 15,
    FieldAccess = 16,
    MethodCall = 17,
    StructLiteral = 18,
    QualifiedIdentifier = 19,
    BuiltinMethodCall = 20,
    BuiltinCall = 21,
    IndexAccess = 22,
    IndexAssign = 23,
    SliceAccess = 24,
    DictLiteral = 25,
    TupleLiteral = 26,
    TupleAccess = 27,
}

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StmtRef(pub u32);

#[derive(Debug, PartialEq, Clone)]
pub struct StmtPool {
    // Multiarray list - each field has its own Vec
    // Statement type discriminant
    pub stmt_types: Vec<StmtType>,
    
    // Common fields
    pub expr_val: Vec<Option<ExprRef>>,          // For expression statements, return values, etc.
    pub symbol_val: Vec<Option<DefaultSymbol>>,  // For variable names, for loop variables
    pub type_decl: Vec<Option<TypeDecl>>,        // For val/var type declarations
    
    // Control flow fields
    pub condition: Vec<Option<ExprRef>>,         // For while loops, if conditions
    pub start_expr: Vec<Option<ExprRef>>,        // For for loop start
    pub end_expr: Vec<Option<ExprRef>>,          // For for loop end
    pub block_expr: Vec<Option<ExprRef>>,        // For loop/while bodies
    
    // Declaration fields
    pub struct_name: Vec<Option<DefaultSymbol>>,             // For struct declarations
    pub struct_fields: Vec<Option<Vec<StructField>>>,        // For struct field lists
    pub visibility: Vec<Option<Visibility>>,                 // For struct/impl visibility
    pub impl_methods: Vec<Option<Vec<Rc<MethodFunction>>>>,  // For impl block methods
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StmtType {
    Expression = 0,
    Val = 1,
    Var = 2,
    Return = 3,
    Break = 4,
    Continue = 5,
    For = 6,
    While = 7,
    StructDecl = 8,
    ImplBlock = 9,
}

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
        ExprPool {
            expr_types: Vec::new(),
            lhs: Vec::new(),
            rhs: Vec::new(),
            operand: Vec::new(),
            operator: Vec::new(),
            unary_op: Vec::new(),
            int64_val: Vec::new(),
            uint64_val: Vec::new(),
            symbol_val: Vec::new(),
            boolean_val: Vec::new(),
            expr_list: Vec::new(),
            stmt_list: Vec::new(),
            symbol_list: Vec::new(),
            field_list: Vec::new(),
            entry_list: Vec::new(),
            builtin_method: Vec::new(),
            builtin_function: Vec::new(),
            index_val: Vec::new(),
            third_operand: Vec::new(),
        }
    }
    
    pub fn with_capacity(cap: usize) -> ExprPool {
        ExprPool {
            expr_types: Vec::with_capacity(cap),
            lhs: Vec::with_capacity(cap),
            rhs: Vec::with_capacity(cap),
            operand: Vec::with_capacity(cap),
            operator: Vec::with_capacity(cap),
            unary_op: Vec::with_capacity(cap),
            int64_val: Vec::with_capacity(cap),
            uint64_val: Vec::with_capacity(cap),
            symbol_val: Vec::with_capacity(cap),
            boolean_val: Vec::with_capacity(cap),
            expr_list: Vec::with_capacity(cap),
            stmt_list: Vec::with_capacity(cap),
            symbol_list: Vec::with_capacity(cap),
            field_list: Vec::with_capacity(cap),
            entry_list: Vec::with_capacity(cap),
            builtin_method: Vec::with_capacity(cap),
            builtin_function: Vec::with_capacity(cap),
            index_val: Vec::with_capacity(cap),
            third_operand: Vec::with_capacity(cap),
        }
    }

    fn extend_to_index(&mut self, index: usize) {
        let current_len = self.expr_types.len();
        if index >= current_len {
            let extend_count = index + 1 - current_len;
            self.expr_types.resize(index + 1, ExprType::Null);
            self.lhs.resize(current_len + extend_count, None);
            self.rhs.resize(current_len + extend_count, None);
            self.operand.resize(current_len + extend_count, None);
            self.operator.resize(current_len + extend_count, None);
            self.unary_op.resize(current_len + extend_count, None);
            self.int64_val.resize(current_len + extend_count, None);
            self.uint64_val.resize(current_len + extend_count, None);
            self.symbol_val.resize(current_len + extend_count, None);
            self.boolean_val.resize(current_len + extend_count, None);
            self.expr_list.resize(current_len + extend_count, None);
            self.stmt_list.resize(current_len + extend_count, None);
            self.symbol_list.resize(current_len + extend_count, None);
            self.field_list.resize(current_len + extend_count, None);
            self.entry_list.resize(current_len + extend_count, None);
            self.builtin_method.resize(current_len + extend_count, None);
            self.builtin_function.resize(current_len + extend_count, None);
            self.index_val.resize(current_len + extend_count, None);
            self.third_operand.resize(current_len + extend_count, None);
        }
    }

    pub fn add(&mut self, expr: Expr) -> ExprRef {
        let index = self.expr_types.len();
        self.extend_to_index(index);
        
        match expr {
            Expr::Assign(lhs, rhs) => {
                self.expr_types[index] = ExprType::Assign;
                self.lhs[index] = Some(lhs);
                self.rhs[index] = Some(rhs);
            }
            Expr::IfElifElse(cond, if_block, elif_pairs, else_block) => {
                self.expr_types[index] = ExprType::IfElifElse;
                self.lhs[index] = Some(cond);
                self.rhs[index] = Some(if_block);
                self.entry_list[index] = Some(elif_pairs);
                self.third_operand[index] = Some(else_block);
            }
            Expr::Binary(op, lhs, rhs) => {
                self.expr_types[index] = ExprType::Binary;
                self.operator[index] = Some(op);
                self.lhs[index] = Some(lhs);
                self.rhs[index] = Some(rhs);
            }
            Expr::Unary(op, operand) => {
                self.expr_types[index] = ExprType::Unary;
                self.unary_op[index] = Some(op);
                self.operand[index] = Some(operand);
            }
            Expr::Block(statements) => {
                self.expr_types[index] = ExprType::Block;
                self.stmt_list[index] = Some(statements);
            }
            Expr::True => {
                self.expr_types[index] = ExprType::True;
                self.boolean_val[index] = Some(true);
            }
            Expr::False => {
                self.expr_types[index] = ExprType::False;
                self.boolean_val[index] = Some(false);
            }
            Expr::Int64(value) => {
                self.expr_types[index] = ExprType::Int64;
                self.int64_val[index] = Some(value);
            }
            Expr::UInt64(value) => {
                self.expr_types[index] = ExprType::UInt64;
                self.uint64_val[index] = Some(value);
            }
            Expr::Number(symbol) => {
                self.expr_types[index] = ExprType::Number;
                self.symbol_val[index] = Some(symbol);
            }
            Expr::Identifier(symbol) => {
                self.expr_types[index] = ExprType::Identifier;
                self.symbol_val[index] = Some(symbol);
            }
            Expr::Null => {
                self.expr_types[index] = ExprType::Null;
            }
            Expr::ExprList(exprs) => {
                self.expr_types[index] = ExprType::ExprList;
                self.expr_list[index] = Some(exprs);
            }
            Expr::Call(fn_name, args) => {
                self.expr_types[index] = ExprType::Call;
                self.symbol_val[index] = Some(fn_name);
                self.operand[index] = Some(args);
            }
            Expr::String(symbol) => {
                self.expr_types[index] = ExprType::String;
                self.symbol_val[index] = Some(symbol);
            }
            Expr::ArrayLiteral(elements) => {
                self.expr_types[index] = ExprType::ArrayLiteral;
                self.expr_list[index] = Some(elements);
            }
            Expr::FieldAccess(object, field) => {
                self.expr_types[index] = ExprType::FieldAccess;
                self.lhs[index] = Some(object);
                self.symbol_val[index] = Some(field);
            }
            Expr::MethodCall(object, method, args) => {
                self.expr_types[index] = ExprType::MethodCall;
                self.lhs[index] = Some(object);
                self.symbol_val[index] = Some(method);
                self.expr_list[index] = Some(args);
            }
            Expr::StructLiteral(type_name, fields) => {
                self.expr_types[index] = ExprType::StructLiteral;
                self.symbol_val[index] = Some(type_name);
                self.field_list[index] = Some(fields);
            }
            Expr::QualifiedIdentifier(path) => {
                self.expr_types[index] = ExprType::QualifiedIdentifier;
                self.symbol_list[index] = Some(path);
            }
            Expr::BuiltinMethodCall(receiver, method, args) => {
                self.expr_types[index] = ExprType::BuiltinMethodCall;
                self.lhs[index] = Some(receiver);
                self.builtin_method[index] = Some(method);
                self.expr_list[index] = Some(args);
            }
            Expr::BuiltinCall(func, args) => {
                self.expr_types[index] = ExprType::BuiltinCall;
                self.builtin_function[index] = Some(func);
                self.expr_list[index] = Some(args);
            }
            Expr::IndexAccess(object, index_expr) => {
                self.expr_types[index] = ExprType::IndexAccess;
                self.lhs[index] = Some(object);
                self.rhs[index] = Some(index_expr);
            }
            Expr::IndexAssign(object, index_expr, value) => {
                self.expr_types[index] = ExprType::IndexAssign;
                self.lhs[index] = Some(object);
                self.rhs[index] = Some(index_expr);
                self.third_operand[index] = Some(value);
            }
            Expr::SliceAccess(object, start, end) => {
                self.expr_types[index] = ExprType::SliceAccess;
                self.lhs[index] = Some(object);
                self.rhs[index] = start;
                self.third_operand[index] = end;
            }
            Expr::DictLiteral(entries) => {
                self.expr_types[index] = ExprType::DictLiteral;
                self.entry_list[index] = Some(entries);
            }
            Expr::TupleLiteral(elements) => {
                self.expr_types[index] = ExprType::TupleLiteral;
                self.expr_list[index] = Some(elements);
            }
            Expr::TupleAccess(tuple, index_val) => {
                self.expr_types[index] = ExprType::TupleAccess;
                self.lhs[index] = Some(tuple);
                self.index_val[index] = Some(index_val);
            }
        }
        
        ExprRef(index as u32)
    }

    pub fn get(&self, expr_ref: &ExprRef) -> Option<Expr> {
        let index = expr_ref.to_index();
        if index >= self.expr_types.len() {
            return None;
        }
        
        match self.expr_types[index] {
            ExprType::Assign => {
                Some(Expr::Assign(
                    self.lhs[index]?,
                    self.rhs[index]?
                ))
            }
            ExprType::IfElifElse => {
                Some(Expr::IfElifElse(
                    self.lhs[index]?,
                    self.rhs[index]?,
                    self.entry_list[index].clone()?,
                    self.third_operand[index]?
                ))
            }
            ExprType::Binary => {
                Some(Expr::Binary(
                    self.operator[index].clone()?,
                    self.lhs[index]?,
                    self.rhs[index]?
                ))
            }
            ExprType::Unary => {
                Some(Expr::Unary(
                    self.unary_op[index].clone()?,
                    self.operand[index]?
                ))
            }
            ExprType::Block => {
                Some(Expr::Block(self.stmt_list[index].clone()?))
            }
            ExprType::True => Some(Expr::True),
            ExprType::False => Some(Expr::False),
            ExprType::Int64 => {
                Some(Expr::Int64(self.int64_val[index]?))
            }
            ExprType::UInt64 => {
                Some(Expr::UInt64(self.uint64_val[index]?))
            }
            ExprType::Number => {
                Some(Expr::Number(self.symbol_val[index]?))
            }
            ExprType::Identifier => {
                Some(Expr::Identifier(self.symbol_val[index]?))
            }
            ExprType::Null => Some(Expr::Null),
            ExprType::ExprList => {
                Some(Expr::ExprList(self.expr_list[index].clone()?))
            }
            ExprType::Call => {
                Some(Expr::Call(
                    self.symbol_val[index]?,
                    self.operand[index]?
                ))
            }
            ExprType::String => {
                Some(Expr::String(self.symbol_val[index]?))
            }
            ExprType::ArrayLiteral => {
                Some(Expr::ArrayLiteral(self.expr_list[index].clone()?))
            }
            ExprType::FieldAccess => {
                Some(Expr::FieldAccess(
                    self.lhs[index]?,
                    self.symbol_val[index]?
                ))
            }
            ExprType::MethodCall => {
                Some(Expr::MethodCall(
                    self.lhs[index]?,
                    self.symbol_val[index]?,
                    self.expr_list[index].clone()?
                ))
            }
            ExprType::StructLiteral => {
                Some(Expr::StructLiteral(
                    self.symbol_val[index]?,
                    self.field_list[index].clone()?
                ))
            }
            ExprType::QualifiedIdentifier => {
                Some(Expr::QualifiedIdentifier(self.symbol_list[index].clone()?))
            }
            ExprType::BuiltinMethodCall => {
                Some(Expr::BuiltinMethodCall(
                    self.lhs[index]?,
                    self.builtin_method[index].clone()?,
                    self.expr_list[index].clone()?
                ))
            }
            ExprType::BuiltinCall => {
                Some(Expr::BuiltinCall(
                    self.builtin_function[index].clone()?,
                    self.expr_list[index].clone()?
                ))
            }
            ExprType::IndexAccess => {
                Some(Expr::IndexAccess(
                    self.lhs[index]?,
                    self.rhs[index]?
                ))
            }
            ExprType::IndexAssign => {
                Some(Expr::IndexAssign(
                    self.lhs[index]?,
                    self.rhs[index]?,
                    self.third_operand[index]?
                ))
            }
            ExprType::SliceAccess => {
                Some(Expr::SliceAccess(
                    self.lhs[index]?,
                    self.rhs[index],
                    self.third_operand[index]
                ))
            }
            ExprType::DictLiteral => {
                Some(Expr::DictLiteral(self.entry_list[index].clone()?))
            }
            ExprType::TupleLiteral => {
                Some(Expr::TupleLiteral(self.expr_list[index].clone()?))
            }
            ExprType::TupleAccess => {
                Some(Expr::TupleAccess(
                    self.lhs[index]?,
                    self.index_val[index]?
                ))
            }
        }
    }

    pub fn len(&self) -> usize {
        self.expr_types.len()
    }
    
    pub fn is_empty(&self) -> bool {
        self.expr_types.is_empty()
    }

    pub fn update(&mut self, expr_ref: &ExprRef, expr: Expr) {
        let index = expr_ref.to_index();
        if index >= self.expr_types.len() {
            return;
        }
        
        // Clear all fields for this index first
        self.lhs[index] = None;
        self.rhs[index] = None;
        self.operand[index] = None;
        self.operator[index] = None;
        self.unary_op[index] = None;
        self.int64_val[index] = None;
        self.uint64_val[index] = None;
        self.symbol_val[index] = None;
        self.boolean_val[index] = None;
        self.expr_list[index] = None;
        self.stmt_list[index] = None;
        self.symbol_list[index] = None;
        self.field_list[index] = None;
        self.entry_list[index] = None;
        self.builtin_method[index] = None;
        self.builtin_function[index] = None;
        self.index_val[index] = None;
        self.third_operand[index] = None;
        
        // Set the new expression data
        match expr {
            Expr::Assign(lhs, rhs) => {
                self.expr_types[index] = ExprType::Assign;
                self.lhs[index] = Some(lhs);
                self.rhs[index] = Some(rhs);
            }
            Expr::IfElifElse(cond, then_expr, elif_branches, else_expr) => {
                self.expr_types[index] = ExprType::IfElifElse;
                self.lhs[index] = Some(cond);
                self.rhs[index] = Some(then_expr);
                self.entry_list[index] = Some(elif_branches);
                self.third_operand[index] = Some(else_expr);
            }
            Expr::Binary(op, lhs, rhs) => {
                self.expr_types[index] = ExprType::Binary;
                self.operator[index] = Some(op);
                self.lhs[index] = Some(lhs);
                self.rhs[index] = Some(rhs);
            }
            Expr::Unary(op, operand) => {
                self.expr_types[index] = ExprType::Unary;
                self.unary_op[index] = Some(op);
                self.operand[index] = Some(operand);
            }
            Expr::Block(stmts) => {
                self.expr_types[index] = ExprType::Block;
                self.stmt_list[index] = Some(stmts);
            }
            Expr::True => {
                self.expr_types[index] = ExprType::True;
            }
            Expr::False => {
                self.expr_types[index] = ExprType::False;
            }
            Expr::Int64(val) => {
                self.expr_types[index] = ExprType::Int64;
                self.int64_val[index] = Some(val);
            }
            Expr::UInt64(val) => {
                self.expr_types[index] = ExprType::UInt64;
                self.uint64_val[index] = Some(val);
            }
            Expr::Number(sym) => {
                self.expr_types[index] = ExprType::Number;
                self.symbol_val[index] = Some(sym);
            }
            Expr::Identifier(sym) => {
                self.expr_types[index] = ExprType::Identifier;
                self.symbol_val[index] = Some(sym);
            }
            Expr::ArrayLiteral(elements) => {
                self.expr_types[index] = ExprType::ArrayLiteral;
                self.expr_list[index] = Some(elements);
            }
            Expr::Call(func, args) => {
                self.expr_types[index] = ExprType::Call;
                self.symbol_val[index] = Some(func);
                self.rhs[index] = Some(args);
            }
            Expr::MethodCall(obj, method, args) => {
                self.expr_types[index] = ExprType::MethodCall;
                self.lhs[index] = Some(obj);
                self.symbol_val[index] = Some(method);
                self.expr_list[index] = Some(args);
            }
            Expr::StructLiteral(name, fields) => {
                self.expr_types[index] = ExprType::StructLiteral;
                self.symbol_val[index] = Some(name);
                self.field_list[index] = Some(fields);
            }
            Expr::QualifiedIdentifier(path) => {
                self.expr_types[index] = ExprType::QualifiedIdentifier;
                self.symbol_list[index] = Some(path);
            }
            Expr::BuiltinMethodCall(obj, method, args) => {
                self.expr_types[index] = ExprType::BuiltinMethodCall;
                self.lhs[index] = Some(obj);
                self.builtin_method[index] = Some(method);
                self.expr_list[index] = Some(args);
            }
            Expr::BuiltinCall(func, args) => {
                self.expr_types[index] = ExprType::BuiltinCall;
                self.builtin_function[index] = Some(func);
                self.expr_list[index] = Some(args);
            }
            Expr::IndexAccess(obj, index_expr) => {
                self.expr_types[index] = ExprType::IndexAccess;
                self.lhs[index] = Some(obj);
                self.rhs[index] = Some(index_expr);
            }
            Expr::IndexAssign(obj, index_expr, value) => {
                self.expr_types[index] = ExprType::IndexAssign;
                self.lhs[index] = Some(obj);
                self.rhs[index] = Some(index_expr);
                self.third_operand[index] = Some(value);
            }
            Expr::SliceAccess(obj, start, end) => {
                self.expr_types[index] = ExprType::SliceAccess;
                self.lhs[index] = Some(obj);
                self.rhs[index] = start;
                self.third_operand[index] = end;
            }
            Expr::DictLiteral(entries) => {
                self.expr_types[index] = ExprType::DictLiteral;
                self.entry_list[index] = Some(entries);
            }
            Expr::TupleLiteral(elements) => {
                self.expr_types[index] = ExprType::TupleLiteral;
                self.expr_list[index] = Some(elements);
            }
            Expr::TupleAccess(obj, idx) => {
                self.expr_types[index] = ExprType::TupleAccess;
                self.lhs[index] = Some(obj);
                self.index_val[index] = Some(idx);
            }
            Expr::Null => {
                self.expr_types[index] = ExprType::Null;
            }
            Expr::ExprList(exprs) => {
                self.expr_types[index] = ExprType::ExprList;
                self.expr_list[index] = Some(exprs);
            }
            Expr::String(sym) => {
                self.expr_types[index] = ExprType::String;
                self.symbol_val[index] = Some(sym);
            }
            Expr::FieldAccess(obj, field) => {
                self.expr_types[index] = ExprType::FieldAccess;
                self.lhs[index] = Some(obj);
                self.symbol_val[index] = Some(field);
            }
        }
    }

    pub fn accept_expr(&self, expr_ref: &ExprRef, visitor: &mut dyn AstVisitor)
                       -> Result<TypeDecl, TypeCheckError> {
        match self.get(expr_ref) {
            Some(mut expr) => expr.accept(visitor),
            None => Err(TypeCheckError::new(format!("Expression not found: {:?}", expr_ref))),
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
        StmtPool {
            stmt_types: Vec::new(),
            expr_val: Vec::new(),
            symbol_val: Vec::new(),
            type_decl: Vec::new(),
            condition: Vec::new(),
            start_expr: Vec::new(),
            end_expr: Vec::new(),
            block_expr: Vec::new(),
            struct_name: Vec::new(),
            struct_fields: Vec::new(),
            visibility: Vec::new(),
            impl_methods: Vec::new(),
        }
    }
    
    pub fn with_capacity(cap: usize) -> StmtPool {
        StmtPool {
            stmt_types: Vec::with_capacity(cap),
            expr_val: Vec::with_capacity(cap),
            symbol_val: Vec::with_capacity(cap),
            type_decl: Vec::with_capacity(cap),
            condition: Vec::with_capacity(cap),
            start_expr: Vec::with_capacity(cap),
            end_expr: Vec::with_capacity(cap),
            block_expr: Vec::with_capacity(cap),
            struct_name: Vec::with_capacity(cap),
            struct_fields: Vec::with_capacity(cap),
            visibility: Vec::with_capacity(cap),
            impl_methods: Vec::with_capacity(cap),
        }
    }

    fn extend_to_index(&mut self, index: usize) {
        let current_len = self.stmt_types.len();
        if index >= current_len {
            let extend_count = index + 1 - current_len;
            self.stmt_types.resize(index + 1, StmtType::Break);
            self.expr_val.resize(current_len + extend_count, None);
            self.symbol_val.resize(current_len + extend_count, None);
            self.type_decl.resize(current_len + extend_count, None);
            self.condition.resize(current_len + extend_count, None);
            self.start_expr.resize(current_len + extend_count, None);
            self.end_expr.resize(current_len + extend_count, None);
            self.block_expr.resize(current_len + extend_count, None);
            self.struct_name.resize(current_len + extend_count, None);
            self.struct_fields.resize(current_len + extend_count, None);
            self.visibility.resize(current_len + extend_count, None);
            self.impl_methods.resize(current_len + extend_count, None);
        }
    }

    pub fn add(&mut self, stmt: Stmt) -> StmtRef {
        let index = self.stmt_types.len();
        self.extend_to_index(index);
        
        match stmt {
            Stmt::Expression(expr) => {
                self.stmt_types[index] = StmtType::Expression;
                self.expr_val[index] = Some(expr);
            }
            Stmt::Val(name, type_decl, value) => {
                self.stmt_types[index] = StmtType::Val;
                self.symbol_val[index] = Some(name);
                self.type_decl[index] = type_decl;
                self.expr_val[index] = Some(value);
            }
            Stmt::Var(name, type_decl, value) => {
                self.stmt_types[index] = StmtType::Var;
                self.symbol_val[index] = Some(name);
                self.type_decl[index] = type_decl;
                self.expr_val[index] = value;
            }
            Stmt::Return(value) => {
                self.stmt_types[index] = StmtType::Return;
                self.expr_val[index] = value;
            }
            Stmt::Break => {
                self.stmt_types[index] = StmtType::Break;
            }
            Stmt::Continue => {
                self.stmt_types[index] = StmtType::Continue;
            }
            Stmt::For(var, start, end, block) => {
                self.stmt_types[index] = StmtType::For;
                self.symbol_val[index] = Some(var);
                self.start_expr[index] = Some(start);
                self.end_expr[index] = Some(end);
                self.block_expr[index] = Some(block);
            }
            Stmt::While(cond, block) => {
                self.stmt_types[index] = StmtType::While;
                self.condition[index] = Some(cond);
                self.block_expr[index] = Some(block);
            }
            Stmt::StructDecl { name, fields, visibility } => {
                self.stmt_types[index] = StmtType::StructDecl;
                self.struct_name[index] = Some(name);
                self.struct_fields[index] = Some(fields);
                self.visibility[index] = Some(visibility);
            }
            Stmt::ImplBlock { target_type, methods } => {
                self.stmt_types[index] = StmtType::ImplBlock;
                self.struct_name[index] = Some(target_type);
                self.impl_methods[index] = Some(methods);
            }
        }
        
        StmtRef(index as u32)
    }

    pub fn get(&self, stmt_ref: &StmtRef) -> Option<Stmt> {
        let index = stmt_ref.to_index();
        if index >= self.stmt_types.len() {
            return None;
        }
        
        match self.stmt_types[index] {
            StmtType::Expression => {
                Some(Stmt::Expression(self.expr_val[index]?))
            }
            StmtType::Val => {
                Some(Stmt::Val(
                    self.symbol_val[index]?,
                    self.type_decl[index].clone(),
                    self.expr_val[index]?
                ))
            }
            StmtType::Var => {
                Some(Stmt::Var(
                    self.symbol_val[index]?,
                    self.type_decl[index].clone(),
                    self.expr_val[index]
                ))
            }
            StmtType::Return => {
                Some(Stmt::Return(self.expr_val[index]))
            }
            StmtType::Break => Some(Stmt::Break),
            StmtType::Continue => Some(Stmt::Continue),
            StmtType::For => {
                Some(Stmt::For(
                    self.symbol_val[index]?,
                    self.start_expr[index]?,
                    self.end_expr[index]?,
                    self.block_expr[index]?
                ))
            }
            StmtType::While => {
                Some(Stmt::While(
                    self.condition[index]?,
                    self.block_expr[index]?
                ))
            }
            StmtType::StructDecl => {
                Some(Stmt::StructDecl {
                    name: self.struct_name[index].clone()?,
                    fields: self.struct_fields[index].clone()?,
                    visibility: self.visibility[index].clone()?,
                })
            }
            StmtType::ImplBlock => {
                Some(Stmt::ImplBlock {
                    target_type: self.struct_name[index].clone()?,
                    methods: self.impl_methods[index].clone()?,
                })
            }
        }
    }

    pub fn len(&self) -> usize {
        self.stmt_types.len()
    }
    
    pub fn is_empty(&self) -> bool {
        self.stmt_types.is_empty()
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

    pub fn unary_expr(&mut self, op: UnaryOp, operand: ExprRef, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Unary(op, operand));
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
    
    pub fn slice_access_expr(&mut self, object: ExprRef, start: Option<ExprRef>, end: Option<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::SliceAccess(object, start, end));
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
    
    pub fn struct_decl_stmt(&mut self, name: DefaultSymbol, fields: Vec<StructField>, visibility: Visibility, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::StructDecl { name, fields, visibility });
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
    
    pub fn impl_block_stmt(&mut self, target_type: DefaultSymbol, methods: Vec<Rc<MethodFunction>>, location: Option<SourceLocation>) -> StmtRef {
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
    pub fn get(&self, expr_ref: &ExprRef) -> Option<Expr> {
        self.expression.get(expr_ref)
    }

    pub fn len(&self) -> usize {
        self.expression.len()
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
        name: DefaultSymbol,
        fields: Vec<StructField>,
        visibility: Visibility,
    },
    ImplBlock {
        target_type: DefaultSymbol,
        methods: Vec<Rc<MethodFunction>>,
    },
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
    IndexAccess(ExprRef, ExprRef),  // x[key] - generic index access
    IndexAssign(ExprRef, ExprRef, ExprRef),  // x[key] = value - index assignment
    SliceAccess(ExprRef, Option<ExprRef>, Option<ExprRef>),  // arr[start..end] - slice access
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
pub enum UnaryOp {
    BitwiseNot,  // ~
    LogicalNot,  // !
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
