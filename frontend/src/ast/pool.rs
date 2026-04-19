use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::type_checker::{Acceptable, TypeCheckError, SourceLocation};
use crate::type_decl::TypeDecl;
use crate::visitor::AstVisitor;
use super::{
    Expr, Stmt, Operator, UnaryOp, SliceInfo, Pattern, EnumVariantDef,
    BuiltinMethod, BuiltinFunction,
    StructField, Visibility, MethodFunction,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExprRef(pub u32);

impl ExprRef {
    pub fn to_index(&self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StmtRef(pub u32);

impl StmtRef {
    pub fn to_index(&self) -> usize {
        self.0 as usize
    }
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
    SliceAccess = 22,
    SliceAssign = 23,
    AssociatedFunctionCall = 24,
    DictLiteral = 25,
    TupleLiteral = 26,
    TupleAccess = 27,
    Cast = 28,
    With = 29,
    Match = 30,
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
    EnumDecl = 10,
}

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
    pub slice_info: Vec<Option<SliceInfo>>,        // For slice access
    pub target_type: Vec<Option<TypeDecl>>,        // For cast expressions
    pub match_arms: Vec<Option<Vec<(Pattern, ExprRef)>>>,  // For match expressions
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
            slice_info: Vec::new(),
            target_type: Vec::new(),
            match_arms: Vec::new(),
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
            slice_info: Vec::with_capacity(cap),
            target_type: Vec::with_capacity(cap),
            match_arms: Vec::with_capacity(cap),
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
            self.slice_info.resize(current_len + extend_count, None);
            self.target_type.resize(current_len + extend_count, None);
            self.match_arms.resize(current_len + extend_count, None);
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
            Expr::SliceAssign(object, start_expr, end_expr, value) => {
                self.expr_types[index] = ExprType::SliceAssign;
                self.lhs[index] = Some(object);
                self.rhs[index] = start_expr;
                self.operand[index] = end_expr;
                self.third_operand[index] = Some(value);
            }
            Expr::AssociatedFunctionCall(struct_name, function_name, args) => {
                self.expr_types[index] = ExprType::AssociatedFunctionCall;
                self.symbol_list[index] = Some(vec![struct_name, function_name]);
                self.expr_list[index] = Some(args);
            }
            Expr::SliceAccess(object, slice_info) => {
                self.expr_types[index] = ExprType::SliceAccess;
                self.lhs[index] = Some(object);
                self.slice_info[index] = Some(slice_info);
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
            Expr::Cast(expr, type_decl) => {
                self.expr_types[index] = ExprType::Cast;
                self.lhs[index] = Some(expr);
                self.target_type[index] = Some(type_decl);
            }
            Expr::With(allocator, body) => {
                self.expr_types[index] = ExprType::With;
                self.lhs[index] = Some(allocator);
                self.rhs[index] = Some(body);
            }
            Expr::Match(scrutinee, arms) => {
                self.expr_types[index] = ExprType::Match;
                self.lhs[index] = Some(scrutinee);
                self.match_arms[index] = Some(arms);
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
                    self.rhs[index]?,
                ))
            }
            ExprType::IfElifElse => {
                Some(Expr::IfElifElse(
                    self.lhs[index]?,
                    self.rhs[index]?,
                    self.entry_list[index].clone()?,
                    self.third_operand[index]?,
                ))
            }
            ExprType::Binary => {
                Some(Expr::Binary(
                    self.operator[index].clone()?,
                    self.lhs[index]?,
                    self.rhs[index]?,
                ))
            }
            ExprType::Unary => {
                Some(Expr::Unary(
                    self.unary_op[index].clone()?,
                    self.operand[index]?,
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
                    self.operand[index]?,
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
                    self.symbol_val[index]?,
                ))
            }
            ExprType::MethodCall => {
                Some(Expr::MethodCall(
                    self.lhs[index]?,
                    self.symbol_val[index]?,
                    self.expr_list[index].clone()?,
                ))
            }
            ExprType::StructLiteral => {
                Some(Expr::StructLiteral(
                    self.symbol_val[index]?,
                    self.field_list[index].clone()?,
                ))
            }
            ExprType::QualifiedIdentifier => {
                Some(Expr::QualifiedIdentifier(self.symbol_list[index].clone()?))
            }
            ExprType::BuiltinMethodCall => {
                Some(Expr::BuiltinMethodCall(
                    self.lhs[index]?,
                    self.builtin_method[index].clone()?,
                    self.expr_list[index].clone()?,
                ))
            }
            ExprType::BuiltinCall => {
                Some(Expr::BuiltinCall(
                    self.builtin_function[index].clone()?,
                    self.expr_list[index].clone()?,
                ))
            }
            ExprType::SliceAssign => {
                Some(Expr::SliceAssign(
                    self.lhs[index]?,
                    self.rhs[index],
                    self.operand[index],
                    self.third_operand[index]?,
                ))
            }
            ExprType::AssociatedFunctionCall => {
                let symbols = self.symbol_list[index].clone()?;
                if symbols.len() >= 2 {
                    Some(Expr::AssociatedFunctionCall(
                        symbols[0],
                        symbols[1],
                        self.expr_list[index].clone()?,
                    ))
                } else {
                    None
                }
            }
            ExprType::SliceAccess => {
                Some(Expr::SliceAccess(
                    self.lhs[index]?,
                    self.slice_info[index].clone()?,
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
                    self.index_val[index]?,
                ))
            }
            ExprType::Cast => {
                Some(Expr::Cast(
                    self.lhs[index]?,
                    self.target_type[index].clone()?,
                ))
            }
            ExprType::With => {
                Some(Expr::With(
                    self.lhs[index]?,
                    self.rhs[index]?,
                ))
            }
            ExprType::Match => {
                Some(Expr::Match(
                    self.lhs[index]?,
                    self.match_arms[index].clone()?,
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
        self.match_arms[index] = None;

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
            Expr::SliceAssign(obj, start_expr, end_expr, value) => {
                self.expr_types[index] = ExprType::SliceAssign;
                self.lhs[index] = Some(obj);
                self.rhs[index] = start_expr;
                self.operand[index] = end_expr;
                self.third_operand[index] = Some(value);
            }
            Expr::AssociatedFunctionCall(struct_name, function_name, args) => {
                self.expr_types[index] = ExprType::AssociatedFunctionCall;
                self.symbol_list[index] = Some(vec![struct_name, function_name]);
                self.expr_list[index] = Some(args);
            }
            Expr::SliceAccess(obj, slice_info) => {
                self.expr_types[index] = ExprType::SliceAccess;
                self.lhs[index] = Some(obj);
                self.slice_info[index] = Some(slice_info);
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
            Expr::Cast(expr, type_decl) => {
                self.expr_types[index] = ExprType::Cast;
                self.lhs[index] = Some(expr);
                self.target_type[index] = Some(type_decl);
            }
            Expr::With(allocator, body) => {
                self.expr_types[index] = ExprType::With;
                self.lhs[index] = Some(allocator);
                self.rhs[index] = Some(body);
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
            Expr::Match(scrutinee, arms) => {
                self.expr_types[index] = ExprType::Match;
                self.lhs[index] = Some(scrutinee);
                self.match_arms[index] = Some(arms);
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
    pub struct_generic_params: Vec<Option<Vec<DefaultSymbol>>>, // For struct generic parameters
    pub struct_generic_bounds: Vec<Option<std::collections::HashMap<DefaultSymbol, TypeDecl>>>, // For struct generic parameter bounds
    pub struct_fields: Vec<Option<Vec<StructField>>>,        // For struct field lists
    pub visibility: Vec<Option<Visibility>>,                 // For struct/impl visibility
    pub impl_methods: Vec<Option<Vec<Rc<MethodFunction>>>>,  // For impl block methods
    pub enum_variants: Vec<Option<Vec<EnumVariantDef>>>,      // For enum declarations
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
            struct_generic_params: Vec::new(),
            struct_generic_bounds: Vec::new(),
            struct_fields: Vec::new(),
            visibility: Vec::new(),
            impl_methods: Vec::new(),
            enum_variants: Vec::new(),
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
            struct_generic_params: Vec::with_capacity(cap),
            struct_generic_bounds: Vec::with_capacity(cap),
            struct_fields: Vec::with_capacity(cap),
            visibility: Vec::with_capacity(cap),
            impl_methods: Vec::with_capacity(cap),
            enum_variants: Vec::with_capacity(cap),
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
            self.struct_generic_params.resize(current_len + extend_count, None);
            self.struct_generic_bounds.resize(current_len + extend_count, None);
            self.struct_fields.resize(current_len + extend_count, None);
            self.visibility.resize(current_len + extend_count, None);
            self.impl_methods.resize(current_len + extend_count, None);
            self.enum_variants.resize(current_len + extend_count, None);
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
            Stmt::StructDecl { name, generic_params, generic_bounds, fields, visibility } => {
                self.stmt_types[index] = StmtType::StructDecl;
                self.struct_name[index] = Some(name);
                self.struct_generic_params[index] = Some(generic_params);
                self.struct_generic_bounds[index] = Some(generic_bounds);
                self.struct_fields[index] = Some(fields);
                self.visibility[index] = Some(visibility);
            }
            Stmt::ImplBlock { target_type, methods } => {
                self.stmt_types[index] = StmtType::ImplBlock;
                self.struct_name[index] = Some(target_type);
                self.impl_methods[index] = Some(methods);
            }
            Stmt::EnumDecl { name, variants, visibility } => {
                self.stmt_types[index] = StmtType::EnumDecl;
                self.struct_name[index] = Some(name);
                self.enum_variants[index] = Some(variants);
                self.visibility[index] = Some(visibility);
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
                    self.expr_val[index]?,
                ))
            }
            StmtType::Var => {
                Some(Stmt::Var(
                    self.symbol_val[index]?,
                    self.type_decl[index].clone(),
                    self.expr_val[index],
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
                    self.block_expr[index]?,
                ))
            }
            StmtType::While => {
                Some(Stmt::While(
                    self.condition[index]?,
                    self.block_expr[index]?,
                ))
            }
            StmtType::StructDecl => {
                Some(Stmt::StructDecl {
                    name: self.struct_name[index]?,
                    generic_params: self.struct_generic_params[index].clone()?,
                    generic_bounds: self.struct_generic_bounds[index].clone()?,
                    fields: self.struct_fields[index].clone()?,
                    visibility: self.visibility[index].clone()?,
                })
            }
            StmtType::ImplBlock => {
                Some(Stmt::ImplBlock {
                    target_type: self.struct_name[index]?,
                    methods: self.impl_methods[index].clone()?,
                })
            }
            StmtType::EnumDecl => {
                Some(Stmt::EnumDecl {
                    name: self.struct_name[index]?,
                    variants: self.enum_variants[index].clone()?,
                    visibility: self.visibility[index].clone()?,
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
