use std::cell::RefCell;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;
use crate::object::Object;
use crate::value::Value;
use crate::error::InterpreterError;
use super::{EvaluationContext, EvaluationResult};

/// Lift an `&Object` into a `Value` without allocating an `Rc`. Used
/// by the legacy `&Object`-shaped operator wrappers; all internal
/// dispatch operates on `Value` directly.
fn object_ref_to_value(obj: &Object) -> Value {
    match obj {
        Object::Bool(b) => Value::Bool(*b),
        Object::Int64(v) => Value::Int64(*v),
        Object::UInt64(v) => Value::UInt64(*v),
        Object::Int8(v) => Value::Int8(*v),
        Object::Int16(v) => Value::Int16(*v),
        Object::Int32(v) => Value::Int32(*v),
        Object::UInt8(v) => Value::UInt8(*v),
        Object::UInt16(v) => Value::UInt16(*v),
        Object::UInt32(v) => Value::UInt32(*v),
        Object::Float64(v) => Value::Float64(*v),
        Object::ConstString(sym) => Value::ConstString(*sym),
        Object::Pointer(addr) => Value::Pointer(*addr),
        Object::Null(td) => Value::Null(td.clone()),
        Object::Unit => Value::Unit,
        // Heap-shaped: we do need a fresh Rc cell here because the
        // caller only has `&Object`. Wrapping a clone keeps the new
        // cell independent of any outer storage. The legacy callers
        // are warm enough that this is acceptable.
        _ => Value::Heap(Rc::new(RefCell::new(obj.clone()))),
    }
}

/// Inverse of `object_ref_to_value` — convert a `Value` back into the
/// owned `Object` form expected by callers that still pass and return
/// `Object` (the public `evaluate_add` / `evaluate_sub` wrappers).
fn value_to_object(v: Value) -> Object {
    match v {
        Value::Bool(b) => Object::Bool(b),
        Value::Int64(v) => Object::Int64(v),
        Value::UInt64(v) => Object::UInt64(v),
        Value::Int8(v) => Object::Int8(v),
        Value::Int16(v) => Object::Int16(v),
        Value::Int32(v) => Object::Int32(v),
        Value::UInt8(v) => Object::UInt8(v),
        Value::UInt16(v) => Object::UInt16(v),
        Value::UInt32(v) => Object::UInt32(v),
        Value::Float64(v) => Object::Float64(v),
        Value::ConstString(sym) => Object::ConstString(sym),
        Value::Pointer(addr) => Object::Pointer(addr),
        Value::Null(td) => Object::Null(td),
        Value::Unit => Object::Unit,
        Value::Heap(rc) => match Rc::try_unwrap(rc) {
            Ok(cell) => cell.into_inner(),
            // Multiple Rc references: clone out the heap value.
            Err(rc) => rc.borrow().clone(),
        },
    }
}

#[derive(Debug)]
pub(super) enum ArithmeticOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

impl ArithmeticOp {
    fn name(&self) -> &str {
        match self {
            ArithmeticOp::Add => "evaluate_add",
            ArithmeticOp::Sub => "evaluate_sub",
            ArithmeticOp::Mul => "evaluate_mul",
            ArithmeticOp::Div => "evaluate_div",
            ArithmeticOp::Mod => "evaluate_mod",
        }
    }

    fn symbol(&self) -> &str {
        match self {
            ArithmeticOp::Add => "+",
            ArithmeticOp::Sub => "-",
            ArithmeticOp::Mul => "*",
            ArithmeticOp::Div => "/",
            ArithmeticOp::Mod => "%",
        }
    }

    fn apply_i64(&self, l: i64, r: i64) -> i64 {
        // Wrapping arithmetic so the interpreter agrees with the
        // compiler / JIT (cranelift's `iadd` / `isub` / `imul` wrap
        // on overflow). Rust's bare `+` would panic in debug mode
        // and wrap in release — we want a single deterministic
        // semantics across all build modes.
        match self {
            ArithmeticOp::Add => l.wrapping_add(r),
            ArithmeticOp::Sub => l.wrapping_sub(r),
            ArithmeticOp::Mul => l.wrapping_mul(r),
            // Division and remainder by zero still trap (cranelift's
            // `sdiv` / `srem` trap, and Rust's `/` / `%` panic);
            // overflow on signed division (i64::MIN / -1) wraps.
            ArithmeticOp::Div => l.wrapping_div(r),
            // Rust's `%` is truncated remainder, matching most C-family
            // languages — `(-7) % 3 == -1`. Diverges from mathematical
            // modulo, which is fine for our use cases.
            ArithmeticOp::Mod => l.wrapping_rem(r),
        }
    }

