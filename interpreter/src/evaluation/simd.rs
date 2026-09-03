//! SIMD intrinsics for the tree-walker (SIMD.md Phase 2).
//!
//! This engine is the correctness oracle the compiled lanes are
//! checked against, so every operation here is written the obvious
//! way — a loop over lanes — and the semantics SIMD.md fixes are
//! visible in the code rather than delegated to a machine
//! instruction:
//!
//! * `__simd_reduce_*` folds lane 0 through n **in order**. A
//!   pairwise tree would give a different `f64` sum here than
//!   cranelift's reduction gives in the compiled lanes.
//! * Integer lanes wrap; nothing traps.
//! * `__simd_load(p, i)` addresses by **element**, not by byte:
//!   lane `k` lives at byte offset `(i + k) * lane_bytes`. This is
//!   the one place the `__simd_*` family departs from
//!   `__builtin_ptr_read`, whose offset is a byte count.

use frontend::ast::{ExprRef, Operator, SimdOp, UnaryOp};
use frontend::type_decl::VectorType;

use std::cell::RefCell;
use std::rc::Rc;

use super::{EvaluationContext, EvaluationResult};
use crate::error::InterpreterError;
use crate::object::{Object, SimdValue};
use crate::try_value;
use crate::value::Value;

impl<'a> EvaluationContext<'a> {
    pub(super) fn builtin_simd(
        &mut self,
        op: SimdOp,
        args: &[ExprRef],
    ) -> Result<EvaluationResult, InterpreterError> {
        let name = op.builtin_name();

        // Every intrinsic evaluates all of its arguments left to
        // right; none of them short-circuits.
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            let v = self.evaluate(arg)?;
            values.push(try_value!(Ok(v)));
        }

        // `__simd_splat` / `__simd_load` carry their result type as a
        // trailing synthetic argument, stamped from the annotation at
        // the call site (`type_checker::simd::stamp_simd_result_types`).
        let result_ty = if op.needs_result_annotation() && values.len() == op.arity() + 1 {
            let code = values[op.arity()].borrow().try_unwrap_uint64().ok();
            values.truncate(op.arity());
            code.and_then(VectorType::from_code)
        } else {
            None
        };
        if values.len() != op.arity() {
            return Err(InterpreterError::InternalError(format!(
                "{name} expects {} argument(s), got {}",
                op.arity(),
                values.len()
            )));
        }
        let scalar = |i: usize| values[i].borrow().clone();

