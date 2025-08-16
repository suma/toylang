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
enum ComparisonOp {
    Eq,  // ==
    Ne,  // !=
    Lt,  // <
    Le,  // <=
    Gt,  // >
    Ge,  // >=
}

impl ComparisonOp {
    fn name(&self) -> &str {
        match self {
            ComparisonOp::Eq => "evaluate_eq",
            ComparisonOp::Ne => "evaluate_ne",
            ComparisonOp::Lt => "evaluate_lt",
            ComparisonOp::Le => "evaluate_le",
            ComparisonOp::Gt => "evaluate_gt",
            ComparisonOp::Ge => "evaluate_ge",
        }
    }

    fn symbol(&self) -> &str {
        match self {
            ComparisonOp::Eq => "==",
            ComparisonOp::Ne => "!=",
            ComparisonOp::Lt => "<",
            ComparisonOp::Le => "<=",
            ComparisonOp::Gt => ">",
            ComparisonOp::Ge => ">=",
        }
    }

    fn apply_i64(&self, l: i64, r: i64) -> bool {
        match self {
            ComparisonOp::Eq => l == r,
            ComparisonOp::Ne => l != r,
            ComparisonOp::Lt => l < r,
            ComparisonOp::Le => l <= r,
            ComparisonOp::Gt => l > r,
            ComparisonOp::Ge => l >= r,
        }
    }

    fn apply_u64(&self, l: u64, r: u64) -> bool {
        match self {
            ComparisonOp::Eq => l == r,
            ComparisonOp::Ne => l != r,
            ComparisonOp::Lt => l < r,
            ComparisonOp::Le => l <= r,
            ComparisonOp::Gt => l > r,
            ComparisonOp::Ge => l >= r,
        }
    }

