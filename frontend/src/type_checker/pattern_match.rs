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
        Pattern::Literal(_) | Pattern::Range(_, _) | Pattern::EnumVariant(_, _, _) => false,
        // A tuple pattern is irrefutable iff every sub-pattern is.
        Pattern::Tuple(subs) => subs.iter().all(is_irrefutable_pattern),
        // PATTERN-STRUCT: a struct has one shape, so the pattern can
        // only fail through a field pattern that can fail.
        Pattern::Struct(_, fields, _) => {
            fields.iter().all(|(_, sub)| is_irrefutable_pattern(sub))
        }
        // PATTERN-EXTEND: `n @ pat` rejects exactly what `pat` rejects.
        Pattern::Binding(_, inner) => is_irrefutable_pattern(inner),
    }
}

/// PATTERN-EXTEND: strip any `n @` wrappers, leaving the pattern that
/// actually decides whether the arm runs. Every coverage rule looks
/// through a binding, so the analyses call this before matching on
/// the shape.
pub(super) fn peel_bindings(pat: &Pattern) -> &Pattern {
    let mut cur = pat;
    while let Pattern::Binding(_, inner) = cur {
        cur = inner;
    }
    cur
}

/// PATTERN-EXTEND: the integer values a `match` has covered so far, as
/// a set of disjoint closed intervals over `i128` — wide enough to
/// hold `i64` and `u64` endpoints without a signed / unsigned split.
/// A literal is the one-value interval `[v, v]`; a half-open `lo..hi`
/// is `[lo, hi - 1]`.
///
/// Two questions are asked of it, and both need the union rather than
/// a per-arm view: whether an arm adds anything (one that does not is
/// unreachable), and whether the type's whole value space is covered
/// (in which case the `match` needs no wildcard).
#[derive(Default)]
struct IntCoverage {
    /// Sorted, disjoint, and re-merged on every insert.
    spans: Vec<(i128, i128)>,
}

impl IntCoverage {
    /// True when every value in `[lo, hi]` is already covered. Exact,
    /// because `insert` keeps the spans maximal: a span that covers
    /// the interval at all covers it on its own.
    fn contains(&self, lo: i128, hi: i128) -> bool {
        self.spans.iter().any(|(a, b)| *a <= lo && hi <= *b)
    }

