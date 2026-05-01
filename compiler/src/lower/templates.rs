//! Per-program struct and enum templates plus their instantiation
//! and substitution machinery.
//!
//! A "template" here is the AST-level shape of a `struct` or `enum`
//! declaration, with generic params still abstract (`TypeDecl::Generic(T)`
//! placeholders inside field / payload types). At each use site the
//! lowering pass materialises a concrete IR `StructDef` / `EnumDef`
//! by substituting in the actual type arguments and interning the
//! result via `Module::intern_struct` / `Module::intern_enum`. The
//! `(base_name, type_args) -> Id` cache lives on `Module` itself so
//! repeated instantiations dedup automatically.
//!
//! Struct- and enum-side helpers are intertwined (a struct field can
//! be an enum and vice versa, so `substitute_field_type` calls
//! `instantiate_enum` and `substitute_payload_type` calls
//! `instantiate_struct`) — keeping them in one module avoids a
//! gnarly visibility dance.

use std::collections::HashMap;

use frontend::ast::{Program, Stmt, StmtRef};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use super::types::{intern_tuple, lower_scalar};
use crate::ir::{EnumId, EnumVariant, Module, StructId, Type};

/// `struct Name { f1: T1, f2: T2, ... }` declarations, indexed by
/// symbol. Field names stay as `String` because the AST stores them
/// that way; the lowering pass compares them against the
/// `DefaultSymbol`-resolved name at field-access sites.
pub(super) type StructDefs = HashMap<DefaultSymbol, StructTemplate>;

/// Per-program struct templates, indexed by base name. Each template
/// keeps the AST `TypeDecl` field shapes verbatim so generic params
/// can be substituted at instantiation time. Non-generic structs sit
/// in the same table with empty `generic_params`.
#[derive(Debug, Clone)]
pub(super) struct StructTemplate {
    pub(super) generic_params: Vec<DefaultSymbol>,
    pub(super) fields: Vec<(String, TypeDecl)>,
}

/// Per-program enum templates. Same shape as `StructDefs` but for
/// `enum` declarations. Generic / non-generic enums share the table.
pub(super) type EnumDefs = HashMap<DefaultSymbol, EnumTemplate>;

#[derive(Debug, Clone)]
pub(super) struct EnumTemplate {
    pub(super) generic_params: Vec<DefaultSymbol>,
    pub(super) variants: Vec<EnumTemplateVariant>,
}

#[derive(Debug, Clone)]
pub(super) struct EnumTemplateVariant {
    pub(super) name: DefaultSymbol,
    pub(super) payload_types: Vec<TypeDecl>,
}

pub(super) fn collect_struct_defs(
    program: &Program,
    interner: &DefaultStringInterner,
) -> Result<StructDefs, String> {
    let _ = interner;
    let mut defs: StructDefs = HashMap::new();
    let stmt_count = program.statement.len();
    for i in 0..stmt_count {
        let stmt_ref = StmtRef(i as u32);
        let stmt = match program.statement.get(&stmt_ref) {
            Some(s) => s,
            None => continue,
        };
        if let Stmt::StructDecl { name, generic_params, fields, .. } = stmt {
            let template_fields: Vec<(String, TypeDecl)> = fields
                .iter()
                .map(|f| (f.name.clone(), f.type_decl.clone()))
                .collect();
            defs.insert(
                name,
                StructTemplate {
                    generic_params: generic_params.clone(),
                    fields: template_fields,
                },
            );
        }
    }
    Ok(defs)
}

pub(super) fn collect_enum_defs(
    program: &Program,
    interner: &DefaultStringInterner,
) -> Result<EnumDefs, String> {
    let _ = interner;
    let mut defs: EnumDefs = HashMap::new();
    let stmt_count = program.statement.len();
    for i in 0..stmt_count {
        let stmt_ref = StmtRef(i as u32);
        let stmt = match program.statement.get(&stmt_ref) {
            Some(s) => s,
            None => continue,
        };
        if let Stmt::EnumDecl { name, generic_params, variants, .. } = stmt {
            let template_variants: Vec<EnumTemplateVariant> = variants
                .iter()
                .map(|v| EnumTemplateVariant {
                    name: v.name,
                    payload_types: v.payload_types.clone(),
                })
                .collect();
            defs.insert(
                name,
                EnumTemplate {
                    generic_params: generic_params.clone(),
                    variants: template_variants,
                },
            );
        }
    }
    Ok(defs)
}