    fn apply_u64(&self, l: u64, r: u64) -> u64 {
        match self {
            ArithmeticOp::Add => l.wrapping_add(r),
            ArithmeticOp::Sub => l.wrapping_sub(r),
            ArithmeticOp::Mul => l.wrapping_mul(r),
            ArithmeticOp::Div => l.wrapping_div(r),
            ArithmeticOp::Mod => l.wrapping_rem(r),
        }
    }

    // NUM-W narrow integer arithmetic. Each width has its own
    // `wrapping_*` family in libcore, so the semantics match the
    // i64 / u64 path: silent wrap on overflow, trap on
    // div-by-zero (Rust's `wrapping_div` panics on rhs == 0).
    fn apply_i32(&self, l: i32, r: i32) -> i32 {
        match self {
            ArithmeticOp::Add => l.wrapping_add(r),
            ArithmeticOp::Sub => l.wrapping_sub(r),
            ArithmeticOp::Mul => l.wrapping_mul(r),
            ArithmeticOp::Div => l.wrapping_div(r),
            ArithmeticOp::Mod => l.wrapping_rem(r),
        }
    }
    fn apply_u32(&self, l: u32, r: u32) -> u32 {
        match self {
            ArithmeticOp::Add => l.wrapping_add(r),
            ArithmeticOp::Sub => l.wrapping_sub(r),
            ArithmeticOp::Mul => l.wrapping_mul(r),
            ArithmeticOp::Div => l.wrapping_div(r),
            ArithmeticOp::Mod => l.wrapping_rem(r),
        }
    }
    fn apply_i16(&self, l: i16, r: i16) -> i16 {
        match self {
            ArithmeticOp::Add => l.wrapping_add(r),
            ArithmeticOp::Sub => l.wrapping_sub(r),
            ArithmeticOp::Mul => l.wrapping_mul(r),
            ArithmeticOp::Div => l.wrapping_div(r),
            ArithmeticOp::Mod => l.wrapping_rem(r),
        }
    }
    fn apply_u16(&self, l: u16, r: u16) -> u16 {
        match self {
            ArithmeticOp::Add => l.wrapping_add(r),
            ArithmeticOp::Sub => l.wrapping_sub(r),
            ArithmeticOp::Mul => l.wrapping_mul(r),
            ArithmeticOp::Div => l.wrapping_div(r),
            ArithmeticOp::Mod => l.wrapping_rem(r),
        }
    }
    fn apply_i8(&self, l: i8, r: i8) -> i8 {
        match self {
            ArithmeticOp::Add => l.wrapping_add(r),
            ArithmeticOp::Sub => l.wrapping_sub(r),
            ArithmeticOp::Mul => l.wrapping_mul(r),
            ArithmeticOp::Div => l.wrapping_div(r),
            ArithmeticOp::Mod => l.wrapping_rem(r),
        }
    }
    fn apply_u8(&self, l: u8, r: u8) -> u8 {
        match self {
            ArithmeticOp::Add => l.wrapping_add(r),
            ArithmeticOp::Sub => l.wrapping_sub(r),
            ArithmeticOp::Mul => l.wrapping_mul(r),
            ArithmeticOp::Div => l.wrapping_div(r),
            ArithmeticOp::Mod => l.wrapping_rem(r),
        }
    }

