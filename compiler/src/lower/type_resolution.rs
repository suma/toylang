//! Struct / enum instance and type-argument resolution.
//!
//! Maps a generic-or-monomorphic type name plus an optional
//! val / var type annotation to the concrete `StructId` /
//! `EnumId` to use, instantiating monomorphised copies on
//! demand and recording the chosen type-arg list.
//!
//! - `resolve_struct_instance`: pick the right `StructId` for
//!   a `base_name` plus an optional annotation. Non-generic
//!   structs hit the index directly; generic structs need
//!   either explicit type args from the annotation or
//!   inference (caller's job — this returns an error if no
//!   hint is available).
//! - `extract_struct_type_args`: pull the `<T1, T2, ...>` list
//!   out of an annotation `TypeDecl` for struct instantiation.
//! - `resolve_enum_instance`: enum counterpart of
//!   `resolve_struct_instance`.
//! - `resolve_enum_instance_with_args`: enum-specific helper
//!   that takes a pre-built `Vec<Type>` and instantiates the
//!   matching `EnumId`.
//! - `extract_enum_type_args`: enum counterpart of
//!   `extract_struct_type_args`.
//! - `lower_type_arg`: lower a single `TypeDecl` to an IR
//!   `Type` for use as a type argument (recurses through
//!   generic struct / enum shapes and tuples).

use std::collections::HashMap;

use frontend::ast::ExprRef;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use super::templates::{instantiate_enum, instantiate_struct};
use super::types::{intern_tuple, lower_scalar};
use super::FunctionLower;
use crate::ir::{EnumId, StructId, Type};

impl<'a> FunctionLower<'a> {
    /// Like `super::types::lower_scalar` but consults the active
    /// monomorphisation substitution first so a `Generic(P)` /
    /// `Identifier(P)` referring to a generic param resolves to the
    /// instance's concrete type. Used by the let-binding intercepts
    /// (`__builtin_ptr_read`) and by `__builtin_sizeof` so the
    /// generic-method body of `core/std/dict.t::insert` etc. can
    /// reach a width when the annotation names `K` or `V`.
    pub(super) fn lower_scalar_with_subst(
        &self,
        ty: &TypeDecl,
    ) -> Option<Type> {
        match ty {
            TypeDecl::Generic(p) | TypeDecl::Identifier(p) => {
                if let Some(t) = self.active_subst.get(p).copied() {
                    return Some(t);
                }
                super::types::lower_scalar(ty)
            }
            _ => super::types::lower_scalar(ty),
        }
    }

    /// Pick the right `StructId` for a `base_name` + optional val/var
    /// type annotation. Same shape as `resolve_enum_instance`.
    pub(super) fn resolve_struct_instance(
        &mut self,
        base_name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
    ) -> Result<StructId, String> {
        let template = self.struct_defs.get(&base_name).ok_or_else(|| {
            format!(
                "internal error: no struct template for `{}`",
                self.interner.resolve(base_name).unwrap_or("?")
            )
        })?;
        if template.generic_params.is_empty() {
            return instantiate_struct(
                self.module,
                self.struct_defs,
                self.enum_defs,
                base_name,
                Vec::new(),
                self.interner,
            );
        }
        let type_args = self
            .extract_struct_type_args(base_name, annotation)
            .ok_or_else(|| {
                format!(
                    "compiler MVP needs an explicit type annotation to instantiate generic \
                     struct `{}` (e.g. `val x: {}<i64> = ...`)",
                    self.interner.resolve(base_name).unwrap_or("?"),
                    self.interner.resolve(base_name).unwrap_or("?"),
                )
            })?;
        instantiate_struct(
            self.module,
            self.struct_defs,
            self.enum_defs,
            base_name,
            type_args,
            self.interner,
        )
    }

