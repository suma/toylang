//! Loop construct lowering (`while` / `for ... in`).
//!
//! - `lower_while`: emits the standard `header -> body -> back-
//!   edge -> exit` brif chain for `while cond { body }`. The
//!   condition is re-evaluated at every iteration.
//! - `lower_for`: lowers `for i in start..end { body }` to a
//!   counter-based loop. Allocates a fresh `LocalId` for the
//!   loop variable, materialises start / end into per-iteration
//!   compares, and writes back the incremented counter at the
//!   end of each iteration.

use frontend::ast::ExprRef;
use string_interner::DefaultSymbol;

use super::bindings::Binding;
use super::FunctionLower;
use crate::ir::{BinOp, BlockId, Const, InstKind, Terminator, Type, ValueId};

impl<'a> FunctionLower<'a> {
    pub(super) fn lower_while(
        &mut self,
        label: Option<DefaultSymbol>,
        cond: &ExprRef,
        body: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let header = self.fresh_block();
        let body_blk = self.fresh_block();
        let exit = self.fresh_block();
        self.terminate(Terminator::Jump(header));
        self.switch_to(header);
        let c = self
            .lower_expr(cond)?
            .ok_or_else(|| "while condition produced no value".to_string())?;
        self.terminate(Terminator::Branch {
            cond: c,
            then_blk: body_blk,
            else_blk: exit,
        });
        self.switch_to(body_blk);
        // #121 Phase B-rest Item 2: snapshot the with-scope depth at
        // loop entry so `break` / `continue` inside the loop body
        // emit `AllocPop` only for `with` scopes opened *inside*
        // the loop, not the outer ones.
        self.loop_stack.push((label, header, exit, self.with_scope_depth, self.drop_scopes.len()));
        let _ = self.lower_expr(body)?;
        self.loop_stack.pop();
        if !self.is_unreachable() {
            self.terminate(Terminator::Jump(header));
        }
        self.switch_to(exit);
        Ok(None)
    }

    /// LABEL: walk loop_stack rev-first matching `label`. `None` returns
    /// innermost. Type checker should already guarantee resolvability.
    pub(super) fn resolve_loop_frame(
        &self,
        label: Option<DefaultSymbol>,
        kw: &str,
    ) -> Result<&(Option<DefaultSymbol>, BlockId, BlockId, usize, usize), String> {
        match label {
            None => self
                .loop_stack
                .last()
                .ok_or_else(|| format!("`{kw}` outside of a loop")),
            Some(sym) => self
                .loop_stack
                .iter()
                .rev()
                .find(|f| f.0 == Some(sym))
                .ok_or_else(|| format!("`{kw}` references undefined loop label")),
        }
    }


    /// Evaluate a call's argument list (`Expr::ExprList(items)`) into
    /// a vector of `ValueId`s. Each argument is lowered through the
    /// regular expression path. Struct-typed identifier arguments are
    /// expanded into per-field values matching the callee signature.
    pub(super) fn lower_for(
        &mut self,
        label: Option<DefaultSymbol>,
        var_name: DefaultSymbol,
        start: &ExprRef,
        end: &ExprRef,
        body: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let scalar = self.value_scalar(start).unwrap_or(Type::U64);
        let start_v = self
            .lower_expr(start)?
            .ok_or_else(|| "for start produced no value".to_string())?;
        let end_v = self
            .lower_expr(end)?
            .ok_or_else(|| "for end produced no value".to_string())?;
        let local = self.module.function_mut(self.func_id).add_local(scalar);
        self.bindings
            .insert(var_name, Binding::Scalar { local, ty: scalar });
        // Stash the upper bound in its own local so the header block can
        // reload it on each iteration without having to thread it through
        // a block parameter.
        let end_local = self.module.function_mut(self.func_id).add_local(scalar);
        self.emit(InstKind::StoreLocal { dst: local, src: start_v }, None);
        self.emit(InstKind::StoreLocal { dst: end_local, src: end_v }, None);

        let header = self.fresh_block();
        let body_blk = self.fresh_block();
        // LABEL/CONTINUE-FIX: dedicated step block so `continue` (bare or
        // labelled) jumps to the increment, not the header — otherwise
        // `continue` would re-test the same `i` and loop forever.
        // Linear body fall-through still goes through `step` too.
        let step = self.fresh_block();
        let exit = self.fresh_block();
        self.terminate(Terminator::Jump(header));

        // Header: cmp i, end.
        self.switch_to(header);
        let i = self
            .emit(InstKind::LoadLocal(local), Some(scalar))
            .unwrap();
        let e = self
            .emit(InstKind::LoadLocal(end_local), Some(scalar))
            .unwrap();
        let cmp = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Lt,
                    lhs: i,
                    rhs: e,
                },
                Some(Type::Bool),
            )
            .unwrap();
        self.terminate(Terminator::Branch {
            cond: cmp,
            then_blk: body_blk,
            else_blk: exit,
        });

        // CONTRACT-ELISION (control flow): inside the body the
        // induction variable is bounded by the range. `for i in
        // 0u64..8u64 { a[i] }` on an `[T; 8]` states exactly what the
        // index guard would test. Both spellings (`..` and `to`) lower
        // to the `i < end` header above, so a literal end is the bound.
        let saved = self.facts.clone();
        let mutated = crate::contract_facts::mutated_names(self.program, body);
        self.facts
            .learn_range(self.program, self.interner, var_name, start, end, &mutated);

        // Body, then jump to step block (which increments + jumps to header).
        self.switch_to(body_blk);
        // #121 Phase B-rest Item 2: snapshot the with-scope depth at
        // loop entry so `break` / `continue` inside the loop body
        // emit `AllocPop` only for `with` scopes opened *inside*
        // the loop, not the outer ones.
        // Continue target = step (increments before jumping back).
        self.loop_stack.push((label, step, exit, self.with_scope_depth, self.drop_scopes.len()));
        let _ = self.lower_expr(body)?;
        self.loop_stack.pop();
        // The range bound belongs to the body; everything after the
        // loop sees the variable gone.
        self.facts = saved;
        if !self.is_unreachable() {
            self.terminate(Terminator::Jump(step));
        }

        // Step block: increment local, jump back to header.
        self.switch_to(step);
        let cur = self
            .emit(InstKind::LoadLocal(local), Some(scalar))
            .unwrap();
        // NUM-W-FOR-RANGE: the step constant has to be in the
        // induction variable's own type. This used to pick between
        // `I64` and `U64` only, so a narrow range (`for i in
        // -3i32..2i32`) added a 64-bit one to a 32-bit counter — the
        // instruction claimed `Type::I32` while carrying a `Const::U64`
        // and cranelift's verifier refused the `iadd`
        // (`arg 1 (v18) has type i64, expected i32`). A crash, not a
        // diagnostic, because the type checker had accepted the loop.
        let one = self
            .emit(
                InstKind::Const(Const::from_usize_in(scalar, 1).unwrap_or(Const::U64(1))),
                Some(scalar),
            )
            .unwrap();
        let next = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Add,
                    lhs: cur,
                    rhs: one,
                },
                Some(scalar),
            )
            .unwrap();
        self.emit(InstKind::StoreLocal { dst: local, src: next }, None);
        self.terminate(Terminator::Jump(header));

        self.switch_to(exit);
        Ok(None)
    }
}
