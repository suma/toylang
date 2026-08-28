use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError,
    AcceptableExpr, AcceptableStmt, TypeInferenceManager
};
use crate::type_checker::generics::GenericTypeChecking;

/// Expression type checking implementation
impl<'a> TypeCheckerVisitor<'a> {
    /// REF-Stage-2 (iii): walk a `&mut <expr>` operand down through
    /// field-, tuple-, and (single-element) index-access chains to
    /// the root binding name. Accepts shapes:
    ///   - `Expr::Identifier(s)` -> `s`
    ///   - `Expr::FieldAccess(obj, _)` -> recurse on `obj`
    ///   - `Expr::TupleAccess(obj, _)` -> recurse on `obj`
    ///   - `Expr::SliceAccess(obj, SingleElement{..})` -> recurse on `obj`
    ///
    /// Range-slice access (`&mut arr[a..b]`) and other non-place
    /// shapes are rejected.
    fn find_borrow_lvalue_root(
        &self,
        expr: &ExprRef,
    ) -> Result<DefaultSymbol, TypeCheckError> {
        let mut cur = *expr;
        loop {
            let obj = self.core.expr_pool.get(&cur).ok_or_else(|| {
                TypeCheckError::generic_error("Invalid lvalue expression reference")
            })?;
            match obj {
                Expr::Identifier(sym) => return Ok(sym),
                Expr::FieldAccess(obj, _) => cur = obj,
                Expr::TupleAccess(obj, _) => cur = obj,
                Expr::SliceAccess(obj, info) => {
                    if !matches!(info.slice_type, crate::ast::SliceType::SingleElement) {
                        return Err(TypeCheckError::generic_error(
                            "cannot take a mutable borrow of a range-slice expression; \
                             only single-element index borrow is supported",
                        ));
                    }
                    cur = obj;
                }
                _ => {
                    return Err(TypeCheckError::generic_error(
                        "cannot take a mutable borrow of a non-place expression; \
                         only `&mut <name>`, `&mut <name>.field`, `&mut <name>.0`, or \
                         `&mut <name>[i]` are supported in REF-Stage-2",
                    ));
                }
            }
        }
    }

    /// Main entry point for expression type checking
    pub fn visit_expr(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // Check cache first
        if let Some(cached_type) = self.get_cached_type(expr) {
            return Ok(cached_type.clone());
        }

        // Set up context hint for nested expressions
        let original_hint = self.type_inference.type_hint.clone();
        let expr_obj = self.core.expr_pool.get(expr)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference"))?;

        // `expr?` — postfix early-return operator. The parser emits
        // `Expr::Try { inner, .. }`; we intercept here (rather than
        // going through `visit_try`) because the desugar needs the
        // Try's own ExprRef so it can rewrite the pool entry in
        // place. After rewriting, the same ExprRef holds a `Match`,
        // so every later visitor (backends, etc.) sees only the
        // desugared form.
        if let Expr::Try { inner, .. } = &expr_obj {
            return self.desugar_try_expr(*expr, *inner);
        }

        if let Some(result) = self.intercept_struct_update(expr)? {
            return Ok(result);
        }


        // From/Into: `expr.into()` rewrites to `Target::from(expr)`
        // when the expected type is known. Like `Try`, this needs the
        // MethodCall's own ExprRef to rewrite the pool entry in place;
        // the result is a plain `AssociatedFunctionCall` that every
        // backend lowers like a hand-written `String::from(...)`.
        // Only a zero-arg `into` on a known-target receiver is
        // intercepted; anything else falls through to normal method
        // dispatch (and its usual "no method named into" error).
        if let Expr::MethodCall(obj_ref, method_sym, args) = &expr_obj {
            let method_name = self
                .core
                .string_interner
                .resolve(*method_sym)
                .unwrap_or("?")
                .to_string();
            if method_name == "into"
                && args.is_empty()
                && self.rewrite_into_call(*expr, *obj_ref)
            {
                return self.visit_expr(expr);
            }
        }

        let result = expr_obj.clone().accept_expr(self);
        
        // Add location information to errors if not already present
        let result = match result {
            Err(mut error) if error.location.is_none() => {
                error.location = self.get_expr_location(expr);
                Err(error)
            }
            other => other,
        };
        
        // Cache result and record type if successful
        if let Ok(ref result_type) = result {
            self.cache_type(expr, result_type.clone());
            self.type_inference.set_expr_type(*expr, result_type.clone());
            self.note_visited_number(expr, result_type);
            
            // Context propagation for numeric types
            if original_hint.is_none() && (result_type == &TypeDecl::Int64 || result_type == &TypeDecl::UInt64)
                && self.type_inference.type_hint.is_none() {
                    self.type_inference.type_hint = Some(result_type.clone());
                }
        }

        result
    }

    /// Type check unary operators
    pub fn visit_unary(&mut self, op: &UnaryOp, operand: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let op = op.clone();
        let operand = *operand;
        let operand_ty = {
            let operand_obj = self.core.expr_pool.get(&operand)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid operand expression reference"))?;
            let ty = operand_obj.clone().accept_expr(self)?;
            self.note_visited_number(&operand, &ty);
            ty
        };

        // REF-Stage-2: explicit `&expr` / `&mut expr` short-circuit
        // before the Number/coercion logic runs, since wrapping an
        // unresolved Number literal in a borrow doesn't make sense.
        if matches!(op, UnaryOp::Borrow | UnaryOp::BorrowMut) {
            return self.check_unary_borrow(&op, &operand, operand_ty);
        }

        // Resolve type with automatic conversion for Number type. Negation
        // implies a signed result, so coerce an unspecified Number to Int64
        // the same way an explicit `-3i64` literal would land.
        let resolved_ty = if operand_ty == TypeDecl::Number {
            match op {
                UnaryOp::BitwiseNot => TypeDecl::UInt64,
                UnaryOp::Negate => TypeDecl::Int64,
                UnaryOp::LogicalNot => TypeDecl::Bool,
                UnaryOp::Borrow | UnaryOp::BorrowMut => unreachable!("borrow handled above"),
            }
        } else {
            operand_ty.clone()
        };

        // Transform AST node if type conversion occurred
        if operand_ty == TypeDecl::Number && resolved_ty != TypeDecl::Number {
            self.transform_numeric_expr(&operand, &resolved_ty)?;
        }

        // OP-OVERLOAD-EXTEND Phase 4: unary operator overload.
        // `-x` / `~x` / `!x` for matching struct values dispatch
        // to the user-defined `neg` / `bitnot` / `not` method
        // (`fn ___(&self) -> Self`). Catch this before the
        // primitive-only checks below so the standard "type
        // mismatch in unary X" diagnostic doesn't preempt the
        // overload.
        if let Some(method_name) = Self::struct_unary_method_name(&op)
            && self.struct_method_compatible(&resolved_ty, &resolved_ty, method_name) {
                return Ok(resolved_ty);
            }

        self.check_unary_primitive(&op, &operand, &resolved_ty)
    }

    /// REF-Stage-2: type-check `&expr` / `&mut expr`. `&mut` requires
    /// the operand to be a mutable lvalue (bare identifier or
    /// field/tuple-access chain rooted at a `var`-declared name);
    /// `&` accepts any operand. Result is `Ref { is_mut, inner }`,
    /// collapsing nested borrows so `&(&x)` doesn't double-wrap.
    fn check_unary_borrow(
        &mut self,
        op: &UnaryOp,
        operand: &ExprRef,
        operand_ty: TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        let is_mut = matches!(op, UnaryOp::BorrowMut);
        // REF-Stage-2 (f) + (iii): `&mut <expr>` is only valid against a
        // mutable lvalue (bare identifier, field-access chain, or
        // tuple-access chain rooted at a `var`-declared binding).
        // Index-borrow (`&mut arr[i]`) is still future work.
        if is_mut {
            let root = self.find_borrow_lvalue_root(operand)?;
            match self.context.is_var_mutable(root) {
                Some(true) => {}
                Some(false) => {
                    let name = self.core.string_interner.resolve(root).unwrap_or("?").to_string();
                    return Err(self.error_with_location(
                        TypeCheckError::generic_error(&format!(
                            "cannot borrow `{}` as mutable: binding is not declared `var`",
                            name
                        )),
                        operand,
                    ));
                }
                None => {
                    // Identifier resolves to something other than a
                    // local binding (e.g. a top-level const). Those
                    // are also not mutable lvalues.
                    let name = self.core.string_interner.resolve(root).unwrap_or("?").to_string();
                    return Err(self.error_with_location(
                        TypeCheckError::generic_error(&format!(
                            "cannot take a mutable borrow of `{}`: not a mutable local binding",
                            name
                        )),
                        operand,
                    ));
                }
            }
        }
        // Collapse `&(&x)` to a single Ref so the type doesn't grow on
        // re-borrow.
        let inner_ty = match operand_ty {
            TypeDecl::Ref { inner, .. } => *inner,
            other => other,
        };
        Ok(TypeDecl::Ref { is_mut, inner: Box::new(inner_ty) })
    }

    /// Per-op result-type rule for primitive unary operators after the
    /// borrow short-circuit and Number-resolution. `Negate` rejects
    /// u64 to avoid the silent-wraparound surprise
    /// `-(1u64) == 2^64 - 1`; cast first if you really want that.
    fn check_unary_primitive(
        &self,
        op: &UnaryOp,
        operand: &ExprRef,
        resolved_ty: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        match op {
            UnaryOp::BitwiseNot => {
                // NUM-W: every integer width, not just the 64-bit pair.
                // `docs/language.md` promises the narrow widths behave
                // identically to `u64` / `i64`, and `~` is an integer
                // operator, so `~x` for `x: u8` has to type-check.
                if resolved_ty.is_integer() {
                    Ok(resolved_ty.clone())
                } else {
                    Err(self.error_with_location(
                        TypeCheckError::type_mismatch_operation("bitwise NOT", resolved_ty.clone(), TypeDecl::Unit),
                        operand,
                    ))
                }
            }
            UnaryOp::LogicalNot => {
                if *resolved_ty == TypeDecl::Bool {
                    Ok(TypeDecl::Bool)
                } else {
                    Err(self.error_with_location(
                        TypeCheckError::type_mismatch_operation("logical NOT", resolved_ty.clone(), TypeDecl::Unit),
                        operand,
                    ))
                }
            }
            UnaryOp::Negate => {
                // NUM-W: any *signed* width, plus f64. Unsigned stays
                // rejected -- there is no value for `-x` to take.
                if resolved_ty.is_signed_integer() || *resolved_ty == TypeDecl::Float64 {
                    Ok(resolved_ty.clone())
                } else {
                    Err(self.error_with_location(
                        TypeCheckError::type_mismatch_operation("unary minus", resolved_ty.clone(), TypeDecl::Int64),
                        operand,
                    ))
                }
            }
            UnaryOp::Borrow | UnaryOp::BorrowMut => unreachable!("borrow handled in check_unary_borrow"),
        }
    }

