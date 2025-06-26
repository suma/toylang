use std::collections::HashMap;
use std::rc::Rc;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::ast::*;
use crate::type_decl::*;
use crate::visitor::AstVisitor;

#[derive(Debug)]
pub struct VarState {
    ty: TypeDecl,
    is_const: bool,
}
#[derive(Debug)]
pub struct TypeCheckContext {
    vars: Vec<HashMap<DefaultSymbol, VarState>>,
    functions: HashMap<DefaultSymbol, Rc<Function>>,
}

#[derive(Debug)]
pub struct TypeCheckError {
    msg: String,
}

pub struct TypeCheckerVisitor <'a, 'b, 'c> {
    pub stmt_pool: &'a StmtPool,
    pub expr_pool: &'b mut ExprPool,
    pub string_interner: &'c DefaultStringInterner,
    pub context: TypeCheckContext,
    pub call_depth: usize,
    pub is_checked_fn: HashMap<DefaultSymbol, Option<TypeDecl>>, // None -> in progress, Some -> Done
}

impl std::fmt::Display for TypeCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl TypeCheckError {
    pub fn new(msg: String) -> Self {
        Self { msg }
    }
}

impl TypeCheckContext {
    pub fn new() -> Self {
        Self {
            vars: vec![HashMap::new()],
            functions: HashMap::new(),
        }
    }

    pub fn set_val(&mut self, name: DefaultSymbol, ty: TypeDecl) {
        let last = self.vars.last_mut().unwrap();
        last.insert(name, VarState { ty, is_const: true });
    }

    pub fn set_var(&mut self, name: DefaultSymbol, ty: TypeDecl) {
        let last = self.vars.last_mut().unwrap();
        let exist = last.get(&name);
        if let Some(exist) = exist {
            if exist.is_const {
                panic!("Cannot re-assign const variable: {:?}", name);
            }
            let ety = exist.ty.clone();
            if ety != ty {
                panic!("Cannot re-assign variable: {:?} with different type: {:?} != {:?}", name, ety, ty);
            }
            // it can overwrite
        } else {
            // or insert a new one
            last.insert(name, VarState { ty, is_const: false });
        }
    }

    pub fn set_fn(&mut self, name: DefaultSymbol, f: Rc<Function>) {
        self.functions.insert(name, f);
    }

    pub fn get_var(&self, name: DefaultSymbol) -> Option<TypeDecl> {
        for v in self.vars.iter().rev() {
            let v_val = v.get(&name);
            if let Some(val) = v_val {
                return Some(val.ty.clone());
            }
        }
        None
    }

    pub fn get_fn(&self, name: DefaultSymbol) -> Option<Rc<Function>> {
        if let Some(val) = self.functions.get(&name) {
            Some(val.clone())
        } else {
            None
        }
    }

    pub fn update_var_type(&mut self, name: DefaultSymbol, new_ty: TypeDecl) -> bool {
        for v in self.vars.iter_mut().rev() {
            if let Some(var_state) = v.get_mut(&name) {
                var_state.ty = new_ty;
                return true;
            }
        }
        false // Variable not found
    }
}


impl<'a, 'b, 'c> TypeCheckerVisitor<'a, 'b, 'c> {
    pub fn new(stmt_pool: &'a StmtPool, expr_pool: &'b mut ExprPool, string_interner: &'c DefaultStringInterner) -> Self {
        Self {
            stmt_pool,
            expr_pool,
            string_interner: string_interner,
            context: TypeCheckContext::new(),
            call_depth: 0,
            is_checked_fn: HashMap::new(),
        }
    }

    pub fn push_context(&mut self) {
        self.context.vars.push(HashMap::new());
    }

    pub fn pop_context(&mut self) {
        self.context.vars.pop();
    }

    pub fn add_function(&mut self, f: Rc<Function>) {
        self.context.set_fn(f.name, f.clone());
    }

