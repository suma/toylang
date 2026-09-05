//! Operator and conditional expression lowering.
//!
//! Covers the four "pure expression form" lowerings that don't
//! involve compound storage:
//!
//! - `lower_binary`: arithmetic / comparison / bitwise / shift
//!   binary operators. Type-checks operands, emits the
//!   corresponding `BinOp` instruction. Logical `&&` / `||`
//!   delegate to `lower_short_circuit`.
//! - `lower_short_circuit`: emits a brif chain so the rhs is
//!   only evaluated when the lhs result requires it (D-style
//!   short-circuit semantics).
//! - `lower_unary`: prefix `!` / `-` / `~`. Emits the matching
//!   `UnaryOp` instruction with the operand's IR type.
//! - `lower_if_chain`: lowers an `if cond { then } elif ...
//!   { ... } else { ... }` chain. Allocates a result local
//!   from the unified body type, emits brif to per-branch
//!   blocks, and joins them at a merge block.

use frontend::ast::{ExprRef, Operator, UnaryOp};
use string_interner::DefaultSymbol;

use super::FunctionLower;
use crate::ir::{
    BinOp, BlockId, Const, InstKind, LocalId, Terminator, Type, UnaryOp as IrUnaryOp, ValueId,
};

impl<'a> FunctionLower<'a> {

    /// Branch to a panic when `lhs < rhs`, so the subtraction that
    /// follows cannot wrap.
    ///
    /// The message is static — `Terminator::Panic` carries an interned
    /// symbol, so the operand values cannot be formatted in. The
    /// tree-walker, which has the values to hand, includes them; the
    /// lowered backends report the operation and the location. Both say
    /// the same thing about what happened.
    fn emit_u64_underflow_guard(&mut self, lhs: ValueId, rhs: ValueId) -> Result<(), String> {
        let ok = self
            .emit(
                InstKind::BinOp { op: BinOp::Ge, lhs, rhs },
                Some(Type::Bool),
            )
            .ok_or_else(|| "underflow guard produced no value".to_string())?;
        // The operands travel with the trap: `1 - 5` is the whole
        // diagnostic, and a fixed sentence about which side was
        // smaller is the same information minus the answer.
        self.emit_trap_values_unless(ok, crate::ir::panic_kind::U64_UNDERFLOW, lhs, rhs);
        Ok(())
    }

    /// RUNTIME-TRAP. Branch to a panic when the divisor is zero, so
    /// the integer `Div` / `Rem` that follows cannot trap in the
    /// host. Before this guard the three lowered backends each
    /// failed in their own way — the IR VM through Rust's
    /// `attempt to divide by zero` panic (backtrace into
    /// `ir_vm/dispatch.rs`, no toylang line) and the AOT binary
    /// through cranelift's own `sdiv` trap.
    ///
    /// `F64` is deliberately excluded: IEEE-754 division by zero
    /// produces an infinity, which is a value rather than a fault.
    fn emit_div_by_zero_guard(&mut self, rhs: ValueId, ty: Type) -> Result<(), String> {
        let zero_const = Const::zero(ty)
            .ok_or_else(|| {
                format!(
                    "divide-by-zero guard needs an integer type, got `{}`",
                    crate::spelling::spell_type(self.module, self.interner, ty)
                )
            })?;
        let zero = self
            .emit(InstKind::Const(zero_const), Some(ty))
            .ok_or_else(|| "divide-by-zero guard produced no zero".to_string())?;
        let ok = self
            .emit(
                InstKind::BinOp { op: BinOp::Ne, lhs: rhs, rhs: zero },
                Some(Type::Bool),
            )
            .ok_or_else(|| "divide-by-zero guard produced no value".to_string())?;
        self.emit_trap_unless(ok, self.contract_msgs.div_by_zero);
        Ok(())
    }