    fn insert(&mut self, lo: i128, hi: i128) {
        self.spans.push((lo, hi));
        self.spans.sort_unstable();
        let mut merged: Vec<(i128, i128)> = Vec::with_capacity(self.spans.len());
        for (a, b) in self.spans.drain(..) {
            match merged.last_mut() {
                // `a <= last.1 + 1` merges *adjacent* spans, not just
                // overlapping ones, so `0i64..5i64` followed by
                // `5i64..10i64` becomes one span. That is what lets a
                // partition of the type count as exhaustive.
                Some(last) if a <= last.1 + 1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        self.spans = merged;
    }
}

/// The full value space of the primitive being matched, as the closed
/// interval a complete set of arms would have to cover.
fn integer_type_span(ty: &TypeDecl) -> Option<(i128, i128)> {
    match ty {
        TypeDecl::Int64 => Some((i64::MIN as i128, i64::MAX as i128)),
        TypeDecl::UInt64 => Some((0, u64::MAX as i128)),
        TypeDecl::Int8 => Some((i8::MIN as i128, i8::MAX as i128)),
        TypeDecl::Int16 => Some((i16::MIN as i128, i16::MAX as i128)),
        TypeDecl::Int32 => Some((i32::MIN as i128, i32::MAX as i128)),
        TypeDecl::UInt8 => Some((0, u8::MAX as i128)),
        TypeDecl::UInt16 => Some((0, u16::MAX as i128)),
        TypeDecl::UInt32 => Some((0, u32::MAX as i128)),
        _ => None,
    }
}

/// CHAR-LITERAL-MATCH: the integers a `match` may switch on — `i64`,
/// `u64` and the six narrow widths.
fn is_matchable_integer(ty: &TypeDecl) -> bool {
    integer_type_span(ty).is_some()
}

/// The value of an integer literal node, whatever width it was written
/// at. A char literal that no position narrowed is still its code
/// point, and an unsuffixed literal is still `Number` in the pool, its
/// interned text the value. `None` for anything that is not an
/// integer literal.
fn integer_literal_value(
    expr: &Expr,
    interner: &string_interner::DefaultStringInterner,
) -> Option<i128> {
    Some(match expr {
        Expr::Int64(v) => *v as i128,
        Expr::UInt64(v) => *v as i128,
        Expr::Int8(v) => *v as i128,
        Expr::Int16(v) => *v as i128,
        Expr::Int32(v) => *v as i128,
        Expr::UInt8(v) => *v as i128,
        Expr::UInt16(v) => *v as i128,
        Expr::UInt32(v) => *v as i128,
        Expr::CharLiteral(cp) => *cp as i128,
        Expr::Number(sym) => interner.resolve(*sym)?.replace('_', "").parse::<i128>().ok()?,
        _ => return None,
    })
}

impl<'a> TypeCheckerVisitor<'a> {
    /// PATTERN-STRUCT: check `Point { x: 0i64, y }` against the value
    /// being matched, and bind whatever the field patterns name.
    ///
    /// The struct's own name has to agree with the value's, since a
    /// pattern naming another struct could never match. Without `..`
    /// every field must be listed: a pattern that silently ignores a
    /// field it does not mention reads as complete when it is not, and
    /// a field added later would slip past every existing pattern.
    pub(super) fn check_struct_pattern(
        &mut self,
        struct_name: DefaultSymbol,
        field_patterns: &[(DefaultSymbol, Pattern)],
        has_rest: bool,
        expected_ty: &TypeDecl,
    ) -> Result<(), TypeCheckError> {
        let value_name = match expected_ty {
            TypeDecl::Struct(name, _) | TypeDecl::Identifier(name) => *name,
            _ => {
                return Err(TypeCheckError::new(format!(
                    "struct pattern requires a struct value, got {}",
                    self.type_name_for_error(expected_ty)
                )));
            }
        };
        if value_name != struct_name {
            return Err(TypeCheckError::new(format!(
                "struct pattern names `{}`, but the value is `{}`",
                self.resolve_symbol_name(struct_name),
                self.resolve_symbol_name(value_name)
            )));
        }
        let Some(declared) = self.context.get_struct_fields(struct_name).cloned() else {
            return Err(TypeCheckError::new(format!(
                "struct `{}` is not defined",
                self.resolve_symbol_name(struct_name)
            )));
        };

        for (field, sub) in field_patterns {
            let field_name = self.resolve_symbol_name(*field);
            let Some(decl) = declared.iter().find(|f| f.name == field_name) else {
                return Err(TypeCheckError::new(format!(
                    "struct `{}` has no field `{}`",
                    self.resolve_symbol_name(struct_name),
                    field_name
                )));
            };
            self.check_sub_pattern(sub, &decl.type_decl)?;
        }

        if !has_rest {
            let listed: Vec<String> = field_patterns
                .iter()
                .map(|(f, _)| self.resolve_symbol_name(*f))
                .collect();
            let missing: Vec<String> = declared
                .iter()
                .filter(|d| !listed.contains(&d.name))
                .map(|d| d.name.clone())
                .collect();
            if !missing.is_empty() {
                // NEWTYPE: a tuple struct's fields are named by index,
                // so "does not mention 1" would read as a count rather
                // than a position. Name what the numbers are.
                let noun = if declared.iter().all(|d| d.is_positional()) {
                    if missing.len() == 1 { "field " } else { "fields " }
                } else {
                    ""
                };
                return Err(TypeCheckError::new(format!(
                    "struct pattern for `{}` does not mention {}{} — list {} or end the pattern with `..`",
                    self.resolve_symbol_name(struct_name),
                    noun,
                    missing.join(", "),
                    if missing.len() == 1 { "it" } else { "them" }
                )));
            }
        }
        Ok(())
    }

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
                if !matches!(expected_ty, TypeDecl::Bool | TypeDecl::String) && !is_matchable_integer(expected_ty) {
                    return Err(TypeCheckError::new(format!(
                        "literal pattern is only valid where a primitive value is expected, got {}",
                        self.type_name_for_error(expected_ty)
                    )));
                }
                let saved_hint = self.type_inference.type_hint.clone();
                self.type_inference.type_hint = Some(expected_ty.clone());
                let lit_ty = self.visit_expr(lit_expr)?;
                self.type_inference.type_hint = saved_hint;
                let lit_ty = self.coerce_char_literal(lit_expr, expected_ty)?.unwrap_or(lit_ty);
                if !lit_ty.is_equivalent(expected_ty) {
                    if let Some(name) = self.const_pattern_origin(lit_expr) {
                        return Err(TypeCheckError::new(format!(
                            "const `{name}` has type {}, but this position holds {}",
                            self.type_name_for_error(&lit_ty),
                            self.type_name_for_error(expected_ty)
                        )));
                    }
                    return Err(TypeCheckError::new(format!(
                        "literal pattern type {} does not match expected {}",
                        self.type_name_for_error(&lit_ty),
                        self.type_name_for_error(expected_ty)
                    )));
                }
                Ok(())
            }
            Pattern::Tuple(sub_patterns) => {
                let element_types = match expected_ty {
                    TypeDecl::Tuple(ts) => ts,
                    _ => {
                        return Err(TypeCheckError::new(format!(
                            "tuple pattern requires a tuple value, got {}",
                            self.type_name_for_error(expected_ty)
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
            Pattern::Struct(struct_name, field_patterns, has_rest) => {
                self.check_struct_pattern(*struct_name, field_patterns, *has_rest, expected_ty)
            }
            // PATTERN-EXTEND: a range at a payload / field position.
            // The endpoints are checked exactly as a literal there is;
            // coverage is only tracked at the top level, where the
            // scrutinee's type is the one being exhausted.
            Pattern::Range(low, high) => {
                self.check_range_endpoints(low, high, expected_ty)?;
                Ok(())
            }
            // PATTERN-EXTEND: the name sees the whole value at this
            // position; the inner pattern is checked against the same
            // type, so `Some(n @ 3i64)` binds `n` to the payload.
            Pattern::Binding(sym, inner) => {
                self.context.set_var(*sym, expected_ty.clone());
                self.check_sub_pattern(inner, expected_ty)
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
                            "enum-variant sub-pattern expects an enum payload, got {}",
                            self.type_name_for_error(expected_ty)
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

    /// PATTERN-EXTEND: type-check a range pattern's endpoints against
    /// the value being matched and read their values back.
    ///
    /// Returns the **closed** interval the half-open range covers, so
    /// `0i64..5i64` comes back as `(0, 4)`. An empty range is refused
    /// here rather than silently becoming an arm that can never run —
    /// writing one is a `..` / inclusive-range mix-up, not an intent.
    fn check_range_endpoints(
        &mut self,
        low: &ExprRef,
        high: &ExprRef,
        expected_ty: &TypeDecl,
    ) -> Result<(i128, i128), TypeCheckError> {
        if integer_type_span(expected_ty).is_none() {
            return Err(TypeCheckError::new(format!(
                "range pattern is only valid where an integer is expected, got {}",
                self.type_name_for_error(expected_ty)
            )));
        }
        let lo = self.range_endpoint(low, expected_ty)?;
        let hi = self.range_endpoint(high, expected_ty)?;
        if hi <= lo {
            return Err(TypeCheckError::new(format!(
                "range pattern {}..{} is empty — `..` excludes its upper bound, \
                 so this arm could never run",
                lo, hi
            )));
        }
        Ok((lo, hi - 1))
    }

    /// One endpoint: checked against the scrutinee's type with it as
    /// the hint (so an unsuffixed literal picks up `i64` / `u64`), then
    /// read back out of the pool.
    fn range_endpoint(
        &mut self,
        endpoint: &ExprRef,
        expected_ty: &TypeDecl,
    ) -> Result<i128, TypeCheckError> {
        let saved_hint = self.type_inference.type_hint.clone();
        self.type_inference.type_hint = Some(expected_ty.clone());
        let ty = self.visit_expr(endpoint)?;
        self.type_inference.type_hint = saved_hint;
        let ty = self.coerce_char_literal(endpoint, expected_ty)?.unwrap_or(ty);
        if !ty.is_equivalent(expected_ty) {
            return Err(TypeCheckError::new(format!(
                "range endpoint type {} does not match {}",
                self.type_name_for_error(&ty),
                self.type_name_for_error(expected_ty)
            )));
        }
        self.core
            .expr_pool
            .get(endpoint)
            .and_then(|e| integer_literal_value(&e, self.core.string_interner))
            .ok_or_else(|| TypeCheckError::new("range endpoints must be integer literals".to_string()))
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

        // MATCH-CONST-PATTERN: a name that is a const compares against
        // it. Rewritten before anything below reads the arms, so the
        // checks see the literal pattern the backends will.
        let rewritten = self.rewrite_patterns(arms)?;
        if let Some(new_arms) = &rewritten {
            self.pattern_rewrites.rewrites.insert(*scrutinee, new_arms.clone());
        }
        let arms: &Vec<MatchArm> = rewritten.as_ref().unwrap_or(arms);

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
            // PATTERN-STRUCT: one shape, so there is nothing to
            // enumerate — the arms are told apart by their field
            // patterns, and the checking happens there.
            Struct,
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
            TypeDecl::Bool | TypeDecl::String => ScrutineeKind::Primitive(scrutinee_ty.clone()),
            // CHAR-LITERAL-MATCH: every integer width, so a byte read
            // out of a string is matched as the `u8` it is.
            t if is_matchable_integer(t) => ScrutineeKind::Primitive(scrutinee_ty.clone()),
            TypeDecl::Tuple(element_types) => ScrutineeKind::Tuple(element_types.clone()),
            TypeDecl::Struct(name, _) | TypeDecl::Identifier(name)
                if self.context.struct_definitions.contains_key(name) =>
            {
                ScrutineeKind::Struct
            }
            _ => {
                return Err(TypeCheckError::new(format!(
                    "match scrutinee must be an enum, struct, primitive (bool / an integer / str), or tuple, got {}",
                    self.type_name_for_error(&scrutinee_ty)
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
        // PATTERN-EXTEND: literals and ranges land in one interval
        // set, so `0i64..5i64` and a later `3i64` arm are compared on
        // the same terms.
        let mut covered_ints = IntCoverage::default();
        let mut covered_bool: std::collections::HashSet<bool> = std::collections::HashSet::new();
        let mut covered_strings: std::collections::HashSet<DefaultSymbol> = std::collections::HashSet::new();
        let mut has_wildcard = false;
        for (arm_index, arm) in arms.iter().enumerate() {
            let body = &arm.body;
            let is_guarded = arm.guard.is_some();
            if has_wildcard {
                return Err(TypeCheckError::new(format!(
                    "unreachable match arm at position {}: a wildcard `_` arm already covers every value",
                    arm_index
                )));
            }
            // Every arm gets a scope of its own: its bindings must not
            // leak into the next arm, and a `n @ pat` binding appears
            // before the pattern's shape is even known.
            self.context.vars.push(std::collections::HashMap::new());
            // PATTERN-EXTEND: `n @ pat` binds the whole scrutinee and
            // leaves the decision to `pat`, so peel the wrappers off
            // and let every rule below see the pattern that decides.
            let mut pat = &arm.pattern;
            while let Pattern::Binding(sym, inner) = pat {
                self.context.set_var(*sym, scrutinee_ty.clone());
                pat = inner;
            }
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
                    self.context.set_var(*sym, scrutinee_ty.clone());
                    if !is_guarded {
                        has_wildcard = true;
                    }
                }
                // PATTERN-STRUCT: a struct has one shape, so a struct
                // pattern covers every value unless one of its field
                // patterns can fail. That makes an unguarded,
                // all-irrefutable one count for exhaustiveness the
                // same way a bare name does.
                Pattern::Struct(struct_name, field_patterns, has_rest) => {
                    self.check_struct_pattern(
                        *struct_name,
                        field_patterns,
                        *has_rest,
                        &scrutinee_ty,
                    )?;
                    if !is_guarded && is_irrefutable_pattern(pat) {
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
                        ScrutineeKind::Struct => {
                            return Err(TypeCheckError::new(
                                "literal pattern cannot be used in a match on a struct — match its fields instead".to_string()
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
                    // CHAR-LITERAL-MATCH: `'h'` in a match on a `u8` is
                    // the byte, the way it is in `b == 'h'`.
                    let lit_ty = self.coerce_char_literal(literal_expr, &prim_ty)?.unwrap_or(lit_ty);
                    if !lit_ty.is_equivalent(&prim_ty) {
                        if let Some(name) = self.const_pattern_origin(literal_expr) {
                            return Err(TypeCheckError::new(format!(
                                "const `{name}` has type {}, but the match is on {}",
                                self.type_name_for_error(&lit_ty),
                                self.type_name_for_error(&prim_ty)
                            )));
                        }
                        return Err(TypeCheckError::new(format!(
                            "literal pattern type {} does not match scrutinee type {}",
                            self.type_name_for_error(&lit_ty),
                            self.type_name_for_error(&prim_ty)
                        )));
                    }
                    // Record the concrete literal value for duplicate /
                    // exhaustiveness checks. A guarded arm does not fully
                    // cover its literal (the guard might be false at run
                    // time), so we skip the bookkeeping when `is_guarded`.
                    if !is_guarded
                        && let Some(lit_expr) = self.core.expr_pool.get(literal_expr) {
                            match lit_expr {
                                ref e if let Some(v) = integer_literal_value(e, self.core.string_interner) => {
                                    if covered_ints.contains(v, v) {
                                        return Err(TypeCheckError::new(format!(
                                            "unreachable match arm: literal {} already handled by an earlier arm", v
                                        )));
                                    }
                                    covered_ints.insert(v, v);
                                }
                                Expr::True
                                    if !covered_bool.insert(true) => {
                                        return Err(TypeCheckError::new(
                                            "unreachable match arm: literal `true` already handled by an earlier arm".to_string()
                                        ));
                                    }
                                Expr::False
                                    if !covered_bool.insert(false) => {
                                        return Err(TypeCheckError::new(
                                            "unreachable match arm: literal `false` already handled by an earlier arm".to_string()
                                        ));
                                    }
                                Expr::String(sym)
                                    if !covered_strings.insert(sym) => {
                                        let s = self.core.string_interner.resolve(sym).unwrap_or("?").to_string();
                                        // DIAG-DEBUG-FMT-OK: `{:?}` here is
                                        // on a `String`, not on a symbol:
                                        // it re-quotes the text so the message
                                        // shows the arm as it was written
                                        // (`literal "hello"`).
                                        return Err(TypeCheckError::new(format!(
                                            "unreachable match arm: literal {:?} already handled by an earlier arm",
                                            s
                                        )));
                                    }
                                _ => {}
                            }
                        }
                }
                // PATTERN-EXTEND: a range covers a span of the value
                // space, which is the same bookkeeping a literal does
                // — one point versus many.
                Pattern::Range(low, high) => {
                    let ScrutineeKind::Primitive(prim_ty) = &kind else {
                        self.context.vars.pop();
                        return Err(TypeCheckError::new(
                            "range pattern is only valid in a match on an integer".to_string(),
                        ));
                    };
                    let prim_ty = prim_ty.clone();
                    let (lo, hi) = self.check_range_endpoints(low, high, &prim_ty)?;
                    if !is_guarded {
                        if covered_ints.contains(lo, hi) {
                            return Err(TypeCheckError::new(format!(
                                "unreachable match arm: {}..{} is already handled by earlier arms",
                                lo,
                                hi + 1
                            )));
                        }
                        covered_ints.insert(lo, hi);
                    }
                }
                // Peeled off above, so the loop never sees one here.
                Pattern::Binding(_, _) => unreachable!("`n @ pat` is peeled before this match"),
                Pattern::Tuple(sub_patterns) => {
                    // Tuple matches are independent of enum dispatch;
                    // require the scrutinee to be a tuple type and
                    // type-check each element through `check_sub_pattern`.
                    let element_types = match &scrutinee_ty {
                        TypeDecl::Tuple(ts) => ts.clone(),
                        _ => {
                            return Err(TypeCheckError::new(format!(
                                "tuple pattern requires a tuple scrutinee, got {}",
                                self.type_name_for_error(&scrutinee_ty)
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
                                "enum-variant pattern cannot be used in a match on {}",
                                self.type_name_for_error(t)
                            )));
                        }
                        ScrutineeKind::Struct => {
                            return Err(TypeCheckError::new(
                                "enum-variant pattern cannot be used in a match on a struct".to_string()
                            ));
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
                            .or_default()
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
                    self.context.vars.pop();
                    return Err(TypeCheckError::new(format!(
                        "match arm guard must be of type bool, got {}",
                        self.type_name_for_error(&guard_ty)
                    )));
                }
            }
            // IF-VAL: an arm body that is a literal empty block (`=> {}`)
            // is treated as Unit, mirroring `visit_if_elif_else`. This
            // keeps `if val PAT = EXPR { THEN }` (no else) — which
            // desugars to a two-arm match where the catch-all body is
            // an empty block — well-typed at statement position. The
            // body is still type-checked normally if it has any
            // statements.
            let body_ty = if matches!(self.core.expr_pool.get(body), Some(crate::ast::Expr::Block(stmts)) if stmts.is_empty())
            {
                TypeDecl::Unit
            } else {
                self.visit_expr(body)?
            };
            self.context.vars.pop();
            arm_types.push(body_ty);
        }

        // Exhaustiveness. Enums must cover every variant. `bool` must cover
        // both `true` and `false`. Other primitives have an unbounded value
        // space, so a wildcard is mandatory.
        if !has_wildcard {
            match &kind {
                // PATTERN-STRUCT: one shape means an irrefutable
                // struct pattern already covers everything, and that
                // sets `has_wildcard`. Reaching here means every arm
                // was refutable, so the match can fall through.
                ScrutineeKind::Struct => {
                    return Err(TypeCheckError::new(
                        "non-exhaustive match on a struct: every arm can fail, so add one whose field patterns always match (or a wildcard `_`)"
                            .to_string(),
                    ));
                }
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
                    // PATTERN-EXTEND: literal and range arms can add
                    // up to the whole type, in which case no wildcard
                    // is needed. In practice that takes a range, since
                    // spelling out 2^64 literals is not a thing.
                    if let Some((min, max)) = integer_type_span(t)
                        && covered_ints.contains(min, max)
                    {
                        // Covered — fall through to the arm-type check.
                    } else {
                        let t_name = self.type_name_for_error(t);
                        return Err(TypeCheckError::new(format!(
                            "non-exhaustive match on {}: the arms leave values uncovered, \
                             add a wildcard `_` arm (or ranges that span the type)",
                            t_name
                        )));
                    }
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
        if !has_wildcard
            && let ScrutineeKind::Enum { name: enum_name, type_args, variants } = &kind {
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

        // All arms must share a common type.
        let first = arm_types[0].clone();
        for (i, t) in arm_types.iter().enumerate().skip(1) {
            if !first.is_equivalent(t) {
                return Err(TypeCheckError::new(format!(
                    "match arms have incompatible types: arm 0 is {}, arm {} is {}",
                    self.type_name_for_error(&first),
                    i,
                    self.type_name_for_error(t)
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
                    "non-exhaustive match {}: position type {} is not fully covered — add a wildcard `_` or a bare name",
                    context, self.type_name_for_error(position_type)
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
            if let Pattern::EnumVariant(p_enum, p_variant, bindings) = peel_bindings(pat) {
                if *p_enum != enum_name {
                    continue;
                }
                covered_variants.insert(*p_variant);
                if bindings.iter().all(is_irrefutable_pattern) {
                    fully_covered.insert(*p_variant);
                } else {
                    variant_arms
                        .entry(*p_variant)
                        .or_default()
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

/// MATCH-CONST-PATTERN: a const's initialiser as a literal typed at the
/// const's declared type, or `None` when it is not a literal.
///
/// A literal that already names its type (`3u64`, `true`, `"GET"`) is
/// copied as is. The two that take their type from context -- a
/// suffix-less number and a char literal -- are pinned to the declared
/// type here, so the copy a pattern gets cannot be retyped by the
/// scrutinee: `const K: u64 = 3` stays a `u64` in a match on an `i64`,
/// and the mismatch is reported instead of silently compared.
fn typed_const_literal(
    expr: &Expr,
    ty: &TypeDecl,
    interner: &string_interner::DefaultStringInterner,
) -> Option<Expr> {
    let value: i128 = match expr {
        Expr::True
        | Expr::False
        | Expr::String(_)
        | Expr::Int64(_)
        | Expr::UInt64(_)
        | Expr::Int8(_)
        | Expr::Int16(_)
        | Expr::Int32(_)
        | Expr::UInt8(_)
        | Expr::UInt16(_)
        | Expr::UInt32(_) => return Some(expr.clone()),
        Expr::CharLiteral(cp) => i128::from(*cp),
        Expr::Number(sym) => {
            let text = interner.resolve(*sym)?.replace('_', "");
            let (negative, digits) = match text.strip_prefix('-') {
                Some(rest) => (true, rest.to_string()),
                None => (false, text),
            };
            let magnitude = match digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
                Some(hex) => i128::from_str_radix(hex, 16).ok()?,
                None => digits.parse::<i128>().ok()?,
            };
            if negative { -magnitude } else { magnitude }
        }
        _ => return None,
    };
    integer_literal_of(value, ty)
}

/// An integer literal node holding `value` at type `ty`, or `None` when
/// `ty` is not an integer type or the value does not fit it.
pub(super) fn integer_literal_of(value: i128, ty: &TypeDecl) -> Option<Expr> {
    Some(match ty {
        TypeDecl::Int64 => Expr::Int64(i64::try_from(value).ok()?),
        TypeDecl::UInt64 => Expr::UInt64(u64::try_from(value).ok()?),
        TypeDecl::Int8 => Expr::Int8(i8::try_from(value).ok()?),
        TypeDecl::Int16 => Expr::Int16(i16::try_from(value).ok()?),
        TypeDecl::Int32 => Expr::Int32(i32::try_from(value).ok()?),
        TypeDecl::UInt8 => Expr::UInt8(u8::try_from(value).ok()?),
        TypeDecl::UInt16 => Expr::UInt16(u16::try_from(value).ok()?),
        TypeDecl::UInt32 => Expr::UInt32(u32::try_from(value).ok()?),
        _ => return None,
    })
}

impl<'a> TypeCheckerVisitor<'a> {
    /// MATCH-CONST-PATTERN: record a top-level `const` so a pattern
    /// naming it compares against its value. Called once per const,
    /// in declaration order, after its initialiser type-checked.
    ///
    /// An initialiser that names an earlier const takes that const's
    /// literal, so `const B: u64 = A` works as a pattern when `A` does.
    pub fn register_const_for_patterns(&mut self, name: DefaultSymbol, ty: &TypeDecl, value: &ExprRef) {
        let literal = match self.core.expr_pool.get(value) {
            Some(Expr::Identifier(sym)) => self.pattern_rewrites.const_values.get(&sym).cloned().flatten(),
            Some(expr) => typed_const_literal(&expr, ty, self.core.string_interner),
            None => None,
        };
        self.pattern_rewrites.const_values.insert(name, literal);
    }

    /// `arms` with the pattern sugar the type checker resolves replaced
    /// by the patterns it stands for, or `None` when an arm has none:
    ///
    /// - MATCH-CONST-PATTERN: a const name becomes a literal pattern.
    ///   Only bare names are candidates -- `n @ pat` is always a
    ///   binding (it says so), and a name that is not a const binds as
    ///   before.
    /// - ENUM-STRUCT-VARIANT: `E::A { x, .. }` (a struct pattern under
    ///   the joined name) becomes the positional `E::A(x, _)`.
    ///
    /// Runs before the arms are checked, so everything downstream --
    /// type agreement, duplicate arms, exhaustiveness -- sees the
    /// rewritten form, which is also what the backends receive.
    pub(super) fn rewrite_patterns(
        &mut self,
        arms: &[MatchArm],
    ) -> Result<Option<Vec<MatchArm>>, TypeCheckError> {
        let mut changed = false;
        let mut out = Vec::with_capacity(arms.len());
        for arm in arms {
            let pattern = self.rewrite_pattern(&arm.pattern, &mut changed)?;
            out.push(MatchArm { pattern, guard: arm.guard, body: arm.body });
        }
        Ok(changed.then_some(out))
    }

    fn rewrite_sub_patterns(
        &mut self,
        pats: &[Pattern],
        changed: &mut bool,
    ) -> Result<Vec<Pattern>, TypeCheckError> {
        pats.iter().map(|p| self.rewrite_pattern(p, changed)).collect()
    }

    fn rewrite_pattern(&mut self, pat: &Pattern, changed: &mut bool) -> Result<Pattern, TypeCheckError> {
        Ok(match pat {
            Pattern::Name(sym) => match self.pattern_rewrites.const_values.get(sym).cloned() {
                None => pat.clone(),
                Some(Some(literal)) => {
                    let literal_ref = self.core.expr_pool.add(literal);
                    self.pattern_rewrites.origins.insert(literal_ref, *sym);
                    *changed = true;
                    Pattern::Literal(literal_ref)
                }
                Some(None) => {
                    let name = self.core.string_interner.resolve(*sym).unwrap_or("?");
                    return Err(TypeCheckError::new(format!(
                        "`{name}` is a const, so this pattern would compare against it — but its \
                         value is not a literal, and a pattern needs one while compiling. Write the \
                         literal, or bind and compare in a guard: `v if v == {name} =>`"
                    )));
                }
            },
            Pattern::EnumVariant(enum_name, variant, subs) => {
                Pattern::EnumVariant(*enum_name, *variant, self.rewrite_sub_patterns(subs, changed)?)
            }
            Pattern::Tuple(subs) => Pattern::Tuple(self.rewrite_sub_patterns(subs, changed)?),
            Pattern::Struct(struct_name, fields, has_rest) => {
                let mut out = Vec::with_capacity(fields.len());
                for (field, sub) in fields {
                    out.push((*field, self.rewrite_pattern(sub, changed)?));
                }
                // ENUM-STRUCT-VARIANT: `E::A { x, .. }`, parsed under the
                // joined name, is the positional `E::A(x, _)`.
                if let Some((enum_name, variant)) = self.split_enum_variant_path(*struct_name) {
                    *changed = true;
                    self.struct_variant_pattern(enum_name, variant, &out, *has_rest)?
                } else {
                    Pattern::Struct(*struct_name, out, *has_rest)
                }
            }
            Pattern::Binding(name, inner) => {
                Pattern::Binding(*name, Box::new(self.rewrite_pattern(inner, changed)?))
            }
            Pattern::Literal(_) | Pattern::Range(_, _) | Pattern::Wildcard => pat.clone(),
        })
    }

    /// MATCH-CONST-PATTERN: the const a literal pattern stands for, for
    /// a diagnostic that should name `K` rather than its value.
    pub(super) fn const_pattern_origin(&self, literal_ref: &ExprRef) -> Option<String> {
        let sym = self.pattern_rewrites.origins.get(literal_ref)?;
        self.core.string_interner.resolve(*sym).map(str::to_string)
    }

    /// Install the rewritten arms, so every backend sees the literal and
    /// positional patterns the sugar stands for.
    ///
    /// One pass over the pool, and only when some match had sugar.
    pub fn apply_pattern_rewrites(&mut self) {
        if self.pattern_rewrites.rewrites.is_empty() {
            return;
        }
        let rewrites = std::mem::take(&mut self.pattern_rewrites.rewrites);
        for index in 0..self.core.expr_pool.len() {
            let expr_ref = ExprRef(index as u32);
            if let Some(Expr::Match(scrutinee, _)) = self.core.expr_pool.get(&expr_ref)
                && let Some(arms) = rewrites.get(&scrutinee)
            {
                self.core.expr_pool.update(&expr_ref, Expr::Match(scrutinee, arms.clone()));
            }
        }
    }
}
