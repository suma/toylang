//! SIMD intrinsic type checking (SIMD.md Phase 2).
//!
//! Lane-wise arithmetic and comparison are *not* here — they are
//! ordinary binary operators over `TypeDecl::Vector`, checked in
//! `expression.rs`. This module covers the sixteen `__simd_*`
//! intrinsics: the operations a type and an operator cannot express.
//!
//! Two rules shape the signatures:
//!
//! * `__simd_splat` / `__simd_load` have no lane-type suffix, so
//!   their result comes from the call site's annotation, exactly as
//!   `__builtin_ptr_read` takes its element type from `val v: T = ...`.
//!   Without an annotation there is no answer, and saying so is
//!   better than guessing a lane type.
//! * A lane index (`__simd_extract` / `__simd_insert`) must be a
//!   literal in range. A runtime lane index would need either a
//!   memory round-trip or a jump table per lane; a compile-time error
//!   is the honest answer and matches what the hardware offers.

use crate::ast::*;
use crate::type_checker::TypeCheckerVisitor;
use crate::type_checker::error::TypeCheckError;
use crate::type_decl::{TypeDecl, VectorType};

impl<'a> TypeCheckerVisitor<'a> {
    /// Stamp the result type of `__simd_splat` / `__simd_load` into
    /// the call itself, from whatever context names it.
    ///
    /// Neither intrinsic carries a lane-type suffix, so the call
    /// alone does not say what it produces — and four backends would
    /// otherwise each need their own channel from an annotation to a
    /// builtin call. Appending the type as a synthetic `u64` argument
    /// means every backend reads it off the argument list it already
    /// walks. (`__builtin_ptr_read` solves the same problem by
    /// special-casing the let-binding in each lowering; doing it once
    /// in the AST is cheaper than doing it four times.)
    ///
    /// The context is the type checker's own hint, so all of
    /// `val v: f64x2 = __simd_load(p, i)`, `a * __simd_splat(x)`, and
    /// an argument position whose parameter is a vector work. Where
    /// no hint reaches, `check_simd_call` reports the missing
    /// annotation.
    /// Returns whether the node was rewritten, so the caller can
    /// re-read it.
    pub(crate) fn stamp_simd_call(&mut self, expr: &ExprRef, expr_obj: &Expr) -> bool {
        let Expr::BuiltinCall(BuiltinFunction::Simd(op), args) = expr_obj else {
            return false;
        };
        if !op.needs_result_annotation() || args.len() != op.arity() {
            return false;
        }
        let Some(TypeDecl::Vector(vec_ty)) = self.type_inference.type_hint else {
            return false;
        };
        let mut stamped = args.clone();
        stamped.push(self.core.expr_pool.add(Expr::UInt64(vec_ty.code())));
        self.core
            .expr_pool
            .update(expr, Expr::BuiltinCall(BuiltinFunction::Simd(*op), stamped));
        true
    }


