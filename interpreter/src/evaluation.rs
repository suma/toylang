use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::environment::{Environment, VariableSetType};
use crate::object::{Object, RcObject};
use crate::error::InterpreterError;

#[derive(Debug)]
enum ArithmeticOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl ArithmeticOp {
    fn name(&self) -> &str {
        match self {
            ArithmeticOp::Add => "evaluate_add",
            ArithmeticOp::Sub => "evaluate_sub",
            ArithmeticOp::Mul => "evaluate_mul",
            ArithmeticOp::Div => "evaluate_div",
        }
    }

    fn symbol(&self) -> &str {
        match self {
            ArithmeticOp::Add => "+",
            ArithmeticOp::Sub => "-", 
            ArithmeticOp::Mul => "*",
            ArithmeticOp::Div => "/",
        }
    }

    fn apply_i64(&self, l: i64, r: i64) -> i64 {
        match self {
            ArithmeticOp::Add => l + r,
            ArithmeticOp::Sub => l - r,
            ArithmeticOp::Mul => l * r,
            ArithmeticOp::Div => l / r,
        }
    }

    fn apply_u64(&self, l: u64, r: u64) -> u64 {
        match self {
            ArithmeticOp::Add => l + r,
            ArithmeticOp::Sub => l - r,
            ArithmeticOp::Mul => l * r,
            ArithmeticOp::Div => l / r,
        }
    }
}

#[derive(Debug)]
pub enum EvaluationResult {
    None,
    Value(Rc<RefCell<Object>>),
    Return(Option<Rc<RefCell<Object>>>),
    Break,  // We assume break and continue are used with a label
    Continue,
}

pub struct EvaluationContext<'a> {
    stmt_pool: &'a StmtPool,
    expr_pool: &'a ExprPool,
    pub string_interner: &'a mut DefaultStringInterner,
    function: HashMap<DefaultSymbol, Rc<Function>>,
    pub environment: Environment,
}

