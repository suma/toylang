use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError,
    Acceptable, TypeInferenceManager
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
        let expr_obj = self.core.expr_pool.get(&expr)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference"))?;
        
        let result = expr_obj.clone().accept(self);
        
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
            self.cache_type(&expr, result_type.clone());
            self.type_inference.set_expr_type(*expr, result_type.clone());
            
            // Context propagation for numeric types
            if original_hint.is_none() && (result_type == &TypeDecl::Int64 || result_type == &TypeDecl::UInt64) {
                if self.type_inference.type_hint.is_none() {
                    self.type_inference.type_hint = Some(result_type.clone());
                }
            }
        }
        
        result
    }

    /// Type check unary operators
    pub fn visit_unary(&mut self, op: &UnaryOp, operand: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let op = op.clone();
        let operand = operand.clone();
        let operand_ty = {
            let operand_obj = self.core.expr_pool.get(&operand)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid operand expression reference"))?;
            operand_obj.clone().accept(self)?
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
        if let Some(method_name) = Self::struct_unary_method_name(&op) {
            if self.struct_method_compatible(&resolved_ty, &resolved_ty, method_name) {
                return Ok(resolved_ty);
            }
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
                if *resolved_ty == TypeDecl::UInt64 {
                    Ok(TypeDecl::UInt64)
                } else if *resolved_ty == TypeDecl::Int64 {
                    Ok(TypeDecl::Int64)
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
                if *resolved_ty == TypeDecl::Int64 {
                    Ok(TypeDecl::Int64)
                } else if *resolved_ty == TypeDecl::Float64 {
                    Ok(TypeDecl::Float64)
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
        let lhs = lhs.clone();
        let rhs = rhs.clone();

        let lhs_ty = {
            let lhs_obj = self.core.expr_pool.get(&lhs)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid left-hand expression reference"))?;
            lhs_obj.clone().accept(self)?
        };

        let rhs_ty = {
            let rhs_obj = self.core.expr_pool.get(&rhs)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid right-hand expression reference"))?;
            rhs_obj.clone().accept(self)?
        };

        // Operator overload (Phase B continuation): arithmetic ops
        // between matching struct values dispatch to `add` / `sub` /
        // `mul` / `div` / `rem` methods on the struct. Catch this
        // *before* `resolve_numeric_types` because that helper
        // rejects struct-typed operands with the generic "incompatible
        // types" diagnostic — defeating the user's overload. Returns
        // the struct as result type so `a + b + c` chains keep
        // checking. (Comparison overloads are reached later via the
        // compare arm below.)
        if let Some(method_name) = Self::struct_arith_method_name(&op) {
            if self.struct_method_compatible(&lhs_ty, &rhs_ty, method_name) {
                return Ok(lhs_ty);
            }
        }

        // Special handling for shift operations where right operand must be UInt64
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

        // Per-category result type computation. Each helper handles
        // its own struct-overload short-circuit and produces a
        // `TypeCheckError` with a category-specific label on
        // mismatch.
        let result_type = match op {
            Operator::IAdd if resolved_lhs_ty == TypeDecl::String && resolved_rhs_ty == TypeDecl::String => {
                // String concat is a special case of `+` that lives
                // outside `check_arith_binary`'s numeric-only rule.
                TypeDecl::String
            }
            Operator::IAdd | Operator::ISub | Operator::IDiv | Operator::IMul | Operator::IMod => {
                self.check_arith_binary(&op, &lhs, &resolved_lhs_ty, &resolved_rhs_ty)?
            }
            Operator::LE | Operator::LT | Operator::GE | Operator::GT | Operator::EQ | Operator::NE => {
                self.check_compare_binary(&op, &lhs, &resolved_lhs_ty, &resolved_rhs_ty)?
            }
            Operator::LogicalAnd | Operator::LogicalOr => {
                self.check_logical_binary(&lhs, &resolved_lhs_ty, &resolved_rhs_ty)?
            }
            Operator::BitwiseAnd | Operator::BitwiseOr | Operator::BitwiseXor => {
                self.check_bitwise_binary(&op, &lhs, &resolved_lhs_ty, &resolved_rhs_ty)?
            }
            Operator::LeftShift | Operator::RightShift => {
                self.check_shift_binary(&op, &lhs, &rhs, &resolved_lhs_ty, &resolved_rhs_ty)?
            }
        };

        Ok(result_type)
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
    /// struct-overload pairs. String concat is handled before the
    /// dispatch (see `visit_binary`). NUM-W narrow integers follow
    /// the same-width rule as i64/u64 — no implicit widening.
    fn check_arith_binary(
        &self,
        op: &Operator,
        lhs: &ExprRef,
        l: &TypeDecl,
        r: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        if *l == TypeDecl::UInt64 && *r == TypeDecl::UInt64 {
            Ok(TypeDecl::UInt64)
        } else if *l == TypeDecl::Int64 && *r == TypeDecl::Int64 {
            Ok(TypeDecl::Int64)
        } else if *l == TypeDecl::UInt32 && *r == TypeDecl::UInt32 {
            Ok(TypeDecl::UInt32)
        } else if *l == TypeDecl::Int32 && *r == TypeDecl::Int32 {
            Ok(TypeDecl::Int32)
        } else if *l == TypeDecl::UInt16 && *r == TypeDecl::UInt16 {
            Ok(TypeDecl::UInt16)
        } else if *l == TypeDecl::Int16 && *r == TypeDecl::Int16 {
            Ok(TypeDecl::Int16)
        } else if *l == TypeDecl::UInt8 && *r == TypeDecl::UInt8 {
            Ok(TypeDecl::UInt8)
        } else if *l == TypeDecl::Int8 && *r == TypeDecl::Int8 {
            Ok(TypeDecl::Int8)
        } else if *l == TypeDecl::Float64 && *r == TypeDecl::Float64 {
            // f64 supports +, -, *, /, %. `%` follows Rust's `f64::rem`,
            // matching the IEEE 754 remainder via fmod-style truncation.
            Ok(TypeDecl::Float64)
        } else if let (TypeDecl::Generic(left_param), TypeDecl::Generic(right_param)) = (l, r) {
            // Generic-type arithmetic when both sides are the same parameter.
            if left_param == right_param {
                Ok(l.clone())
            } else {
                Err(self.error_with_location(
                    TypeCheckError::type_mismatch_operation("arithmetic", l.clone(), r.clone()),
                    lhs,
                ))
            }
        } else if let Some(method_name) = Self::struct_arith_method_name(op) {
            // Operator overload fallback: same-shape struct pair with
            // the matching `add` / `sub` / `mul` / `div` / `rem` method
            // (`fn ___(&self, other: &Self) -> Self`). Result is the
            // struct itself so chained expressions keep type-checking.
            if self.struct_method_compatible(l, r, method_name) {
                Ok(l.clone())
            } else {
                Err(self.error_with_location(
                    TypeCheckError::type_mismatch_operation("arithmetic", l.clone(), r.clone()),
                    lhs,
                ))
            }
        } else {
            Err(self.error_with_location(
                TypeCheckError::type_mismatch_operation("arithmetic", l.clone(), r.clone()),
                lhs,
            ))
        }
    }

    /// Result-type rule for `< <= > >= == !=`: bool for any
    /// matching int width, f64, bool, allocator-handle (== / != only),
    /// or struct overload (eq / lt / le / gt / ge).
    fn check_compare_binary(
        &self,
        op: &Operator,
        lhs: &ExprRef,
        l: &TypeDecl,
        r: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        // NUM-W: same-width narrow-int compares.
        let same_int_width = matches!((l, r),
            (TypeDecl::UInt32, TypeDecl::UInt32)
            | (TypeDecl::Int32, TypeDecl::Int32)
            | (TypeDecl::UInt16, TypeDecl::UInt16)
            | (TypeDecl::Int16, TypeDecl::Int16)
            | (TypeDecl::UInt8, TypeDecl::UInt8)
            | (TypeDecl::Int8, TypeDecl::Int8));
        if (*l == TypeDecl::UInt64 || *l == TypeDecl::Int64) &&
           (*r == TypeDecl::UInt64 || *r == TypeDecl::Int64) {
            Ok(TypeDecl::Bool)
        } else if same_int_width {
            Ok(TypeDecl::Bool)
        } else if *l == TypeDecl::Float64 && *r == TypeDecl::Float64 {
            // f64 compares use IEEE 754 — NaN is false for ordering / equality.
            Ok(TypeDecl::Bool)
        } else if *l == TypeDecl::Bool && *r == TypeDecl::Bool {
            Ok(TypeDecl::Bool)
        } else if matches!(op, Operator::EQ | Operator::NE)
                  && self.is_allocator_compatible(l)
                  && self.is_allocator_compatible(r) {
            // Allocator handles support only identity (== / !=), not ordering.
            // A generic parameter bounded by Allocator counts as allocator-compatible
            // so expressions like `current_allocator() == a` type-check inside a
            // `<A: Allocator>` function body.
            Ok(TypeDecl::Bool)
        } else if let Some(method_name) = Self::struct_cmp_method_name(op) {
            // Operator overload (Phase B + Phase 2 ext): same-shape
            // struct pair with `eq` / `lt` / `le` / `gt` / `ge`
            // method (`fn ___(&self, other: &Self) -> bool`).
            if self.struct_method_compatible(l, r, method_name) {
                Ok(TypeDecl::Bool)
            } else {
                Err(self.error_with_location(
                    TypeCheckError::type_mismatch_operation("comparison", l.clone(), r.clone()),
                    lhs,
                ))
            }
        } else {
            Err(self.error_with_location(
                TypeCheckError::type_mismatch_operation("comparison", l.clone(), r.clone()),
                lhs,
            ))
        }
    }

    /// Result-type rule for `&& ||`: bool only. (No struct overload —
    /// short-circuit semantics are not user-redefinable.)
    fn check_logical_binary(
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
    fn check_bitwise_binary(
        &self,
        op: &Operator,
        lhs: &ExprRef,
        l: &TypeDecl,
        r: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        if *l == TypeDecl::UInt64 && *r == TypeDecl::UInt64 {
            Ok(TypeDecl::UInt64)
        } else if *l == TypeDecl::Int64 && *r == TypeDecl::Int64 {
            Ok(TypeDecl::Int64)
        } else if let Some(method_name) = Self::struct_self_returning_method_name(op) {
            if self.struct_method_compatible(l, r, method_name) {
                Ok(l.clone())
            } else {
                Err(self.error_with_location(
                    TypeCheckError::type_mismatch_operation("bitwise", l.clone(), r.clone()),
                    lhs,
                ))
            }
        } else {
            Err(self.error_with_location(
                TypeCheckError::type_mismatch_operation("bitwise", l.clone(), r.clone()),
                lhs,
            ))
        }
    }

    /// Result-type rule for `<< >>`: struct overload (`shl` / `shr`)
    /// is checked first so the primitive `rhs must be UInt64` rule
    /// doesn't preempt it. Otherwise rhs must be `UInt64` and lhs
    /// must be `UInt64` / `Int64`; result matches the lhs's signedness.
    fn check_shift_binary(
        &self,
        op: &Operator,
        lhs: &ExprRef,
        rhs: &ExprRef,
        l: &TypeDecl,
        r: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        if let Some(method_name) = Self::struct_self_returning_method_name(op) {
            if self.struct_method_compatible(l, r, method_name) {
                return Ok(l.clone());
            }
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
    pub fn visit_block(&mut self, statements: &Vec<StmtRef>) -> Result<TypeDecl, TypeCheckError> {
        let mut last_empty = true;
        let mut last: Option<TypeDecl> = None;
        
        // Clear type cache at the start of each block to limit cache scope
        self.optimization.type_cache.clear();
        
        // Pre-scan for explicit type declarations and establish global type context
        let original_hint = self.type_inference.type_hint.clone();
        // Only override the inherited hint when it's unset, so an outer hint
        // (e.g. the method's declared return type) isn't clobbered by a
        // numeric-scan result from a transient `val x: u64 = ...` in the body.
        if original_hint.is_none() {
            if let Some(numeric_type) = self.scan_numeric_type_hint(statements) {
                self.type_inference.type_hint = Some(numeric_type);
            }
        }

        // Process each statement
        // This code assumes Block(expression) don't make nested function
        // so `return` expression always return for this context.
        for s in statements.iter() {
            let stmt = self.core.stmt_pool.get(&s)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference in block"))?;
            
            let stmt_type = match stmt {
                Stmt::Return(None) => Ok(TypeDecl::Unit),
                Stmt::Return(ret_ty) => {
                    if let Some(e) = ret_ty {
                        let e = e.clone();
                        let expr_obj = self.core.expr_pool.get(&e)
                            .ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference in return"))?;
                        let ty = expr_obj.clone().accept(self)?;
                        if last_empty {
                            last_empty = false;
                            Ok(ty)
                        } else if let Some(last_ty) = last.clone() {
                            if last_ty == ty {
                                Ok(ty)
                            } else {
                                return Err(TypeCheckError::type_mismatch(last_ty, ty).with_context("return statement"));
                            }
                        } else {
                            Ok(ty)
                        }
                    } else {
                        Ok(TypeDecl::Unit)
                    }
                }
                _ => {
                    let stmt_obj = self.core.stmt_pool.get(&s)
                        .ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference"))?;
                    stmt_obj.clone().accept(self)
                }
            };

            match stmt_type {
                Ok(def_ty) => last = Some(def_ty),
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
    pub fn visit_if_elif_else(&mut self, _cond: &ExprRef, then_block: &ExprRef, elif_pairs: &Vec<(ExprRef, ExprRef)>, else_block: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let mut block_types = Vec::new();

        // Check if-block
        let if_block = then_block.clone();
        let is_if_empty = match self.core.expr_pool.get(&if_block)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid if block expression reference"))? {
            Expr::Block(expressions) => expressions.is_empty(),
            _ => false,
        };
        if !is_if_empty {
            let if_expr = self.core.expr_pool.get(&if_block)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid if block expression reference"))?;
            let if_ty = if_expr.clone().accept(self)?;
            block_types.push(if_ty);
        }

        // Check elif-blocks
        for (_, elif_block) in elif_pairs {
            let elif_block = elif_block.clone();
            let is_elif_empty = match self.core.expr_pool.get(&elif_block)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid elif block expression reference"))? {
                Expr::Block(expressions) => expressions.is_empty(),
                _ => false,
            };
            if !is_elif_empty {
                let elif_expr = self.core.expr_pool.get(&elif_block)
                    .ok_or_else(|| TypeCheckError::generic_error("Invalid elif block expression reference"))?;
                let elif_ty = elif_expr.clone().accept(self)?;
                block_types.push(elif_ty);
            }
        }

        // Check else-block
        let else_block = else_block.clone();
        let is_else_empty = match self.core.expr_pool.get(&else_block)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid else block expression reference"))? {
            Expr::Block(expressions) => expressions.is_empty(),
            _ => false,
        };
        if !is_else_empty {
            let else_expr = self.core.expr_pool.get(&else_block)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid else block expression reference"))?;
            let else_ty = else_expr.clone().accept(self)?;
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

    /// Type check assignment expressions
    pub fn visit_assign(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let lhs = lhs.clone();
        let rhs = rhs.clone();
        
        let lhs_ty = {
            let lhs_obj = self.core.expr_pool.get(&lhs)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid left-hand expression reference"))?;
            lhs_obj.clone().accept(self)?
        };
        
        let rhs_ty = {
            let rhs_obj = self.core.expr_pool.get(&rhs)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid right-hand expression reference"))?;
            rhs_obj.clone().accept(self)?
        };
        
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
                        .unwrap_or_else(|| TypeDecl::Generic(*param))
                }).collect()
            } else {
                vec![]
            };
            Ok(TypeDecl::Struct(name, type_params))
        } else {
            let name_str = self.resolve_symbol_name(name);
            // Note: Location information will be added by visit_expr
            return Err(TypeCheckError::not_found("Identifier", &name_str));
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

    /// Whether `lhs` and `rhs` are both the same struct type and that struct
    /// has an `eq` method registered. Drives the Phase B operator-overload
    /// path that lets `s == t` dispatch to `s.eq(t)` for nominal struct
    /// values (e.g. `String` / `Vec<u8>`). Generic struct args must also
    /// match so `Vec<u8> == Vec<u8>` compares but `Vec<u8> == Vec<i64>`
    /// continues to bail with the standard mismatch error.
    fn struct_eq_compatible(&self, lhs: &TypeDecl, rhs: &TypeDecl) -> bool {
        self.struct_method_compatible(lhs, rhs, "eq")
    }

    /// Generalised version of `struct_eq_compatible` for arithmetic
    /// operator overloading (Phase B continuation). `+` / `-` / `*`
    /// / `/` / `%` dispatch to `add` / `sub` / `mul` / `div` / `rem`
    /// methods on the matching struct. Same nominal-identity rule
    /// as `eq`: same struct name + same generic args, otherwise
    /// fall through to the standard mismatch diagnostic.
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
        self.context.struct_methods.get(&lhs_name)
            .and_then(|m| m.get(&method_sym))
            .is_some()
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

    /// Type check function calls
    pub fn visit_call(&mut self, fn_name: DefaultSymbol, args_ref: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let _fn_name_str = self.resolve_symbol_name(fn_name);

        // Namespace-only enforcement: functions that came in through
        // `import` are only callable via the qualified
        // `module::func(args)` form (handled by
        // `visit_associated_function_call_impl`'s module-dispatch
        // branch). Bare `func(args)` calls into them are rejected so
        // every import site spells out where the function lives.
        if self.imported_function_names.contains(&fn_name) {
            let module_hint = self
                .imported_modules
                .keys()
                .find_map(|alias| alias.first().copied())
                .map(|sym| self.resolve_symbol_name(sym).to_string())
                .unwrap_or_else(|| "<module>".to_string());
            return Err(TypeCheckError::generic_error(&format!(
                "imported function '{}' must be called with the qualified form `{}::{}(...)`; bare-name calls into imported modules are not allowed",
                self.resolve_symbol_name(fn_name),
                module_hint,
                self.resolve_symbol_name(fn_name),
            )));
        }

        self.push_context();

        if let Some(fun) = self.context.get_fn(fn_name) {
            // Check visibility access control
            if let Err(err) = self.check_function_access(&fun) {
                self.pop_context();
                return Err(err);
            }

            // Auto-inject `ambient` for omitted trailing Allocator-typed parameters.
            // A parameter is defaultable when its type is `TypeDecl::Allocator` or a
            // generic parameter bounded by `Allocator`. Injection happens before the
            // generic-call dispatch so both paths see the extended argument list.
            self.inject_ambient_defaults(args_ref, &fun);

            // Handle generic function calls
            if !fun.generic_params.is_empty() {
                return self.visit_generic_call(fn_name, args_ref, &fun);
            }
            
            // Check if function has been type checked
            let status = self.function_checking.is_checked_fn.get(&fn_name);
            if status.is_none() || status.as_ref().and_then(|s| s.as_ref()).is_none() {
                // not checked yet
                let fun_copy = self.context.get_fn(fn_name)
                    .ok_or_else(|| TypeCheckError::not_found("Function", "<INTERNAL_ERROR>"))?;
                self.type_check(fun_copy.clone())?;
            }

            // Type-check the argument list against the resolved function
            // parameters, restoring the type-hint state and popping the
            // context-frame on every exit path.
            if let Err(err) = self.check_call_args_against_params(fn_name, args_ref, &fun) {
                self.pop_context();
                return Err(err);
            }

            self.pop_context();
            Ok(self.normalize_call_return_type(fun.return_type.clone().unwrap_or(TypeDecl::Unknown)))
        } else {
            // Function lookup miss — try the indirect-call path.
            // Closures Phase 2: when a binding holds a value of type
            // `TypeDecl::Function(params, ret)`, `f(args)` is an
            // indirect call. The parser can't tell statically whether
            // an `Identifier` is a fn decl or a value, so it always
            // emits `Expr::Call(name, args)`; this branch handles
            // the value case.
            self.pop_context();
            if let Some(callee_ty) = self.context.get_var(fn_name) {
                if let TypeDecl::Function(param_tys, ret_ty) = callee_ty {
                    return self.visit_indirect_call(fn_name, args_ref, &param_tys, &ret_ty);
                }
            }
            let fn_name_str = self.resolve_symbol_name(fn_name);
            Err(TypeCheckError::not_found("Function", &fn_name_str))
        }
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
            if let TypeDecl::Identifier(name) = ty {
                if self.context.struct_definitions.contains_key(name) {
                    return TypeDecl::Struct(*name, vec![]);
                }
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
            // `is_arg_compatible` handles the Identifier↔Struct /
            // Identifier↔Enum cases plus REF-Stage-2 auto-borrow
            // (`T` → `&T` at call sites).
            if !TypeDecl::is_arg_compatible(&arg_type, expected_type) && arg_type != TypeDecl::Unknown {
                self.type_inference.type_hint = original_hint;
                let fn_name_str = self.resolve_symbol_name(fn_name);
                return Err(TypeCheckError::generic_error(&format!(
                    "Type error: expected {:?}, found {:?}. Function '{}' argument {} type mismatch",
                    expected_type, arg_type, fn_name_str, arg_index + 1
                )));
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
        if let TypeDecl::Identifier(name) = &ret {
            if self.context.struct_definitions.contains_key(name) {
                return TypeDecl::Struct(*name, vec![]);
            }
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
            if !TypeDecl::is_arg_compatible(&arg_ty, expected) && arg_ty != TypeDecl::Unknown {
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

        // Push a fresh scope and bind each parameter.
        self.push_context();
        for (name, ty) in params {
            self.context.set_var(*name, ty.clone());
        }
        let body_result = self.visit_expr(body);
        self.pop_context();
        let body_ty = body_result?;

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
        if let Some(ret) = return_type {
            if Self::type_mentions_any_generic(ret) {
                return Err(TypeCheckError::generic_error(
                    "generic-parameterised closures are not yet supported",
                ));
            }
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
            Pattern::Wildcard | Pattern::Literal(_) => {}
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
        // Null value type is determined by context
        // If we have a type hint, use that; otherwise return Unknown
        if let Some(hint) = self.type_inference.get_type_hint() {
            Ok(hint)
        } else {
            Ok(TypeDecl::Unknown)
        }
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

}
