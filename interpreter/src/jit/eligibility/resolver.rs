use std::collections::HashMap;

use frontend::ast::Function;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use super::checker::note;
use super::extern_dispatch::{enum_layout_for, primitive_type_decl_for_target_sym};
use super::layout::{EnumLayout, StructLayout};
use super::scalar::ScalarTy;
use super::signature::{FuncSignature, MonomorphSource, ParamTy};

/// Resolve a TypeDecl to its concrete ScalarTy after applying any active
/// generic substitutions. Returns None if the type cannot be represented
/// in the JIT (or if a referenced generic is unbound in this monomorph).
pub(crate) fn substitute_to_scalar(
    td: &TypeDecl,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
) -> Option<ScalarTy> {
    match td {
        TypeDecl::Generic(g) => substitutions.get(g).copied(),
        _ => ScalarTy::from_type_decl(td),
    }
}

/// Given a callee function and the resolved argument types at a call
/// site, derive the substitution map for the callee's generic params.
/// `caller_subs` is used to resolve `Generic(_)` references that appear
/// in non-generic param positions (e.g. when a generic function calls
/// another with one of its own generics as the arg type — though that
/// path is uncommon in our current scope).
pub(super) fn infer_substitutions(
    callee: &Function,
    arg_tys: &[ScalarTy],
    caller_subs: &HashMap<DefaultSymbol, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<HashMap<DefaultSymbol, ScalarTy>> {
    if callee.parameter.len() != arg_tys.len() {
        note(reject_reason, || {
            format!(
                "call has {} arg(s), callee expects {}",
                arg_tys.len(),
                callee.parameter.len()
            )
        });
        return None;
    }
    let mut subs: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
    for ((_, param_td), &arg_ty) in callee.parameter.iter().zip(arg_tys.iter()) {
        // Struct / tuple param positions skip generic inference and
        // scalar matching — the caller has already validated that the
        // arg's struct/tuple type lines up with the callee's declared
        // type.
        if matches!(
            param_td,
            TypeDecl::Identifier(_) | TypeDecl::Struct(_, _) | TypeDecl::Tuple(_)
        ) {
            continue;
        }
        match param_td {
            TypeDecl::Generic(g) => {
                if let Some(prev) = subs.insert(*g, arg_ty) {
                    if prev != arg_ty {
                        note(reject_reason, || {
                            format!(
                                "generic parameter bound to conflicting types {prev:?} and {arg_ty:?}"
                            )
                        });
                        return None;
                    }
                }
            }
            other => {
                let resolved = substitute_to_scalar(other, caller_subs);
                match resolved {
                    Some(r) if r == arg_ty => {}
                    _ => {
                        note(reject_reason, || {
                            format!(
                                "callee parameter type {other:?} does not match arg type {arg_ty:?}"
                            )
                        });
                        return None;
                    }
                }
            }
        }
    }
    // Every generic_param must be bound by now.
    for g in &callee.generic_params {
        if !subs.contains_key(g) {
            note(reject_reason, || {
                "could not infer all generic type arguments from call site".to_string()
            });
            return None;
        }
    }
    Some(subs)
}

/// Compute a JIT signature for either a free function or a method.
/// `receiver_struct` is `Some(struct)` when `source` is a method bound
/// to that struct; the function uses it to resolve any `TypeDecl::Self_`
/// references in the parameter list / return type.
pub(super) fn callable_signature(
    source: &MonomorphSource,
    receiver_struct: Option<DefaultSymbol>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    reject_reason: &mut Option<String>,
) -> Option<FuncSignature> {
    let parameters = source.parameter();
    let mut params = Vec::with_capacity(parameters.len());
    let self_struct = receiver_struct;
    for (_, td) in parameters {
        // Map `Self_` to the receiver's type for methods. For a
        // primitive impl target (extension trait — Step C onward),
        // expand `Self_` directly to the matching primitive
        // `TypeDecl` so `resolve_param_ty` can reduce it to a
        // `ParamTy::Scalar`. Struct receivers fall back to
        // `TypeDecl::Identifier` as before.
        let resolved_td = match (td, self_struct) {
            (TypeDecl::Self_, Some(s)) => primitive_type_decl_for_target_sym(s)
                .unwrap_or_else(|| TypeDecl::Identifier(s)),
            (other, _) => other.clone(),
        };
        let pt = match resolve_param_ty(&resolved_td, substitutions, struct_layouts) {
            Some(p) => p,
            None => {
                note(reject_reason, || {
                    format!("parameter has unsupported type {resolved_td:?}")
                });
                return None;
            }
        };
        if matches!(pt, ParamTy::Scalar(ScalarTy::Unit)) {
            note(reject_reason, || {
                "parameter type Unit is not supported".to_string()
            });
            return None;
        }
        params.push((parameters[params.len()].0, pt));
    }
    // Return type. Scalars and structs are both allowed; struct returns
    // expand into cranelift multi-returns (one cranelift return per
    // field) at the ABI layer.
    let ret = match source.return_type() {
        Some(td) => {
            // Map `Self_` similarly for methods. Primitive impl
            // targets resolve to the matching primitive `TypeDecl`
            // so the return is a `ParamTy::Scalar`.
            let resolved_td = match (td, self_struct) {
                (TypeDecl::Self_, Some(s)) => primitive_type_decl_for_target_sym(s)
                    .unwrap_or_else(|| TypeDecl::Identifier(s)),
                (other, _) => other.clone(),
            };
            match resolve_param_ty(&resolved_td, substitutions, struct_layouts) {
                Some(p) => p,
                None => {
                    note(reject_reason, || {
                        format!("return type {resolved_td:?} is not supported")
                    });
                    return None;
                }
            }
        }
        None => ParamTy::Scalar(ScalarTy::Unit),
    };
    Some(FuncSignature { params, ret })
}

/// Resolve a TypeDecl into a JIT parameter type, considering both scalar
/// substitutions (for generic monomorphs) and known struct layouts.
/// Tuples whose elements all resolve to scalars become `ParamTy::Tuple`.
pub(crate) fn resolve_param_ty(
    td: &TypeDecl,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
) -> Option<ParamTy> {
    // STR-INTERP-INTERP-JIT: str is supported INSIDE a JIT function
    // (interpolation chain → println), but must NOT cross the
    // function boundary as a parameter or return type. The JIT's
    // heap-allocated str pointer has no `Object` lifecycle owner,
    // so handing it to interpreter code (or accepting one from
    // interpreter code) would be unsound.
    if matches!(td, TypeDecl::String) {
        return None;
    }
    if let Some(s) = substitute_to_scalar(td, substitutions) {
        return Some(ParamTy::Scalar(s));
    }
    match td {
        TypeDecl::Identifier(s) | TypeDecl::Struct(s, _)
            if struct_layouts.contains_key(s) =>
        {
            Some(ParamTy::Struct(*s))
        }
        // Phase JE-2d: enum-typed parameters / returns. The
        // `enum_layouts` map only contains JIT-eligible enums
        // (non-generic, uniform / no payload), so any hit is safe
        // to expand at the boundary.
        TypeDecl::Identifier(s) if enum_layout_for(*s).is_some() => {
            // Phase JE-5/JE-6: non-generic enum payload_ty is
            // determined entirely by the layout. For generic enums
            // referenced via bare `Identifier` (e.g. `Self_` resolved
            // to an enum name when the impl had generic params), the
            // substitution map drives the resolution — required for
            // method dispatch on generic enums (`Option<T>::is_some`).
            let layout = enum_layout_for(*s)?;
            let payload_ty = if layout.generic_params.is_empty() {
                layout.payload_ty()
            } else {
                let resolved = layout.resolve_uniform_payload(substitutions);
                if layout.variant_payloads.iter().any(|v| v.is_some()) && resolved.is_none() {
                    return None;
                }
                resolved
            };
            Some(ParamTy::Enum { base_name: *s, payload_ty })
        }
        // Phase JE-5: parser-ambiguous form. The frontend emits
        // `TypeDecl::Struct(s, args)` for any `Name<Args>` (it
        // can't tell enums from structs at parse time). When the
        // name actually resolves to a JIT-eligible enum (and not
        // a struct), treat it as `ParamTy::Enum` with payload
        // resolved from the args.
        TypeDecl::Struct(s, args)
            if enum_layout_for(*s).is_some() && !struct_layouts.contains_key(s) =>
        {
            let layout = enum_layout_for(*s)?;
            let synthetic_td = TypeDecl::Enum(*s, args.clone());
            let payload_ty = if args.is_empty() {
                layout.payload_ty()
            } else {
                payload_ty_from_annotation(&synthetic_td, &layout)
            };
            if layout.variant_payloads.iter().any(|v| v.is_some())
                && payload_ty.is_none()
            {
                return None;
            }
            Some(ParamTy::Enum { base_name: *s, payload_ty })
        }
        TypeDecl::Enum(s, args) if enum_layout_for(*s).is_some() => {
            // Phase JE-5: generic enum monomorph at a function
            // boundary. Build a substitution from the layout's
            // generic_params to the call-site type args, then
            // resolve to the uniform per-monomorph payload scalar.
            let layout = enum_layout_for(*s)?;
            let payload_ty = if args.is_empty() {
                layout.payload_ty()
            } else {
                payload_ty_from_annotation(td, &layout)
            };
            if layout.variant_payloads.iter().any(|v| v.is_some())
                && payload_ty.is_none()
            {
                return None;
            }
            Some(ParamTy::Enum { base_name: *s, payload_ty })
        }
        TypeDecl::Tuple(elements) => {
            // Tuples are scalar-only at the JIT layer; any non-scalar
            // element (a nested tuple `((a,b),c)` or a struct element
            // `(Point, i64)`) drops us back to the interpreter. todo
            // #160 tracks lifting this by extending `ParamTy::Tuple`
            // to a tree of element shapes — large enough to defer.
            let mut scalars: Vec<ScalarTy> = Vec::with_capacity(elements.len());
            for e in elements {
                let s = substitute_to_scalar(e, substitutions)?;
                if s == ScalarTy::Unit {
                    return None;
                }
                scalars.push(s);
            }
            if scalars.len() < 2 {
                return None;
            }
            Some(ParamTy::Tuple(scalars))
        }
        _ => None,
    }
}

/// Public re-export of `payload_ty_from_annotation` so codegen
/// can reuse the resolution helper without re-implementing the
/// substitution logic.
pub(crate) fn payload_ty_from_annotation_pub(
    annotation: &TypeDecl,
    layout: &EnumLayout,
) -> Option<ScalarTy> {
    payload_ty_from_annotation(annotation, layout)
}

/// Phase JE-3/JE-4: extract the payload scalar type from a val/var
/// annotation, given the enum's layout. Handles `Option<i64>` form
/// (`TypeDecl::Enum(name, [args])`) and resolves the layout's
/// generic params against the annotation's type args.
///
/// For multi-generic enums (`Result<T, E>`), all variant payloads
/// must resolve to the same scalar (the JIT's single-payload-slot
/// representation). `resolve_uniform_payload` returns `None` if
/// any pair disagrees (e.g. `Result<i64, bool>`), causing the JIT
/// to skip the program with the regular fallback.
pub(super) fn payload_ty_from_annotation(
    annotation: &TypeDecl,
    layout: &EnumLayout,
) -> Option<ScalarTy> {
    let args = match annotation {
        TypeDecl::Enum(_, args) => args,
        TypeDecl::Struct(_, args) => args,
        _ => return None,
    };
    if layout.generic_params.len() != args.len() || args.is_empty() {
        return None;
    }
    // Build a substitution map from the layout's generic params to
    // the annotation's type args.
    let mut subst: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
    for (p, a) in layout.generic_params.iter().zip(args.iter()) {
        let sty = ScalarTy::from_type_decl(a)?;
        subst.insert(*p, sty);
    }
    layout.resolve_uniform_payload(&subst)
}
