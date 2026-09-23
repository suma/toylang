//! ENUM-DISCRIMINANT: an enum's numbers, and `as` from an enum to an
//! integer.
//!
//! `enum Kind { Plain, Syslog = 1, Apache = 3 }` gives each unit variant
//! a number -- written after `=`, or the previous one's plus one (the
//! first is 0). The number is **not the tag**: layout and `match`
//! dispatch keep the variant index, so nothing below the type checker
//! learns that discriminants exist. It is only what `e as u32` yields.
//!
//! The checker rewrites `e as T` into the exhaustive match it means,
//! `match e { Kind::Plain => 0u32, Kind::Syslog => 1u32, ... }`, and a
//! bare path (`Kind::Syslog as u32`) straight into its literal, so the
//! backends only ever see forms they already lower. The rewrite is a
//! post-pass for the reason `??`'s is: a cast reaches the checker
//! through routes that do not all carry the node's own `ExprRef`. It is
//! keyed by the operand, the one ref every route holds.

use std::collections::HashMap;
use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::pattern_match::integer_literal_of;
use crate::type_checker::{TypeCheckError, TypeCheckerVisitor};

/// Every variant's number, in declaration order: the one written, or
/// the previous variant's plus one, the first 0.
pub(super) fn discriminant_values(variants: &[EnumVariantDef]) -> Vec<i128> {
    let mut out = Vec::with_capacity(variants.len());
    let mut next: i128 = 0;
    for v in variants {
        let value = v.discriminant.unwrap_or(next);
        out.push(value);
        next = value + 1;
    }
    out
}

impl<'a> TypeCheckerVisitor<'a> {
    /// Reject two variants that stand for the same number. A payload
    /// variant with a discriminant is rejected by the parser.
    pub(super) fn check_enum_discriminants(
        &self,
        enum_name: DefaultSymbol,
        variants: &[EnumVariantDef],
    ) -> Result<(), TypeCheckError> {
        if variants.iter().all(|v| v.discriminant.is_none()) {
            return Ok(());
        }
        let values = discriminant_values(variants);
        let mut seen: HashMap<i128, DefaultSymbol> = HashMap::new();
        for (v, value) in variants.iter().zip(values) {
            if let Some(first) = seen.insert(value, v.name) {
                let resolve = |s: DefaultSymbol| self.core.string_interner.resolve(s).unwrap_or("?");
                return Err(TypeCheckError::new(format!(
                    "`{}::{}` and `{}::{}` both stand for {value}: a discriminant names one variant",
                    resolve(enum_name),
                    resolve(first),
                    resolve(enum_name),
                    resolve(v.name),
                )));
            }
        }
        Ok(())
    }

    /// The enum `ty` names, if it names one.
    pub(super) fn enum_named_by(&self, ty: &TypeDecl) -> Option<DefaultSymbol> {
        match ty {
            TypeDecl::Enum(name, _) => Some(*name),
            TypeDecl::Identifier(name) | TypeDecl::Struct(name, _)
                if self.context.enum_definitions.contains_key(name) =>
            {
                Some(*name)
            }
            _ => None,
        }
    }

    /// `operand as target` where the operand is the enum `enum_name`.
    ///
    /// Legal when every variant is a unit variant -- a payload has no
    /// number to stand for -- and every variant's number fits `target`.
    /// Checking every variant rather than the ones a program happens to
    /// hold is what makes the rewrite total: the match it becomes has
    /// an arm for each. Records the rewrite and answers the target type.
    pub(super) fn check_enum_cast(
        &mut self,
        operand: &ExprRef,
        enum_name: DefaultSymbol,
        target: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        let resolve = |this: &Self, s: DefaultSymbol| {
            this.core.string_interner.resolve(s).unwrap_or("?").to_string()
        };
        let variants = self
            .context
            .enum_definitions
            .get(&enum_name)
            .cloned()
            .unwrap_or_default();
        if integer_literal_of(0, target).is_none() {
            return Err(TypeCheckError::new(format!(
                "Cannot cast enum `{}` to {}: `as` turns an enum into an integer type only",
                resolve(self, enum_name),
                self.type_name_for_error(target)
            )));
        }
        if let Some(v) = variants.iter().find(|v| !v.payload_types.is_empty()) {
            return Err(TypeCheckError::new(format!(
                "Cannot cast enum `{}` to {}: `{}::{}` carries data, so the enum has no \
                 number per variant — `as` needs every variant to be a unit variant. \
                 Match on the value instead",
                resolve(self, enum_name),
                self.type_name_for_error(target),
                resolve(self, enum_name),
                resolve(self, v.name),
            )));
        }
        for (v, value) in variants.iter().zip(discriminant_values(&variants)) {
            if integer_literal_of(value, target).is_none() {
                return Err(TypeCheckError::new(format!(
                    "Cannot cast enum `{}` to {}: `{}::{}` stands for {value}, which does not fit",
                    resolve(self, enum_name),
                    self.type_name_for_error(target),
                    resolve(self, enum_name),
                    resolve(self, v.name),
                )));
            }
        }
        self.enum_casts.insert(*operand, (enum_name, target.clone()));
        Ok(target.clone())
    }

    /// ENUM-DISCRIMINANT: replace every recorded `e as T` with the match
    /// (or, for a bare path, the literal) it stands for.
    pub fn apply_enum_cast_rewrites(&mut self) {
        if self.enum_casts.is_empty() {
            return;
        }
        let casts = std::mem::take(&mut self.enum_casts);
        for index in 0..self.core.expr_pool.len() {
            let expr_ref = ExprRef(index as u32);
            let Some(Expr::Cast(operand, _)) = self.core.expr_pool.get(&expr_ref) else {
                continue;
            };
            let Some((enum_name, target)) = casts.get(&operand) else {
                continue;
            };
            let variants = self
                .context
                .enum_definitions
                .get(enum_name)
                .cloned()
                .unwrap_or_default();
            let values = discriminant_values(&variants);
            // `Kind::Syslog as u32`: the variant is known here, so the
            // answer is too. The compiled lanes also refuse a bare
            // path as a `match` scrutinee, so folding is not optional.
            if let Some(Expr::QualifiedIdentifier(path)) = self.core.expr_pool.get(&operand)
                && let [_, variant] = path.as_slice()
                && let Some(i) = variants.iter().position(|v| v.name == *variant)
                && let Some(literal) = integer_literal_of(values[i], target)
            {
                self.core.expr_pool.update(&expr_ref, literal);
                continue;
            }
            let mut arms = Vec::with_capacity(variants.len());
            for (v, value) in variants.iter().zip(values) {
                // Checked when the cast was recorded, so every value fits.
                let Some(literal) = integer_literal_of(value, target) else { continue };
                let body = self.core.expr_pool.add(literal);
                arms.push(MatchArm {
                    pattern: Pattern::EnumVariant(*enum_name, v.name, Vec::new()),
                    guard: None,
                    body,
                });
            }
            self.core.expr_pool.update(&expr_ref, Expr::Match(operand, arms));
        }
    }
}
