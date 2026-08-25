use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;
use crate::object::Object;
use crate::value::Value;
use crate::error::InterpreterError;
use super::{EvaluationContext, EvaluationResult};

#[derive(Debug)]
pub(super) enum ArithmeticOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

/// Generate the per-width integer `apply_*` methods. Every width's
/// `wrapping_*` family in libcore has the same semantics (silent wrap
/// on overflow, trap on div-by-zero), so the bodies are identical —
/// only the operand type differs. This mirrors `object_unwrap_methods!`.
macro_rules! arithmetic_apply {
    ($(($method:ident, $ty:ty)),+ $(,)?) => {
        $(
            fn $method(&self, l: $ty, r: $ty) -> $ty {
                match self {
                    ArithmeticOp::Add => l.wrapping_add(r),
                    ArithmeticOp::Sub => l.wrapping_sub(r),
                    ArithmeticOp::Mul => l.wrapping_mul(r),
                    ArithmeticOp::Div => l.wrapping_div(r),
                    ArithmeticOp::Mod => l.wrapping_rem(r),
                }
            }
        )+
    };
}

/// Whether the operands are the one signed division that overflows:
/// the most negative value of the width divided by `-1`
/// (RUNTIME-TRAP). Mixed widths cannot occur — the type checker
/// requires both sides to share a type — so each arm checks one width.
fn is_signed_min_over_minus_one(lhs: &Value, rhs: &Value) -> bool {
    matches!(
        (lhs, rhs),
        (Value::Int64(i64::MIN), Value::Int64(-1))
            | (Value::Int32(i32::MIN), Value::Int32(-1))
            | (Value::Int16(i16::MIN), Value::Int16(-1))
            | (Value::Int8(i8::MIN), Value::Int8(-1))
    )
}

