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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use frontend::ast::{File, Stmt, StmtRef};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use super::types::{intern_tuple, lower_scalar};
use crate::ir::{EnumId, EnumVariant, Module, StructId, Type};

/// Instantiations currently being lowered, as a cycle guard.
///
/// `instantiate_struct` / `instantiate_enum` memoise only once every
/// member is lowered, so a type reachable from its own members
/// re-entered itself forever: the process died with
/// `fatal runtime error: stack overflow` and no diagnostic
/// (RECURSIVE-TYPES). The frontend now rejects those declarations
/// outright — `frontend::type_checker::check_recursive_types` — and
/// this is the backstop for any path that reaches lowering anyway
/// (an embedder calling `lower_program` directly, or a future edge the
/// frontend graph does not model). It turns an abort into an ordinary
/// lowering error, which every caller already handles.
///
/// Thread-local rather than threaded through the ~15 call sites: the
/// set is scratch state for one `lower_program` call, and lowering is
/// single-threaded within a program. `Guard` removes the entry on
/// drop, so an early `?` return cannot leave a stale key behind.
type InstanceKey = (bool, DefaultSymbol, Vec<Type>);

thread_local! {
    static IN_PROGRESS: RefCell<HashSet<InstanceKey>> = RefCell::new(HashSet::new());

    /// The cycle message from the innermost refused `Guard::enter`.
    ///
    /// The member-lowering helpers (`substitute_payload_type` /
    /// `substitute_field_type`) return `Option`, so they drop the inner
    /// `Err` and their caller reports its own "unsupported member type"
    /// fallback — which names the wrong cause. The outer frame takes
    /// this instead when it has one. Cleared whenever a fresh
    /// instantiation starts, so a message can never outlive the walk
    /// that produced it.
    static PENDING_CYCLE: RefCell<Option<String>> = const { RefCell::new(None) };
}

struct Guard(InstanceKey);

impl Guard {
    /// `None` when this instantiation is already on the stack.
    fn enter(key: InstanceKey) -> Option<Self> {
        IN_PROGRESS.with(|set| {
            if !set.borrow_mut().insert(key.clone()) {
                return None;
            }
            PENDING_CYCLE.with(|c| *c.borrow_mut() = None);
            Some(Guard(key))
        })
    }
}

/// Record the cycle so the frame that swallows this `Err` can report
/// it, and hand the same message back for the immediate return.
fn note_cycle(message: String) -> String {
    PENDING_CYCLE.with(|c| *c.borrow_mut() = Some(message.clone()));
    message
}

/// The cycle message, if a nested instantiation refused since this one
/// started. Consumes it.
fn take_pending_cycle() -> Option<String> {
    PENDING_CYCLE.with(|c| c.borrow_mut().take())
}

impl Drop for Guard {
    fn drop(&mut self) {
        IN_PROGRESS.with(|set| {
            set.borrow_mut().remove(&self.0);
        });
    }
}

