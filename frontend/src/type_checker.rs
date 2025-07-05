use std::collections::HashMap;
use std::rc::Rc;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::ast::*;
use crate::type_decl::*;
use crate::visitor::AstVisitor;

#[derive(Debug)]
pub struct VarState {
    ty: TypeDecl,
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
    pub type_hint: Option<TypeDecl>, // Type hint for Number literal inference
    pub number_usage_context: Vec<(ExprRef, TypeDecl)>, // Track Number expressions and their usage context
    pub variable_expr_mapping: HashMap<DefaultSymbol, ExprRef>, // Track which expression belongs to which variable
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
        last.insert(name, VarState { ty });
    }

    pub fn set_var(&mut self, name: DefaultSymbol, ty: TypeDecl) {
        let last = self.vars.last_mut().unwrap();
        let exist = last.get(&name);
        if let Some(exist) = exist {
            let ety = exist.ty.clone();
            if ety != ty {
                // Re-define with other type
                last.insert(name, VarState { ty });
            }
            // it can overwrite
        } else {
            // or insert a new one
            last.insert(name, VarState { ty });
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
            type_hint: None,
            number_usage_context: Vec::new(),
            variable_expr_mapping: HashMap::new(),
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

        // Pre-scan for explicit type declarations and establish global type context
        let mut global_numeric_type: Option<TypeDecl> = None;
        for s in &statements {
            if let Some(stmt) = self.stmt_pool.get(s.to_index()) {
                match stmt {
                    Stmt::Val(_, Some(type_decl), _) | Stmt::Var(_, Some(type_decl), _) => {
                        if matches!(type_decl, TypeDecl::Int64 | TypeDecl::UInt64) {
                            global_numeric_type = Some(type_decl.clone());
                            break; // Use the first explicit numeric type found
                        }
                    }
                    _ => {}
                }
            }
        }
        
        // Set global type hint if found
        let original_hint = self.type_hint.clone();
        if let Some(ref global_type) = global_numeric_type {
            self.type_hint = Some(global_type.clone());
        }

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

        // Restore original type hint
        self.type_hint = original_hint;

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
            Expr::IfElifElse(cond, then_block, elif_pairs, else_block) => visitor.visit_if_elif_else(cond, then_block, elif_pairs, else_block),
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
            Expr::ArrayLiteral(elements) => visitor.visit_array_literal(elements),
            Expr::ArrayAccess(array, index) => visitor.visit_array_access(array, index),
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
        // Set up context hint for nested expressions
        let original_hint = self.type_hint.clone();
        let result = self.expr_pool.get(expr.to_index()).unwrap().clone().accept(self);
        
        // Context propagation: if this expression resolved to a concrete numeric type,
        // and we don't have a current hint, set it for sibling expressions
        if let Ok(ref result_type) = result {
            if original_hint.is_none() && (result_type == &TypeDecl::Int64 || result_type == &TypeDecl::UInt64) {
                if self.type_hint.is_none() {
                    self.type_hint = Some(result_type.clone());
                }
            }
        }
        
        result
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
        
        // Context propagation: if we have a type hint, propagate it to Number expressions
        if let Some(hint) = self.type_hint.clone() {
            if lhs_ty == TypeDecl::Number && (hint == TypeDecl::Int64 || hint == TypeDecl::UInt64) {
                self.propagate_type_to_number_expr(&lhs, &hint)?;
            }
            if rhs_ty == TypeDecl::Number && (hint == TypeDecl::Int64 || hint == TypeDecl::UInt64) {
                self.propagate_type_to_number_expr(&rhs, &hint)?;
            }
        }
        
        // Record Number usage context for later finalization
        self.record_number_usage_context(&lhs, &lhs_ty, &resolved_lhs_ty)?;
        self.record_number_usage_context(&rhs, &rhs_ty, &resolved_rhs_ty)?;
        
        // Immediate propagation: if one side has concrete type, propagate to Number variables
        if resolved_lhs_ty != TypeDecl::Number && rhs_ty == TypeDecl::Number {
            self.propagate_to_number_variable(&rhs, &resolved_lhs_ty)?;
        }
        if resolved_rhs_ty != TypeDecl::Number && lhs_ty == TypeDecl::Number {
            self.propagate_to_number_variable(&lhs, &resolved_rhs_ty)?;
        }
        
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
        
        let result_type = match op {
            Operator::IAdd if resolved_lhs_ty == TypeDecl::String && resolved_rhs_ty == TypeDecl::String => {
                TypeDecl::String
            }
            Operator::IAdd | Operator::ISub | Operator::IDiv | Operator::IMul => {
                if resolved_lhs_ty == TypeDecl::UInt64 && resolved_rhs_ty == TypeDecl::UInt64 {
                    TypeDecl::UInt64
                } else if resolved_lhs_ty == TypeDecl::Int64 && resolved_rhs_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else {
                    return Err(TypeCheckError::new(format!("Type mismatch: arithmetic operations require matching numeric types, but got {:?} and {:?}", resolved_lhs_ty, resolved_rhs_ty)));
                }
            }
            Operator::LE | Operator::LT | Operator::GE | Operator::GT | Operator::EQ | Operator::NE => {
                if (resolved_lhs_ty == TypeDecl::UInt64 || resolved_lhs_ty == TypeDecl::Int64) && 
                   (resolved_rhs_ty == TypeDecl::UInt64 || resolved_rhs_ty == TypeDecl::Int64) {
                    TypeDecl::Bool
                } else if resolved_lhs_ty == TypeDecl::Bool && resolved_rhs_ty == TypeDecl::Bool {
                    TypeDecl::Bool
                } else {
                    return Err(TypeCheckError::new(format!("Type mismatch: comparison operators require matching types, but got {:?} and {:?}", resolved_lhs_ty, resolved_rhs_ty)));
                }
            }
            Operator::LogicalAnd | Operator::LogicalOr => {
                if resolved_lhs_ty == TypeDecl::Bool && resolved_rhs_ty == TypeDecl::Bool {
                    TypeDecl::Bool
                } else {
                    return Err(TypeCheckError::new(format!("Type mismatch: logical operators require Bool types, but got {:?} and {:?}", resolved_lhs_ty, resolved_rhs_ty)));
                }
            }
        };
        
        Ok(result_type)
    }

    fn visit_block(&mut self, statements: &Vec<StmtRef>) -> Result<TypeDecl, TypeCheckError> {
        let mut last_empty = true;
        let mut last: Option<TypeDecl> = None;
        
        // Pre-scan for explicit type declarations and establish global type context
        let mut global_numeric_type: Option<TypeDecl> = None;
        for s in statements {
            if let Some(stmt) = self.stmt_pool.get(s.to_index()) {
                match stmt {
                    Stmt::Val(_, Some(type_decl), _) | Stmt::Var(_, Some(type_decl), _) => {
                        if matches!(type_decl, TypeDecl::Int64 | TypeDecl::UInt64) {
                            global_numeric_type = Some(type_decl.clone());
                            break; // Use the first explicit numeric type found
                        }
                    }
                    _ => {}
                }
            }
        }
        
        // Set global type hint if found
        let original_hint = self.type_hint.clone();
        if let Some(ref global_type) = global_numeric_type {
            self.type_hint = Some(global_type.clone());
        }
        
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
        
        // Restore original type hint
        self.type_hint = original_hint;

        if let Some(last_type) = last {
            Ok(last_type)
        } else {
            Err(TypeCheckError::new(format!("Type of block mismatch: expected {:?}", last)))
        }
    }


    fn visit_if_elif_else(&mut self, _cond: &ExprRef, then_block: &ExprRef, elif_pairs: &Vec<(ExprRef, ExprRef)>, else_block: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // Collect all block types
        let mut block_types = Vec::new();

        // Check if-block
        let if_block = then_block.clone();
        let is_if_empty = match self.expr_pool.get(if_block.to_index()).unwrap() {
            Expr::Block(expressions) => expressions.is_empty(),
            _ => false,
        };
        if !is_if_empty {
            let if_ty = self.expr_pool.get(if_block.to_index()).unwrap().clone().accept(self)?;
            block_types.push(if_ty);
        }

        // Check elif-blocks
        for (_, elif_block) in elif_pairs {
            let elif_block = elif_block.clone();
            let is_elif_empty = match self.expr_pool.get(elif_block.to_index()).unwrap() {
                Expr::Block(expressions) => expressions.is_empty(),
                _ => false,
            };
            if !is_elif_empty {
                let elif_ty = self.expr_pool.get(elif_block.to_index()).unwrap().clone().accept(self)?;
                block_types.push(elif_ty);
            }
        }

        // Check else-block
        let else_block = else_block.clone();
        let is_else_empty = match self.expr_pool.get(else_block.to_index()).unwrap() {
            Expr::Block(expressions) => expressions.is_empty(),
            _ => false,
        };
        if !is_else_empty {
            let else_ty = self.expr_pool.get(else_block.to_index()).unwrap().clone().accept(self)?;
            block_types.push(else_ty);
        }

        // If no blocks have values or all blocks are empty, return Unit
        if block_types.is_empty() {
            return Ok(TypeDecl::Unit);
        }

        // Check if all blocks have the same type
        let first_type = &block_types[0];
        for block_type in &block_types[1..] {
            if block_type != first_type {
                return Ok(TypeDecl::Unit); // Different types, return Unit
            }
        }

        Ok(first_type.clone())
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
        
        // If we have a type hint from val/var declaration, validate and return the hint type
        if let Some(hint) = self.type_hint.clone() {
            match hint {
                TypeDecl::Int64 => {
                    if let Ok(_val) = num_str.parse::<i64>() {
                        // Return the hinted type - transformation will happen in visit_val or array processing
                        return Ok(hint);
                    } else {
                        return Err(TypeCheckError::new(format!("Cannot convert {} to Int64", num_str)));
                    }
                },
                TypeDecl::UInt64 => {
                    if let Ok(_val) = num_str.parse::<u64>() {
                        // Return the hinted type - transformation will happen in visit_val or array processing
                        return Ok(hint);
                    } else {
                        return Err(TypeCheckError::new(format!("Cannot convert {} to UInt64", num_str)));
                    }
                },
                _ => {
                    // Other types, fall through to default logic
                }
            }
        }
        
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

    fn visit_array_literal(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if elements.is_empty() {
            return Err(TypeCheckError::new("Empty array literals are not supported".to_string()));
        }

        // Save the original type hint to restore later
        let original_hint = self.type_hint.clone();
        
        // If we have a type hint for the array element type, use it for element type inference
        let element_type_hint = if let Some(TypeDecl::Array(element_types, _)) = &self.type_hint {
            if !element_types.is_empty() {
                Some(element_types[0].clone())
            } else {
                None
            }
        } else {
            None
        };

        // Type check all elements with proper type hint for each element
        let mut element_types = Vec::new();
        for element in elements {
            // Set the element type hint for each element individually
            if let Some(ref hint) = element_type_hint {
                self.type_hint = Some(hint.clone());
            }
            
            let element_type = self.visit_expr(element)?;
            element_types.push(element_type);
            
            // Restore original hint after processing each element
            self.type_hint = original_hint.clone();
        }

        // If we have array type hint, handle type inference for all elements
        if let Some(TypeDecl::Array(ref expected_element_types, _)) = original_hint {
            if !expected_element_types.is_empty() {
                let expected_element_type = &expected_element_types[0];
                
                // Handle type inference for each element
                for (i, element) in elements.iter().enumerate() {
                    match &element_types[i] {
                        TypeDecl::Number => {
                            // Transform Number literals to the expected type
                            self.transform_numeric_expr(element, expected_element_type)?;
                            element_types[i] = expected_element_type.clone();
                        },
                        actual_type if actual_type == expected_element_type => {
                            // Element already has the expected type, but may need AST transformation
                            // Check if this is a number literal that needs transformation
                            if let Some(expr) = self.expr_pool.get(element.to_index()) {
                                if matches!(expr, Expr::Number(_)) {
                                    self.transform_numeric_expr(element, expected_element_type)?;
                                }
                            }
                        },
                        TypeDecl::Unknown => {
                            // For variables with unknown type, try to infer from context
                            element_types[i] = expected_element_type.clone();
                        },
                        actual_type if actual_type != expected_element_type => {
                            // Check if type conversion is possible
                            match (actual_type, expected_element_type) {
                                (TypeDecl::Int64, TypeDecl::UInt64) | 
                                (TypeDecl::UInt64, TypeDecl::Int64) => {
                                    return Err(TypeCheckError::new(format!(
                                        "Cannot mix signed and unsigned integers in array. Element {} has type {:?} but expected {:?}",
                                        i, actual_type, expected_element_type
                                    )));
                                },
                                _ => {
                                    // Accept the actual type if it matches expectations
                                    if actual_type == expected_element_type {
                                        // Already matches, no change needed
                                    } else {
                                        return Err(TypeCheckError::new(format!(
                                            "Array element {} has type {:?} but expected {:?}",
                                            i, actual_type, expected_element_type
                                        )));
                                    }
                                }
                            }
                        },
                        _ => {
                            // Type already matches expected type
                        }
                    }
                }
            }
        }

        // Restore the original type hint
        self.type_hint = original_hint;

        let first_type = &element_types[0];
        for (i, element_type) in element_types.iter().enumerate() {
            if element_type != first_type {
                return Err(TypeCheckError::new(format!(
                    "Array elements must have the same type, but element {} has type {:?} while first element has type {:?}",
                    i, element_type, first_type
                )));
            }
        }

        Ok(TypeDecl::Array(element_types, elements.len()))
    }

    fn visit_array_access(&mut self, array: &ExprRef, index: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let array_type = self.visit_expr(array)?;
        let index_type = self.visit_expr(index)?;

        // Index must be an integer type
        if index_type != TypeDecl::UInt64 && index_type != TypeDecl::Int64 {
            return Err(TypeCheckError::new(format!(
                "Array index must be an integer type, but got {:?}", index_type
            )));
        }

        // Array must be an array type
        match array_type {
            TypeDecl::Array(ref element_types, _size) => {
                if element_types.is_empty() {
                    return Err(TypeCheckError::new("Cannot access elements of empty array".to_string()));
                }
                Ok(element_types[0].clone())
            }
            _ => Err(TypeCheckError::new(format!(
                "Cannot index into non-array type {:?}", array_type
            )))
        }
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
        let type_decl = type_decl.clone();
        
        // Set type hint: explicit declaration takes priority, otherwise use current hint
        let old_hint = self.type_hint.clone();
        if let Some(decl) = &type_decl {
            match decl {
                TypeDecl::Array(element_types, _) => {
                    // For array types, set the array type as hint for array literal processing
                    if !element_types.is_empty() {
                        self.type_hint = Some(decl.clone());
                    }
                },
                _ if decl != &TypeDecl::Unknown && decl != &TypeDecl::Number => {
                    self.type_hint = Some(decl.clone());
                },
                _ => {}
            }
        }
        
        // Visit the expression with type hint context
        let expr_ty = self.visit_expr(&expr_ref)?;
        
        // Record variable-expression mapping for Number types (remove old mapping)
        if expr_ty == TypeDecl::Number || (expr_ty != TypeDecl::Number && self.has_number_in_expr(&expr_ref)) {
            self.variable_expr_mapping.insert(name, expr_ref.clone());
        } else {
            // Remove old mapping for non-Number types to prevent stale references
            self.variable_expr_mapping.remove(&name);
            // Also remove from number_usage_context to prevent stale type inference
            let indices_to_remove: Vec<usize> = self.number_usage_context
                .iter()
                .enumerate()
                .filter_map(|(i, (old_expr, _))| {
                    if self.is_old_number_for_variable(name, old_expr) {
                        Some(i)
                    } else {
                        None
                    }
                })
                .collect();
            
            // Remove in reverse order to maintain valid indices
            for &index in indices_to_remove.iter().rev() {
                self.number_usage_context.remove(index);
            }
        }
        
        // Apply type transformation if needed
        if type_decl.is_none() && expr_ty == TypeDecl::Number {
            // No explicit type, but we have a Number - use type hint if available
            if let Some(hint) = self.type_hint.clone() {
                if matches!(hint, TypeDecl::Int64 | TypeDecl::UInt64) {
                    // Transform Number to hinted type
                    self.transform_numeric_expr(&expr_ref, &hint)?;
                }
            }
        } else if type_decl.is_some() && type_decl.as_ref().unwrap() == &TypeDecl::Unknown && expr_ty == TypeDecl::Int64 {
            // Unknown type declaration with Int64 inference - also transform
            if let Some(hint) = self.type_hint.clone() {
                if matches!(hint, TypeDecl::Int64 | TypeDecl::UInt64) {
                    self.transform_numeric_expr(&expr_ref, &hint)?;
                }
            }
        } else if let Some(decl) = &type_decl {
            if decl != &TypeDecl::Unknown && decl != &TypeDecl::Number && expr_ty == *decl {
                // Expression returned the hinted type, transform Number literals to concrete type
                if let Some(expr) = self.expr_pool.get(expr_ref.to_index()) {
                    if let Expr::Number(_) = expr {
                        self.transform_numeric_expr(&expr_ref, decl)?;
                    }
                }
            }
        }
        
        // Store the variable with the final type (after transformation)
        let final_type = match (&type_decl, &expr_ty) {
            (Some(TypeDecl::Unknown), _) => expr_ty,
            (Some(decl), _) if decl != &TypeDecl::Unknown && decl != &TypeDecl::Number => decl.clone(),
            (None, _) => expr_ty,
            _ => expr_ty,
        };
        
        // Set the variable directly without calling process_val_type to avoid double evaluation
        self.context.set_var(name, final_type);
        
        // Restore previous type hint
        self.type_hint = old_hint;
        
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
                    // Update the variable's type
                    self.context.update_var_type(*name, resolved_ty.clone());
                }
            }
        }
        Ok(())
    }

    // Record Number usage context for identifiers
    fn record_number_usage_context(&mut self, expr_ref: &ExprRef, original_ty: &TypeDecl, resolved_ty: &TypeDecl) -> Result<(), TypeCheckError> {
        if original_ty == &TypeDecl::Number && resolved_ty != &TypeDecl::Number {
            if let Some(expr) = self.expr_pool.get(expr_ref.to_index()) {
                if let Expr::Identifier(name) = expr {
                    // Find all Number expressions that might belong to this variable
                    // and record the context type
                    for i in 0..self.expr_pool.len() {
                        if let Some(candidate_expr) = self.expr_pool.get(i) {
                            if let Expr::Number(_) = candidate_expr {
                                let candidate_ref = ExprRef(i as u32);
                                // Check if this Number might be associated with this variable
                                if self.is_number_for_variable(*name, &candidate_ref) {
                                    self.number_usage_context.push((candidate_ref, resolved_ty.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
        
        Ok(())
    }

    // Check if an expression contains Number literals
    fn has_number_in_expr(&self, expr_ref: &ExprRef) -> bool {
        if let Some(expr) = self.expr_pool.get(expr_ref.to_index()) {
            match expr {
                Expr::Number(_) => true,
                _ => false, // For now, only check direct Number literals
            }
        } else {
            false
        }
    }

    // Check if a Number expression is associated with a specific variable
    fn is_number_for_variable(&self, var_name: DefaultSymbol, number_expr_ref: &ExprRef) -> bool {
        // Use the recorded mapping to check if this Number expression belongs to this variable
        if let Some(mapped_expr_ref) = self.variable_expr_mapping.get(&var_name) {
            return mapped_expr_ref == number_expr_ref;
        }
        false
    }
    
    // Check if an old Number expression might be associated with a variable for cleanup
    fn is_old_number_for_variable(&self, _var_name: DefaultSymbol, number_expr_ref: &ExprRef) -> bool {
        // Check if this Number expression was previously mapped to this variable
        // This is used for cleanup when variables are redefined
        if let Some(expr) = self.expr_pool.get(number_expr_ref.to_index()) {
            if let Expr::Number(_) = expr {
                // For now, we'll be conservative and remove all Number contexts when variables are redefined
                return true;
            }
        }
        false
    }

    // Propagate concrete type to Number variable immediately
    fn propagate_to_number_variable(&mut self, expr_ref: &ExprRef, target_type: &TypeDecl) -> Result<(), TypeCheckError> {
        if let Some(expr) = self.expr_pool.get(expr_ref.to_index()) {
            if let Expr::Identifier(name) = expr {
                if let Some(var_type) = self.context.get_var(*name) {
                    if var_type == TypeDecl::Number {
                        // Find and record the Number expression for this variable
                        for i in 0..self.expr_pool.len() {
                            if let Some(candidate_expr) = self.expr_pool.get(i) {
                                if let Expr::Number(_) = candidate_expr {
                                    let candidate_ref = ExprRef(i as u32);
                                    if self.is_number_for_variable(*name, &candidate_ref) {
                                        self.number_usage_context.push((candidate_ref, target_type.clone()));
                                        // Update variable type in context
                                        self.context.update_var_type(*name, target_type.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // Finalize any remaining Number types with context-aware inference
    fn finalize_number_types(&mut self) -> Result<(), TypeCheckError> {
        // Use recorded context information to transform Number expressions
        let context_info = self.number_usage_context.clone();
        for (expr_ref, target_type) in context_info {
            if let Some(expr) = self.expr_pool.get(expr_ref.to_index()) {
                if let Expr::Number(_) = expr {
                    self.transform_numeric_expr(&expr_ref, &target_type)?;
                    
                    // Update variable types in context if this expression is mapped to a variable
                    for (var_name, mapped_expr_ref) in &self.variable_expr_mapping.clone() {
                        if mapped_expr_ref == &expr_ref {
                            self.context.update_var_type(*var_name, target_type.clone());
                        }
                    }
                }
            }
        }
        
        // Second pass: handle any remaining Number types by using variable context
        let expr_len = self.expr_pool.len();
        for i in 0..expr_len {
            if let Some(expr) = self.expr_pool.get(i) {
                if let Expr::Number(_) = expr {
                    let expr_ref = ExprRef(i as u32);
                    
                    // Find if this Number is associated with a variable and use its final type
                    let mut target_type = TypeDecl::UInt64; // default
                    
                    for (var_name, mapped_expr_ref) in &self.variable_expr_mapping {
                        if mapped_expr_ref == &expr_ref {
                            // Check the current type of this variable in context
                            if let Some(var_type) = self.context.get_var(*var_name) {
                                if var_type != TypeDecl::Number {
                                    target_type = var_type;
                                    break;
                                }
                            }
                        }
                    }
                    
                    self.transform_numeric_expr(&expr_ref, &target_type)?;
                    
                    // Update variable types in context if this expression is mapped to a variable
                    for (var_name, mapped_expr_ref) in &self.variable_expr_mapping.clone() {
                        if mapped_expr_ref == &expr_ref {
                            self.context.update_var_type(*var_name, target_type.clone());
                        }
                    }
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
            
            // Two Number types - check if we have a context hint, otherwise default to UInt64
            (TypeDecl::Number, TypeDecl::Number) => {
                if let Some(hint) = &self.type_hint {
                    match hint {
                        TypeDecl::Int64 => Ok((TypeDecl::Int64, TypeDecl::Int64)),
                        TypeDecl::UInt64 => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
                        _ => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
                    }
                } else {
                    Ok((TypeDecl::UInt64, TypeDecl::UInt64))
                }
            },
            
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
    
    // Propagate type to Number expression and associated variables
    fn propagate_type_to_number_expr(&mut self, expr_ref: &ExprRef, target_type: &TypeDecl) -> Result<(), TypeCheckError> {
        if let Some(expr) = self.expr_pool.get(expr_ref.to_index()) {
            match expr {
                Expr::Identifier(name) => {
                    // If this is an identifier with Number type, update it
                    if let Some(var_type) = self.context.get_var(*name) {
                        if var_type == TypeDecl::Number {
                            self.context.update_var_type(*name, target_type.clone());
                            // Also record for Number expression transformation
                            if let Some(mapped_expr) = self.variable_expr_mapping.get(name) {
                                self.number_usage_context.push((mapped_expr.clone(), target_type.clone()));
                            }
                        }
                    }
                },
                Expr::Number(_) => {
                    // Direct Number literal
                    self.number_usage_context.push((expr_ref.clone(), target_type.clone()));
                },
                _ => {
                    // For other expression types, we might need to recurse
                }
            }
        }
        Ok(())
    }
}