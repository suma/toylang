//! Assignment and cast expression lowering.
//!
//! - `lower_cast`: lowers `expr as Target`. The `(from, to)`
//!   pair is recorded on the IR `Cast` so codegen can pick the
//!   right cranelift instruction. Unsupported pairs (e.g.
//!   struct casts) are rejected here so the IR stays in
//!   scalar territory.
//! - `expect_string_literal`: helper that pulls a
//!   `DefaultSymbol` out of a string-literal `ExprRef` (used
//!   by builtins like `panic` / `assert` / `print` that need
//!   a compile-time string).
//! - `lower_assign`: lowers `lhs = rhs` for identifier LHS,
//!   field-access LHS, tuple-access LHS, and array-element
//!   LHS. Compound (struct / tuple / enum) whole-value
//!   assignment is currently unsupported and rejected with a
//!   targeted error.

use frontend::ast::{Expr, ExprRef};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use super::bindings::Binding;
use super::types::lower_scalar;
use super::FunctionLower;
use crate::ir::{InstKind, Type, ValueId};

impl<'a> FunctionLower<'a> {
    /// Lower `expr as Target`. The pair `(from, to)` is recorded on
    /// the IR `Cast` so codegen can pick the right cranelift
    /// instruction. Unsupported pairs (e.g. struct casts) are rejected
    /// here so the IR stays in scalar territory.
    pub(super) fn lower_cast(
        &mut self,
        inner: &ExprRef,
        target_ty: &TypeDecl,
    ) -> Result<Option<ValueId>, String> {
        let to = lower_scalar(target_ty).ok_or_else(|| {
            format!(
                "compiler MVP only supports scalar `as` targets; `{:?}` is not supported yet",
                target_ty
            )
        })?;
        if matches!(to, Type::Unit) {
            return Err("`as` cannot target Unit".to_string());
        }
        let from = self.value_scalar(inner).ok_or_else(|| {
            "compiler MVP could not infer source scalar type for `as` cast".to_string()
        })?;
        if matches!(from, Type::Unit) {
            return Err("`as` cannot convert from Unit".to_string());
        }
        // Same-type casts are accepted but do not need any value
        // movement; we still emit a Cast instruction so callers see the
        // expected `Some(value_id)` and downstream type inference
        // remains stable.
        let v = self
            .lower_expr(inner)?
            .ok_or_else(|| "`as` operand produced no value".to_string())?;
        Ok(self.emit(
            InstKind::Cast { value: v, from, to },
            Some(to),
        ))
    }

    /// `panic` and `assert` only accept a string-literal message in this
    /// MVP, mirroring the JIT's eligibility check. Anything else (a
    /// dynamic concat, a const-binding, etc.) is rejected with an error
    /// instead of silently allowing it.
    pub(super) fn expect_string_literal(&self, expr: &ExprRef, ctx: &str) -> Result<DefaultSymbol, String> {
        match self
            .program
            .expression
            .get(expr)
            .ok_or_else(|| format!("{ctx} message expression missing"))?
        {
            Expr::String(sym) => Ok(sym),
            _ => Err(format!(
                "{ctx} requires a string literal message in this compiler MVP"
            )),
        }
    }