/// The message both instantiation paths report for a cycle. Kept in
/// one place so the two read identically.
fn recursive_type_error(
    kind: &str,
    base_name: DefaultSymbol,
    interner: &DefaultStringInterner,
) -> String {
    format!(
        "{kind} `{}` is recursive: it contains itself with no indirection, so it has no \
         finite layout. Break the cycle with a `ptr` field or an index into a `Vec`",
        interner.resolve(base_name).unwrap_or("?"),
    )
}

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
    program: &File,
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
    program: &File,
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
    let Some(_guard) = Guard::enter((true, base_name, type_args.clone())) else {
        return Err(note_cycle(recursive_type_error("enum", base_name, interner)));
    };
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
                take_pending_cycle().unwrap_or_else(|| {
                    format!(
                        "enum `{}::{}` has unsupported payload type `{:?}` \
                         (compiler MVP accepts i64 / u64 / f64 / bool, or another \
                         enum substituted from a generic parameter)",
                        interner.resolve(base_name).unwrap_or("?"),
                        interner.resolve(v.name).unwrap_or("?"),
                        pt,
                    )
                })
            })?;
            if !is_supported_enum_payload(lowered) {
                return Err(format!(
                    "enum `{}::{}` has unsupported payload type `{lowered}` \
                     (compiler MVP accepts i64 / u64 / f64 / bool / str / nested enum / struct / tuple)",
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
            | Type::Str
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
    let Some(_guard) = Guard::enter((false, base_name, type_args.clone())) else {
        return Err(note_cycle(recursive_type_error("struct", base_name, interner)));
    };
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
                    take_pending_cycle().unwrap_or_else(|| {
                        format!(
                            "compiler MVP cannot lower struct field `{}.{}: {:?}`",
                            interner.resolve(base_name).unwrap_or("?"),
                            fname,
                            ftype,
                        )
                    })
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
    // REF-Stage-2 (b)+(c)+(g): scalar `&T` / `&mut T` parameter
    // lowers to a single pointer-sized IR slot (`Type::U64`).
    // Compound `&T` (struct / tuple / enum) still leaf-flattens
    // through erasure for now — `Type::Ref` for compound pointees
    // would require stack-slot allocation for every leaf, which
    // is out of scope until the codegen layer learns to flatten
    // ref-of-struct.
    if let TypeDecl::Ref { inner, .. } = ty {
        if let Some(scalar) = lower_scalar(inner)
            && matches!(
                scalar,
                Type::I64 | Type::U64 | Type::F64 | Type::Bool
                    | Type::I8 | Type::U8 | Type::I16 | Type::U16
                    | Type::I32 | Type::U32
            ) {
                return Some(Type::U64);
            }
        // A5-P2: `&dyn Trait` is the trait-object form. Lower to a
        // 2-tuple (data_ptr, vtable_ptr) so it slots into the
        // existing tuple-passing ABI. The fat-pointer's trait
        // identity is recovered later by the lower pass via the
        // original `TypeDecl`, not from the IR Type alone.
        if matches!(inner.as_ref(), TypeDecl::Dyn(_)) {
            return Some(super::types::lower_dyn_fat_ptr(module));
        }
        return lower_param_or_return_type(inner, struct_defs, enum_defs, module, interner);
    }
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
                // AOT-COMPOUND-PTR-RW: recurse so type args
                // themselves can be compound (`Vec<Vec<u8>>` is the
                // landing case). Pre-fix `lower_scalar(a)?` rejected
                // any non-primitive arg, which was fine when no
                // user-space stdlib produced nested generic structs
                // yet — but Phase B's `Split<Vec<u8>, Vec<Vec<u8>>>`
                // and the underlying `__builtin_ptr_read/write` of
                // compound values now do.
                let l = lower_param_or_return_type(a, struct_defs, enum_defs, module, interner)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The frontend rejects recursive declarations before lowering
    /// ever sees them, but nothing stops an embedder from calling
    /// `lower_program` on an AST of its own. This is the backstop:
    /// the instantiation has to *return* rather than exhaust the
    /// host stack, since a stack overflow aborts the whole process
    /// and no caller can recover from it.
    #[test]
    fn a_self_referential_enum_errors_instead_of_recursing() {
        let mut interner = DefaultStringInterner::new();
        let list = interner.get_or_intern("List");
        let cons = interner.get_or_intern("Cons");
        let mut enum_defs: EnumDefs = HashMap::new();
        enum_defs.insert(
            list,
            EnumTemplate {
                generic_params: Vec::new(),
                variants: vec![EnumTemplateVariant {
                    name: cons,
                    payload_types: vec![TypeDecl::Identifier(list)],
                }],
            },
        );
        let struct_defs: StructDefs = HashMap::new();
        let mut module = Module::new();

        let err = instantiate_enum(
            &mut module,
            &enum_defs,
            &struct_defs,
            list,
            Vec::new(),
            &interner,
        )
        .expect_err("a self-referential enum has no finite layout");
        assert!(err.contains("is recursive"), "unexpected message: {err}");
    }

    #[test]
    fn a_self_referential_struct_errors_instead_of_recursing() {
        let mut interner = DefaultStringInterner::new();
        let node = interner.get_or_intern("Node");
        let mut struct_defs: StructDefs = HashMap::new();
        struct_defs.insert(
            node,
            StructTemplate {
                generic_params: Vec::new(),
                fields: vec![("next".to_string(), TypeDecl::Identifier(node))],
            },
        );
        let enum_defs: EnumDefs = HashMap::new();
        let mut module = Module::new();

        let err = instantiate_struct(
            &mut module,
            &struct_defs,
            &enum_defs,
            node,
            Vec::new(),
            &interner,
        )
        .expect_err("a self-referential struct has no finite layout");
        assert!(err.contains("is recursive"), "unexpected message: {err}");
    }

    /// The guard is scoped to one instantiation, not to the whole
    /// program: lowering the same type twice in sequence has to
    /// succeed both times (the second through the memo).
    #[test]
    fn the_guard_does_not_leak_between_instantiations() {
        let mut interner = DefaultStringInterner::new();
        let point = interner.get_or_intern("Point");
        let mut struct_defs: StructDefs = HashMap::new();
        struct_defs.insert(
            point,
            StructTemplate {
                generic_params: Vec::new(),
                fields: vec![("x".to_string(), TypeDecl::Int64)],
            },
        );
        let enum_defs: EnumDefs = HashMap::new();
        let mut module = Module::new();

        let first =
            instantiate_struct(&mut module, &struct_defs, &enum_defs, point, Vec::new(), &interner)
                .expect("first instantiation");
        let second =
            instantiate_struct(&mut module, &struct_defs, &enum_defs, point, Vec::new(), &interner)
                .expect("second instantiation");
        assert_eq!(first, second, "the same type interns to the same id");
    }
}