    fn process_val_type(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let expr_ty = match expr {
            Some(e) => {
                let ty = self.visit_expr(e)?;
                if ty == TypeDecl::Unit {
                    return Err(TypeCheckError::new(format!("Type mismatch: expected <expression>, but got {:?}", ty)));
                }
                Some(ty)
            }
            None => None,
        };

        match (type_decl, expr_ty.as_ref()) {
            (Some(TypeDecl::Unknown), Some(ty)) => {
                self.context.set_var(name, ty.clone());
            }
            (Some(decl), Some(ty)) => {
                if decl != ty {
                    return Err(TypeCheckError::new(format!("Type mismatch: expected {:?}, but got {:?}", decl, ty)));
                }
                self.context.set_var(name, ty.clone());
            }
            (None, Some(ty)) => {
                // No explicit type declaration - store the inferred type
                self.context.set_var(name, ty.clone());
            }
            _ => (),
        }

        Ok(TypeDecl::Unit)
    }

    pub fn type_check(&mut self, func: Rc<Function>) -> Result<TypeDecl, TypeCheckError> {
        let mut last = TypeDecl::Unit;
        let s = func.code.clone();

        // Is already checked
        match self.is_checked_fn.get(&func.name) {
            Some(Some(result_ty)) => return Ok(result_ty.clone()),  // already checked
            Some(None) => return Ok(TypeDecl::Unknown), // now checking
            None => (),
        }

        // Now checking...
        self.is_checked_fn.insert(func.name, None);

        self.call_depth += 1;

        let statements = match self.stmt_pool.get(s.to_index()).unwrap() {
            Stmt::Expression(e) => {
                match self.expr_pool.0.get(e.to_index()).unwrap() {
                    Expr::Block(statements) => {
                        statements.clone()  // TODO: I want to avoid clone
                    }
                    _ => {
                        panic!("type_check: expected block but {:?}", self.expr_pool.get(s.to_index()).unwrap());
                    }
                }
            }
            _ => panic!("type_check: expected block but {:?}", self.expr_pool.get(s.to_index()).unwrap()),
        };

        self.push_context();
        // Define variable of argument for this `func`
        func.parameter.iter().for_each(|(name, type_decl)| {
            self.context.set_var(*name, type_decl.clone());
        });

        for stmt in statements {
            let res = self.stmt_pool.get(stmt.to_index()).unwrap().clone().accept(self);
            if res.is_err() {
                return res;
            } else {
                last = res?;
            }
        }
        self.pop_context();
        self.call_depth -= 1;

        // Final pass: convert any remaining Number literals to default type (UInt64)
        self.finalize_number_types()?;
        
        self.is_checked_fn.insert(func.name, Some(last.clone()));
        Ok(last)
    }
}
pub trait Acceptable {
    fn accept(&mut self, visitor: &mut dyn AstVisitor) -> Result<TypeDecl, TypeCheckError>;
}

impl Acceptable for Expr {
    fn accept(&mut self, visitor: &mut dyn AstVisitor) -> Result<TypeDecl, TypeCheckError> {
        match self {
            Expr::Binary(op, lhs, rhs) => visitor.visit_binary(op, lhs, rhs),
            Expr::Block(statements) => visitor.visit_block(statements),
            Expr::IfElse(cond, then_block, else_block) => visitor.visit_if_else(cond, then_block, else_block),
            Expr::Assign(lhs, rhs) => visitor.visit_assign(lhs, rhs),
            Expr::Identifier(name) => visitor.visit_identifier(*name),
            Expr::Call(fn_name, args) => visitor.visit_call(*fn_name, args),
            Expr::Int64(val) => visitor.visit_int64_literal(val),
            Expr::UInt64(val) => visitor.visit_uint64_literal(val),
            Expr::Number(val) => visitor.visit_number_literal(*val),
            Expr::String(val) => visitor.visit_string_literal(*val),
            Expr::True | Expr::False => visitor.visit_boolean_literal(self),
            Expr::Null => visitor.visit_null_literal(),
            Expr::ExprList(items) => visitor.visit_expr_list(items),
        }
    }
}

