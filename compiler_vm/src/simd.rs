//! SIMD execution for the IR VM (SIMD.md Phase 2).
//!
//! A vector is one `RawSlot`, holding the same 16-byte little-endian
//! image the compiled backends put in a register and `__simd_store`
//! writes to memory. Every operation here decodes lanes, works on
//! them, and re-encodes — slower than the cranelift instruction the
//! AOT lane gets, but identical in result, which is what
//! `assert_consistent` checks.
//!
//! The semantics SIMD.md fixes, restated where they are implemented:
//! integer lanes wrap and never trap, comparisons produce an
//! all-ones / all-zeros mask, and `reduce` folds lane 0 through n in
//! order rather than as a pairwise tree.

use compiler_ir::{BinOp, SimdReduceOp, UnaryOp, VecTy};

use crate::slot::RawSlot;

/// The lanes of a vector, decoded from its memory image.
#[derive(Clone, Copy)]
pub enum Lanes {
    F64([f64; 2]),
    F32([f32; 4]),
    I32([i32; 4]),
    I64([i64; 2]),
    U8([u8; 16]),
}

/// Decode the 16-byte image into typed lanes.
pub fn decode(bytes: [u8; 16], ty: VecTy) -> Lanes {
    macro_rules! read {
        ($t:ty, $n:expr, $w:expr) => {{
            let mut out = [<$t>::default(); $n];
            for (i, slot) in out.iter_mut().enumerate() {
                *slot = <$t>::from_le_bytes(bytes[i * $w..i * $w + $w].try_into().unwrap());
            }
            out
        }};
    }
    match ty {
        VecTy::F64x2 => Lanes::F64(read!(f64, 2, 8)),
        VecTy::F32x4 => Lanes::F32(read!(f32, 4, 4)),
        VecTy::I32x4 => Lanes::I32(read!(i32, 4, 4)),
        VecTy::I64x2 => Lanes::I64(read!(i64, 2, 8)),
        VecTy::U8x16 => Lanes::U8(bytes),
    }
}

/// The inverse of [`decode`].
pub fn encode(lanes: Lanes) -> [u8; 16] {
    let mut out = [0u8; 16];
    macro_rules! write {
        ($v:expr, $w:expr) => {{
            for (i, x) in $v.iter().enumerate() {
                out[i * $w..i * $w + $w].copy_from_slice(&x.to_le_bytes());
            }
        }};
    }
    match lanes {
        Lanes::F64(v) => write!(v, 8),
        Lanes::F32(v) => write!(v, 4),
        Lanes::I32(v) => write!(v, 4),
        Lanes::I64(v) => write!(v, 8),
        Lanes::U8(v) => out.copy_from_slice(&v),
    }
    out
}

/// One lane broadcast into all of them. The scalar arrives in a
/// `RawSlot` of the lane's own type.
pub fn splat(scalar: RawSlot, ty: VecTy) -> [u8; 16] {
    encode(match ty {
        VecTy::F64x2 => Lanes::F64([unsafe { scalar.f64 }; 2]),
        VecTy::F32x4 => Lanes::F32([scalar.read_f32(); 4]),
        VecTy::I32x4 => Lanes::I32([unsafe { scalar.i64 } as i32; 4]),
        VecTy::I64x2 => Lanes::I64([unsafe { scalar.i64 }; 2]),
        VecTy::U8x16 => Lanes::U8([unsafe { scalar.u64 } as u8; 16]),
    })
}

/// Lane `k` as a scalar slot of the lane type.
pub fn extract(bytes: [u8; 16], lane: usize, ty: VecTy) -> RawSlot {
    match decode(bytes, ty) {
        Lanes::F64(v) => RawSlot::from_f64(v[lane]),
        Lanes::F32(v) => RawSlot::from_f32(v[lane]),
        Lanes::I32(v) => RawSlot::from_i64(v[lane] as i64),
        Lanes::I64(v) => RawSlot::from_i64(v[lane]),
        Lanes::U8(v) => RawSlot::from_u64(v[lane] as u64),
    }
}