    /// RUNTIME-TRAP. Branch to a panic on signed `MIN / -1`, the one
    /// integer division whose result is not representable. Cranelift
    /// lowers `sdiv` to a machine divide that faults on it (the
    /// compiled binary died with `Illegal instruction`), while the
    /// interpreter's `wrapping_div` quietly produced `MIN`; the guard
    /// makes all four engines stop with the same message.
    ///
    /// Emitted as a nested branch — `if rhs == -1 { if lhs == MIN {
    /// panic } }` — rather than a single `&&`, so the common path
    /// costs one comparison and the IR needs no boolean conjunction.
    fn emit_div_overflow_guard(
        &mut self,
        lhs: ValueId,
        rhs: ValueId,
        ty: Type,
    ) -> Result<(), String> {
        let (Some(min_const), Some(minus_one)) = (Const::signed_min(ty), Const::minus_one(ty))
        else {
            return Ok(());
        };
        let neg_one_v = self
            .emit(InstKind::Const(minus_one), Some(ty))
            .ok_or_else(|| "division overflow guard produced no constant".to_string())?;
        let is_minus_one = self
            .emit(
                InstKind::BinOp { op: BinOp::Eq, lhs: rhs, rhs: neg_one_v },
                Some(Type::Bool),
            )
            .ok_or_else(|| "division overflow guard produced no value".to_string())?;
        let check = self.fresh_block();
        let cont = self.fresh_block();
        self.terminate(Terminator::Branch {
            cond: is_minus_one,
            then_blk: check,
            else_blk: cont,
        });
        self.switch_to(check);
        let min_v = self
            .emit(InstKind::Const(min_const), Some(ty))
            .ok_or_else(|| "division overflow guard produced no constant".to_string())?;
        let is_min = self
            .emit(
                InstKind::BinOp { op: BinOp::Eq, lhs, rhs: min_v },
                Some(Type::Bool),
            )
            .ok_or_else(|| "division overflow guard produced no value".to_string())?;
        let fail = self.fresh_block();
        self.terminate(Terminator::Branch {
            cond: is_min,
            then_blk: fail,
            else_blk: cont,
        });
        self.switch_to(fail);
        let site = self.current_site();
        self.terminate(Terminator::Panic {
            message: self.contract_msgs.div_overflow,
            site,
        });
        self.switch_to(cont);
        Ok(())
    }

    /// CONTRACT-ELISION: whether a `requires` clause already proved
    /// this divisor non-zero.
    ///
    /// Only a bare parameter name qualifies. A field, an index, or any
    /// computed expression could have changed since entry, and a
    /// wrongly elided guard is an unchecked division rather than a
    /// missed optimisation — so anything not obviously the parameter
    /// itself keeps its guard.
    fn contract_rules_out_zero(&self, divisor: &ExprRef) -> bool {
        self.parameter_name(divisor)
            .is_some_and(|sym| self.facts.is_nonzero(sym))
    }

    /// CONTRACT-ELISION: whether a `requires` clause proved `lhs >= rhs`,
    /// which is exactly the condition the u64 subtraction guard tests.
    fn contract_rules_out_underflow(&self, lhs: &ExprRef, rhs: &ExprRef) -> bool {
        match (self.parameter_name(lhs), self.parameter_name(rhs)) {
            (Some(a), Some(b)) => self.facts.is_at_least(a, b),
            _ => false,
        }
    }

    /// CONTRACT-ELISION: whether a `requires` clause proved one half of
    /// the signed `MIN / -1` condition impossible. Either the divisor
    /// is known `!= -1` (`requires b != -1i64`, or any non-negativity),
    /// or the dividend is known non-negative — MIN is negative, so a
    /// non-negative lhs can never equal it.
    fn contract_rules_out_div_overflow(&self, lhs: &ExprRef, rhs: &ExprRef) -> bool {
        if let Some(sym) = self.parameter_name(rhs)
            && self.facts.is_not_minus_one(sym)
        {
            return true;
        }
        if let Some(sym) = self.parameter_name(lhs)
            && self.facts.is_nonneg(sym)
        {
            return true;
        }
        false
    }