impl Acceptable for Stmt {
    fn accept(&mut self, visitor: &mut dyn AstVisitor) -> Result<TypeDecl, TypeCheckError> {
        match self {
            Stmt::Expression(expr) => visitor.visit_expression_stmt(expr),
            Stmt::Var(name, type_decl, expr) => visitor.visit_var(*name, type_decl, expr),
            Stmt::Val(name, type_decl, expr) => visitor.visit_val(*name, type_decl, expr),
            Stmt::Return(expr) => visitor.visit_return(expr),
            Stmt::For(init, cond, step, body) => visitor.visit_for(*init, cond, step, body),
            Stmt::While(cond, body) => visitor.visit_while(cond, body),
            Stmt::Break => visitor.visit_break(),
            Stmt::Continue => visitor.visit_continue()
        }
    }
}

impl<'a, 'b, 'c> AstVisitor for TypeCheckerVisitor<'a, 'b, 'c> {
    fn visit_expr(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.expr_pool.get(expr.to_index()).unwrap().clone().accept(self)
    }

    fn visit_stmt(&mut self, stmt: &StmtRef) -> Result<TypeDecl, TypeCheckError> {
        self.stmt_pool.get(stmt.to_index()).unwrap_or(&Stmt::Break).clone().accept(self)
    }

    fn visit_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let op = op.clone();
        let lhs = lhs.clone();
        let rhs = rhs.clone();
        let lhs_ty = self.expr_pool.get(lhs.to_index()).unwrap().clone().accept(self)?;
        let rhs_ty = self.expr_pool.get(rhs.to_index()).unwrap().clone().accept(self)?;
        
        // Resolve types with automatic conversion for Number type
        let (resolved_lhs_ty, resolved_rhs_ty) = self.resolve_numeric_types(&lhs_ty, &rhs_ty)?;
        
        // Transform AST nodes if type conversion occurred
        if lhs_ty == TypeDecl::Number && resolved_lhs_ty != TypeDecl::Number {
            self.transform_numeric_expr(&lhs, &resolved_lhs_ty)?;
        }
        if rhs_ty == TypeDecl::Number && resolved_rhs_ty != TypeDecl::Number {
            self.transform_numeric_expr(&rhs, &resolved_rhs_ty)?;
        }
        