    pub(super) fn lower_assign(
        &mut self,
        lhs: &ExprRef,
        rhs: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let lhs_expr = self
            .program
            .expression
            .get(lhs)
            .ok_or_else(|| "assign lhs missing".to_string())?;
        match lhs_expr {
            Expr::Identifier(sym) => {
                // Enum reassignment: peek at the binding first so we
                // can route the rhs through `lower_into_enum_storage`
                // and reuse the existing storage tree (no need to
                // allocate fresh tag / payload locals — cranelift's
                // SSA construction copes with multiple def_var sites
                // for the same Variable).
                if let Some(Binding::Enum(storage)) = self.bindings.get(&sym).cloned() {
                    self.lower_into_enum_storage(rhs, &storage)?;
                    return Ok(None);
                }
                // REF-Stage-2 (g): assignment to a `&mut T` parameter
                // binding writes through the pointer. Reads of the
                // pointer happen first (via LoadLocal) so the rhs
                // can side-effect freely. `&T` (immutable) bindings
                // reject assignment here — the type checker also
                // catches this earlier, so this is a defence-in-depth.
                // OP-OVERLOAD-EXTEND Phase 1: compound-assign
                // shape (`a += b` desugared to `a = a + b`) for
                // struct receivers. Without this arm the
                // `Binding::Struct` case below would bail with
                // "compiler MVP cannot reassign a struct binding
                // whole". Detect the desugared shape (rhs is a
                // `Binary` with one of the arithmetic overload
                // ops + lhs identifier matches `sym`) and emit
                // `CallStruct` into the binding's existing leaf
                // locals — same dispatch as
                // `let_lowering.rs::Binary` arm except the dest
                // locals are reused instead of freshly allocated.
                if let Some(Binding::Struct { struct_id, fields }) =
                    self.bindings.get(&sym).cloned()
                {
                    if let Some(Expr::Binary(op, b_lhs, b_rhs)) =
                        self.program.expression.get(rhs)
                    {
                        let op_method: Option<&'static str> = match op {
                            frontend::ast::Operator::IAdd => Some("add"),
                            frontend::ast::Operator::ISub => Some("sub"),
                            frontend::ast::Operator::IMul => Some("mul"),
                            frontend::ast::Operator::IDiv => Some("div"),
                            frontend::ast::Operator::IMod => Some("rem"),
                            frontend::ast::Operator::BitwiseAnd => Some("bitand"),
                            frontend::ast::Operator::BitwiseOr => Some("bitor"),
                            frontend::ast::Operator::BitwiseXor => Some("bitxor"),
                            frontend::ast::Operator::LeftShift => Some("shl"),
                            frontend::ast::Operator::RightShift => Some("shr"),
                            _ => None,
                        };
                        if let Some(method_name) = op_method {
                            let struct_def = self.module.struct_def(struct_id);
                            let target_sym = struct_def.base_name;
                            let type_args = struct_def.type_args.clone();
                            if let Some(method_sym) = self.interner.get(method_name) {
                                if let Some(func_id) =
                                    super::method_registry::lookup_method_func(
                                        self.method_func_ids,
                                        target_sym,
                                        method_sym,
                                        &type_args,
                                    )
                                {
                                    use super::bindings::flatten_struct_locals;
                                    let lhs_leaves = match self
                                        .program
                                        .expression
                                        .get(&b_lhs)
                                    {
                                        Some(Expr::Identifier(s)) => match self
                                            .bindings
                                            .get(&s)
                                            .cloned()
                                        {
                                            Some(Binding::Struct { fields: f, .. }) => {
                                                flatten_struct_locals(&f)
                                            }
                                            _ => {
                                                return Err(
                                                    "compound-assign: arith lhs needs struct binding".to_string(),
                                                );
                                            }
                                        },
                                        _ => {
                                            return Err(
                                                "compound-assign: arith lhs must be a bare identifier".to_string(),
                                            );
                                        }
                                    };
                                    let rhs_leaves = match self
                                        .program
                                        .expression
                                        .get(&b_rhs)
                                    {
                                        Some(Expr::Identifier(s)) => match self
                                            .bindings
                                            .get(&s)
                                            .cloned()
                                        {
                                            Some(Binding::Struct { fields: f, .. }) => {
                                                flatten_struct_locals(&f)
                                            }
                                            _ => {
                                                return Err(
                                                    "compound-assign: arith rhs needs struct binding".to_string(),
                                                );
                                            }
                                        },
                                        _ => {
                                            return Err(
                                                "compound-assign: arith rhs must be a bare identifier".to_string(),
                                            );
                                        }
                                    };
                                    let mut all_args: Vec<ValueId> = Vec::with_capacity(
                                        lhs_leaves.len() + rhs_leaves.len(),
                                    );
                                    for (local, ty) in
                                        lhs_leaves.iter().chain(rhs_leaves.iter())
                                    {
                                        let v = self
                                            .emit(
                                                InstKind::LoadLocal(*local),
                                                Some(*ty),
                                            )
                                            .expect("LoadLocal returns a value");
                                        all_args.push(v);
                                    }
                                    // Dest = the existing binding's
                                    // leaf locals (reuse, no fresh
                                    // allocate).
                                    let dests: Vec<crate::ir::LocalId> =
                                        flatten_struct_locals(&fields)
                                            .into_iter()
                                            .map(|(l, _)| l)
                                            .collect();
                                    self.emit(
                                        InstKind::CallStruct {
                                            target: func_id,
                                            args: all_args,
                                            dests,
                                        },
                                        None,
                                    );
                                    return Ok(None);
                                }
                            }
                        }
                    }
                }
                if let Some(Binding::RefScalar { local, pointee_ty, is_mut }) =
                    self.bindings.get(&sym).cloned()
                {
                    if !is_mut {
                        return Err(format!(
                            "cannot assign through immutable reference binding `{}`",
                            self.interner.resolve(sym).unwrap_or("?"),
                        ));
                    }
                    let rhs_val = self
                        .lower_expr(rhs)?
                        .ok_or_else(|| "assignment rhs produced no value".to_string())?;
                    let ptr = self
                        .emit(InstKind::LoadLocal(local), Some(Type::U64))
                        .ok_or_else(|| "RefScalar assign: LoadLocal returned no value".to_string())?;
                    self.emit(InstKind::StoreRef { ptr, value: rhs_val, ty: pointee_ty }, None);
                    return Ok(None);
                }
                let rhs_val = self
                    .lower_expr(rhs)?
                    .ok_or_else(|| "assignment rhs produced no value".to_string())?;
                let local = match self.bindings.get(&sym) {
                    Some(Binding::Scalar { local, .. }) => *local,
                    Some(Binding::RefScalar { .. }) => {
                        // Already handled above; the early-return
                        // above means we never reach here.
                        unreachable!("RefScalar assign was peeked");
                    }
                    Some(Binding::Struct { .. }) => {
                        return Err(format!(
                            "compiler MVP cannot reassign a struct binding `{}` whole (assign individual fields instead)",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                    Some(Binding::Tuple { .. }) => {
                        return Err(format!(
                            "compiler MVP cannot reassign a tuple binding `{}` whole (assign individual elements via `{}.N = ...`)",
                            self.interner.resolve(sym).unwrap_or("?"),
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                    Some(Binding::Enum(_)) => {
                        // Already handled above.
                        unreachable!("enum reassign was peeked");
                    }
                    Some(Binding::Array { .. }) => {
                        return Err(format!(
                            "compiler MVP cannot reassign an array binding `{}` whole (assign individual elements via `{}[i] = ...` instead)",
                            self.interner.resolve(sym).unwrap_or("?"),
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                    Some(Binding::FunctionPtr { .. }) => {
                        return Err(format!(
                            "compiler MVP cannot reassign a function-value binding `{}`",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                    None => {
                        return Err(format!(
                            "undefined identifier `{}`",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                };
                self.emit(InstKind::StoreLocal { dst: local, src: rhs_val }, None);
                Ok(None)
            }
            Expr::TupleAccess(tuple, index) => {
                // `t.N = rhs`. Resolve to the tuple element local
                // and store. Mirrors struct field assignment.
                let local = self.resolve_tuple_element_local(&tuple, index)?;
                let rhs_val = self
                    .lower_expr(rhs)?
                    .ok_or_else(|| "tuple-element assignment rhs produced no value".to_string())?;
                self.emit(InstKind::StoreLocal { dst: local, src: rhs_val }, None);
                Ok(None)
            }
            Expr::FieldAccess(obj, field) => {
                // `obj.field = rhs`. Resolve obj statically to a struct
                // binding, then store rhs into that field's local.
                let local = self.resolve_field_local(&obj, field)?;
                let rhs_val = self
                    .lower_expr(rhs)?
                    .ok_or_else(|| "field assignment rhs produced no value".to_string())?;
                self.emit(InstKind::StoreLocal { dst: local, src: rhs_val }, None);
                Ok(None)
            }
            _ => Err("assignment to non-identifier / non-field-access is not supported yet".into()),
        }
    }

}
