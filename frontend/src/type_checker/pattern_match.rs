//! Type checking for `match` expressions and their patterns.
//!
//! Split out of `visitor_impl.rs` so the 300+ lines of pattern / arm /
//! exhaustiveness logic live next to each other and don't drown the
//! dispatch wrappers. Entry points used from the trait impl:
//!
//! - `TypeCheckerVisitor::visit_match_impl` — top-level match type check
//! - `TypeCheckerVisitor::check_sub_pattern` — recursive sub-pattern
//!   validation against a payload type

use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckError, TypeCheckerVisitor};

/// A pattern is irrefutable when it always matches any value of the expected
/// type. `Name` and `Wildcard` are irrefutable. Literals are refutable by
/// value. An `EnumVariant` pattern narrows to a single variant, so it is
/// refutable in any enum with more than one variant — we conservatively
/// treat it as refutable, since the check only affects whether an already-
/// seen top-level variant triggers an "unreachable arm" error when the
/// same variant reappears with different sub-patterns.
pub(super) fn is_irrefutable_pattern(pat: &Pattern) -> bool {
    match pat {
        Pattern::Wildcard | Pattern::Name(_) => true,
        Pattern::Literal(_) | Pattern::EnumVariant(_, _, _) => false,
        // A tuple pattern is irrefutable iff every sub-pattern is.
        Pattern::Tuple(subs) => subs.iter().all(is_irrefutable_pattern),
    }
}

impl<'a> TypeCheckerVisitor<'a> {
    /// Recursively type-check a sub-pattern against the expected payload type.
    /// Introduces any `Name` bindings into the *current* variable scope, which
    /// callers are responsible for pushing/popping around the arm body.
    pub(super) fn check_sub_pattern(&mut self, pat: &Pattern, expected_ty: &TypeDecl) -> Result<(), TypeCheckError> {
        match pat {
            Pattern::Wildcard => Ok(()),
            Pattern::Name(sym) => {
                self.context.set_var(*sym, expected_ty.clone());
                Ok(())
            }
            Pattern::Literal(lit_expr) => {
                if !matches!(expected_ty, TypeDecl::Bool | TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::String) {
                    return Err(TypeCheckError::new(format!(
                        "literal pattern is only valid where a primitive value is expected, got {:?}",
                        expected_ty
                    )));
                }
                let saved_hint = self.type_inference.type_hint.clone();
                self.type_inference.type_hint = Some(expected_ty.clone());
                let lit_ty = self.visit_expr(lit_expr)?;
                self.type_inference.type_hint = saved_hint;
                if !lit_ty.is_equivalent(expected_ty) {
                    return Err(TypeCheckError::new(format!(
                        "literal pattern type {:?} does not match expected {:?}",
                        lit_ty, expected_ty
                    )));
                }
                Ok(())
            }
            Pattern::Tuple(sub_patterns) => {
                let element_types = match expected_ty {
                    TypeDecl::Tuple(ts) => ts,
                    _ => {
                        return Err(TypeCheckError::new(format!(
                            "tuple pattern requires a tuple value, got {:?}",
                            expected_ty
                        )));
                    }
                };
                if sub_patterns.len() != element_types.len() {
                    return Err(TypeCheckError::new(format!(
                        "tuple pattern has {} element(s), expected {}",
                        sub_patterns.len(),
                        element_types.len()
                    )));
                }
                for (sub, ty) in sub_patterns.iter().zip(element_types.iter()) {
                    self.check_sub_pattern(sub, ty)?;
                }
                Ok(())
            }
            Pattern::EnumVariant(pat_enum, pat_variant, sub_patterns) => {
                // Extract the enum name + type args from the expected payload
                // type. Accept Enum, Struct (parser can emit this), or
                // Identifier forms, the same way the top-level match logic
                // does.
                let (enum_name, enum_type_args) = match expected_ty {
                    TypeDecl::Enum(name, args) => (*name, args.clone()),
                    TypeDecl::Struct(name, args)
                        if self.context.enum_definitions.contains_key(name) => (*name, args.clone()),
                    TypeDecl::Identifier(name)
                        if self.context.enum_definitions.contains_key(name) => (*name, Vec::new()),
                    _ => {
                        return Err(TypeCheckError::new(format!(
                            "enum-variant sub-pattern expects an enum payload, got {:?}",
                            expected_ty
                        )));
                    }
                };
                if *pat_enum != enum_name {
                    let expected = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                    let got = self.core.string_interner.resolve(*pat_enum).unwrap_or("?").to_string();
                    return Err(TypeCheckError::new(format!(
                        "nested pattern refers to enum '{}', but payload type is enum '{}'", got, expected
                    )));
                }
                let variants = self.context.enum_definitions.get(&enum_name).cloned()
                    .ok_or_else(|| TypeCheckError::new("nested match on unknown enum".to_string()))?;
                let variant_def = variants.iter().find(|v| v.name == *pat_variant)
                    .cloned()
                    .ok_or_else(|| {
                        let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                        let v_str = self.core.string_interner.resolve(*pat_variant).unwrap_or("?").to_string();
                        TypeCheckError::new(format!("'{}' is not a variant of enum '{}'", v_str, enum_str))
                    })?;
                if sub_patterns.len() != variant_def.payload_types.len() {
                    let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                    let v_str = self.core.string_interner.resolve(*pat_variant).unwrap_or("?").to_string();
                    return Err(TypeCheckError::new(format!(
                        "variant '{}::{}' has {} payload field(s) but pattern bound {}",
                        enum_str, v_str, variant_def.payload_types.len(), sub_patterns.len()
                    )));
                }
                let generic_params = self.context.enum_generic_params.get(&enum_name).cloned().unwrap_or_default();
                let mut substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> = std::collections::HashMap::new();
                for (param, arg) in generic_params.iter().zip(enum_type_args.iter()) {
                    substitutions.insert(*param, arg.clone());
                }
                for (sub, payload_ty) in sub_patterns.iter().zip(variant_def.payload_types.iter()) {
                    let resolved = payload_ty.substitute_generics(&substitutions);
                    self.check_sub_pattern(sub, &resolved)?;
                }
                Ok(())
            }
        }
    }