    /// `__simd_shuffle(a, b, [k...])`: two vectors of one type and a
    /// constant mask with one index per lane, each selecting a lane
    /// of `a` followed by `b`.
    ///
    /// Everything about the mask is decided here, because this is
    /// the first point where both the mask and the lane count are
    /// known. An out-of-range index is an error rather than a zero
    /// lane: the mask is a constant, so the compiler can say so, and
    /// `__simd_swizzle` is the intrinsic for indices that are not.
    fn check_simd_shuffle(&mut self, args: &[ExprRef]) -> Result<TypeDecl, TypeCheckError> {
        let name = SimdOp::Shuffle.builtin_name();
        // Four arguments means `stamp_simd_shuffle_mask` accepted the
        // mask; three means it could not read it, and the operands
        // are still worth checking so the report names the lane count.
        let mask = if args.len() == SimdOp::Shuffle.consumed_args() {
            let lo = self.const_lane_index(&args[2]);
            let hi = self.const_lane_index(&args[3]);
            match (lo, hi) {
                (Some(lo), Some(hi)) => Some(SimdOp::unpack_shuffle_mask(lo, hi)),
                _ => None,
            }
        } else {
            None
        };

        let a_ty = self.simd_vector_arg(&args[0], name, "the `a` vector")?;
        let b_ty = self.simd_vector_arg(&args[1], name, "the `b` vector")?;
        if a_ty != b_ty {
            return Err(self.error_with_location(
                TypeCheckError::generic_error(&format!(
                    "{name} permutes two vectors of the same type, but got `{}` and `{}`",
                    a_ty.source_name(),
                    b_ty.source_name()
                )),
                &args[1],
            ));
        }

        let lanes = a_ty.lanes();
        let Some(mask) = mask else {
            return Err(self.error_with_location(
                TypeCheckError::generic_error(&format!(
                    "{name} takes its mask as an array literal of integer literals —                      the permutation is part of the instruction, not a value it reads.                      `{}` has {lanes} lanes, so write {lanes} indices in `0..{}`, where                      an index below {lanes} selects that lane of `a` and one at or above                      it selects lane `k - {lanes}` of `b`; for indices computed at run                      time use `__simd_swizzle`",
                    a_ty.source_name(),
                    lanes * 2
                )),
                &args[2],
            ));
        };
        if mask.len() != lanes {
            return Err(self.error_with_location(
                TypeCheckError::generic_error(&format!(
                    "{name} needs one index per lane: `{}` has {lanes}, but the mask                      has {}",
                    a_ty.source_name(),
                    mask.len()
                )),
                &args[2],
            ));
        }
        for index in &mask {
            if (*index as usize) >= lanes * 2 {
                return Err(self.error_with_location(
                    TypeCheckError::generic_error(&format!(
                        "index {index} is out of range for {name} on `{}`: the mask                          selects from `a` then `b`, so an index has to be in `0..{}`",
                        a_ty.source_name(),
                        lanes * 2
                    )),
                    &args[2],
                ));
            }
        }
        Ok(TypeDecl::Vector(a_ty))
    }