/// Substitute the template's generic parameters with `type_args` and
/// intern (or re-use) the resulting concrete enum in the IR module.
/// Non-generic enums short-circuit to a single instance shared
/// across the whole program; generic enums get one instance per
/// distinct concrete arg tuple. Returns the canonical `EnumId`.
pub(super) fn instantiate_enum(
    module: &mut Module,
    templates: &EnumDefs,
    struct_templates: &StructDefs,
    base_name: DefaultSymbol,
    type_args: Vec<Type>,
    interner: &DefaultStringInterner,
) -> Result<EnumId, String> {
    let template = templates.get(&base_name).ok_or_else(|| {
        format!(
            "internal error: no enum template for `{}`",
            interner.resolve(base_name).unwrap_or("?")
        )
    })?;
    if template.generic_params.len() != type_args.len() {
        return Err(format!(
            "enum `{}` expects {} type argument(s), got {}",
            interner.resolve(base_name).unwrap_or("?"),
            template.generic_params.len(),
            type_args.len(),
        ));
    }
    if let Some(id) = module.enum_index.get(&(base_name, type_args.clone())).copied() {
        return Ok(id);
    }
    let template = template.clone();
    let subst: HashMap<DefaultSymbol, Type> = template
        .generic_params
        .iter()
        .copied()
        .zip(type_args.iter().copied())
        .collect();
    let mut ir_variants: Vec<EnumVariant> = Vec::with_capacity(template.variants.len());
    for v in &template.variants {
        let mut payload_types: Vec<Type> = Vec::with_capacity(v.payload_types.len());
        for pt in &v.payload_types {
            let lowered = substitute_payload_type(
                pt,
                &subst,
                module,
                templates,
                struct_templates,
                interner,
            )
            .ok_or_else(|| {
                format!(
                    "enum `{}::{}` has unsupported payload type `{:?}` \
                     (compiler MVP accepts i64 / u64 / f64 / bool, or another \
                     enum substituted from a generic parameter)",
                    interner.resolve(base_name).unwrap_or("?"),
                    interner.resolve(v.name).unwrap_or("?"),
                    pt,
                )
            })?;
            if !is_supported_enum_payload(lowered) {
                return Err(format!(
                    "enum `{}::{}` has unsupported payload type `{lowered}` \
                     (compiler MVP only accepts i64 / u64 / f64 / bool / nested enum)",
                    interner.resolve(base_name).unwrap_or("?"),
                    interner.resolve(v.name).unwrap_or("?"),
                ));
            }
            payload_types.push(lowered);
        }
        ir_variants.push(EnumVariant { name: v.name, payload_types });
    }
    Ok(module.intern_enum(base_name, type_args, ir_variants))
}

pub(super) fn is_supported_enum_payload(t: Type) -> bool {
    matches!(
        t,
        Type::I64
            | Type::U64
            | Type::F64
            | Type::Bool
            | Type::Enum(_)
            | Type::Struct(_)
            | Type::Tuple(_)
    )
}

