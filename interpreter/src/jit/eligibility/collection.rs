use std::collections::HashMap;
use std::rc::Rc;

use frontend::ast::{MethodFunction, Program, Stmt, StmtRef};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use super::layout::{EnumLayout, StructLayout};
use super::scalar::{PayloadRepr, ScalarTy};

/// Look up a method on a struct by linear scanning ImplBlock decls.
pub(super) fn find_method(
    program: &Program,
    struct_name: DefaultSymbol,
    method_name: DefaultSymbol,
) -> Option<Rc<MethodFunction>> {
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(Stmt::ImplBlock { target_type, methods, .. }) = program.statement.get(&stmt_ref) {
            if target_type == struct_name {
                if let Some(m) = methods.iter().find(|m| m.name == method_name) {
                    return Some(m.clone());
                }
            }
        }
    }
    None
}

/// Build a `(struct_name, method_name) -> MethodFunction` map from every
/// top-level `Stmt::ImplBlock` in the program.
pub(super) fn collect_method_map(
    program: &Program,
) -> HashMap<(DefaultSymbol, DefaultSymbol), Rc<MethodFunction>> {
    let mut out: HashMap<(DefaultSymbol, DefaultSymbol), Rc<MethodFunction>> =
        HashMap::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(Stmt::ImplBlock { target_type, methods, .. }) = program.statement.get(&stmt_ref) {
            for m in &methods {
                out.insert((target_type, m.name), m.clone());
            }
        }
    }
    out
}

pub(super) fn collect_struct_layouts(
    program: &Program,
    interner: &DefaultStringInterner,
) -> HashMap<DefaultSymbol, StructLayout> {
    let mut out: HashMap<DefaultSymbol, StructLayout> = HashMap::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(Stmt::StructDecl {
            name,
            generic_params,
            fields,
            ..
        }) = program.statement.get(&stmt_ref)
        {
            // Generic structs aren't supported in this iteration — the
            // JIT would need per-monomorph layouts.
            if !generic_params.is_empty() {
                continue;
            }
            let mut scalar_fields: Vec<(DefaultSymbol, ScalarTy)> = Vec::with_capacity(fields.len());
            let mut all_scalar = true;
            for f in &fields {
                match ScalarTy::from_type_decl(&f.type_decl) {
                    Some(t) if t != ScalarTy::Unit => {
                        // Resolving the field name to its symbol via the
                        // interner avoids an extra string lookup at every
                        // FieldAccess site.
                        let sym = interner
                            .get(f.name.as_str())
                            .unwrap_or_else(|| {
                                // Fall back: insert into a clone of the
                                // interner. This shouldn't happen in
                                // practice because the parser interned
                                // every identifier already.
                                let mut tmp = interner.clone();
                                tmp.get_or_intern(f.name.as_str())
                            });
                        scalar_fields.push((sym, t));
                    }
                    _ => {
                        all_scalar = false;
                        break;
                    }
                }
            }
            if all_scalar {
                out.insert(
                    name,
                    StructLayout {
                        fields: scalar_fields,
                    },
                );
            }
        }
    }
    out
}

/// Pre-pass over `Stmt::EnumDecl` declarations: build a layout for
/// every JIT-compatible enum (Phase JE-1: non-generic, unit-only).
/// Anything with a tuple variant or generic param is silently
/// omitted; eligibility checks downstream will reject references to
/// it via the regular "JIT does not yet model enum values" path.
pub(super) fn collect_enum_layouts(program: &Program) -> HashMap<DefaultSymbol, EnumLayout> {
    let mut out: HashMap<DefaultSymbol, EnumLayout> = HashMap::new();
    for i in 0..program.statement.len() {
        if let Some(Stmt::EnumDecl {
            name,
            generic_params,
            variants,
            ..
        }) = program.statement.get(&StmtRef(i as u32))
        {
            // Phase JE-4: accept enums with any number of generic
            // params (`Option<T>` / `Result<T, E>` etc.). Each
            // variant's payload may reference any one of those
            // params, or be a concrete scalar. The JIT's
            // single-payload-slot representation still requires
            // that every variant's payload resolves to the same
            // scalar at instantiation time — checked via
            // `resolve_uniform_payload`. Mixed-width payloads at
            // a given monomorph fail to resolve and skip JIT.
            let collected_generic_params: Vec<DefaultSymbol> =
                generic_params.iter().copied().collect();
            // Phase JE-2a: every tuple variant has exactly one
            // payload (multi-payload variants are still out of
            // scope). Unit variants are allowed alongside tuple
            // variants.
            let mut variant_names: Vec<DefaultSymbol> = Vec::with_capacity(variants.len());
            let mut variant_has_payload: Vec<bool> = Vec::with_capacity(variants.len());
            let mut variant_payloads: Vec<Option<PayloadRepr>> =
                Vec::with_capacity(variants.len());
            let mut accept = true;
            for v in &variants {
                variant_names.push(v.name);
                match v.payload_types.as_slice() {
                    [] => {
                        variant_has_payload.push(false);
                        variant_payloads.push(None);
                    }
                    [single] => {
                        // Phase JE-3/JE-4: detect "payload IS one of
                        // the enum's generic params" by matching on
                        // `TypeDecl::Generic(sym)` (which the type
                        // checker emits for generic-param refs) or
                        // `TypeDecl::Identifier(sym)` for the bare
                        // form. Concrete payloads fall back to
                        // ScalarTy::from_type_decl.
                        let payload_is_generic = match single {
                            TypeDecl::Generic(sym)
                                if collected_generic_params.contains(sym) =>
                            {
                                Some(*sym)
                            }
                            TypeDecl::Identifier(sym)
                                if collected_generic_params.contains(sym) =>
                            {
                                Some(*sym)
                            }
                            _ => None,
                        };
                        let proposed: PayloadRepr = if let Some(p) = payload_is_generic {
                            PayloadRepr::Generic(p)
                        } else {
                            match ScalarTy::from_type_decl(single) {
                                Some(s) if !matches!(
                                    s,
                                    ScalarTy::Unit | ScalarTy::Allocator | ScalarTy::Never
                                ) => PayloadRepr::Concrete(s),
                                _ => {
                                    accept = false;
                                    break;
                                }
                            }
                        };
                        variant_has_payload.push(true);
                        variant_payloads.push(Some(proposed));
                    }
                    _ => {
                        // Multi-payload tuple variant — JE-2a scope
                        // is single-payload only.
                        accept = false;
                        break;
                    }
                }
            }
            if accept {
                out.insert(
                    name,
                    EnumLayout {
                        base_name: name,
                        variants: variant_names,
                        variant_payloads,
                        variant_has_payload,
                        generic_params: collected_generic_params,
                    },
                );
            }
        }
    }
    out
}