    /// Learn what `cond` (or its negation) states about the code in
    /// `body`, for the duration of that body's lowering.
    ///
    /// The caller is responsible for restoring the previous facts:
    /// what a branch knows is not what the code after the merge knows.
    pub(super) fn learn_branch(&mut self, cond: &ExprRef, negated: bool, body: &ExprRef) {
        let mutated = crate::contract_facts::mutated_names(self.program, body);
        self.facts
            .learn_condition(self.program, self.interner, cond, negated, &mutated);
    }

    /// The symbol behind `expr` when it is written as a plain name.
    pub(super) fn parameter_name(&self, expr: &ExprRef) -> Option<DefaultSymbol> {
        if self.facts.is_empty() {
            return None;
        }
        match self.program.expression.get(expr)? {
            frontend::ast::Expr::Identifier(sym) => Some(sym),
            _ => None,
        }
    }

    /// Emit `if !ok { panic(message) }` and continue lowering in the
    /// passing block. Shared by every RUNTIME-TRAP guard so all of
    /// them terminate the same way; `Terminator::Panic` carries an
    /// interned symbol, so the message is static and the operand
    /// values cannot be formatted in. The tree-walker, which has the
    /// values to hand, includes them; the lowered backends report the
    /// operation and the location. Both say the same thing about what
    /// happened.
    /// As [`Self::emit_trap_unless`], for a trap whose message is
    /// built from two runtime values.
    pub(super) fn emit_trap_values_unless(
        &mut self,
        ok: ValueId,
        kind: u64,
        a: ValueId,
        b: ValueId,
    ) {
        if self.known_true(ok) {
            return;
        }
        let pass = self.fresh_block();
        let fail = self.fresh_block();
        self.terminate(Terminator::Branch {
            cond: ok,
            then_blk: pass,
            else_blk: fail,
        });
        self.switch_to(fail);
        let site = self.current_site();
        self.terminate(Terminator::PanicValues { kind, a, b, site });
        self.switch_to(pass);
    }

    pub(super) fn emit_trap_unless(&mut self, ok: ValueId, message: string_interner::DefaultSymbol) {
        // COMPILE-TIME-EVAL C2: a guard whose condition folded to
        // `true` cannot fire. Dropping it keeps `2u64 / 1u64` down to
        // one instruction, and — because the block is not split — lets
        // the fold carry on through the rest of the expression.
        //
        // A condition that folded to `false` is left alone: the trap
        // has to happen, and this pass is in no position to say the
        // block runs (see `crate::fold`).
        if self.known_true(ok) {
            return;
        }
        let pass = self.fresh_block();
        let fail = self.fresh_block();
        self.terminate(Terminator::Branch {
            cond: ok,
            then_blk: pass,
            else_blk: fail,
        });
        self.switch_to(fail);
        let site = self.current_site();
        self.terminate(Terminator::Panic { message, site });
        self.switch_to(pass);
    }