    fn apply_f64(&self, l: f64, r: f64) -> f64 {
        match self {
            ArithmeticOp::Add => l + r,
            ArithmeticOp::Sub => l - r,
            ArithmeticOp::Mul => l * r,
            ArithmeticOp::Div => l / r,
            // f64::rem (Rust's `%` for floats) gives an IEEE-style remainder
            // that matches the sign of the dividend.
            ArithmeticOp::Mod => l % r,
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

    // NUM-W narrow-int comparisons. Same shape as i64 / u64;
    // each width compares natively in its own range.
    fn apply_i32(&self, l: i32, r: i32) -> bool {
        match self {
            ComparisonOp::Eq => l == r, ComparisonOp::Ne => l != r,
            ComparisonOp::Lt => l < r, ComparisonOp::Le => l <= r,
            ComparisonOp::Gt => l > r, ComparisonOp::Ge => l >= r,
        }
    }
    fn apply_u32(&self, l: u32, r: u32) -> bool {
        match self {
            ComparisonOp::Eq => l == r, ComparisonOp::Ne => l != r,
            ComparisonOp::Lt => l < r, ComparisonOp::Le => l <= r,
            ComparisonOp::Gt => l > r, ComparisonOp::Ge => l >= r,
        }
    }
    fn apply_i16(&self, l: i16, r: i16) -> bool {
        match self {
            ComparisonOp::Eq => l == r, ComparisonOp::Ne => l != r,
            ComparisonOp::Lt => l < r, ComparisonOp::Le => l <= r,
            ComparisonOp::Gt => l > r, ComparisonOp::Ge => l >= r,
        }
    }
    fn apply_u16(&self, l: u16, r: u16) -> bool {
        match self {
            ComparisonOp::Eq => l == r, ComparisonOp::Ne => l != r,
            ComparisonOp::Lt => l < r, ComparisonOp::Le => l <= r,
            ComparisonOp::Gt => l > r, ComparisonOp::Ge => l >= r,
        }
    }
    fn apply_i8(&self, l: i8, r: i8) -> bool {
        match self {
            ComparisonOp::Eq => l == r, ComparisonOp::Ne => l != r,
            ComparisonOp::Lt => l < r, ComparisonOp::Le => l <= r,
            ComparisonOp::Gt => l > r, ComparisonOp::Ge => l >= r,
        }
    }
    fn apply_u8(&self, l: u8, r: u8) -> bool {
        match self {
            ComparisonOp::Eq => l == r, ComparisonOp::Ne => l != r,
            ComparisonOp::Lt => l < r, ComparisonOp::Le => l <= r,
            ComparisonOp::Gt => l > r, ComparisonOp::Ge => l >= r,
        }
    }

    fn apply_f64(&self, l: f64, r: f64) -> bool {
        // Standard IEEE 754 ordering: NaN compares false against everything.
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
    /// Phase 2 operator dispatch over `Value`. All-primitive cases
    /// (the overwhelming majority of arithmetic / comparison work)
    /// take an inline-tagged fast path: no `RefCell` borrow, no
    /// `Rc::clone`. Heap-shaped operands (dynamic strings, Allocator
    /// identity) borrow into the underlying `HeapObject` (still
    /// `Object` in this phase) just where needed.
    fn evaluate_comparison_op_v(&self, lhs: &Value, rhs: &Value, op: ComparisonOp) -> Result<Value, InterpreterError> {
        let mismatch = |l: &Value, r: &Value, msg: String| InterpreterError::TypeError {
            expected: l.get_type(),
            found: r.get_type(),
            message: msg,
        };
        Ok(match (lhs, rhs) {
            (Value::Int64(l), Value::Int64(r)) => Value::Bool(op.apply_i64(*l, *r)),
            (Value::UInt64(l), Value::UInt64(r)) => Value::Bool(op.apply_u64(*l, *r)),
            (Value::Int32(l), Value::Int32(r)) => Value::Bool(op.apply_i32(*l, *r)),
            (Value::UInt32(l), Value::UInt32(r)) => Value::Bool(op.apply_u32(*l, *r)),
            (Value::Int16(l), Value::Int16(r)) => Value::Bool(op.apply_i16(*l, *r)),
            (Value::UInt16(l), Value::UInt16(r)) => Value::Bool(op.apply_u16(*l, *r)),
            (Value::Int8(l), Value::Int8(r)) => Value::Bool(op.apply_i8(*l, *r)),
            (Value::UInt8(l), Value::UInt8(r)) => Value::Bool(op.apply_u8(*l, *r)),
            (Value::Float64(l), Value::Float64(r)) => Value::Bool(op.apply_f64(*l, *r)),
            (Value::Bool(l), Value::Bool(r)) => match op {
                ComparisonOp::Eq => Value::Bool(l == r),
                ComparisonOp::Ne => Value::Bool(l != r),
                _ => return Err(mismatch(lhs, rhs, format!(
                    "{}: Bool comparison only supports == and !=", op.name()
                ))),
            },
            (Value::ConstString(l), Value::ConstString(r)) => match op {
                ComparisonOp::Eq | ComparisonOp::Ne => Value::Bool(op.apply_string(*l, *r)),
                _ => return Err(mismatch(lhs, rhs, format!(
                    "{}: String comparison only supports == and !=", op.name()
                ))),
            },
            // Const-vs-dynamic string mixing: resolve the literal once
            // and compare with the heap-side `String`.
            (Value::ConstString(l), Value::Heap(rhs_rc)) => {
                let rhs_obj = rhs_rc.borrow();
                match (&*rhs_obj, &op) {
                    (Object::String(r), ComparisonOp::Eq) => {
                        let l_str = self.string_interner.resolve(*l).unwrap_or("");
                        Value::Bool(l_str == r)
                    }
                    (Object::String(r), ComparisonOp::Ne) => {
                        let l_str = self.string_interner.resolve(*l).unwrap_or("");
                        Value::Bool(l_str != r)
                    }
                    _ => return Err(mismatch(lhs, rhs, format!(
                        "{}: Bad types for binary '{}' operation",
                        op.name(), op.symbol()
                    ))),
                }
            }
            (Value::Heap(lhs_rc), Value::ConstString(r)) => {
                let lhs_obj = lhs_rc.borrow();
                match (&*lhs_obj, &op) {
                    (Object::String(l), ComparisonOp::Eq) => {
                        let r_str = self.string_interner.resolve(*r).unwrap_or("");
                        Value::Bool(l == r_str)
                    }
                    (Object::String(l), ComparisonOp::Ne) => {
                        let r_str = self.string_interner.resolve(*r).unwrap_or("");
                        Value::Bool(l != r_str)
                    }
                    _ => return Err(mismatch(lhs, rhs, format!(
                        "{}: Bad types for binary '{}' operation",
                        op.name(), op.symbol()
                    ))),
                }
            }
            (Value::Heap(lhs_rc), Value::Heap(rhs_rc)) => {
                let lhs_obj = lhs_rc.borrow();
                let rhs_obj = rhs_rc.borrow();
                match (&*lhs_obj, &*rhs_obj) {
                    (Object::String(l), Object::String(r)) => match op {
                        ComparisonOp::Eq => Value::Bool(l == r),
                        ComparisonOp::Ne => Value::Bool(l != r),
                        _ => return Err(mismatch(lhs, rhs, format!(
                            "{}: String comparison only supports == and !=", op.name()
                        ))),
                    },
                    (Object::Allocator(l), Object::Allocator(r)) => {
                        let same = Rc::ptr_eq(l, r);
                        match op {
                            ComparisonOp::Eq => Value::Bool(same),
                            ComparisonOp::Ne => Value::Bool(!same),
                            _ => return Err(mismatch(lhs, rhs, format!(
                                "{}: Allocator comparison only supports == and !=", op.name()
                            ))),
                        }
                    }
                    _ => return Err(mismatch(lhs, rhs, format!(
                        "{}: Bad types for binary '{}' operation",
                        op.name(), op.symbol()
                    ))),
                }
            }
            _ => return Err(mismatch(lhs, rhs, format!(
                "{}: Bad types for binary '{}' operation",
                op.name(), op.symbol()
            ))),
        })
    }

    fn evaluate_arithmetic_op_v(&self, lhs: &Value, rhs: &Value, op: ArithmeticOp) -> Result<Value, InterpreterError> {
        Ok(match (lhs, rhs) {
            (Value::Int64(l), Value::Int64(r)) => Value::Int64(op.apply_i64(*l, *r)),
            (Value::UInt64(l), Value::UInt64(r)) => Value::UInt64(op.apply_u64(*l, *r)),
            // NUM-W narrow integer arithmetic: same-width only
            // (no implicit widening). Cast required to mix
            // widths, mirroring Rust's discipline. Wrap-on-
            // overflow semantics inherited from `apply_*`.
            (Value::Int32(l), Value::Int32(r)) => Value::Int32(op.apply_i32(*l, *r)),
            (Value::UInt32(l), Value::UInt32(r)) => Value::UInt32(op.apply_u32(*l, *r)),
            (Value::Int16(l), Value::Int16(r)) => Value::Int16(op.apply_i16(*l, *r)),
            (Value::UInt16(l), Value::UInt16(r)) => Value::UInt16(op.apply_u16(*l, *r)),
            (Value::Int8(l), Value::Int8(r)) => Value::Int8(op.apply_i8(*l, *r)),
            (Value::UInt8(l), Value::UInt8(r)) => Value::UInt8(op.apply_u8(*l, *r)),
            (Value::Float64(l), Value::Float64(r)) => Value::Float64(op.apply_f64(*l, *r)),
            _ => return Err(InterpreterError::TypeError {
                expected: lhs.get_type(),
                found: rhs.get_type(),
                message: format!(
                    "{}: Bad types for binary '{}' operation due to different type: {:?}",
                    op.name(), op.symbol(), lhs
                ),
            }),
        })
    }

    // Legacy `&Object`-flavoured wrappers retained while other modules
    // still funnel through them (e.g. older tests). They go through
    // the Value path so there's a single source of truth.
    fn evaluate_comparison_op(&self, lhs: &Object, rhs: &Object, op: ComparisonOp) -> Result<Object, InterpreterError> {
        let lv = object_ref_to_value(lhs);
        let rv = object_ref_to_value(rhs);
        Ok(value_to_object(self.evaluate_comparison_op_v(&lv, &rv, op)?))
    }

    fn evaluate_arithmetic_op(&self, lhs: &Object, rhs: &Object, op: ArithmeticOp) -> Result<Object, InterpreterError> {
        let lv = object_ref_to_value(lhs);
        let rv = object_ref_to_value(rhs);
        Ok(value_to_object(self.evaluate_arithmetic_op_v(&lv, &rv, op)?))
    }

    pub fn evaluate_unary(&mut self, op: &UnaryOp, operand: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        use crate::try_value_v;
        let operand_result = self.evaluate(operand);
        let operand_v = try_value_v!(operand_result);

        let result_v = match op {
            UnaryOp::BitwiseNot => match &operand_v {
                Value::UInt64(v) => Value::UInt64(!*v),
                Value::Int64(v) => Value::Int64(!*v),
                _ => return Err(InterpreterError::TypeError {
                    expected: TypeDecl::UInt64,
                    found: operand_v.get_type(),
                    message: format!("Bitwise NOT requires integer type, got {:?}", operand_v),
                }),
            },
            UnaryOp::LogicalNot => match &operand_v {
                Value::Bool(v) => Value::Bool(!*v),
                _ => return Err(InterpreterError::TypeError {
                    expected: TypeDecl::Bool,
                    found: operand_v.get_type(),
                    message: format!("Logical NOT requires boolean type, got {:?}", operand_v),
                }),
            },
            // `wrapping_neg` mirrors the type checker: it only accepts Int64,
            // and the wrapping form avoids panics on `-i64::MIN`.
            UnaryOp::Negate => match &operand_v {
                Value::Int64(v) => Value::Int64(v.wrapping_neg()),
                Value::Float64(v) => Value::Float64(-*v),
                _ => return Err(InterpreterError::TypeError {
                    expected: TypeDecl::Int64,
                    found: operand_v.get_type(),
                    message: format!("Unary minus requires i64 or f64, got {:?}", operand_v),
                }),
            },
        };

        Ok(EvaluationResult::Value(result_v))
    }

    pub fn evaluate_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Short-circuit evaluation for logical operators
        match op {
            Operator::LogicalAnd => return self.evaluate_logical_and_short_circuit(lhs, rhs),
            Operator::LogicalOr => return self.evaluate_logical_or_short_circuit(lhs, rhs),
            _ => {}
        }

        // Lift each operand to `Value` once at entry. Primitive
        // operands (the bulk of arithmetic / comparison work) stay
        // inline thereafter — no `RefCell` borrow per binary op.
        use crate::try_value_v;
        let lhs_result = self.evaluate(lhs);
        let rhs_result = self.evaluate(rhs);
        let lhs_v = try_value_v!(lhs_result);
        let rhs_v = try_value_v!(rhs_result);

        let result_v = match op {
            Operator::IAdd => self.evaluate_arithmetic_op_v(&lhs_v, &rhs_v, ArithmeticOp::Add)?,
            Operator::ISub => self.evaluate_arithmetic_op_v(&lhs_v, &rhs_v, ArithmeticOp::Sub)?,
            Operator::IMul => self.evaluate_arithmetic_op_v(&lhs_v, &rhs_v, ArithmeticOp::Mul)?,
            Operator::IDiv => self.evaluate_arithmetic_op_v(&lhs_v, &rhs_v, ArithmeticOp::Div)?,
            Operator::IMod => self.evaluate_arithmetic_op_v(&lhs_v, &rhs_v, ArithmeticOp::Mod)?,
            Operator::EQ => self.evaluate_comparison_op_v(&lhs_v, &rhs_v, ComparisonOp::Eq)?,
            Operator::NE => self.evaluate_comparison_op_v(&lhs_v, &rhs_v, ComparisonOp::Ne)?,
            Operator::LT => self.evaluate_comparison_op_v(&lhs_v, &rhs_v, ComparisonOp::Lt)?,
            Operator::LE => self.evaluate_comparison_op_v(&lhs_v, &rhs_v, ComparisonOp::Le)?,
            Operator::GT => self.evaluate_comparison_op_v(&lhs_v, &rhs_v, ComparisonOp::Gt)?,
            Operator::GE => self.evaluate_comparison_op_v(&lhs_v, &rhs_v, ComparisonOp::Ge)?,
            Operator::BitwiseAnd => self.evaluate_bitwise_and_v(&lhs_v, &rhs_v)?,
            Operator::BitwiseOr => self.evaluate_bitwise_or_v(&lhs_v, &rhs_v)?,
            Operator::BitwiseXor => self.evaluate_bitwise_xor_v(&lhs_v, &rhs_v)?,
            Operator::LeftShift => self.evaluate_left_shift_v(&lhs_v, &rhs_v)?,
            Operator::RightShift => self.evaluate_right_shift_v(&lhs_v, &rhs_v)?,
            Operator::LogicalAnd | Operator::LogicalOr => unreachable!("Should be handled above"),
        };

        Ok(EvaluationResult::Value(result_v))
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

    pub fn evaluate_mod(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        self.evaluate_arithmetic_op(lhs, rhs, ArithmeticOp::Mod)
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

    // Bitwise operations — Value-flavoured fast paths plus thin
    // `&Object` wrappers retained for any external callers (none in
    // tree today, kept for symmetry with the public API in this file).
    fn evaluate_bitwise_and_v(&self, lhs: &Value, rhs: &Value) -> Result<Value, InterpreterError> {
        match (lhs, rhs) {
            (Value::UInt64(l), Value::UInt64(r)) => Ok(Value::UInt64(*l & *r)),
            (Value::Int64(l), Value::Int64(r)) => Ok(Value::Int64(*l & *r)),
            _ => Err(InterpreterError::TypeError {
                expected: lhs.get_type(),
                found: rhs.get_type(),
                message: format!("Bitwise AND requires same integer types, got {:?} and {:?}", lhs, rhs),
            }),
        }
    }

    fn evaluate_bitwise_or_v(&self, lhs: &Value, rhs: &Value) -> Result<Value, InterpreterError> {
        match (lhs, rhs) {
            (Value::UInt64(l), Value::UInt64(r)) => Ok(Value::UInt64(*l | *r)),
            (Value::Int64(l), Value::Int64(r)) => Ok(Value::Int64(*l | *r)),
            _ => Err(InterpreterError::TypeError {
                expected: lhs.get_type(),
                found: rhs.get_type(),
                message: format!("Bitwise OR requires same integer types, got {:?} and {:?}", lhs, rhs),
            }),
        }
    }

    fn evaluate_bitwise_xor_v(&self, lhs: &Value, rhs: &Value) -> Result<Value, InterpreterError> {
        match (lhs, rhs) {
            (Value::UInt64(l), Value::UInt64(r)) => Ok(Value::UInt64(*l ^ *r)),
            (Value::Int64(l), Value::Int64(r)) => Ok(Value::Int64(*l ^ *r)),
            _ => Err(InterpreterError::TypeError {
                expected: lhs.get_type(),
                found: rhs.get_type(),
                message: format!("Bitwise XOR requires same integer types, got {:?} and {:?}", lhs, rhs),
            }),
        }
    }

    fn evaluate_left_shift_v(&self, lhs: &Value, rhs: &Value) -> Result<Value, InterpreterError> {
        let shift_amount = match rhs {
            Value::UInt64(r) => *r,
            _ => return Err(InterpreterError::TypeError {
                expected: TypeDecl::UInt64,
                found: rhs.get_type(),
                message: format!("Shift amount must be UInt64, got {:?}", rhs),
            }),
        };
        match lhs {
            Value::UInt64(l) => Ok(Value::UInt64(l.wrapping_shl(shift_amount as u32))),
            Value::Int64(l) => Ok(Value::Int64(l.wrapping_shl(shift_amount as u32))),
            _ => Err(InterpreterError::TypeError {
                expected: TypeDecl::UInt64,
                found: lhs.get_type(),
                message: format!("Left shift requires integer type on left side, got {:?}", lhs),
            }),
        }
    }

    fn evaluate_right_shift_v(&self, lhs: &Value, rhs: &Value) -> Result<Value, InterpreterError> {
        let shift_amount = match rhs {
            Value::UInt64(r) => *r,
            _ => return Err(InterpreterError::TypeError {
                expected: TypeDecl::UInt64,
                found: rhs.get_type(),
                message: format!("Shift amount must be UInt64, got {:?}", rhs),
            }),
        };
        match lhs {
            Value::UInt64(l) => Ok(Value::UInt64(l.wrapping_shr(shift_amount as u32))),
            Value::Int64(l) => Ok(Value::Int64(l.wrapping_shr(shift_amount as u32))),
            _ => Err(InterpreterError::TypeError {
                expected: TypeDecl::UInt64,
                found: lhs.get_type(),
                message: format!("Right shift requires integer type on left side, got {:?}", lhs),
            }),
        }
    }

    pub fn evaluate_bitwise_and(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lv = object_ref_to_value(lhs);
        let rv = object_ref_to_value(rhs);
        Ok(value_to_object(self.evaluate_bitwise_and_v(&lv, &rv)?))
    }

    pub fn evaluate_bitwise_or(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lv = object_ref_to_value(lhs);
        let rv = object_ref_to_value(rhs);
        Ok(value_to_object(self.evaluate_bitwise_or_v(&lv, &rv)?))
    }

    pub fn evaluate_bitwise_xor(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lv = object_ref_to_value(lhs);
        let rv = object_ref_to_value(rhs);
        Ok(value_to_object(self.evaluate_bitwise_xor_v(&lv, &rv)?))
    }

    pub fn evaluate_left_shift(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lv = object_ref_to_value(lhs);
        let rv = object_ref_to_value(rhs);
        Ok(value_to_object(self.evaluate_left_shift_v(&lv, &rv)?))
    }

    pub fn evaluate_right_shift(&self, lhs: &Object, rhs: &Object) -> Result<Object, InterpreterError> {
        let lv = object_ref_to_value(lhs);
        let rv = object_ref_to_value(rhs);
        Ok(value_to_object(self.evaluate_right_shift_v(&lv, &rv)?))
    }

    // Short-circuit evaluation for logical AND
    pub fn evaluate_logical_and_short_circuit(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        use crate::try_value_v;
        let lhs_v = try_value_v!(self.evaluate(lhs));
        let lhs_bool = lhs_v.try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
        if !lhs_bool {
            return Ok(EvaluationResult::Value(Value::Bool(false)));
        }
        let rhs_v = try_value_v!(self.evaluate(rhs));
        let rhs_bool = rhs_v.try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
        Ok(EvaluationResult::Value(Value::Bool(rhs_bool)))
    }

    // Short-circuit evaluation for logical OR
    pub fn evaluate_logical_or_short_circuit(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        use crate::try_value_v;
        let lhs_v = try_value_v!(self.evaluate(lhs));
        let lhs_bool = lhs_v.try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
        if lhs_bool {
            return Ok(EvaluationResult::Value(Value::Bool(true)));
        }
        let rhs_v = try_value_v!(self.evaluate(rhs));
        let rhs_bool = rhs_v.try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
        Ok(EvaluationResult::Value(Value::Bool(rhs_bool)))
    }
}