        let value = match op {
            SimdOp::Splat => {
                let ty = Self::simd_annotation(name, result_ty)?;
                let lane = scalar(0);
                Object::Simd(SimdValue::splat(ty, &lane).ok_or_else(|| {
                    InterpreterError::InternalError(format!(
                        "{name}: lane value is not a {}",
                        ty.lane().display_name()
                    ))
                })?)
            }
            SimdOp::Load => {
                let ty = Self::simd_annotation(name, result_ty)?;
                let (addr, index) = self.simd_addr(name, &values)?;
                Object::Simd(self.simd_read(ty, addr, index))
            }
            SimdOp::Store => {
                let (addr, index) = self.simd_addr(name, &values)?;
                let vector = Self::simd_operand(name, &scalar(2))?;
                self.simd_write(vector, addr, index);
                Object::Unit
            }
            SimdOp::Extract => {
                let vector = Self::simd_operand(name, &scalar(0))?;
                let k = Self::simd_lane_index(name, &scalar(1), vector.lanes())?;
                vector.lane(k)
            }
            SimdOp::Insert => {
                let vector = Self::simd_operand(name, &scalar(0))?;
                let k = Self::simd_lane_index(name, &scalar(1), vector.lanes())?;
                Object::Simd(vector.with_lane(k, &scalar(2)))
            }
            SimdOp::Select => {
                let mask = Self::simd_operand(name, &scalar(0))?;
                let a = Self::simd_operand(name, &scalar(1))?;
                let b = Self::simd_operand(name, &scalar(2))?;
                let mut out = b;
                for k in 0..a.lanes() {
                    if mask.lane_is_set(k) {
                        out = out.with_lane(k, &a.lane(k));
                    }
                }
                Object::Simd(out)
            }
            SimdOp::ReduceAdd
            | SimdOp::ReduceMin
            | SimdOp::ReduceMax
            | SimdOp::ReduceAnd
            | SimdOp::ReduceOr => {
                let vector = Self::simd_operand(name, &scalar(0))?;
                Self::simd_reduce(op, vector)
            }
            SimdOp::Any => {
                let mask = Self::simd_operand(name, &scalar(0))?;
                Object::Bool((0..mask.lanes()).any(|k| mask.lane_is_set(k)))
            }
            SimdOp::All => {
                let mask = Self::simd_operand(name, &scalar(0))?;
                Object::Bool((0..mask.lanes()).all(|k| mask.lane_is_set(k)))
            }
            SimdOp::Bitmask => {
                let mask = Self::simd_operand(name, &scalar(0))?;
                let mut bits = 0u64;
                for k in 0..mask.lanes() {
                    if mask.lane_high_bit(k) {
                        bits |= 1u64 << k;
                    }
                }
                Object::UInt64(bits)
            }
            SimdOp::Swizzle => {
                let table = Self::simd_operand(name, &scalar(0))?.to_bytes();
                let indices = Self::simd_operand(name, &scalar(1))?.to_bytes();
                let mut out = [0u8; 16];
                for (k, slot) in out.iter_mut().enumerate() {
                    // Out of range selects zero, which is what both
                    // `pshufb` (after cranelift's normalisation) and
                    // NEON's `tbl` do.
                    let i = indices[k] as usize;
                    *slot = if i < 16 { table[i] } else { 0 };
                }
                Object::Simd(SimdValue::U8x16(out))
            }
            SimdOp::Bitcast => {
                let ty = Self::simd_annotation(name, result_ty)?;
                let vector = Self::simd_operand(name, &scalar(0))?;
                Object::Simd(SimdValue::from_bytes(ty, &vector.to_bytes()))
            }
        };
        Ok(EvaluationResult::Value(value.into()))
    }

    /// Fold every lane left to right. The order is the language
    /// definition, not an implementation choice — see the module note.
    fn simd_reduce(op: SimdOp, vector: SimdValue) -> Object {
        macro_rules! fold {
            ($lanes:expr, $init:expr, $add:expr, $min:expr, $max:expr, $and:expr, $or:expr, $wrap:expr) => {{
                let lanes = $lanes;
                let mut acc = $init;
                for (i, x) in lanes.iter().enumerate() {
                    let x = *x;
                    acc = if i == 0 {
                        x
                    } else {
                        match op {
                            SimdOp::ReduceAdd => $add(acc, x),
                            SimdOp::ReduceMin => $min(acc, x),
                            SimdOp::ReduceMax => $max(acc, x),
                            SimdOp::ReduceAnd => $and(acc, x),
                            SimdOp::ReduceOr => $or(acc, x),
                            _ => acc,
                        }
                    };
                }
                $wrap(acc)
            }};
        }
        match vector {
            // `f64::min` / `f64::max` propagate the non-NaN operand,
            // which is what cranelift's `fmin` / `fmax` do.
            SimdValue::F64x2(v) => fold!(v, 0.0f64,
                |a: f64, b: f64| a + b, f64::min, f64::max,
                |a, _| a, |a, _| a, Object::Float64),
            SimdValue::F32x4(v) => fold!(v, 0.0f32,
                |a: f32, b: f32| a + b, f32::min, f32::max,
                |a, _| a, |a, _| a, Object::Float32),
            SimdValue::I32x4(v) => fold!(v, 0i32,
                i32::wrapping_add, i32::min, i32::max,
                |a: i32, b: i32| a & b, |a: i32, b: i32| a | b, Object::Int32),
            SimdValue::I64x2(v) => fold!(v, 0i64,
                i64::wrapping_add, i64::min, i64::max,
                |a: i64, b: i64| a & b, |a: i64, b: i64| a | b, Object::Int64),
            SimdValue::U8x16(v) => fold!(v, 0u8,
                u8::wrapping_add, u8::min, u8::max,
                |a: u8, b: u8| a & b, |a: u8, b: u8| a | b, Object::UInt8),
        }
    }

    /// The vector type `__simd_splat` / `__simd_load` were asked for.
    /// The type checker has already rejected a call site with no
    /// annotation, so a missing hint here is an internal error.
    fn simd_annotation(
        name: &str,
        stamped: Option<VectorType>,
    ) -> Result<VectorType, InterpreterError> {
        stamped.ok_or_else(|| {
            InterpreterError::InternalError(format!(
                "{name}: no vector type stamped on the call"
            ))
        })
    }

    /// The `(pointer, element index)` pair the memory intrinsics
    /// share.
    fn simd_addr(
        &self,
        name: &str,
        values: &[crate::object::RcObject],
    ) -> Result<(usize, u64), InterpreterError> {
        let addr = values[0].borrow().try_unwrap_pointer().map_err(|_| {
            InterpreterError::InternalError(format!("{name} expects a pointer as its first argument"))
        })?;
        let index = values[1].borrow().try_unwrap_uint64().map_err(|_| {
            InterpreterError::InternalError(format!("{name} expects a u64 element index"))
        })?;
        Ok((addr, index))
    }

    /// Unwrap a vector argument.
    fn simd_operand(name: &str, value: &Object) -> Result<SimdValue, InterpreterError> {
        match value {
            Object::Simd(v) => Ok(*v),
            other => Err(InterpreterError::InternalError(format!(
                "{name} expects a vector, got {}",
                other.get_type().display_name()
            ))),
        }
    }

    /// Unwrap a lane index. The type checker has already proved it is
    /// a literal in range.
    fn simd_lane_index(name: &str, value: &Object, lanes: usize) -> Result<usize, InterpreterError> {
        let k = value.try_unwrap_uint64().map_err(|_| {
            InterpreterError::InternalError(format!("{name} expects a u64 lane index"))
        })? as usize;
        if k >= lanes {
            return Err(InterpreterError::InternalError(format!(
                "{name}: lane {k} is out of range for a {lanes}-lane vector"
            )));
        }
        Ok(k)
    }

    /// Read one vector out of the heap, lane by lane.
    ///
    /// A lane comes from the typed-slot map when a
    /// `__builtin_ptr_write` (or an earlier `__simd_store`) put a
    /// value of the lane type there, and from the raw byte buffer
    /// otherwise. Consulting both is what makes a vector read back
    /// what `Vec<T>::push` wrote — the same two-sided rule
    /// `HeapManager::read_byte_at` documents.
    fn simd_read(&self, ty: VectorType, addr: usize, index: u64) -> SimdValue {
        let stride = ty.lane_bytes();
        // Fast path: every lane has a typed slot of the lane's own
        // type, which is what `Vec<T>::push` and an earlier
        // `__simd_store` leave behind.
        let mut lanes: Vec<Object> = Vec::with_capacity(ty.lanes());
        for k in 0..ty.lanes() {
            let offset = (index as usize + k) * stride;
            match self.heap_manager.borrow().typed_read(addr, offset) {
                Some(slot) if slot.borrow().get_type() == ty.lane() => {
                    lanes.push(slot.borrow().clone());
                }
                _ => break,
            }
        }
        if lanes.len() == ty.lanes() {
            return SimdValue::from_lanes(ty, &lanes);
        }
        // Otherwise read the whole vector as bytes. Every lane goes
        // through `read_byte_at`, including the ones that did have a
        // typed slot: mixing the two would leave the typed lanes as
        // zeros in `bytes`.
        let mut bytes = [0u8; 16];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = self
                .heap_manager
                .borrow()
                .read_byte_at(addr, index as usize * stride + i);
        }
        SimdValue::from_bytes(ty, &bytes)
    }

    /// Write one vector into the heap, lane by lane, updating both the
    /// typed slots and the raw bytes so either kind of reader sees it.
    fn simd_write(&mut self, vector: SimdValue, addr: usize, index: u64) {
        let ty = vector.vector_type();
        let stride = ty.lane_bytes();
        let bytes = vector.to_bytes();
        for k in 0..ty.lanes() {
            let offset = (index as usize + k) * stride;
            let lane = vector.lane(k);
            self.heap_manager
                .borrow_mut()
                .typed_write(addr, offset, Rc::new(RefCell::new(lane)));
            self.heap_manager
                .borrow_mut()
                .write_bytes_raw(addr + offset, &bytes[k * stride..(k + 1) * stride]);
        }
    }
}