    /// Type-check one `__simd_*` call and report its result type.
    pub(crate) fn check_simd_call(
        &mut self,
        op: SimdOp,
        args: &[ExprRef],
    ) -> Result<TypeDecl, TypeCheckError> {
        let name = op.builtin_name();
        // `__simd_shuffle`'s stamp *replaces* an argument instead of
        // following one, so it does not fit the prologue below.
        if matches!(op, SimdOp::Shuffle) {
            return self.check_simd_shuffle(args);
        }
        // `stamp_simd_result_types` appends the result type to
        // `__simd_splat` / `__simd_load` when the call site annotated
        // it, so those two carry one more argument than the user
        // wrote. Read it back here and check the rest normally.
        let mut result_ty = None;
        let mut args = args;
        if op.needs_result_annotation() && args.len() == op.arity() + 1 {
            result_ty = self
                .const_lane_index(&args[op.arity()])
                .and_then(VectorType::from_code);
            args = &args[..op.arity()];
        }
        if args.len() != op.arity() {
            return Err(TypeCheckError::generic_error(&format!(
                "{name} expects {} argument(s), got {}",
                op.arity(),
                args.len()
            )));
        }

        match op {
            SimdOp::Splat => {
                let vec_ty = Self::simd_result_annotation(op, result_ty)?;
                let lane = vec_ty.lane();
                self.expect_simd_arg(&args[0], &lane, name, "the lane value")?;
                Ok(TypeDecl::Vector(vec_ty))
            }
            SimdOp::Load => {
                let vec_ty = Self::simd_result_annotation(op, result_ty)?;
                self.expect_simd_arg(&args[0], &TypeDecl::Ptr, name, "the base pointer")?;
                self.expect_simd_arg(&args[1], &TypeDecl::UInt64, name, "the element index")?;
                Ok(TypeDecl::Vector(vec_ty))
            }
            SimdOp::Store => {
                self.expect_simd_arg(&args[0], &TypeDecl::Ptr, name, "the base pointer")?;
                self.expect_simd_arg(&args[1], &TypeDecl::UInt64, name, "the element index")?;
                self.simd_vector_arg(&args[2], name, "the value")?;
                Ok(TypeDecl::Unit)
            }
            SimdOp::Extract => {
                let vec_ty = self.simd_vector_arg(&args[0], name, "the vector")?;
                self.expect_const_lane_index(&args[1], vec_ty, name)?;
                Ok(vec_ty.lane())
            }
            SimdOp::Insert => {
                let vec_ty = self.simd_vector_arg(&args[0], name, "the vector")?;
                self.expect_const_lane_index(&args[1], vec_ty, name)?;
                let lane = vec_ty.lane();
                self.expect_simd_arg(&args[2], &lane, name, "the replacement lane")?;
                Ok(TypeDecl::Vector(vec_ty))
            }
            SimdOp::Select => {
                let value_ty = self.simd_vector_arg(&args[1], name, "the `a` vector")?;
                let mask_ty = self.simd_vector_arg(&args[0], name, "the mask")?;
                if mask_ty != value_ty.mask() {
                    return Err(self.error_with_location(
                        TypeCheckError::generic_error(&format!(
                            "{name} takes the mask a `{}` comparison produces, which is \
                             `{}`, but this mask is `{}`",
                            value_ty.source_name(),
                            value_ty.mask().source_name(),
                            mask_ty.source_name()
                        )),
                        &args[0],
                    ));
                }
                let b_ty = self.simd_vector_arg(&args[2], name, "the `b` vector")?;
                if b_ty != value_ty {
                    return Err(self.error_with_location(
                        TypeCheckError::generic_error(&format!(
                            "{name} chooses between two vectors of the same type, but \
                             got `{}` and `{}`",
                            value_ty.source_name(),
                            b_ty.source_name()
                        )),
                        &args[2],
                    ));
                }
                Ok(TypeDecl::Vector(value_ty))
            }
            SimdOp::ReduceAdd
            | SimdOp::ReduceMin
            | SimdOp::ReduceMax
            | SimdOp::ReduceAnd
            | SimdOp::ReduceOr => {
                let vec_ty = self.simd_vector_arg(&args[0], name, "the vector")?;
                if op.integer_lanes_only() && vec_ty.is_float() {
                    return Err(self.error_with_location(
                        TypeCheckError::generic_error(&format!(
                            "{name} folds integer lanes, but `{}` has float lanes",
                            vec_ty.source_name()
                        )),
                        &args[0],
                    ));
                }
                Ok(vec_ty.lane())
            }
            SimdOp::Any | SimdOp::All => {
                self.simd_vector_arg(&args[0], name, "the mask")?;
                Ok(TypeDecl::Bool)
            }
            // `u64` regardless of lane count, so a caller can hand
            // the result to `Bits::trailing_zeros` without knowing
            // which vector produced it. Bits at or above the lane
            // count are always zero.
            SimdOp::Bitmask => {
                self.simd_vector_arg(&args[0], name, "the mask")?;
                Ok(TypeDecl::UInt64)
            }
            // Byte lanes on both sides: `pshufb` and `tbl` index
            // bytes, and a wider-lane swizzle would have to be
            // synthesised differently on each ISA. `__simd_bitcast`
            // is the way in from another lane type.
            SimdOp::Swizzle => {
                let a_ty = self.simd_vector_arg(&args[0], name, "the table")?;
                let idx_ty = self.simd_vector_arg(&args[1], name, "the indices")?;
                for (ty, arg, role) in
                    [(a_ty, &args[0], "the table"), (idx_ty, &args[1], "the indices")]
                {
                    if ty != VectorType::U8x16 {
                        return Err(self.error_with_location(
                            TypeCheckError::generic_error(&format!(
                                "{name} indexes bytes, so {role} has to be `u8x16`,                                  got `{}`; reinterpret it with `__simd_bitcast` first",
                                ty.source_name()
                            )),
                            arg,
                        ));
                    }
                }
                Ok(TypeDecl::Vector(VectorType::U8x16))
            }
            // Every vector is 128 bits wide, so any pair of vector
            // types is a legal reinterpretation and there is nothing
            // to check beyond "the argument is a vector".
            SimdOp::Bitcast => {
                let vec_ty = Self::simd_result_annotation(op, result_ty)?;
                self.simd_vector_arg(&args[0], name, "the vector")?;
                Ok(TypeDecl::Vector(vec_ty))
            }
            // Handled before the prologue above: its stamp replaces
            // an argument rather than following one.
            SimdOp::Shuffle => self.check_simd_shuffle(args),
        }
    }