/// `bytes` with lane `k` replaced by `scalar`.
pub fn insert(bytes: [u8; 16], lane: usize, scalar: RawSlot, ty: VecTy) -> [u8; 16] {
    encode(match decode(bytes, ty) {
        Lanes::F64(mut v) => {
            v[lane] = unsafe { scalar.f64 };
            Lanes::F64(v)
        }
        Lanes::F32(mut v) => {
            v[lane] = scalar.read_f32();
            Lanes::F32(v)
        }
        Lanes::I32(mut v) => {
            v[lane] = unsafe { scalar.i64 } as i32;
            Lanes::I32(v)
        }
        Lanes::I64(mut v) => {
            v[lane] = unsafe { scalar.i64 };
            Lanes::I64(v)
        }
        Lanes::U8(mut v) => {
            v[lane] = unsafe { scalar.u64 } as u8;
            Lanes::U8(v)
        }
    })
}

/// Lane-wise choice: `a` where the mask lane is non-zero, `b`
/// otherwise. The mask's lanes are the same width as the value's, so
/// one index walks both.
pub fn select(mask: [u8; 16], a: [u8; 16], b: [u8; 16], ty: VecTy) -> [u8; 16] {
    let width = ty.lane_bytes();
    let mut out = b;
    for k in 0..ty.lanes() {
        let set = mask[k * width..(k + 1) * width].iter().any(|byte| *byte != 0);
        if set {
            out[k * width..(k + 1) * width].copy_from_slice(&a[k * width..(k + 1) * width]);
        }
    }
    out
}

/// `__simd_any` / `__simd_all`.
pub fn test(mask: [u8; 16], all: bool, ty: VecTy) -> bool {
    let width = ty.lane_bytes();
    let mut lanes = (0..ty.lanes())
        .map(|k| mask[k * width..(k + 1) * width].iter().any(|byte| *byte != 0));
    if all { lanes.all(|x| x) } else { lanes.any(|x| x) }
}

/// A horizontal fold, lane 0 through n in order.
pub fn reduce(bytes: [u8; 16], op: SimdReduceOp, ty: VecTy) -> RawSlot {
    macro_rules! fold {
        ($v:expr, $add:expr, $min:expr, $max:expr, $wrap:expr) => {{
            let v = $v;
            let mut acc = v[0];
            for x in v.iter().skip(1) {
                acc = match op {
                    SimdReduceOp::Add => $add(acc, *x),
                    SimdReduceOp::Min => $min(acc, *x),
                    SimdReduceOp::Max => $max(acc, *x),
                    // Bitwise folds never reach a float arm — the
                    // type checker rejects `__simd_reduce_and` on
                    // float lanes.
                    SimdReduceOp::And | SimdReduceOp::Or => acc,
                };
            }
            $wrap(acc)
        }};
    }
    macro_rules! fold_int {
        ($v:expr, $t:ty, $wrap:expr) => {{
            let v = $v;
            let mut acc = v[0];
            for x in v.iter().skip(1) {
                acc = match op {
                    SimdReduceOp::Add => acc.wrapping_add(*x),
                    SimdReduceOp::Min => acc.min(*x),
                    SimdReduceOp::Max => acc.max(*x),
                    SimdReduceOp::And => acc & *x,
                    SimdReduceOp::Or => acc | *x,
                };
            }
            let _: $t = acc;
            $wrap(acc)
        }};
    }
    match decode(bytes, ty) {
        // `f64::min` / `f64::max` propagate the non-NaN operand,
        // matching cranelift's `fmin` / `fmax`.
        Lanes::F64(v) => fold!(v, |a: f64, b: f64| a + b, f64::min, f64::max, RawSlot::from_f64),
        Lanes::F32(v) => fold!(v, |a: f32, b: f32| a + b, f32::min, f32::max, RawSlot::from_f32),
        Lanes::I32(v) => fold_int!(v, i32, |a: i32| RawSlot::from_i64(a as i64)),
        Lanes::I64(v) => fold_int!(v, i64, RawSlot::from_i64),
        Lanes::U8(v) => fold_int!(v, u8, |a: u8| RawSlot::from_u64(a as u64)),
    }
}