/// Lane-wise binary operators for the tree-walker.
///
/// Written as a loop over lanes so the semantics are visible:
/// integer lanes **wrap** (never trap, unlike the scalar `u64 -` /
/// `/` which panic), comparison produces an all-ones / all-zeros
/// mask rather than a `bool`, and integer `/` `%` never get here at
/// all — the type checker rejects them.
pub(super) fn simd_binary(
    op: &Operator,
    a: SimdValue,
    b: SimdValue,
) -> Result<Value, InterpreterError> {
    let ty = a.vector_type();
    if matches!(
        op,
        Operator::EQ | Operator::NE | Operator::LT | Operator::LE | Operator::GT | Operator::GE
    ) {
        let bits: Vec<bool> = (0..ty.lanes()).map(|k| simd_compare_lane(op, &a, &b, k)).collect();
        return Ok(Value::Simd(SimdValue::mask_from_bools(ty, &bits)));
    }
    let mut out = SimdValue::zeroed(ty);
    for k in 0..ty.lanes() {
        out = out.with_lane(k, &simd_arith_lane(op, &a, &b, k)?);
    }
    Ok(Value::Simd(out))
}

/// Lane-wise shift: every lane by the same scalar amount, taken
/// modulo the lane width the way every SIMD shift instruction does.
pub(super) fn simd_shift(op: &Operator, v: SimdValue, amount: u64) -> Value {
    let ty = v.vector_type();
    let bits = (ty.lane_bytes() * 8) as u32;
    let k = (amount % bits as u64) as u32;
    let left = matches!(op, Operator::LeftShift);
    let mut out = SimdValue::zeroed(ty);
    for lane in 0..ty.lanes() {
        let shifted = match v.lane(lane) {
            Object::Int32(x) => {
                Object::Int32(if left { x.wrapping_shl(k) } else { x.wrapping_shr(k) })
            }
            Object::Int64(x) => {
                Object::Int64(if left { x.wrapping_shl(k) } else { x.wrapping_shr(k) })
            }
            Object::UInt8(x) => {
                Object::UInt8(if left { x.wrapping_shl(k) } else { x.wrapping_shr(k) })
            }
            // Float lanes are rejected by the type checker.
            other => other,
        };
        out = out.with_lane(lane, &shifted);
    }
    Value::Simd(out)
}