    /// Result type of a lane-wise binary operator, or `None` when
    /// neither operand is a vector and the ordinary scalar rules
    /// apply.
    ///
    /// Three rules, all from SIMD.md's "意味論" section:
    ///
    /// * Integer lanes **wrap** and never trap. `u64 -` panics on
    ///   underflow and `/` panics on zero for scalars (RUNTIME-TRAP),
    ///   but a per-lane guard would defeat the point of vectorising,
    ///   so integer `/` and `%` are rejected outright rather than
    ///   silently given a different failure mode. Float `/` is fine —
    ///   IEEE division does not trap.
    /// * A comparison produces a **mask**, not a `bool`: `a == b` is
    ///   sixteen answers, not one. `__simd_all(a == b)` is how a
    ///   program asks the whole-vector question.
    /// * Bitwise operators need integer lanes; shifts take a vector
    ///   on both sides (lane-wise shift amounts).
    pub(crate) fn simd_binary_result(
        &mut self,
        op: &Operator,
        lhs: &ExprRef,
        l: &TypeDecl,
        r: &TypeDecl,
    ) -> Result<Option<TypeDecl>, TypeCheckError> {
        // A shift takes a **scalar** amount, not a vector of them:
        // that is the shape every SIMD shift instruction has, and
        // cranelift's `ishl` / `ushr` / `sshr` on a vector expect a
        // single integer. Every lane shifts by the same amount.
        if matches!(op, Operator::LeftShift | Operator::RightShift)
            && let TypeDecl::Vector(lv) = l
        {
            let lv = *lv;
            if lv.is_float() {
                return Err(self.error_with_location(
                    TypeCheckError::generic_error(&format!(
                        "`{}` needs integer lanes, but `{}` has float lanes",
                        Self::simd_op_spelling(op),
                        lv.source_name()
                    )),
                    lhs,
                ));
            }
            if !matches!(r, TypeDecl::UInt64 | TypeDecl::Number) {
                return Err(self.error_with_location(
                    TypeCheckError::generic_error(&format!(
                        "lane-wise `{}` shifts every lane by the same `u64` amount, \
                         got `{}` on the right",
                        Self::simd_op_spelling(op),
                        self.format_type_for_error(r)
                    )),
                    lhs,
                ));
            }
            return Ok(Some(TypeDecl::Vector(lv)));
        }

        let (lv, rv) = match (l, r) {
            (TypeDecl::Vector(lv), TypeDecl::Vector(rv)) => (*lv, *rv),
            (TypeDecl::Vector(lv), other) => {
                return Err(self.error_with_location(
                    TypeCheckError::generic_error(&format!(
                        "lane-wise `{}` needs a `{}` on both sides, got `{}`; \
                         broadcast the scalar with `__simd_splat` first",
                        Self::simd_op_spelling(op),
                        lv.source_name(),
                        self.format_type_for_error(other)
                    )),
                    lhs,
                ));
            }
            (other, TypeDecl::Vector(rv)) => {
                return Err(self.error_with_location(
                    TypeCheckError::generic_error(&format!(
                        "lane-wise `{}` needs a `{}` on both sides, got `{}`; \
                         broadcast the scalar with `__simd_splat` first",
                        Self::simd_op_spelling(op),
                        rv.source_name(),
                        self.format_type_for_error(other)
                    )),
                    lhs,
                ));
            }
            _ => return Ok(None),
        };

        if lv != rv {
            return Err(self.error_with_location(
                TypeCheckError::generic_error(&format!(
                    "lane-wise `{}` needs both sides to have the same lane type, \
                     got `{}` and `{}`",
                    Self::simd_op_spelling(op),
                    lv.source_name(),
                    rv.source_name()
                )),
                lhs,
            ));
        }

        let result = match op {
            Operator::IAdd | Operator::ISub | Operator::IMul => TypeDecl::Vector(lv),
            Operator::IDiv if lv.is_float() => TypeDecl::Vector(lv),
            Operator::IDiv | Operator::IMod => {
                return Err(self.error_with_location(
                    TypeCheckError::generic_error(&format!(
                        "`{}` is not defined on `{}`: a scalar integer divide \
                         panics on zero (RUNTIME-TRAP), and checking sixteen lanes \
                         would cost more than the vectorisation saves; divide by a \
                         reciprocal computed with float lanes, or fall back to a \
                         scalar loop",
                        Self::simd_op_spelling(op),
                        lv.source_name()
                    )),
                    lhs,
                ));
            }
            Operator::LE | Operator::LT | Operator::GE | Operator::GT
            | Operator::EQ | Operator::NE => TypeDecl::Vector(lv.mask()),
            Operator::BitwiseAnd | Operator::BitwiseOr | Operator::BitwiseXor
            // Shifts are handled above (scalar amount), so reaching
            // here means both sides were vectors.
            | Operator::LeftShift | Operator::RightShift => {
                if lv.is_float() {
                    return Err(self.error_with_location(
                        TypeCheckError::generic_error(&format!(
                            "`{}` needs integer lanes, but `{}` has float lanes",
                            Self::simd_op_spelling(op),
                            lv.source_name()
                        )),
                        lhs,
                    ));
                }
                TypeDecl::Vector(lv)
            }
            Operator::LogicalAnd | Operator::LogicalOr => {
                return Err(self.error_with_location(
                    TypeCheckError::generic_error(&format!(
                        "`{}` short-circuits, which has no lane-wise meaning; use \
                         the bitwise `{}` on masks instead",
                        Self::simd_op_spelling(op),
                        if matches!(op, Operator::LogicalAnd) { "&" } else { "|" }
                    )),
                    lhs,
                ));
            }
        };
        Ok(Some(result))
    }

