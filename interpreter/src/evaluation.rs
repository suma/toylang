use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::environment::{Environment, VariableSetType};
use crate::object::{Object, ObjectKey, RcObject};
use crate::error::InterpreterError;
use crate::heap::HeapManager;

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
    heap_manager: HeapManager, // Heap memory manager for pointer operations
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

    fn evaluate_comparison_op(&self, lhs: &Object, rhs: &Object, op: ComparisonOp) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(op.apply_i64(*l, *r)),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(op.apply_u64(*l, *r)),
            (Object::ConstString(l), Object::ConstString(r)) => {
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
            (Object::String(l), Object::String(r)) => {
                match op {
                    ComparisonOp::Eq => Object::Bool(l == r),
                    ComparisonOp::Ne => Object::Bool(l != r),
                    _ => return Err(InterpreterError::TypeError{
                        expected: lhs_ty, 
                        found: rhs_ty, 
                        message: format!("{}: String comparison only supports == and !=: {:?}", 
                                       op.name(), lhs)
                    }),
                }
            }
            // Mixed string type comparisons
            (Object::ConstString(l), Object::String(r)) => {
                let l_str = self.string_interner.resolve(*l).unwrap_or("");
                match op {
                    ComparisonOp::Eq => Object::Bool(l_str == r),
                    ComparisonOp::Ne => Object::Bool(l_str != r),
                    _ => return Err(InterpreterError::TypeError{
                        expected: lhs_ty, 
                        found: rhs_ty, 
                        message: format!("{}: String comparison only supports == and !=: {:?}", 
                                       op.name(), lhs)
                    }),
                }
            }
            (Object::String(l), Object::ConstString(r)) => {
                let r_str = self.string_interner.resolve(*r).unwrap_or("");
                match op {
                    ComparisonOp::Eq => Object::Bool(l == r_str),
                    ComparisonOp::Ne => Object::Bool(l != r_str),
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
            null_object: Rc::new(RefCell::new(Object::null_unknown())),
            recursion_depth: 0,
            max_recursion_depth: 1000, // Increased to support deeper recursion like fib(20)
            heap_manager: HeapManager::new(),
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
        
        // Bind method parameters - first parameter should be self
        for (param_symbol, _param_type) in &method.parameter {
            if param_index == 0 {
                // First parameter is 'self' - bind the object
                self.environment.set_val(*param_symbol, self_obj.clone());
            } else if param_index - 1 < args.len() {
                // Subsequent parameters are regular args
                self.environment.set_val(*param_symbol, args[param_index - 1].clone());
            }
            param_index += 1;
        }
        
        // Execute method body
        let result = self.evaluate_method(&method);
        
        // Clean up scope
        self.environment.exit_block();
        
        result
    }

    fn evaluate_method(&mut self, method: &MethodFunction) -> Result<EvaluationResult, InterpreterError> {
        // Get the method body from the statement pool
        let stmt = self.stmt_pool.get(&method.code)
            .ok_or_else(|| InterpreterError::InternalError("Invalid method code reference".to_string()))?;
        
        // Execute the method body 
        match stmt {
            frontend::ast::Stmt::Expression(expr_ref) => {
                if let Some(Expr::Block(statements)) = self.expr_pool.get(&expr_ref) {
                    self.evaluate_block(&statements)
                } else {
                    // Single expression method body
                    self.evaluate(&expr_ref)
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

    pub fn evaluate_unary(&mut self, op: &UnaryOp, operand: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let operand_result = self.evaluate(operand);
        let operand_val = self.extract_value(operand_result)?;
        let operand_obj = operand_val.borrow();

        let result = match op {
            UnaryOp::BitwiseNot => match &*operand_obj {
                Object::UInt64(v) => Object::UInt64(!v),
                Object::Int64(v) => Object::Int64(!v),
                _ => return Err(InterpreterError::TypeError{
                    expected: TypeDecl::UInt64,
                    found: operand_obj.get_type(),
                    message: format!("Bitwise NOT requires integer type, got {:?}", operand_obj)
                }),
            },
            UnaryOp::LogicalNot => match &*operand_obj {
                Object::Bool(v) => Object::Bool(!v),
                _ => return Err(InterpreterError::TypeError{
                    expected: TypeDecl::Bool,
                    found: operand_obj.get_type(),
                    message: format!("Logical NOT requires boolean type, got {:?}", operand_obj)
                }),
            },
        };

        Ok(EvaluationResult::Value(Rc::new(RefCell::new(result))))
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
            Operator::BitwiseAnd => self.evaluate_bitwise_and(&lhs_obj, &rhs_obj)?,
            Operator::BitwiseOr => self.evaluate_bitwise_or(&lhs_obj, &rhs_obj)?,
            Operator::BitwiseXor => self.evaluate_bitwise_xor(&lhs_obj, &rhs_obj)?,
            Operator::LeftShift => self.evaluate_left_shift(&lhs_obj, &rhs_obj)?,
            Operator::RightShift => self.evaluate_right_shift(&lhs_obj, &rhs_obj)?,
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

    // Bitwise operations
    pub fn evaluate_bitwise_and(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        match (lhs, rhs) {
            (Object::UInt64(l), Object::UInt64(r)) => Ok(Object::UInt64(*l & *r)),
            (Object::Int64(l), Object::Int64(r)) => Ok(Object::Int64(*l & *r)),
            _ => Err(InterpreterError::TypeError{
                expected: lhs.get_type(), 
                found: rhs.get_type(), 
                message: format!("Bitwise AND requires same integer types, got {:?} and {:?}", lhs, rhs)
            }),
        }
    }

    pub fn evaluate_bitwise_or(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        match (lhs, rhs) {
            (Object::UInt64(l), Object::UInt64(r)) => Ok(Object::UInt64(*l | *r)),
            (Object::Int64(l), Object::Int64(r)) => Ok(Object::Int64(*l | *r)),
            _ => Err(InterpreterError::TypeError{
                expected: lhs.get_type(), 
                found: rhs.get_type(), 
                message: format!("Bitwise OR requires same integer types, got {:?} and {:?}", lhs, rhs)
            }),
        }
    }

    pub fn evaluate_bitwise_xor(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        match (lhs, rhs) {
            (Object::UInt64(l), Object::UInt64(r)) => Ok(Object::UInt64(*l ^ *r)),
            (Object::Int64(l), Object::Int64(r)) => Ok(Object::Int64(*l ^ *r)),
            _ => Err(InterpreterError::TypeError{
                expected: lhs.get_type(), 
                found: rhs.get_type(), 
                message: format!("Bitwise XOR requires same integer types, got {:?} and {:?}", lhs, rhs)
            }),
        }
    }

    pub fn evaluate_left_shift(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        // For shift operations, right operand should always be UInt64
        let shift_amount = match rhs {
            Object::UInt64(r) => *r,
            _ => return Err(InterpreterError::TypeError{
                expected: TypeDecl::UInt64, 
                found: rhs.get_type(), 
                message: format!("Shift amount must be UInt64, got {:?}", rhs)
            }),
        };

        match lhs {
            Object::UInt64(l) => Ok(Object::UInt64(l.wrapping_shl(shift_amount as u32))),
            Object::Int64(l) => Ok(Object::Int64(l.wrapping_shl(shift_amount as u32))),
            _ => Err(InterpreterError::TypeError{
                expected: TypeDecl::UInt64, 
                found: lhs.get_type(), 
                message: format!("Left shift requires integer type on left side, got {:?}", lhs)
            }),
        }
    }

    pub fn evaluate_right_shift(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        // For shift operations, right operand should always be UInt64
        let shift_amount = match rhs {
            Object::UInt64(r) => *r,
            _ => return Err(InterpreterError::TypeError{
                expected: TypeDecl::UInt64, 
                found: rhs.get_type(), 
                message: format!("Shift amount must be UInt64, got {:?}", rhs)
            }),
        };

        match lhs {
            Object::UInt64(l) => Ok(Object::UInt64(l.wrapping_shr(shift_amount as u32))),
            Object::Int64(l) => Ok(Object::Int64(l.wrapping_shr(shift_amount as u32))),
            _ => Err(InterpreterError::TypeError{
                expected: TypeDecl::UInt64, 
                found: lhs.get_type(), 
                message: format!("Right shift requires integer type on left side, got {:?}", lhs)
            }),
        }
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
        let expr = self.expr_pool.get(e)
            .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {:?}", e)))?;
        match expr {
            Expr::Binary(op, lhs, rhs) => {
                self.evaluate_binary(&op, &lhs, &rhs)
            }
            Expr::Unary(op, operand) => {
                self.evaluate_unary(&op, &operand)
            }
            Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) | Expr::True | Expr::False => {
                self.evaluate_literal(&expr)
            }
            Expr::Number(_v) => {
                // Type-unspecified numbers should be resolved during type checking
                Err(InterpreterError::InternalError("Expr::Number should be transformed to concrete type during type checking".to_string()))
            }
            Expr::Identifier(s) => {
                let val = self.environment.get_val(s)
                    .ok_or_else(|| InterpreterError::UndefinedVariable(format!("Variable not found: {s:?}")))?;
                Ok(EvaluationResult::Value(val))
            }
            Expr::IfElifElse(cond, then, elif_pairs, _else) => {
                self.evaluate_if_elif_else(&cond, &then, &elif_pairs, &_else)
            }
            Expr::Call(name, args) => {
                self.evaluate_function_call(&name, &args)
            }
            Expr::ArrayLiteral(elements) => {
                self.evaluate_array_literal(&elements)
            }
            Expr::FieldAccess(obj, field) => {
                self.evaluate_field_access(&obj, &field)
            }
            Expr::MethodCall(obj, method, args) => {
                self.evaluate_method_call(&obj, &method, &args)
            }
            Expr::BuiltinMethodCall(receiver, method, args) => {
                self.evaluate_builtin_method_call(&receiver, &method, &args)
            }
            Expr::BuiltinCall(func, args) => {
                self.evaluate_builtin_call(&func, &args)
            }
            Expr::StructLiteral(struct_name, fields) => {
                self.evaluate_struct_literal(&struct_name, &fields)
            }
            Expr::QualifiedIdentifier(path) => {
                self.evaluate_qualified_identifier(&path)
            }
            Expr::Null => {
                Err(InterpreterError::InternalError("Null reference error".to_string()))
            }
            Expr::SliceAssign(object, start, end, value) => {
                self.evaluate_slice_assign(&object, &start, &end, &value)
            }
            Expr::SliceAccess(object, slice_info) => {
                self.evaluate_slice_access_with_info(&object, &slice_info)
            }
            Expr::DictLiteral(entries) => {
                self.evaluate_dict_literal(&entries)
            }
            Expr::TupleLiteral(elements) => {
                self.evaluate_tuple_literal(&elements)
            }
            Expr::TupleAccess(tuple, index) => {
                self.evaluate_tuple_access(&tuple, index)
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
            let then_expr = self.expr_pool.get(&then)
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
                    let elif_expr = self.expr_pool.get(&elif_block)
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
                let else_expr = self.expr_pool.get(&_else)
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
                if let Some(Expr::Block(statements)) = self.expr_pool.get(&block_expr) {
                    self.evaluate_block(&statements)
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
            let args = self.expr_pool.get(&args)
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
                        
                        if !actual_type.is_equivalent(expected_type) {
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

    /// Evaluates field access expressions
    fn evaluate_field_access(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<EvaluationResult, InterpreterError> {
        // First check if this is a module qualified name (e.g., math.add)
        if let Some(Expr::Identifier(module_name)) = self.expr_pool.get(&obj) {
            if let Some(module_value) = self.resolve_module_qualified_name(module_name, *field) {
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
            Object::ConstString(_) | Object::String(_) => {
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
                        
                        // Get the actual string value regardless of internal representation
                        let string_value = obj_borrowed.to_string_value(&self.string_interner);
                        let len = string_value.len() as u64;
                        
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::UInt64(len)))))
                    }
                    "contains" => {
                        if args.len() != 1 {
                            return Err(InterpreterError::InternalError(format!(
                                "String.contains() method takes 1 argument, but {} provided",
                                args.len()
                            )));
                        }
                        
                        let string_value = obj_borrowed.to_string_value(&self.string_interner);
                        
                        let arg_value = self.evaluate(&args[0])?;
                        let arg_obj = self.extract_value(Ok(arg_value))?;
                        let arg_borrowed = arg_obj.borrow();
                        let arg_string = arg_borrowed.to_string_value(&self.string_interner);
                        
                        let contains = string_value.contains(&arg_string);
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(contains)))))
                    }
                    "concat" => {
                        if args.len() != 1 {
                            return Err(InterpreterError::InternalError(format!(
                                "String.concat() method takes 1 argument, but {} provided",
                                args.len()
                            )));
                        }
                        
                        let string_value = obj_borrowed.to_string_value(&self.string_interner);
                        
                        let arg_value = self.evaluate(&args[0])?;
                        let arg_obj = self.extract_value(Ok(arg_value))?;
                        let arg_borrowed = arg_obj.borrow();
                        let arg_string = arg_borrowed.to_string_value(&self.string_interner);
                        
                        let concatenated = format!("{}{}", string_value, arg_string);
                        // Return as dynamic String, not interned - this is the key improvement
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(concatenated)))))
                    }
                    "trim" => {
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "String.trim() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }
                        
                        let string_value = obj_borrowed.to_string_value(&self.string_interner);
                        let trimmed = string_value.trim().to_string();
                        // Return as dynamic String, not interned
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(trimmed)))))
                    }
                    "to_upper" => {
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "String.to_upper() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }
                        
                        let string_value = obj_borrowed.to_string_value(&self.string_interner);
                        let upper = string_value.to_uppercase();
                        // Return as dynamic String, not interned
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(upper)))))
                    }
                    "to_lower" => {
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "String.to_lower() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }
                        
                        let string_value = obj_borrowed.to_string_value(&self.string_interner);
                        let lower = string_value.to_lowercase();
                        // Return as dynamic String, not interned
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(lower)))))
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
            let expr = self.expr_pool.get(&field_expr)
                .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {:?}", field_expr)))?;
            
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

    pub fn evaluate_function(&mut self, function: Rc<Function>, args: &[ExprRef]) -> Result<RcObject, InterpreterError> {
        let block = match self.stmt_pool.get(&function.code) {
            Some(Stmt::Expression(e)) => {
                match self.expr_pool.get(&e) {
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
                    return Ok(v.unwrap_or_else(|| Rc::new(RefCell::new(Object::null_unknown()))));
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

        let res = self.evaluate_block(&block)?;
        self.environment.exit_block();

        if function.return_type.as_ref().is_none_or(|t| *t == TypeDecl::Unit) {
            Ok(Rc::new(RefCell::new(Object::Unit)))
        } else {
            Ok(match res {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(None) => Rc::new(RefCell::new(Object::Unit)),
                EvaluationResult::Return(v) => v.unwrap_or_else(|| Rc::new(RefCell::new(Object::null_unknown()))),
                EvaluationResult::Break | EvaluationResult::Continue | EvaluationResult::None => Rc::new(RefCell::new(Object::Unit)),
            })
        }
    }

    /// Evaluates function with pre-evaluated argument values (used when type checking has already been done)
    pub fn evaluate_function_with_values(&mut self, function: Rc<Function>, args: &[RcObject]) -> Result<RcObject, InterpreterError> {
        let block = match self.stmt_pool.get(&function.code) {
            Some(Stmt::Expression(e)) => {
                match self.expr_pool.get(&e) {
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

        let res = self.evaluate_block(&block)?;
        self.environment.exit_block();

        if function.return_type.as_ref().is_none_or(|t| *t == TypeDecl::Unit) {
            Ok(Rc::new(RefCell::new(Object::Unit)))
        } else {
            Ok(match res {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(None) => Rc::new(RefCell::new(Object::Unit)),
                EvaluationResult::Return(v) => v.unwrap_or_else(|| Rc::new(RefCell::new(Object::null_unknown()))),
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
        Expr::String(v) => Ok(Object::ConstString(*v)),
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

    /// Evaluate qualified identifier (e.g., math::add)
    fn evaluate_qualified_identifier(&mut self, path: &Vec<DefaultSymbol>) -> Result<EvaluationResult, InterpreterError> {
        if path.is_empty() {
            return Err(InterpreterError::InternalError("Empty qualified identifier path".to_string()));
        }
        
        // For now, treat qualified identifiers as simple variable lookups using the last component
        // In the future, this can be enhanced for proper module resolution
        if let Some(last_symbol) = path.last() {
            // Try to look up the qualified name in the environment
            if let Some(val) = self.environment.get_val(*last_symbol) {
                Ok(EvaluationResult::Value(val))
            } else {
                Err(InterpreterError::UndefinedVariable(format!("Qualified identifier not found: {:?}", path)))
            }
        } else {
            Err(InterpreterError::InternalError("Empty qualified identifier path".to_string()))
        }
    }

    /// Evaluate builtin method calls
    fn evaluate_builtin_method_call(&mut self, receiver: &ExprRef, method: &BuiltinMethod, args: &Vec<ExprRef>) -> Result<EvaluationResult, InterpreterError> {
        let receiver_value = self.evaluate(receiver)?;
        let receiver_obj = self.extract_value(Ok(receiver_value))?;

        self.execute_builtin_method(&receiver_obj, method, args)
    }
    
    fn evaluate_dict_literal(&mut self, entries: &[(ExprRef, ExprRef)]) -> Result<EvaluationResult, InterpreterError> {
        let mut dict = HashMap::new();
        
        for (key_ref, value_ref) in entries {
            // Evaluate key - now supports any Object type that can be used as a key
            let key_val = self.evaluate(key_ref)?;
            let key_obj_rc = self.extract_value(Ok(key_val))?;
            
            // Convert to ObjectKey - clone the object for use as a key
            let key_object = key_obj_rc.borrow().clone();
            let object_key = ObjectKey::new(key_object);
            
            // Evaluate value
            let value_val = self.evaluate(value_ref)?;
            let value_obj = self.extract_value(Ok(value_val))?;
            
            dict.insert(object_key, value_obj);
        }
        
        let dict_obj = Object::Dict(Box::new(dict));
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(dict_obj))))
    }
    
    fn evaluate_tuple_literal(&mut self, elements: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        let mut tuple_elements = Vec::new();
        
        for element_ref in elements {
            let element_val = self.evaluate(element_ref);
            let element_obj = self.extract_value(element_val)?;
            tuple_elements.push(element_obj);
        }
        
        let tuple_obj = Object::Tuple(Box::new(tuple_elements));
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(tuple_obj))))
    }
    
    fn evaluate_tuple_access(&mut self, tuple: &ExprRef, index: usize) -> Result<EvaluationResult, InterpreterError> {
        let tuple_val = self.evaluate(tuple);
        let tuple_obj = self.extract_value(tuple_val)?;
        
        let tuple_borrowed = tuple_obj.borrow();
        match &*tuple_borrowed {
            Object::Tuple(elements) => {
                if index >= elements.len() {
                    return Err(InterpreterError::IndexOutOfBounds { 
                        index: index as isize, 
                        size: elements.len() 
                    });
                }
                Ok(EvaluationResult::Value(Rc::clone(&elements[index])))
            }
            _ => {
                Err(InterpreterError::InternalError(format!(
                    "Cannot access index {} on non-tuple type",
                    index
                )))
            }
        }
    }
    
    
    /// Convert index (positive or negative) to array index
    fn resolve_array_index(&self, index_obj: &RcObject, array_len: usize) -> Result<usize, InterpreterError> {
        let borrowed = index_obj.borrow();
        match &*borrowed {
            Object::UInt64(idx) => {
                let idx = *idx as usize;
                if idx >= array_len {
                    return Err(InterpreterError::IndexOutOfBounds { 
                        index: idx as isize, 
                        size: array_len 
                    });
                }
                Ok(idx)
            }
            Object::Int64(idx) => {
                if *idx >= 0 {
                    // Positive i64, treat as u64
                    let idx = *idx as usize;
                    if idx >= array_len {
                        return Err(InterpreterError::IndexOutOfBounds { 
                            index: idx as isize, 
                            size: array_len 
                        });
                    }
                    Ok(idx)
                } else {
                    // Negative index: convert to positive
                    let abs_idx = (-*idx) as usize;
                    if abs_idx > array_len {
                        return Err(InterpreterError::IndexOutOfBounds { 
                            index: *idx as isize, 
                            size: array_len 
                        });
                    }
                    Ok(array_len - abs_idx)
                }
            }
            _ => Err(InterpreterError::InternalError("Array index must be an integer".to_string()))
        }
    }

    fn evaluate_slice_access_with_info(&mut self, object: &ExprRef, slice_info: &SliceInfo) -> Result<EvaluationResult, InterpreterError> {
        let object_val = self.evaluate(object)?;
        let object_obj = self.extract_value(Ok(object_val))?;
        
        let obj_borrowed = object_obj.borrow();
        match &*obj_borrowed {
            Object::Array(elements) => {
                let array_len = elements.len();
                
                // Evaluate start index (default to 0)
                let start_idx = if let Some(start_expr) = &slice_info.start {
                    let start_val = self.evaluate(start_expr)?;
                    let start_obj = self.extract_value(Ok(start_val))?;
                    self.resolve_array_index(&start_obj, array_len)?
                } else {
                    0
                };
                
                // Evaluate end index (default to array length)
                let end_idx = if let Some(end_expr) = &slice_info.end {
                    let end_val = self.evaluate(end_expr)?;
                    let end_obj = self.extract_value(Ok(end_val))?;
                    // Use same logic as in original function for end index
                    let borrowed = end_obj.borrow();
                    match &*borrowed {
                        Object::UInt64(idx) => {
                            let idx = *idx as usize;
                            if idx > array_len {
                                return Err(InterpreterError::IndexOutOfBounds { 
                                    index: idx as isize, 
                                    size: array_len 
                                });
                            }
                            idx
                        }
                        Object::Int64(idx) => {
                            if *idx >= 0 {
                                let idx = *idx as usize;
                                if idx > array_len {
                                    return Err(InterpreterError::IndexOutOfBounds { 
                                        index: idx as isize, 
                                        size: array_len 
                                    });
                                }
                                idx
                            } else {
                                // Negative end index: convert to positive
                                let abs_idx = (-*idx) as usize;
                                if abs_idx > array_len {
                                    return Err(InterpreterError::IndexOutOfBounds { 
                                        index: *idx as isize, 
                                        size: array_len 
                                    });
                                }
                                array_len - abs_idx
                            }
                        }
                        _ => return Err(InterpreterError::InternalError("Array index must be an integer".to_string()))
                    }
                } else {
                    array_len
                };
                
                // Validate indices
                if start_idx > array_len {
                    return Err(InterpreterError::IndexOutOfBounds { 
                        index: start_idx as isize, 
                        size: array_len 
                    });
                }
                if end_idx > array_len {
                    return Err(InterpreterError::IndexOutOfBounds { 
                        index: end_idx as isize, 
                        size: array_len 
                    });
                }
                if start_idx > end_idx {
                    return Err(InterpreterError::InternalError(
                        format!("Invalid slice range: start ({}) > end ({})", start_idx, end_idx)
                    ));
                }
                
                // Use SliceInfo to distinguish single element vs range slice
                match slice_info.slice_type {
                    SliceType::SingleElement => {
                        // Single element access: arr[i] returns the element directly
                        if start_idx >= array_len {
                            return Err(InterpreterError::IndexOutOfBounds { 
                                index: start_idx as isize, 
                                size: array_len 
                            });
                        }
                        Ok(EvaluationResult::Value(elements[start_idx].clone()))
                    }
                    SliceType::RangeSlice => {
                        // Range slice: arr[start..end] returns array
                        let slice_elements = elements[start_idx..end_idx].to_vec();
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Array(Box::new(slice_elements))))))
                    }
                }
            }
            Object::Dict(_dict) => {
                // Dictionary access uses the original method
                self.evaluate_slice_access(object, &slice_info.start, &slice_info.end)
            }
            _ => Err(InterpreterError::InternalError("Slice access is only supported on arrays and dictionaries".to_string()))
        }
    }
    
    fn evaluate_slice_access(&mut self, object: &ExprRef, start: &Option<ExprRef>, end: &Option<ExprRef>) -> Result<EvaluationResult, InterpreterError> {
        let object_val = self.evaluate(object)?;
        let object_obj = self.extract_value(Ok(object_val))?;
        
        let obj_borrowed = object_obj.borrow();
        match &*obj_borrowed {
            Object::Array(elements) => {
                let array_len = elements.len();
                
                // Evaluate start index (default to 0)
                let start_idx = if let Some(start_expr) = start {
                    let start_val = self.evaluate(start_expr)?;
                    let start_obj = self.extract_value(Ok(start_val))?;
                    self.resolve_array_index(&start_obj, array_len)?
                } else {
                    0
                };
                
                // Evaluate end index (default to array length)
                let end_idx = if let Some(end_expr) = end {
                    let end_val = self.evaluate(end_expr)?;
                    let end_obj = self.extract_value(Ok(end_val))?;
                    // For end index, we need to allow array_len as valid (exclusive end)
                    let borrowed = end_obj.borrow();
                    match &*borrowed {
                        Object::UInt64(idx) => {
                            let idx = *idx as usize;
                            if idx > array_len {
                                return Err(InterpreterError::IndexOutOfBounds { 
                                    index: idx as isize, 
                                    size: array_len 
                                });
                            }
                            idx
                        }
                        Object::Int64(idx) => {
                            if *idx >= 0 {
                                let idx = *idx as usize;
                                if idx > array_len {
                                    return Err(InterpreterError::IndexOutOfBounds { 
                                        index: idx as isize, 
                                        size: array_len 
                                    });
                                }
                                idx
                            } else {
                                let abs_idx = (-*idx) as usize;
                                if abs_idx > array_len {
                                    return Err(InterpreterError::IndexOutOfBounds { 
                                        index: *idx as isize, 
                                        size: array_len 
                                    });
                                }
                                array_len - abs_idx
                            }
                        }
                        _ => return Err(InterpreterError::InternalError("Array index must be an integer".to_string()))
                    }
                } else {
                    array_len
                };
                
                // Validate indices
                if start_idx > array_len {
                    return Err(InterpreterError::IndexOutOfBounds { 
                        index: start_idx as isize, 
                        size: array_len 
                    });
                }
                if end_idx > array_len {
                    return Err(InterpreterError::IndexOutOfBounds { 
                        index: end_idx as isize, 
                        size: array_len 
                    });
                }
                if start_idx > end_idx {
                    return Err(InterpreterError::InternalError(
                        format!("Invalid slice range: start ({}) > end ({})", start_idx, end_idx)
                    ));
                }
                
                // Check if this is single element access (start provided, end is None)
                if start.is_some() && end.is_none() {
                    // Single element access: arr[i] returns the element directly
                    if start_idx >= array_len {
                        return Err(InterpreterError::IndexOutOfBounds { 
                            index: start_idx as isize, 
                            size: array_len 
                        });
                    }
                    Ok(EvaluationResult::Value(elements[start_idx].clone()))
                } else {
                    // Range slice: arr[start..end] returns array
                    let slice_elements = elements[start_idx..end_idx].to_vec();
                    Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Array(Box::new(slice_elements))))))
                }
            }
            Object::Dict(dict) => {
                // Dictionary access: dict[key] (only single element access)
                if start.is_some() && end.is_none() {
                    // Single element access: dict[key]
                    if let Some(start_expr) = start {
                        let start_val = self.evaluate(start_expr)?;
                        let start_obj = self.extract_value(Ok(start_val))?;
                        
                        // Create ObjectKey for dictionary lookup
                        let key_borrowed = start_obj.borrow();
                        let key_object = key_borrowed.clone();
                        let object_key = ObjectKey::new(key_object);
                        
                        dict.get(&object_key)
                            .cloned()
                            .map(EvaluationResult::Value)
                            .ok_or_else(|| InterpreterError::InternalError(format!("Key not found: {:?}", object_key)))
                    } else {
                        Err(InterpreterError::InternalError("Dictionary access requires key index".to_string()))
                    }
                } else {
                    // Range slicing not supported for dictionaries
                    Err(InterpreterError::InternalError("Dictionary slicing not supported - use single key access dict[key]".to_string()))
                }
            }
            Object::Struct { type_name, .. } => {
                // Struct access: check for __getitem__ method (only single element access)
                if start.is_some() && end.is_none() {
                    // Single element access: struct[key]
                    if let Some(start_expr) = start {
                        let struct_name_val = *type_name;
                        drop(obj_borrowed); // Release borrow before method call
                        
                        let start_val = self.evaluate(start_expr)?;
                        let start_obj = self.extract_value(Ok(start_val))?;
                        
                        // Resolve names first before method call
                        let struct_name_str = self.string_interner.resolve(struct_name_val)
                            .ok_or_else(|| InterpreterError::InternalError("Failed to resolve struct name".to_string()))?
                            .to_string();
                        let getitem_method = self.string_interner.get_or_intern("__getitem__");
                        
                        // Call __getitem__(self, index)
                        let args = vec![start_obj];
                        self.call_struct_method(object_obj, getitem_method, &args, &struct_name_str)
                    } else {
                        Err(InterpreterError::InternalError("Struct access requires index".to_string()))
                    }
                } else {
                    // Range slicing not supported for structs
                    Err(InterpreterError::InternalError("Struct slicing not supported - use single index access struct[key]".to_string()))
                }
            }
            _ => Err(InterpreterError::InternalError(
                format!("Cannot access type: {:?} - only arrays, dictionaries, and structs with __getitem__ are supported", obj_borrowed.get_type())
            ))
        }
    }
    
    fn evaluate_slice_assign(&mut self, object: &ExprRef, start: &Option<ExprRef>, end: &Option<ExprRef>, value: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Get the object being indexed
        let object_val = self.evaluate(object)?;
        let object_obj = self.extract_value(Ok(object_val))?;
        
        // Evaluate the value to assign
        let value_val = self.evaluate(value)?;
        let value_obj = self.extract_value(Ok(value_val))?;
        
        let obj_borrowed = object_obj.borrow();
        match &*obj_borrowed {
            Object::Array(elements) => {
                let array_len = elements.len();
                drop(obj_borrowed);
                
                // Check if this is single element assignment (start provided, end is None)
                if start.is_some() && end.is_none() {
                    // Single element assignment: arr[i] = value
                    if let Some(start_expr) = start {
                        let start_val = self.evaluate(start_expr)?;
                        let start_obj = self.extract_value(Ok(start_val))?;
                        let resolved_idx = self.resolve_array_index(&start_obj, array_len)?;
                        
                        let mut obj_borrowed = object_obj.borrow_mut();
                        if let Object::Array(elements) = &mut *obj_borrowed {
                            elements[resolved_idx] = value_obj.clone();
                            Ok(EvaluationResult::Value(value_obj))
                        } else {
                            Err(InterpreterError::InternalError("Expected array for slice assignment".to_string()))
                        }
                    } else {
                        Err(InterpreterError::InternalError("Single element assignment requires start index".to_string()))
                    }
                } else {
                    // Range slice assignment: arr[start..end] = value (not implemented yet)
                    Err(InterpreterError::InternalError("Range slice assignment not yet implemented".to_string()))
                }
            }
            Object::Dict(_) => {
                drop(obj_borrowed);
                // Dictionary assignment: dict[key] = value (only single element assignment)
                if start.is_some() && end.is_none() {
                    // Single element assignment: dict[key] = value
                    if let Some(start_expr) = start {
                        let start_val = self.evaluate(start_expr)?;
                        let start_obj = self.extract_value(Ok(start_val))?;
                        
                        // Create ObjectKey for dictionary assignment
                        let key_borrowed = start_obj.borrow();
                        let key_object = key_borrowed.clone();
                        let object_key = ObjectKey::new(key_object);
                        
                        let mut obj_borrowed = object_obj.borrow_mut();
                        if let Object::Dict(dict) = &mut *obj_borrowed {
                            dict.insert(object_key, value_obj.clone());
                            Ok(EvaluationResult::Value(value_obj))
                        } else {
                            Err(InterpreterError::InternalError("Expected dict for assignment".to_string()))
                        }
                    } else {
                        Err(InterpreterError::InternalError("Dictionary assignment requires key index".to_string()))
                    }
                } else {
                    // Range slice assignment not supported for dictionaries
                    Err(InterpreterError::InternalError("Dictionary slice assignment not supported - use single key assignment dict[key] = value".to_string()))
                }
            }
            Object::Struct { type_name, .. } => {
                // Struct assignment: check for __setitem__ method (only single element assignment)
                let struct_name_val = *type_name;
                drop(obj_borrowed);
                
                if start.is_some() && end.is_none() {
                    // Single element assignment: struct[key] = value
                    if let Some(start_expr) = start {
                        let start_val = self.evaluate(start_expr)?;
                        let start_obj = self.extract_value(Ok(start_val))?;
                        
                        // Resolve names first before method call  
                        let struct_name_str = self.string_interner.resolve(struct_name_val)
                            .ok_or_else(|| InterpreterError::InternalError("Failed to resolve struct name".to_string()))?
                            .to_string();
                        let setitem_method = self.string_interner.get_or_intern("__setitem__");
                        
                        // Call __setitem__(self, index, value)
                        let args = vec![start_obj, value_obj.clone()];
                        self.call_struct_method(object_obj, setitem_method, &args, &struct_name_str)?;
                        
                        // Return the assigned value
                        Ok(EvaluationResult::Value(value_obj))
                    } else {
                        Err(InterpreterError::InternalError("Struct assignment requires index".to_string()))
                    }
                } else {
                    // Range slice assignment not supported for structs
                    Err(InterpreterError::InternalError("Struct slice assignment not supported - use single index assignment struct[key] = value".to_string()))
                }
            }
            _ => {
                drop(obj_borrowed);
                Err(InterpreterError::InternalError(
                    "Cannot assign to type - only arrays, dictionaries, and structs with __setitem__ are supported".to_string()
                ))
            }
        }
    }

    /// Execute builtin method with table-driven approach
    fn execute_builtin_method(&mut self, receiver: &RcObject, method: &BuiltinMethod, args: &Vec<ExprRef>) -> Result<EvaluationResult, InterpreterError> {
        match method {
            BuiltinMethod::IsNull => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "is_null() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }
                let is_null = receiver.borrow().is_null();
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(is_null)))))
            }
            
            BuiltinMethod::StrLen => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "len() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }
                
                let string_value = receiver.borrow().to_string_value(&self.string_interner);
                let length = string_value.len() as u64;
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::UInt64(length)))))
            }
            
            BuiltinMethod::StrConcat => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "concat(str) takes exactly one string argument".to_string(),
                        expected: 1,
                        found: args.len()
                    });
                }
                
                let string_value = receiver.borrow().to_string_value(&self.string_interner);
                
                let arg_value = self.evaluate(&args[0])?;
                let arg_obj = self.extract_value(Ok(arg_value))?;
                let arg_string = arg_obj.borrow().to_string_value(&self.string_interner);
                
                let concatenated = format!("{}{}", string_value, arg_string);
                // Return as dynamic String, not interned - this is the key improvement
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(concatenated)))))
            }
            
            BuiltinMethod::StrSubstring => {
                if args.len() != 2 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "substring(start, end) takes exactly two u64 arguments".to_string(),
                        expected: 2,
                        found: args.len()
                    });
                }
                
                let string_symbol = receiver.borrow().try_unwrap_string().map_err(InterpreterError::ObjectError)?;
                let string_value = self.string_interner.resolve(string_symbol)
                    .ok_or_else(|| InterpreterError::InternalError("String symbol not found in interner".to_string()))?
                    .to_string();
                
                let start_value = self.evaluate(&args[0])?;
                let start_obj = self.extract_value(Ok(start_value))?;
                let start = start_obj.borrow().try_unwrap_uint64().map_err(InterpreterError::ObjectError)? as usize;
                
                let end_value = self.evaluate(&args[1])?;
                let end_obj = self.extract_value(Ok(end_value))?;
                let end = end_obj.borrow().try_unwrap_uint64().map_err(InterpreterError::ObjectError)? as usize;
                
                if start >= string_value.len() || end > string_value.len() || start > end {
                    return Err(InterpreterError::InternalError("Invalid substring indices".to_string()));
                }
                
                let substring = string_value[start..end].to_string();
                // Return as dynamic String, not interned
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(substring)))))
            }
            
            BuiltinMethod::StrContains => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "contains(str) takes exactly one string argument".to_string(),
                        expected: 1,
                        found: args.len()
                    });
                }
                
                let string_symbol = receiver.borrow().try_unwrap_string().map_err(InterpreterError::ObjectError)?;
                let string_value = self.string_interner.resolve(string_symbol)
                    .ok_or_else(|| InterpreterError::InternalError("String symbol not found in interner".to_string()))?
                    .to_string();
                
                let arg_value = self.evaluate(&args[0])?;
                let arg_obj = self.extract_value(Ok(arg_value))?;
                let arg_symbol = arg_obj.borrow().try_unwrap_string().map_err(InterpreterError::ObjectError)?;
                let arg_string = self.string_interner.resolve(arg_symbol)
                    .ok_or_else(|| InterpreterError::InternalError("Argument string symbol not found in interner".to_string()))?
                    .to_string();
                
                let contains = string_value.contains(&arg_string);
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(contains)))))
            }
            
            BuiltinMethod::StrTrim => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "trim() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }
                
                let string_value = receiver.borrow().to_string_value(&self.string_interner);
                let trimmed = string_value.trim().to_string();
                // Return as dynamic String, not interned
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(trimmed)))))
            }
            
            BuiltinMethod::StrToUpper => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "to_upper() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }
                
                let string_value = receiver.borrow().to_string_value(&self.string_interner);
                let upper = string_value.to_uppercase();
                // Return as dynamic String, not interned
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(upper)))))
            }
            
            BuiltinMethod::StrToLower => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "to_lower() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }
                
                let string_value = receiver.borrow().to_string_value(&self.string_interner);
                let lower = string_value.to_lowercase();
                // Return as dynamic String, not interned
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(lower)))))
            }
            
            BuiltinMethod::StrSplit => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "split(str) takes exactly one string argument".to_string(),
                        expected: 1,
                        found: args.len()
                    });
                }
                
                let string_value = receiver.borrow().to_string_value(&self.string_interner);
                
                let separator_value = self.evaluate(&args[0])?;
                let separator_obj = self.extract_value(Ok(separator_value))?;
                let separator = separator_obj.borrow().to_string_value(&self.string_interner);
                
                let parts: Vec<_> = string_value.split(&separator)
                    .map(|part| {
                        // Return split parts as dynamic Strings, not interned
                        Rc::new(RefCell::new(Object::String(part.to_string())))
                    })
                    .collect();
                
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Array(Box::new(parts))))))
            }
        }
    }

    /// Call a struct method by name
    pub fn call_struct_method(
        &mut self, 
        object: RcObject, 
        method_name: DefaultSymbol, 
        args: &[RcObject], 
        struct_name: &str
    ) -> Result<EvaluationResult, InterpreterError> {
        // Look for the method in the function map first
        if let Some(method_func) = self.function.get(&method_name).cloned() {
            // This is a regular function, call it directly
            let mut method_args = vec![object];
            method_args.extend_from_slice(args);
            let result = self.evaluate_function_with_values(method_func, &method_args)?;
            return Ok(EvaluationResult::Value(result));
        }
        
        // Look for struct method
        let struct_symbol = self.string_interner.get(struct_name)
            .ok_or_else(|| InterpreterError::InternalError(format!("Unknown struct: {}", struct_name)))?;
        
        if let Some(struct_methods) = self.method_registry.get(&struct_symbol) {
            if let Some(method) = struct_methods.get(&method_name) {
                let method_args = args.to_vec();
                return self.call_method(method.clone(), object, method_args);
            }
        }
        
        Err(InterpreterError::FunctionNotFound(
            format!("Method '{}' not found for struct '{}'", 
                    self.string_interner.resolve(method_name).unwrap_or("<unknown>"),
                    struct_name)
        ))
    }

    /// Evaluate builtin function calls
    fn evaluate_builtin_call(&mut self, func: &BuiltinFunction, args: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        match func {
            // Memory management
            BuiltinFunction::HeapAlloc => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "heap_alloc takes 1 argument".to_string(),
                        expected: 1,
                        found: args.len(),
                    });
                }
                
                let size_result = self.evaluate(&args[0])?;
                let size_obj = self.extract_value(Ok(size_result))?;
                let size = size_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("heap_alloc expects u64 size".to_string()))?;
                
                let addr = self.heap_manager.alloc(size as usize);
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Pointer(addr)))))
            }
            
            BuiltinFunction::HeapFree => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "heap_free takes 1 argument".to_string(),
                        expected: 1,
                        found: args.len(),
                    });
                }
                
                let ptr_result = self.evaluate(&args[0])?;
                let ptr_obj = self.extract_value(Ok(ptr_result))?;
                let addr = ptr_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("heap_free expects pointer".to_string()))?;
                
                self.heap_manager.free(addr);
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))))
            }
            
            BuiltinFunction::HeapRealloc => {
                if args.len() != 2 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "heap_realloc takes 2 arguments".to_string(),
                        expected: 2,
                        found: args.len(),
                    });
                }
                
                let ptr_result = self.evaluate(&args[0])?;
                let ptr_obj = self.extract_value(Ok(ptr_result))?;
                let old_addr = ptr_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("heap_realloc expects pointer as first argument".to_string()))?;
                
                let size_result = self.evaluate(&args[1])?;
                let size_obj = self.extract_value(Ok(size_result))?;
                let new_size = size_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("heap_realloc expects u64 size as second argument".to_string()))?;
                
                let new_addr = self.heap_manager.realloc(old_addr, new_size as usize);
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Pointer(new_addr)))))
            }
            
            // Pointer operations
            BuiltinFunction::PtrRead => {
                if args.len() != 2 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "ptr_read takes 2 arguments".to_string(),
                        expected: 2,
                        found: args.len(),
                    });
                }
                
                let ptr_result = self.evaluate(&args[0])?;
                let ptr_obj = self.extract_value(Ok(ptr_result))?;
                let addr = ptr_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("ptr_read expects pointer as first argument".to_string()))?;
                
                let offset_result = self.evaluate(&args[1])?;
                let offset_obj = self.extract_value(Ok(offset_result))?;
                let offset = offset_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("ptr_read expects u64 offset as second argument".to_string()))?;
                
                match self.heap_manager.read_u64(addr, offset as usize) {
                    Some(value) => Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::UInt64(value))))),
                    None => Err(InterpreterError::InternalError("Invalid memory access in ptr_read".to_string())),
                }
            }
            
            BuiltinFunction::PtrWrite => {
                if args.len() != 3 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "ptr_write takes 3 arguments".to_string(),
                        expected: 3,
                        found: args.len(),
                    });
                }
                
                let ptr_result = self.evaluate(&args[0])?;
                let ptr_obj = self.extract_value(Ok(ptr_result))?;
                let addr = ptr_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("ptr_write expects pointer as first argument".to_string()))?;
                
                let offset_result = self.evaluate(&args[1])?;
                let offset_obj = self.extract_value(Ok(offset_result))?;
                let offset = offset_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("ptr_write expects u64 offset as second argument".to_string()))?;
                
                let value_result = self.evaluate(&args[2])?;
                let value_obj = self.extract_value(Ok(value_result))?;
                let value = value_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("ptr_write expects u64 value as third argument".to_string()))?;
                
                if self.heap_manager.write_u64(addr, offset as usize, value) {
                    Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))))
                } else {
                    Err(InterpreterError::InternalError("Invalid memory access in ptr_write".to_string()))
                }
            }
            
            BuiltinFunction::PtrIsNull => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "ptr_is_null takes 1 argument".to_string(),
                        expected: 1,
                        found: args.len(),
                    });
                }
                
                let ptr_result = self.evaluate(&args[0])?;
                let ptr_obj = self.extract_value(Ok(ptr_result))?;
                let addr = ptr_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("ptr_is_null expects pointer".to_string()))?;
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(addr == 0)))))
            }
            
            // Memory operations
            BuiltinFunction::MemCopy => {
                if args.len() != 3 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "mem_copy takes 3 arguments".to_string(),
                        expected: 3,
                        found: args.len(),
                    });
                }
                
                let src_result = self.evaluate(&args[0])?;
                let src_obj = self.extract_value(Ok(src_result))?;
                let src_addr = src_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("mem_copy expects pointer as first argument".to_string()))?;
                
                let dest_result = self.evaluate(&args[1])?;
                let dest_obj = self.extract_value(Ok(dest_result))?;
                let dest_addr = dest_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("mem_copy expects pointer as second argument".to_string()))?;
                
                let size_result = self.evaluate(&args[2])?;
                let size_obj = self.extract_value(Ok(size_result))?;
                let size = size_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("mem_copy expects u64 size as third argument".to_string()))?;
                
                if self.heap_manager.copy_memory(src_addr, dest_addr, size as usize) {
                    Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))))
                } else {
                    Err(InterpreterError::InternalError("Invalid memory access in mem_copy".to_string()))
                }
            }
            
            BuiltinFunction::MemMove => {
                if args.len() != 3 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "mem_move takes 3 arguments".to_string(),
                        expected: 3,
                        found: args.len(),
                    });
                }
                
                let src_result = self.evaluate(&args[0])?;
                let src_obj = self.extract_value(Ok(src_result))?;
                let src_addr = src_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("mem_move expects pointer as first argument".to_string()))?;
                
                let dest_result = self.evaluate(&args[1])?;
                let dest_obj = self.extract_value(Ok(dest_result))?;
                let dest_addr = dest_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("mem_move expects pointer as second argument".to_string()))?;
                
                let size_result = self.evaluate(&args[2])?;
                let size_obj = self.extract_value(Ok(size_result))?;
                let size = size_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("mem_move expects u64 size as third argument".to_string()))?;
                
                if self.heap_manager.move_memory(src_addr, dest_addr, size as usize) {
                    Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))))
                } else {
                    Err(InterpreterError::InternalError("Invalid memory access in mem_move".to_string()))
                }
            }
            
            BuiltinFunction::MemSet => {
                if args.len() != 3 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "mem_set takes 3 arguments".to_string(),
                        expected: 3,
                        found: args.len(),
                    });
                }
                
                let ptr_result = self.evaluate(&args[0])?;
                let ptr_obj = self.extract_value(Ok(ptr_result))?;
                let addr = ptr_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("mem_set expects pointer as first argument".to_string()))?;
                
                let value_result = self.evaluate(&args[1])?;
                let value_obj = self.extract_value(Ok(value_result))?;
                let value = value_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("mem_set expects u64 value as second argument".to_string()))?;
                
                let size_result = self.evaluate(&args[2])?;
                let size_obj = self.extract_value(Ok(size_result))?;
                let size = size_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("mem_set expects u64 size as third argument".to_string()))?;
                
                if self.heap_manager.set_memory(addr, value as u8, size as usize) {
                    Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))))
                } else {
                    Err(InterpreterError::InternalError("Invalid memory access in mem_set".to_string()))
                }
            }
        }
    }
}