/// Lane-wise binary operator. Comparisons produce a mask of the same
/// lane width; everything else produces a vector of the operand type.
pub fn binop(op: BinOp, lhs: [u8; 16], rhs: [u8; 16], ty: VecTy) -> [u8; 16] {
    if op.produces_bool() {
        return compare(op, lhs, rhs, ty);
    }
    macro_rules! arith_int {
        ($a:expr, $b:expr, $ctor:expr) => {{
            let (a, b) = ($a, $b);
            let mut out = a;
            for (k, slot) in out.iter_mut().enumerate() {
                let (x, y) = (a[k], b[k]);
                *slot = match op {
                    BinOp::Add => x.wrapping_add(y),
                    BinOp::Sub => x.wrapping_sub(y),
                    BinOp::Mul => x.wrapping_mul(y),
                    BinOp::BitAnd => x & y,
                    BinOp::BitOr => x | y,
                    BinOp::BitXor => x ^ y,
                    BinOp::Min => x.min(y),
                    BinOp::Max => x.max(y),
                    // `/` and `%` on integer lanes are rejected by
                    // the type checker (a per-lane zero guard would
                    // defeat the vectorisation), and `Pow` has no
                    // lane-wise form.
                    _ => x,
                };
            }
            $ctor(out)
        }};
    }
    macro_rules! arith_float {
        ($a:expr, $b:expr, $ctor:expr) => {{
            let (a, b) = ($a, $b);
            let mut out = a;
            for (k, slot) in out.iter_mut().enumerate() {
                let (x, y) = (a[k], b[k]);
                *slot = match op {
                    BinOp::Add => x + y,
                    BinOp::Sub => x - y,
                    BinOp::Mul => x * y,
                    // IEEE division does not trap, so float lanes
                    // get `/` even though integer lanes do not.
                    BinOp::Div => x / y,
                    BinOp::Min => x.min(y),
                    BinOp::Max => x.max(y),
                    _ => x,
                };
            }
            $ctor(out)
        }};
    }
    encode(match (decode(lhs, ty), decode(rhs, ty)) {
        (Lanes::F64(a), Lanes::F64(b)) => arith_float!(a, b, Lanes::F64),
        (Lanes::F32(a), Lanes::F32(b)) => arith_float!(a, b, Lanes::F32),
        (Lanes::I32(a), Lanes::I32(b)) => arith_int!(a, b, Lanes::I32),
        (Lanes::I64(a), Lanes::I64(b)) => arith_int!(a, b, Lanes::I64),
        (Lanes::U8(a), Lanes::U8(b)) => arith_int!(a, b, Lanes::U8),
        // Both sides share `ty`, so the decoders always agree.
        _ => unreachable!("simd binop operands disagree on lane type"),
    })
}

/// Lane-wise shift: every lane by the same scalar amount, taken
/// modulo the lane width — the shape cranelift's vector `ishl` /
/// `ushr` / `sshr` have, and the one every SIMD ISA offers.
pub fn shift(op: BinOp, value: [u8; 16], amount: u64, ty: VecTy) -> [u8; 16] {
    let bits = (ty.lane_bytes() * 8) as u32;
    let k = (amount % bits as u64) as u32;
    let left = matches!(op, BinOp::Shl);
    encode(match decode(value, ty) {
        Lanes::I32(mut v) => {
            for x in v.iter_mut() {
                *x = if left { x.wrapping_shl(k) } else { x.wrapping_shr(k) };
            }
            Lanes::I32(v)
        }
        Lanes::I64(mut v) => {
            for x in v.iter_mut() {
                *x = if left { x.wrapping_shl(k) } else { x.wrapping_shr(k) };
            }
            Lanes::I64(v)
        }
        Lanes::U8(mut v) => {
            for x in v.iter_mut() {
                *x = if left { x.wrapping_shl(k) } else { x.wrapping_shr(k) };
            }
            Lanes::U8(v)
        }
        // Float lanes are rejected by the type checker.
        other => other,
    })
}

