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
    static PENDING_REFUSAL: RefCell<Option<String>> = const { RefCell::new(None) };
}

struct Guard(InstanceKey);

impl Guard {
    /// `None` when this instantiation is already on the stack.
    fn enter(key: InstanceKey) -> Option<Self> {
        IN_PROGRESS.with(|set| {
            if !set.borrow_mut().insert(key.clone()) {
                return None;
            }
            PENDING_REFUSAL.with(|c| *c.borrow_mut() = None);
            Some(Guard(key))
        })
    }
}

/// Whether this instantiation is on the stack right now — i.e. its
/// index entry is a reservation rather than a finished type.
fn in_progress(key: &InstanceKey) -> bool {
    IN_PROGRESS.with(|set| set.borrow().contains(key))
}

/// Record the reason so the frame that swallows this `Err` can report
/// it, and hand the same message back for the immediate return. Used
/// for a recursive type and for a member shape the MVP has no storage
/// for — both are refusals a caller would otherwise mis-describe.
fn note_refusal(message: String) -> String {
    PENDING_REFUSAL.with(|c| *c.borrow_mut() = Some(message.clone()));
    message
}

/// The reason a nested instantiation refused since this one started,
/// if there was one. Consumes it.
fn take_pending_refusal() -> Option<String> {
    PENDING_REFUSAL.with(|c| c.borrow_mut().take())
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
    instantiate_enum_inner(module, templates, struct_templates, base_name, type_args, interner, false)
}

/// `instantiate_enum` for a *type-argument* position, which is allowed
/// to see a reservation.
///
/// The difference is the whole point of the two-phase form. A member
/// position needs the type's shape, so meeting it mid-flight is a
/// genuine by-value cycle and must be refused. A type argument needs
/// only its identity — `Vec<Tree>` keys off `Tree`'s id and stores a
/// `ptr` — so meeting `Tree` mid-flight is exactly the case that should
/// resolve rather than recurse.
pub(super) fn instantiate_enum_type_arg(
    module: &mut Module,
    templates: &EnumDefs,
    struct_templates: &StructDefs,
    base_name: DefaultSymbol,
    type_args: Vec<Type>,
    interner: &DefaultStringInterner,
) -> Result<EnumId, String> {
    instantiate_enum_inner(module, templates, struct_templates, base_name, type_args, interner, true)
}

