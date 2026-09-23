//! ENUM-STRUCT-VARIANT: `enum E { A { x: u64, y: u64 }, B }`.
//!
//! A struct variant is a tuple variant whose payload positions have
//! names. The parser records the names on the declaration
//! (`EnumVariantDef::field_names`) and hands the two use sites over in
//! the struct forms, under the joined name `E::A`
//! (`Parser::enum_variant_path_symbol`):
//!
//! - the literal `E::A { y: 2u64, x: 1u64 }` as an `Expr::StructLiteral`,
//!   which this file types as the construction `E::A(1u64, 2u64)` and
//!   rewrites into it after checking (`apply_enum_struct_literal_rewrites`,
//!   keyed by the first initializer -- the literal's own `ExprRef` is not
//!   in hand, the NEWTYPE situation);
//! - the pattern `E::A { x, .. }` as a `Pattern::Struct`, which the
//!   pattern rewrite in `pattern_match.rs` turns into `E::A(x, _)` before
//!   the arm is checked.
//!
//! Layout, exhaustiveness and every backend are therefore the tuple
//! variant's, and nothing past the type checker sees a field name.
//! One consequence worth knowing: the literal's fields are evaluated in
//! **declaration** order, since that is the order of the call it
//! becomes.

use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckError, TypeCheckerVisitor};

impl<'a> TypeCheckerVisitor<'a> {
    /// `E::A` as the parser joined it, split back into the enum and the
    /// variant. `None` for any other name -- including a joined one whose
    /// halves were never interned, which no declaration can match.
    pub(super) fn split_enum_variant_path(&self, joined: DefaultSymbol) -> Option<(DefaultSymbol, DefaultSymbol)> {
        let text = self.core.string_interner.resolve(joined)?;
        let (e, v) = text.split_once("::")?;
        Some((self.core.string_interner.get(e)?, self.core.string_interner.get(v)?))
    }

    /// The field names of the struct variant `E::A`, or why `E::A` is
    /// not one.
    fn struct_variant_fields(
        &self,
        enum_name: DefaultSymbol,
        variant: DefaultSymbol,
    ) -> Result<Vec<DefaultSymbol>, TypeCheckError> {
        let resolve = |s: DefaultSymbol| self.core.string_interner.resolve(s).unwrap_or("?").to_string();
        let path = format!("{}::{}", resolve(enum_name), resolve(variant));
        let Some(variants) = self.context.enum_definitions.get(&enum_name) else {
            return Err(TypeCheckError::new(format!(
                "`{path} {{ .. }}`: there is no enum named `{}`",
                resolve(enum_name)
            )));
        };
        let Some(def) = variants.iter().find(|v| v.name == variant) else {
            return Err(TypeCheckError::new(format!(
                "`{path} {{ .. }}`: enum `{}` has no variant `{}`",
                resolve(enum_name),
                resolve(variant)
            )));
        };
        if def.field_names.is_empty() {
            let spelling = if def.payload_types.is_empty() {
                format!("it carries no data: write `{path}`")
            } else {
                format!("its fields are positional: write `{path}(..)`")
            };
            return Err(TypeCheckError::new(format!("`{path}` is not a struct variant — {spelling}")));
        }
        Ok(def.field_names.clone())
    }

    /// Named entries in declaration order: slot `i` holds what was given
    /// for field `i`. An unknown or repeated name is an error, and so is
    /// a missing one unless `allow_missing` (a pattern ending in `..`).
    fn order_by_field<T: Clone>(
        &self,
        enum_name: DefaultSymbol,
        variant: DefaultSymbol,
        field_names: &[DefaultSymbol],
        given: &[(DefaultSymbol, T)],
        allow_missing: bool,
    ) -> Result<Vec<Option<T>>, TypeCheckError> {
        let resolve = |s: DefaultSymbol| self.core.string_interner.resolve(s).unwrap_or("?").to_string();
        let path = format!("{}::{}", resolve(enum_name), resolve(variant));
        let mut slots: Vec<Option<T>> = vec![None; field_names.len()];
        for (field, value) in given {
            let Some(i) = field_names.iter().position(|f| f == field) else {
                return Err(TypeCheckError::new(format!(
                    "`{path}` has no field `{}`",
                    resolve(*field)
                )));
            };
            if slots[i].is_some() {
                return Err(TypeCheckError::new(format!(
                    "field `{}` of `{path}` is given twice",
                    resolve(*field)
                )));
            }
            slots[i] = Some(value.clone());
        }
        if !allow_missing {
            let missing: Vec<String> = field_names
                .iter()
                .zip(&slots)
                .filter(|(_, s)| s.is_none())
                .map(|(f, _)| format!("`{}`", resolve(*f)))
                .collect();
            if !missing.is_empty() {
                return Err(TypeCheckError::new(format!(
                    "`{path}` is missing {}: name every field, or end a pattern with `..`",
                    missing.join(", ")
                )));
            }
        }
        Ok(slots)
    }

    /// `E::A { x: .., y: .. }` as an expression: typed as the
    /// construction `E::A(x, y)` it becomes.
    pub(super) fn visit_enum_struct_literal(
        &mut self,
        enum_name: DefaultSymbol,
        variant: DefaultSymbol,
        fields: &[(DefaultSymbol, ExprRef)],
    ) -> Result<TypeDecl, TypeCheckError> {
        let names = self.struct_variant_fields(enum_name, variant)?;
        let args: Vec<ExprRef> = self
            .order_by_field(enum_name, variant, &names, fields, false)?
            .into_iter()
            .flatten()
            .collect();
        let ty = self.visit_associated_function_call_impl(enum_name, variant, &args)?;
        if let Some((_, first)) = fields.first() {
            self.enum_struct_literals.insert(*first, (enum_name, variant, args));
        }
        Ok(ty)
    }

    /// `E::A { x, y: 0u64, .. }` as a pattern: the positional
    /// `E::A(x, 0u64, _)` it stands for. The field patterns have
    /// already been through the rewrite themselves.
    pub(super) fn struct_variant_pattern(
        &self,
        enum_name: DefaultSymbol,
        variant: DefaultSymbol,
        fields: &[(DefaultSymbol, Pattern)],
        has_rest: bool,
    ) -> Result<Pattern, TypeCheckError> {
        let names = self.struct_variant_fields(enum_name, variant)?;
        let subs = self
            .order_by_field(enum_name, variant, &names, fields, has_rest)?
            .into_iter()
            .map(|p| p.unwrap_or(Pattern::Wildcard))
            .collect();
        Ok(Pattern::EnumVariant(enum_name, variant, subs))
    }

    /// ENUM-STRUCT-VARIANT: install the constructions the literals were
    /// checked as.
    pub fn apply_enum_struct_literal_rewrites(&mut self) {
        if self.enum_struct_literals.is_empty() {
            return;
        }
        let literals = std::mem::take(&mut self.enum_struct_literals);
        for index in 0..self.core.expr_pool.len() {
            let expr_ref = ExprRef(index as u32);
            let Some(Expr::StructLiteral(_, inits)) = self.core.expr_pool.get(&expr_ref) else {
                continue;
            };
            let Some((_, first)) = inits.first() else { continue };
            if let Some((enum_name, variant, args)) = literals.get(first) {
                self.core.expr_pool.update(
                    &expr_ref,
                    Expr::AssociatedFunctionCall(*enum_name, *variant, args.clone()),
                );
            }
        }
    }
}