/// Lane-wise unary operators. `!` never reaches here (rejected by
/// the type checker); `-` and `~` are per-lane.
pub(super) fn simd_unary(op: &UnaryOp, v: SimdValue) -> Result<Value, InterpreterError> {
    let ty = v.vector_type();
    let mut out = SimdValue::zeroed(ty);
    for k in 0..ty.lanes() {
        let lane = match (op, v.lane(k)) {
            (UnaryOp::Negate, Object::Float64(x)) => Object::Float64(-x),
            (UnaryOp::Negate, Object::Float32(x)) => Object::Float32(-x),
            (UnaryOp::Negate, Object::Int32(x)) => Object::Int32(x.wrapping_neg()),
            (UnaryOp::Negate, Object::Int64(x)) => Object::Int64(x.wrapping_neg()),
            (UnaryOp::BitwiseNot, Object::Int32(x)) => Object::Int32(!x),
            (UnaryOp::BitwiseNot, Object::Int64(x)) => Object::Int64(!x),
            (UnaryOp::BitwiseNot, Object::UInt8(x)) => Object::UInt8(!x),
            (other, lane) => {
                return Err(InterpreterError::InternalError(format!(
                    "lane-wise {other:?} is not defined on {}",
                    lane.get_type().display_name()
                )));
            }
        };
        out = out.with_lane(k, &lane);
    }
    Ok(Value::Simd(out))
}