fn instantiate_enum_inner(
    module: &mut Module,
    templates: &EnumDefs,
    struct_templates: &StructDefs,
    base_name: DefaultSymbol,
    type_args: Vec<Type>,
    interner: &DefaultStringInterner,
    allow_pending: bool,
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
    let key: InstanceKey = (true, base_name, type_args.clone());
    if let Some(id) = module.enum_index.get(&(base_name, type_args.clone())).copied()
        && (allow_pending || !in_progress(&key))
    {
        return Ok(id);
    }
    let Some(_guard) = Guard::enter(key) else {
        return Err(note_refusal(recursive_type_error("enum", base_name, interner)));
    };
    // Claim the id before the payloads are lowered: one of them may
    // name a type whose own lowering needs *this* id back (an enum
    // parameterising a `Vec`, say). Interning afterwards made that walk
    // re-enter with the memo still empty.
    let id = match module.reserve_enum(base_name, type_args.clone()) {
        Ok(id) => id,
        Err(existing) => return Ok(existing),
    };
    let template = template.clone();
    let subst: HashMap<DefaultSymbol, Type> = template
        .generic_params
        .iter()
        .copied()
        .zip(type_args.iter().copied())
        .collect();
    let lowered_variants = (|| -> Result<Vec<EnumVariant>, String> {
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
                    take_pending_refusal().unwrap_or_else(|| {
                        format!(
                            "enum `{}::{}` has unsupported payload type `{}` \
                             (compiler MVP accepts i64 / u64 / f64 / bool, or another \
                             enum substituted from a generic parameter)",
                            interner.resolve(base_name).unwrap_or("?"),
                            interner.resolve(v.name).unwrap_or("?"),
                            crate::spelling::spell_type_decl(interner, pt),
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
        Ok(ir_variants)
    })();
    // A reservation whose payloads did not lower must not survive as a
    // finished type: the compile fails either way today, but a
    // key-reachable placeholder with no variants is the kind of thing
    // that turns a later error into a silently wrong program.
    match lowered_variants {
        Ok(ir_variants) => {
            module.fill_enum_variants(id, ir_variants);
            Ok(id)
        }
        Err(e) => {
            module.unreserve_enum(base_name, &type_args);
            Err(e)
        }
    }
}

pub(super) fn is_supported_enum_payload(t: Type) -> bool {
    // NUM-W-ENUMERATION: every scalar (the list this replaced had
    // every width but `f32`) and every compound.
    //
    // UNIT-TYPE-ARG: and `()`, for `Result<(), E>`'s `Ok` payload. It
    // occupies nothing (`flatten_compound_leaf_types` gives it zero
    // leaves), so it needs no representation, only permission.
    t.is_scalar() || matches!(t, Type::Enum(_) | Type::Struct(_) | Type::Tuple(_) | Type::Unit)
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
                let t = substitute_type_arg(
                    a,
                    subst,
                    module,
                    struct_templates,
                    enum_templates,
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
                let t = substitute_type_arg(
                    a,
                    subst,
                    module,
                    struct_templates,
                    enum_templates,
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
                // NUM-W-ENUMERATION: any scalar element. The list this
                // replaced had four, so `Pair<(u8, u64)>` asked for the
                // annotation it already had.
                if !t.is_scalar() {
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
    instantiate_struct_inner(module, templates, enum_templates, base_name, type_args, interner, false)
}

/// `instantiate_struct` for a *type-argument* position. See
/// `instantiate_enum_type_arg` for why the two differ.
pub(super) fn instantiate_struct_type_arg(
    module: &mut Module,
    templates: &StructDefs,
    enum_templates: &EnumDefs,
    base_name: DefaultSymbol,
    type_args: Vec<Type>,
    interner: &DefaultStringInterner,
) -> Result<StructId, String> {
    instantiate_struct_inner(module, templates, enum_templates, base_name, type_args, interner, true)
}

fn instantiate_struct_inner(
    module: &mut Module,
    templates: &StructDefs,
    enum_templates: &EnumDefs,
    base_name: DefaultSymbol,
    type_args: Vec<Type>,
    interner: &DefaultStringInterner,
    allow_pending: bool,
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
    let key: InstanceKey = (false, base_name, type_args.clone());
    if let Some(id) = module.struct_index.get(&(base_name, type_args.clone())).copied()
        && (allow_pending || !in_progress(&key))
    {
        return Ok(id);
    }
    let Some(_guard) = Guard::enter(key) else {
        return Err(note_refusal(recursive_type_error("struct", base_name, interner)));
    };
    // Same two-phase reservation as `instantiate_enum`: a field can
    // name a generic instantiated over the struct being built
    // (`struct Tree { kids: Vec<Tree> }`), and `Vec` needs a `Type` for
    // its argument rather than `Tree`'s field list.
    let id = match module.reserve_struct(base_name, type_args.clone()) {
        Ok(id) => id,
        Err(existing) => return Ok(existing),
    };
    let template = template.clone();
    let subst: HashMap<DefaultSymbol, Type> = template
        .generic_params
        .iter()
        .copied()
        .zip(type_args.iter().copied())
        .collect();
    let lowered_fields = (|| -> Result<Vec<(String, Type)>, String> {
        let mut concrete_fields: Vec<(String, Type)> = Vec::with_capacity(template.fields.len());
        for (fname, ftype) in &template.fields {
            let lowered =
                substitute_field_type(ftype, &subst, module, templates, enum_templates, interner)
                    .ok_or_else(|| {
                        take_pending_refusal().unwrap_or_else(|| {
                            format!(
                                "compiler MVP cannot lower struct field `{}.{}: {}`",
                                interner.resolve(base_name).unwrap_or("?"),
                                fname,
                                crate::spelling::spell_type_decl(interner, ftype),
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
        Ok(concrete_fields)
    })();
    // See `instantiate_enum`: a reservation that failed to lower is
    // withdrawn rather than left reachable as a field-less type.
    match lowered_fields {
        Ok(concrete_fields) => {
            module.fill_struct_fields(id, concrete_fields);
            Ok(id)
        }
        Err(e) => {
            module.unreserve_struct(base_name, &type_args);
            Err(e)
        }
    }
}

/// Recursively lower a struct field's declared type, applying the
/// active generic substitution. Recurses through nested generic
/// struct types so `Cell<Cell<i64>>` resolves all the way down.
/// Lower a named type's type arguments under the active generic
/// substitution. Shared by the struct and enum arms of
/// [`substitute_field_type`], which differ only in what they hand the
/// results to.
fn substitute_type_args(
    args: &[TypeDecl],
    subst: &HashMap<DefaultSymbol, Type>,
    module: &mut Module,
    templates: &StructDefs,
    enum_templates: &EnumDefs,
    interner: &DefaultStringInterner,
) -> Option<Vec<Type>> {
    let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
    for a in args {
        concrete.push(substitute_type_arg(
            a,
            subst,
            module,
            templates,
            enum_templates,
            interner,
        )?);
    }
    Some(concrete)
}

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
            let concrete = substitute_type_args(
                args,
                subst,
                module,
                templates,
                enum_templates,
                interner,
            )?;
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
        // A generic enum field (`value: Option<i64>`). The parser
        // cannot tell a struct name from an enum one, so this arrives
        // as `Struct(name, args)` too and only the template tables
        // say which it is — without this arm the non-generic spelling
        // lowered and the generic one did not
        // (STRUCT-FIELD-GENERIC-ENUM).
        TypeDecl::Struct(name, args) | TypeDecl::Enum(name, args)
            if enum_templates.contains_key(name) =>
        {
            let concrete = substitute_type_args(
                args,
                subst,
                module,
                templates,
                enum_templates,
                interner,
            )?;
            instantiate_enum(module, enum_templates, templates, *name, concrete, interner)
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

/// Lower a `TypeDecl` sitting in a **type-argument** position.
///
/// Same walk as `substitute_field_type`, except that it instantiates
/// through the pending-tolerant entry points: a type argument needs the
/// referenced type's identity, not its shape, so meeting a reservation
/// is the case that should resolve. Using `substitute_field_type` here
/// instead would make `struct Tree { kids: Vec<Tree> }` refuse itself —
/// the argument `Tree` would be read as a by-value member of `Vec`.
pub(super) fn substitute_type_arg(
    ty: &TypeDecl,
    subst: &HashMap<DefaultSymbol, Type>,
    module: &mut Module,
    struct_templates: &StructDefs,
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
            if struct_templates.contains_key(name) {
                instantiate_struct_type_arg(
                    module,
                    struct_templates,
                    enum_templates,
                    *name,
                    Vec::new(),
                    interner,
                )
                .ok()
                .map(Type::Struct)
            } else if enum_templates.contains_key(name) {
                instantiate_enum_type_arg(
                    module,
                    enum_templates,
                    struct_templates,
                    *name,
                    Vec::new(),
                    interner,
                )
                .ok()
                .map(Type::Enum)
            } else {
                None
            }
        }
        TypeDecl::Struct(name, args) if struct_templates.contains_key(name) => {
            let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                concrete.push(substitute_type_arg(
                    a,
                    subst,
                    module,
                    struct_templates,
                    enum_templates,
                    interner,
                )?);
            }
            instantiate_struct_type_arg(
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
        // STRUCT-FIELD-GENERIC-ENUM, in argument position: the parser
        // cannot tell a struct name from an enum one, so `Option<T>`
        // arrives as `Struct("Option", [T])` and only the template
        // tables say which it is. `substitute_field_type` has had
        // this arm since the field case was found; without the same
        // arm here, an enum *inside* another generic
        // (`Vec<Option<TcpStream>>` — a table of slots) lowered
        // nowhere, and the refusal surfaced as "cannot lower
        // parameter `c: &mut Conns`" one level up.
        TypeDecl::Struct(name, args) | TypeDecl::Enum(name, args)
            if enum_templates.contains_key(name) =>
        {
            let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
            for a in args {
                concrete.push(substitute_type_arg(
                    a,
                    subst,
                    module,
                    struct_templates,
                    enum_templates,
                    interner,
                )?);
            }
            instantiate_enum_type_arg(
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
        _ => None,
    }
}

/// Like `lower_scalar` but additionally accepts `Type::Struct(name)`
/// and `Type::Tuple(id)` for known struct types and structural
/// tuples respectively. Used at function-signature boundaries
/// (params and return type) where these compound shapes are
/// allowed; values inside the IR's value graph stay scalar.
/// The message for a parameter / return type the MVP could not lower.
///
/// `lower_param_or_return_type` returns `Option`, so a nested refusal
/// with a real explanation ("cannot hold an enum in struct field
/// `S.c`") would otherwise be replaced by the caller's own guess,
/// which only knows the spelling of the type. Prefer the recorded
/// reason when there is one.
pub(super) fn unlowerable_type_message(fallback: impl FnOnce() -> String) -> String {
    take_pending_refusal().unwrap_or_else(fallback)
}

/// The pointee's scalar IR type for a `&T` / `&mut T` parameter, or
/// `None` for anything the boundary does not hand over as a pointer.
///
/// Deliberately mirrors the scalar arm of `lower_param_or_return_type`:
/// that decides which parameters get a pointer slot, this decides
/// which call-site arguments have to have an address made for them,
/// and the two answers must be the same one. A compound pointee is
/// `None` because it is leaf-flattened at the boundary — no address
/// is involved on either side.
pub(super) fn param_ref_pointee_ty(ty: &TypeDecl) -> Option<Type> {
    let TypeDecl::Ref { inner, .. } = ty else {
        return None;
    };
    let scalar = lower_scalar(inner)?;
    is_scalar_pointee(scalar).then_some(scalar)
}

/// Whether a reference to this IR type is passed as an address. The
/// generic-instantiation path asks this about an *already substituted*
/// type argument, where there is no `TypeDecl` left to hand
/// [`param_ref_pointee_ty`].
pub(super) fn is_scalar_pointee(scalar: Type) -> bool {
    // Every scalar but `str`, whose value is already an address.
    scalar.is_scalar() && scalar != Type::Str
}

/// Replace every `Self` inside `ty` with the impl target's type.
///
/// The substitution used to be written inline as a top-level `match`,
/// which resolved `Self` for `self: Self` and `-> Self` but not for
/// anything wrapping it. `other: &Self` — the spelling `docs/language.md`
/// gives for every binary operator overload — therefore reached
/// `lower_param_or_return_type` still holding `Ref { inner: Self_ }` and
/// failed as "cannot lower method parameter", so the compiled lanes
/// rejected the documented form while accepting `other: &Vec3`.
/// Recurse through the type constructors instead.
pub(super) fn substitute_self(
    ty: &TypeDecl,
    self_decl: &TypeDecl,
    interner: &DefaultStringInterner,
) -> TypeDecl {
    let sub = |t: &TypeDecl| substitute_self(t, self_decl, interner);
    match ty {
        TypeDecl::Self_ => self_decl.clone(),
        // The parser spells the keyword as a bare identifier when it
        // appears where a user type could go.
        TypeDecl::Identifier(sym) if interner.resolve(*sym) == Some("Self") => self_decl.clone(),
        TypeDecl::Ref { is_mut, inner } => TypeDecl::Ref {
            is_mut: *is_mut,
            inner: Box::new(sub(inner)),
        },
        TypeDecl::Array(elems, size, soa) => {
            TypeDecl::Array(elems.iter().map(&sub).collect(), size.clone(), *soa)
        }
        TypeDecl::Tuple(elems) => TypeDecl::Tuple(elems.iter().map(&sub).collect()),
        TypeDecl::Struct(name, args) => TypeDecl::Struct(*name, args.iter().map(&sub).collect()),
        TypeDecl::Enum(name, args) => TypeDecl::Enum(*name, args.iter().map(&sub).collect()),
        TypeDecl::Dict(k, v) => TypeDecl::Dict(Box::new(sub(k)), Box::new(sub(v))),
        TypeDecl::Range(inner) => TypeDecl::Range(Box::new(sub(inner))),
        TypeDecl::Function(params, ret) => {
            TypeDecl::Function(params.iter().map(&sub).collect(), Box::new(sub(ret)))
        }
        other => other.clone(),
    }
}

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
            && is_scalar_pointee(scalar)
        {
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
                // STDLIB-ITER: recurse (like the struct arm above) so
                // a tuple type argument — `Option<(K, V)>` from
                // `DictIter::next` — lowers instead of being rejected
                // by `lower_scalar`.
                // UNIT-TYPE-ARG: a `()` argument is allowed here —
                // `Result<(), E>` — because an enum payload of no
                // width is exactly what a unit variant already has.
                let l = lower_param_or_return_type(a, struct_defs, enum_defs, module, interner)?;
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
                // STDLIB-ITER: recurse (see the `TypeDecl::Enum` arm
                // above) so tuple type arguments lower.
                // UNIT-TYPE-ARG: a `()` argument is allowed here —
                // `Result<(), E>` — because an enum payload of no
                // width is exactly what a unit variant already has.
                let l = lower_param_or_return_type(a, struct_defs, enum_defs, module, interner)?;
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

    /// What the two-phase reservation is for: a struct whose field is
    /// a generic instantiated over the struct itself.
    ///
    /// `Vec<Tree>` has a perfectly finite layout — `Vec` holds a `ptr`
    /// and never a `T` by value — but instantiating it needs a `Type`
    /// for the argument, and that argument is the `Tree` currently
    /// being built. Interning `Tree` only after its fields were lowered
    /// meant the walk re-entered `Tree` with the memo still empty and
    /// recursed until the host stack was gone. Reserving the id first
    /// makes the argument resolvable and the walk terminate.
    #[test]
    fn a_field_holding_a_generic_over_the_struct_itself_terminates() {
        let mut interner = DefaultStringInterner::new();
        let tree = interner.get_or_intern("Tree");
        let vec = interner.get_or_intern("Vec");
        let t_param = interner.get_or_intern("T");

        let mut struct_defs: StructDefs = HashMap::new();
        struct_defs.insert(
            vec,
            StructTemplate {
                generic_params: vec![t_param],
                // `T` appears nowhere in the fields — the element type
                // lives behind `data`.
                fields: vec![
                    ("data".to_string(), TypeDecl::Ptr),
                    ("len".to_string(), TypeDecl::UInt64),
                ],
            },
        );
        struct_defs.insert(
            tree,
            StructTemplate {
                generic_params: Vec::new(),
                fields: vec![
                    ("v".to_string(), TypeDecl::Int64),
                    (
                        "kids".to_string(),
                        TypeDecl::Struct(vec, vec![TypeDecl::Identifier(tree)]),
                    ),
                ],
            },
        );
        let enum_defs: EnumDefs = HashMap::new();
        let mut module = Module::new();

        let id = instantiate_struct(
            &mut module,
            &struct_defs,
            &enum_defs,
            tree,
            Vec::new(),
            &interner,
        )
        .expect("Vec<Tree> is finite, so Tree is");
        let fields = &module.struct_def(id).fields;
        // DIAG-DEBUG-FMT-OK: test assertions — the lowered field list is
        // what a failure here needs to show.
        assert_eq!(fields.len(), 2, "both fields lowered: {fields:?}");
        assert!(
            matches!(fields[1].1, Type::Struct(_)),
            "`kids` lowered to the Vec instance: {fields:?}"
        );
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