    pub(super) fn lower_binary(
        &mut self,
        op: &Operator,
        lhs: &ExprRef,
        rhs: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        if matches!(op, Operator::LogicalAnd | Operator::LogicalOr) {
            return self.lower_short_circuit(op, lhs, rhs);
        }
        let lhs_ty = self.value_scalar(lhs).unwrap_or(Type::U64);
        // Phase B operator overload: `==` / `!=` between two struct
        // values dispatches to the struct's
        // `eq(&self, other: &Self) -> bool` method. Mirrors the
        // interpreter side (operators.rs::evaluate_binary). The
        // type checker has already vetted that both sides are the
        // same nominal struct with an `eq` method, so this lookup
        // is expected to succeed; if it doesn't (concrete-impl
        // dispatch edge cases, etc.) fall through to the regular
        // BinOp path which will error out with the standard
        // mismatch.
        // Comparison-operator method-name table (Phase B + Phase 2
        // extension). Mirrors the frontend's `struct_cmp_method_name`
        // and the interpreter's `overload_method_name` table for
        // the cmp half. Arithmetic overload (`+` / `-` / ...) goes
        // through `let_lowering.rs::Binary` arm because it returns
        // a compound `Self`; cmp returns Bool which fits the
        // single-ValueId return contract here.
        let cmp_method: Option<&'static str> = match op {
            Operator::EQ | Operator::NE => Some("eq"),
            Operator::LT => Some("lt"),
            Operator::LE => Some("le"),
            Operator::GT => Some("gt"),
            Operator::GE => Some("ge"),
            _ => None,
        };
        if let Some(method_name) = cmp_method
            && let Type::Struct(struct_id) = lhs_ty
                && let Some(value) = self.try_lower_struct_cmp(struct_id, lhs, rhs, op, method_name)? {
                    return Ok(Some(value));
                }
        let l = self
            .lower_expr(lhs)?
            .ok_or_else(|| "binary lhs produced no value".to_string())?;
        let r = self
            .lower_expr(rhs)?
            .ok_or_else(|| "binary rhs produced no value".to_string())?;