/// Lower an enum payload `TypeDecl`, applying any active generic
/// substitution. Recursively instantiates nested generic enums so
/// `Option<Option<i64>>` resolves all the way down.
pub(super) fn substitute_payload_type(
    pt: &TypeDecl,
    subst: &HashMap<DefaultSymbol, Type>,
    module: &mut Module,
    enum_templates: &EnumDefs,
    struct_templates: &StructDefs,
    interner: &DefaultStringInterner,
) -> Option<Type> {
    if let Some(t) = lower_scalar(pt) {
        return Some(t);
    }
    match pt {
        TypeDecl::Generic(name) => subst.get(name).copied(),
        TypeDecl::Identifier(name) => {
            if let Some(t) = subst.get(name).copied() {
                return Some(t);
            }
            if enum_templates.contains_key(name) {
                instantiate_enum(
                    module,
                    enum_templates,
                    struct_templates,
                    *name,
                    Vec::new(),
                    interner,
                )
                .ok()
                .map(Type::Enum)
            } else if struct_templates.contains_key(name) {
                instantiate_struct(
                    module,
                    struct_templates,
                    enum_templates,
                    *name,
                    Vec::new(),
                    interner,
                )
                .ok()
                .map(Type::Struct)
            } else {
                None
            }
        }
        TypeDecl::Enum(name, args) | TypeDecl::Struct(name, args)
            if !args.is_empty() && enum_templates.contains_key(name) =>
        {
            let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                let t = substitute_payload_type(
                    a,
                    subst,
                    module,
                    enum_templates,
                    struct_templates,
                    interner,
                )?;
                concrete.push(t);
            }
            instantiate_enum(
                module,
                enum_templates,
                struct_templates,
                *name,
                concrete,
                interner,
            )
            .ok()
            .map(Type::Enum)
        }
        TypeDecl::Struct(name, args)
            if !args.is_empty() && struct_templates.contains_key(name) =>
        {
            let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                let t = substitute_payload_type(
                    a,
                    subst,
                    module,
                    enum_templates,
                    struct_templates,
                    interner,
                )?;
                concrete.push(t);
            }
            instantiate_struct(
                module,
                struct_templates,
                enum_templates,
                *name,
                concrete,
                interner,
            )
            .ok()
            .map(Type::Struct)
        }
        TypeDecl::Enum(name, args) | TypeDecl::Struct(name, args)
            if args.is_empty() && enum_templates.contains_key(name) =>
        {
            instantiate_enum(
                module,
                enum_templates,
                struct_templates,
                *name,
                Vec::new(),
                interner,
            )
            .ok()
            .map(Type::Enum)
        }
        TypeDecl::Tuple(elements) => {
            let mut lowered: Vec<Type> = Vec::with_capacity(elements.len());
            for e in elements {
                let t = substitute_payload_type(
                    e,
                    subst,
                    module,
                    enum_templates,
                    struct_templates,
                    interner,
                )?;
                if !matches!(t, Type::I64 | Type::U64 | Type::F64 | Type::Bool) {
                    return None;
                }
                lowered.push(t);
            }
            let id = intern_tuple(module, lowered);
            Some(Type::Tuple(id))
        }
        _ => None,
    }
}

/// Substitute the template's generic parameters with `type_args` and
/// intern the resulting concrete struct in the IR module. Same shape
/// as `instantiate_enum`.
pub(super) fn instantiate_struct(
    module: &mut Module,
    templates: &StructDefs,
    enum_templates: &EnumDefs,
    base_name: DefaultSymbol,
    type_args: Vec<Type>,
    interner: &DefaultStringInterner,
) -> Result<StructId, String> {
    let template = templates.get(&base_name).ok_or_else(|| {
        format!(
            "internal error: no struct template for `{}`",
            interner.resolve(base_name).unwrap_or("?")
        )
    })?;
    if template.generic_params.len() != type_args.len() {
        return Err(format!(
            "struct `{}` expects {} type argument(s), got {}",
            interner.resolve(base_name).unwrap_or("?"),
            template.generic_params.len(),
            type_args.len(),
        ));
    }
    if let Some(id) = module
        .struct_index
        .get(&(base_name, type_args.clone()))
        .copied()
    {
        return Ok(id);
    }
    let template = template.clone();
    let subst: HashMap<DefaultSymbol, Type> = template
        .generic_params
        .iter()
        .copied()
        .zip(type_args.iter().copied())
        .collect();
    let mut concrete_fields: Vec<(String, Type)> = Vec::with_capacity(template.fields.len());
    for (fname, ftype) in &template.fields {
        let lowered =
            substitute_field_type(ftype, &subst, module, templates, enum_templates, interner)
                .ok_or_else(|| {
                    format!(
                        "compiler MVP cannot lower struct field `{}.{}: {:?}`",
                        interner.resolve(base_name).unwrap_or("?"),
                        fname,
                        ftype,
                    )
                })?;
        if matches!(lowered, Type::Unit) {
            return Err(format!(
                "struct field `{}.{}` cannot have type Unit",
                interner.resolve(base_name).unwrap_or("?"),
                fname
            ));
        }
        concrete_fields.push((fname.clone(), lowered));
    }
    Ok(module.intern_struct(base_name, type_args, concrete_fields))
}