    /// Pull a `Vec<Type>` of concrete type args from a val/var
    /// annotation that names this struct. Mirrors
    /// `extract_enum_type_args`.
    pub(super) fn extract_struct_type_args(
        &mut self,
        base_name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
    ) -> Option<Vec<Type>> {
        let anno = annotation?;
        let args = match anno {
            TypeDecl::Struct(name, args) if *name == base_name => args.clone(),
            TypeDecl::Identifier(name) if *name == base_name => Vec::new(),
            _ => return None,
        };
        let mut out: Vec<Type> = Vec::with_capacity(args.len());
        for a in &args {
            out.push(self.lower_type_arg(a)?);
        }
        Some(out)
    }

    pub(super) fn resolve_enum_instance(
        &mut self,
        base_name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
    ) -> Result<EnumId, String> {
        let template = self.enum_defs.get(&base_name).ok_or_else(|| {
            format!(
                "internal error: no enum template for `{}`",
                self.interner.resolve(base_name).unwrap_or("?")
            )
        })?;
        if template.generic_params.is_empty() {
            return instantiate_enum(
                self.module,
                self.enum_defs,
                self.struct_defs,
                base_name,
                Vec::new(),
                self.interner,
            );
        }
        let type_args = self
            .extract_enum_type_args(base_name, annotation)
            .ok_or_else(|| {
                format!(
                    "compiler MVP needs an explicit type annotation to instantiate generic \
                     enum `{}` (e.g. `val x: {}<i64> = ...`)",
                    self.interner.resolve(base_name).unwrap_or("?"),
                    self.interner.resolve(base_name).unwrap_or("?"),
                )
            })?;
        instantiate_enum(
            self.module,
            self.enum_defs,
            self.struct_defs,
            base_name,
            type_args,
            self.interner,
        )
    }

    /// Same idea as `resolve_enum_instance`, but a tuple-variant
    /// construction site can also infer the type arguments from its
    /// payload values when no annotation is supplied. We only
    /// substitute the *first* generic param this way (`Option<T>`-style
    /// enums are by far the common case); enums with multiple type
    /// params still need an annotation.
    pub(super) fn resolve_enum_instance_with_args(
        &mut self,
        base_name: DefaultSymbol,
        variant_name: DefaultSymbol,
        args: &[ExprRef],
        annotation: Option<&TypeDecl>,
    ) -> Result<EnumId, String> {
        let template = self
            .enum_defs
            .get(&base_name)
            .ok_or_else(|| {
                format!(
                    "internal error: no enum template for `{}`",
                    self.interner.resolve(base_name).unwrap_or("?")
                )
            })?
            .clone();
        if template.generic_params.is_empty() {
            return instantiate_enum(
                self.module,
                self.enum_defs,
                self.struct_defs,
                base_name,
                Vec::new(),
                self.interner,
            );
        }
        if let Some(args_from_anno) = self.extract_enum_type_args(base_name, annotation) {
            return instantiate_enum(
                self.module,
                self.enum_defs,
                self.struct_defs,
                base_name,
                args_from_anno,
                self.interner,
            );
        }
        // Try inferring from argument types. Look up the chosen
        // variant's template payload pattern and match generic
        // parameters against the actual arg scalar types.
        let variant = template
            .variants
            .iter()
            .find(|v| v.name == variant_name)
            .ok_or_else(|| {
                format!(
                    "unknown enum variant `{}::{}`",
                    self.interner.resolve(base_name).unwrap_or("?"),
                    self.interner.resolve(variant_name).unwrap_or("?"),
                )
            })?;
        let mut inferred: HashMap<DefaultSymbol, Type> = HashMap::new();
        for (pt, arg) in variant.payload_types.iter().zip(args.iter()) {
            let generic = match pt {
                TypeDecl::Generic(g) => Some(*g),
                TypeDecl::Identifier(g) if template.generic_params.contains(g) => Some(*g),
                _ => None,
            };
            if let Some(g) = generic {
                if let Some(ty) = self.value_scalar(arg) {
                    inferred.entry(g).or_insert(ty);
                }
            }
        }
        let type_args: Option<Vec<Type>> = template
            .generic_params
            .iter()
            .map(|p| inferred.get(p).copied())
            .collect();
        let type_args = type_args.ok_or_else(|| {
            format!(
                "cannot infer type arguments for generic enum `{}::{}`; add an explicit \
                 type annotation (e.g. `val x: {}<i64> = ...`)",
                self.interner.resolve(base_name).unwrap_or("?"),
                self.interner.resolve(variant_name).unwrap_or("?"),
                self.interner.resolve(base_name).unwrap_or("?"),
            )
        })?;
        instantiate_enum(
            self.module,
            self.enum_defs,
            self.struct_defs,
            base_name,
            type_args,
            self.interner,
        )
    }