        // Update variable types if identifiers were involved in type conversion
        self.update_identifier_types(&lhs, &lhs_ty, &resolved_lhs_ty)?;
        self.update_identifier_types(&rhs, &rhs_ty, &resolved_rhs_ty)?;
        match op {
            Operator::IAdd if resolved_lhs_ty == TypeDecl::String && resolved_rhs_ty == TypeDecl::String => {
                Ok(TypeDecl::String)
            }
            Operator::IAdd | Operator::ISub | Operator::IDiv | Operator::IMul => {
                if resolved_lhs_ty == TypeDecl::UInt64 && resolved_rhs_ty == TypeDecl::UInt64 {
                    Ok(TypeDecl::UInt64)
                } else if resolved_lhs_ty == TypeDecl::Int64 && resolved_rhs_ty == TypeDecl::Int64 {
                    Ok(TypeDecl::Int64)
                } else {
                    return Err(TypeCheckError::new(format!("Type mismatch: arithmetic operations require matching numeric types, but got {:?} and {:?}", resolved_lhs_ty, resolved_rhs_ty)));
                }
            }
            Operator::LE | Operator::LT | Operator::GE | Operator::GT | Operator::EQ | Operator::NE => {
                if (resolved_lhs_ty == TypeDecl::UInt64 || resolved_lhs_ty == TypeDecl::Int64) && 
                   (resolved_rhs_ty == TypeDecl::UInt64 || resolved_rhs_ty == TypeDecl::Int64) {
                    Ok(TypeDecl::Bool)
                } else if resolved_lhs_ty == TypeDecl::Bool && resolved_rhs_ty == TypeDecl::Bool {
                    Ok(TypeDecl::Bool)
                } else {
                    return Err(TypeCheckError::new(format!("Type mismatch: comparison operators require matching types, but got {:?} and {:?}", resolved_lhs_ty, resolved_rhs_ty)));
                }
            }
            Operator::LogicalAnd | Operator::LogicalOr => {
                if resolved_lhs_ty == TypeDecl::Bool && resolved_rhs_ty == TypeDecl::Bool {
                    Ok(TypeDecl::Bool)
                } else {
                    Err(TypeCheckError::new(format!("Type mismatch: logical operators require Bool types, but got {:?} and {:?}", resolved_lhs_ty, resolved_rhs_ty)))
                }
            }
        }
    }

    fn visit_block(&mut self, statements: &Vec<StmtRef>) -> Result<TypeDecl, TypeCheckError> {
        let mut last_empty = true;
        let mut last: Option<TypeDecl> = None;
        // This code assumes Block(expression) don't make nested function
        // so `return` expression always return for this context.
        for s in statements {
            let stmt = self.stmt_pool.get(s.to_index()).unwrap();
            let stmt_type = match stmt {
                Stmt::Return(None) => Ok(TypeDecl::Unit),
                Stmt::Return(ret_ty) => {
                    if let Some(e) = ret_ty {
                        let e = e.clone();
                        let ty = self.expr_pool.get(e.to_index()).unwrap().clone().accept(self)?;
                        if last_empty {
                            last_empty = false;
                            Ok(ty)
                        } else if let Some(last_ty) = last.clone() {
                            if last_ty == ty {
                                Ok(ty)
                            } else {
                                let ret_expr = self.expr_pool.get(e.to_index()).unwrap();
                                Err(TypeCheckError::new(format!("Type mismatch(return): expected {:?}, but got {:?} : {:?}", last, ret_expr, s)))?
                            }
                        } else {
                            Ok(ty)
                        }
                    } else {
                        Ok(TypeDecl::Unit)
                    }
                }
                _ => self.stmt_pool.get(s.to_index()).unwrap().clone().accept(self),
            };

            match stmt_type {
                Ok(def_ty) => last = Some(def_ty),
                Err(e) => return Err(e),
            }
        }

        if let Some(last_type) = last {
            Ok(last_type)
        } else {
            Err(TypeCheckError::new(format!("Type of block mismatch: expected {:?}", last)))
        }
    }

    fn visit_if_else(&mut self, _cond: &ExprRef, then_block: &ExprRef, else_block: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let is_block_empty = |blk: ExprRef| -> bool {
            match self.expr_pool.get(blk.to_index()).unwrap() {
                Expr::Block(expressions) => {
                    expressions.is_empty()
                }
                _ => false,
            }
        };
        let blk1 = then_block.clone();
        let blk2 = else_block.clone();
        if is_block_empty(blk1) || is_block_empty(blk2) {
            return Ok(TypeDecl::Unit); // ignore to infer empty of blk
        }

        let blk1_ty = self.expr_pool.get(blk1.to_index()).unwrap().clone().accept(self)?;
        let blk2_ty = self.expr_pool.get(blk2.to_index()).unwrap().clone().accept(self)?;
        if blk1_ty != blk2_ty {
            Ok(TypeDecl::Unit)
        } else {
            Ok(blk1_ty)
        }
    }

    fn visit_assign(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let lhs = lhs.clone();
        let rhs = rhs.clone();
        let lhs_ty = self.expr_pool.get(lhs.to_index()).unwrap().clone().accept(self)?;
        let rhs_ty = self.expr_pool.get(rhs.to_index()).unwrap().clone().accept(self)?;
        if lhs_ty != rhs_ty {
            return Err(TypeCheckError::new(format!("Type mismatch: lhs expected {:?}, but rhs got {:?}", lhs_ty, rhs_ty)));
        }
        Ok(lhs_ty)
    }

    fn visit_identifier(&mut self, name: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        if let Some(val_type) = self.context.get_var(name) {
            // Return the stored type, which may be Number for type inference
            Ok(val_type.clone())
        } else if let Some(fun) = self.context.get_fn(name) {
            Ok(fun.return_type.clone().unwrap_or(TypeDecl::Unknown))
        } else {
            let name = self.string_interner.resolve(name).unwrap_or("<NOT_FOUND>");
            return Err(TypeCheckError::new(format!("Identifier {:?} not found", name)));
        }
    }

    fn visit_call(&mut self, fn_name: DefaultSymbol, _args: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.push_context();
        if let Some(fun) = self.context.get_fn(fn_name) {
            let status = self.is_checked_fn.get(&fn_name);
            if status.is_none() || status.clone().unwrap().is_none() {
                // not checked yet
                let fun = self.context.get_fn(fn_name).unwrap();
                self.type_check(fun.clone())?;
            }

            self.pop_context();
            Ok(fun.return_type.clone().unwrap_or(TypeDecl::Unknown))
        } else {
            self.pop_context();
            let fn_name = self.string_interner.resolve(fn_name).unwrap_or("<NOT_FOUND>");
            Err(TypeCheckError::new(format!("Function {:?} not found", fn_name)))
        }
    }

    fn visit_int64_literal(&mut self, _value: &i64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Int64)
    }

    fn visit_uint64_literal(&mut self, _value: &u64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::UInt64)
    }

    fn visit_number_literal(&mut self, value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        let num_str = self.string_interner.resolve(value)
            .ok_or_else(|| TypeCheckError::new("Failed to resolve number literal".to_string()))?;
        
        // Parse the number and determine appropriate type
        if let Ok(val) = num_str.parse::<i64>() {
            if val >= 0 && val <= (i64::MAX) {
                // Positive number that fits in both i64 and u64 - use Number for inference
                Ok(TypeDecl::Number)
            } else {
                // Negative number or very large positive - must be i64
                Ok(TypeDecl::Int64)
            }
        } else if let Ok(_val) = num_str.parse::<u64>() {
            // Very large positive number that doesn't fit in i64 - must be u64
            Ok(TypeDecl::UInt64)
        } else {
            Err(TypeCheckError::new(format!("Invalid number literal: {}", num_str)))
        }
    }

    fn visit_string_literal(&mut self, _value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::String)
    }

    fn visit_boolean_literal(&mut self, _value: &Expr) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Bool)

    }

    fn visit_null_literal(&mut self) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Any)
    }

    fn visit_expr_list(&mut self, _items: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }

    fn visit_expression_stmt(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.expr_pool.get(expr.to_index()).unwrap().clone().accept(self)
    }

    fn visit_var(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let type_decl = type_decl.clone();
        let expr = expr.clone();
        self.process_val_type(name, &type_decl, &expr)?;
        Ok(TypeDecl::Unit)
    }

    fn visit_val(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let expr_ref = expr.clone();
        let expr = Some(expr.clone());
        let type_decl = type_decl.clone();
        
        // Visit the expression first to get its type
        let expr_ty = self.visit_expr(&expr_ref)?;
        
        // If it's a Number type and we have an explicit type declaration, convert it
        if expr_ty == TypeDecl::Number {
            if let Some(decl) = &type_decl {
                if decl != &TypeDecl::Unknown {
                    // Transform to the explicitly declared type
                    self.transform_numeric_expr(&expr_ref, decl)?;
                }
            }
            // If no explicit type is declared, leave as Number for context-based inference
        }
        
        self.process_val_type(name, &type_decl, &expr)?;
        Ok(TypeDecl::Unit)
    }

    fn visit_return(&mut self, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if expr.is_none() {
            Ok(TypeDecl::Unit)
        } else {
            let e = expr.unwrap();
            self.expr_pool.get(e.to_index()).unwrap().clone().accept(self)?;
            Ok(TypeDecl::Unit)
        }
    }

    fn visit_for(&mut self, init: DefaultSymbol, _cond: &ExprRef, range: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.push_context();
        let range_ty = self.expr_pool.get(range.to_index()).unwrap().clone().accept(self)?;
        let ty = Some(range_ty);
        self.process_val_type(init, &ty, &Some(*range))?;
        let res = self.expr_pool.get(body.to_index()).unwrap().clone().accept(self);
        self.pop_context();
        res
    }

    fn visit_while(&mut self, _cond: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.expr_pool.get(body.to_index()).unwrap().clone().accept(self)
    }

    fn visit_break(&mut self) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }

    fn visit_continue(&mut self) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }
}