/// Whether `v` is an integer zero of any width (RUNTIME-TRAP's
/// divide-by-zero guard). `Float64(0.0)` is deliberately not a match —
/// dividing an f64 by zero is defined by IEEE-754.
fn is_integer_zero(v: &Value) -> bool {
    matches!(
        v,
        Value::Int64(0)
            | Value::UInt64(0)
            | Value::Int8(0)
            | Value::Int16(0)
            | Value::Int32(0)
            | Value::UInt8(0)
            | Value::UInt16(0)
            | Value::UInt32(0)
    )
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

    // Wrapping arithmetic so the interpreter agrees with the
    // compiler / JIT (cranelift's `iadd` / `isub` / `imul` wrap
    // on overflow). Rust's bare `+` would panic in debug mode
    // and wrap in release — we want a single deterministic
    // semantics across all build modes.
    //
    // Division and remainder by zero still trap (cranelift's
    // `sdiv` / `srem` trap, and Rust's `/` / `%` panic);
    // overflow on signed division (i64::MIN / -1) wraps.
    // Rust's `%` is truncated remainder, matching most C-family
    // languages — `(-7) % 3 == -1`. Diverges from mathematical
    // modulo, which is fine for our use cases.
    arithmetic_apply! {
        (apply_i64, i64),
        (apply_u64, u64),
        (apply_i32, i32),
        (apply_u32, u32),
        (apply_i16, i16),
        (apply_u16, u16),
        (apply_i8, i8),
        (apply_u8, u8),
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

/// Generate the per-width integer `apply_*` comparison methods. Each
/// width compares natively in its own range, so the bodies are
/// identical — only the operand type differs.
macro_rules! comparison_apply {
    ($(($method:ident, $ty:ty)),+ $(,)?) => {
        $(
            fn $method(&self, l: $ty, r: $ty) -> bool {
                match self {
                    ComparisonOp::Eq => l == r,
                    ComparisonOp::Ne => l != r,
                    ComparisonOp::Lt => l < r,
                    ComparisonOp::Le => l <= r,
                    ComparisonOp::Gt => l > r,
                    ComparisonOp::Ge => l >= r,
                }
            }
        )+
    };
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

    comparison_apply! {
        (apply_i64, i64),
        (apply_u64, u64),
        (apply_i32, i32),
        (apply_u32, u32),
        (apply_i16, i16),
        (apply_u16, u16),
        (apply_i8, i8),
        (apply_u8, u8),
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

    /// `site` is where the operator was written, so an arithmetic trap
    /// can point at it (LLM-LOOP P6-3). `None` when the caller has no
    /// expression to attribute it to.
    fn evaluate_arithmetic_op_v(
        &self,
        lhs: &Value,
        rhs: &Value,
        op: ArithmeticOp,
        site: Option<frontend::type_checker::SourceLocation>,
    ) -> Result<Value, InterpreterError> {
        // RUNTIME-TRAP: integer `/` and `%` by zero. `wrapping_div` /
        // `wrapping_rem` panic in the host on a zero divisor, which
        // surfaced as a Rust backtrace into this file rather than as a
        // toylang panic carrying the source line. Checked ahead of the
        // width dispatch so every integer width traps identically, and
        // matching the guard `compiler_lower` emits for the other three
        // backends. `f64` is excluded: IEEE division by zero yields an
        // infinity, which is a value rather than a fault.
        if matches!(op, ArithmeticOp::Div | ArithmeticOp::Mod) && is_integer_zero(rhs) {
            return Err(self.panic_error(
                "integer division by zero".to_string(),
                site,
            ));
        }
        // RUNTIME-TRAP: signed `MIN / -1` has no representable result.
        // `wrapping_div` answers `MIN` here, but cranelift's `sdiv`
        // faults, so the compiled binary died with SIGILL while this
        // engine carried on with a wrapped value. Both now stop.
        if matches!(op, ArithmeticOp::Div | ArithmeticOp::Mod)
            && is_signed_min_over_minus_one(lhs, rhs)
        {
            return Err(self.panic_error(
                "integer division overflowed (most negative value divided by -1)".to_string(),
                site,
            ));
        }
        Ok(match (lhs, rhs) {
            (Value::Int64(l), Value::Int64(r)) => Value::Int64(op.apply_i64(*l, *r)),
            (Value::UInt64(l), Value::UInt64(r)) => {
                // LLM-LOOP P6-3: `0u64 - 1u64` wrapping to
                // 18446744073709551615 is a favourite way to lose an
                // afternoon — the value looks like a plausible large
                // number, so the cause is nowhere near where the symptom
                // shows up. Routed through the panic path so it arrives
                // with the location and backtrace P6-1 added.
                if matches!(op, ArithmeticOp::Sub) && *l < *r {
                    return Err(self.panic_error(
                        format!("u64 subtraction underflowed: {l} - {r}"),
                        site,
                    ));
                }
                Value::UInt64(op.apply_u64(*l, *r))
            }
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

    pub fn evaluate_unary(&mut self, op: &UnaryOp, operand: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        use crate::try_value_v;
        let operand_result = self.evaluate(operand);
        let operand_v = try_value_v!(operand_result);

        // OP-OVERLOAD-EXTEND Phase 4 unary overload. `-x` / `~x`
        // / `!x` for a struct receiver dispatches to `neg` /
        // `bitnot` / `not` (each `fn (&self) -> Self`). Mirrors
        // the binary overload arm above.
        let unary_method_name: Option<&'static str> = match op {
            UnaryOp::Negate => Some("neg"),
            UnaryOp::BitwiseNot => Some("bitnot"),
            UnaryOp::LogicalNot => Some("not"),
            UnaryOp::Borrow | UnaryOp::BorrowMut => None,
        };
        if let Some(method_name) = unary_method_name {
            if let Value::Heap(operand_rc) = &operand_v {
                let struct_info = {
                    let obj = operand_rc.borrow();
                    if let Object::Struct { type_name, type_args, .. } = &*obj {
                        Some((*type_name, type_args.clone()))
                    } else {
                        None
                    }
                };
                if let Some((struct_name, type_args)) = struct_info {
                    if let Some(method_sym) = self.string_interner.get(method_name) {
                        if let Some(method) =
                            self.get_method(struct_name, method_sym, &type_args)
                        {
                            let result = self.call_method(
                                method,
                                operand_rc.clone(),
                                vec![],
                            )?;
                            let result_v = match result {
                                EvaluationResult::Value(v) => v,
                                _ => return Err(InterpreterError::InternalError(
                                    format!(
                                        "unary overload: {} returned non-value",
                                        method_name
                                    )
                                )),
                            };
                            return Ok(EvaluationResult::Value(result_v));
                        }
                    }
                }
            }
        }

        let result_v = match op {
            UnaryOp::BitwiseNot => match &operand_v {
                Value::UInt64(v) => Value::UInt64(!*v),
                Value::Int64(v) => Value::Int64(!*v),
                // NUM-W: the narrow widths complement at their own
                // width, so `~0u8` is `255u8` and not a widened
                // `18446744073709551615`.
                Value::UInt32(v) => Value::UInt32(!*v),
                Value::UInt16(v) => Value::UInt16(!*v),
                Value::UInt8(v) => Value::UInt8(!*v),
                Value::Int32(v) => Value::Int32(!*v),
                Value::Int16(v) => Value::Int16(!*v),
                Value::Int8(v) => Value::Int8(!*v),
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
            // `wrapping_neg` mirrors the type checker: signed widths and
            // f64 only, and the wrapping form avoids panics on
            // `-i64::MIN` (and `-i8::MIN`, and so on down).
            UnaryOp::Negate => match &operand_v {
                Value::Int64(v) => Value::Int64(v.wrapping_neg()),
                Value::Int32(v) => Value::Int32(v.wrapping_neg()),
                Value::Int16(v) => Value::Int16(v.wrapping_neg()),
                Value::Int8(v) => Value::Int8(v.wrapping_neg()),
                Value::Float64(v) => Value::Float64(-*v),
                _ => return Err(InterpreterError::TypeError {
                    expected: TypeDecl::Int64,
                    found: operand_v.get_type(),
                    message: format!("Unary minus requires i64 or f64, got {:?}", operand_v),
                }),
            },
            // REF-Stage-2: explicit borrow expressions are erased at
            // runtime — there is no `Ref` `Value` variant. The type
            // checker enforces caller / callee distinction; the
            // interpreter just passes the value through.
            UnaryOp::Borrow | UnaryOp::BorrowMut => operand_v,
        };

        Ok(EvaluationResult::Value(result_v))
    }

    pub fn evaluate_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Where to point an arithmetic trap (LLM-LOOP P6-3).
        let site = self.expr_location(lhs);
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

        // Phase B operator overload: `s == t` / `s != t` between
        // two struct values of the same nominal type dispatches to
        // the user-defined `eq(&self, other: &Self) -> bool`
        // method on the struct. The frontend type checker ensures
        // the receiver / argument shape matches before reaching
        // here, so we only need to look up `eq` and forward the
        // call. `!=` negates the boolean result. Falls through to
        // the regular comparison path when no `eq` method is
        // registered (the type checker's allow-list also catches
        // the unsupported case before this point, but the
        // defensive fall-through keeps non-overloaded comparisons
        // (Allocator identity, etc.) routed through their existing
        // arms).
        // Phase B operator overload + arithmetic continuation:
        // `s OP t` between two struct values of the same nominal
        // type dispatches to the user-defined method (`eq` for
        // `==` / `!=`; `add` / `sub` / `mul` / `div` / `rem` for
        // `+` / `-` / `*` / `/` / `%`). The frontend type
        // checker has already vetted the receiver / argument
        // shape; we only need to look up the method and forward
        // the call. `!=` negates the boolean result. Falls
        // through to the regular arithmetic / comparison path
        // when no matching method is registered (the type
        // checker's allow-list catches the unsupported cases
        // before this point — the defensive fall-through keeps
        // primitive operators routed through their existing
        // arms).
        let overload_method_name: Option<&'static str> = match op {
            Operator::EQ | Operator::NE => Some("eq"),
            Operator::LT => Some("lt"),
            Operator::LE => Some("le"),
            Operator::GT => Some("gt"),
            Operator::GE => Some("ge"),
            Operator::IAdd => Some("add"),
            Operator::ISub => Some("sub"),
            Operator::IMul => Some("mul"),
            Operator::IDiv => Some("div"),
            Operator::IMod => Some("rem"),
            Operator::BitwiseAnd => Some("bitand"),
            Operator::BitwiseOr => Some("bitor"),
            Operator::BitwiseXor => Some("bitxor"),
            Operator::LeftShift => Some("shl"),
            Operator::RightShift => Some("shr"),
            _ => None,
        };
        if let Some(method_name) = overload_method_name {
            if let (Value::Heap(lhs_rc), Value::Heap(rhs_rc)) = (&lhs_v, &rhs_v) {
                let struct_info = {
                    let lhs_obj = lhs_rc.borrow();
                    let rhs_obj = rhs_rc.borrow();
                    match (&*lhs_obj, &*rhs_obj) {
                        (Object::Struct { type_name: ln, type_args: la, .. },
                         Object::Struct { type_name: rn, type_args: ra, .. })
                            if ln == rn && la == ra => Some((*ln, la.clone())),
                        _ => None,
                    }
                };
                if let Some((struct_name, type_args)) = struct_info {
                    if let Some(method_sym) = self.string_interner.get(method_name) {
                        if let Some(method) =
                            self.get_method(struct_name, method_sym, &type_args)
                        {
                            let result = self.call_method(
                                method,
                                lhs_rc.clone(),
                                vec![rhs_rc.clone()],
                            )?;
                            let result_v = match result {
                                EvaluationResult::Value(v) => v,
                                _ => return Err(InterpreterError::InternalError(
                                    format!(
                                        "operator overload: {} returned non-value",
                                        method_name
                                    )
                                )),
                            };
                            // For `==` / `!=` / `<` / `<=` / `>`
                            // / `>=` the bool result is (optionally)
                            // negated and returned directly. For
                            // arithmetic operators the method returns
                            // Self (the same struct) which we forward
                            // through the existing Value pipeline.
                            if matches!(op,
                                Operator::EQ | Operator::NE
                                | Operator::LT | Operator::LE
                                | Operator::GT | Operator::GE
                            ) {
                                let bool_result = match result_v {
                                    Value::Bool(b) => b,
                                    other => return Err(InterpreterError::InternalError(
                                        format!(
                                            "operator overload: eq returned {:?}, expected bool",
                                            other
                                        ),
                                    )),
                                };
                                // `!=` is the only operator that
                                // routes through `eq` and inverts.
                                // The ordering operators have their
                                // own dedicated methods (`lt` / `le`
                                // / `gt` / `ge`) and don't need a
                                // negation step.
                                let final_bool = if matches!(op, Operator::NE) {
                                    !bool_result
                                } else {
                                    bool_result
                                };
                                return Ok(EvaluationResult::Value(Value::Bool(final_bool)));
                            } else {
                                return Ok(EvaluationResult::Value(result_v));
                            }
                        }
                    }
                }
            }
        }

        let result_v = match op {
            Operator::IAdd => self.evaluate_arithmetic_op_v(&lhs_v, &rhs_v, ArithmeticOp::Add, site)?,
            Operator::ISub => self.evaluate_arithmetic_op_v(&lhs_v, &rhs_v, ArithmeticOp::Sub, site)?,
            Operator::IMul => self.evaluate_arithmetic_op_v(&lhs_v, &rhs_v, ArithmeticOp::Mul, site)?,
            Operator::IDiv => self.evaluate_arithmetic_op_v(&lhs_v, &rhs_v, ArithmeticOp::Div, site)?,
            Operator::IMod => self.evaluate_arithmetic_op_v(&lhs_v, &rhs_v, ArithmeticOp::Mod, site)?,
            Operator::EQ => self.evaluate_comparison_op_v(&lhs_v, &rhs_v, ComparisonOp::Eq)?,
            Operator::NE => self.evaluate_comparison_op_v(&lhs_v, &rhs_v, ComparisonOp::Ne)?,
            Operator::LT => self.evaluate_comparison_op_v(&lhs_v, &rhs_v, ComparisonOp::Lt)?,
            Operator::LE => self.evaluate_comparison_op_v(&lhs_v, &rhs_v, ComparisonOp::Le)?,
            Operator::GT => self.evaluate_comparison_op_v(&lhs_v, &rhs_v, ComparisonOp::Gt)?,
            Operator::GE => self.evaluate_comparison_op_v(&lhs_v, &rhs_v, ComparisonOp::Ge)?,
            Operator::BitwiseAnd => self.evaluate_bitwise_v(&lhs_v, &rhs_v, "Bitwise AND", |l, r| l & r, |l, r| l & r)?,
            Operator::BitwiseOr => self.evaluate_bitwise_v(&lhs_v, &rhs_v, "Bitwise OR", |l, r| l | r, |l, r| l | r)?,
            Operator::BitwiseXor => self.evaluate_bitwise_v(&lhs_v, &rhs_v, "Bitwise XOR", |l, r| l ^ r, |l, r| l ^ r)?,
            Operator::LeftShift => self.evaluate_shift_v(&lhs_v, &rhs_v, "Left shift", |l, r| l.wrapping_shl(r), |l, r| l.wrapping_shl(r))?,
            Operator::RightShift => self.evaluate_shift_v(&lhs_v, &rhs_v, "Right shift", |l, r| l.wrapping_shr(r), |l, r| l.wrapping_shr(r))?,
            Operator::LogicalAnd | Operator::LogicalOr => unreachable!("Should be handled above"),
        };

        Ok(EvaluationResult::Value(result_v))
    }

    // Bitwise operations — Value-flavoured fast paths used by
    // `evaluate_binary`. The three ops differ only in the applied
    // operator, so a single helper parameterised by the op name keeps
    // the error message and the integer-type match in one place.
    fn evaluate_bitwise_v(
        &self,
        lhs: &Value,
        rhs: &Value,
        op_name: &str,
        apply_u64: fn(u64, u64) -> u64,
        apply_i64: fn(i64, i64) -> i64,
    ) -> Result<Value, InterpreterError> {
        match (lhs, rhs) {
            (Value::UInt64(l), Value::UInt64(r)) => Ok(Value::UInt64(apply_u64(*l, *r))),
            (Value::Int64(l), Value::Int64(r)) => Ok(Value::Int64(apply_i64(*l, *r))),
            _ => Err(InterpreterError::TypeError {
                expected: lhs.get_type(),
                found: rhs.get_type(),
                message: format!("{op_name} requires same integer types, got {:?} and {:?}", lhs, rhs),
            }),
        }
    }

    // Shifts share the "rhs must be a UInt64 shift amount, then
    // operate on the lhs's integer type" shape; only the direction
    // of the `wrapping_sh*` call differs.
    fn evaluate_shift_v(
        &self,
        lhs: &Value,
        rhs: &Value,
        op_name: &str,
        apply_u64: fn(u64, u32) -> u64,
        apply_i64: fn(i64, u32) -> i64,
    ) -> Result<Value, InterpreterError> {
        let shift_amount = match rhs {
            Value::UInt64(r) => *r,
            _ => return Err(InterpreterError::TypeError {
                expected: TypeDecl::UInt64,
                found: rhs.get_type(),
                message: format!("Shift amount must be UInt64, got {:?}", rhs),
            }),
        };
        match lhs {
            Value::UInt64(l) => Ok(Value::UInt64(apply_u64(*l, shift_amount as u32))),
            Value::Int64(l) => Ok(Value::Int64(apply_i64(*l, shift_amount as u32))),
            _ => Err(InterpreterError::TypeError {
                expected: TypeDecl::UInt64,
                found: lhs.get_type(),
                message: format!("{op_name} requires integer type on left side, got {:?}", lhs),
            }),
        }
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