/// Lane-wise unary operator.
pub fn unaryop(op: UnaryOp, value: [u8; 16], ty: VecTy) -> [u8; 16] {
    encode(match decode(value, ty) {
        Lanes::F64(mut v) => {
            for x in v.iter_mut() {
                *x = -*x;
            }
            Lanes::F64(v)
        }
        Lanes::F32(mut v) => {
            for x in v.iter_mut() {
                *x = -*x;
            }
            Lanes::F32(v)
        }
        Lanes::I32(mut v) => {
            for x in v.iter_mut() {
                *x = if matches!(op, UnaryOp::BitNot) { !*x } else { x.wrapping_neg() };
            }
            Lanes::I32(v)
        }
        Lanes::I64(mut v) => {
            for x in v.iter_mut() {
                *x = if matches!(op, UnaryOp::BitNot) { !*x } else { x.wrapping_neg() };
            }
            Lanes::I64(v)
        }
        Lanes::U8(mut v) => {
            for x in v.iter_mut() {
                *x = if matches!(op, UnaryOp::BitNot) { !*x } else { x.wrapping_neg() };
            }
            Lanes::U8(v)
        }
    })
}

/// A comparison, producing all-ones for true and all-zeros for false
/// in a mask of the same lane width.
fn compare(op: BinOp, lhs: [u8; 16], rhs: [u8; 16], ty: VecTy) -> [u8; 16] {
    macro_rules! cmp {
        ($a:expr, $b:expr) => {{
            let (a, b) = ($a, $b);
            (0..a.len())
                .map(|k| match op {
                    BinOp::Eq => a[k] == b[k],
                    BinOp::Ne => a[k] != b[k],
                    BinOp::Lt => a[k] < b[k],
                    BinOp::Le => a[k] <= b[k],
                    BinOp::Gt => a[k] > b[k],
                    BinOp::Ge => a[k] >= b[k],
                    _ => false,
                })
                .collect::<Vec<bool>>()
        }};
    }
    let bits = match (decode(lhs, ty), decode(rhs, ty)) {
        (Lanes::F64(a), Lanes::F64(b)) => cmp!(a, b),
        (Lanes::F32(a), Lanes::F32(b)) => cmp!(a, b),
        (Lanes::I32(a), Lanes::I32(b)) => cmp!(a, b),
        (Lanes::I64(a), Lanes::I64(b)) => cmp!(a, b),
        (Lanes::U8(a), Lanes::U8(b)) => cmp!(a, b),
        _ => unreachable!("simd compare operands disagree on lane type"),
    };
    let width = ty.lane_bytes();
    let mut out = [0u8; 16];
    for (k, set) in bits.iter().enumerate() {
        if *set {
            for byte in out[k * width..(k + 1) * width].iter_mut() {
                *byte = 0xFF;
            }
        }
    }
    out
}

/// The display form: the type name applied to its lanes, e.g.
/// `f64x2(1.0, 2.0)` — the same shape a tuple struct prints in.
pub fn format(bytes: [u8; 16], ty: VecTy) -> String {
    let lanes: Vec<String> = match decode(bytes, ty) {
        Lanes::F64(v) => v.iter().map(|x| crate::heap::format_f64(*x)).collect(),
        Lanes::F32(v) => v.iter().map(|x| crate::heap::format_f32(*x)).collect(),
        Lanes::I32(v) => v.iter().map(|x| x.to_string()).collect(),
        Lanes::I64(v) => v.iter().map(|x| x.to_string()).collect(),
        Lanes::U8(v) => v.iter().map(|x| x.to_string()).collect(),
    };
    format!("{}({})", ty.source_name(), lanes.join(", "))
}
