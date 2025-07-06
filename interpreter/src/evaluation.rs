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
        }
    }

    pub fn register_method(&mut self, struct_name: DefaultSymbol, method_name: DefaultSymbol, method: Rc<MethodFunction>) {
        self.method_registry
            .entry(struct_name)
            .or_insert_with(HashMap::new)
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
            let self_symbol = self.string_interner.get_or_intern("self".to_string());
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
        let stmt = self.stmt_pool.get(method.code.to_index()).unwrap();
        
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
            _ => Err(InterpreterError::InternalError(format!("evaluate_method: unexpected method body type: {:?}", stmt)))
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
        let expr = self.expr_pool.get(e.to_index())
            .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {}", e.to_index())))?;
        match expr {
            Expr::Binary(op, lhs, rhs) => {
                self.evaluate_binary(op, lhs, rhs)
            }
            Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) | Expr::True | Expr::False => {
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(convert_object(expr)))))
            }
            Expr::Number(_v) => {
                // Type-unspecified numbers should be resolved during type checking
                Err(InterpreterError::InternalError("Expr::Number should be transformed to concrete type during type checking".to_string()))
            }
            Expr::Identifier(s) => {
                Ok(EvaluationResult::Value(self.environment.get_val(*s).unwrap()))
            }

            Expr::IfElifElse(cond, then, elif_pairs, _else) => {
                // Evaluate if condition
                let cond = self.evaluate(cond);
                let cond = self.extract_value(cond)?;
                let cond = cond.borrow();
                if cond.get_type() != TypeDecl::Bool {
                    return Err(InterpreterError::TypeError{expected: TypeDecl::Bool, found: cond.get_type(), message: format!("evaluate: Bad types for if condition: {:?}", expr)});
                }

                let mut selected_block = None;

                // Check if condition
                if cond.try_unwrap_bool().map_err(InterpreterError::ObjectError)? {
                    assert!(self.expr_pool.get(then.to_index()).unwrap().is_block(), "evaluate: if-then is not block");
                    selected_block = Some(then);
                } else {
                    // Check elif conditions
                    for (elif_cond, elif_block) in elif_pairs {
                        let elif_cond = self.evaluate(elif_cond);
                        let elif_cond = self.extract_value(elif_cond)?;
                        let elif_cond = elif_cond.borrow();
                        if elif_cond.get_type() != TypeDecl::Bool {
                            return Err(InterpreterError::TypeError{expected: TypeDecl::Bool, found: elif_cond.get_type(), message: format!("evaluate: Bad types for elif condition: {:?}", expr)});
                        }

                        if elif_cond.try_unwrap_bool().map_err(InterpreterError::ObjectError)? {
                            assert!(self.expr_pool.get(elif_block.to_index()).unwrap().is_block(), "evaluate: elif block is not block");
                            selected_block = Some(elif_block);
                            break;
                        }
                    }

                    // If no elif condition matched, use else block
                    if selected_block.is_none() {
                        assert!(self.expr_pool.get(_else.to_index()).unwrap().is_block(), "evaluate: else block is not block");
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
                            return Err(InterpreterError::InternalError(format!("evaluate: selected block is not block: {:?}", expr)))
                        }
                    };
                    self.environment.exit_block();
                    res
                } else {
                    Err(InterpreterError::InternalError("evaluate: no block selected in if-elif-else".to_string()))
                }
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

            Expr::ArrayLiteral(elements) => {
                let mut array_objects = Vec::new();
                for element in elements {
                    let value = self.evaluate(element)?;
                    let obj = self.extract_value(Ok(value))?;
                    array_objects.push(obj);
                }
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Array(array_objects)))))
            }

            Expr::ArrayAccess(array, index) => {
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

            Expr::FieldAccess(obj, field) => {
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
                            .ok_or_else(|| InterpreterError::InternalError(format!("Field '{}' not found", field_name)))
                    }
                    _ => Err(InterpreterError::InternalError(format!("Cannot access field on non-struct object: {:?}", obj_borrowed)))
                }
            }

            Expr::MethodCall(obj, method, args) => {
                let obj_val = self.evaluate(obj)?;
                let obj_val = self.extract_value(Ok(obj_val))?;
                let obj_borrowed = obj_val.borrow();
                
                match &*obj_borrowed {
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
                            let method_name = self.string_interner.resolve(*method).unwrap_or("<unknown>");
                            Err(InterpreterError::InternalError(format!("Method '{}' not found for struct '{:?}'", method_name, type_name)))
                        }
                    }
                    _ => {
                        let method_name = self.string_interner.resolve(*method).unwrap_or("<unknown>");
                        Err(InterpreterError::InternalError(format!("Cannot call method '{}' on non-struct object: {:?}", method_name, obj_borrowed)))
                    }
                }
            }

            Expr::StructLiteral(struct_name, fields) => {
                // Create a struct instance
                let mut field_values = HashMap::new();
                
                for (field_name, field_expr) in fields {
                    let field_value = self.evaluate(field_expr)?;
                    let field_value = self.extract_value(Ok(field_value))?;
                    let field_name_str = self.string_interner.resolve(*field_name).unwrap_or("unknown").to_string();
                    field_values.insert(field_name_str, field_value);
                }
                
                let struct_obj = Object::Struct {
                    type_name: *struct_name,
                    fields: field_values,
                };
                
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(struct_obj))))
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
                Stmt::StructDecl { .. } => {
                    // Struct declarations are handled at compile time
                    last = None;
                }
                Stmt::ImplBlock { .. } => {
                    // Impl blocks are handled at compile time
                    last = None;
                }
                Stmt::While(cond, body) => {
                    loop {
                        let cond_result = self.evaluate(cond)?;
                        let cond_value = self.extract_value(Ok(cond_result))?;
                        let cond_bool = cond_value.borrow().try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
                        
                        if !cond_bool {
                            break;
                        }
                        
                        let body_expr = self.expr_pool.get(body.to_index()).unwrap();
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
                    last = Some(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))));
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
                    last = Some(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))));
                }
                Stmt::Expression(expr) => {
                    let e = self.expr_pool.get(expr.to_index()).unwrap();
                    match e {
                        Expr::Assign(lhs, rhs) => {
                            if let Some(Expr::Identifier(name)) = self.expr_pool.get(lhs.to_index()) {
                                // Variable assignment
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
                            } else if let Some(Expr::ArrayAccess(array, index)) = self.expr_pool.get(lhs.to_index()) {
                                // Array element assignment: a[0] = value
                                let array_value = self.evaluate(array)?;
                                let array_obj = self.extract_value(Ok(array_value))?;
                                let index_value = self.evaluate(index)?;
                                let index_obj = self.extract_value(Ok(index_value))?;
                                let rhs_value = self.evaluate(rhs)?;
                                let rhs_obj = self.extract_value(Ok(rhs_value))?;
                                
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
                                last = Some(EvaluationResult::Value(rhs_obj));
                            } else {
                                return Err(InterpreterError::InternalError(format!("evaluate_block: bad assignment due to lhs is not identifier or array access: {:?}", expr)));
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

        if function.return_type.is_none() || function.return_type.as_ref().unwrap() == &TypeDecl::Unit {
            Ok(Rc::new(RefCell::new(Object::Unit)))
        } else {
            Ok(match res {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(None) => Rc::new(RefCell::new(Object::Unit)),
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
        Expr::Number(_v) => {
            // Type-unspecified numbers should be resolved during type checking
            panic!("Expr::Number should be transformed to concrete type during type checking: {:?}", e)
        },
        _ => panic!("Not handled yet {:?}", e),
    }
}
