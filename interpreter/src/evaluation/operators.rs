use std::cell::RefCell;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;
use crate::object::Object;
use crate::error::InterpreterError;
use super::{EvaluationContext, EvaluationResult};

#[derive(Debug)]
pub(super) enum ArithmeticOp {
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
pub(super) enum ComparisonOp {
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

impl EvaluationContext<'_> {
    fn evaluate_comparison_op(&self, lhs: &Object, rhs: &Object, op: ComparisonOp) -> Result<Object, InterpreterError> {
        let lhs_ty = lhs.get_type();
        let rhs_ty = rhs.get_type();

        Ok(match (lhs, rhs) {
            (Object::Int64(l), Object::Int64(r)) => Object::Bool(op.apply_i64(*l, *r)),
            (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(op.apply_u64(*l, *r)),
            (Object::Allocator(l), Object::Allocator(r)) => {
                let same = Rc::ptr_eq(l, r);
                match op {
                    ComparisonOp::Eq => Object::Bool(same),
                    ComparisonOp::Ne => Object::Bool(!same),
                    _ => return Err(InterpreterError::TypeError{
                        expected: lhs_ty,
                        found: rhs_ty,
                        message: format!("{}: Allocator comparison only supports == and !=", op.name()),
                    }),
                }
            }
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
}