/// One lane of an arithmetic / bitwise operator.
fn simd_arith_lane(
    op: &Operator,
    a: &SimdValue,
    b: &SimdValue,
    k: usize,
) -> Result<Object, InterpreterError> {
    macro_rules! int_lane {
        ($x:expr, $y:expr, $ctor:path, $ty:ty) => {{
            let (x, y) = ($x, $y);
            let _ = std::mem::size_of::<$ty>();
            $ctor(match op {
                Operator::IAdd => x.wrapping_add(y),
                Operator::ISub => x.wrapping_sub(y),
                Operator::IMul => x.wrapping_mul(y),
                Operator::BitwiseAnd => x & y,
                Operator::BitwiseOr => x | y,
                Operator::BitwiseXor => x ^ y,
                other => {
                    return Err(InterpreterError::InternalError(format!(
                        "lane-wise {other:?} is not defined on integer lanes"
                    )));
                }
            })
        }};
    }
    macro_rules! float_lane {
        ($x:expr, $y:expr, $ctor:path) => {{
            let (x, y) = ($x, $y);
            $ctor(match op {
                Operator::IAdd => x + y,
                Operator::ISub => x - y,
                Operator::IMul => x * y,
                // IEEE division does not trap, so unlike the integer
                // lanes this one is offered.
                Operator::IDiv => x / y,
                other => {
                    return Err(InterpreterError::InternalError(format!(
                        "lane-wise {other:?} is not defined on float lanes"
                    )));
                }
            })
        }};
    }
    Ok(match (a.lane(k), b.lane(k)) {
        (Object::Float64(x), Object::Float64(y)) => float_lane!(x, y, Object::Float64),
        (Object::Float32(x), Object::Float32(y)) => float_lane!(x, y, Object::Float32),
        (Object::Int32(x), Object::Int32(y)) => int_lane!(x, y, Object::Int32, i32),
        (Object::Int64(x), Object::Int64(y)) => int_lane!(x, y, Object::Int64, i64),
        (Object::UInt8(x), Object::UInt8(y)) => int_lane!(x, y, Object::UInt8, u8),
        (x, _) => {
            return Err(InterpreterError::InternalError(format!(
                "lane-wise operator on mismatched lane type {}",
                x.get_type().display_name()
            )));
        }
    })
}

/// One lane of a comparison. Float lanes use IEEE ordering, so a NaN
/// lane compares false everywhere except `!=`.
fn simd_compare_lane(op: &Operator, a: &SimdValue, b: &SimdValue, k: usize) -> bool {
    macro_rules! cmp {
        ($x:expr, $y:expr) => {{
            let (x, y) = ($x, $y);
            match op {
                Operator::EQ => x == y,
                Operator::NE => x != y,
                Operator::LT => x < y,
                Operator::LE => x <= y,
                Operator::GT => x > y,
                Operator::GE => x >= y,
                _ => false,
            }
        }};
    }
    match (a.lane(k), b.lane(k)) {
        (Object::Float64(x), Object::Float64(y)) => cmp!(x, y),
        (Object::Float32(x), Object::Float32(y)) => cmp!(x, y),
        (Object::Int32(x), Object::Int32(y)) => cmp!(x, y),
        (Object::Int64(x), Object::Int64(y)) => cmp!(x, y),
        (Object::UInt8(x), Object::UInt8(y)) => cmp!(x, y),
        _ => false,
    }
}