        // `==` / `!=` between two `str` values compares their bytes.
        // A `BinOp::Eq` here would compare the runtime handles, which
        // are pointers, so `"h".concat("i") == "hi"` came out false
        // once compiled while the interpreter said true.
        if matches!(op, Operator::EQ | Operator::NE) && matches!(lhs_ty, Type::Str) {
            let eq = self
                .emit(InstKind::StrEq { a: l, b: r }, Some(Type::Bool))
                .expect("StrEq returns a value");
            if matches!(op, Operator::NE) {
                let const_false = self
                    .emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))
                    .expect("Const returns a value");
                return Ok(self.emit(
                    InstKind::BinOp { op: BinOp::Eq, lhs: eq, rhs: const_false },
                    Some(Type::Bool),
                ));
            }
            return Ok(Some(eq));
        }

        // One operator table for the whole crate: `crate::fold` owns
        // it, so the fold of a `const` initialiser and the lowering of
        // a body cannot come to different conclusions about an
        // operator.
        let ir_op = crate::fold::binop_for(op).expect("short-circuit ops handled above");
        // SIMD: a lane-wise comparison answers once per lane, so it
        // produces a mask of the same lane width rather than one
        // `bool`.
        let result_ty = match (ir_op.produces_bool(), lhs_ty) {
            (true, Type::Vector(v)) => Type::Vector(v.mask()),
            (true, _) => Type::Bool,
            (false, _) => lhs_ty,
        };
        // DEBUG-OBS D3: a trapping binary operation is reported at its
        // **left operand's** position, which is where the tree-walker
        // has always put it. The whole expression would arguably be a
        // better caret, but a diagnostic that differs by engine is one
        // a reader cannot trust — and the tree-walker's is the wording
        // D0 fixed as the target.
        let trap_site = self.current_expr.replace(*lhs);
        // LLM-LOOP P6-3: trap on unsigned subtraction that would wrap.
        // `0u64 - 1u64` silently becoming 18446744073709551615 is a
        // favourite way to lose an afternoon: the result looks like a
        // plausible large number, so the symptom shows up far from the
        // cause. Emitted here rather than in codegen so the AOT
        // compiler, the IR VM and the compiler-side JIT — all of which
        // consume this IR — get the check from one place.
        if matches!(ir_op, BinOp::Sub)
            && matches!(lhs_ty, Type::U64)
            && !self.contract_rules_out_underflow(lhs, rhs)
        {
            self.emit_u64_underflow_guard(l, r)?;
        }
        // RUNTIME-TRAP: integer division / remainder by zero, and the
        // signed `MIN / -1` whose result is not representable.
        if matches!(ir_op, BinOp::Div | BinOp::Rem) && lhs_ty.is_integer() {
            // CONTRACT-ELISION: a `requires` that proved the divisor
            // non-zero has already been checked at entry, so the guard
            // here can only ever fall through.
            if !self.contract_rules_out_zero(rhs) {
                self.emit_div_by_zero_guard(r, lhs_ty)?;
            }
            if lhs_ty.is_signed() && !self.contract_rules_out_div_overflow(lhs, rhs) {
                self.emit_div_overflow_guard(l, r, lhs_ty)?;
            }
        }
        self.current_expr = trap_site;

        Ok(self.emit(
            InstKind::BinOp {
                op: ir_op,
                lhs: l,
                rhs: r,
            },
            Some(result_ty),
        ))
    }

    /// Phase B operator overload helper. Returns `Ok(Some(value))`
    /// when the struct dispatches `==` / `!=` to its `eq` method;
    /// `Ok(None)` when the lookup misses (caller falls back to the
    /// regular BinOp path). The struct's leaf locals are loaded
    /// for both sides and concatenated as `Call` arguments — same
    /// shape the regular method-call lowering uses for
    /// `Binding::Struct` / borrow-of-struct args.
    fn try_lower_struct_cmp(
        &mut self,
        struct_id: crate::ir::StructId,
        lhs: &ExprRef,
        rhs: &ExprRef,
        op: &Operator,
        method_name: &str,
    ) -> Result<Option<ValueId>, String> {
        let struct_def = self.module.struct_def(struct_id);
        let target_sym = struct_def.base_name;
        let method_sym = match self.interner.get(method_name) {
            Some(s) => s,
            None => return Ok(None),
        };
        // CONCRETE-IMPL-Phase-2c: unified dispatch — a generic-impl
        // `eq` catches receivers the concrete impls don't exactly
        // match.
        let func_id = match self.resolve_struct_method_func_id(
            target_sym, method_sym, struct_id, &[*lhs, *rhs],
        )? {
            Some(f) => f,
            None => return Ok(None),
        };
        // OP-OVERLOAD-CHAIN: both sides take the ordinary
        // compound-argument path — a binding, a literal, a
        // compound-returning call, or another overloaded operator.
        // Restricting them to a bare identifier is what made
        // `if (a + b) == c` fail with "binary lhs produced no value",
        // a sentence that named neither the operand nor the rule.
        //
        // A side that produces no leaves at all is not this shape:
        // fall back to the regular BinOp path, as before — the type
        // checker rejects most such cases up front and the survivors
        // get the standard "Bad types" diagnostic.
        // CODE-SIZE-SELF-ABI: each operand is lowered against the
        // parameter slot it fills, so a wide one is handed over as an
        // address.
        let (mut args, lhs_reload) = match self.lower_arg_values_for(lhs, Some(func_id), 0) {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };
        let rhs_reload = match self.lower_arg_values_for(rhs, Some(func_id), 1) {
            Ok((v, r)) => {
                args.extend(v);
                r
            }
            Err(_) => {
                lhs_reload.skip();
                return Ok(None);
            }
        };
        let bool_v = self
            .emit(InstKind::Call { target: func_id, args }, Some(Type::Bool))
            .ok_or_else(|| format!(
                "operator overload: {} call produced no value", method_name
            ))?;
        lhs_reload.apply(self);
        rhs_reload.apply(self);
        // `!=` is the only operator that routes through `eq` and
        // negates. `<` / `<=` / `>` / `>=` use their own dedicated
        // methods (`lt` / `le` / `gt` / `ge`) and don't need
        // negation. `==` returns the eq result directly.
        if matches!(op, Operator::NE) {
            // emit `bool_v == false` to invert.
            let const_false = self
                .emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))
                .expect("Const returns a value");
            Ok(self.emit(
                InstKind::BinOp { op: BinOp::Eq, lhs: bool_v, rhs: const_false },
                Some(Type::Bool),
            ))
        } else {
            Ok(Some(bool_v))
        }
    }

    pub(super) fn lower_short_circuit(
        &mut self,
        op: &Operator,
        lhs: &ExprRef,
        rhs: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        // We model `lhs && rhs` and `lhs || rhs` as if-expressions that
        // store the result into a fresh bool local, then read it back at
        // the merge point. This keeps the IR a strict block-based shape
        // (no phi-equivalents needed at this layer).
        let result_local = self.module.function_mut(self.func_id).add_local(Type::Bool);
        let then_blk = self.fresh_block();
        let else_blk = self.fresh_block();
        let merge = self.fresh_block();

        let l = self
            .lower_expr(lhs)?
            .ok_or_else(|| "short-circuit lhs produced no value".to_string())?;
        let (true_dest, false_dest) = match op {
            Operator::LogicalAnd => (then_blk, else_blk),
            Operator::LogicalOr => (else_blk, then_blk),
            _ => unreachable!(),
        };
        self.terminate(Terminator::Branch {
            cond: l,
            then_blk: true_dest,
            else_blk: false_dest,
        });

        // `then_blk` evaluates the right operand and stores it.
        self.switch_to(then_blk);
        let r = self
            .lower_expr(rhs)?
            .ok_or_else(|| "short-circuit rhs produced no value".to_string())?;
        self.emit(InstKind::StoreLocal { dst: result_local, src: r }, None);
        self.terminate(Terminator::Jump(merge));

        // `else_blk` writes the short-circuited constant.
        self.switch_to(else_blk);
        let const_val = match op {
            Operator::LogicalAnd => self
                .emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))
                .unwrap(),
            Operator::LogicalOr => self
                .emit(InstKind::Const(Const::Bool(true)), Some(Type::Bool))
                .unwrap(),
            _ => unreachable!(),
        };
        self.emit(
            InstKind::StoreLocal {
                dst: result_local,
                src: const_val,
            },
            None,
        );
        self.terminate(Terminator::Jump(merge));

        self.switch_to(merge);
        Ok(self.emit(InstKind::LoadLocal(result_local), Some(Type::Bool)))
    }

    pub(super) fn lower_unary(
        &mut self,
        op: &UnaryOp,
        operand: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        // REF-Stage-2: borrow ops are erased at the IR layer.
        // Just lower the operand and pass the value through; the
        // frontend type checker is the one that already enforced
        // the `&T` / `&mut T` distinction at call sites.
        if matches!(op, UnaryOp::Borrow | UnaryOp::BorrowMut) {
            return self.lower_expr(operand);
        }
        let operand_ty = self.value_scalar(operand).unwrap_or(Type::U64);
        let v = self
            .lower_expr(operand)?
            .ok_or_else(|| "unary operand produced no value".to_string())?;
        let (ir_op, result_ty) = match op {
            UnaryOp::Negate => (IrUnaryOp::Neg, operand_ty),
            UnaryOp::BitwiseNot => (IrUnaryOp::BitNot, operand_ty),
            UnaryOp::LogicalNot => (IrUnaryOp::LogicalNot, Type::Bool),
            UnaryOp::Borrow | UnaryOp::BorrowMut => unreachable!("handled above"),
        };
        Ok(self.emit(
            InstKind::UnaryOp {
                op: ir_op,
                operand: v,
            },
            Some(result_ty),
        ))
    }

    pub(super) fn lower_if_chain(
        &mut self,
        cond: &ExprRef,
        then_body: &ExprRef,
        elif_pairs: &Vec<(ExprRef, ExprRef)>,
        else_body: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        // Strategy: a fresh bool / scalar local holds the result; each
        // branch writes into it and jumps to the merge block, where the
        // merged value is loaded once. This avoids needing phi-equivalent
        // block parameters in the IR layer.
        //
        // Inferring `result_ty` from `then_body` alone breaks when that
        // branch diverges (e.g. `panic("...")`) — `value_scalar` can't
        // see through `BuiltinCall(Panic, _)`. Fall back to scanning the
        // elif and else bodies in order so the first non-divergent
        // branch picks the type. If every branch diverges we treat the
        // expression as Unit; the merge block will be unreachable but
        // still has to exist for the CFG to be well-formed.
        let result_ty = self
            .value_scalar(then_body)
            .or_else(|| {
                elif_pairs
                    .iter()
                    .find_map(|(_, body)| self.value_scalar(body))
            })
            .or_else(|| self.value_scalar(else_body))
            .unwrap_or(Type::Unit);
        let result_local = if result_ty.produces_value() {
            Some(self.module.function_mut(self.func_id).add_local(result_ty))
        } else {
            None
        };
        let merge = self.fresh_block();

        let mut cond_blocks: Vec<BlockId> = Vec::with_capacity(elif_pairs.len());
        for _ in 0..elif_pairs.len() {
            cond_blocks.push(self.fresh_block());
        }
        let then_blk = self.fresh_block();
        let else_blk = self.fresh_block();

        let c = self
            .lower_expr(cond)?
            .ok_or_else(|| "if condition produced no value".to_string())?;
        let next_after_cond = if !cond_blocks.is_empty() {
            cond_blocks[0]
        } else {
            else_blk
        };
        self.terminate(Terminator::Branch {
            cond: c,
            then_blk,
            else_blk: next_after_cond,
        });

        // Emit each branch body.
        let emit_branch = |this: &mut FunctionLower<'a>, body: &ExprRef, result_local: Option<LocalId>| -> Result<(), String> {
            let v = this.lower_expr(body)?;
            if !this.is_unreachable() {
                if let (Some(local), Some(v)) = (result_local, v) {
                    this.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                }
                this.terminate(Terminator::Jump(merge));
            }
            Ok(())
        };

        // CONTRACT-ELISION (control flow): inside a branch, the
        // condition that led there is known. `if b != 0u64 { a / b }`
        // states exactly what the division guard would test, and so
        // does the `else` of `if b == 0u64 { ... }` — read the other
        // way round. Unlike the facts from `requires`, these hold
        // under `--release` too: the branch is evaluated either way.
        let saved = self.facts.clone();

        // then
        self.learn_branch(cond, false, then_body);
        self.switch_to(then_blk);
        emit_branch(self, then_body, result_local)?;
        self.facts = saved.clone();

        // each elif: cond block then body block
        for (i, (elif_cond, elif_body)) in elif_pairs.iter().enumerate() {
            let cond_blk = cond_blocks[i];
            self.switch_to(cond_blk);
            let body_blk = self.fresh_block();
            let next = if i + 1 < cond_blocks.len() {
                cond_blocks[i + 1]
            } else {
                else_blk
            };
            let c = self
                .lower_expr(elif_cond)?
                .ok_or_else(|| "elif condition produced no value".to_string())?;
            self.terminate(Terminator::Branch {
                cond: c,
                then_blk: body_blk,
                else_blk: next,
            });
            self.switch_to(body_blk);
            // Reaching this body means every earlier condition failed
            // and this one held.
            self.learn_branch(cond, true, elif_body);
            for (earlier, _) in &elif_pairs[..i] {
                self.learn_branch(earlier, true, elif_body);
            }
            self.learn_branch(elif_cond, false, elif_body);
            emit_branch(self, elif_body, result_local)?;
            self.facts = saved.clone();
        }

        // else: every condition failed
        self.learn_branch(cond, true, else_body);
        for (earlier, _) in elif_pairs.iter() {
            self.learn_branch(earlier, true, else_body);
        }
        self.switch_to(else_blk);
        emit_branch(self, else_body, result_local)?;
        self.facts = saved;

        // merge
        self.switch_to(merge);
        if let Some(local) = result_local {
            Ok(self.emit(InstKind::LoadLocal(local), Some(result_ty)))
        } else {
            Ok(None)
        }
    }

}