    /// Result type of a lane-wise unary operator, or `None` when the
    /// operand is not a vector.
    pub(crate) fn simd_unary_result(
        &mut self,
        op: &UnaryOp,
        operand: &ExprRef,
        ty: &TypeDecl,
    ) -> Result<Option<TypeDecl>, TypeCheckError> {
        let TypeDecl::Vector(v) = ty else {
            return Ok(None);
        };
        let v = *v;
        match op {
            // Negating unsigned lanes is the same wrap the scalar
            // narrow ints already do, but spelling `-u8x16` is much
            // more likely to be a mistake than an intent.
            UnaryOp::Negate if matches!(v, VectorType::U8x16) => Err(self.error_with_location(
                TypeCheckError::generic_error(
                    "`-` needs signed or float lanes, but `u8x16` lanes are unsigned",
                ),
                operand,
            )),
            UnaryOp::Negate => Ok(Some(TypeDecl::Vector(v))),
            UnaryOp::BitwiseNot if v.is_float() => Err(self.error_with_location(
                TypeCheckError::generic_error(&format!(
                    "`~` needs integer lanes, but `{}` has float lanes",
                    v.source_name()
                )),
                operand,
            )),
            UnaryOp::BitwiseNot => Ok(Some(TypeDecl::Vector(v))),
            UnaryOp::LogicalNot => Err(self.error_with_location(
                TypeCheckError::generic_error(&format!(
                    "`!` takes a `bool`, and `{}` is {} lanes; invert a mask with `~`",
                    v.source_name(),
                    v.lanes()
                )),
                operand,
            )),
            UnaryOp::Borrow | UnaryOp::BorrowMut => Ok(None),
        }
    }