impl<'a, 'b, 'c> TypeCheckerVisitor<'a, 'b, 'c> {
    // Transform Expr::Number nodes to concrete types based on resolved types
    fn transform_numeric_expr(&mut self, expr_ref: &ExprRef, target_type: &TypeDecl) -> Result<(), TypeCheckError> {
        if let Some(expr) = self.expr_pool.get_mut(expr_ref.to_index()) {
            if let Expr::Number(value) = expr {
                let num_str = self.string_interner.resolve(*value)
                    .ok_or_else(|| TypeCheckError::new("Failed to resolve number literal".to_string()))?;
                
                match target_type {
                    TypeDecl::UInt64 => {
                        if let Ok(val) = num_str.parse::<u64>() {
                            *expr = Expr::UInt64(val);
                        } else {
                            return Err(TypeCheckError::new(format!("Cannot convert {} to UInt64", num_str)));
                        }
                    },
                    TypeDecl::Int64 => {
                        if let Ok(val) = num_str.parse::<i64>() {
                            *expr = Expr::Int64(val);
                        } else {
                            return Err(TypeCheckError::new(format!("Cannot convert {} to Int64", num_str)));
                        }
                    },
                    _ => {
                        return Err(TypeCheckError::new(format!("Cannot transform number to type: {:?}", target_type)));
                    }
                }
            }
        }
        Ok(())
    }