    /// Pull a `Vec<Type>` of concrete type arguments out of a val /
    /// var annotation that names this enum. Accepts both
    /// `TypeDecl::Enum(name, args)` and the parser's
    /// `TypeDecl::Struct(name, args)` form (the parser uses Struct
    /// for any `Name<...>` annotation since it can't tell enum from
    /// struct pre-typecheck). Returns `None` if the annotation
    /// doesn't name `base_name` or carries no usable args.
    pub(super) fn extract_enum_type_args(
        &mut self,
        base_name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
    ) -> Option<Vec<Type>> {
        let anno = annotation?;
        let args = match anno {
            TypeDecl::Enum(name, args) if *name == base_name => args.clone(),
            TypeDecl::Struct(name, args) if *name == base_name => args.clone(),
            _ => return None,
        };
        let mut out: Vec<Type> = Vec::with_capacity(args.len());
        for a in &args {
            out.push(self.lower_type_arg(a)?);
        }
        Some(out)
    }

    /// Lower one type-argument-position TypeDecl to an IR Type.
    /// Accepts scalars and (recursively) other enum instantiations
    /// — that's what allows nested annotations like
    /// `Option<Option<i64>>` to thread through the whole tree.
    pub(super) fn lower_type_arg(&mut self, t: &TypeDecl) -> Option<Type> {
        if let Some(s) = lower_scalar(t) {
            return Some(s);
        }
        match t {
            TypeDecl::Enum(name, args) | TypeDecl::Struct(name, args)
                if self.enum_defs.contains_key(name) =>
            {
                let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
                for a in args {
                    concrete.push(self.lower_type_arg(a)?);
                }
                instantiate_enum(
                    self.module,
                    self.enum_defs,
                    self.struct_defs,
                    *name,
                    concrete,
                    self.interner,
                )
                .ok()
                .map(Type::Enum)
            }
            TypeDecl::Struct(name, args) if self.struct_defs.contains_key(name) => {
                let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
                for a in args {
                    concrete.push(self.lower_type_arg(a)?);
                }
                instantiate_struct(
                    self.module,
                    self.struct_defs,
                    self.enum_defs,
                    *name,
                    concrete,
                    self.interner,
                )
                .ok()
                .map(Type::Struct)
            }
            TypeDecl::Identifier(name) if self.enum_defs.contains_key(name) => {
                instantiate_enum(
                    self.module,
                    self.enum_defs,
                    self.struct_defs,
                    *name,
                    Vec::new(),
                    self.interner,
                )
                .ok()
                .map(Type::Enum)
            }
            TypeDecl::Identifier(name) if self.struct_defs.contains_key(name) => {
                instantiate_struct(
                    self.module,
                    self.struct_defs,
                    self.enum_defs,
                    *name,
                    Vec::new(),
                    self.interner,
                )
                .ok()
                .map(Type::Struct)
            }
            TypeDecl::Tuple(elements) => {
                // `Option<(i64, i64)>` arrives as
                // `Enum("Option", [Tuple([I64, I64])])`. Lower each
                // element to a scalar Type and intern the tuple
                // shape so type-arg substitution can refer back to
                // the same `Type::Tuple(id)`.
                let mut lowered: Vec<Type> = Vec::with_capacity(elements.len());
                for e in elements {
                    let t = self.lower_type_arg(e)?;
                    if !matches!(
                        t,
                        Type::I64 | Type::U64 | Type::F64 | Type::Bool
                    ) {
                        return None;
                    }
                    lowered.push(t);
                }
                let id = intern_tuple(self.module, lowered);
                Some(Type::Tuple(id))
            }
            _ => None,
        }
    }


}