    /// How an operator is spelled in a diagnostic.
    fn simd_op_spelling(op: &Operator) -> &'static str {
        match op {
            Operator::IAdd => "+",
            Operator::ISub => "-",
            Operator::IMul => "*",
            Operator::IDiv => "/",
            Operator::IMod => "%",
            Operator::LT => "<",
            Operator::LE => "<=",
            Operator::GT => ">",
            Operator::GE => ">=",
            Operator::EQ => "==",
            Operator::NE => "!=",
            Operator::BitwiseAnd => "&",
            Operator::BitwiseOr => "|",
            Operator::BitwiseXor => "^",
            Operator::LeftShift => "<<",
            Operator::RightShift => ">>",
            Operator::LogicalAnd => "&&",
            Operator::LogicalOr => "||",
        }
    }

    /// The vector type `__simd_splat` / `__simd_load` produce, as
    /// stamped from the annotation at the call site.
    fn simd_result_annotation(
        op: SimdOp,
        stamped: Option<VectorType>,
    ) -> Result<VectorType, TypeCheckError> {
        stamped.ok_or_else(|| {
            let name = op.builtin_name();
            TypeCheckError::generic_error(&format!(
                "{name} has no lane-type suffix, so it takes its type from a \
                 `val` / `var` annotation naming it: write \
                 `val v: f64x2 = {name}(...)` (or one of `f32x4` / `i32x4` / \
                 `i64x2` / `u8x16`). A call nested inside a larger expression \
                 has no annotation to read, so bind it first."
            ))
        })
    }

    /// Check one argument against an expected scalar type, letting a
    /// suffix-less numeric literal take the expected type the way
    /// every other argument position does.
    fn expect_simd_arg(
        &mut self,
        arg: &ExprRef,
        expected: &TypeDecl,
        name: &str,
        role: &str,
    ) -> Result<(), TypeCheckError> {
        let saved = self.type_inference.type_hint.clone();
        self.type_inference.type_hint = Some(expected.clone());
        let actual = self.visit_expr(arg);
        self.type_inference.type_hint = saved;
        let actual = actual?;
        if actual == *expected || actual == TypeDecl::Number {
            return Ok(());
        }
        Err(self.error_with_location(
            TypeCheckError::generic_error(&format!(
                "{name} expects `{}` for {role}, got `{}`",
                self.format_type_for_error(expected),
                self.format_type_for_error(&actual)
            )),
            arg,
        ))
    }

    /// Check one argument is a vector and report which one.
    fn simd_vector_arg(
        &mut self,
        arg: &ExprRef,
        name: &str,
        role: &str,
    ) -> Result<VectorType, TypeCheckError> {
        // A vector argument names its own type, so an outer
        // annotation (`val v: f64x2 = ...`) must not leak in and
        // silently retype a mask.
        let saved = self.type_inference.type_hint.take();
        let actual = self.visit_expr(arg);
        self.type_inference.type_hint = saved;
        match actual? {
            TypeDecl::Vector(v) => Ok(v),
            other => Err(self.error_with_location(
                TypeCheckError::generic_error(&format!(
                    "{name} expects a vector for {role}, got `{}`",
                    self.format_type_for_error(&other)
                )),
                arg,
            )),
        }
    }

    /// A lane index has to be a literal the compiler can read, and it
    /// has to be in range for the vector it indexes.
    fn expect_const_lane_index(
        &mut self,
        arg: &ExprRef,
        vec_ty: VectorType,
        name: &str,
    ) -> Result<(), TypeCheckError> {
        let index = self.const_lane_index(arg);
        match index {
            Some(k) if (k as usize) < vec_ty.lanes() => Ok(()),
            Some(k) => Err(self.error_with_location(
                TypeCheckError::generic_error(&format!(
                    "lane {k} is out of range for `{}`, which has {} lanes",
                    vec_ty.source_name(),
                    vec_ty.lanes()
                )),
                arg,
            )),
            None => Err(self.error_with_location(
                TypeCheckError::generic_error(&format!(
                    "{name} needs a literal lane index (the lane is part of the \
                     instruction, not a value it reads); to select a lane computed \
                     at run time, store the vector with `__simd_store` and read the \
                     element back"
                )),
                arg,
            )),
        }
    }

    /// Read a lane index written as an integer literal. Any other
    /// expression — including a `const` identifier — is rejected;
    /// folding those would mean running CTFE from inside the type
    /// checker, which happens later in the pipeline.
    pub(crate) fn const_lane_index(&self, arg: &ExprRef) -> Option<u64> {
        match self.core.expr_pool.get(arg)? {
            Expr::UInt64(v) => Some(v),
            Expr::Int64(v) if v >= 0 => Some(v as u64),
            Expr::UInt32(v) => Some(v as u64),
            Expr::UInt16(v) => Some(v as u64),
            Expr::UInt8(v) => Some(v as u64),
            Expr::Int32(v) if v >= 0 => Some(v as u64),
            Expr::Number(sym) => self
                .core
                .string_interner
                .resolve(sym)
                .and_then(|s| s.parse::<u64>().ok()),
            _ => None,
        }
    }
}