/// Recursively lower a struct field's declared type, applying the
/// active generic substitution. Recurses through nested generic
/// struct types so `Cell<Cell<i64>>` resolves all the way down.
pub(super) fn substitute_field_type(
    ty: &TypeDecl,
    subst: &HashMap<DefaultSymbol, Type>,
    module: &mut Module,
    templates: &StructDefs,
    enum_templates: &EnumDefs,
    interner: &DefaultStringInterner,
) -> Option<Type> {
    if let Some(s) = lower_scalar(ty) {
        return Some(s);
    }
    match ty {
        TypeDecl::Generic(name) => subst.get(name).copied(),
        TypeDecl::Identifier(name) => {
            if let Some(t) = subst.get(name).copied() {
                return Some(t);
            }
            if templates.contains_key(name) {
                instantiate_struct(
                    module,
                    templates,
                    enum_templates,
                    *name,
                    Vec::new(),
                    interner,
                )
                .ok()
                .map(Type::Struct)
            } else if enum_templates.contains_key(name) {
                instantiate_enum(module, enum_templates, templates, *name, Vec::new(), interner)
                    .ok()
                    .map(Type::Enum)
            } else {
                None
            }
        }
        TypeDecl::Struct(name, args) if templates.contains_key(name) => {
            let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                concrete.push(substitute_field_type(
                    a,
                    subst,
                    module,
                    templates,
                    enum_templates,
                    interner,
                )?);
            }
            instantiate_struct(
                module,
                templates,
                enum_templates,
                *name,
                concrete,
                interner,
            )
            .ok()
            .map(Type::Struct)
        }
        TypeDecl::Tuple(elements) => {
            let mut lowered: Vec<Type> = Vec::with_capacity(elements.len());
            for e in elements {
                let s = lower_scalar(e)?;
                if matches!(s, Type::Unit) {
                    return None;
                }
                lowered.push(s);
            }
            let id = intern_tuple(module, lowered);
            Some(Type::Tuple(id))
        }
        _ => None,
    }
}

/// Like `lower_scalar` but additionally accepts `Type::Struct(name)`
/// and `Type::Tuple(id)` for known struct types and structural
/// tuples respectively. Used at function-signature boundaries
/// (params and return type) where these compound shapes are
/// allowed; values inside the IR's value graph stay scalar.
pub(super) fn lower_param_or_return_type(
    ty: &TypeDecl,
    struct_defs: &StructDefs,
    enum_defs: &EnumDefs,
    module: &mut Module,
    interner: &DefaultStringInterner,
) -> Option<Type> {
    if let Some(t) = lower_scalar(ty) {
        return Some(t);
    }
    match ty {
        TypeDecl::Identifier(name) if struct_defs.contains_key(name) => {
            instantiate_struct(module, struct_defs, enum_defs, *name, Vec::new(), interner)
                .ok()
                .map(Type::Struct)
        }
        TypeDecl::Struct(name, args) if args.is_empty() && struct_defs.contains_key(name) => {
            instantiate_struct(module, struct_defs, enum_defs, *name, Vec::new(), interner)
                .ok()
                .map(Type::Struct)
        }
        TypeDecl::Struct(name, args)
            if !args.is_empty() && struct_defs.contains_key(name) =>
        {
            let mut lowered_args: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                let l = lower_scalar(a)?;
                if matches!(l, Type::Unit) {
                    return None;
                }
                lowered_args.push(l);
            }
            instantiate_struct(module, struct_defs, enum_defs, *name, lowered_args, interner)
                .ok()
                .map(Type::Struct)
        }
        TypeDecl::Identifier(name) if enum_defs.contains_key(name) => {
            instantiate_enum(module, enum_defs, struct_defs, *name, Vec::new(), interner)
                .ok()
                .map(Type::Enum)
        }
        TypeDecl::Enum(name, args) if enum_defs.contains_key(name) => {
            let mut lowered_args: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                let l = lower_scalar(a)?;
                if matches!(l, Type::Unit) {
                    return None;
                }
                lowered_args.push(l);
            }
            instantiate_enum(module, enum_defs, struct_defs, *name, lowered_args, interner)
                .ok()
                .map(Type::Enum)
        }
        TypeDecl::Struct(name, args)
            if !args.is_empty() && enum_defs.contains_key(name) =>
        {
            let mut lowered_args: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                let l = lower_scalar(a)?;
                if matches!(l, Type::Unit) {
                    return None;
                }
                lowered_args.push(l);
            }
            instantiate_enum(module, enum_defs, struct_defs, *name, lowered_args, interner)
                .ok()
                .map(Type::Enum)
        }
        TypeDecl::Tuple(elements) => {
            let mut lowered: Vec<Type> = Vec::with_capacity(elements.len());
            for e in elements {
                let s = lower_scalar(e)?;
                if matches!(s, Type::Unit) {
                    return None;
                }
                lowered.push(s);
            }
            let id = intern_tuple(module, lowered);
            Some(Type::Tuple(id))
        }
        _ => None,
    }
}