    /// Unary operator overload table (Phase 4 extension). Maps
    /// `Negate` / `BitwiseNot` / `LogicalNot` to the user-defined
    /// `neg` / `bitnot` / `not` methods (each `fn (&self) -> Self`).
    /// Borrow / BorrowMut are intentionally excluded — they're
    /// reference-construction operators, not arithmetic-style
    /// overloads.
    pub(crate) fn struct_unary_method_name(op: &UnaryOp) -> Option<&'static str> {
        match op {
            UnaryOp::Negate => Some("neg"),
            UnaryOp::BitwiseNot => Some("bitnot"),
            UnaryOp::LogicalNot => Some("not"),
            UnaryOp::Borrow | UnaryOp::BorrowMut => None,
        }
    }

    /// Type check binary operators
    pub fn visit_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let op = op.clone();
        let lhs = *lhs;
        let rhs = *rhs;

        let lhs_ty = {
            let lhs_obj = self.core.expr_pool.get(&lhs)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid left-hand expression reference"))?;
            let ty = lhs_obj.clone().accept_expr(self)?;
            self.note_visited_number(&lhs, &ty);
            ty
        };

        let rhs_ty = {
            let rhs_obj = self.core.expr_pool.get(&rhs)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid right-hand expression reference"))?;
            let ty = rhs_obj.clone().accept_expr(self)?;
            self.note_visited_number(&rhs, &ty);
            ty
        };


        // `Unknown` is the checker's poison type: it marks an operand
        // whose real type could not be determined, either because it
        // diverges (`panic("...")`) or because its defining statement
        // already reported an error and LLM-LOOP P1 recovery bound it to
        // `Unknown` to keep checking. Either way the operands cannot be
        // compared usefully, and reporting "expected Unknown, but got
        // UInt64" would bury the real diagnostic under noise that names
        // an internal type the user never wrote. Propagate instead.
        if lhs_ty == TypeDecl::Unknown || rhs_ty == TypeDecl::Unknown {
            return Ok(TypeDecl::Unknown);
        }

        // Resolve concrete types from generics / Number placeholders.
        // Shift ops get their own resolver because the rhs must be UInt64
        // regardless of any Number context hint.
        let (resolved_lhs_ty, resolved_rhs_ty) = if matches!(op, Operator::LeftShift | Operator::RightShift) {
            self.resolve_shift_operand_types(&lhs_ty, &rhs_ty)
        } else {
            self.resolve_numeric_types(&lhs_ty, &rhs_ty)
                .map_err(|error| self.error_with_location(error, &lhs))?
        };

        // Type-hint propagation, Number resolution, and AST transform
        // for any side that resolved to a concrete type. Shared by
        // every operator category; the per-category result-type rule
        // below operates on the post-propagation `resolved_*` types.
        self.propagate_number_types(&lhs, &rhs, &lhs_ty, &rhs_ty, &resolved_lhs_ty, &resolved_rhs_ty)?;

        // Dispatch to the per-category visitor. Each one handles its
        // own operator-overload short-circuit and produces a
        // `TypeCheckError` with a category-specific label on mismatch.
        match op {
            Operator::IAdd | Operator::ISub | Operator::IDiv | Operator::IMul | Operator::IMod => {
                self.visit_arith_binary(&op, &lhs, &resolved_lhs_ty, &resolved_rhs_ty)
            }
            Operator::LE | Operator::LT | Operator::GE | Operator::GT | Operator::EQ | Operator::NE => {
                self.visit_compare_binary(&op, &lhs, &resolved_lhs_ty, &resolved_rhs_ty)
            }
            Operator::LogicalAnd | Operator::LogicalOr => {
                self.visit_logical_binary(&lhs, &resolved_lhs_ty, &resolved_rhs_ty)
            }
            Operator::BitwiseAnd | Operator::BitwiseOr | Operator::BitwiseXor => {
                self.visit_bitwise_binary(&op, &lhs, &resolved_lhs_ty, &resolved_rhs_ty)
            }
            Operator::LeftShift | Operator::RightShift => {
                self.visit_shift_binary(&op, &lhs, &rhs, &resolved_lhs_ty, &resolved_rhs_ty)
            }
        }
    }

    /// Shared Number-type bookkeeping for `visit_binary`: propagate
    /// type hints, immediate-propagate concrete types into bare
    /// `Number` literals, transform Number AST nodes whose target
    /// type just settled, and update identifier types. Extracted
    /// from `visit_binary` so the per-category result-type helpers
    /// can be small.
    fn propagate_number_types(
        &mut self,
        lhs: &ExprRef,
        rhs: &ExprRef,
        lhs_ty: &TypeDecl,
        rhs_ty: &TypeDecl,
        resolved_lhs_ty: &TypeDecl,
        resolved_rhs_ty: &TypeDecl,
    ) -> Result<(), TypeCheckError> {
        // Context propagation: if we have a type hint, propagate it to Number expressions
        if let Some(hint) = self.type_inference.type_hint.clone() {
            if *lhs_ty == TypeDecl::Number && (hint == TypeDecl::Int64 || hint == TypeDecl::UInt64) {
                self.propagate_type_to_number_expr(lhs, &hint)?;
            }
            if *rhs_ty == TypeDecl::Number && (hint == TypeDecl::Int64 || hint == TypeDecl::UInt64) {
                self.propagate_type_to_number_expr(rhs, &hint)?;
            }
        }

        // Record Number usage context for later finalization
        self.record_number_usage_context(lhs, lhs_ty, resolved_lhs_ty)?;
        self.record_number_usage_context(rhs, rhs_ty, resolved_rhs_ty)?;

        // Immediate propagation: if one side has concrete type, propagate to Number variables
        if *resolved_lhs_ty != TypeDecl::Number && *rhs_ty == TypeDecl::Number {
            self.propagate_to_number_variable(rhs, resolved_lhs_ty)?;
        }
        if *resolved_rhs_ty != TypeDecl::Number && *lhs_ty == TypeDecl::Number {
            self.propagate_to_number_variable(lhs, resolved_rhs_ty)?;
        }

        // Transform AST nodes if type conversion occurred
        if *lhs_ty == TypeDecl::Number && *resolved_lhs_ty != TypeDecl::Number {
            self.transform_numeric_expr(lhs, resolved_lhs_ty)?;
        }
        if *rhs_ty == TypeDecl::Number && *resolved_rhs_ty != TypeDecl::Number {
            self.transform_numeric_expr(rhs, resolved_rhs_ty)?;
        }

        // Update variable types if identifiers were involved in type conversion
        self.update_identifier_types(lhs, lhs_ty, resolved_lhs_ty)?;
        self.update_identifier_types(rhs, rhs_ty, resolved_rhs_ty)?;
        Ok(())
    }

    /// Result-type rule for `+ - * / %` between numeric / generic /
    /// struct-overload pairs. There is no string `+` — concatenation
    /// is `a.concat(b)`, and neither `str` (a primitive) nor `String`
    /// provides an `add` overload, so `"a" + "b"` is a type error.
    /// NUM-W narrow integers follow the same-width rule as i64/u64 —
    /// no implicit widening.
    fn visit_arith_binary(
        &self,
        op: &Operator,
        lhs: &ExprRef,
        l: &TypeDecl,
        r: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        // Operator overload (Phase B): `add` / `sub` / `mul` / `div` / `rem`
        // on matching struct pairs. Checked first so numeric-only
        // diagnostics don't preempt the user's overload.
        if let Some(method_name) = Self::struct_arith_method_name(op)
            && self.struct_method_compatible(l, r, method_name) {
            return Ok(l.clone());
        }

        // Same-width primitive numeric pair (u64, i64, f64, and NUM-W narrow ints).
        if let Some(ty) = Self::same_numeric_pair(l, r) {
            return Ok(ty);
        }

        if let (TypeDecl::Generic(left_param), TypeDecl::Generic(right_param)) = (l, r) {
            // Generic-type arithmetic when both sides are the same parameter.
            if left_param == right_param {
                return Ok(l.clone());
            }
        }

        // TYPECHECK-LIES: `"a" + "b"` is the first thing most people
        // try, and "incompatible types str and str" reads like a
        // compiler bug when both sides plainly have the same type.
        // Name the operations that do work instead. `String` (the
        // heap buffer) lands here too, since it has no `add` overload.
        if matches!(op, Operator::IAdd) && self.is_string_like(l) && self.is_string_like(r) {
            return Err(self.error_with_location(
                TypeCheckError::unsupported_operation(
                    "`+` (concatenate with `a.concat(b)`, or interpolate: \"{a}{b}\")",
                    l.clone(),
                ),
                lhs,
            ));
        }

        Err(self.error_with_location(
            TypeCheckError::type_mismatch_operation("arithmetic", l.clone(), r.clone()),
            lhs,
        ))
    }

    /// Whether `ty` is one of the two string types: the `str`
    /// primitive, or the stdlib `String` heap buffer.
    fn is_string_like(&self, ty: &TypeDecl) -> bool {
        match ty {
            TypeDecl::String => true,
            TypeDecl::Identifier(sym) | TypeDecl::Struct(sym, _) => {
                self.resolve_symbol_name(*sym) == "String"
            }
            _ => false,
        }
    }

    /// Returns the concrete `TypeDecl` when `l` and `r` are the same
    /// primitive numeric type (UInt64, Int64, Float64, or any NUM-W
    /// narrow width). Used by `visit_arith_binary` and
    /// `visit_compare_binary` to eliminate repetitive match arms.
    fn same_numeric_pair(l: &TypeDecl, r: &TypeDecl) -> Option<TypeDecl> {
        if l == r && l.is_numeric() {
            Some(l.clone())
        } else {
            None
        }
    }

    /// Result-type rule for `< <= > >= == !=`: bool for any
    /// matching int width, f64, bool, allocator-handle (== / != only),
    /// or struct overload (eq / lt / le / gt / ge).
    fn visit_compare_binary(
        &self,
        op: &Operator,
        lhs: &ExprRef,
        l: &TypeDecl,
        r: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        // u64/i64 cross-width compares (e.g. `0u64 < 1i64` is rejected,
        // but the generic numeric-match helper below covers same-width).
        if (*l == TypeDecl::UInt64 || *l == TypeDecl::Int64)
            && (*r == TypeDecl::UInt64 || *r == TypeDecl::Int64) {
            return Ok(TypeDecl::Bool);
        }

        // Same-width primitive numeric pair (narrow ints + f64).
        if Self::same_numeric_pair(l, r).is_some() {
            return Ok(TypeDecl::Bool);
        }

        if *l == TypeDecl::Bool && *r == TypeDecl::Bool {
            return Ok(TypeDecl::Bool);
        }

        // `str == str` / `str != str` compares bytes (STR-EQ). The
        // comparison was previously only reachable through unchecked
        // if/while conditions — the checker had no path for it, so a
        // value position (`val x: bool = a == b`) was rejected while
        // the same expression inside an `if` sailed through. First-class
        // here; the lowering emits `InstKind::StrEq` either way.
        if matches!(op, Operator::EQ | Operator::NE)
            && *l == TypeDecl::String
            && *r == TypeDecl::String
        {
            return Ok(TypeDecl::Bool);
        }

        // Same generic parameter on both sides (either canonical
        // form — a bare mention of `K` resolves to `Identifier(K)`
        // while the binding is `Generic(K)`). `Dict::get`'s
        // `existing == key` relies on it, and generic functions are
        // monomorphised before lowering, so the comparison becomes a
        // concrete same-type check at every instantiation. This rule
        // had to exist all along; unchecked if/while conditions were
        // hiding its absence.
        if matches!(op, Operator::EQ | Operator::NE)
            && match (l, r) {
                (TypeDecl::Generic(a), TypeDecl::Generic(b))
                | (TypeDecl::Generic(a), TypeDecl::Identifier(b))
                | (TypeDecl::Identifier(a), TypeDecl::Generic(b))
                | (TypeDecl::Identifier(a), TypeDecl::Identifier(b)) => a == b,
                _ => false,
            }
        {
            return Ok(TypeDecl::Bool);
        }

        if matches!(op, Operator::EQ | Operator::NE)
            && self.is_allocator_compatible(l)
            && self.is_allocator_compatible(r) {
            // Allocator handles support only identity (== / !=), not ordering.
            // A generic parameter bounded by Allocator counts as allocator-compatible
            // so expressions like `current_allocator() == a` type-check inside a
            // `<A: Allocator>` function body.
            return Ok(TypeDecl::Bool);
        }

        if let Some(method_name) = Self::struct_cmp_method_name(op) {
            // Operator overload (Phase B + Phase 2 ext): same-shape
            // struct pair with `eq` / `lt` / `le` / `gt` / `ge`
            // method (`fn ___(&self, other: &Self) -> bool`).
            if self.struct_method_compatible(l, r, method_name) {
                return Ok(TypeDecl::Bool);
            }
        }

        Err(self.error_with_location(
            TypeCheckError::type_mismatch_operation("comparison", l.clone(), r.clone()),
            lhs,
        ))
    }

    /// Result-type rule for `&& ||`: bool only. (No struct overload —
    /// short-circuit semantics are not user-redefinable.)
    fn visit_logical_binary(
        &self,
        lhs: &ExprRef,
        l: &TypeDecl,
        r: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        if *l == TypeDecl::Bool && *r == TypeDecl::Bool {
            Ok(TypeDecl::Bool)
        } else {
            Err(self.error_with_location(
                TypeCheckError::type_mismatch_operation("logical", l.clone(), r.clone()),
                lhs,
            ))
        }
    }

    /// Result-type rule for `& | ^`: u64/i64 same-width pairs, or
    /// struct overload (`bitand` / `bitor` / `bitxor`).
    fn visit_bitwise_binary(
        &self,
        op: &Operator,
        lhs: &ExprRef,
        l: &TypeDecl,
        r: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        if let Some(ty) = Self::same_numeric_pair(l, r) {
            return Ok(ty);
        }

        if let Some(method_name) = Self::struct_self_returning_method_name(op)
            && self.struct_method_compatible(l, r, method_name) {
            return Ok(l.clone());
        }

        Err(self.error_with_location(
            TypeCheckError::type_mismatch_operation("bitwise", l.clone(), r.clone()),
            lhs,
        ))
    }

    /// Result-type rule for `<< >>`: struct overload (`shl` / `shr`)
    /// is checked first so the primitive `rhs must be UInt64` rule
    /// doesn't preempt it. Otherwise rhs must be `UInt64` and lhs
    /// must be `UInt64` / `Int64`; result matches the lhs's signedness.
    fn visit_shift_binary(
        &self,
        op: &Operator,
        lhs: &ExprRef,
        rhs: &ExprRef,
        l: &TypeDecl,
        r: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        if let Some(method_name) = Self::struct_self_returning_method_name(op)
            && self.struct_method_compatible(l, r, method_name) {
                return Ok(l.clone());
            }
        if *r != TypeDecl::UInt64 {
            return Err(self.error_with_location(
                TypeCheckError::type_mismatch_operation("shift", TypeDecl::UInt64, r.clone()),
                rhs,
            ));
        }
        if *l == TypeDecl::UInt64 {
            Ok(TypeDecl::UInt64)
        } else if *l == TypeDecl::Int64 {
            Ok(TypeDecl::Int64)
        } else {
            Err(self.error_with_location(
                TypeCheckError::type_mismatch_operation("shift", l.clone(), TypeDecl::UInt64),
                lhs,
            ))
        }
    }

    /// Type check block expressions
    /// Type check one statement of a block. Split out of `visit_block` so
    /// every `?` inside it lands on the loop's error arm rather than
    /// unwinding past it -- statement-level recovery (LLM-LOOP P1) only
    /// works if the whole statement is one fallible unit.
    fn visit_block_stmt(
        &mut self,
        s: &StmtRef,
        last_empty: &mut bool,
        last: &Option<TypeDecl>,
    ) -> Result<TypeDecl, TypeCheckError> {
        let stmt = self.core.stmt_pool.get(s)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference in block"))?;

        match stmt {
            Stmt::Return(None) => Ok(TypeDecl::Unit),
            Stmt::Return(ret_ty) => {
                if let Some(e) = ret_ty {
                    let expr_obj = self.core.expr_pool.get(&e)
                        .ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference in return"))?;
                    let ty = expr_obj.clone().accept_expr(self)?;
                    self.note_visited_number(&e, &ty);
                    // NUMBER-HINT: `return 0` inside a block names the
                    // enclosing function's return type, same as the
                    // tail expression does.
                    let ty = match self.current_fn_return_type.clone() {
                        Some(fn_ret) => self.coerce_number_expr(&e, &ty, &fn_ret)?,
                        None => ty,
                    };
                    if *last_empty {
                        *last_empty = false;
                        Ok(ty)
                    } else if let Some(last_ty) = last.clone() {
                        if last_ty == ty {
                            Ok(ty)
                        } else {
                            Err(TypeCheckError::type_mismatch(last_ty, ty).with_context("return statement"))
                        }
                    } else {
                        Ok(ty)
                    }
                } else {
                    Ok(TypeDecl::Unit)
                }
            }
            _ => {
                let stmt_obj = self.core.stmt_pool.get(s)
                    .ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference"))?;
                stmt_obj.clone().accept_stmt(self)
            }
        }
    }

    pub fn visit_block(&mut self, statements: &Vec<StmtRef>) -> Result<TypeDecl, TypeCheckError> {
        let mut last_empty = true;
        let mut last: Option<TypeDecl> = None;
        
        // Clear type cache at the start of each block to limit cache scope
        self.optimization.type_cache.clear();
        
        // NUMBER-HINT: the inherited hint is the block's numeric
        // context. A pre-scan for the first annotated `val` in the
        // body used to fill it in when unset, which let one binding's
        // annotation retype unrelated literals elsewhere in the
        // block; positions now claim their own.
        let original_hint = self.type_inference.type_hint.clone();

        // Process each statement
        // This code assumes Block(expression) don't make nested function
        // so `return` expression always return for this context.
        for s in statements.iter() {
            match self.visit_block_stmt(s, &mut last_empty, &last) {
                Ok(def_ty) => last = Some(def_ty),
                // LLM-LOOP P1: record and move on to the next statement so
                // one bad statement doesn't hide the rest of the block.
                Err(e) if self.recovery_enabled => {
                    self.recover_stmt_error(s, e);
                    last = Some(TypeDecl::Unknown);
                }
                Err(e) => return Err(e),
            }
        }
        
        // Restore original type hint
        self.type_inference.type_hint = original_hint;

        if let Some(last_type) = last {
            Ok(last_type)
        } else {
            Err(TypeCheckError::generic_error("Empty block - no return value"))
        }
    }

    /// Type check if-elif-else expressions
    pub fn visit_if_elif_else(&mut self, cond: &ExprRef, then_block: &ExprRef, elif_pairs: &Vec<(ExprRef, ExprRef)>, else_block: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let mut block_types = Vec::new();

        // The condition must be a bool (R4-era fix; it used to be
        // unchecked, so `if 42u64 { ... }` sailed through and the
        // condition's expressions were never validated at all).
        let cond_ty = self.check_expr_located(cond)?;
        if cond_ty != TypeDecl::Bool {
            return Err(self.error_with_location(
                TypeCheckError::type_mismatch(TypeDecl::Bool, cond_ty),
                cond,
            ));
        }
        for (elif_cond, _) in elif_pairs {
            let elif_ty = self.check_expr_located(elif_cond)?;
            if elif_ty != TypeDecl::Bool {
                return Err(self.error_with_location(
                    TypeCheckError::type_mismatch(TypeDecl::Bool, elif_ty),
                    elif_cond,
                ));
            }
        }

        // Check if-block
        let if_block = *then_block;
        let is_if_empty = match self.core.expr_pool.get(&if_block)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid if block expression reference"))? {
            Expr::Block(expressions) => expressions.is_empty(),
            _ => false,
        };
        if !is_if_empty {
            let if_expr = self.core.expr_pool.get(&if_block)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid if block expression reference"))?;
            let if_ty = if_expr.clone().accept_expr(self)?;
            block_types.push(if_ty);
        }

        // Check elif-blocks
        for (_, elif_block) in elif_pairs {
            let elif_block = *elif_block;
            let is_elif_empty = match self.core.expr_pool.get(&elif_block)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid elif block expression reference"))? {
                Expr::Block(expressions) => expressions.is_empty(),
                _ => false,
            };
            if !is_elif_empty {
                let elif_expr = self.core.expr_pool.get(&elif_block)
                    .ok_or_else(|| TypeCheckError::generic_error("Invalid elif block expression reference"))?;
                let elif_ty = elif_expr.clone().accept_expr(self)?;
                block_types.push(elif_ty);
            }
        }

        // Check else-block
        let else_block = *else_block;
        let is_else_empty = match self.core.expr_pool.get(&else_block)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid else block expression reference"))? {
            Expr::Block(expressions) => expressions.is_empty(),
            _ => false,
        };
        if !is_else_empty {
            let else_expr = self.core.expr_pool.get(&else_block)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid else block expression reference"))?;
            let else_ty = else_expr.clone().accept_expr(self)?;
            block_types.push(else_ty);
        }

        // If no blocks have values or all blocks are empty, return Unit
        if block_types.is_empty() {
            return Ok(TypeDecl::Unit);
        }

        // Pick the first concrete (non-Unknown) branch type as the result;
        // Unknown branches (e.g. ones ending in `panic("...")`) unify with
        // any concrete sibling. If every branch is Unknown the if-expression
        // itself is Unknown — the surrounding context resolves it.
        let result_ty = block_types.iter()
            .find(|t| **t != TypeDecl::Unknown)
            .cloned()
            .unwrap_or(TypeDecl::Unknown);
        for block_type in &block_types {
            if *block_type != TypeDecl::Unknown && !block_type.is_equivalent(&result_ty) {
                return Ok(TypeDecl::Unit); // Different types, return Unit
            }
        }

        Ok(result_ty)
    }

    /// CLOSURE-CAPTURE E1/E2: is this assignment target a binding the
    /// enclosing closure captured, or a path into one?
    ///
    /// Returns `(target, root)` — what was written and the captured
    /// binding it reaches — or `None` when no closure body is open,
    /// the root is local to the closure, or the target is not rooted
    /// in a plain binding at all (`f().x = ...`).
    ///
    /// The root is what matters: a capture is one binding, and
    /// reaching into it does not change which binding is being
    /// written. Rendering the whole path is only so the message can
    /// quote what the author wrote.
    pub(crate) fn captured_assign_target(&self, lhs: &ExprRef) -> Option<(String, String)> {
        let mut path: Vec<String> = Vec::new();
        let mut cursor = *lhs;
        loop {
            match self.core.expr_pool.get(&cursor)? {
                Expr::Identifier(name) => {
                    // E3: a shared capture is written like any other
                    // binding, so only a copied one is reported.
                    if self.context.capture_of_open_closure(name) != Some(false) {
                        return None;
                    }
                    let root = self.resolve_symbol_name(name);
                    let mut target = root.clone();
                    for segment in path.iter().rev() {
                        target.push_str(segment);
                    }
                    return Some((target, root));
                }
                Expr::FieldAccess(obj, field) => {
                    path.push(format!(".{}", self.resolve_symbol_name(field)));
                    cursor = obj;
                }
                Expr::TupleAccess(obj, index) => {
                    path.push(format!(".{index}"));
                    cursor = obj;
                }
                Expr::SliceAccess(obj, _) => {
                    path.push("[..]".to_string());
                    cursor = obj;
                }
                // Anything else (a call, a literal) is not rooted in a
                // binding, so there is no capture to speak of.
                _ => return None,
            }
        }
    }

    /// Type check assignment expressions
    pub fn visit_assign(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let lhs = *lhs;
        let rhs = *rhs;

        // Reject assignment to an immutable (`val`) binding at type-check
        // time. The tree-walker enforces this at runtime; hoisting it to
        // the frontend makes every backend (interpreter / compiler / IR VM)
        // reject it uniformly, before execution. Only bare-identifier
        // targets are checked here — field / slice assignment mutability is
        // handled on their own paths.
        if let Some(Expr::Identifier(name)) = self.core.expr_pool.get(&lhs) {
            // CLOSURE-CAPTURE E1: a write to a binding the closure
            // copied reaches nothing, so it is an error rather than a
            // silently discarded store. Checked *before* the `val`
            // rule because that rule's advice ("use `var`") would be a
            // dead end for a copy — a captured `var` cannot be written
            // either, so the author would fix one error into another.
            //
            // E3: a closure that shares its captures is exempt. The
            // write reaches the outer binding, so the only question
            // left is the ordinary one below — whether that binding
            // is mutable, where "use `var`" is the right advice again.
            if self.context.capture_of_open_closure(name) == Some(false) {
                let name_str = self.resolve_symbol_name(name);
                // Anchored on the target rather than left for the
                // statement-level recovery to place, which lands the
                // caret on whatever the block's tail expression is.
                return Err(self.error_with_location(
                    TypeCheckError::captured_assign(name_str.clone(), name_str),
                    &lhs,
                ));
            }
            if self.context.is_var_mutable(name) == Some(false) {
                let name_str = self.resolve_symbol_name(name);
                return Err(TypeCheckError::generic_error(&format!(
                    "cannot assign to `{name_str}`: binding is immutable (declared with `val`; use `var` to allow reassignment)"
                )));
            }
        }

        // CLOSURE-CAPTURE E2: the same rule for a write *through* a
        // capture (`p.x = ...`). Without it the shape of the value
        // decided the semantics — a captured compound keeps its `Rc`
        // cell, so the interpreter engines let the write reach the
        // outer binding while a bare rebind of a scalar was discarded,
        // and the compiled engines could not build either.
        if let Some((target, root)) = self.captured_assign_target(&lhs) {
            return Err(self.error_with_location(
                TypeCheckError::captured_assign(target, root),
                &lhs,
            ));
        }

        let lhs_ty = {
            let lhs_obj = self.core.expr_pool.get(&lhs)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid left-hand expression reference"))?;
            lhs_obj.clone().accept_expr(self)?
        };
        
        // Located: an error raised inside the right-hand side (an
        // unrunnable literal, a bad call) otherwise reached the
        // statement-level recovery with no location of its own, and
        // was then anchored on whatever statement came next.
        let rhs_ty = self.check_expr_located(&rhs)?;
        // NUMBER-HINT: the assignment target's type names what an
        // unsuffixed literal on the right should become, so `x = 5`
        // works for an `i64` binding without a suffix.
        let rhs_ty = self.coerce_number_expr(&rhs, &rhs_ty, &lhs_ty)?;
        // Allow assignment compatibility. `is_equivalent` covers the
        // user-named-type cases the parser emits ambiguously
        // (`Identifier(name)` vs `Enum(name, _)` / `Struct(name, _)`),
        // so a `var b: Box = Box::Filled(42u64)` form does not
        // false-positive even though the bare `==` comparison would.
        if !lhs_ty.is_equivalent(&rhs_ty) {
            match (&lhs_ty, &rhs_ty) {
                // Allow unknown type (null values) assignment to any concrete type
                (_, TypeDecl::Unknown) => {
                    // Allow assignment of unknown/null to any type
                }
                // Allow assignment when types are equivalent (for type inference)
                (TypeDecl::Unknown, _) => {
                    // Allow assignment from any type to unknown (type inference)
                }
                _ => {
                    return Err(TypeCheckError::type_mismatch(lhs_ty, rhs_ty).with_context("assignment"));
                }
            }
        }
        Ok(lhs_ty)
    }

    /// Type check identifiers
    pub fn visit_identifier(&mut self, name: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        if let Some(val_type) = self.context.get_var(name) {
            // Return the stored type, which may be Number for type inference.
            // REF-Stage-2 (g): auto-dereference reference bindings in value
            // position. Reads of a `&mut T` / `&T` parameter behave as
            // reads of `T`; the lowering layer emits the LoadRef for the
            // scalar pointee. Method dispatch (`obj.method`) and explicit
            // borrow (`&x`) sit on different paths so their type-checker
            // arms don't see the auto-deref and can still inspect the
            // ref-ness directly. Forwarding a ref binding to a `&mut T`
            // parameter today requires `&mut x` — which is rejected on
            // a `&mut T` binding because the operand isn't a `var`-
            // declared local; a future phase can add ref forwarding.
            let _name_str = self.resolve_symbol_name(name);
            if let TypeDecl::Ref { inner, .. } = &val_type {
                return Ok((**inner).clone());
            }
            Ok(val_type.clone())
        } else if let Some(fun) = self.context.get_fn(name) {
            Ok(fun.return_type.clone().unwrap_or(TypeDecl::Unknown))
        } else if let Some(generic_type) = self.type_inference.lookup_generic_type(name) {
            // Check if this is a generic type parameter
            Ok(generic_type.clone())
        } else if let Some(_struct_def) = self.context.get_struct_definition(name) {
            // Check if this is a struct type
            // If the struct has generic parameters, include them
            let type_params = if let Some(generic_params) = self.context.get_struct_generic_params(name) {
                generic_params.iter().map(|param| {
                    // Try to resolve from current generic scope, otherwise use Generic type
                    self.type_inference.lookup_generic_type(*param)
                        .unwrap_or(TypeDecl::Generic(*param))
                }).collect()
            } else {
                vec![]
            };
            Ok(TypeDecl::Struct(name, type_params))
        } else {
            let name_str = self.resolve_symbol_name(name);
            // Note: Location information will be added by visit_expr
            Err(TypeCheckError::not_found("Identifier", &name_str))
        }
    }

    /// Whether `ty` can participate in an Allocator equality comparison — either
    /// the concrete Allocator type or a generic parameter bounded by Allocator.
    fn is_allocator_compatible(&self, ty: &TypeDecl) -> bool {
        match ty {
            TypeDecl::Allocator => true,
            TypeDecl::Generic(sym) => matches!(
                self.context.current_fn_generic_bounds.get(sym),
                Some(TypeDecl::Allocator)
            ),
            _ => false,
        }
    }

    /// Whether `lhs` and `rhs` are the same struct type and that struct
    /// has `method_name` registered — the Phase B operator-overload test.
    ///
    /// `==` was the first operator to use it, dispatching `s == t` to
    /// `s.eq(t)` for nominal structs like `String` / `Vec<u8>`; the
    /// arithmetic operators followed, with `+` / `-` / `*` / `/` / `%`
    /// going to `add` / `sub` / `mul` / `div` / `rem`.
    ///
    /// The nominal-identity rule is the same either way: same struct name
    /// *and* same generic args, so `Vec<u8> == Vec<u8>` compares while
    /// `Vec<u8> == Vec<i64>` falls through to the standard mismatch
    /// diagnostic.
    fn struct_method_compatible(
        &self,
        lhs: &TypeDecl,
        rhs: &TypeDecl,
        method_name: &str,
    ) -> bool {
        // Both `Struct(name, args)` and `Identifier(name)`
        // (= bare struct name pre-canonicalisation) are accepted —
        // the type checker hands us the latter for non-generic
        // struct values that bypass `Struct(...)` canonical form.
        // Treating them uniformly lets `Vec3 + Vec3` reach the
        // dispatch even when both operands carry the Identifier
        // shape.
        let extract = |t: &TypeDecl| -> Option<(DefaultSymbol, Vec<TypeDecl>)> {
            match t {
                TypeDecl::Struct(name, args) => Some((*name, args.clone())),
                TypeDecl::Identifier(name) => Some((*name, Vec::new())),
                _ => None,
            }
        };
        let (lhs_name, lhs_args) = match extract(lhs) {
            Some(x) => x,
            None => return false,
        };
        let (rhs_name, rhs_args) = match extract(rhs) {
            Some(x) => x,
            None => return false,
        };
        if lhs_name != rhs_name || lhs_args != rhs_args {
            return false;
        }
        let method_sym = match self.core.string_interner.get(method_name) {
            Some(s) => s,
            None => return false,
        };
        // CONCRETE-IMPL-Phase-2c: dispatch the overload check against
        // the *receiver's* concrete type args, so `a + b` on two
        // `C<u8>` values checks the `impl C<u8>` spec and a `C<i64>`
        // pair the `impl C<i64>` one — not whichever impl registered
        // last.
        self.context.get_struct_method(lhs_name, method_sym, &lhs_args).is_some()
    }

    /// Returns the inherent method name an arithmetic operator
    /// overloads to (`+` -> `add`, etc.). Used by both the type
    /// checker (compatibility check) and the interpreter / AOT
    /// dispatchers (method lookup). Mirrors Rust's `std::ops`
    /// trait method names.
    pub(crate) fn struct_arith_method_name(op: &Operator) -> Option<&'static str> {
        Self::struct_self_returning_method_name(op)
    }

    /// Self-returning binary operator overload table. Covers the
    /// arithmetic ops (Phase OP-OVERLOAD-ARITH) and the bitwise +
    /// shift ops (Phase 3 extension). All return `Self` (the same
    /// nominal struct), unlike the comparison family which return
    /// `bool`. Method names mirror Rust's `std::ops::*` traits.
    pub(crate) fn struct_self_returning_method_name(op: &Operator) -> Option<&'static str> {
        match op {
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
        }
    }

    /// Comparison-operator method-name table (Phase B + Phase 2
    /// extension). Mirrors Rust's `PartialEq` / `PartialOrd`
    /// trait method names. All return `bool`.
    pub(crate) fn struct_cmp_method_name(op: &Operator) -> Option<&'static str> {
        match op {
            Operator::EQ => Some("eq"),
            Operator::NE => Some("eq"), // routes through eq + negate
            Operator::LT => Some("lt"),
            Operator::LE => Some("le"),
            Operator::GT => Some("gt"),
            Operator::GE => Some("ge"),
            _ => None,
        }
    }

    /// If the call to `fun` omits trailing Allocator-typed parameters, extend the
    /// argument `ExprList` with synthetic `__builtin_current_allocator()` calls so
    /// downstream type checking and interpretation see the defaults. A parameter
    /// is considered defaultable when its declared type is `TypeDecl::Allocator`
    /// or a generic parameter bounded by `Allocator`. Only trailing positions are
    /// filled; once a non-defaultable parameter is reached the rest is left alone
    /// so the existing arity-mismatch error path still triggers.
    fn inject_ambient_defaults(&mut self, args_ref: &ExprRef, fun: &Function) {
        let args = match self.core.expr_pool.get(args_ref) {
            Some(Expr::ExprList(args)) => args,
            _ => return,
        };
        if args.len() >= fun.parameter.len() {
            return;
        }
        let mut extended = args.clone();
        for (_, param_ty) in fun.parameter.iter().skip(extended.len()) {
            let is_defaultable = match param_ty {
                TypeDecl::Allocator => true,
                TypeDecl::Generic(sym) => matches!(
                    fun.generic_bounds.get(sym),
                    Some(TypeDecl::Allocator)
                ),
                _ => false,
            };
            if !is_defaultable {
                break;
            }
            let ambient_call = Expr::BuiltinCall(
                crate::ast::BuiltinFunction::CurrentAllocator,
                vec![],
            );
            let expr_ref = self.core.expr_pool.add(ambient_call);
            extended.push(expr_ref);
        }
        if extended.len() > args.len() {
            self.core.expr_pool.update(args_ref, Expr::ExprList(extended));
        }
    }

    /// Type check function calls.
    ///
    /// Orchestrator: namespace enforcement → lookup → dispatch (generic /
    /// direct / indirect). Per-path details live in the helpers below.
    pub fn visit_call(&mut self, fn_name: DefaultSymbol, args_ref: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // Note: imported functions are now callable via bare name.
        // `lookup_fn(None, name)` already prefers user-authored functions
        // and falls back to a unique imported entry, so namespace
        // enforcement has been relaxed.

        // Lexical scoping: a function-typed local binding (a closure
        // parameter, or a `val f = fn(...) -> R { ... }`) shadows a
        // top-level function of the same name. The global table must not
        // win here -- consulting it first lets a user-defined `fn f(..)`
        // hijack stdlib call sites such as the `f(v)` in
        // `core/std/option.t::map`, which call a closure parameter rather
        // than a global. Non-function locals (`val print = 3u64`) do not
        // shadow, so ordinary calls keep resolving to the global.
        if matches!(self.context.get_var(fn_name), Some(TypeDecl::Function(_, _))) {
            return self.visit_call_indirect_fallback(fn_name, args_ref);
        }

        self.push_context();

        if let Some(fun) = self.context.get_fn(fn_name) {
            if let Err(err) = self.check_function_access(&fun) {
                self.pop_context();
                return Err(err);
            }

            // Auto-inject `ambient` for omitted trailing Allocator-typed parameters.
            // Injection happens before the generic-call dispatch so both paths see
            // the extended argument list.
            self.inject_ambient_defaults(args_ref, &fun);

            if !fun.generic_params.is_empty() {
                return self.visit_generic_call(fn_name, args_ref, &fun);
            }

            self.type_check_forward_ref(fn_name)?;

            if let Err(err) = self.check_call_args_against_params(fn_name, args_ref, &fun) {
                self.pop_context();
                return Err(err);
            }

            self.pop_context();
            Ok(self.normalize_call_return_type(fun.return_type.clone().unwrap_or(TypeDecl::Unknown)))
        } else {
            self.pop_context();
            // NEWTYPE: `Meters(v)` parses as a call; a tuple struct of
            // that name takes over only when no function or
            // function-typed local answers to it, so an `fn Meters(..)`
            // keeps winning.
            if let Some(ty) = self.check_tuple_struct_construction(fn_name, args_ref) {
                return ty;
            }
            self.visit_call_indirect_fallback(fn_name, args_ref)
        }
    }

    /// Type-check a function that hasn't been visited yet (forward
    /// reference). No-op when the function is already checked.
    fn type_check_forward_ref(&mut self, fn_name: DefaultSymbol) -> Result<(), TypeCheckError> {
        let status = self.function_checking.is_checked_fn.get(&fn_name);
        if status.is_none() || status.as_ref().and_then(|s| s.as_ref()).is_none() {
            let fun_copy = self.context.get_fn(fn_name)
                .ok_or_else(|| TypeCheckError::not_found("Function", "<INTERNAL_ERROR>"))?;
            self.type_check(fun_copy.clone())?;
        }
        Ok(())
    }

    /// When `visit_call` can't find a function declaration, try the
    /// indirect-call (closure-value) path. Returns `not_found` when
    /// the identifier is neither a function nor a function-typed value.
    fn visit_call_indirect_fallback(
        &mut self,
        fn_name: DefaultSymbol,
        args_ref: &ExprRef,
    ) -> Result<TypeDecl, TypeCheckError> {
        if let Some(callee_ty) = self.context.get_var(fn_name)
            && let TypeDecl::Function(param_tys, ret_ty) = callee_ty {
                return self.visit_indirect_call(fn_name, args_ref, &param_tys, &ret_ty);
            }
        let fn_name_str = self.resolve_symbol_name(fn_name);
        // ALLOC-CONTRACT: `old(...)` is a contextual form the parser
        // only recognises inside an `ensures` clause, so writing it
        // anywhere else arrives here as an ordinary missing function.
        // Say what it is instead of letting the reader hunt for a
        // function they never wrote.
        if fn_name_str == "old" {
            return Err(TypeCheckError::generic_error(
                "`old(...)` is only meaningful in an `ensures` clause: it snapshots \
                 the value an expression had on entry to the function",
            ));
        }
        let error = TypeCheckError::not_found("Function", &fn_name_str);
        Err(self.suggest_known_function_name(error, &fn_name_str))
    }

    /// LLM-LOOP P3: attach a "did you mean" replacement when exactly one
    /// declared function is a near-miss for `name`. Built here because
    /// this is where the candidate set lives.
    ///
    /// `closest_candidate` declines on ties, so a typo sitting between
    /// two real names produces no suggestion at all -- picking one would
    /// send the reader to the wrong function with full confidence.
    fn suggest_known_function_name(
        &self,
        mut error: TypeCheckError,
        name: &str,
    ) -> TypeCheckError {
        let candidates: Vec<&str> = self
            .context
            .functions
            .keys()
            .filter_map(|(_, sym)| self.core.string_interner.resolve(*sym))
            .collect();
        let Some(best) = crate::diagnostic::closest_candidate(name, candidates) else {
            return error;
        };
        // The call site's span is stamped onto this error further up the
        // stack, so the suggestion targets the diagnostic's own span
        // rather than one resolved here.
        error.suggestions.push(crate::diagnostic::Suggestion::over_primary_span(
            &format!("a function named `{best}` exists"),
            best.to_string(),
        ));
        error
    }

    /// Type-check the argument list of a non-generic direct call
    /// against a resolved `Function`. Extracted from `visit_call` so
    /// the orchestrator stays focused on lookup + dispatch. The
    /// caller is responsible for `pop_context` on the way out
    /// (success or failure); this helper restores `type_hint` on
    /// every return path.
    fn check_call_args_against_params(
        &mut self,
        fn_name: DefaultSymbol,
        args_ref: &ExprRef,
        fun: &Function,
    ) -> Result<(), TypeCheckError> {
        // Pull the argument list. A non-ExprList is an internal IR drift
        // (parser only produces ExprList here); a missing slot means
        // the args ref dangles.
        let args = match self.core.expr_pool.get(args_ref) {
            Some(Expr::ExprList(args)) => args.clone(),
            Some(_) => return Ok(()),
            None => return Err(TypeCheckError::generic_error("Invalid arguments reference")),
        };

        // Normalize Identifier params to Struct for known struct types
        // so the per-arg compatibility check below sees the canonical
        // shape regardless of how the user spelled the parameter type.
        let param_types: Vec<_> = fun.parameter.iter().map(|(_, ty)| {
            if let TypeDecl::Identifier(name) = ty
                && self.context.struct_definitions.contains_key(name) {
                    return TypeDecl::Struct(*name, vec![]);
                }
            ty.clone()
        }).collect();

        if args.len() != param_types.len() {
            let fn_name_str = self.resolve_symbol_name(fn_name);
            return Err(TypeCheckError::generic_error(&format!(
                "Function '{}' argument count mismatch: expected {}, found {}",
                fn_name_str, param_types.len(), args.len()
            )));
        }

        // Type-check each argument with the parameter type as the hint
        // so Number literals resolve to the expected concrete type.
        let original_hint = self.type_inference.type_hint.clone();
        for (arg_index, (arg, expected_type)) in args.iter().zip(&param_types).enumerate() {
            self.type_inference.type_hint = Some(expected_type.clone());
            let arg_type = match self.visit_expr(arg) {
                Ok(t) => t,
                Err(e) => {
                    self.type_inference.type_hint = original_hint;
                    return Err(e);
                }
            };
            // NUMBER-HINT: the parameter type is what an unsuffixed
            // literal argument should become. `f(21)` for
            // `fn f(x: i64)` used to be rejected as `u64`.
            let arg_type = match self.coerce_number_expr(arg, &arg_type, expected_type) {
                Ok(t) => t,
                Err(e) => {
                    self.type_inference.type_hint = original_hint;
                    return Err(e);
                }
            };
            // `is_arg_compatible_dyn_aware` extends the context-free
            // helper with A5 `&Struct → &dyn Trait` coercion (struct
            // must implement the trait); the underlying helper
            // covers Identifier↔Struct / Identifier↔Enum plus the
            // REF-Stage-2 auto-borrow (`T` → `&T`).
            if !self.is_arg_compatible_dyn_aware(&arg_type, expected_type) && arg_type != TypeDecl::Unknown {
                self.type_inference.type_hint = original_hint;
                let fn_name_str = self.resolve_symbol_name(fn_name);
                // LLM-LOOP P2: anchor at the offending argument. Without
                // this the error inherits the call's location and points
                // at the callee, which says nothing about *which*
                // argument to change.
                // A wrong argument type is a type mismatch, so it gets
                // E0001 rather than the E0010 catch-all it used to fall
                // into -- it is one of the most common errors in the
                // language, and a code the reader can look up is the
                // point of having codes at all.
                let err = TypeCheckError::type_mismatch(expected_type.clone(), arg_type.clone())
                    .with_context(&format!(
                        "argument {} of function '{}'",
                        arg_index + 1,
                        fn_name_str
                    ));
                let err = self.error_with_location(err, arg);
                return Err(self.suggest_numeric_cast(err, arg, &arg_type, expected_type));
            }
        }
        self.type_inference.type_hint = original_hint;
        Ok(())
    }

    /// Normalize a function's declared return type. Bare
    /// `Identifier(name)` for known structs is rewritten to
    /// `Struct(name, [])` so downstream method dispatch (which
    /// matches on `Struct`) works on values produced by
    /// `fn make_list() -> List { ... }`.
    fn normalize_call_return_type(&self, ret: TypeDecl) -> TypeDecl {
        if let TypeDecl::Identifier(name) = &ret
            && self.context.struct_definitions.contains_key(name) {
                return TypeDecl::Struct(*name, vec![]);
            }
        ret
    }

    /// Closures Phase 2: type check a call site whose callee is a
    /// function-typed value (e.g. a `val f = fn(...) -> R { ... }`
    /// binding or a parameter of `(T1, T2) -> R` type). Mirrors the
    /// argument validation arm of `visit_call` but without the
    /// generic-monomorphisation / visibility-check / pre-typecheck
    /// machinery, none of which apply to a value.
    fn visit_indirect_call(
        &mut self,
        callee_name: DefaultSymbol,
        args_ref: &ExprRef,
        param_tys: &[TypeDecl],
        ret_ty: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        let args_data = match self.core.expr_pool.get(args_ref) {
            Some(Expr::ExprList(args)) => args.clone(),
            _ => return Err(TypeCheckError::generic_error("Invalid arguments reference")),
        };
        if args_data.len() != param_tys.len() {
            let name_str = self.resolve_symbol_name(callee_name);
            return Err(TypeCheckError::generic_error(&format!(
                "function value '{}' argument count mismatch: expected {}, found {}",
                name_str,
                param_tys.len(),
                args_data.len()
            )));
        }
        let original_hint = self.type_inference.type_hint.clone();
        for (idx, (arg, expected)) in args_data.iter().zip(param_tys.iter()).enumerate() {
            self.type_inference.type_hint = Some(expected.clone());
            let arg_ty = self.visit_expr(arg)?;
            // NUMBER-HINT: same rule as a direct call — the parameter
            // type names what an unsuffixed literal should become.
            let arg_ty = self.coerce_number_expr(arg, &arg_ty, expected)?;
            if !self.is_arg_compatible_dyn_aware(&arg_ty, expected) && arg_ty != TypeDecl::Unknown {
                self.type_inference.type_hint = original_hint;
                let name_str = self.resolve_symbol_name(callee_name);
                return Err(TypeCheckError::generic_error(&format!(
                    "Type error: expected {:?}, found {:?}. Function value '{}' argument {} type mismatch",
                    expected,
                    arg_ty,
                    name_str,
                    idx + 1
                )));
            }
        }
        self.type_inference.type_hint = original_hint;
        Ok(ret_ty.clone())
    }

    /// Closures Phase 2: type check a closure / lambda literal
    /// `fn(params) -> Ret { body }`.
    ///
    /// Walks the body under a fresh scope that binds each declared
    /// parameter, then validates the body type against the optional
    /// declared return type. Returns the resulting
    /// `TypeDecl::Function(param_tys, ret_ty)`.
    ///
    /// Capture analysis: after the body type-checks, walk it once to
    /// collect identifier references not bound by the closure's own
    /// parameter scope. Each capture is recorded in
    /// `context.closure_captures` keyed by the closure's `ExprRef`.
    /// Captures whose type carries an enclosing function's generic
    /// parameter are rejected up front — generic-parameterised
    /// closures are deferred to a future phase.
    pub fn visit_closure_impl(
        &mut self,
        params: &ParameterList,
        return_type: &Option<TypeDecl>,
        body: &ExprRef,
    ) -> Result<TypeDecl, TypeCheckError> {
        // Generic-param leakage in the closure signature is rejected
        // up front (captures get the same check below, after the body
        // type-checks).
        Self::reject_generic_in_closure_signature(params, return_type)?;

        // Push a fresh scope and bind each parameter. CLOSURE-CAPTURE
        // E1: remember which scope that is, so an assignment in the
        // body can tell a local apart from a capture. The body is
        // checked on top of the *enclosing* scope (that is how a
        // capture's type is looked up), so the depth is the only
        // thing that separates them.
        self.push_context();
        let by_ref = self.context.closure_by_ref_bodies.contains(body);
        self.context
            .closure_scope_floors
            .push(crate::type_checker::context::ClosureFrame {
                floor: self.context.vars.len() - 1,
                by_ref,
            });
        for (name, ty) in params {
            self.context.set_var(*name, ty.clone());
        }
        let body_result = self.visit_expr(body);
        self.context.closure_scope_floors.pop();
        let body_ty = match body_result {
            Ok(ty) => {
                // NUMBER-HINT: the closure's declared return type is
                // what an unsuffixed literal in its body should
                // become, exactly as a function's is. Coerced before
                // `pop_context` so the scope the body was checked in
                // is still open.
                let coerced = match return_type {
                    Some(declared) => self.coerce_number_expr(body, &ty, declared),
                    None => Ok(ty),
                };
                self.pop_context();
                coerced?
            }
            Err(e) => {
                self.pop_context();
                return Err(e);
            }
        };

        // Validate body type against the declared return type when
        // present; otherwise the body type drives the inferred return.
        let ret_ty = match return_type {
            Some(declared) => {
                if !TypeDecl::is_arg_compatible(&body_ty, declared)
                    && body_ty != TypeDecl::Unknown
                {
                    return Err(TypeCheckError::generic_error(&format!(
                        "closure body returns {:?} but declared return type is {:?}",
                        body_ty, declared
                    )));
                }
                declared.clone()
            }
            None => body_ty,
        };

        // Capture analysis is also a side-effect (records into
        // `context.closure_captures`); see helper for details.
        self.record_closure_captures(params, body)?;

        let param_tys: Vec<_> = params.iter().map(|(_, t)| t.clone()).collect();
        Ok(TypeDecl::Function(param_tys, Box::new(ret_ty)))
    }

    /// Closures Phase 2: reject any `TypeDecl::Generic(_)` that
    /// reaches the closure signature. The body's type would then
    /// depend on the enclosing function's generic params, which the
    /// MVP can't lower into either an independent function value or
    /// a monomorphic instantiation. Captures are checked separately
    /// in `record_closure_captures` after the body type-checks.
    fn reject_generic_in_closure_signature(
        params: &ParameterList,
        return_type: &Option<TypeDecl>,
    ) -> Result<(), TypeCheckError> {
        for (_, ty) in params {
            if Self::type_mentions_any_generic(ty) {
                return Err(TypeCheckError::generic_error(
                    "generic-parameterised closures are not yet supported",
                ));
            }
        }
        if let Some(ret) = return_type
            && Self::type_mentions_any_generic(ret) {
                return Err(TypeCheckError::generic_error(
                    "generic-parameterised closures are not yet supported",
                ));
            }
        Ok(())
    }

    /// Closures Phase 2: walk the body to enumerate identifiers that
    /// are not bound by the closure's own parameter scope. The body
    /// type-check has already proven each free identifier resolves
    /// somewhere on the enclosing stack, so each capture's type can
    /// be looked up directly. Captures whose type still mentions an
    /// enclosing generic param are rejected here (the signature
    /// check earlier doesn't see them). Records the result into
    /// `context.closure_captures` keyed by the body's `ExprRef`.
    fn record_closure_captures(
        &mut self,
        params: &ParameterList,
        body: &ExprRef,
    ) -> Result<(), TypeCheckError> {
        let bound: std::collections::HashSet<DefaultSymbol> =
            params.iter().map(|(n, _)| *n).collect();
        let mut captures: Vec<(DefaultSymbol, TypeDecl)> = Vec::new();
        let mut seen: std::collections::HashSet<DefaultSymbol> =
            std::collections::HashSet::new();
        self.collect_closure_free_vars(*body, &bound, &mut captures, &mut seen);

        for (_, ty) in &captures {
            if Self::type_mentions_any_generic(ty) {
                return Err(TypeCheckError::generic_error(
                    "generic-parameterised closures are not yet supported",
                ));
            }
        }

        // Side-table key is the body's ExprRef — unique per closure
        // even when the trait `visit_closure` doesn't have access to
        // the closure's own ExprRef.
        self.context.closure_captures.insert(*body, captures);
        Ok(())
    }

    /// Returns true when `ty` mentions any `TypeDecl::Generic(_)`
    /// placeholder anywhere in its tree. Walks compound shapes
    /// (Array / Tuple / Dict / Struct / Enum / Range / Ref /
    /// Function). Used by the closure type checker to reject
    /// signatures and captures that depend on an enclosing generic
    /// parameter — generic-parameterised closures are deferred.
    fn type_mentions_any_generic(ty: &TypeDecl) -> bool {
        match ty {
            TypeDecl::Generic(_) => true,
            TypeDecl::Array(elems, _) | TypeDecl::Tuple(elems) => {
                elems.iter().any(Self::type_mentions_any_generic)
            }
            TypeDecl::Dict(k, v) => {
                Self::type_mentions_any_generic(k) || Self::type_mentions_any_generic(v)
            }
            TypeDecl::Struct(_, args) | TypeDecl::Enum(_, args) => {
                args.iter().any(Self::type_mentions_any_generic)
            }
            TypeDecl::Range(t) => Self::type_mentions_any_generic(t),
            TypeDecl::Ref { inner, .. } => Self::type_mentions_any_generic(inner),
            TypeDecl::Function(params, ret) => {
                params.iter().any(Self::type_mentions_any_generic)
                    || Self::type_mentions_any_generic(ret)
            }
            _ => false,
        }
    }

    /// Walk an expression tree looking for `Expr::Identifier` /
    /// `Expr::Assign(Identifier, _)` / `Expr::Call(name, _)` references
    /// to symbols that are NOT in `bound`. Each matched name is looked
    /// up in the current type-checker context (which still holds the
    /// outer scope when this is called from `visit_closure_impl`) and
    /// recorded in `out` with its current type. Already-recorded
    /// symbols are skipped via `seen`. Nested closures extend the
    /// `bound` set with their own params.
    fn collect_closure_free_vars(
        &self,
        expr_ref: ExprRef,
        bound: &std::collections::HashSet<DefaultSymbol>,
        out: &mut Vec<(DefaultSymbol, TypeDecl)>,
        seen: &mut std::collections::HashSet<DefaultSymbol>,
    ) {
        let expr = match self.core.expr_pool.get(&expr_ref) {
            Some(e) => e,
            None => return,
        };
        let record = |s: DefaultSymbol,
                          out: &mut Vec<(DefaultSymbol, TypeDecl)>,
                          seen: &mut std::collections::HashSet<DefaultSymbol>| {
            if bound.contains(&s) || seen.contains(&s) {
                return;
            }
            // Only record names that actually resolve to a variable in
            // the enclosing scope. Function names, struct names, and
            // builtin symbols intentionally skip the capture set.
            if let Some(ty) = self.context.get_var(s) {
                seen.insert(s);
                out.push((s, ty));
            }
        };
        match expr {
            Expr::Identifier(s) => record(s, out, seen),
            Expr::Call(name, args_ref) => {
                record(name, out, seen);
                self.collect_closure_free_vars(args_ref, bound, out, seen);
            }
            Expr::Assign(lhs, rhs) => {
                self.collect_closure_free_vars(lhs, bound, out, seen);
                self.collect_closure_free_vars(rhs, bound, out, seen);
            }
            Expr::Binary(_, l, r)
            | Expr::Range(l, r)
            | Expr::With(l, r)
            | Expr::IfElifElse(l, r, _, _) => {
                self.collect_closure_free_vars(l, bound, out, seen);
                self.collect_closure_free_vars(r, bound, out, seen);
                if let Expr::IfElifElse(_, _, elif_pairs, else_block) = expr.clone() {
                    for (c, b) in elif_pairs {
                        self.collect_closure_free_vars(c, bound, out, seen);
                        self.collect_closure_free_vars(b, bound, out, seen);
                    }
                    self.collect_closure_free_vars(else_block, bound, out, seen);
                }
            }
            Expr::Unary(_, operand) => {
                self.collect_closure_free_vars(operand, bound, out, seen);
            }
            Expr::Block(stmts) => {
                let mut bound = bound.clone();
                for s in stmts {
                    if let Some(stmt) = self.core.stmt_pool.get(&s) {
                        self.collect_stmt_free_vars(&stmt, &mut bound, out, seen);
                    }
                }
            }
            Expr::ExprList(items)
            | Expr::ArrayLiteral(items)
            | Expr::TupleLiteral(items) => {
                for e in items {
                    self.collect_closure_free_vars(e, bound, out, seen);
                }
            }
            Expr::FieldAccess(obj, _) | Expr::TupleAccess(obj, _) => {
                self.collect_closure_free_vars(obj, bound, out, seen);
            }
            Expr::MethodCall(obj, _, args) => {
                self.collect_closure_free_vars(obj, bound, out, seen);
                for a in args {
                    self.collect_closure_free_vars(a, bound, out, seen);
                }
            }
            Expr::BuiltinMethodCall(receiver, _, args) => {
                self.collect_closure_free_vars(receiver, bound, out, seen);
                for a in args {
                    self.collect_closure_free_vars(a, bound, out, seen);
                }
            }
            Expr::BuiltinCall(_, args) => {
                for a in args {
                    self.collect_closure_free_vars(a, bound, out, seen);
                }
            }
            Expr::StructLiteral(_, fields) => {
                for (_, e) in fields {
                    self.collect_closure_free_vars(e, bound, out, seen);
                }
            }
            Expr::AssociatedFunctionCall(_, _, args) => {
                for a in args {
                    self.collect_closure_free_vars(a, bound, out, seen);
                }
            }
            Expr::SliceAccess(obj, info) => {
                self.collect_closure_free_vars(obj, bound, out, seen);
                if let Some(s) = info.start {
                    self.collect_closure_free_vars(s, bound, out, seen);
                }
                if let Some(e) = info.end {
                    self.collect_closure_free_vars(e, bound, out, seen);
                }
            }
            Expr::SliceAssign(obj, start, end, value) => {
                self.collect_closure_free_vars(obj, bound, out, seen);
                if let Some(s) = start {
                    self.collect_closure_free_vars(s, bound, out, seen);
                }
                if let Some(e) = end {
                    self.collect_closure_free_vars(e, bound, out, seen);
                }
                self.collect_closure_free_vars(value, bound, out, seen);
            }
            Expr::DictLiteral(entries) => {
                for (k, v) in entries {
                    self.collect_closure_free_vars(k, bound, out, seen);
                    self.collect_closure_free_vars(v, bound, out, seen);
                }
            }
            Expr::Cast(e, _) => self.collect_closure_free_vars(e, bound, out, seen),
            Expr::Match(scrut, arms) => {
                self.collect_closure_free_vars(scrut, bound, out, seen);
                for arm in arms {
                    let mut arm_bound = bound.clone();
                    Self::pattern_bound_names(&arm.pattern, &mut arm_bound);
                    if let Some(g) = arm.guard {
                        self.collect_closure_free_vars(g, &arm_bound, out, seen);
                    }
                    self.collect_closure_free_vars(arm.body, &arm_bound, out, seen);
                }
            }
            Expr::Closure { params, body, .. } => {
                let mut nested_bound = bound.clone();
                for (p, _) in &params {
                    nested_bound.insert(*p);
                }
                self.collect_closure_free_vars(body, &nested_bound, out, seen);
            }
            // `?` operator — descends into the inner expression. The
            // type checker normally rewrites this to a Match before
            // closure-capture analysis runs, but the arm exists for
            // defence-in-depth in case the order ever changes.
            Expr::Try { inner, .. } => {
                self.collect_closure_free_vars(inner, bound, out, seen);
            }
            // `P { x: e, ..base }` — same defence-in-depth as `Try`:
            // the desugar normally runs first, but the written field
            // values and the base are ordinary expressions.
            Expr::StructUpdate { fields, base, .. } => {
                for (_, value) in &fields {
                    self.collect_closure_free_vars(*value, bound, out, seen);
                }
                self.collect_closure_free_vars(base, bound, out, seen);
            }
            Expr::QualifiedIdentifier(_)
            | Expr::Int64(_) | Expr::UInt64(_) | Expr::Float64(_)
            | Expr::Int8(_) | Expr::Int16(_) | Expr::Int32(_)
            | Expr::UInt8(_) | Expr::UInt16(_) | Expr::UInt32(_)
            | Expr::Number(_) | Expr::String(_)
            | Expr::True | Expr::False | Expr::Null => {}
        }
    }

    fn collect_stmt_free_vars(
        &self,
        stmt: &Stmt,
        bound: &mut std::collections::HashSet<DefaultSymbol>,
        out: &mut Vec<(DefaultSymbol, TypeDecl)>,
        seen: &mut std::collections::HashSet<DefaultSymbol>,
    ) {
        match stmt {
            Stmt::Expression(e) => self.collect_closure_free_vars(*e, bound, out, seen),
            Stmt::Val(name, _, e) => {
                self.collect_closure_free_vars(*e, bound, out, seen);
                bound.insert(*name);
            }
            Stmt::Var(name, _, e) => {
                if let Some(e) = e {
                    self.collect_closure_free_vars(*e, bound, out, seen);
                }
                bound.insert(*name);
            }
            Stmt::Return(e) => {
                if let Some(e) = e {
                    self.collect_closure_free_vars(*e, bound, out, seen);
                }
            }
            Stmt::For(_label, name, start, end, body) => {
                self.collect_closure_free_vars(*start, bound, out, seen);
                self.collect_closure_free_vars(*end, bound, out, seen);
                let mut inner = bound.clone();
                inner.insert(*name);
                self.collect_closure_free_vars(*body, &inner, out, seen);
            }
            Stmt::While(_label, cond, body) => {
                self.collect_closure_free_vars(*cond, bound, out, seen);
                self.collect_closure_free_vars(*body, bound, out, seen);
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::StructDecl { .. }
            | Stmt::ImplBlock { .. }
            | Stmt::EnumDecl { .. }
            | Stmt::TraitDecl { .. }
            | Stmt::TypeAlias { .. } => {}
        }
    }

    /// Collect symbols that a pattern binds — used to extend the
    /// in-scope set when walking a `match` arm body for free vars.
    fn pattern_bound_names(
        pat: &Pattern,
        bound: &mut std::collections::HashSet<DefaultSymbol>,
    ) {
        match pat {
            Pattern::Name(s) => {
                bound.insert(*s);
            }
            Pattern::EnumVariant(_, _, subs) | Pattern::Tuple(subs) => {
                for sp in subs {
                    Self::pattern_bound_names(sp, bound);
                }
            }
            Pattern::Struct(_, fields, _) => {
                for (_, sp) in fields {
                    Self::pattern_bound_names(sp, bound);
                }
            }
            // PATTERN-EXTEND: `n @ pat` binds `n` on top of whatever
            // `pat` binds.
            Pattern::Binding(s, inner) => {
                bound.insert(*s);
                Self::pattern_bound_names(inner, bound);
            }
            Pattern::Wildcard | Pattern::Literal(_) | Pattern::Range(_, _) => {}
        }
    }

    /// Type check literal values
    pub fn visit_int64_literal(&mut self, _value: &i64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Int64)
    }

    pub fn visit_uint64_literal(&mut self, _value: &u64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::UInt64)
    }

    pub fn visit_float64_literal(&mut self, _value: &f64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Float64)
    }

    pub fn visit_number_literal(&mut self, _value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Number)
    }

    pub fn visit_string_literal(&mut self, _value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::String)
    }

    pub fn visit_boolean_literal(&mut self, _value: &Expr) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Bool)
    }

    pub fn visit_null_literal(&mut self) -> Result<TypeDecl, TypeCheckError> {
        // TYPECHECK-LIES: `null` used to take on whatever type the
        // position wanted and pass the check, then stop the program the
        // moment it was evaluated — a type system that accepted a
        // program no backend could run. Refused here instead, with the
        // supported spellings named in the message.
        Err(TypeCheckError::reserved_literal(
            "null",
            "model an absent value with `Option<T>`, or a raw null pointer \
             with `__builtin_null_ptr()`",
        ))
    }

    /// Type check expression lists
    pub fn visit_expr_list(&mut self, _items: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }

    /// Type check array literals
    pub fn visit_array_literal(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if elements.is_empty() {
            return Err(TypeCheckError::array_error("Empty array literals are not supported"));
        }

        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in array literal type inference - possible circular reference"
            ));
        }
        
        self.type_inference.recursion_depth += 1;
        
        // Execute the main logic and capture result
        let result = self.visit_array_literal_impl(elements);
        
        // Always decrement recursion depth before returning
        self.type_inference.recursion_depth -= 1;

        result
    }

    /// NEWTYPE: type `Meters(v0, v1)` as the struct literal
    /// `Meters { 0: v0, 1: v1 }` when `Meters` names a tuple struct.
    /// Returns `None` when the callee is not one, leaving the caller's
    /// ordinary "function not found" path in charge.
    ///
    /// The pool node is not rewritten here -- this visitor is reached
    /// through `accept_expr` from a dozen call sites that do not carry
    /// the node's own `ExprRef` (block tails, operands, branches), so
    /// the rewrite is *recorded* and applied by
    /// `apply_tuple_struct_rewrites` once checking is done. Backends
    /// therefore only ever see `StructLiteral`.
    pub(crate) fn check_tuple_struct_construction(
        &mut self,
        callee: DefaultSymbol,
        args_ref: &ExprRef,
    ) -> Option<Result<TypeDecl, TypeCheckError>> {
        let fields = self.context.get_struct_fields(callee)?;
        if !fields.first().is_some_and(|f| f.is_positional()) {
            return None;
        }
        let field_names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
        let args = match self.core.expr_pool.get(args_ref) {
            Some(Expr::ExprList(args)) => args.clone(),
            _ => return None,
        };
        if args.len() != field_names.len() {
            let struct_name = self.resolve_symbol_name(callee);
            return Some(Err(TypeCheckError::generic_error(&format!(
                "`{struct_name}` takes {} field(s), but {} argument(s) were given",
                field_names.len(),
                args.len()
            ))));
        }
        let mut initializers = Vec::with_capacity(args.len());
        for (name, arg) in field_names.iter().zip(args.iter()) {
            // Interned by the parser when it read the declaration, so
            // the lookup cannot miss for a struct that exists.
            let field_symbol = self.core.string_interner.get(name.as_str())?;
            initializers.push((field_symbol, *arg));
        }
        let ty = self.visit_struct_literal_impl(&callee, &initializers);
        if ty.is_ok() {
            self.tuple_struct_rewrites.constructions.insert(*args_ref, initializers);
        }
        Some(ty)
    }

    /// NEWTYPE: install the tuple-struct rewrites collected while
    /// checking, so every backend sees the named-struct forms
    /// (`StructLiteral` / `FieldAccess`) it already lowers.
    ///
    /// One pass over the pool, and only when something was recorded --
    /// a program without tuple structs pays nothing.
    pub fn apply_tuple_struct_rewrites(&mut self) {
        if self.tuple_struct_rewrites.is_empty() {
            return;
        }
        let rewrites = std::mem::take(&mut self.tuple_struct_rewrites);
        for index in 0..self.core.expr_pool.len() {
            let expr_ref = ExprRef(index as u32);
            let replacement = match self.core.expr_pool.get(&expr_ref) {
                Some(Expr::Call(name, args)) => rewrites
                    .constructions
                    .get(&args)
                    .map(|inits| Expr::StructLiteral(name, inits.clone())),
                Some(Expr::TupleAccess(obj, _)) => rewrites
                    .accesses
                    .get(&obj)
                    .map(|field| Expr::FieldAccess(obj, *field)),
                _ => None,
            };
            if let Some(replacement) = replacement {
                self.core.expr_pool.update(&expr_ref, replacement);
            }
        }
    }

    /// `?` operator desugar. The parser emits `Expr::Try(inner)`; we
    /// rewrite the pool entry in place so backends only ever see the
    /// resulting `Match`. The desugar depends on the inner type:
    ///
    /// ```text
    /// expr?   where expr : Result<T, E>
    /// // becomes:
    /// match expr {
    ///     Result::Ok(__try_v_N)  => __try_v_N,
    ///     Result::Err(__try_e_N) => {
    ///         return Result::Err(__try_e_N)
    ///         panic("?-unreachable")
    ///     },
    /// }
    /// ```
    ///
    /// (and analogously for `Option<T>` with `Some` / `None`). The
    /// trailing `panic` is unreachable at runtime — `return` always
    /// fires first — but it pins the arm body's block type to
    /// `Unknown` so the two arms unify into `T`. Without it the
    /// arm would type as `Result<T, E>` / `Option<T>` and clash
    /// with the `T` arm.
    pub fn desugar_try_expr(
        &mut self,
        try_ref: ExprRef,
        inner: ExprRef,
    ) -> Result<TypeDecl, TypeCheckError> {
        // Pull pre-interned synthetic symbols out of the Try node
        // before we visit `inner` (visiting may mutate `expr_pool`
        // and invalidate clones taken later).
        let (t_sym, v_sym, e_sym, panic_msg_sym, conv_sym, err_sym) =
            match self.core.expr_pool.get(&try_ref) {
                Some(Expr::Try {
                    scrutinee_binding,
                    success_binding,
                    error_binding,
                    panic_msg,
                    converted_binding,
                    result_binding,
                    ..
                }) => (
                    scrutinee_binding,
                    success_binding,
                    error_binding,
                    panic_msg,
                    converted_binding,
                    result_binding,
                ),
                _ => {
                    return Err(TypeCheckError::generic_error(
                        "desugar_try_expr: pool entry no longer a Try node",
                    ));
                }
            };

        // Determine inner type first.
        let inner_ty = self.visit_expr(&inner)?;

        // Resolve the enum name. The parser emits `Identifier` for
        // user-named types until the type checker has seen all decls;
        // accept all three shapes the rest of the type-checker uses.
        let enum_name = match &inner_ty {
            TypeDecl::Enum(name, _) => *name,
            TypeDecl::Identifier(name) if self.context.enum_definitions.contains_key(name) => *name,
            TypeDecl::Struct(name, _) if self.context.enum_definitions.contains_key(name) => *name,
            _ => {
                return Err(TypeCheckError::generic_error(&format!(
                    "`?` requires Result<T, E> or Option<T>, got {:?}",
                    inner_ty
                )));
            }
        };
        let enum_name_str = self
            .core
            .string_interner
            .resolve(enum_name)
            .unwrap_or("?")
            .to_string();
        let (success_variant, error_variant, error_is_unit) = match enum_name_str.as_str() {
            "Result" => ("Ok", "Err", false),
            "Option" => ("Some", "None", true),
            _ => {
                return Err(TypeCheckError::generic_error(&format!(
                    "`?` requires Result or Option, got enum `{}`",
                    enum_name_str
                )));
            }
        };

        // Success-arm body wraps `Identifier(v_sym)` in a `Cast` to
        // the inferred success type. Without the cast, the AOT
        // compiler's static type-inference (`value_scalar`) walks
        // into the success arm and sees a bare identifier with no
        // binding yet (pattern bindings are scope-local), so it
        // cannot tell the outer `val rhs = expr?` binding what
        // scalar type to allocate. Naming the type via `Cast`
        // makes the inference deterministic.
        let success_type = match &inner_ty {
            TypeDecl::Enum(_, args) | TypeDecl::Struct(_, args) if !args.is_empty() => {
                args[0].clone()
            }
            _ => TypeDecl::Unknown,
        };

        // Look up the variant symbols. The stdlib auto-load already
        // interned `"Ok"` / `"Err"` (`core/std/result.t`) and
        // `"Some"` / `"None"` (`core/std/option.t`), so `.get()`
        // (which only needs `&self`) is sufficient.
        let success_sym = self.core.string_interner.get(success_variant).ok_or_else(|| {
            TypeCheckError::generic_error(&format!(
                "`?` desugar: `{}` variant symbol not interned — is stdlib loaded?",
                success_variant
            ))
        })?;
        let error_sym = self.core.string_interner.get(error_variant).ok_or_else(|| {
            TypeCheckError::generic_error(&format!(
                "`?` desugar: `{}` variant symbol not interned — is stdlib loaded?",
                error_variant
            ))
        })?;

        // --- Success arm: `Result::Ok(__try_v_N) => __try_v_N as T`
        // (analogously for `Option::Some`). The trailing `as T`
        // pins the arm body's static type for the AOT's
        // `value_scalar` inference (otherwise a bare pattern
        // binding has no resolvable type at lowering time).
        let success_pattern = Pattern::EnumVariant(
            enum_name,
            success_sym,
            vec![Pattern::Name(v_sym)],
        );
        let v_ident = self.core.expr_pool.add(Expr::Identifier(v_sym));
        let success_body = self
            .core
            .expr_pool
            .add(Expr::Cast(v_ident, success_type));
        let success_arm = MatchArm {
            pattern: success_pattern,
            guard: None,
            body: success_body,
        };

        // --- Error arm body: `{ return <scrutinee>; panic("?-unreachable") }`
        //
        // For both `Result::Err(e)` and `Option::None`, the error-arm
        // simply re-returns the already-bound scrutinee value (which
        // is known to *be* the error variant). This sidesteps the
        // AOT compiler's MVP constraint that `return` accept only a
        // bare identifier — manually re-constructing
        // `Result::Err(e)` / `Option::None` would be an
        // `AssociatedFunctionCall` / `QualifiedIdentifier` and fail
        // that check.
        //
        // From/Into cross-error conversion: when the inner type is
        // `Result<T, E1>` and the enclosing function returns
        // `Result<T, E2>` with E1 != E2 and `E2: From<E1>`, the arm
        // instead converts the error before re-returning:
        //
        //     Result::Err(__try_e_N) => {
        //         val __try_conv_N = E2::from(__try_e_N)
        //         val __try_err_N = Result::Err(__try_conv_N)
        //         return __try_err_N
        //         panic("?-unreachable")
        //     }
        //
        // `return __try_err_N` stays a bare identifier (AOT-safe); the
        // conversion call and the enum re-construction both live in
        // `val` bindings, which every backend lowers like ordinary
        // let statements.
        let error_pattern = if error_is_unit {
            Pattern::EnumVariant(enum_name, error_sym, vec![])
        } else {
            Pattern::EnumVariant(enum_name, error_sym, vec![Pattern::Name(e_sym)])
        };
        // From/Into: decide whether a cross-error conversion applies.
        // Only `Result` (a payload-bearing error) can convert; an
        // `Option::None` has no value to feed a `From` impl.
        //
        // When the enclosing function's return type names a *different*
        // error type (`Result<T, E2>` vs inner `Result<T, E1>`), the
        // conversion impl is mandatory: without `E2: From<E1>` the
        // error arm would silently `return` the E1-typed scrutinee and
        // the caller would observe the wrong variant payload. Reject
        // that here rather than letting it through as a "type-checks
        // but answers wrong" program.
        let conversion_stmts: Vec<StmtRef> = if error_is_unit {
            Vec::new()
        } else {
            let inner_err_ty = match &inner_ty {
                TypeDecl::Enum(_, args) | TypeDecl::Struct(_, args) if args.len() == 2 => {
                    args[1].clone()
                }
                _ => TypeDecl::Unknown,
            };
            let fn_ret = self.current_fn_return_type.clone();
            let target_err_ty = match &fn_ret {
                Some(TypeDecl::Enum(_, args)) | Some(TypeDecl::Struct(_, args))
                    if args.len() == 2 =>
                {
                    args[1].clone()
                }
                _ => TypeDecl::Unknown,
            };
            if inner_err_ty != TypeDecl::Unknown
                && target_err_ty != TypeDecl::Unknown
                && inner_err_ty != target_err_ty
            {
                let inner_name = self.type_name_for_error(&inner_err_ty);
                let target_name = self.type_name_for_error(&target_err_ty);
                if !self.type_implements_from(&target_err_ty, &inner_err_ty) {
                    return Err(TypeCheckError::generic_error(&format!(
                        "`?` cannot convert error type `{}` to `{}`; implement \
                         `From<{}> for {}` (e.g. `impl From<str> for String`)",
                        inner_name, target_name, inner_name, target_name,
                    )));
                }
            }
            self.cross_error_conversion(&inner_ty, e_sym, conv_sym, err_sym, enum_name, error_sym)
        };

        let mut error_stmts: Vec<StmtRef> = if conversion_stmts.is_empty() {
            // Plain propagation: `return <scrutinee>` — the scrutinee
            // is already the error variant, and a bare identifier
            // keeps the AOT's `return <ident>` constraint satisfied.
            let return_value = self.core.expr_pool.add(Expr::Identifier(t_sym));
            vec![self.core.stmt_pool.add(Stmt::Return(Some(return_value)))]
        } else {
            conversion_stmts
        };

        // `panic("?-unreachable")` after `return` is dead code at
        // runtime, but it makes the block's static type `Unknown`,
        // which is what unifies the two match arms into `T`.
        let panic_msg_expr = self.core.expr_pool.add(Expr::String(panic_msg_sym));
        let panic_call = self.core.expr_pool.add(Expr::BuiltinCall(
            BuiltinFunction::Panic,
            vec![panic_msg_expr],
        ));
        let panic_stmt = self
            .core
            .stmt_pool
            .add(Stmt::Expression(panic_call));
        error_stmts.push(panic_stmt);

        let error_block = self
            .core
            .expr_pool
            .add(Expr::Block(error_stmts));
        let error_arm = MatchArm {
            pattern: error_pattern,
            guard: None,
            body: error_block,
        };

        // Bind the inner expression to a synthetic temp before
        // matching on it. The AOT compiler's match-lowering MVP
        // (`compiler/src/lower/match_lowering.rs::classify_match_scrutinee`)
        // only accepts enum scrutinees that are bare identifiers,
        // method calls, or scalar primitives — bare function-call
        // scrutinees fall outside that set. A dedicated `t_sym`
        // (separate from the success-arm pattern's `v_sym`) makes
        // the error-arm's `return <t_sym>` self-contained without
        // relying on shadowing.
        let scrutinee_ident = self.core.expr_pool.add(Expr::Identifier(t_sym));
        let bind_stmt = self
            .core
            .stmt_pool
            .add(Stmt::Val(t_sym, None, inner));
        let match_expr = self.core.expr_pool.add(Expr::Match(
            scrutinee_ident,
            vec![success_arm, error_arm],
        ));
        let match_stmt = self
            .core
            .stmt_pool
            .add(Stmt::Expression(match_expr));

        // Rewrite the original Try slot in place to the outer block.
        // Backends observe a fully-formed `Block` containing the val
        // binding plus the match, at the same `ExprRef`.
        self.core.expr_pool.update(
            &try_ref,
            Expr::Block(vec![bind_stmt, match_stmt]),
        );

        // Re-visit the rewritten node. `visit_expr` cache lookup will
        // miss (no prior cache entry for try_ref), so it fetches the
        // updated Expr (now `Block`) and processes it normally.
        self.visit_expr(&try_ref)
    }

    /// From/Into `?` cross-error conversion. Returns the statement
    /// sequence for the error arm when the conversion applies:
    ///
    /// ```text
    /// val __try_conv_N = E2::from(__try_e_N)
    /// val __try_err_N = Result::Err(__try_conv_N)
    /// return __try_err_N
    /// ```
    ///
    /// The enclosing function's return type (`current_fn_return_type`,
    /// set per function / method body) is `Result<T, E2>`; the inner
    /// expression is `Result<T, E1>`. When E1 != E2 and
    /// `E2: From<E1>` is implemented, the error value is converted
    /// before the reconstructed `Result::Err(E2)` is re-returned.
    /// Returns `None` when no conversion applies (matching types, or
    /// no `From` impl — the arm falls back to returning the scrutinee
    /// as-is, which type-checks because the types are equal).
    ///
    /// Note: the conversion is only emitted when `E2: From<E1>`
    /// exists. Without it the desugar is unchanged — the error value
    /// propagates with the inner type, so a `?` across mismatched
    /// error types without a `From` impl remains a type error at the
    /// enclosing function's return-type check.
    fn cross_error_conversion(
        &mut self,
        inner_ty: &TypeDecl,
        e_sym: DefaultSymbol,
        conv_sym: DefaultSymbol,
        err_sym: DefaultSymbol,
        enum_name: DefaultSymbol,
        error_sym: DefaultSymbol,
    ) -> Vec<StmtRef> {
        // Inner error type: `Result<T, E1>` -> E1.
        let inner_err_ty = match inner_ty {
            TypeDecl::Enum(_, args) | TypeDecl::Struct(_, args) if args.len() == 2 => {
                args[1].clone()
            }
            _ => return Vec::new(),
        };
        // Enclosing function's error type: `Result<T, E2>` -> E2.
        let fn_ret = match self.current_fn_return_type.as_ref() {
            Some(ty) => ty.clone(),
            None => return Vec::new(),
        };
        let target_err_ty = match &fn_ret {
            TypeDecl::Enum(_, args) | TypeDecl::Struct(_, args) if args.len() == 2 => {
                args[1].clone()
            }
            _ => return Vec::new(),
        };
        if target_err_ty == inner_err_ty {
            return Vec::new();
        }
        // `E2: From<E1>` must be implemented.
        if !self.type_implements_from(&target_err_ty, &inner_err_ty) {
            return Vec::new();
        }
        let from_method = match self.from_method_symbol() {
            Some(sym) => sym,
            None => return Vec::new(),
        };
        let target_sym = match &target_err_ty {
            TypeDecl::Struct(sym, _) | TypeDecl::Identifier(sym) | TypeDecl::Enum(sym, _) => *sym,
            _ => return Vec::new(),
        };

        // `val __try_conv_N = E2::from(__try_e_N)`
        let e_ident = self.core.expr_pool.add(Expr::Identifier(e_sym));
        let conv_call = self.core.expr_pool.add(Expr::AssociatedFunctionCall(
            target_sym,
            from_method,
            vec![e_ident],
        ));
        // Type annotation: the AOT's `value_scalar` cannot infer the
        // associated call's compound return shape from the call site
        // alone, and `Result::Err(...)` below cannot infer the generic
        // enum's type args from a bare identifier payload. Pin both
        // with explicit annotations (`E2` and `Result<T, E2>`).
        let conv_annotation = Some(target_err_ty.clone());
        let conv_stmt = self
            .core
            .stmt_pool
            .add(Stmt::Val(conv_sym, conv_annotation, conv_call));

        // `val __try_err_N = Result::Err(__try_conv_N)` — annotation
        // `Result<T, E2>` with T taken from the inner expression's
        // success type.
        let success_ty = match inner_ty {
            TypeDecl::Enum(_, args) | TypeDecl::Struct(_, args) if args.len() == 2 => {
                args[0].clone()
            }
            _ => TypeDecl::Unknown,
        };
        let err_annotation = match &inner_ty {
            TypeDecl::Enum(name, _) => TypeDecl::Enum(*name, vec![success_ty, target_err_ty]),
            TypeDecl::Struct(name, _) => TypeDecl::Struct(*name, vec![success_ty, target_err_ty]),
            _ => TypeDecl::Unknown,
        };
        let conv_ident = self.core.expr_pool.add(Expr::Identifier(conv_sym));
        let err_construct = self.core.expr_pool.add(Expr::AssociatedFunctionCall(
            enum_name,
            error_sym,
            vec![conv_ident],
        ));
        let err_stmt = self
            .core
            .stmt_pool
            .add(Stmt::Val(err_sym, Some(err_annotation), err_construct));

        // `return __try_err_N`
        let err_ident = self.core.expr_pool.add(Expr::Identifier(err_sym));
        let return_stmt = self
            .core
            .stmt_pool
            .add(Stmt::Return(Some(err_ident)));

        vec![conv_stmt, err_stmt, return_stmt]
    }

    /// `Display` dispatch (`core/std/display.t`).
    ///
    /// Rewrites the argument of the three builtins that turn a value
    /// into text, when that value's type knows how to render itself:
    ///
    /// ```text
    /// println(v)              ->  println(v.to_str())
    /// __builtin_to_string(v)  ->  __builtin_to_string(v.to_str())
    /// ```
    ///
    /// Rewriting the *argument* rather than the whole call keeps one
    /// code path for all three: `__builtin_to_string` of a `str` is
    /// identity on every backend, so the extra wrapper costs nothing
    /// and the builtin does not have to be removed. It also means the
    /// rewrite can happen here, in the one place both routes into a
    /// builtin call converge — a statement-position `println(v)` goes
    /// through `check_expr_located` -> `accept_expr` and never reaches
    /// `visit_expr`, where `Try` is intercepted.
    ///
    /// String interpolation desugars to `__builtin_to_string`, so
    /// `"{v}"` is covered by the same rewrite.
    ///
    /// Idempotent: the replacement's type is `str`, which is not a
    /// nominal type, so a second visit rewrites nothing.
    ///
    /// Dispatch is on the presence of the method, not on a recorded
    /// `impl Display for`, matching how `==` finds `eq` and `+` finds
    /// `add`. An inherent `to_str` therefore works too.
    pub(super) fn apply_display_dispatch(
        &mut self,
        func: &BuiltinFunction,
        args: &[ExprRef],
    ) -> Result<(), TypeCheckError> {
        if !matches!(
            func,
            BuiltinFunction::ToString | BuiltinFunction::Print | BuiltinFunction::Println
        ) {
            return Ok(());
        }
        let [arg] = args else { return Ok(()) };
        let arg_ty = self.visit_expr(arg)?;
        if !self.type_renders_itself(&arg_ty) {
            return Ok(());
        }
        let Some(to_str) = self.core.string_interner.get(DISPLAY_METHOD) else {
            return Ok(());
        };
        // Move the receiver into a fresh slot first: the method call
        // has to point at it, and it is about to be overwritten.
        let Some(receiver) = self.core.expr_pool.get(arg) else {
            return Ok(());
        };
        let receiver_ref = self.core.expr_pool.add(receiver);
        self.core
            .expr_pool
            .update(arg, Expr::MethodCall(receiver_ref, to_str, Vec::new()));
        Ok(())
    }

    /// Whether `ty` is a struct or enum that renders itself.
    ///
    /// Restricted to nominal types on purpose. Primitives already
    /// render correctly and route through per-type fast paths in every
    /// backend; sending them via a method call would be slower, and
    /// would let a stray `to_str` in scope change how integers print.
    fn type_renders_itself(&mut self, ty: &TypeDecl) -> bool {
        let name = match ty {
            TypeDecl::Struct(name, _) | TypeDecl::Enum(name, _) => *name,
            // Bare nominal name, pre-canonicalisation — the same shape
            // `struct_method_compatible` has to accept.
            TypeDecl::Identifier(name) => *name,
            _ => return false,
        };
        self.display_types().contains(&name)
    }

    /// Every type with a `to_str(&self) -> str`, collected from the
    /// statement pool once.
    ///
    /// The shape is checked rather than assumed so that a same-named
    /// method that is not a renderer keeps its own meaning. Dispatching
    /// to `fn to_str(&self, radix: u64) -> str` would turn a `println`
    /// into an arity error about a call the user never wrote, and one
    /// returning `u64` is not a rendering at all.
    fn display_types(&mut self) -> &std::collections::HashSet<DefaultSymbol> {
        if self.display_types.is_none() {
            let mut found = std::collections::HashSet::new();
            if let Some(method) = self.core.string_interner.get(DISPLAY_METHOD) {
                for i in 0..self.core.stmt_pool.len() {
                    let Some(Stmt::ImplBlock { target_type, methods, .. }) =
                        self.core.stmt_pool.get(&StmtRef(i as u32))
                    else {
                        continue;
                    };
                    let renders = methods.iter().any(|m| {
                        m.name == method
                            // `parameter` excludes the receiver, so a
                            // renderer takes none.
                            && m.has_self_param
                            && m.parameter.is_empty()
                            && matches!(m.return_type, Some(TypeDecl::String))
                    });
                    if renders {
                        found.insert(target_type);
                    }
                }
            }
            self.display_types = Some(found);
        }
        self.display_types.as_ref().expect("just populated")
    }
}

/// The method `Display` dispatches to. Named `to_str` rather than
/// `to_string` because `str` and `String` are different types here and
/// `String::to_string() -> String` already means the idempotent clone;
/// interpolation splices `str`, which is what this returns.
const DISPLAY_METHOD: &str = "to_str";