    // Update variable type in context if identifier was type-converted
    fn update_identifier_types(&mut self, expr_ref: &ExprRef, original_ty: &TypeDecl, resolved_ty: &TypeDecl) -> Result<(), TypeCheckError> {
        if original_ty == &TypeDecl::Number && resolved_ty != &TypeDecl::Number {
            if let Some(expr) = self.expr_pool.get(expr_ref.to_index()) {
                if let Expr::Identifier(name) = expr {
                    // Update the variable's type in context while preserving is_const flag
                    self.context.update_var_type(*name, resolved_ty.clone());
                }
            }
        }
        Ok(())
    }

    // Finalize any remaining Number types to default UInt64
    fn finalize_number_types(&mut self) -> Result<(), TypeCheckError> {
        let expr_len = self.expr_pool.len();
        for i in 0..expr_len {
            if let Some(expr) = self.expr_pool.get(i) {
                if let Expr::Number(_) = expr {
                    let expr_ref = ExprRef(i as u32);
                    self.transform_numeric_expr(&expr_ref, &TypeDecl::UInt64)?;
                }
            }
        }
        Ok(())
    }

    // Helper method to resolve numeric types with automatic conversion
    fn resolve_numeric_types(&self, lhs_ty: &TypeDecl, rhs_ty: &TypeDecl) -> Result<(TypeDecl, TypeDecl), TypeCheckError> {
        match (lhs_ty, rhs_ty) {
            // Both types are already concrete - no conversion needed
            (TypeDecl::UInt64, TypeDecl::UInt64) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            (TypeDecl::Int64, TypeDecl::Int64) => Ok((TypeDecl::Int64, TypeDecl::Int64)),
            (TypeDecl::Bool, TypeDecl::Bool) => Ok((TypeDecl::Bool, TypeDecl::Bool)),
            (TypeDecl::String, TypeDecl::String) => Ok((TypeDecl::String, TypeDecl::String)),
            
            // Number type automatic conversion
            (TypeDecl::Number, TypeDecl::UInt64) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            (TypeDecl::UInt64, TypeDecl::Number) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            (TypeDecl::Number, TypeDecl::Int64) => Ok((TypeDecl::Int64, TypeDecl::Int64)),
            (TypeDecl::Int64, TypeDecl::Number) => Ok((TypeDecl::Int64, TypeDecl::Int64)),
            
            // Two Number types - default to UInt64 for positive literals
            (TypeDecl::Number, TypeDecl::Number) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            
            // Cross-type operations (UInt64 vs Int64) - generally not allowed for safety
            (TypeDecl::UInt64, TypeDecl::Int64) | (TypeDecl::Int64, TypeDecl::UInt64) => {
                Err(TypeCheckError::new(format!("Cannot mix signed and unsigned integer types: {:?} and {:?}", lhs_ty, rhs_ty)))
            },
            
            // Other type mismatches
            _ => {
                if lhs_ty == rhs_ty {
                    Ok((lhs_ty.clone(), rhs_ty.clone()))
                } else {
                    Err(TypeCheckError::new(format!("Type mismatch: cannot convert between {:?} and {:?}", lhs_ty, rhs_ty)))
                }
            }
        }
    }
}