impl<'a> EvaluationContext<'a> {
    fn execute_for_loop<T>(
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

            let res_block = self.evaluate_block(statements);
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
        
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Null))))
    }
    pub fn new(stmt_pool: &'a StmtPool, expr_pool: &'a ExprPool, string_interner: &'a mut DefaultStringInterner, function: HashMap<DefaultSymbol, Rc<Function>>) -> Self {
        Self {
            stmt_pool,
            expr_pool,
            string_interner,
            function,
            environment: Environment::new(),
        }
    }

    fn extract_value(&mut self, result: Result<EvaluationResult, InterpreterError>) -> Result<Rc<RefCell<Object>>, InterpreterError> {
        match result {
            Ok(EvaluationResult::Value(v)) => Ok(v),
            Ok(EvaluationResult::Return(v)) => Err(InterpreterError::PropagateFlow(EvaluationResult::Return(v))),
            Ok(EvaluationResult::Break) => Err(InterpreterError::PropagateFlow(EvaluationResult::Break)),
            Ok(EvaluationResult::Continue) => Err(InterpreterError::PropagateFlow(EvaluationResult::Continue)),
            Ok(EvaluationResult::None) => Err(InterpreterError::InternalError("unexpected None".to_string())),
            Err(e) => Err(e),
        }
    }

    pub fn evaluate_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let lhs = self.evaluate(lhs);
        let rhs = self.evaluate(rhs);
        let lhs_val = self.extract_value(lhs)?;
        let rhs_val = self.extract_value(rhs)?;

        let lhs_obj = lhs_val.borrow();
        let rhs_obj = rhs_val.borrow();

        let result = match op {
            Operator::IAdd => self.evaluate_add(&lhs_obj, &rhs_obj)?,
            Operator::ISub => self.evaluate_sub(&lhs_obj, &rhs_obj)?,
            Operator::IMul => self.evaluate_mul(&lhs_obj, &rhs_obj)?,
            Operator::IDiv => self.evaluate_div(&lhs_obj, &rhs_obj)?,
            Operator::EQ => self.evaluate_eq(&lhs_obj, &rhs_obj)?,
            Operator::NE => self.evaluate_ne(&lhs_obj, &rhs_obj)?,
            Operator::LT => self.evaluate_lt(&lhs_obj, &rhs_obj)?,
            Operator::LE => self.evaluate_le(&lhs_obj, &rhs_obj)?,
            Operator::GT => self.evaluate_gt(&lhs_obj, &rhs_obj)?,
            Operator::GE => self.evaluate_ge(&lhs_obj, &rhs_obj)?,
            Operator::LogicalAnd => self.evaluate_logical_and(&lhs_obj, &rhs_obj)?,
            Operator::LogicalOr => self.evaluate_logical_or(&lhs_obj, &rhs_obj)?,
        };

        Ok(EvaluationResult::Value(Rc::new(RefCell::new(result))))
    }

    fn evaluate_arithmetic_op(&self, lhs: &Object, rhs: &Object, op: ArithmeticOp) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Int64(op.apply_i64(*l, *r)),
            (Object::UInt64(l), Object::UInt64(r)) => Object::UInt64(op.apply_u64(*l, *r)),
            _ => return Err(InterpreterError::TypeError{
                expected: lhs_ty, 
                found: rhs_ty, 
                message: format!("{}: Bad types for binary '{}' operation due to different type: {:?}", 
                               op.name(), op.symbol(), lhs)
            }),
        })
    }

    pub fn evaluate_add(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        self.evaluate_arithmetic_op(lhs, rhs, ArithmeticOp::Add)
    }

    pub fn evaluate_sub(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        self.evaluate_arithmetic_op(lhs, rhs, ArithmeticOp::Sub)
    }

    pub fn evaluate_mul(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        self.evaluate_arithmetic_op(lhs, rhs, ArithmeticOp::Mul)
    }

    pub fn evaluate_div(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        self.evaluate_arithmetic_op(lhs, rhs, ArithmeticOp::Div)
    }

    pub fn evaluate_eq(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(l == r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l == r),
            (Object::String(l), Object::String(r)) => Object::Bool(l == r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_eq: Bad types for binary '==' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_ne(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(l != r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l != r),
            (Object::String(l), Object::String(r)) => Object::Bool(l != r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_ne: Bad types for binary '!=' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_ge(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(l >= r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l >= r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_ge: Bad types for binary '>=' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_gt(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(l > r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l > r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_gt: Bad types for binary '>' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_le(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(l <= r),
            (Object::UInt64(l), Object::UInt64(r)) => {
                Object::Bool(l <= r)
            },
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_le: Bad types for binary '<=' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_lt(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(l < r),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l < r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_lt: Bad types for binary '<' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_logical_and(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Bool(l), Object::Bool(r)) => Object::Bool(*l && *r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_logical_and: Bad types for binary '&&' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate_logical_or(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Bool(l), Object::Bool(r)) => Object::Bool(*l || *r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_logical_or: Bad types for binary '||' operation due to different type: {:?}", lhs)}),
        })
    }

    pub fn evaluate(&mut self, e: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let expr = self.expr_pool.get(e.to_index())
            .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {}", e.to_index())))?;
        match expr {
            Expr::Binary(op, lhs, rhs) => {
                self.evaluate_binary(op, lhs, rhs)
            }
            Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) | Expr::True | Expr::False => {
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(convert_object(expr)))))
            }
            Expr::Identifier(s) => {
                Ok(EvaluationResult::Value(self.environment.get_val(*s).unwrap()))
            }
            Expr::IfElse(cond, then, _else) => {
                let cond = self.evaluate(cond);
                let cond = self.extract_value(cond)?;
                let cond = cond.borrow();
                if cond.get_type() != TypeDecl::Bool {
                    return Err(InterpreterError::TypeError{expected: TypeDecl::Bool, found: cond.get_type(), message: format!("evaluate: Bad types for if-else due to different type: {:?}", expr)});
                }
                assert!(self.expr_pool.get(then.to_index()).unwrap().is_block(), "evaluate: then is not block");
                assert!(self.expr_pool.get(_else.to_index()).unwrap().is_block(), "evaluate: else is not block");

                let block_expr = if cond.try_unwrap_bool().map_err(InterpreterError::ObjectError)? {
                    then
                } else {
                    _else
                };

                self.environment.enter_block();
                let res =  {
                    if let Some(Expr::Block(statements)) = self.expr_pool.get(block_expr.to_index()) { self.evaluate_block(statements) }
                    else { return Err(InterpreterError::InternalError(format!("evaluate: then-else Expr is not block: {:?}", expr))) }
                };
                self.environment.exit_block();
                res
            }

            Expr::Call(name, args) => {
                if let Some(func) = self.function.get::<DefaultSymbol>(name) {
                    // TODO: check arguments type
                    let args = self.expr_pool.get(args.to_index()).unwrap();
                    match args {
                        Expr::ExprList(args) => {
                            if args.len() != func.parameter.len() {
                                return Err(
                                    InterpreterError::FunctionParameterMismatch {
                                        message: format!("evaluate_function: bad function parameter length: {:?}", args.len()),
                                        expected: func.parameter.len(),
                                        found: args.len()
                                    }
                                );
                            }

                            Ok(EvaluationResult::Value(self.evaluate_function(func.clone(), args)?))
                        }
                        _ => Err(InterpreterError::InternalError(format!("evaluate_function: expected ExprList but: {:?}", expr))),
                    }
                } else {
                    let name = self.string_interner.resolve(*name).unwrap_or("<NOT_FOUND>");
                    Err(InterpreterError::FunctionNotFound(name.to_string()))
                }
            }

            _ => Err(InterpreterError::InternalError(format!("evaluate: unexpected expr: {:?}", expr))),
        }
    }

    pub fn evaluate_block(&mut self, statements: &Vec<StmtRef> ) -> Result<EvaluationResult, InterpreterError> {
        let to_stmt = |s: &StmtRef| { self.stmt_pool.get(s.to_index()).unwrap() };
        let statements = statements.iter().map(|s| to_stmt(s)).collect::<Vec<_>>();
        let mut last: Option<EvaluationResult> = None;
        for stmt in statements {
            match stmt {
                Stmt::Val(name, _, e) => {
                    let value = self.evaluate(&e);
                    let value = self.extract_value(value)?;
                    self.environment.set_val(*name, value);
                    last = None;
                }
                Stmt::Var(name, _, e) => {
                    let value = if e.is_none() {
                        Rc::new(RefCell::new(Object::Null))
                    } else {
                        match self.evaluate(&e.unwrap())? {
                            EvaluationResult::Value(v) => v,
                            EvaluationResult::Return(v) => v.unwrap(),
                            _ => Rc::new(RefCell::new(Object::Null)),
                        }
                    };
                    self.environment.set_var(*name, value, VariableSetType::Insert, self.string_interner)?;
                    last = None;
                }
                Stmt::Return(e) => {
                    if e.is_none() {
                        return Ok(EvaluationResult::Return(None));
                    }
                    return match self.evaluate(&e.unwrap())? {
                        EvaluationResult::Value(v) => Ok(EvaluationResult::Return(Some(v))),
                        EvaluationResult::Return(v) => Ok(EvaluationResult::Return(v)),
                        EvaluationResult::Break => Err(InterpreterError::InternalError("break cannot be used in here".to_string())),
                        EvaluationResult::Continue => Err(InterpreterError::InternalError("continue cannot be used in here".to_string())),
                        EvaluationResult::None => Err(InterpreterError::InternalError("unexpected None".to_string())),
                    };
                }
                Stmt::Break => {
                    return Ok(EvaluationResult::Break);
                }
                Stmt::Continue => {
                    return Ok(EvaluationResult::Continue);
                }
                Stmt::While(_cond, _body) => {
                    todo!("while");
                }
                Stmt::For(identifier, start, end, block) => {
                    let start = self.evaluate(&start);
                    let start = self.extract_value(start)?;
                    let end = self.evaluate(&end);
                    let end = self.extract_value(end)?;
                    let start_ty = start.borrow().get_type();
                    let end_ty = end.borrow().get_type();
                    if start_ty != end_ty {
                        return Err(InterpreterError::TypeError { expected: start_ty, found: end_ty, message: "evaluate_block: Bad types for 'for' loop due to different type".to_string()});
                    }
                    let block = self.expr_pool.get(block.to_index()).unwrap();
                    if let Expr::Block(statements) = block {
                        let result = match start_ty {
                            TypeDecl::UInt64 => {
                                let start_val = start.borrow().try_unwrap_uint64().map_err(InterpreterError::ObjectError)?;
                                let end_val = end.borrow().try_unwrap_uint64().map_err(InterpreterError::ObjectError)?;
                                self.execute_for_loop(*identifier, start_val, end_val, statements, Object::UInt64)?
                            }
                            TypeDecl::Int64 => {
                                let start_val = start.borrow().try_unwrap_int64().map_err(InterpreterError::ObjectError)?;
                                let end_val = end.borrow().try_unwrap_int64().map_err(InterpreterError::ObjectError)?;
                                self.execute_for_loop(*identifier, start_val, end_val, statements, Object::Int64)?
                            }
                            _ => {
                                return Err(InterpreterError::TypeError {
                                    expected: TypeDecl::UInt64,
                                    found: start_ty,
                                    message: "For loop range must be UInt64 or Int64".to_string()
                                });
                            }
                        };
                        
                        match result {
                            EvaluationResult::Return(v) => return Ok(EvaluationResult::Return(v)),
                            EvaluationResult::Break => return Ok(EvaluationResult::Break),
                            EvaluationResult::Continue => return Ok(EvaluationResult::Continue),
                            _ => (),
                        }
                    }
                    last = Some(EvaluationResult::Value(Rc::new(RefCell::new(Object::Null))));
                }
                Stmt::Expression(expr) => {
                    let e = self.expr_pool.get(expr.to_index()).unwrap();
                    match e {
                        Expr::Assign(lhs, rhs) => {
                            if let Some(Expr::Identifier(name)) = self.expr_pool.get(lhs.to_index()) {
                                // Currently, lhs assumes Identifier only
                                let rhs = self.evaluate(&rhs);
                                let rhs = self.extract_value(rhs)?;
                                let rhs_borrow = rhs.borrow();

                                // type check
                                let existing_val = self.environment.get_val(*name);
                                if existing_val.is_none() {
                                    return Err(InterpreterError::UndefinedVariable(format!("evaluate_block: bad assignment due to variable was not set: {:?}", name)));
                                }
                                let existing_val = existing_val.unwrap();
                                let val = existing_val.borrow();
                                let val_ty = val.get_type();
                                let rhs_ty = rhs_borrow.get_type();
                                if val_ty != rhs_ty {
                                    return Err(InterpreterError::TypeError { expected: val_ty, found: rhs_ty, message: "evaluate_block: Bad types for assignment due to different type".to_string()});
                                } else {
                                    self.environment.set_var(*name, rhs.clone(), VariableSetType::Overwrite, self.string_interner)?;
                                    last = Some(EvaluationResult::Value(Rc::new(RefCell::new(rhs.borrow().clone()))));
                                }
                            } else {
                                return Err(InterpreterError::InternalError(format!("evaluate_block: bad assignment due to lhs is not identifier: {:?}", expr)));
                            }
                        }
                        Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) => {
                            last = Some(EvaluationResult::Value(Rc::new(RefCell::new(convert_object(e)))));
                        }
                        Expr::Identifier(s) => {
                            let obj = self.environment.get_val(*s);
                            let obj_ref = obj.clone();
                            if obj.is_none() || obj.unwrap().borrow().is_null() {
                                let s = self.string_interner.resolve(*s).unwrap_or("<NOT_FOUND>");
                                return Err(InterpreterError::UndefinedVariable(format!("evaluate_block: Identifier {} is null", s)));
                            }
                            last = Some(EvaluationResult::Value(obj_ref.unwrap()));
                        }
                        Expr::Block(blk_expr) => {
                            self.environment.enter_block();
                            let result = self.evaluate_block(&blk_expr)?;
                            self.environment.exit_block();
                            match result {
                                EvaluationResult::Value(v) => last = Some(EvaluationResult::Value(v)),
                                EvaluationResult::Return(v) => return Ok(EvaluationResult::Return(v)),
                                EvaluationResult::Break => return Ok(EvaluationResult::Break),
                                EvaluationResult::Continue => return Ok(EvaluationResult::Continue),
                                EvaluationResult::None => last = None,
                            };
                        }
                        _ => {
                            // Take care to handle loop control flow correctly when break/continue is executed
                            // in nested loops. These statements affect only their immediate enclosing loop.
                            match self.evaluate(&expr) {
                                Ok(EvaluationResult::Value(v)) =>
                                    last = Some(EvaluationResult::Value(v)),
                                Ok(EvaluationResult::Return(v)) =>
                                    return Ok(EvaluationResult::Return(v)),
                                Ok(EvaluationResult::Break) =>
                                    return Ok(EvaluationResult::Break),
                                Ok(EvaluationResult::Continue) =>
                                    return Ok(EvaluationResult::Continue),
                                Ok(EvaluationResult::None) =>
                                    last = None,
                                Err(e) => return Err(e),
                            };
                        }
                    }
                }
            }
        }
        if last.is_some() {
            Ok(last.unwrap())
        } else {
            Ok(EvaluationResult::None)
        }
    }

    pub fn evaluate_function(&mut self, function: Rc<Function>, args: &Vec<ExprRef>) -> Result<RcObject, InterpreterError> {
        let block = match self.stmt_pool.get(function.code.to_index()) {
            Some(Stmt::Expression(e)) => {
                match self.expr_pool.get(e.to_index()) {
                    Some(Expr::Block(statements)) => statements,
                    _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function: Not handled yet {:?}", function.code))),
                }
            }
            _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function: Not handled yet {:?}", function.code))),
        };

        self.environment.enter_block();
        for i in 0..args.len() {
            let name = function.parameter.get(i).unwrap().0;
            let value = match self.evaluate(&args[i]) {
                Ok(EvaluationResult::Value(v)) => v,
                Ok(EvaluationResult::Return(v)) => {
                    self.environment.exit_block();
                    return Ok(v.unwrap());
                },
                Ok(EvaluationResult::Break) | Ok(EvaluationResult::Continue) => {
                    self.environment.exit_block();
                    return Ok(Rc::new(RefCell::new(Object::Unit)));
                },
                Ok(EvaluationResult::None) => Rc::new(RefCell::new(Object::Null)),
                Err(e) => {
                    self.environment.exit_block();
                    return Err(e);
                },
            };
            self.environment.set_val(name, value);
        }

        let res = self.evaluate_block(block)?;
        self.environment.exit_block();

        if function.return_type.is_none() || function.return_type.as_ref().unwrap() == &TypeDecl::Unit {
            Ok(Rc::new(RefCell::new(Object::Unit)))
        } else {
            Ok(match res {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(None) => Rc::new(RefCell::new(Object::Null)),
                EvaluationResult::Return(v) => v.unwrap(),
                EvaluationResult::Break | EvaluationResult::Continue | EvaluationResult::None => Rc::new(RefCell::new(Object::Unit)),
            })
        }
    }
}

pub fn convert_object(e: &Expr) -> Object {
    match e {
        Expr::True => Object::Bool(true),
        Expr::False => Object::Bool(false),
        Expr::Int64(v) => Object::Int64(*v),
        Expr::UInt64(v) => Object::UInt64(*v),
        Expr::String(v) => Object::String(*v),
        _ => panic!("Not handled yet {:?}", e),
    }
}