    fn apply_string(&self, l: DefaultSymbol, r: DefaultSymbol) -> bool {
        match self {
            ComparisonOp::Eq => l == r,
            ComparisonOp::Ne => l != r,
            // String comparison for <, <=, >, >= not implemented
            _ => false,
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
    method_registry: HashMap<DefaultSymbol, HashMap<DefaultSymbol, Rc<MethodFunction>>>, // struct_name -> method_name -> method
    null_object: RcObject, // Pre-created null object for reuse
    recursion_depth: u32,
    max_recursion_depth: u32,
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

    fn evaluate_comparison_op(&self, lhs: &Object, rhs: &Object, op: ComparisonOp) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(op.apply_i64(*l, *r)),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(op.apply_u64(*l, *r)),
            (Object::String(l), Object::String(r)) => {
                match op {
                    ComparisonOp::Eq | ComparisonOp::Ne => Object::Bool(op.apply_string(*l, *r)),
                    _ => return Err(InterpreterError::TypeError{
                        expected: lhs_ty, 
                        found: rhs_ty, 
                        message: format!("{}: String comparison only supports == and !=: {:?}", 
                                       op.name(), lhs)
                    }),
                }
            }
            _ => return Err(InterpreterError::TypeError{
                expected: lhs_ty, 
                found: rhs_ty, 
                message: format!("{}: Bad types for binary '{}' operation due to different type: {:?}", 
                               op.name(), op.symbol(), lhs)
            }),
        })
    }

    pub fn new(stmt_pool: &'a StmtPool, expr_pool: &'a ExprPool, string_interner: &'a mut DefaultStringInterner, function: HashMap<DefaultSymbol, Rc<Function>>) -> Self {
        Self {
            stmt_pool,
            expr_pool,
            string_interner,
            function,
            environment: Environment::new(),
            method_registry: HashMap::new(),
            null_object: Rc::new(RefCell::new(Object::Null)),
            recursion_depth: 0,
            max_recursion_depth: 1000, // Increased to support deeper recursion like fib(20)
        }
    }

    pub fn register_method(&mut self, struct_name: DefaultSymbol, method_name: DefaultSymbol, method: Rc<MethodFunction>) {
        self.method_registry
            .entry(struct_name)
            .or_default()
            .insert(method_name, method);
    }

    pub fn get_method(&self, struct_name: DefaultSymbol, method_name: DefaultSymbol) -> Option<Rc<MethodFunction>> {
        self.method_registry
            .get(&struct_name)?
            .get(&method_name)
            .cloned()
    }

    fn call_method(&mut self, method: Rc<MethodFunction>, self_obj: RcObject, args: Vec<RcObject>) -> Result<EvaluationResult, InterpreterError> {
        // Create new scope for method execution
        self.environment.enter_block();
        
        // Set up method parameters
        let mut param_index = 0;
        
        // If method has &self parameter, bind it
        if method.has_self_param {
            // For now, we'll use a placeholder symbol for self
            let self_symbol = self.string_interner.get_or_intern("self");
            self.environment.set_val(self_symbol, self_obj);
        }
        
        // Bind regular parameters
        for (param_symbol, _param_type) in &method.parameter {
            if param_index < args.len() {
                self.environment.set_val(*param_symbol, args[param_index].clone());
                param_index += 1;
            }
        }
        
        // Execute method body
        let result = self.evaluate_method(&method);
        
        // Clean up scope
        self.environment.exit_block();
        
        result
    }

    fn evaluate_method(&mut self, method: &MethodFunction) -> Result<EvaluationResult, InterpreterError> {
        // Get the method body from the statement pool
        let stmt = self.stmt_pool.get(method.code.to_index())
            .ok_or_else(|| InterpreterError::InternalError("Invalid method code reference".to_string()))?;
        
        // Execute the method body 
        match stmt {
            frontend::ast::Stmt::Expression(expr_ref) => {
                if let Some(Expr::Block(statements)) = self.expr_pool.get(expr_ref.to_index()) {
                    self.evaluate_block(statements)
                } else {
                    // Single expression method body
                    self.evaluate(expr_ref)
                }
            }
            _ => Err(InterpreterError::InternalError(format!("evaluate_method: unexpected method body type: {stmt:?}")))
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
        // Short-circuit evaluation for logical operators
        match op {
            Operator::LogicalAnd => return self.evaluate_logical_and_short_circuit(lhs, rhs),
            Operator::LogicalOr => return self.evaluate_logical_or_short_circuit(lhs, rhs),
            _ => {}
        }

        // Regular evaluation for all other operators
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
            Operator::LogicalAnd | Operator::LogicalOr => unreachable!("Should be handled above"),
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
        self.evaluate_comparison_op(lhs, rhs, ComparisonOp::Eq)
    }

    pub fn evaluate_ne(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        self.evaluate_comparison_op(lhs, rhs, ComparisonOp::Ne)
    }

    pub fn evaluate_ge(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        self.evaluate_comparison_op(lhs, rhs, ComparisonOp::Ge)
    }

    pub fn evaluate_gt(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        self.evaluate_comparison_op(lhs, rhs, ComparisonOp::Gt)
    }

    pub fn evaluate_le(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        self.evaluate_comparison_op(lhs, rhs, ComparisonOp::Le)
    }

    pub fn evaluate_lt(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        self.evaluate_comparison_op(lhs, rhs, ComparisonOp::Lt)
    }

    pub fn evaluate_logical_and(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Bool(l), Object::Bool(r)) => Object::Bool(*l && *r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_logical_and: Bad types for binary '&&' operation due to different type: {lhs:?}")}),
        })
    }

    pub fn evaluate_logical_or(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Bool(l), Object::Bool(r)) => Object::Bool(*l || *r),
            _ => return Err(InterpreterError::TypeError{expected: lhs_ty, found: rhs_ty, message: format!("evaluate_logical_or: Bad types for binary '||' operation due to different type: {lhs:?}")}),
        })
    }

    // Short-circuit evaluation for logical AND
    pub fn evaluate_logical_and_short_circuit(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let lhs_result = self.evaluate(lhs);
        let lhs_val = self.extract_value(lhs_result)?;
        let lhs_obj = lhs_val.borrow();
        
        let lhs_bool = lhs_obj.try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
        
        // Short-circuit: if left is false, return false without evaluating right
        if !lhs_bool {
            return Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(false)))));
        }
        
        // Left is true, so evaluate right side
        let rhs_result = self.evaluate(rhs);
        let rhs_val = self.extract_value(rhs_result)?;
        let rhs_obj = rhs_val.borrow();
        
        let rhs_bool = rhs_obj.try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
        
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(rhs_bool)))))
    }

    // Short-circuit evaluation for logical OR
    pub fn evaluate_logical_or_short_circuit(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let lhs_result = self.evaluate(lhs);
        let lhs_val = self.extract_value(lhs_result)?;
        let lhs_obj = lhs_val.borrow();
        
        let lhs_bool = lhs_obj.try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
        
        // Short-circuit: if left is true, return true without evaluating right
        if lhs_bool {
            return Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(true)))));
        }
        
        // Left is false, so evaluate right side
        let rhs_result = self.evaluate(rhs);
        let rhs_val = self.extract_value(rhs_result)?;
        let rhs_obj = rhs_val.borrow();
        
        let rhs_bool = rhs_obj.try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
        
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(rhs_bool)))))
    }

    pub fn evaluate(&mut self, e: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Check recursion depth to prevent stack overflow
        if self.recursion_depth >= self.max_recursion_depth {
            return Err(InterpreterError::InternalError(
                "Maximum recursion depth reached in expression evaluation - possible circular reference".to_string()
            ));
        }
        
        self.recursion_depth += 1;
        let result = self.evaluate_impl(e);
        self.recursion_depth -= 1;
        result
    }
    
    fn evaluate_impl(&mut self, e: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let expr = self.expr_pool.get(e.to_index())
            .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {}", e.to_index())))?;
        match expr {
            Expr::Binary(op, lhs, rhs) => {
                self.evaluate_binary(op, lhs, rhs)
            }
            Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) | Expr::True | Expr::False => {
                self.evaluate_literal(expr)
            }
            Expr::Number(_v) => {
                // Type-unspecified numbers should be resolved during type checking
                Err(InterpreterError::InternalError("Expr::Number should be transformed to concrete type during type checking".to_string()))
            }
            Expr::Identifier(s) => {
                let val = self.environment.get_val(*s)
                    .ok_or_else(|| InterpreterError::UndefinedVariable(format!("Variable not found: {s:?}")))?;
                Ok(EvaluationResult::Value(val))
            }
            Expr::IfElifElse(cond, then, elif_pairs, _else) => {
                self.evaluate_if_elif_else(cond, then, elif_pairs, _else)
            }
            Expr::Call(name, args) => {
                self.evaluate_function_call(name, args)
            }
            Expr::ArrayLiteral(elements) => {
                self.evaluate_array_literal(elements)
            }
            Expr::ArrayAccess(array, index) => {
                self.evaluate_array_access(array, index)
            }
            Expr::FieldAccess(obj, field) => {
                self.evaluate_field_access(obj, field)
            }
            Expr::MethodCall(obj, method, args) => {
                self.evaluate_method_call(obj, method, args)
            }
            Expr::StructLiteral(struct_name, fields) => {
                self.evaluate_struct_literal(struct_name, fields)
            }
            Expr::Null => {
                Err(InterpreterError::InternalError("Null reference error".to_string()))
            }
            _ => Err(InterpreterError::InternalError(format!("evaluate: unexpected expr: {expr:?}"))),
        }
    }

    /// Evaluates literal values (Int64, UInt64, String, True, False)
    fn evaluate_literal(&self, expr: &Expr) -> Result<EvaluationResult, InterpreterError> {
        let obj = convert_object(expr)?;
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(obj))))
    }

    /// Evaluates if-elif-else control structure
    fn evaluate_if_elif_else(&mut self, cond: &ExprRef, then: &ExprRef, elif_pairs: &[(ExprRef, ExprRef)], _else: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Evaluate if condition
        let cond = self.evaluate(cond);
        let cond = self.extract_value(cond)?;
        let cond = cond.borrow();
        if cond.get_type() != TypeDecl::Bool {
            return Err(InterpreterError::TypeError{expected: TypeDecl::Bool, found: cond.get_type(), message: "evaluate: Bad types for if condition".to_string()});
        }

        let mut selected_block = None;

        // Check if condition
        if cond.try_unwrap_bool().map_err(InterpreterError::ObjectError)? {
            let then_expr = self.expr_pool.get(then.to_index())
                .ok_or_else(|| InterpreterError::InternalError("Invalid then block reference".to_string()))?;
            if !then_expr.is_block() {
                return Err(InterpreterError::InternalError("if-then is not block".to_string()));
            }
            selected_block = Some(then);
        } else {
            // Check elif conditions
            for (elif_cond, elif_block) in elif_pairs {
                let elif_cond = self.evaluate(elif_cond);
                let elif_cond = self.extract_value(elif_cond)?;
                let elif_cond = elif_cond.borrow();
                if elif_cond.get_type() != TypeDecl::Bool {
                    return Err(InterpreterError::TypeError{expected: TypeDecl::Bool, found: elif_cond.get_type(), message: "evaluate: Bad types for elif condition".to_string()});
                }

                if elif_cond.try_unwrap_bool().map_err(InterpreterError::ObjectError)? {
                    let elif_expr = self.expr_pool.get(elif_block.to_index())
                        .ok_or_else(|| InterpreterError::InternalError("Invalid elif block reference".to_string()))?;
                    if !elif_expr.is_block() {
                        return Err(InterpreterError::InternalError("elif block is not block".to_string()));
                    }
                    selected_block = Some(elif_block);
                    break;
                }
            }

            // If no elif condition matched, use else block
            if selected_block.is_none() {
                let else_expr = self.expr_pool.get(_else.to_index())
                    .ok_or_else(|| InterpreterError::InternalError("Invalid else block reference".to_string()))?;
                if !else_expr.is_block() {
                    return Err(InterpreterError::InternalError("else block is not block".to_string()));
                }
                selected_block = Some(_else);
            }
        }

        // Execute selected block
        if let Some(block_expr) = selected_block {
            self.environment.enter_block();
            let res = {
                if let Some(Expr::Block(statements)) = self.expr_pool.get(block_expr.to_index()) {
                    self.evaluate_block(statements)
                } else {
                    return Err(InterpreterError::InternalError("evaluate: selected block is not block".to_string()))
                }
            };
            self.environment.exit_block();
            res
        } else {
            Err(InterpreterError::InternalError("evaluate: no block selected in if-elif-else".to_string()))
        }
    }

    /// Evaluates function calls
    fn evaluate_function_call(&mut self, name: &DefaultSymbol, args: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        if let Some(func) = self.function.get::<DefaultSymbol>(name).cloned() {
            let args = self.expr_pool.get(args.to_index())
                .ok_or_else(|| InterpreterError::InternalError("Invalid arguments reference".to_string()))?;
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

                    // Evaluate arguments once and perform type checking
                    let mut evaluated_args = Vec::new();
                    for (i, (arg_expr, (_param_name, expected_type))) in args.iter().zip(func.parameter.iter()).enumerate() {
                        let arg_result = self.evaluate(arg_expr)?;
                        let arg_value = self.extract_value(Ok(arg_result))?;
                        let actual_type = arg_value.borrow().get_type();
                        
                        if actual_type != *expected_type {
                            let func_name = self.string_interner.resolve(*name).unwrap_or("<unknown>");
                            return Err(InterpreterError::TypeError {
                                expected: expected_type.clone(),
                                found: actual_type,
                                message: format!("Function '{}' argument {} type mismatch", func_name, i + 1)
                            });
                        }
                        
                        evaluated_args.push(arg_value);
                    }

                    // Call function with pre-evaluated arguments
                    Ok(EvaluationResult::Value(self.evaluate_function_with_values(func, &evaluated_args)?))
                }
                _ => Err(InterpreterError::InternalError("evaluate_function: expected ExprList".to_string())),
            }
        } else {
            let name = self.string_interner.resolve(*name).unwrap_or("<NOT_FOUND>");
            Err(InterpreterError::FunctionNotFound(name.to_string()))
        }
    }

    /// Evaluates array literal expressions
    fn evaluate_array_literal(&mut self, elements: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        let mut array_objects = Vec::new();
        for element in elements {
            let value = self.evaluate(element)?;
            let obj = self.extract_value(Ok(value))?;
            array_objects.push(obj);
        }
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Array(Box::new(array_objects))))))
    }

    /// Evaluates array access expressions
    fn evaluate_array_access(&mut self, array: &ExprRef, index: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let array_value = self.evaluate(array)?;
        let array_obj = self.extract_value(Ok(array_value))?;
        let index_value = self.evaluate(index)?;
        let index_obj = self.extract_value(Ok(index_value))?;
        
        let array_borrowed = array_obj.borrow();
        let index_borrowed = index_obj.borrow();
        
        let array_vec = array_borrowed.try_unwrap_array()
            .map_err(InterpreterError::ObjectError)?;
            
        let index_val = match &*index_borrowed {
            Object::UInt64(i) => *i as usize,
            Object::Int64(i) => {
                if *i < 0 {
                    return Err(InterpreterError::IndexOutOfBounds { index: *i as isize, size: array_vec.len() });
                }
                *i as usize
            }
            _ => return Err(InterpreterError::TypeError {
                expected: TypeDecl::UInt64,
                found: index_borrowed.get_type(),
                message: "Array index must be an integer".to_string()
            })
        };
        
        if index_val >= array_vec.len() {
            return Err(InterpreterError::IndexOutOfBounds { index: index_val as isize, size: array_vec.len() });
        }
        
        Ok(EvaluationResult::Value(array_vec[index_val].clone()))
    }

    /// Evaluates field access expressions
    fn evaluate_field_access(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<EvaluationResult, InterpreterError> {
        // First check if this is a module qualified name (e.g., math.add)
        if let Some(Expr::Identifier(module_name)) = self.expr_pool.get(obj.to_index()) {
            if let Some(module_value) = self.resolve_module_qualified_name(*module_name, *field) {
                return Ok(EvaluationResult::Value(module_value));
            }
        }
        
        // If not a module qualified name, evaluate as struct field access
        let obj_val = self.evaluate(obj)?;
        let obj_val = self.extract_value(Ok(obj_val))?;
        let obj_borrowed = obj_val.borrow();
        
        match &*obj_borrowed {
            Object::Struct { fields, .. } => {
                let field_name = self.string_interner.resolve(*field)
                    .ok_or_else(|| InterpreterError::InternalError("Field name not found in string interner".to_string()))?;
                
                fields.get(field_name)
                    .cloned()
                    .map(EvaluationResult::Value)
                    .ok_or_else(|| InterpreterError::InternalError(format!("Field '{field_name}' not found")))
            }
            _ => Err(InterpreterError::InternalError(format!("Cannot access field on non-struct object: {obj_borrowed:?}")))
        }
    }

    /// Evaluates method call expressions
    fn evaluate_method_call(&mut self, obj: &ExprRef, method: &DefaultSymbol, args: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        let obj_val = self.evaluate(obj)?;
        let obj_val = self.extract_value(Ok(obj_val))?;
        let obj_borrowed = obj_val.borrow();
        let method_name = self.string_interner.resolve(*method).unwrap_or("<unknown>");
        
        // Handle universal is_null() method first
        if method_name == "is_null" {
            if !args.is_empty() {
                return Err(InterpreterError::InternalError(format!(
                    "is_null() method takes no arguments, but {} provided",
                    args.len()
                )));
            }
            let is_null = obj_borrowed.is_null();
            return Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(is_null)))));
        }

        match &*obj_borrowed {
            Object::String(string_symbol) => {
                // Handle built-in String methods
                match method_name {
                    "len" => {
                        // String.len() method - no arguments required, returns u64
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "String.len() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }
                        
                        // Get the actual string from the interner and calculate its length
                        let string_value = self.string_interner.resolve(*string_symbol)
                            .ok_or_else(|| InterpreterError::InternalError("String value not found in interner".to_string()))?;
                        let len = string_value.len() as u64;
                        
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::UInt64(len)))))
                    }
                    _ => {
                        Err(InterpreterError::InternalError(format!(
                            "Method '{method_name}' not found for String type"
                        )))
                    }
                }
            }
            Object::Struct { type_name, .. } => {
                let struct_name_symbol = *type_name;
                
                if let Some(method_func) = self.get_method(struct_name_symbol, *method) {
                    drop(obj_borrowed); // Release borrow before method call
                    
                    // Evaluate method arguments
                    let mut arg_values = Vec::new();
                    for arg in args {
                        let arg_val = self.evaluate(arg)?;
                        let arg_val = self.extract_value(Ok(arg_val))?;
                        arg_values.push(arg_val);
                    }
                    
                    // Call method with self as first argument
                    self.call_method(method_func, obj_val, arg_values)
                } else {
                    Err(InterpreterError::InternalError(format!("Method '{method_name}' not found for struct '{type_name:?}'")))
                }
            }
            _ => {
                Err(InterpreterError::InternalError(format!("Cannot call method '{method_name}' on non-struct object: {obj_borrowed:?}")))
            }
        }
    }

    /// Evaluates struct literal expressions
    fn evaluate_struct_literal(&mut self, struct_name: &DefaultSymbol, fields: &[(DefaultSymbol, ExprRef)]) -> Result<EvaluationResult, InterpreterError> {
        // Create a struct instance
        let mut field_values = HashMap::new();
        
        for (field_name, field_expr) in fields {
            // Handle null expressions specially in struct literals
            let expr = self.expr_pool.get(field_expr.to_index())
                .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {}", field_expr.to_index())))?;
            
            let field_value = match expr {
                Expr::Null => {
                    // Use pre-created null object for struct fields
                    self.null_object.clone()
                }
                _ => {
                    let field_value = self.evaluate(field_expr)?;
                    self.extract_value(Ok(field_value))?
                }
            };
            
            let field_name_str = self.string_interner.resolve(*field_name).unwrap_or("unknown").to_string();
            field_values.insert(field_name_str, field_value);
        }
        
        let struct_obj = Object::Struct {
            type_name: *struct_name,
            fields: Box::new(field_values),
        };
        
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(struct_obj))))
    }

    pub fn evaluate_block(&mut self, statements: &[StmtRef] ) -> Result<EvaluationResult, InterpreterError> {
        let to_stmt = |s: &StmtRef| -> Result<&Stmt, InterpreterError> {
            self.stmt_pool.get(s.to_index())
                .ok_or_else(|| InterpreterError::InternalError("Invalid statement reference".to_string()))
        };
        let statements = statements.iter()
            .map(to_stmt)
            .collect::<Result<Vec<_>, _>>()?;
        let mut last: Option<EvaluationResult> = None;
        
        for stmt in statements {
            match stmt {
                Stmt::Val(name, _, e) => {
                    last = self.handle_val_declaration(*name, e)?;
                }
                Stmt::Var(name, _, e) => {
                    last = self.handle_var_declaration(*name, e)?;
                }
                Stmt::Return(e) => {
                    return self.handle_return_statement(e);
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
                    last = Some(self.handle_while_loop(cond, body)?);
                }
                Stmt::For(identifier, start, end, block) => {
                    let result = self.handle_for_loop(*identifier, start, end, block)?;
                    match result {
                        EvaluationResult::Return(v) => return Ok(EvaluationResult::Return(v)),
                        EvaluationResult::Break => return Ok(EvaluationResult::Break),
                        EvaluationResult::Continue => return Ok(EvaluationResult::Continue),
                        _ => last = Some(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit)))),
                    }
                }
                Stmt::Expression(expr) => {
                    let result = self.handle_expression_statement(expr)?;
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
            
            let body_expr = self.expr_pool.get(body.to_index())
                .ok_or_else(|| InterpreterError::InternalError("Invalid body expression reference".to_string()))?;
            if let Expr::Block(statements) = body_expr {
                self.environment.enter_block();
                let res = self.evaluate_block(statements);
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
        
        let block = self.expr_pool.get(block.to_index())
            .ok_or_else(|| InterpreterError::InternalError("Invalid block expression reference".to_string()))?;
        if let Expr::Block(statements) = block {
            match start_ty {
                TypeDecl::UInt64 => {
                    let start_val = start.borrow().try_unwrap_uint64().map_err(InterpreterError::ObjectError)?;
                    let end_val = end.borrow().try_unwrap_uint64().map_err(InterpreterError::ObjectError)?;
                    self.execute_for_loop(identifier, start_val, end_val, statements, Object::UInt64)
                }
                TypeDecl::Int64 => {
                    let start_val = start.borrow().try_unwrap_int64().map_err(InterpreterError::ObjectError)?;
                    let end_val = end.borrow().try_unwrap_int64().map_err(InterpreterError::ObjectError)?;
                    self.execute_for_loop(identifier, start_val, end_val, statements, Object::Int64)
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
        let e = self.expr_pool.get(expr.to_index())
            .ok_or_else(|| InterpreterError::InternalError("Invalid expression reference".to_string()))?;
        match e {
            Expr::Assign(lhs, rhs) => {
                self.handle_assignment(lhs, rhs)
            }
            Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) => {
                let obj = convert_object(e)?;
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(obj))))
            }
            Expr::Identifier(s) => {
                self.handle_identifier_expression(*s)
            }
            Expr::Block(blk_expr) => {
                self.handle_nested_block(blk_expr)
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
        if let Some(Expr::Identifier(name)) = self.expr_pool.get(lhs.to_index()) {
            self.handle_variable_assignment(*name, rhs)
        } else if let Some(Expr::ArrayAccess(array, index)) = self.expr_pool.get(lhs.to_index()) {
            self.handle_array_element_assignment(array, index, rhs)
        } else {
            Err(InterpreterError::InternalError("bad assignment due to lhs is not identifier or array access".to_string()))
        }
    }

    /// Handles variable assignment
    fn handle_variable_assignment(&mut self, name: DefaultSymbol, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Handle null expressions specially in variable assignments
        let expr = self.expr_pool.get(rhs.to_index())
            .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {}", rhs.to_index())))?;
        
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
            if rhs_ty == TypeDecl::Null {
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

    /// Handles array element assignment
    fn handle_array_element_assignment(&mut self, array: &ExprRef, index: &ExprRef, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let array_value = self.evaluate(array)?;
        let array_obj = self.extract_value(Ok(array_value))?;
        let index_value = self.evaluate(index)?;
        let index_obj = self.extract_value(Ok(index_value))?;
        
        // Handle null expressions specially in array element assignments
        let expr = self.expr_pool.get(rhs.to_index())
            .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {}", rhs.to_index())))?;
        
        let rhs_obj = match expr {
            Expr::Null => {
                // Use pre-created null object for array element assignments
                self.null_object.clone()
            }
            _ => {
                let rhs_value = self.evaluate(rhs)?;
                self.extract_value(Ok(rhs_value))?
            }
        };
        
        let index_borrowed = index_obj.borrow();
        let index_val = match &*index_borrowed {
            Object::UInt64(i) => *i as usize,
            Object::Int64(i) => {
                if *i < 0 {
                    return Err(InterpreterError::IndexOutOfBounds { index: *i as isize, size: 0 });
                }
                *i as usize
            }
            _ => return Err(InterpreterError::TypeError {
                expected: TypeDecl::UInt64,
                found: index_borrowed.get_type(),
                message: "Array index must be an integer".to_string()
            })
        };
        
        let mut array_borrowed = array_obj.borrow_mut();
        let array_vec = array_borrowed.unwrap_array_mut();
        
        if index_val >= array_vec.len() {
            return Err(InterpreterError::IndexOutOfBounds { index: index_val as isize, size: array_vec.len() });
        }
        
        // Type check
        let existing_element_type = array_vec[index_val].borrow().get_type();
        let rhs_type = rhs_obj.borrow().get_type();
        if existing_element_type != rhs_type {
            return Err(InterpreterError::TypeError {
                expected: existing_element_type,
                found: rhs_type,
                message: "Array element assignment type mismatch".to_string()
            });
        }
        
        array_vec[index_val] = rhs_obj.clone();
        Ok(EvaluationResult::Value(rhs_obj))
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

    pub fn evaluate_function(&mut self, function: Rc<Function>, args: &[ExprRef]) -> Result<RcObject, InterpreterError> {
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
        for (i, arg) in args.iter().enumerate() {
            let name = function.parameter.get(i)
                .ok_or_else(|| InterpreterError::InternalError("Invalid parameter index".to_string()))?.0;
            let value = match self.evaluate(arg) {
                Ok(EvaluationResult::Value(v)) => v,
                Ok(EvaluationResult::Return(v)) => {
                    self.environment.exit_block();
                    return Ok(v.unwrap_or_else(|| Rc::new(RefCell::new(Object::Null))));
                },
                Ok(EvaluationResult::Break) | Ok(EvaluationResult::Continue) => {
                    self.environment.exit_block();
                    return Ok(Rc::new(RefCell::new(Object::Unit)));
                },
                Ok(EvaluationResult::None) => Rc::new(RefCell::new(Object::Unit)),
                Err(e) => {
                    self.environment.exit_block();
                    return Err(e);
                },
            };
            self.environment.set_val(name, value);
        }

        let res = self.evaluate_block(block)?;
        self.environment.exit_block();

        if function.return_type.as_ref().is_none_or(|t| *t == TypeDecl::Unit) {
            Ok(Rc::new(RefCell::new(Object::Unit)))
        } else {
            Ok(match res {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(None) => Rc::new(RefCell::new(Object::Unit)),
                EvaluationResult::Return(v) => v.unwrap_or_else(|| Rc::new(RefCell::new(Object::Null))),
                EvaluationResult::Break | EvaluationResult::Continue | EvaluationResult::None => Rc::new(RefCell::new(Object::Unit)),
            })
        }
    }

    /// Evaluates function with pre-evaluated argument values (used when type checking has already been done)
    pub fn evaluate_function_with_values(&mut self, function: Rc<Function>, args: &[RcObject]) -> Result<RcObject, InterpreterError> {
        let block = match self.stmt_pool.get(function.code.to_index()) {
            Some(Stmt::Expression(e)) => {
                match self.expr_pool.get(e.to_index()) {
                    Some(Expr::Block(statements)) => statements,
                    _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function_with_values: Not handled yet {:?}", function.code))),
                }
            }
            _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function_with_values: Not handled yet {:?}", function.code))),
        };

        self.environment.enter_block();
        for (i, value) in args.iter().enumerate() {
            let name = function.parameter.get(i)
                .ok_or_else(|| InterpreterError::InternalError("Invalid parameter index".to_string()))?.0;
            self.environment.set_val(name, value.clone());
        }

        let res = self.evaluate_block(block)?;
        self.environment.exit_block();

        if function.return_type.as_ref().is_none_or(|t| *t == TypeDecl::Unit) {
            Ok(Rc::new(RefCell::new(Object::Unit)))
        } else {
            Ok(match res {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(None) => Rc::new(RefCell::new(Object::Unit)),
                EvaluationResult::Return(v) => v.unwrap_or_else(|| Rc::new(RefCell::new(Object::Null))),
                EvaluationResult::Break | EvaluationResult::Continue | EvaluationResult::None => Rc::new(RefCell::new(Object::Unit)),
            })
        }
    }
}

pub fn convert_object(e: &Expr) -> Result<Object, InterpreterError> {
    match e {
        Expr::True => Ok(Object::Bool(true)),
        Expr::False => Ok(Object::Bool(false)),
        Expr::Int64(v) => Ok(Object::Int64(*v)),
        Expr::UInt64(v) => Ok(Object::UInt64(*v)),
        Expr::String(v) => Ok(Object::String(*v)),
        Expr::Number(_v) => {
            // Type-unspecified numbers should be resolved during type checking
            Err(InterpreterError::InternalError(format!(
                "Expr::Number should be transformed to concrete type during type checking: {e:?}"
            )))
        },
        _ => Err(InterpreterError::InternalError(format!(
            "Expression type not handled in convert_object: {e:?}"
        ))),
    }
}

impl EvaluationContext<'_> {
    /// Resolve module qualified name (e.g., math.add -> module [math], variable add)
    fn resolve_module_qualified_name(&self, module_name: DefaultSymbol, variable_name: DefaultSymbol) -> Option<RcObject> {
        // Convert single module name to module path (could be extended for nested modules)
        let module_path = vec![module_name];
        
        // Look up variable in the specified module
        if let Some(variable_value) = self.environment.resolve_qualified_name(&module_path, variable_name) {
            Some(variable_value.value.clone())
        } else {
            None
        }
    }
}
