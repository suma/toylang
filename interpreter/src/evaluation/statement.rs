use std::cell::RefCell;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;
use crate::environment::VariableSetType;
use crate::object::Object;
use crate::error::InterpreterError;
use super::{convert_object, EvaluationContext, EvaluationResult};

impl EvaluationContext<'_> {
    pub(super) fn execute_for_loop<T>(
        &mut self,
        identifier: DefaultSymbol,
        start: T,
        end: T,
        statements: &Vec<StmtRef>,
        create_object: fn(T) -> Object,
    ) -> Result<EvaluationResult, InterpreterError>
    where
        T: Copy + std::cmp::PartialOrd + std::ops::Add<Output = T> + From<u8>,
    {
        let mut current = start;
        let one = T::from(1);

        while current < end {
            self.environment.enter_block();
            self.environment.set_var(
                identifier,
                Rc::new(RefCell::new(create_object(current))),
                VariableSetType::Insert,
                self.string_interner,
            )?;

            let res_block = self.evaluate_block(&statements);
            self.environment.exit_block();

            match res_block {
                Ok(EvaluationResult::Value(_)) => (),
                Ok(EvaluationResult::Return(v)) => return Ok(EvaluationResult::Return(v)),
                Ok(EvaluationResult::Break) => break,
                Ok(EvaluationResult::Continue) => {
                    current = current + one;
                    continue;
                }
                Ok(EvaluationResult::None) => (),
                Err(e) => return Err(e),
            }

            current = current + one;
        }

        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::null_unknown()))))
    }

    pub fn evaluate_block(&mut self, statements: &[StmtRef] ) -> Result<EvaluationResult, InterpreterError> {
        let to_stmt = |s: &StmtRef| -> Result<Stmt, InterpreterError> {
            self.stmt_pool.get(&s)
                .ok_or_else(|| InterpreterError::InternalError("Invalid statement reference".to_string()))
        };
        let statements = statements.iter()
            .map(to_stmt)
            .collect::<Result<Vec<_>, _>>()?;
        let mut last: Option<EvaluationResult> = None;

        for stmt in statements {
            match stmt {
                Stmt::Val(name, _, e) => {
                    last = self.handle_val_declaration(name, &e)?;
                }
                Stmt::Var(name, _, e) => {
                    last = self.handle_var_declaration(name, &e)?;
                }
                Stmt::Return(e) => {
                    return self.handle_return_statement(&e);
                }
                Stmt::Break => {
                    return Ok(EvaluationResult::Break);
                }
                Stmt::Continue => {
                    return Ok(EvaluationResult::Continue);
                }
                Stmt::StructDecl { .. } => {
                    // Struct declarations are handled at compile time
                    last = None;
                }
                Stmt::ImplBlock { .. } => {
                    // Impl blocks are handled at compile time
                    last = None;
                }
                Stmt::While(cond, body) => {
                    last = Some(self.handle_while_loop(&cond, &body)?);
                }
                Stmt::For(identifier, start, end, block) => {
                    let result = self.handle_for_loop(identifier, &start, &end, &block)?;
                    match result {
                        EvaluationResult::Return(v) => return Ok(EvaluationResult::Return(v)),
                        EvaluationResult::Break => return Ok(EvaluationResult::Break),
                        EvaluationResult::Continue => return Ok(EvaluationResult::Continue),
                        _ => last = Some(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit)))),
                    }
                }
                Stmt::Expression(expr) => {
                    let result = self.handle_expression_statement(&expr)?;
                    match result {
                        EvaluationResult::Return(v) => return Ok(EvaluationResult::Return(v)),
                        EvaluationResult::Break => return Ok(EvaluationResult::Break),
                        EvaluationResult::Continue => return Ok(EvaluationResult::Continue),
                        other => last = Some(other),
                    }
                }
            }
        }

        if last.is_some() {
            last.ok_or_else(|| InterpreterError::InternalError("Empty block evaluation".to_string()))
        } else {
            Ok(EvaluationResult::None)
        }
    }

    /// Handles val (immutable variable) declarations
    fn handle_val_declaration(&mut self, name: DefaultSymbol, expr: &ExprRef) -> Result<Option<EvaluationResult>, InterpreterError> {
        let value = self.evaluate(expr);
        let value = self.extract_value(value)?;
        self.environment.set_val(name, value);
        Ok(None)
    }

    /// Handles var (mutable variable) declarations
    fn handle_var_declaration(&mut self, name: DefaultSymbol, expr: &Option<ExprRef>) -> Result<Option<EvaluationResult>, InterpreterError> {
        let value = if expr.is_none() {
            self.null_object.clone()
        } else {
            match self.evaluate(expr.as_ref().ok_or_else(|| InterpreterError::InternalError("Missing expression in value".to_string()))?)? {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(v) => v.unwrap_or_else(|| self.null_object.clone()),
                _ => self.null_object.clone(),
            }
        };
        self.environment.set_var(name, value, VariableSetType::Insert, self.string_interner)?;
        Ok(None)
    }

    /// Handles return statements
    fn handle_return_statement(&mut self, expr: &Option<ExprRef>) -> Result<EvaluationResult, InterpreterError> {
        if expr.is_none() {
            return Ok(EvaluationResult::Return(None));
        }
        match self.evaluate(expr.as_ref().ok_or_else(|| InterpreterError::InternalError("Missing expression in return".to_string()))?)? {
            EvaluationResult::Value(v) => Ok(EvaluationResult::Return(Some(v))),
            EvaluationResult::Return(v) => Ok(EvaluationResult::Return(v)),
            EvaluationResult::Break => Err(InterpreterError::InternalError("break cannot be used in here".to_string())),
            EvaluationResult::Continue => Err(InterpreterError::InternalError("continue cannot be used in here".to_string())),
            EvaluationResult::None => Err(InterpreterError::InternalError("unexpected None".to_string())),
        }
    }

    /// Handles while loop execution
    fn handle_while_loop(&mut self, cond: &ExprRef, body: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        loop {
            let cond_result = self.evaluate(cond)?;
            let cond_value = self.extract_value(Ok(cond_result))?;
            let cond_bool = cond_value.borrow().try_unwrap_bool().map_err(InterpreterError::ObjectError)?;

            if !cond_bool {
                break;
            }

            let body_expr = self.expr_pool.get(&body)
                .ok_or_else(|| InterpreterError::InternalError("Invalid body expression reference".to_string()))?;
            if let Expr::Block(statements) = body_expr {
                self.environment.enter_block();
                let res = self.evaluate_block(&statements);
                self.environment.exit_block();

                match res {
                    Ok(EvaluationResult::Value(_)) => (),
                    Ok(EvaluationResult::Return(v)) => return Ok(EvaluationResult::Return(v)),
                    Ok(EvaluationResult::Break) => break,
                    Ok(EvaluationResult::Continue) => continue,
                    Ok(EvaluationResult::None) => (),
                    Err(e) => return Err(e),
                }
            } else {
                return Err(InterpreterError::InternalError("While body is not a block".to_string()));
            }
        }
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))))
    }

    /// Handles for loop execution
    fn handle_for_loop(&mut self, identifier: DefaultSymbol, start: &ExprRef, end: &ExprRef, block: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let start = self.evaluate(start);
        let start = self.extract_value(start)?;
        let end = self.evaluate(end);
        let end = self.extract_value(end)?;
        let start_ty = start.borrow().get_type();
        let end_ty = end.borrow().get_type();

        if start_ty != end_ty {
            return Err(InterpreterError::TypeError {
                expected: start_ty,
                found: end_ty,
                message: "evaluate_block: Bad types for 'for' loop due to different type".to_string()
            });
        }

        let block = self.expr_pool.get(&block)
            .ok_or_else(|| InterpreterError::InternalError("Invalid block expression reference".to_string()))?;
        if let Expr::Block(statements) = block {
            match start_ty {
                TypeDecl::UInt64 => {
                    let start_val = start.borrow().try_unwrap_uint64().map_err(InterpreterError::ObjectError)?;
                    let end_val = end.borrow().try_unwrap_uint64().map_err(InterpreterError::ObjectError)?;
                    self.execute_for_loop(identifier, start_val, end_val, &statements, Object::UInt64)
                }
                TypeDecl::Int64 => {
                    let start_val = start.borrow().try_unwrap_int64().map_err(InterpreterError::ObjectError)?;
                    let end_val = end.borrow().try_unwrap_int64().map_err(InterpreterError::ObjectError)?;
                    self.execute_for_loop(identifier, start_val, end_val, &statements, Object::Int64)
                }
                _ => {
                    Err(InterpreterError::TypeError {
                        expected: TypeDecl::UInt64,
                        found: start_ty,
                        message: "For loop range must be UInt64 or Int64".to_string()
                    })
                }
            }
        } else {
            Err(InterpreterError::InternalError("For loop body is not a block".to_string()))
        }
    }

    /// Handles expression statements
    fn handle_expression_statement(&mut self, expr: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let e = self.expr_pool.get(&expr)
            .ok_or_else(|| InterpreterError::InternalError("Invalid expression reference".to_string()))?;
        match e {
            Expr::Assign(lhs, rhs) => {
                self.handle_assignment(&lhs, &rhs)
            }
            Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) => {
                let obj = convert_object(&e)?;
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(obj))))
            }
            Expr::Identifier(s) => {
                self.handle_identifier_expression(s)
            }
            Expr::Block(blk_expr) => {
                self.handle_nested_block(&blk_expr)
            }
            _ => {
                // Take care to handle loop control flow correctly when break/continue is executed
                // in nested loops. These statements affect only their immediate enclosing loop.
                self.evaluate(expr)
            }
        }
    }

    /// Handles assignment expressions (both variable and array element assignment)
    fn handle_assignment(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        if let Some(lhs_expr) = self.expr_pool.get(&lhs) {
            match lhs_expr {
                Expr::Identifier(name) => self.handle_variable_assignment(name, rhs),
                _ => {
                    Err(InterpreterError::InternalError("bad assignment due to lhs is not identifier or array access".to_string()))
                }
            }
        } else {
            Err(InterpreterError::InternalError("bad assignment due to invalid lhs reference".to_string()))
        }
    }

    /// Handles variable assignment
    fn handle_variable_assignment(&mut self, name: DefaultSymbol, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Handle null expressions specially in variable assignments
        let expr = self.expr_pool.get(&rhs)
            .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {:?}", rhs)))?;

        let rhs = match expr {
            Expr::Null => {
                // Use pre-created null object for variable assignments
                self.null_object.clone()
            }
            _ => {
                let rhs = self.evaluate(rhs);
                self.extract_value(rhs)?
            }
        };
        let rhs_borrow = rhs.borrow();

        // type check
        let existing_val = self.environment.get_val(name);
        if existing_val.is_none() {
            return Err(InterpreterError::UndefinedVariable("bad assignment due to variable was not set".to_string()));
        }
        let existing_val = existing_val.unwrap();
        let val = existing_val.borrow();
        let val_ty = val.get_type();
        let rhs_ty = rhs_borrow.get_type();

        if val_ty != rhs_ty {
            // Allow null assignment to any type
            if matches!(rhs_ty, TypeDecl::Unknown) {
                // Allow null assignment
            } else {
                return Err(InterpreterError::TypeError {
                    expected: val_ty,
                    found: rhs_ty,
                    message: "Bad types for assignment due to different type".to_string()
                });
            }
        }

        self.environment.set_var(name, rhs.clone(), VariableSetType::Overwrite, self.string_interner)?;
        let cloned_value = rhs.borrow().clone();
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(cloned_value))))
    }


    /// Handles identifier expressions
    fn handle_identifier_expression(&mut self, symbol: DefaultSymbol) -> Result<EvaluationResult, InterpreterError> {
        let obj = self.environment.get_val(symbol);
        let obj_ref = obj.clone();
        if obj.is_none() || obj.unwrap().borrow().is_null() {
            let s = self.string_interner.resolve(symbol).unwrap_or("<NOT_FOUND>");
            return Err(InterpreterError::UndefinedVariable(format!("Identifier {s} is null")));
        }
        Ok(EvaluationResult::Value(obj_ref.unwrap()))
    }

    /// Handles nested block expressions
    fn handle_nested_block(&mut self, statements: &[StmtRef]) -> Result<EvaluationResult, InterpreterError> {
        self.environment.enter_block();
        let result = self.evaluate_block(statements)?;
        self.environment.exit_block();
        Ok(result)
    }
}