    /// Entry point for `Expr::Match`. Classifies the scrutinee, walks arms
    /// accumulating coverage, then enforces exhaustiveness and arm-type
    /// agreement.
    pub(super) fn visit_match_impl(
        &mut self,
        scrutinee: &ExprRef,
        arms: &Vec<MatchArm>,
    ) -> Result<TypeDecl, TypeCheckError> {
        if arms.is_empty() {
            return Err(TypeCheckError::new("match expression must have at least one arm".to_string()));
        }
        let scrutinee_ty = self.visit_expr(scrutinee)?;

        // Classify the scrutinee. Enum matches and primitive matches accept
        // different pattern shapes, so we dispatch on this up-front.
        enum ScrutineeKind {
            Enum {
                name: DefaultSymbol,
                type_args: Vec<TypeDecl>,
                variants: Vec<crate::ast::EnumVariantDef>,
            },
            Primitive(TypeDecl),
            // The element types are validated when each tuple-pattern
            // arm is processed, but the wrapper here keeps the
            // dispatch-by-kind shape uniform.
            Tuple(#[allow(dead_code)] Vec<TypeDecl>),
        }
        let kind = match &scrutinee_ty {
            TypeDecl::Enum(name, args) => {
                let variants = self.context.enum_definitions.get(name)
                    .cloned()
                    .ok_or_else(|| TypeCheckError::new("match on unknown enum".to_string()))?;
                ScrutineeKind::Enum { name: *name, type_args: args.clone(), variants }
            }
            TypeDecl::Identifier(name) if self.context.enum_definitions.contains_key(name) => {
                let variants = self.context.enum_definitions.get(name).cloned().unwrap();
                ScrutineeKind::Enum { name: *name, type_args: Vec::new(), variants }
            }
            TypeDecl::Struct(name, args) if self.context.enum_definitions.contains_key(name) => {
                let variants = self.context.enum_definitions.get(name).cloned().unwrap();
                ScrutineeKind::Enum { name: *name, type_args: args.clone(), variants }
            }
            TypeDecl::Bool | TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::String => ScrutineeKind::Primitive(scrutinee_ty.clone()),
            TypeDecl::Tuple(element_types) => ScrutineeKind::Tuple(element_types.clone()),
            _ => {
                return Err(TypeCheckError::new(format!(
                    "match scrutinee must be an enum, primitive (bool / i64 / u64 / str), or tuple, got {:?}",
                    scrutinee_ty
                )));
            }
        };

        // Track coverage to enforce exhaustiveness and reject unreachable arms.
        let mut arm_types: Vec<TypeDecl> = Vec::with_capacity(arms.len());
        // Two sets because of nested patterns:
        //  - `fully_covered_variants` gates the unreachable-arm check and only
        //    includes variants whose sub-patterns were all irrefutable.
        //  - `seen_variants` gates exhaustiveness; any arm for a variant
        //    counts, since exhaustiveness across arbitrary nested patterns is
        //    undecidable in our simple analysis.
        let mut fully_covered_variants: std::collections::HashSet<DefaultSymbol> = std::collections::HashSet::new();
        let mut seen_variants: std::collections::HashSet<DefaultSymbol> = std::collections::HashSet::new();
        // Deep-exhaustiveness tracking (96残 前半): for each top-level
        // variant that some arm matched without a guard, record the
        // arm's payload binding list. After the simple
        // `seen_variants` check passes, we recursively walk these
        // bindings to confirm every nested case is also covered.
        // Without this, `match opt: Option<Option<i64>> {
        // Some(Some(v)) => ..., None => ... }` slipped through with
        // a runtime "no matching arm" panic on `Some(None)`.
        let mut variant_payload_arms: std::collections::HashMap<DefaultSymbol, Vec<Vec<crate::ast::Pattern>>> =
            std::collections::HashMap::new();
        let mut covered_int64: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let mut covered_uint64: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut covered_bool: std::collections::HashSet<bool> = std::collections::HashSet::new();
        let mut covered_strings: std::collections::HashSet<DefaultSymbol> = std::collections::HashSet::new();
        let mut has_wildcard = false;
        for (arm_index, arm) in arms.iter().enumerate() {
            let pat = &arm.pattern;
            let body = &arm.body;
            let is_guarded = arm.guard.is_some();
            if has_wildcard {
                return Err(TypeCheckError::new(format!(
                    "unreachable match arm at position {}: a wildcard `_` arm already covers every value",
                    arm_index
                )));
            }
            let mut pushed_scope = false;
            match pat {
                Pattern::Wildcard => {
                    if !is_guarded {
                        has_wildcard = true;
                    }
                }
                Pattern::Name(sym) => {
                    // Bare name at top level binds the whole scrutinee; it is
                    // irrefutable and therefore acts like a wildcard for
                    // exhaustiveness — unless guarded, in which case the
                    // guard can fail at runtime so coverage is not total.
                    self.context.vars.push(std::collections::HashMap::new());
                    pushed_scope = true;
                    self.context.set_var(*sym, scrutinee_ty.clone());
                    if !is_guarded {
                        has_wildcard = true;
                    }
                }
                Pattern::Literal(literal_expr) => {
                    let prim_ty = match &kind {
                        ScrutineeKind::Primitive(t) => t.clone(),
                        ScrutineeKind::Enum { .. } => {
                            return Err(TypeCheckError::new(
                                "literal pattern cannot be used in a match on an enum".to_string()
                            ));
                        }
                        ScrutineeKind::Tuple(_) => {
                            return Err(TypeCheckError::new(
                                "literal pattern cannot be used in a match on a tuple".to_string()
                            ));
                        }
                    };
                    // Literal expression must have the same primitive type as
                    // the scrutinee. We visit it with the scrutinee type as a
                    // hint so bare numeric literals pick up i64 / u64.
                    let saved_hint = self.type_inference.type_hint.clone();
                    self.type_inference.type_hint = Some(prim_ty.clone());
                    let lit_ty = self.visit_expr(literal_expr)?;
                    self.type_inference.type_hint = saved_hint;
                    if !lit_ty.is_equivalent(&prim_ty) {
                        return Err(TypeCheckError::new(format!(
                            "literal pattern type {:?} does not match scrutinee type {:?}",
                            lit_ty, prim_ty
                        )));
                    }
                    // Record the concrete literal value for duplicate /
                    // exhaustiveness checks. A guarded arm does not fully
                    // cover its literal (the guard might be false at run
                    // time), so we skip the bookkeeping when `is_guarded`.
                    if !is_guarded {
                        if let Some(lit_expr) = self.core.expr_pool.get(literal_expr) {
                            match lit_expr {
                                Expr::Int64(v) => {
                                    if !covered_int64.insert(v) {
                                        return Err(TypeCheckError::new(format!(
                                            "unreachable match arm: literal {} already handled by an earlier arm", v
                                        )));
                                    }
                                }
                                Expr::UInt64(v) => {
                                    if !covered_uint64.insert(v) {
                                        return Err(TypeCheckError::new(format!(
                                            "unreachable match arm: literal {} already handled by an earlier arm", v
                                        )));
                                    }
                                }
                                Expr::True => {
                                    if !covered_bool.insert(true) {
                                        return Err(TypeCheckError::new(
                                            "unreachable match arm: literal `true` already handled by an earlier arm".to_string()
                                        ));
                                    }
                                }
                                Expr::False => {
                                    if !covered_bool.insert(false) {
                                        return Err(TypeCheckError::new(
                                            "unreachable match arm: literal `false` already handled by an earlier arm".to_string()
                                        ));
                                    }
                                }
                                Expr::String(sym) => {
                                    if !covered_strings.insert(sym) {
                                        let s = self.core.string_interner.resolve(sym).unwrap_or("?").to_string();
                                        return Err(TypeCheckError::new(format!(
                                            "unreachable match arm: literal {:?} already handled by an earlier arm",
                                            s
                                        )));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Pattern::Tuple(sub_patterns) => {
                    // Tuple matches are independent of enum dispatch;
                    // require the scrutinee to be a tuple type and
                    // type-check each element through `check_sub_pattern`.
                    let element_types = match &scrutinee_ty {
                        TypeDecl::Tuple(ts) => ts.clone(),
                        _ => {
                            return Err(TypeCheckError::new(format!(
                                "tuple pattern requires a tuple scrutinee, got {:?}",
                                scrutinee_ty
                            )));
                        }
                    };
                    if sub_patterns.len() != element_types.len() {
                        return Err(TypeCheckError::new(format!(
                            "tuple pattern has {} element(s), expected {}",
                            sub_patterns.len(),
                            element_types.len()
                        )));
                    }
                    self.context.vars.push(std::collections::HashMap::new());
                    pushed_scope = true;
                    for (sub, ty) in sub_patterns.iter().zip(element_types.iter()) {
                        self.check_sub_pattern(sub, ty)?;
                    }
                    // A tuple of irrefutable sub-patterns covers all
                    // possible tuple values, so it acts as a wildcard
                    // for exhaustiveness — except when the arm is
                    // guarded (the guard can fail at runtime).
                    if !is_guarded && sub_patterns.iter().all(is_irrefutable_pattern) {
                        has_wildcard = true;
                    }
                }
                Pattern::EnumVariant(pat_enum, pat_variant, bindings) => {
                    let (enum_name, enum_type_args, variants) = match &kind {
                        ScrutineeKind::Enum { name, type_args, variants } => (*name, type_args.clone(), variants.clone()),
                        ScrutineeKind::Primitive(t) => {
                            return Err(TypeCheckError::new(format!(
                                "enum-variant pattern cannot be used in a match on {:?}", t
                            )));
                        }
                        ScrutineeKind::Tuple(_) => {
                            return Err(TypeCheckError::new(
                                "enum-variant pattern cannot be used in a match on a tuple".to_string()
                            ));
                        }
                    };
                    if *pat_enum != enum_name {
                        let expected = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                        let got = self.core.string_interner.resolve(*pat_enum).unwrap_or("?").to_string();
                        return Err(TypeCheckError::new(format!(
                            "match pattern refers to enum '{}', but scrutinee is '{}'", got, expected
                        )));
                    }
                    let variant_def = variants.iter().find(|v| v.name == *pat_variant);
                    let variant_def = match variant_def {
                        Some(v) => v,
                        None => {
                            let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                            let v_str = self.core.string_interner.resolve(*pat_variant).unwrap_or("?").to_string();
                            return Err(TypeCheckError::new(format!(
                                "'{}' is not a variant of enum '{}'", v_str, enum_str
                            )));
                        }
                    };
                    // `Option::Some(Some(x))` and `Option::Some(None)` share
                    // the top variant `Some` but aren't redundant — they
                    // cover disjoint sub-patterns. So we only treat a
                    // variant as redundant when an earlier arm's sub-patterns
                    // are all irrefutable (Name / Wildcard at every slot).
                    if fully_covered_variants.contains(pat_variant) {
                        let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                        let v_str = self.core.string_interner.resolve(*pat_variant).unwrap_or("?").to_string();
                        return Err(TypeCheckError::new(format!(
                            "unreachable match arm: variant '{}::{}' already fully covered by an earlier arm",
                            enum_str, v_str
                        )));
                    }
                    if bindings.len() != variant_def.payload_types.len() {
                        let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                        let v_str = self.core.string_interner.resolve(*pat_variant).unwrap_or("?").to_string();
                        return Err(TypeCheckError::new(format!(
                            "variant '{}::{}' has {} payload field(s) but pattern bound {}",
                            enum_str, v_str, variant_def.payload_types.len(), bindings.len()
                        )));
                    }
                    if !bindings.is_empty() {
                        self.context.vars.push(std::collections::HashMap::new());
                        pushed_scope = true;
                        let generic_params = self.context.enum_generic_params.get(&enum_name).cloned().unwrap_or_default();
                        let mut substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> = std::collections::HashMap::new();
                        for (param, arg) in generic_params.iter().zip(enum_type_args.iter()) {
                            substitutions.insert(*param, arg.clone());
                        }
                        for (sub_pat, payload_ty) in bindings.iter().zip(variant_def.payload_types.iter()) {
                            let resolved = payload_ty.substitute_generics(&substitutions);
                            self.check_sub_pattern(sub_pat, &resolved)?;
                        }
                    }
                    // Only mark the variant as fully covered if every
                    // sub-pattern is irrefutable AND the arm is unguarded.
                    // Refutable sub-patterns or a guard leave part of the
                    // variant's value space unmatched, so another arm
                    // targeting the same variant can still be useful.
                    if !is_guarded && bindings.iter().all(is_irrefutable_pattern) {
                        fully_covered_variants.insert(*pat_variant);
                    }
                    if !is_guarded {
                        seen_variants.insert(*pat_variant);
                        // Stash the arm's payload bindings so the
                        // deep-exhaustiveness pass below can walk
                        // them position-by-position. Guarded arms
                        // are excluded for the same reason
                        // `seen_variants` excludes them — a guard
                        // can fail at runtime so the arm doesn't
                        // contribute to compile-time coverage.
                        variant_payload_arms
                            .entry(*pat_variant)
                            .or_insert_with(Vec::new)
                            .push(bindings.clone());
                    }
                }
            }
            // Guards see the pattern's bindings, so type-check them in
            // the arm scope before the body.
            if let Some(guard_expr) = arm.guard {
                let saved_hint = self.type_inference.type_hint.clone();
                self.type_inference.type_hint = Some(TypeDecl::Bool);
                let guard_ty = self.visit_expr(&guard_expr)?;
                self.type_inference.type_hint = saved_hint;
                if !guard_ty.is_equivalent(&TypeDecl::Bool) {
                    if pushed_scope {
                        self.context.vars.pop();
                    }
                    return Err(TypeCheckError::new(format!(
                        "match arm guard must be of type bool, got {:?}",
                        guard_ty
                    )));
                }
            }
            let body_ty = self.visit_expr(body)?;
            if pushed_scope {
                self.context.vars.pop();
            }
            arm_types.push(body_ty);
        }

        // Exhaustiveness. Enums must cover every variant. `bool` must cover
        // both `true` and `false`. Other primitives have an unbounded value
        // space, so a wildcard is mandatory.
        if !has_wildcard {
            match &kind {
                ScrutineeKind::Enum { name, variants, .. } => {
                    let missing: Vec<DefaultSymbol> = variants.iter()
                        .filter(|v| !seen_variants.contains(&v.name))
                        .map(|v| v.name)
                        .collect();
                    if !missing.is_empty() {
                        let enum_str = self.core.string_interner.resolve(*name).unwrap_or("?").to_string();
                        let missing_strs: Vec<String> = missing.iter()
                            .map(|s| self.core.string_interner.resolve(*s).unwrap_or("?").to_string())
                            .collect();
                        return Err(TypeCheckError::new(format!(
                            "non-exhaustive match on enum '{}': missing variant(s) {} — add an arm for each or a wildcard `_`",
                            enum_str,
                            missing_strs.join(", ")
                        )));
                    }
                }
                ScrutineeKind::Primitive(TypeDecl::Bool) => {
                    if !covered_bool.contains(&true) || !covered_bool.contains(&false) {
                        return Err(TypeCheckError::new(
                            "non-exhaustive match on bool: cover both `true` and `false` or add a wildcard `_`".to_string()
                        ));
                    }
                }
                ScrutineeKind::Primitive(t) => {
                    let t_name = match t {
                        TypeDecl::Int64 => "i64".to_string(),
                        TypeDecl::UInt64 => "u64".to_string(),
                        TypeDecl::String => "str".to_string(),
                        other => format!("{:?}", other),
                    };
                    return Err(TypeCheckError::new(format!(
                        "non-exhaustive match on {}: primitive value space is unbounded, add a wildcard `_` arm",
                        t_name
                    )));
                }
                ScrutineeKind::Tuple(_) => {
                    // Tuple value space is unbounded along each element;
                    // the user must include either an irrefutable tuple
                    // pattern (`(x, y)`) or a wildcard `_` arm.
                    return Err(TypeCheckError::new(
                        "non-exhaustive match on tuple: add an arm with an irrefutable tuple pattern or a wildcard `_`".to_string()
                    ));
                }
            }
        }

        // Deep exhaustiveness (96残 前半): for each top variant
        // covered without a fully-irrefutable arm, walk the
        // payload positions and confirm any nested enum sub-
        // patterns are also exhaustive. The overall match is
        // exhaustive only if every top variant is fully covered
        // (has_wildcard catches the rest). Wildcard / Name arm
        // earlier already shortcuts every check, so this only
        // runs when the user spelled out the variant arms.
        if !has_wildcard {
            if let ScrutineeKind::Enum { name: enum_name, type_args, variants } = &kind {
                for variant in variants.iter() {
                    if fully_covered_variants.contains(&variant.name) {
                        continue;
                    }
                    if !seen_variants.contains(&variant.name) {
                        // Already reported by the simple
                        // missing-variant check above.
                        continue;
                    }
                    let arms_for_variant = match variant_payload_arms.get(&variant.name) {
                        Some(v) => v.clone(),
                        None => continue,
                    };
                    let generic_params = self.context.enum_generic_params.get(enum_name).cloned().unwrap_or_default();
                    let mut substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> = std::collections::HashMap::new();
                    for (param, arg) in generic_params.iter().zip(type_args.iter()) {
                        substitutions.insert(*param, arg.clone());
                    }
                    for (pos, payload_ty) in variant.payload_types.iter().enumerate() {
                        let resolved = payload_ty.substitute_generics(&substitutions);
                        let subpatterns_at_pos: Vec<crate::ast::Pattern> = arms_for_variant
                            .iter()
                            .map(|bindings| bindings[pos].clone())
                            .collect();
                        let enum_str = self.core.string_interner.resolve(*enum_name).unwrap_or("?").to_string();
                        let v_str = self.core.string_interner.resolve(variant.name).unwrap_or("?").to_string();
                        let context = format!("inside `{}::{}` payload position {}", enum_str, v_str, pos);
                        self.check_subpatterns_exhaustive(&subpatterns_at_pos, &resolved, &context)?;
                    }
                }
            }
        }

        // All arms must share a common type.
        let first = arm_types[0].clone();
        for (i, t) in arm_types.iter().enumerate().skip(1) {
            if !first.is_equivalent(t) {
                return Err(TypeCheckError::new(format!(
                    "match arms have incompatible types: arm 0 is {:?}, arm {} is {:?}",
                    first, i, t
                )));
            }
        }
        Ok(first)
    }

    /// Recursive helper for deep exhaustiveness. Determines whether
    /// the given patterns cover every value of `position_type`.
    /// The `context` string is used in error messages to point at
    /// the payload position being checked.
    ///
    /// Strategy:
    /// - If any pattern is irrefutable (Wildcard / Name), the position
    ///   is fully covered — return Ok.
    /// - If `position_type` is an Enum, group EnumVariant patterns by
    ///   variant name. For each variant of the enum:
    ///     * If no arm covers it → missing, error.
    ///     * If some arm covers it with all-irrefutable sub-bindings →
    ///       fully covered, continue.
    ///     * Otherwise: recursively check each payload position with
    ///       the gathered sub-patterns.
    /// - For other types (primitive / tuple / unsupported), require an
    ///   irrefutable pattern — without one, conservatively error so
    ///   the runtime never sees an unmatched value.
    fn check_subpatterns_exhaustive(
        &self,
        patterns: &[crate::ast::Pattern],
        position_type: &TypeDecl,
        context: &str,
    ) -> Result<(), TypeCheckError> {
        use crate::ast::Pattern;
        // Any irrefutable pattern (Wildcard, Name) at this position
        // covers all values — short-circuit.
        if patterns.iter().any(is_irrefutable_pattern) {
            return Ok(());
        }
        // Resolve the position type. Generic substitution has already
        // been applied at the call site, so this only re-shapes
        // `Identifier(enum_name)` into the canonical Enum form when
        // the type checker hasn't fully propagated it.
        // The frontend sometimes carries generic enum types as
        // `TypeDecl::Struct(name, args)` (the type checker hasn't
        // yet promoted them to `Enum`). Treat both forms uniformly
        // by consulting `enum_definitions` for any name-bearing
        // shape.
        let resolved = match position_type {
            TypeDecl::Identifier(sym) if self.context.enum_definitions.contains_key(sym) => {
                TypeDecl::Enum(*sym, Vec::new())
            }
            TypeDecl::Struct(sym, args) if self.context.enum_definitions.contains_key(sym) => {
                TypeDecl::Enum(*sym, args.clone())
            }
            other => other.clone(),
        };
        let (enum_name, type_args, variants) = match &resolved {
            TypeDecl::Enum(name, args) => {
                let v = match self.context.enum_definitions.get(name) {
                    Some(v) => v.clone(),
                    None => return Ok(()), // Unknown enum — defer to other checks.
                };
                (*name, args.clone(), v)
            }
            _ => {
                // Non-enum position with no irrefutable pattern means
                // the position can hide unmatched values. Be
                // conservative and reject.
                return Err(TypeCheckError::new(format!(
                    "non-exhaustive match {}: position type {:?} is not fully covered — add a wildcard `_` or a bare name",
                    context, position_type
                )));
            }
        };
        // Group sub-patterns by variant name; collect refutability
        // and per-arm payload bindings.
        let mut covered_variants: std::collections::HashSet<DefaultSymbol> = std::collections::HashSet::new();
        let mut fully_covered: std::collections::HashSet<DefaultSymbol> = std::collections::HashSet::new();
        let mut variant_arms: std::collections::HashMap<DefaultSymbol, Vec<Vec<Pattern>>> =
            std::collections::HashMap::new();
        for pat in patterns {
            if let Pattern::EnumVariant(p_enum, p_variant, bindings) = pat {
                if *p_enum != enum_name {
                    continue;
                }
                covered_variants.insert(*p_variant);
                if bindings.iter().all(is_irrefutable_pattern) {
                    fully_covered.insert(*p_variant);
                } else {
                    variant_arms
                        .entry(*p_variant)
                        .or_insert_with(Vec::new)
                        .push(bindings.clone());
                }
            }
        }
        // Missing variants: any enum variant not covered at all.
        let missing: Vec<DefaultSymbol> = variants
            .iter()
            .filter(|v| !covered_variants.contains(&v.name))
            .map(|v| v.name)
            .collect();
        if !missing.is_empty() {
            let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
            let missing_strs: Vec<String> = missing
                .iter()
                .map(|s| self.core.string_interner.resolve(*s).unwrap_or("?").to_string())
                .collect();
            return Err(TypeCheckError::new(format!(
                "non-exhaustive match {}: missing nested variant(s) {}::{{{}}} — add an arm for each or a wildcard / bare name",
                context,
                enum_str,
                missing_strs.join(", ")
            )));
        }
        // For each refutable-only variant, recurse into each payload
        // position to check the gathered sub-patterns.
        let generic_params = self.context.enum_generic_params.get(&enum_name).cloned().unwrap_or_default();
        let mut substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> = std::collections::HashMap::new();
        for (param, arg) in generic_params.iter().zip(type_args.iter()) {
            substitutions.insert(*param, arg.clone());
        }
        for variant in variants.iter() {
            if fully_covered.contains(&variant.name) {
                continue;
            }
            let arms = match variant_arms.get(&variant.name) {
                Some(a) => a,
                None => continue,
            };
            for (pos, payload_ty) in variant.payload_types.iter().enumerate() {
                let resolved_pty = payload_ty.substitute_generics(&substitutions);
                let subpatterns_at_pos: Vec<Pattern> =
                    arms.iter().map(|b| b[pos].clone()).collect();
                let v_str = self.core.string_interner.resolve(variant.name).unwrap_or("?").to_string();
                let nested_context = format!("{} → `{}` payload position {}", context, v_str, pos);
                self.check_subpatterns_exhaustive(&subpatterns_at_pos, &resolved_pty, &nested_context)?;
            }
        }
        Ok(())
    }
}
