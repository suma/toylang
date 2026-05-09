use std::collections::HashMap;

use frontend::ast::{
    BuiltinFunction, Expr, ExprRef, Operator, Pattern, Program, Stmt, StmtRef, UnaryOp,
};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use super::analyze::{expr_kind_name, MonoCall};
use super::collection::find_method;
use super::extern_dispatch::{
    concat_sym, enum_layout_for, jit_extern_dispatch_for,
    primitive_target_sym_for_scalar, primitive_type_decl_for_target_sym,
    scalar_ty_for_enum_decl,
};
use super::layout::{CompoundLocals, StructLayout};
use super::resolver::{
    infer_substitutions, payload_ty_from_annotation, resolve_param_ty, substitute_to_scalar,
};
use super::scalar::{EnumLocalInfo, PayloadRepr, ScalarTy};
use super::signature::{FuncSignature, MonoTarget, MonomorphSource, ParamTy};

/// Records the *first* reason eligibility analysis rejected the program.
/// Subsequent rejections deeper in the recursion are ignored — the user
/// only needs the closest hint to the surface.
pub(super) fn note(reason: &mut Option<String>, msg: impl FnOnce() -> String) {
    if reason.is_none() {
        *reason = Some(msg());
    }
}

/// Walks a function or method body to confirm it only uses supported
/// constructs and reports every callee found via `callees`. Returns
/// false on the first unsupported construct.
pub(super) fn check_callable_body(
    program: &Program,
    source: &MonomorphSource,
    sig: &FuncSignature,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let code = source.code();
    // Generic sources are forbidden from using `__builtin_ptr_read`
    // because the hint table is keyed by ExprRef, which is shared across
    // monomorphs of the same body. Reject early so the diagnostic is
    // clearer than a per-arm rejection deep inside the body.
    if !source.generic_params().is_empty() && body_has_ptr_read(program, &code) {
        note(reject_reason, || {
            "generic functions cannot use __builtin_ptr_read in JIT".to_string()
        });
        return false;
    }
    let mut locals: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
    let mut compound_locals = CompoundLocals::new();
    for (n, t) in &sig.params {
        match t {
            ParamTy::Scalar(s) => {
                locals.insert(*n, *s);
            }
            ParamTy::Struct(struct_name) => {
                compound_locals.structs.insert(*n, *struct_name);
            }
            ParamTy::Tuple(elements) => {
                compound_locals.tuples.insert(*n, elements.clone());
            }
            // Phase JE-2d/JE-5: enum-typed param registers as an
            // enum local. `ParamTy::Enum` now carries the resolved
            // per-monomorph `payload_ty` (JE-5), so generic enums
            // at the boundary (`Opt<i64>` / `Result<T, E>`) also
            // work — the boundary expansion uses the same payload
            // type as the local does.
            ParamTy::Enum { base_name, payload_ty } => {
                compound_locals.enums.insert(*n, EnumLocalInfo::new(*base_name, *payload_ty));
            }
        }
    }

    // Phase JE-2d: enum-returning bodies use a separate validator
    // mirroring struct/tuple — the body's tail expression must be
    // an enum producer (identifier of an enum local, constructor,
    // or a Match whose arms each produce the right enum).
    if let ParamTy::Enum { base_name: enum_name, payload_ty: enum_payload_ty } = &sig.ret {
        return check_enum_returning_body(
            program,
            &code,
            *enum_name,
            *enum_payload_ty,
            &mut locals,
            &mut compound_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        );
    }
    // For struct-returning functions, the body's terminal expression
    // must produce a struct value (Identifier of a struct local, or a
    // StructLiteral). check_expr rejects struct literals in arbitrary
    // positions, so we process the leading statements normally and then
    // validate the trailing expression by hand.
    if let ParamTy::Struct(struct_name) = &sig.ret {
        return check_struct_returning_body(
            program,
            &code,
            *struct_name,
            &mut locals,
            &mut compound_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        );
    }
    // Tuple-returning functions follow a similar shape — last expression
    // must produce a tuple value, fields are gathered for multi-return.
    if let ParamTy::Tuple(element_tys) = &sig.ret {
        return check_tuple_returning_body(
            program,
            &code,
            element_tys,
            &mut locals,
            &mut compound_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        );
    }

    check_stmt(
        program,
        &code,
        &mut locals,
        &mut compound_locals,
        substitutions,
        struct_layouts,
        callees,
        ptr_read_hints,
        reject_reason,
    )
}

#[allow(clippy::too_many_arguments)]
fn check_struct_returning_body(
    program: &Program,
    body_stmt_ref: &StmtRef,
    struct_name: DefaultSymbol,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let body_stmt = match program.statement.get(body_stmt_ref) {
        Some(s) => s,
        None => return false,
    };
    let body_expr_ref = match body_stmt {
        Stmt::Expression(e) => e,
        _ => {
            note(reject_reason, || {
                "struct-returning function body must be an expression".to_string()
            });
            return false;
        }
    };
    let body_expr = match program.expression.get(&body_expr_ref) {
        Some(e) => e,
        None => return false,
    };
    let block_stmts = match body_expr {
        Expr::Block(stmts) => stmts,
        _ => {
            note(reject_reason, || {
                "struct-returning function body must be a block".to_string()
            });
            return false;
        }
    };
    if block_stmts.is_empty() {
        note(reject_reason, || {
            "struct-returning function body cannot be empty".to_string()
        });
        return false;
    }
    let (last_ref, leading) = block_stmts.split_last().unwrap();
    for s in leading {
        if !check_stmt(
            program,
            s,
            locals,
            compound_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        ) {
            return false;
        }
    }
    // The trailing statement must produce the declared struct value.
    let last_stmt = match program.statement.get(last_ref) {
        Some(s) => s,
        None => return false,
    };
    let result_expr_ref = match last_stmt {
        Stmt::Expression(e) => e,
        _ => {
            note(reject_reason, || {
                "struct-returning function body must end in an expression".to_string()
            });
            return false;
        }
    };
    let result_expr = match program.expression.get(&result_expr_ref) {
        Some(e) => e,
        None => return false,
    };
    match result_expr {
        Expr::Identifier(name) => {
            match compound_locals.structs.get(&name).copied() {
                Some(s) if s == struct_name => true,
                Some(_) => {
                    note(reject_reason, || {
                        "returned struct local has a different type than declared"
                            .to_string()
                    });
                    false
                }
                None => {
                    note(reject_reason, || {
                        "returned identifier is not a known struct local".to_string()
                    });
                    false
                }
            }
        }
        Expr::StructLiteral(lit_name, _) => {
            if lit_name != struct_name {
                note(reject_reason, || {
                    "returned struct literal does not match declared return type"
                        .to_string()
                });
                return false;
            }
            // Validate fields against the layout. Use a temporary
            // variable name so check_struct_literal_fields can be reused
            // — we don't need to actually keep the struct local around.
            check_struct_literal_fields(
                program,
                &result_expr_ref,
                struct_name,
                locals,
                compound_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            )
        }
        _ => {
            note(reject_reason, || {
                "struct return value must be an identifier or struct literal"
                    .to_string()
            });
            false
        }
    }
}

/// Tuple-returning analog of `check_struct_returning_body`. The body's
/// last expression must be either a TupleLiteral with the declared
/// element types or an Identifier of a tuple local with the same shape.
#[allow(clippy::too_many_arguments)]
/// Phase JE-2d: enum-returning function body validator. The body's
/// tail expression must be an enum producer that codegen's
/// `gather_enum_values` can lower:
///   - `Identifier` of a known enum local
///   - `QualifiedIdentifier([enum, variant])` for unit constructors
///   - `AssociatedFunctionCall(enum, variant, [arg])` for tuple constructors
///   - `Match` whose every arm body is itself a valid enum producer
///     for `enum_name` (recursive).
#[allow(clippy::too_many_arguments)]
fn check_enum_returning_body(
    program: &Program,
    body_stmt_ref: &StmtRef,
    enum_name: DefaultSymbol,
    enum_payload_ty: Option<ScalarTy>,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let body_stmt = match program.statement.get(body_stmt_ref) {
        Some(s) => s,
        None => return false,
    };
    let body_expr_ref = match body_stmt {
        Stmt::Expression(e) => e,
        _ => {
            note(reject_reason, || {
                "enum-returning function body must be an expression".to_string()
            });
            return false;
        }
    };
    let body_expr = match program.expression.get(&body_expr_ref) {
        Some(e) => e,
        None => return false,
    };
    let block_stmts = match body_expr {
        Expr::Block(stmts) => stmts,
        _ => {
            note(reject_reason, || {
                "enum-returning function body must be a block".to_string()
            });
            return false;
        }
    };
    if block_stmts.is_empty() {
        note(reject_reason, || {
            "enum-returning function body cannot be empty".to_string()
        });
        return false;
    }
    let (last_ref, leading) = block_stmts.split_last().unwrap();
    for s in leading {
        if !check_stmt(
            program, s, locals, compound_locals,
            substitutions, struct_layouts, callees, ptr_read_hints,
            reject_reason,
        ) {
            return false;
        }
    }
    let last_stmt = match program.statement.get(last_ref) {
        Some(s) => s,
        None => return false,
    };
    let result_expr_ref = match last_stmt {
        Stmt::Expression(e) => e,
        _ => {
            note(reject_reason, || {
                "enum-returning function body must end in an expression".to_string()
            });
            return false;
        }
    };
    check_enum_producing_expr(
        program, &result_expr_ref, enum_name, enum_payload_ty, locals, compound_locals, substitutions, struct_layouts, callees,
        ptr_read_hints, reject_reason,
    )
}

/// Phase JE-2d: validate an enum-producing expression for a target
/// enum type. Recurses through `Match` so each arm body is checked
/// against the same target.
#[allow(clippy::too_many_arguments)]
fn check_enum_producing_expr(
    program: &Program,
    expr_ref: &ExprRef,
    enum_name: DefaultSymbol,
    enum_payload_ty: Option<ScalarTy>,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let expr = match program.expression.get(expr_ref) {
        Some(e) => e,
        None => return false,
    };
    match expr {
        Expr::Identifier(name) => {
            match compound_locals.enums.get(&name).copied() {
                Some(info) if info.base_name == enum_name => true,
                _ => {
                    note(reject_reason, || {
                        "returned identifier is not an enum local of the declared type".to_string()
                    });
                    false
                }
            }
        }
        Expr::QualifiedIdentifier(path) if path.len() == 2 && path[0] == enum_name => {
            // Unit constructor; layout existence already verified at
            // signature-resolution time. variant_tag must succeed.
            match enum_layout_for(enum_name).and_then(|l| l.variant_tag(path[1])) {
                Some(_) => true,
                None => {
                    note(reject_reason, || {
                        "returned constructor variant is not declared on this enum".to_string()
                    });
                    false
                }
            }
        }
        Expr::AssociatedFunctionCall(callee_enum, variant, args) if callee_enum == enum_name => {
            // Tuple constructor — validate the payload arg.
            let layout = match enum_layout_for(enum_name) {
                Some(l) => l,
                None => return false,
            };
            let idx = match layout.variants.iter().position(|n| *n == variant) {
                Some(i) => i,
                None => {
                    note(reject_reason, || {
                        "returned constructor variant is not declared on this enum".to_string()
                    });
                    return false;
                }
            };
            if !layout.variant_has_payload[idx] {
                note(reject_reason, || {
                    "returned unit variant constructed with `(...)`".to_string()
                });
                return false;
            }
            // Phase JE-5: prefer the per-monomorph payload_ty
            // (provided by the caller — e.g. the function's
            // `ParamTy::Enum.payload_ty` for return-type checking)
            // over the layout's, so generic enums work.
            let payload_ty = match enum_payload_ty.or_else(|| layout.payload_ty()) {
                Some(t) => t,
                None => return false,
            };
            if args.len() != 1 {
                note(reject_reason, || {
                    "tuple constructor must have one payload arg".to_string()
                });
                return false;
            }
            match check_expr(
                program, &args[0], locals, compound_locals, substitutions, struct_layouts, callees,
                ptr_read_hints, reject_reason,
            ) {
                Some(t) if t == payload_ty => true,
                _ => {
                    note(reject_reason, || {
                        "tuple constructor payload type does not match declared".to_string()
                    });
                    false
                }
            }
        }
        Expr::Match(scrutinee, arms) => {
            // Type-check the scrutinee + patterns, then recurse on
            // each arm body against the same enum target.
            let scrut_ty = match check_expr(
                program, &scrutinee, locals, compound_locals, substitutions, struct_layouts, callees,
                ptr_read_hints, reject_reason,
            ) {
                Some(t) => t,
                None => return false,
            };
            let scrut_enum_info = match program.expression.get(&scrutinee) {
                Some(Expr::Identifier(s)) => compound_locals.enums.get(&s).copied(),
                _ => None,
            };
            for arm in &arms {
                let payload_binding = match check_match_pattern(
                    program, &arm.pattern, scrut_ty, scrut_enum_info, reject_reason,
                ) {
                    Some(b) => b,
                    None => return false,
                };
                if arm.guard.is_some() {
                    note(reject_reason, || {
                        "JIT match arm guards are not yet supported".to_string()
                    });
                    return false;
                }
                let prev_local = if let Some((name, ty)) = payload_binding {
                    Some((name, locals.insert(name, ty)))
                } else {
                    None
                };
                let ok = check_enum_producing_expr(
                    program, &arm.body, enum_name, enum_payload_ty, locals, compound_locals, substitutions, struct_layouts,
                    callees, ptr_read_hints, reject_reason,
                );
                if let Some((name, prior)) = prev_local {
                    match prior {
                        Some(t) => { locals.insert(name, t); }
                        None => { locals.remove(&name); }
                    }
                }
                if !ok {
                    return false;
                }
            }
            true
        }
        _ => {
            note(reject_reason, || {
                "enum return expression must be an enum local, constructor, or match".to_string()
            });
            false
        }
    }
}

fn check_tuple_returning_body(
    program: &Program,
    body_stmt_ref: &StmtRef,
    element_tys: &[ScalarTy],
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let body_stmt = match program.statement.get(body_stmt_ref) {
        Some(s) => s,
        None => return false,
    };
    let body_expr_ref = match body_stmt {
        Stmt::Expression(e) => e,
        _ => {
            note(reject_reason, || {
                "tuple-returning function body must be an expression".to_string()
            });
            return false;
        }
    };
    let body_expr = match program.expression.get(&body_expr_ref) {
        Some(e) => e,
        None => return false,
    };
    let block_stmts = match body_expr {
        Expr::Block(stmts) => stmts,
        _ => {
            note(reject_reason, || {
                "tuple-returning function body must be a block".to_string()
            });
            return false;
        }
    };
    if block_stmts.is_empty() {
        note(reject_reason, || {
            "tuple-returning function body cannot be empty".to_string()
        });
        return false;
    }
    let (last_ref, leading) = block_stmts.split_last().unwrap();
    for s in leading {
        if !check_stmt(
            program,
            s,
            locals,
            compound_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        ) {
            return false;
        }
    }
    let last_stmt = match program.statement.get(last_ref) {
        Some(s) => s,
        None => return false,
    };
    let result_expr_ref = match last_stmt {
        Stmt::Expression(e) => e,
        _ => {
            note(reject_reason, || {
                "tuple-returning function body must end in an expression".to_string()
            });
            return false;
        }
    };
    let result_expr = match program.expression.get(&result_expr_ref) {
        Some(e) => e,
        None => return false,
    };
    match result_expr {
        Expr::Identifier(name) => match compound_locals.tuples.get(&name).cloned() {
            Some(shape) if shape.as_slice() == element_tys => true,
            Some(_) => {
                note(reject_reason, || {
                    "returned tuple local has a different shape than declared".to_string()
                });
                false
            }
            None => {
                note(reject_reason, || {
                    "returned identifier is not a known tuple local".to_string()
                });
                false
            }
        },
        Expr::TupleLiteral(elems) => check_tuple_literal_fields(
            program,
            &elems,
            element_tys,
            locals,
            compound_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        ),
        _ => {
            note(reject_reason, || {
                "tuple return value must be an identifier or tuple literal".to_string()
            });
            false
        }
    }
}

/// Validate every element of a tuple literal against the expected
/// element types. Records callees / ptr_read hints encountered while
/// typing the individual element initializers.
#[allow(clippy::too_many_arguments)]
fn check_tuple_literal_fields(
    program: &Program,
    elements: &[ExprRef],
    expected: &[ScalarTy],
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    if elements.len() != expected.len() {
        note(reject_reason, || {
            format!(
                "tuple literal has {} element(s), expected {}",
                elements.len(),
                expected.len()
            )
        });
        return false;
    }
    for (e, want) in elements.iter().zip(expected.iter()) {
        let actual = match check_expr(
            program,
            e,
            locals,
            compound_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        ) {
            Some(t) => t,
            None => return false,
        };
        if actual != *want {
            note(reject_reason, || {
                format!(
                    "tuple literal element type {actual:?} does not match expected {want:?}"
                )
            });
            return false;
        }
    }
    true
}

/// If `value_ref` is a `TupleLiteral`, derive its element types by
/// type-checking each child expression. Returns the shape only when all
/// elements are JIT scalars.
#[allow(clippy::too_many_arguments)]
fn tuple_literal_target(
    program: &Program,
    value_ref: &ExprRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<Vec<ScalarTy>> {
    let expr = program.expression.get(value_ref)?;
    let elems = match expr {
        Expr::TupleLiteral(es) => es,
        _ => return None,
    };
    if elems.len() < 2 {
        return None;
    }
    let mut shape: Vec<ScalarTy> = Vec::with_capacity(elems.len());
    for e in &elems {
        let t = check_expr(
            program,
            e,
            locals,
            compound_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        )?;
        if t == ScalarTy::Unit {
            note(reject_reason, || {
                "tuple element of Unit type is not supported in JIT".to_string()
            });
            return None;
        }
        shape.push(t);
    }
    Some(shape)
}

/// If `value_ref` is a `Call(callee, args)` whose callee returns a
/// scalar tuple, validate args and record the call site, returning the
/// tuple's element-type vector for the caller to register as a tuple
/// local.
#[allow(clippy::too_many_arguments)]
fn check_tuple_returning_call(
    program: &Program,
    value_ref: &ExprRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<Vec<ScalarTy>> {
    let expr = program.expression.get(value_ref)?;
    let callee_name = match expr {
        Expr::Call(n, _) => n,
        _ => return None,
    };
    let callee = program
        .function
        .iter()
        .find(|f| f.name == callee_name)
        .cloned()?;
    // Only proceed when the callee's return is a tuple of scalars.
    let element_tys = match &callee.return_type {
        Some(td) => match resolve_param_ty(td, substitutions, struct_layouts) {
            Some(ParamTy::Tuple(t)) => t,
            _ => return None,
        },
        None => return None,
    };
    // Reuse the regular Call analysis for argument validation and
    // monomorph recording.
    let saved_callees_len = callees.len();
    let _ = check_expr(
        program,
        value_ref,
        locals,
        compound_locals,
        substitutions,
        struct_layouts,
        callees,
        ptr_read_hints,
        reject_reason,
    );
    if callees.len() == saved_callees_len {
        return None;
    }
    Some(element_tys)
}

/// If the value-position expression is a `StructLiteral` whose struct
/// name has a registered scalar layout, return that struct name. Used to
/// special-case `val p = Point { … }` / `var p = Point { … }`.
fn struct_literal_target(
    program: &Program,
    value_ref: &ExprRef,
    type_decl: Option<&TypeDecl>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    reject_reason: &mut Option<String>,
) -> Option<DefaultSymbol> {
    let expr = program.expression.get(value_ref)?;
    let lit_name = match expr {
        Expr::StructLiteral(name, _) => name,
        _ => return None,
    };
    if !struct_layouts.contains_key(&lit_name) {
        // Distinguish generic struct (todo #159 / T6) from
        // other reject reasons (non-scalar field, etc.) so the
        // diagnostic points users at the right todo entry.
        let is_generic = (0..program.statement.len()).any(|i| {
            if let Some(Stmt::StructDecl { name, generic_params, .. }) =
                program.statement.get(&StmtRef(i as u32))
            {
                name == lit_name && !generic_params.is_empty()
            } else {
                false
            }
        });
        note(reject_reason, || {
            if is_generic {
                "struct literal references a generic struct (JIT does not yet \
                 model generic struct values; see #159)".to_string()
            } else {
                "struct literal references a struct without a JIT-eligible scalar layout".to_string()
            }
        });
        return None;
    }
    // If a type annotation is present, it must agree with the literal's
    // struct name. Unknown is the parser's placeholder for "no annotation"
    // (the type checker leaves it in place for many shapes), so accept it
    // as if it weren't there. Generic struct annotations (`Point<T>`) and
    // unrelated names are rejected.
    if let Some(td) = type_decl {
        match td {
            // The parser leaves Unknown when the user writes
            // `var p = Point { … }` without an annotation, so accept it.
            TypeDecl::Unknown => {}
            TypeDecl::Identifier(s) | TypeDecl::Struct(s, _) if *s == lit_name => {}
            _ => {
                note(reject_reason, || {
                    "struct literal type annotation does not match literal name".to_string()
                });
                return None;
            }
        }
    }
    Some(lit_name)
}

/// If `value_ref` is a `Call(callee, args)` whose callee returns a known
/// struct type, validate each argument against the callee's parameters
/// (Identifier-of-struct-local for struct params; ScalarTy for scalar
/// params), record the monomorphization, and return the resulting
/// struct's name. Caller registers the struct local.
#[allow(clippy::too_many_arguments)]
fn check_struct_returning_call(
    program: &Program,
    value_ref: &ExprRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<DefaultSymbol> {
    let expr = program.expression.get(value_ref)?;
    let (callee_name, args_ref) = match expr {
        Expr::Call(n, a) => (n, a),
        _ => return None,
    };
    let callee = program
        .function
        .iter()
        .find(|f| f.name == callee_name)
        .cloned()?;
    // Only proceed when the callee returns a known struct.
    let ret_struct = match &callee.return_type {
        Some(td) => match td {
            TypeDecl::Identifier(s) | TypeDecl::Struct(s, _)
                if struct_layouts.contains_key(s) =>
            {
                *s
            }
            _ => return None,
        },
        None => return None,
    };
    // Reuse the regular Call analysis by delegating to check_expr; it
    // populates callees/call_targets and validates arguments. The
    // expected return type from check_expr will be None (struct returns
    // aren't representable as ScalarTy), but the side effects we need
    // already happened.
    //
    // We re-run the call's argument validation manually here so the
    // overall eligibility analysis stays in sync. The check_expr's
    // existing Call branch handles struct-typed parameters, generic
    // inference, and call_targets registration.
    let saved_callees_len = callees.len();
    let result = check_expr(
        program,
        value_ref,
        locals,
        compound_locals,
        substitutions,
        struct_layouts,
        callees,
        ptr_read_hints,
        reject_reason,
    );
    // For struct-returning calls, check_expr returns None (since its
    // result type isn't a ScalarTy). That's fine — we only care that
    // the side-effects (call recording, argument validation) succeeded.
    // If check_expr failed before recording the call, treat that as a
    // genuine eligibility failure; otherwise propagate the struct
    // return type.
    if result.is_none() && callees.len() == saved_callees_len {
        return None;
    }
    let _ = args_ref; // suppress unused warning
    Some(ret_struct)
}

/// Phase JE-2d: detect an enum-returning call as a val/var rhs.
/// Mirrors `check_struct_returning_call` / `check_tuple_returning_call`
/// — recurse into `check_expr` so the call's args are validated and
/// `callees` is populated, then return the enum-type-name when the
/// call's return is `ParamTy::Enum`.
#[allow(clippy::too_many_arguments)]
fn check_enum_returning_call(
    program: &Program,
    value_ref: &ExprRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<EnumLocalInfo> {
    let expr = program.expression.get(value_ref)?;
    let callee_name = match expr {
        Expr::Call(n, _) => n,
        _ => return None,
    };
    let callee = program
        .function
        .iter()
        .find(|f| f.name == callee_name)
        .cloned()?;
    // Phase JE-5: accept generic enum returns. Resolve the
    // monomorph payload_ty from the return TypeDecl's args via
    // `payload_ty_from_annotation`.
    // Note the parser-ambiguous `Struct(name, args)` form for
    // user-named types.
    let (ret_enum, ret_payload_ty) = match &callee.return_type {
        Some(td) => match td {
            TypeDecl::Identifier(s) if enum_layout_for(*s).is_some() => {
                let layout = enum_layout_for(*s).unwrap();
                (*s, layout.payload_ty())
            }
            TypeDecl::Enum(s, _) | TypeDecl::Struct(s, _)
                if enum_layout_for(*s).is_some() =>
            {
                let layout = enum_layout_for(*s).unwrap();
                // Synthesize a TypeDecl::Enum form so
                // payload_ty_from_annotation resolves correctly
                // regardless of which variant the parser emitted.
                let synthetic_td = match td {
                    TypeDecl::Enum(_, args) => TypeDecl::Enum(*s, args.clone()),
                    TypeDecl::Struct(_, args) => TypeDecl::Enum(*s, args.clone()),
                    _ => unreachable!(),
                };
                let args_empty = matches!(
                    td,
                    TypeDecl::Enum(_, a) | TypeDecl::Struct(_, a) if a.is_empty()
                );
                let pty = if args_empty {
                    layout.payload_ty()
                } else {
                    payload_ty_from_annotation(&synthetic_td, &layout)
                };
                // For payload-bearing enums the resolution must
                // produce a scalar — otherwise the boundary is
                // undefined.
                if layout.variant_payloads.iter().any(|v| v.is_some())
                    && pty.is_none()
                {
                    return None;
                }
                (*s, pty)
            }
            _ => return None,
        },
        None => return None,
    };
    let saved_callees_len = callees.len();
    let result = check_expr(
        program, value_ref, locals, compound_locals,
        substitutions, struct_layouts, callees, ptr_read_hints, reject_reason,
    );
    if result.is_none() && callees.len() == saved_callees_len {
        return None;
    }
    Some(EnumLocalInfo::new(ret_enum, ret_payload_ty))
}

/// Phase JE-2b/JE-3: detect an enum constructor RHS and validate
/// the payload. Returns `Some(EnumLocalInfo)` when `value_ref` is
/// one of:
///   - `Expr::QualifiedIdentifier([enum, variant])` — unit constructor
///   - `Expr::AssociatedFunctionCall(enum, variant, args)` — tuple
///     constructor; the single arg's type must match the enum's
///     payload_repr (Concrete or Generic resolved per call-site).
/// Returns `None` for everything else (the regular check_expr path
/// runs). Side effect: validates the payload arg via `check_expr`,
/// which recursively records callees / ptr_read hints.
///
/// `annotation_hint` is the val/var annotation if available (the
/// declared enum type with type args). Used when the rhs is a unit
/// constructor of a generic enum (e.g. `val o: Option<i64> = Option::None`)
/// — the payload_ty has to come from somewhere.
#[allow(clippy::too_many_arguments)]
fn check_enum_constructor_rhs(
    program: &Program,
    value_ref: &ExprRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
    annotation_hint: Option<&TypeDecl>,
) -> Option<EnumLocalInfo> {
    let expr = program.expression.get(value_ref)?;
    match expr {
        Expr::QualifiedIdentifier(path) if path.len() == 2 => {
            let enum_name = path[0];
            let variant = path[1];
            let layout = enum_layout_for(enum_name)?;
            // Variant must exist and be a unit variant.
            let idx = layout.variants.iter().position(|n| *n == variant)?;
            if layout.variant_has_payload[idx] {
                // Caller wrote `Status::Ok` (no parens) but the
                // variant carries a payload — reject so the regular
                // path can produce the right error.
                return None;
            }
            // Phase JE-3/JE-4: payload_ty resolution depends on
            // whether the enum has any tuple variant. Unit-only
            // enums report `None`. Otherwise the per-monomorph
            // uniform payload comes from the val/var annotation
            // (via `resolve_uniform_payload`). For non-generic
            // enums the annotation is unnecessary because
            // `resolve_uniform_payload(empty subst)` already
            // produces the right answer.
            let payload_ty = if !layout.variant_payloads.iter().any(|v| v.is_some()) {
                None
            } else if let Some(t) = layout.resolve_uniform_payload(&HashMap::new()) {
                Some(t)
            } else {
                match annotation_hint.and_then(|td| payload_ty_from_annotation(td, &layout)) {
                    Some(t) => Some(t),
                    None => {
                        note(reject_reason, || {
                            "JIT generic enum unit constructor needs an annotation \
                             with concrete type args (e.g. `val o: Option<i64> = Option::None`)"
                                .to_string()
                        });
                        return None;
                    }
                }
            };
            Some(EnumLocalInfo::new(enum_name, payload_ty))
        }
        Expr::AssociatedFunctionCall(enum_name, variant, args) => {
            let layout = enum_layout_for(enum_name)?;
            let idx = layout.variants.iter().position(|n| *n == variant)?;
            if !layout.variant_has_payload[idx] {
                // `Status::Bad(5i64)` — variant doesn't take a payload.
                return None;
            }
            if args.len() != 1 {
                note(reject_reason, || {
                    "JIT enum tuple constructor: only single-payload \
                     variants are supported (JE-2a scope; see JIT-enum-1)".to_string()
                });
                return None;
            }
            let arg_ty = check_expr(
                program, &args[0], locals, compound_locals, substitutions, struct_layouts, callees,
                ptr_read_hints, reject_reason,
            )?;
            // Phase JE-3/JE-4: variant payload comes from
            // `variant_payloads[idx]`. For Concrete the arg type
            // must match; for Generic the arg type itself fixes
            // that variant's payload (and we cross-check against
            // the annotation when present so multi-generic enums
            // like `Result<T, E>` get the consistency check).
            let variant_repr = layout.variant_payloads[idx].clone()?;
            let payload_ty = match variant_repr {
                PayloadRepr::Concrete(declared) => {
                    if arg_ty != declared {
                        note(reject_reason, || {
                            format!(
                                "JIT enum tuple constructor: payload type {arg_ty:?} does not \
                                 match declared {declared:?}"
                            )
                        });
                        return None;
                    }
                    arg_ty
                }
                PayloadRepr::Generic(_) => {
                    if let Some(td) = annotation_hint {
                        if let Some(t) = payload_ty_from_annotation(td, &layout) {
                            if t != arg_ty {
                                note(reject_reason, || {
                                    format!(
                                        "JIT generic enum constructor: arg type {arg_ty:?} does \
                                         not match annotation type {t:?}"
                                    )
                                });
                                return None;
                            }
                        }
                    }
                    arg_ty
                }
                PayloadRepr::None => return None,
            };
            Some(EnumLocalInfo::new(enum_name, Some(payload_ty)))
        }
        _ => None,
    }
}

/// Validate every field of a struct literal against the registered
/// layout. Records callees / ptr_read hints encountered while typing the
/// individual field initializers.
#[allow(clippy::too_many_arguments)]
fn check_struct_literal_fields(
    program: &Program,
    value_ref: &ExprRef,
    struct_name: DefaultSymbol,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let layout = match struct_layouts.get(&struct_name) {
        Some(l) => l.clone(),
        None => {
            // Distinguish generic vs other reasons so the
            // diagnostic points users at the right todo entry.
            // `collect_struct_layouts` skips structs whose
            // `generic_params` is non-empty (no per-monomorph
            // layout yet — todo #159 / T6); a missing layout
            // for a generic struct should say so.
            let is_generic = (0..program.statement.len()).any(|i| {
                if let Some(Stmt::StructDecl { name, generic_params, .. }) =
                    program.statement.get(&StmtRef(i as u32))
                {
                    name == struct_name && !generic_params.is_empty()
                } else {
                    false
                }
            });
            note(reject_reason, || {
                if is_generic {
                    "JIT does not yet model generic struct values \
                     (would need per-monomorph struct_layouts; see #159)"
                        .to_string()
                } else {
                    "struct layout missing in JIT analysis".to_string()
                }
            });
            return false;
        }
    };
    let expr = match program.expression.get(value_ref) {
        Some(e) => e,
        None => return false,
    };
    let lit_fields = match expr {
        Expr::StructLiteral(_, fields) => fields,
        _ => return false,
    };
    if lit_fields.len() != layout.fields.len() {
        note(reject_reason, || {
            format!(
                "struct literal has {} field(s), layout expects {}",
                lit_fields.len(),
                layout.fields.len()
            )
        });
        return false;
    }
    for (field_sym, field_expr) in &lit_fields {
        let want = match layout.field(*field_sym) {
            Some(t) => t,
            None => {
                note(reject_reason, || "unknown field in struct literal".to_string());
                return false;
            }
        };
        let actual = match check_expr(
            program,
            field_expr,
            locals,
            compound_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        ) {
            Some(t) => t,
            None => return false,
        };
        if actual != want {
            note(reject_reason, || {
                format!("struct literal field type {actual:?} does not match layout {want:?}")
            });
            return false;
        }
    }
    true
}

/// Quick syntactic walk to detect any PtrRead within a function body.
fn body_has_ptr_read(program: &Program, stmt_ref: &StmtRef) -> bool {
    let mut found = false;
    walk_stmt_for_ptr_read(program, stmt_ref, &mut found);
    found
}

/// Phase JE-1b: validate that a `Pattern` is supported by the JIT
/// match codegen for the given scrutinee scalar type. Patterns
/// reduce to one of:
///   - `Wildcard` — accepted for any scrutinee type
///   - `Literal(ExprRef)` — accepted when the literal's scalar type
///     matches the scrutinee
///   - `EnumVariant(enum, variant, [])` — accepted when the enum is
///     in `enum_layouts`, the variant is unit, and the scrutinee is
///     a U64 tag (the JIT representation of unit-only enums)
/// Tuple patterns and named bindings (which require payload
/// extraction) are rejected.
/// Returns `Ok(payload_binding)` when the pattern is accepted.
/// `payload_binding` is `Some((name, ty))` when an EnumVariant
/// pattern carries a single Pattern::Name sub-pattern (JE-2b
/// payload binding); otherwise `None`. Caller installs the
/// binding in `locals` for the arm body and removes it after.
///
/// `scrut_enum` carries the per-local enum info when the
/// scrutinee is an enum identifier — the per-local `payload_ty`
/// determines what type the pattern's Name binds to (important
/// for generic enums where the layout's payload_repr is `Generic`).
fn check_match_pattern(
    program: &Program,
    pat: &Pattern,
    scrut_ty: ScalarTy,
    scrut_enum: Option<EnumLocalInfo>,
    reject_reason: &mut Option<String>,
) -> Option<Option<(DefaultSymbol, ScalarTy)>> {
    match pat {
        Pattern::Wildcard => Some(None),
        Pattern::Literal(eref) => {
            let lit_ty = match program.expression.get(eref) {
                Some(Expr::Int64(_)) => ScalarTy::I64,
                Some(Expr::UInt64(_)) => ScalarTy::U64,
                Some(Expr::True) | Some(Expr::False) => ScalarTy::Bool,
                _ => {
                    note(reject_reason, || {
                        "JIT match: unsupported literal pattern shape".to_string()
                    });
                    return None;
                }
            };
            if lit_ty != scrut_ty {
                note(reject_reason, || {
                    format!(
                        "JIT match: literal pattern type {lit_ty:?} does not match scrutinee {scrut_ty:?}"
                    )
                });
                return None;
            }
            Some(None)
        }
        Pattern::EnumVariant(enum_sym, variant_sym, sub_pats) => {
            if scrut_ty != ScalarTy::U64 {
                note(reject_reason, || {
                    format!(
                        "JIT match: enum variant pattern but scrutinee is {scrut_ty:?}, expected U64 tag"
                    )
                });
                return None;
            }
            let layout = match enum_layout_for(*enum_sym) {
                Some(l) => l,
                None => {
                    note(reject_reason, || {
                        "JIT match: enum is not JIT-eligible (generic / mixed payloads; \
                     see JIT-enum-1)".to_string()
                    });
                    return None;
                }
            };
            let idx = match layout.variants.iter().position(|n| *n == *variant_sym) {
                Some(i) => i,
                None => {
                    note(reject_reason, || {
                        "JIT match: variant not declared on this enum".to_string()
                    });
                    return None;
                }
            };
            // Phase JE-2b: variant with payload — accept Pattern::Name
            // for binding the payload value, or empty sub_pats when
            // the user wrote `Status::Ok =>` (which would still match
            // semantically; treat as no binding).
            if layout.variant_has_payload[idx] {
                // Phase JE-3: prefer the scrutinee's per-local
                // payload_ty (which knows the resolved generic
                // monomorph) over the layout's. Fall back to layout
                // payload_ty for non-generic enums when scrut_enum
                // is missing (legacy callers).
                let payload_ty = scrut_enum
                    .and_then(|e| e.payload_ty)
                    .or_else(|| layout.payload_ty())?;
                match sub_pats.as_slice() {
                    [] => Some(None),
                    [single] => match single {
                        Pattern::Name(payload_name) => {
                            Some(Some((*payload_name, payload_ty)))
                        }
                        Pattern::Wildcard => Some(None),
                        _ => {
                            note(reject_reason, || {
                                "JIT match: only Name / Wildcard sub-pattern \
                                 supported for tuple-variant payload (JE-2b)".to_string()
                            });
                            None
                        }
                    },
                    _ => {
                        note(reject_reason, || {
                            "JIT match: multi-payload tuple variants not supported \
                             (JE-2a scope)".to_string()
                        });
                        None
                    }
                }
            } else {
                if !sub_pats.is_empty() {
                    note(reject_reason, || {
                        "JIT match: unit variant cannot bind payload".to_string()
                    });
                    return None;
                }
                Some(None)
            }
        }
        Pattern::Tuple(_) | Pattern::Name(_) => {
            note(reject_reason, || {
                "JIT match: tuple / top-level name patterns not yet supported".to_string()
            });
            None
        }
    }
}

/// Returns true if `name` matches any top-level `enum` declaration
/// in the program. Used by the AssociatedFunctionCall reject path so
/// it can distinguish enum constructors (`Option::Some(...)`) from
/// other unsupported associated calls and report a precise reason.
fn enum_decl_lookup_by_name(
    program: &Program,
    name: DefaultSymbol,
) -> Option<()> {
    for i in 0..program.statement.len() {
        if let Some(Stmt::EnumDecl { name: n, .. }) = program.statement.get(&StmtRef(i as u32)) {
            if n == name {
                return Some(());
            }
        }
    }
    None
}

fn walk_stmt_for_ptr_read(program: &Program, stmt_ref: &StmtRef, found: &mut bool) {
    if *found {
        return;
    }
    let Some(stmt) = program.statement.get(stmt_ref) else {
        return;
    };
    match stmt {
        Stmt::Expression(e) => walk_expr_for_ptr_read(program, &e, found),
        Stmt::Val(_, _, e) => walk_expr_for_ptr_read(program, &e, found),
        Stmt::Var(_, _, Some(e)) => walk_expr_for_ptr_read(program, &e, found),
        Stmt::Return(Some(e)) => walk_expr_for_ptr_read(program, &e, found),
        Stmt::For(_, _, s, e, body) => {
            walk_expr_for_ptr_read(program, &s, found);
            walk_expr_for_ptr_read(program, &e, found);
            walk_expr_for_ptr_read(program, &body, found);
        }
        Stmt::While(_, c, body) => {
            walk_expr_for_ptr_read(program, &c, found);
            walk_expr_for_ptr_read(program, &body, found);
        }
        _ => {}
    }
}

fn walk_expr_for_ptr_read(program: &Program, expr_ref: &ExprRef, found: &mut bool) {
    if *found {
        return;
    }
    let Some(expr) = program.expression.get(expr_ref) else {
        return;
    };
    match expr {
        Expr::BuiltinCall(BuiltinFunction::PtrRead, _) => *found = true,
        Expr::Block(stmts) => {
            for s in &stmts {
                walk_stmt_for_ptr_read(program, s, found);
            }
        }
        Expr::Binary(_, l, r) | Expr::Assign(l, r) | Expr::Range(l, r) => {
            walk_expr_for_ptr_read(program, &l, found);
            walk_expr_for_ptr_read(program, &r, found);
        }
        Expr::Unary(_, e) | Expr::Cast(e, _) => {
            walk_expr_for_ptr_read(program, &e, found);
        }
        Expr::IfElifElse(c, t, elifs, el) => {
            walk_expr_for_ptr_read(program, &c, found);
            walk_expr_for_ptr_read(program, &t, found);
            for (ec, eb) in &elifs {
                walk_expr_for_ptr_read(program, ec, found);
                walk_expr_for_ptr_read(program, eb, found);
            }
            walk_expr_for_ptr_read(program, &el, found);
        }
        Expr::Call(_, args) => walk_expr_for_ptr_read(program, &args, found),
        Expr::ExprList(es) | Expr::ArrayLiteral(es) | Expr::TupleLiteral(es) => {
            for e in &es {
                walk_expr_for_ptr_read(program, e, found);
            }
        }
        Expr::BuiltinCall(_, args) => {
            for a in &args {
                walk_expr_for_ptr_read(program, a, found);
            }
        }
        _ => {}
    }
}

fn check_stmt(
    program: &Program,
    stmt_ref: &StmtRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let stmt = match program.statement.get(stmt_ref) {
        Some(s) => s,
        None => return false,
    };
    match stmt {
        Stmt::Expression(e) => {
            check_expr(program, &e, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason).is_some()
        }
        Stmt::Val(name, type_decl, value) => {
            // Special-case: a struct-literal RHS registers `name` as a
            // struct local. Field-by-field types are validated against the
            // struct's known layout; everything else falls through to the
            // scalar path.
            if let Some(struct_name) =
                struct_literal_target(program, &value, type_decl.as_ref(), struct_layouts, reject_reason)
            {
                if !check_struct_literal_fields(
                    program,
                    &value,
                    struct_name,
                    locals,
                    compound_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                ) {
                    return false;
                }
                compound_locals.structs.insert(name, struct_name);
                return true;
            }
            // Special-case: a struct-returning function call also lands as
            // a fresh struct local. Validate the call site (and its args)
            // through the normal Call eligibility path.
            if let Some(struct_name) = check_struct_returning_call(
                program,
                &value,
                locals,
                compound_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            ) {
                compound_locals.structs.insert(name, struct_name);
                return true;
            }
            // Tuple literal RHS — `val pair = (1i64, 2u64)` — registers
            // `name` as a tuple local with the inferred element shape.
            if let Some(shape) = tuple_literal_target(
                program,
                &value,
                locals,
                compound_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            ) {
                compound_locals.tuples.insert(name, shape);
                return true;
            }
            // Tuple-returning call — `val pair = make_pair()`.
            if let Some(shape) = check_tuple_returning_call(
                program,
                &value,
                locals,
                compound_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            ) {
                compound_locals.tuples.insert(name, shape);
                return true;
            }
            // Tuple alias — `val q = pair` where `pair` is already a
            // known tuple local.
            if let Some(Expr::Identifier(rhs_name)) = program.expression.get(&value) {
                if let Some(shape) = compound_locals.tuples.get(&rhs_name).cloned() {
                    compound_locals.tuples.insert(name, shape);
                    return true;
                }
            }
            // Phase JE-2b: enum constructor RHS registers `name` as
            // an enum local so subsequent match scrutinees can recover
            // both the variant tag and the payload (if any). Both
            // unit-variant (`Color::Red` / `Status::Bad`) and tuple-
            // variant (`Status::Ok(5i64)`) constructors land here when
            // the enum is in `enum_layouts`. Tuple-variant arg type
            // is validated against `EnumLayout::payload_ty`.
            //
            // Phase JE-3: pass the val/var annotation as a hint so
            // generic-enum unit constructors (`Option::None`) can
            // resolve T from the annotation.
            if let Some(info) = check_enum_constructor_rhs(
                program, &value, locals, compound_locals, substitutions, struct_layouts, callees,
                ptr_read_hints, reject_reason, type_decl.as_ref(),
            ) {
                compound_locals.enums.insert(name, info);
                return true;
            }
            // Enum alias — `val n: Box = b` where `b` is already a
            // known enum local of the same type.
            if let Some(Expr::Identifier(rhs_name)) = program.expression.get(&value) {
                if let Some(info) = compound_locals.enums.get(&rhs_name).copied() {
                    compound_locals.enums.insert(name, info);
                    return true;
                }
            }
            // Phase JE-2d: enum-returning call as val/var rhs.
            // Mirrors `check_struct_returning_call` /
            // `check_tuple_returning_call` — runs the call's
            // arg-validation through check_expr (which records
            // callees) and registers the enum local.
            if let Some(info) = check_enum_returning_call(
                program, &value, locals, compound_locals, substitutions, struct_layouts, callees,
                ptr_read_hints, reject_reason,
            ) {
                compound_locals.enums.insert(name, info);
                return true;
            }
            let declared_hint = type_decl.as_ref().and_then(ScalarTy::from_type_decl);
            // If both the annotation and the RHS are PtrRead-shaped, record
            // the expected return type before recursing so check_expr can
            // accept the otherwise type-polymorphic builtin.
            if let Some(t) = declared_hint {
                register_ptr_read_hint(program, &value, t, ptr_read_hints);
            }
            let val_ty = match check_expr(program, &value, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            let declared = match type_decl {
                // The parser leaves Unknown when the user wrote no
                // annotation; treat it as "infer from rhs".
                Some(TypeDecl::Unknown) | None => val_ty,
                Some(td) => match ScalarTy::from_type_decl(&td) {
                    Some(t) => t,
                    None => match scalar_ty_for_enum_decl(&td) {
                        // Phase JE-1b: a JIT-eligible enum
                        // annotation (`val c: Color = ...`)
                        // resolves to ScalarTy::U64 because the
                        // enum's representation at the JIT layer
                        // is just its tag.
                        Some(t) => t,
                        None => return false,
                    },
                },
            };
            if declared != val_ty {
                return false;
            }
            // Reject Unit and Never RHS: there is no value to bind, and
            // recording `Never` in `locals` would poison subsequent
            // expressions that read `name`. `val x = panic(...)` is the
            // typical Never case — silent-fallback is fine since the
            // expression does the same observable thing in the
            // interpreter.
            if matches!(declared, ScalarTy::Unit | ScalarTy::Never) {
                return false;
            }
            locals.insert(name, declared);
            true
        }
        Stmt::Var(name, type_decl, value) => {
            // Mirror the Val struct-literal special case — `var p = Point { ... }`
            // also registers a struct local.
            if let Some(v) = value {
                if let Some(struct_name) =
                    struct_literal_target(program, &v, type_decl.as_ref(), struct_layouts, reject_reason)
                {
                    if !check_struct_literal_fields(
                        program,
                        &v,
                        struct_name,
                        locals,
                        compound_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    ) {
                        return false;
                    }
                    compound_locals.structs.insert(name, struct_name);
                    return true;
                }
                if let Some(struct_name) = check_struct_returning_call(
                    program,
                    &v,
                    locals,
                    compound_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                ) {
                    compound_locals.structs.insert(name, struct_name);
                    return true;
                }
                if let Some(shape) = tuple_literal_target(
                    program,
                    &v,
                    locals,
                    compound_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                ) {
                    compound_locals.tuples.insert(name, shape);
                    return true;
                }
                if let Some(shape) = check_tuple_returning_call(
                    program,
                    &v,
                    locals,
                    compound_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                ) {
                    compound_locals.tuples.insert(name, shape);
                    return true;
                }
                if let Some(Expr::Identifier(rhs_name)) = program.expression.get(&v) {
                    if let Some(shape) = compound_locals.tuples.get(&rhs_name).cloned() {
                        compound_locals.tuples.insert(name, shape);
                        return true;
                    }
                }
            }
            let declared = match (type_decl.as_ref(), value) {
                // Treat `Some(Unknown)` like `None` — the parser inserts
                // it when the user wrote no annotation.
                (Some(TypeDecl::Unknown), Some(v)) | (None, Some(v)) => {
                    match check_expr(program, &v, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                        Some(t) => t,
                        None => return false,
                    }
                }
                (Some(td), _) => match ScalarTy::from_type_decl(td) {
                    Some(t) => t,
                    None => return false,
                },
                (None, None) => return false,
            };
            if let Some(v) = value {
                if type_decl.is_some() {
                    register_ptr_read_hint(program, &v, declared, ptr_read_hints);
                }
                let val_ty = match check_expr(program, &v, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                    Some(t) => t,
                    None => return false,
                };
                if val_ty != declared {
                    return false;
                }
            }
            if matches!(declared, ScalarTy::Unit | ScalarTy::Never) {
                return false;
            }
            locals.insert(name, declared);
            true
        }
        Stmt::Return(value) => {
            if let Some(v) = value {
                check_expr(program, &v, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason).is_some()
            } else {
                true
            }
        }
        Stmt::Break(_) | Stmt::Continue(_) => true,
        Stmt::For(_label, var, start, end, block) => {
            let start_ty = match check_expr(program, &start, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            let end_ty = match check_expr(program, &end, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            if start_ty != end_ty {
                return false;
            }
            if !matches!(start_ty, ScalarTy::I64 | ScalarTy::U64) {
                return false;
            }
            let prev = locals.insert(var, start_ty);
            let body_ok =
                check_expr(program, &block, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason).is_some();
            match prev {
                Some(t) => {
                    locals.insert(var, t);
                }
                None => {
                    locals.remove(&var);
                }
            }
            body_ok
        }
        Stmt::While(_label, cond, block) => {
            let cond_ty = match check_expr(program, &cond, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            if cond_ty != ScalarTy::Bool {
                return false;
            }
            check_expr(program, &block, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason).is_some()
        }
        // No struct / impl / enum declarations are tolerated inside an
        // eligible function body. Top-level decls live outside of any
        // function so they don't affect us here.
        Stmt::StructDecl { .. } | Stmt::ImplBlock { .. } | Stmt::EnumDecl { .. } | Stmt::TraitDecl { .. } => false,
        // Type aliases are resolved at parse time; their presence inside
        // a function body (which the parser doesn't actually allow) is
        // a no-op and would not disqualify the body either way.
        Stmt::TypeAlias { .. } => true,
    }
}

/// If `value_ref` is a direct `__builtin_ptr_read(...)` call, register
/// `expected` as the read's return type so check_expr can accept it. The
/// JIT only supports PtrRead in positions where the expected type is
/// statically known (val/var with annotation, assignment to a typed
/// identifier).
fn register_ptr_read_hint(
    program: &Program,
    value_ref: &ExprRef,
    expected: ScalarTy,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
) {
    if let Some(Expr::BuiltinCall(BuiltinFunction::PtrRead, _)) =
        program.expression.get(value_ref)
    {
        ptr_read_hints.insert(*value_ref, expected);
    }
}

/// Returns the type produced by the expression, or `None` if the expression
/// uses an unsupported construct. As a side effect, populates `callees` with
/// names of user-defined functions invoked by this expression and
/// `ptr_read_hints` with PtrRead expected return types where statically
/// derivable from context.
pub(crate) fn check_expr(
    program: &Program,
    expr_ref: &ExprRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<ScalarTy> {
    let expr = program.expression.get(expr_ref)?;
    match expr {
        Expr::Int64(_) => Some(ScalarTy::I64),
        Expr::UInt64(_) => Some(ScalarTy::U64),
        // NUM-W narrow integer literals.
        Expr::Int8(_) => Some(ScalarTy::I8),
        Expr::Int16(_) => Some(ScalarTy::I16),
        Expr::Int32(_) => Some(ScalarTy::I32),
        Expr::UInt8(_) => Some(ScalarTy::U8),
        Expr::UInt16(_) => Some(ScalarTy::U16),
        Expr::UInt32(_) => Some(ScalarTy::U32),
        Expr::Float64(_) => Some(ScalarTy::F64),
        Expr::True | Expr::False => Some(ScalarTy::Bool),
        // STR-INTERP-INTERP-JIT: a string literal in an expression
        // position lowers to a `jit_string_literal(sym_id)` call
        // that materialises a heap str. The literal flows through
        // the JIT as a `ScalarTy::Str` value (i64 pointer).
        Expr::String(_) => Some(ScalarTy::Str),
        Expr::Identifier(sym) => {
            // Phase JE-2b: enum-typed locals report their tag type
            // (U64) so they participate in match scrutinees and bare
            // value-position uses transparently. Codegen distinguishes
            // them via `enum_locals`; eligibility downstream just sees
            // a U64.
            if let Some(_) = locals.get(&sym).copied() {
                return locals.get(&sym).copied();
            }
            if compound_locals.enums.contains_key(&sym) {
                return Some(ScalarTy::U64);
            }
            None
        }
        // Phase JE-1b: unit-variant constructor (`Color::Red`).
        // Reduce to `ScalarTy::U64` so the rest of eligibility +
        // codegen treats the value as just the tag — that's the
        // entire representation for unit variants. Generic enums
        // and enums with payload variants miss the layout map and
        // fall through to the catch-all reject.
        Expr::QualifiedIdentifier(path)
            if path.len() == 2
                && enum_layout_for(path[0])
                    .and_then(|l| l.variant_tag(path[1]))
                    .is_some() =>
        {
            Some(ScalarTy::U64)
        }
        // Phase JE-1b: `match scrutinee { ... }` for scalar / unit-
        // enum-tag scrutinees. Each arm's body must produce a
        // value-typed result; all arms must agree on the result
        // type. Variant patterns over a JIT-eligible enum reduce
        // to a u64 tag comparison, just like the constructor
        // returns the tag.
        Expr::Match(scrutinee, arms) => {
            let scrut_ty = check_expr(
                program, &scrutinee, locals, compound_locals,
                substitutions, struct_layouts, callees, ptr_read_hints,
                reject_reason,
            )?;
            // Phase JE-3: peek the scrutinee for its enum-local info
            // so payload-binding patterns can use the per-local
            // payload_ty (resolved monomorph) instead of the layout's
            // generic placeholder.
            let scrut_enum_info = match program.expression.get(&scrutinee) {
                Some(Expr::Identifier(s)) => compound_locals.enums.get(&s).copied(),
                _ => None,
            };
            // All arms unify to a single type. Walk each arm's
            // pattern (rejecting unsupported shapes) and body.
            let mut result_ty: Option<ScalarTy> = None;
            for arm in &arms {
                let payload_binding = check_match_pattern(
                    program, &arm.pattern, scrut_ty, scrut_enum_info, reject_reason,
                )?;
                if arm.guard.is_some() {
                    note(reject_reason, || {
                        "JIT match arm guards are not yet supported".to_string()
                    });
                    return None;
                }
                // Phase JE-2b: install the payload binding (if any)
                // for the duration of arm body checking, then remove
                // it so subsequent arms / siblings don't see it.
                let prev_local = if let Some((name, ty)) = payload_binding {
                    Some((name, locals.insert(name, ty)))
                } else {
                    None
                };
                let body_ty = check_expr(
                    program, &arm.body, locals, compound_locals,
                    substitutions, struct_layouts, callees, ptr_read_hints,
                    reject_reason,
                )?;
                if let Some((name, prior)) = prev_local {
                    match prior {
                        Some(t) => { locals.insert(name, t); }
                        None => { locals.remove(&name); }
                    }
                }
                match result_ty {
                    None => result_ty = Some(body_ty),
                    Some(prev) if prev == body_ty => {}
                    Some(prev) => {
                        note(reject_reason, || {
                            format!(
                                "match arms disagree on result type: {prev:?} vs {body_ty:?}"
                            )
                        });
                        return None;
                    }
                }
            }
            result_ty
        }
        Expr::Binary(op, lhs, rhs) => {
            let lt = check_expr(program, &lhs, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            let rt = check_expr(program, &rhs, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            if lt != rt {
                return None;
            }
            // NUM-W: narrow integers (I8/I16/I32/U8/U16/U32) accept
            // the same operator set as their I64/U64 cousins. cranelift's
            // `iadd` / `isub` / `imul` / `udiv` / `sdiv` / `urem` /
            // `srem` are all width-polymorphic on operand type, and
            // `icmp` likewise picks the right width from the operands.
            let int_like = lt.is_narrow_int()
                || matches!(lt, ScalarTy::I64 | ScalarTy::U64);
            match op {
                Operator::IAdd | Operator::ISub | Operator::IMul | Operator::IDiv => {
                    if int_like || lt == ScalarTy::F64 {
                        Some(lt)
                    } else {
                        None
                    }
                }
                Operator::IMod => {
                    // Cranelift exposes srem/urem for ints but no native f64
                    // remainder; reject f64 mod here so codegen never sees it.
                    if int_like {
                        Some(lt)
                    } else {
                        None
                    }
                }
                Operator::EQ | Operator::NE => {
                    if lt == ScalarTy::Unit {
                        None
                    } else {
                        Some(ScalarTy::Bool)
                    }
                }
                Operator::LT | Operator::LE | Operator::GT | Operator::GE => {
                    if int_like || lt == ScalarTy::F64 {
                        Some(ScalarTy::Bool)
                    } else {
                        None
                    }
                }
                Operator::LogicalAnd | Operator::LogicalOr => {
                    if lt == ScalarTy::Bool {
                        Some(ScalarTy::Bool)
                    } else {
                        None
                    }
                }
                Operator::BitwiseAnd | Operator::BitwiseOr | Operator::BitwiseXor => {
                    if int_like || lt == ScalarTy::Bool {
                        Some(lt)
                    } else {
                        None
                    }
                }
                Operator::LeftShift | Operator::RightShift => {
                    if int_like {
                        Some(lt)
                    } else {
                        None
                    }
                }
            }
        }
        Expr::Unary(op, operand) => {
            let t = check_expr(program, &operand, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            match op {
                UnaryOp::BitwiseNot => {
                    if matches!(t, ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Bool) {
                        Some(t)
                    } else {
                        None
                    }
                }
                UnaryOp::LogicalNot => {
                    if t == ScalarTy::Bool {
                        Some(ScalarTy::Bool)
                    } else {
                        None
                    }
                }
                UnaryOp::Negate => {
                    // Negation of u64 is rejected at the type-check phase
                    // already. Allow i64 and f64 (cranelift `fneg`).
                    if matches!(t, ScalarTy::I64 | ScalarTy::F64) {
                        Some(t)
                    } else {
                        None
                    }
                }
                // REF-Stage-2: borrow ops are erased — eligibility
                // simply forwards the operand type, codegen emits
                // the operand value.
                UnaryOp::Borrow | UnaryOp::BorrowMut => Some(t),
            }
        }
        Expr::Block(stmts) => {
            let mut last_ty = ScalarTy::Unit;
            let mut snapshot = locals.clone();
            for s in &stmts {
                let stmt = program.statement.get(s)?;
                if let Stmt::Expression(e) = &stmt {
                    last_ty = check_expr(program, e, &mut snapshot, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                } else {
                    if !check_stmt(program, s, &mut snapshot, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    last_ty = ScalarTy::Unit;
                }
            }
            Some(last_ty)
        }
        Expr::IfElifElse(cond, if_block, elif_pairs, else_block) => {
            let ct = check_expr(program, &cond, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            if ct != ScalarTy::Bool {
                return None;
            }
            // Unify each branch's type via `ScalarTy::unify_branch`, which
            // treats `Never` (panic / divergence) as a wildcard — so
            // `if cond { panic("...") } else { 5i64 }` types as I64.
            let then_ty = check_expr(program, &if_block, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            let mut unified = then_ty;
            for (ec, eb) in &elif_pairs {
                let et = check_expr(program, ec, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                if et != ScalarTy::Bool {
                    return None;
                }
                let bt = check_expr(program, eb, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                unified = ScalarTy::unify_branch(unified, bt)?;
            }
            let else_ty = check_expr(program, &else_block, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            ScalarTy::unify_branch(unified, else_ty)
        }
        Expr::Assign(lhs, rhs) => {
            // Two assignment shapes are supported:
            //   1) `name = value` for a previously declared scalar local
            //   2) `name.field = value` for a struct local's field
            let lhs_expr = program.expression.get(&lhs)?;
            match lhs_expr {
                Expr::Identifier(name) => {
                    let lhs_ty = locals.get(&name).copied()?;
                    let rhs_ty = check_expr(
                        program,
                        &rhs,
                        locals,
                        compound_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    if rhs_ty != lhs_ty {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                Expr::FieldAccess(receiver, field_name) => {
                    let receiver_expr = program.expression.get(&receiver)?;
                    let recv_name = match receiver_expr {
                        Expr::Identifier(s) => s,
                        _ => {
                            note(reject_reason, || {
                                "field-assign receiver must be a struct local".to_string()
                            });
                            return None;
                        }
                    };
                    let struct_name = match compound_locals.structs.get(&recv_name).copied() {
                        Some(s) => s,
                        None => {
                            note(reject_reason, || {
                                "field-assign target is not a struct local".to_string()
                            });
                            return None;
                        }
                    };
                    let field_ty = struct_layouts
                        .get(&struct_name)
                        .and_then(|l| l.field(field_name))?;
                    let rhs_ty = check_expr(
                        program,
                        &rhs,
                        locals,
                        compound_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    if rhs_ty != field_ty {
                        note(reject_reason, || {
                            format!(
                                "field assign rhs type {rhs_ty:?} does not match field type {field_ty:?}"
                            )
                        });
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                _ => {
                    note(reject_reason, || {
                        "assignment target must be an identifier or struct field".to_string()
                    });
                    None
                }
            }
        }
        Expr::AssociatedFunctionCall(struct_name, function_name, args) => {
            // Phase JE-2b: tuple-variant enum constructor in expression
            // position (`Status::Ok(x + x)` as a match arm body). When
            // the qualifier names a JIT-eligible enum and the variant
            // exists, validate the payload type and report the value
            // as `ScalarTy::U64` (its tag, the simple scalar lens at
            // the eligibility layer). Codegen routes the actual
            // (tag, payload) lowering through `gather_enum_values` /
            // `lower_into_enum_return`.
            if let Some(layout) = enum_layout_for(struct_name) {
                if let Some(idx) = layout.variants.iter().position(|n| *n == function_name) {
                    if layout.variant_has_payload[idx] {
                        let payload_ty = layout.payload_ty()?;
                        if args.len() == 1 {
                            let arg_ty = check_expr(
                                program, &args[0], locals, compound_locals, substitutions, struct_layouts, callees,
                                ptr_read_hints, reject_reason,
                            )?;
                            if arg_ty == payload_ty {
                                return Some(ScalarTy::U64);
                            }
                        }
                    }
                }
            }
            // Module-qualified call (`math::add(args)`): when the
            // qualifier doesn't refer to a struct / enum but the
            // function name lives in the (post-import) flat function
            // table, treat it as a plain `Call(function_name, args)`
            // and reuse the same eligibility logic. Real associated
            // function calls (`Container::new(args)` with `Container`
            // a struct) keep the unsupported reject path because the
            // JIT doesn't lower instance methods yet.
            if struct_layouts.contains_key(&struct_name)
                || !program.function.iter().any(|f| f.name == function_name)
            {
                // Differentiate the common "enum constructor" case
                // (`Option::Some(...)`, `Result::Err(...)`, etc.)
                // from the generic struct-associated-function reject
                // so the verbose JIT log points at the actual blocker.
                // Enum values aren't represented in the JIT yet —
                // adding them would mean a `ParamTy::Enum`,
                // `enum_locals` map, tag-dispatch codegen, etc.
                // (essentially the AOT compiler's enum phases). The
                // interpreter handles the call correctly via
                // fallback; this just makes the reason precise.
                let is_enum_qualifier = enum_decl_lookup_by_name(program, struct_name).is_some();
                // Phase JE-1a: a JIT-eligible enum (non-generic,
                // unit-variant-only) shows up in `enum_layouts`. The
                // architecture for tag-based dispatch is in place
                // (EnumLayout + ENUM_LAYOUTS thread-local), but
                // constructor / match codegen hasn't landed yet —
                // use a precise "infrastructure ready, codegen
                // pending" message so a later JE-1b commit knows
                // which programs to enable.
                note(reject_reason, || {
                    if is_enum_qualifier {
                        if enum_layout_for(struct_name).is_some() {
                            "JIT enum support pending: unit-variant constructor codegen \
                             (Phase JE-1b will lower this via the existing tag layout)"
                                .to_string()
                        } else {
                            "JIT does not yet model enum values \
                             (constructors / match / methods; see JIT-enum-1)"
                                .to_string()
                        }
                    } else {
                        "uses unsupported expression associated function call".to_string()
                    }
                });
                return None;
            }
            return check_plain_call(
                program, expr_ref, function_name, &args, locals, compound_locals, substitutions, struct_layouts, callees,
                ptr_read_hints, reject_reason,
            );
        }
        Expr::Call(name, args_ref) => {
            let args_expr = program.expression.get(&args_ref)?;
            let arg_list = match args_expr {
                Expr::ExprList(v) => v,
                _ => return None,
            };

            check_plain_call(
                program, expr_ref, name, &arg_list, locals, compound_locals, substitutions, struct_layouts, callees,
                ptr_read_hints, reject_reason,
            )
        }
        Expr::BuiltinCall(func, args) => {
            // Type-check each argument against an expected ScalarTy.
            let check_args = |expected: &[ScalarTy],
                              args: &Vec<ExprRef>,
                              locals: &mut HashMap<DefaultSymbol, ScalarTy>,
                              compound_locals: &mut CompoundLocals,
                              callees: &mut Vec<MonoCall>,
                              ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
                              reject_reason: &mut Option<String>|
             -> bool {
                if args.len() != expected.len() {
                    note(reject_reason, || {
                        format!(
                            "builtin called with {} arg(s), expected {}",
                            args.len(),
                            expected.len()
                        )
                    });
                    return false;
                }
                for (a, want) in args.iter().zip(expected.iter()) {
                    match check_expr(
                        program,
                        a,
                        locals,
                        compound_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    ) {
                        Some(t) if t == *want => {}
                        _ => return false,
                    }
                }
                true
            };
            match func {
                BuiltinFunction::Panic => {
                    // `panic("literal")` is the only form the JIT can lower:
                    // the message has to be a parse-time `Expr::String(sym)`
                    // so codegen can pass the symbol id as a u64 immediate
                    // to `jit_panic`. Anything dynamic (a const, a runtime
                    // str, etc.) falls back to the interpreter where the
                    // value is already a real Object::ConstString / String.
                    //
                    // Returns `Never` (the bottom type) so that an
                    // expression-position panic — e.g. the then-branch of
                    // `if cond { panic("...") } else { 5i64 }` — unifies
                    // with the other branch's value type instead of
                    // forcing the if-expression to be Unit.
                    if args.len() != 1 {
                        note(reject_reason, || "panic takes 1 argument".to_string());
                        return None;
                    }
                    let arg = program.expression.get(&args[0])?;
                    if !matches!(arg, Expr::String(_)) {
                        note(reject_reason, || {
                            "panic argument must be a string literal in JIT".to_string()
                        });
                        return None;
                    }
                    Some(ScalarTy::Never)
                }
                BuiltinFunction::Assert => {
                    // `assert(cond, "literal")` — same constraint on the
                    // message as `panic` (literal only). The condition is a
                    // regular bool expression and is checked recursively.
                    if args.len() != 2 {
                        note(reject_reason, || {
                            "assert takes 2 arguments (cond, msg)".to_string()
                        });
                        return None;
                    }
                    let cond_ty = check_expr(
                        program, &args[0], locals, compound_locals,
                        substitutions, struct_layouts, callees, ptr_read_hints,
                        reject_reason,
                    )?;
                    if cond_ty != ScalarTy::Bool {
                        note(reject_reason, || {
                            "assert condition must be bool".to_string()
                        });
                        return None;
                    }
                    let msg_arg = program.expression.get(&args[1])?;
                    if !matches!(msg_arg, Expr::String(_)) {
                        note(reject_reason, || {
                            "assert message must be a string literal in JIT".to_string()
                        });
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::Print | BuiltinFunction::Println => {
                    if args.len() != 1 {
                        return None;
                    }
                    // Phase JE-3: reject `println(enum_local)` —
                    // enum identifiers report as U64 (their tag) so
                    // they would otherwise type-check, but the JIT
                    // would print just the tag whereas the
                    // interpreter / AOT print the full formatted
                    // enum value. Skip so the fallback handles it.
                    if let Some(Expr::Identifier(s)) = program.expression.get(&args[0]) {
                        if compound_locals.enums.contains_key(&s) {
                            note(reject_reason, || {
                                "JIT does not yet format enum values for print/println \
                                 (would print only the tag); see JIT-enum-1".to_string()
                            });
                            return None;
                        }
                    }
                    let t = check_expr(program, &args[0], locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                    // STR-INTERP-INTERP-JIT: Str is now a supported
                    // print value via `jit_print_str` / `jit_println_str`.
                    if !matches!(
                        t,
                        ScalarTy::I64 | ScalarTy::U64 | ScalarTy::F64
                            | ScalarTy::Bool | ScalarTy::Str
                    ) && !t.is_narrow_int()
                    {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::HeapAlloc => {
                    if !check_args(&[ScalarTy::U64], &args, locals, compound_locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Ptr)
                }
                BuiltinFunction::HeapFree => {
                    if !check_args(&[ScalarTy::Ptr], &args, locals, compound_locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::HeapRealloc => {
                    if !check_args(&[ScalarTy::Ptr, ScalarTy::U64], &args, locals, compound_locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Ptr)
                }
                BuiltinFunction::PtrIsNull => {
                    if !check_args(&[ScalarTy::Ptr], &args, locals, compound_locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Bool)
                }
                BuiltinFunction::StrToPtr => {
                    // `__builtin_str_to_ptr(s: str) -> ptr` is not yet
                    // hot-path JIT-eligible: the JIT has no `ScalarTy::Str`
                    // yet (string values aren't modelled as scalars in the
                    // JIT IR), so the call falls back to the interpreter
                    // path where the helper does its work.
                    *reject_reason = Some(
                        "__builtin_str_to_ptr (JIT does not yet model str scalar values)".to_string(),
                    );
                    None
                }
                BuiltinFunction::StrLen => {
                    *reject_reason = Some(
                        "__builtin_str_len (JIT does not yet model str scalar values)".to_string(),
                    );
                    None
                }
                BuiltinFunction::MemCopy | BuiltinFunction::MemMove => {
                    if !check_args(
                        &[ScalarTy::Ptr, ScalarTy::Ptr, ScalarTy::U64],
                        &args,
                        locals,
                    compound_locals,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    ) {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::MemSet => {
                    if !check_args(
                        &[ScalarTy::Ptr, ScalarTy::U64, ScalarTy::U64],
                        &args,
                        locals,
                    compound_locals,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    ) {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::PtrRead => {
                    // Args must be (ptr, u64). Return type is decided at the
                    // call site context — we look it up in the hint map. If
                    // the read appears in a position where eligibility never
                    // got to register a hint, fail.
                    if !check_args(
                        &[ScalarTy::Ptr, ScalarTy::U64],
                        &args,
                        locals,
                    compound_locals,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    ) {
                        return None;
                    }
                    let resolved = ptr_read_hints.get(expr_ref).copied();
                    if resolved.is_none() {
                        note(reject_reason, || {
                            "ptr_read used outside a typed val/var/assign — JIT \
                             needs the result type to be statically known"
                                .to_string()
                        });
                    }
                    resolved
                }
                BuiltinFunction::SizeOf => {
                    if args.len() != 1 {
                        note(reject_reason, || {
                            format!(
                                "__builtin_sizeof takes 1 argument, got {}",
                                args.len()
                            )
                        });
                        return None;
                    }
                    let t = check_expr(
                        program,
                        &args[0],
                        locals,
                        compound_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    if !matches!(
                        t,
                        ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Bool | ScalarTy::Ptr
                            | ScalarTy::F64
                    ) && !t.is_narrow_int()
                    {
                        note(reject_reason, || {
                            format!("__builtin_sizeof of {t:?} is not supported in JIT")
                        });
                        return None;
                    }
                    Some(ScalarTy::U64)
                }
                BuiltinFunction::ToString => {
                    // STR-INTERP-INTERP-JIT: __builtin_to_string(value)
                    // produces a heap-allocated str via the matching
                    // `jit_to_string_<ty>` runtime helper. Accept any
                    // scalar arg whose ScalarTy maps to a known
                    // helper — primitives only (struct / tuple / enum
                    // formatting still falls back).
                    if args.len() != 1 {
                        note(reject_reason, || {
                            format!("__builtin_to_string takes 1 arg, got {}", args.len())
                        });
                        return None;
                    }
                    let arg_ty = check_expr(
                        program, &args[0], locals, compound_locals, substitutions,
                        struct_layouts, callees, ptr_read_hints, reject_reason,
                    )?;
                    if !matches!(
                        arg_ty,
                        ScalarTy::I64 | ScalarTy::U64 | ScalarTy::F64 | ScalarTy::Bool
                            | ScalarTy::Str | ScalarTy::I8 | ScalarTy::U8
                            | ScalarTy::I16 | ScalarTy::U16
                            | ScalarTy::I32 | ScalarTy::U32
                    ) {
                        note(reject_reason, || {
                            format!(
                                "__builtin_to_string of {arg_ty:?} not supported in JIT \
                                 (only primitives lower to runtime helpers)"
                            )
                        });
                        return None;
                    }
                    Some(ScalarTy::Str)
                }
                BuiltinFunction::PtrWrite => {
                    if args.len() != 3 {
                        return None;
                    }
                    let p = check_expr(program, &args[0], locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                    let off = check_expr(program, &args[1], locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                    let v = check_expr(program, &args[2], locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                    if p != ScalarTy::Ptr || off != ScalarTy::U64 {
                        return None;
                    }
                    if !matches!(
                        v,
                        ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Bool | ScalarTy::Ptr
                    ) {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::DefaultAllocator
                | BuiltinFunction::ArenaAllocator
                | BuiltinFunction::CurrentAllocator => {
                    if !args.is_empty() {
                        note(reject_reason, || {
                            format!(
                                "{:?} expects no arguments, got {}",
                                func,
                                args.len()
                            )
                        });
                        return None;
                    }
                    Some(ScalarTy::Allocator)
                }
                BuiltinFunction::FixedBufferAllocator => {
                    if args.len() != 1 {
                        note(reject_reason, || {
                            format!(
                                "fixed_buffer_allocator expects 1 argument, got {}",
                                args.len()
                            )
                        });
                        return None;
                    }
                    let cap_ty = check_expr(
                        program,
                        &args[0],
                        locals,
                        compound_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    if cap_ty != ScalarTy::U64 {
                        note(reject_reason, || {
                            "fixed_buffer_allocator capacity must be u64".to_string()
                        });
                        return None;
                    }
                    Some(ScalarTy::Allocator)
                }
                BuiltinFunction::ArenaDrop => {
                    // #121 Phase B-rest Item 2 follow-up: explicit
                    // arena drop. JIT path falls back to interpreter
                    // since the JIT lowering for the user-callable
                    // form isn't wired (the AOT path is). Reject
                    // here so callers fall back cleanly.
                    note(reject_reason, || {
                        "JIT does not yet model __builtin_arena_drop".to_string()
                    });
                    None
                }
                BuiltinFunction::FixedBufferDrop => {
                    // Phase 5: explicit fixed_buffer drop, same JIT
                    // story as ArenaDrop — silent fallback to
                    // interpreter (the AOT path is wired).
                    note(reject_reason, || {
                        "JIT does not yet model __builtin_fixed_buffer_drop".to_string()
                    });
                    None
                }
                BuiltinFunction::Abs => {
                    if args.len() != 1 {
                        note(reject_reason, || {
                            format!("abs expects 1 argument, got {}", args.len())
                        });
                        return None;
                    }
                    let t = check_expr(
                        program,
                        &args[0],
                        locals,
                        compound_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    // Polymorphic: i64 / f64 both produce same-type
                    // result. f64 lowers to cranelift's `fabs`
                    // instruction, i64 to `select(x < 0, -x, x)`.
                    match t {
                        ScalarTy::I64 => Some(ScalarTy::I64),
                        ScalarTy::F64 => Some(ScalarTy::F64),
                        _ => {
                            note(reject_reason, || {
                                "abs expects an i64 or f64 argument".to_string()
                            });
                            None
                        }
                    }
                }
                // NOTE: f64 math arms (Sqrt/Pow and Sin..=Ceil) lived
                // here before Phase 4. Each is now an `extern fn
                // __extern_*_f64` declaration whose JIT lowering goes
                // through `try_gen_extern_call` (codegen) +
                // `JIT_EXTERN_DISPATCH` (eligibility's extern table).
                BuiltinFunction::Min | BuiltinFunction::Max => {
                    if args.len() != 2 {
                        let name = if matches!(func, BuiltinFunction::Min) { "min" } else { "max" };
                        note(reject_reason, || {
                            format!("{name} expects 2 arguments, got {}", args.len())
                        });
                        return None;
                    }
                    let a = check_expr(
                        program,
                        &args[0],
                        locals,
                        compound_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    let b = check_expr(
                        program,
                        &args[1],
                        locals,
                        compound_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    if !matches!(a, ScalarTy::I64 | ScalarTy::U64) || a != b {
                        let name = if matches!(func, BuiltinFunction::Min) { "min" } else { "max" };
                        note(reject_reason, || {
                            format!("{name} expects matching i64 or u64 operands")
                        });
                        return None;
                    }
                    Some(a)
                }
            }
        }
        Expr::With(allocator_expr, body_expr) => {
            // Validate the allocator producer first; it must yield an
            // Allocator handle.
            let alloc_ty = check_expr(
                program,
                &allocator_expr,
                locals,
                compound_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            )?;
            if alloc_ty != ScalarTy::Allocator {
                note(reject_reason, || {
                    "`with allocator =` requires an Allocator-typed expression".to_string()
                });
                return None;
            }
            // Early exits inside the body (`return` / `break` /
            // `continue`) are now supported: the codegen tracks the
            // active `with` depth and emits the matching pops before
            // each early-exit terminator.
            check_expr(
                program,
                &body_expr,
                locals,
                compound_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            )
        }
        Expr::MethodCall(receiver, method_name, args) => {
            // STR-INTERP-INTERP-JIT: `s.concat(t)` where the receiver
            // and arg are str. Codegen emits a direct call to the
            // `jit_str_concat` runtime helper. Intercept here before
            // the extension-trait lookup (which would miss it
            // because `concat` is registered as a `BuiltinMethod`
            // in the type checker, not as a stdlib trait impl).
            if Some(method_name) == concat_sym() && args.len() == 1 {
                let recv_ty = check_expr(
                    program, &receiver, locals, compound_locals, substitutions,
                    struct_layouts, callees, ptr_read_hints, reject_reason,
                )?;
                if matches!(recv_ty, ScalarTy::Str) {
                    let arg_ty = check_expr(
                        program, &args[0], locals, compound_locals, substitutions,
                        struct_layouts, callees, ptr_read_hints, reject_reason,
                    )?;
                    if !matches!(arg_ty, ScalarTy::Str) {
                        note(reject_reason, || {
                            format!("str.concat argument must be str, got {arg_ty:?}")
                        });
                        return None;
                    }
                    return Some(ScalarTy::Str);
                }
            }
            // The receiver may be:
            //   1. A struct local (existing struct-method dispatch).
            //   2. A primitive scalar local (Step C extension-trait
            //      dispatch — `i64.neg()` etc.).
            //   3. An arbitrary primitive-scalar-typed expression
            //      (#194 chained-call relaxation — `x.abs().abs()`,
            //      `make_i64().abs()`, etc.) where the receiver is
            //      not a bare identifier but type-checks to a known
            //      primitive scalar.
            // Eligibility rejects anything that doesn't reduce to
            // one of these.
            let recv_expr = program.expression.get(&receiver)?;
            let recv_name_opt = match recv_expr {
                Expr::Identifier(s) => Some(s),
                _ => None,
            };

            // Resolve the receiver's primitive type. Identifier
            // receivers consult `locals` directly; non-identifier
            // receivers re-enter `check_expr` so a chained
            // `MethodCall` / `Call` / `BinaryOp` etc. that returns a
            // scalar gets its scalar type back.
            let recv_prim_ty: Option<ScalarTy> = if let Some(name) = recv_name_opt {
                locals.get(&name).copied()
            } else {
                check_expr(
                    program,
                    &receiver,
                    locals,
                    compound_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                )
            };

            // Step C / #194: extension-trait dispatch on a primitive
            // receiver. The receiver scalar type keys into the
            // same `find_method` lookup the struct path uses. On
            // success we register the call as a `MonoTarget::Method`
            // so the analyzer queues the method body for compilation,
            // mirroring the struct path.
            if let Some(prim_ty) = recv_prim_ty {
                let target_sym = match primitive_target_sym_for_scalar(prim_ty) {
                    Some(s) => s,
                    None => {
                        note(reject_reason, || {
                            "method receiver primitive has no extension impls".to_string()
                        });
                        return None;
                    }
                };
                let method = match find_method(program, target_sym, method_name) {
                    Some(m) => m,
                    None => {
                        note(reject_reason, || {
                            "method not found on primitive type".to_string()
                        });
                        return None;
                    }
                };
                if !method.generic_params.is_empty() {
                    note(reject_reason, || {
                        "generic methods are not yet JIT-compatible".to_string()
                    });
                    return None;
                }
                if method.parameter.is_empty() {
                    note(reject_reason, || {
                        "method has no parameters; expected `self`".to_string()
                    });
                    return None;
                }
                let expected_param_count = method.parameter.len() - 1;
                if args.len() != expected_param_count {
                    note(reject_reason, || {
                        format!(
                            "primitive method call has {} arg(s), expects {}",
                            args.len(),
                            expected_param_count
                        )
                    });
                    return None;
                }
                // Type-check each arg against the parameter, with
                // `Self_` resolved to the receiver's primitive type.
                for (i, arg) in args.iter().enumerate() {
                    let raw_param_td = &method.parameter[i + 1].1;
                    let resolved_param_td = match raw_param_td {
                        TypeDecl::Self_ => primitive_type_decl_for_target_sym(target_sym)
                            .unwrap_or_else(|| raw_param_td.clone()),
                        other => other.clone(),
                    };
                    let actual = check_expr(
                        program, arg, locals, compound_locals,
                        substitutions, struct_layouts, callees, ptr_read_hints,
                        reject_reason,
                    )?;
                    let want = match resolve_param_ty(&resolved_param_td, substitutions, struct_layouts) {
                        Some(ParamTy::Scalar(s)) => s,
                        _ => {
                            note(reject_reason, || {
                                "primitive method parameter type unsupported".to_string()
                            });
                            return None;
                        }
                    };
                    if actual != want {
                        note(reject_reason, || {
                            format!("primitive method arg type mismatch: got {actual:?}, want {want:?}")
                        });
                        return None;
                    }
                }
                callees.push(MonoCall {
                    call_expr: *expr_ref,
                    target: MonoTarget::Method(target_sym, method_name),
                    mono_args: Vec::new(),
                });
                // Resolve the return type, with `Self_` mapping to the
                // receiver's primitive type.
                let ret_td = match &method.return_type {
                    Some(td) => match td {
                        TypeDecl::Self_ => primitive_type_decl_for_target_sym(target_sym)
                            .unwrap_or_else(|| td.clone()),
                        other => other.clone(),
                    },
                    None => TypeDecl::Unit,
                };
                return match resolve_param_ty(&ret_td, substitutions, struct_layouts) {
                    Some(ParamTy::Scalar(s)) => Some(s),
                    _ => {
                        note(reject_reason, || {
                            "primitive method return type unsupported".to_string()
                        });
                        None
                    }
                };
            }

            // Struct-method dispatch: the existing path requires a
            // bare `Identifier` receiver because struct values flow
            // through `struct_locals` (per-field SSA Variables) and
            // there's no machinery to materialise a chained struct
            // value back into that representation. Reject anything
            // that didn't come through the Identifier shortcut.
            let recv_name = match recv_name_opt {
                Some(s) => s,
                None => {
                    note(reject_reason, || {
                        "non-primitive method receiver must be a local identifier".to_string()
                    });
                    return None;
                }
            };
            // Phase JE-6: enum receiver method dispatch. When the
            // receiver is an enum local, look up the method on the
            // enum's base name. Generic methods (`impl<T>
            // Option<T>`) are instantiated by zipping the layout's
            // `generic_params` with the receiver's per-monomorph
            // type args (currently always `[payload_ty]` because
            // the JIT's single-payload-slot representation requires
            // all variants to share one scalar). Self_ resolves to
            // the enum name; the substitution map carries T from
            // the receiver.
            if let Some(enum_info) = compound_locals.enums.get(&recv_name).copied() {
                let method = match find_method(program, enum_info.base_name, method_name) {
                    Some(m) => m,
                    None => {
                        note(reject_reason, || {
                            "method not found on enum".to_string()
                        });
                        return None;
                    }
                };
                if method.parameter.is_empty() {
                    note(reject_reason, || {
                        "enum method has no parameters; expected `self`".to_string()
                    });
                    return None;
                }
                let layout = enum_layout_for(enum_info.base_name)?;
                // Build subst from method.generic_params. For the
                // common single-generic-param case (impl<T>
                // Option<T>) the method's [T] aligns with the
                // layout's [T]; we map T -> receiver.payload_ty.
                // For unit-only enums there's no payload_ty so
                // no substitution is bound (acceptable when the
                // method doesn't reference any generic param).
                let mut method_subst: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
                if !method.generic_params.is_empty() {
                    if layout.generic_params.is_empty() {
                        note(reject_reason, || {
                            "JIT enum method: generic method on non-generic enum is not yet supported".to_string()
                        });
                        return None;
                    }
                    // For multi-generic enums (Result<T, E>) the
                    // single-payload-slot constraint forces all
                    // params to bind to the same scalar. This is a
                    // strong restriction but matches the JE-4 layout.
                    let payload_ty = match enum_info.payload_ty {
                        Some(t) => t,
                        None => {
                            note(reject_reason, || {
                                "JIT enum method: receiver has no payload type to bind".to_string()
                            });
                            return None;
                        }
                    };
                    for p in &method.generic_params {
                        method_subst.insert(*p, payload_ty);
                    }
                }
                // Merge in the receiver-context subst (rare; methods
                // usually only see their own generics).
                for (k, v) in substitutions.iter() {
                    method_subst.entry(*k).or_insert(*v);
                }
                let expected_param_count = method.parameter.len() - 1;
                if args.len() != expected_param_count {
                    note(reject_reason, || {
                        format!(
                            "enum method call has {} arg(s), expects {}",
                            args.len(),
                            expected_param_count
                        )
                    });
                    return None;
                }
                // Validate each arg type against the (substituted)
                // method param type.
                for (i, arg) in args.iter().enumerate() {
                    let raw_param_td = &method.parameter[i + 1].1;
                    let resolved_param_td = match raw_param_td {
                        TypeDecl::Self_ => TypeDecl::Identifier(enum_info.base_name),
                        other => other.clone(),
                    };
                    let want = match resolve_param_ty(
                        &resolved_param_td, &method_subst, struct_layouts,
                    ) {
                        Some(ParamTy::Scalar(s)) => s,
                        _ => {
                            note(reject_reason, || {
                                "JIT enum method: only scalar parameters are supported"
                                    .to_string()
                            });
                            return None;
                        }
                    };
                    let actual = check_expr(
                        program, arg, locals, compound_locals, substitutions, struct_layouts, callees,
                        ptr_read_hints, reject_reason,
                    )?;
                    if actual != want {
                        note(reject_reason, || {
                            format!(
                                "enum method arg type mismatch: got {actual:?}, want {want:?}"
                            )
                        });
                        return None;
                    }
                }
                // Build mono_args from the layout's generic_params
                // resolved through method_subst. Empty for non-
                // generic enums.
                let mono_args: Vec<ScalarTy> = layout
                    .generic_params
                    .iter()
                    .filter_map(|p| method_subst.get(p).copied())
                    .collect();
                callees.push(MonoCall {
                    call_expr: *expr_ref,
                    target: MonoTarget::Method(enum_info.base_name, method_name),
                    mono_args,
                });
                // Return type with Self_ -> enum, generics
                // substituted.
                let ret = match &method.return_type {
                    Some(td) => {
                        let resolved = match td {
                            TypeDecl::Self_ => TypeDecl::Identifier(enum_info.base_name),
                            other => other.clone(),
                        };
                        match resolve_param_ty(&resolved, &method_subst, struct_layouts) {
                            Some(ParamTy::Scalar(s)) => return Some(s),
                            Some(ParamTy::Enum { .. }) => {
                                note(reject_reason, || {
                                    "JIT enum method returning enum must be the rhs of a val/var \
                                     (JE-6 expression-position scope)".to_string()
                                });
                                return None;
                            }
                            _ => {
                                note(reject_reason, || {
                                    "enum method return type unsupported".to_string()
                                });
                                return None;
                            }
                        }
                    }
                    None => return Some(ScalarTy::Unit),
                };
                let _ = ret;
            }
            let struct_name = match compound_locals.structs.get(&recv_name).copied() {
                Some(s) => s,
                None => {
                    note(reject_reason, || {
                        "method receiver is not a known struct local".to_string()
                    });
                    return None;
                }
            };
            // Validate each argument's type against the corresponding
            // method parameter (skipping `self`). Methods don't yet
            // support generics, so callee_subs is always empty.
            // Linear scan over top-level ImplBlock decls is fine — only
            // run once per call site, and the analyzer already pre-built
            // a method_map for the work-stack pass.
            let method = match find_method(program, struct_name, method_name) {
                Some(m) => m,
                None => {
                    note(reject_reason, || {
                        "method not found on struct".to_string()
                    });
                    return None;
                }
            };
            if !method.generic_params.is_empty() {
                note(reject_reason, || {
                    "generic methods are not yet JIT-compatible".to_string()
                });
                return None;
            }
            // The first parameter is the receiver (`self: Self` in the
            // language's preferred style). Remaining parameters must
            // line up with the explicit arguments at the call site.
            if method.parameter.is_empty() {
                note(reject_reason, || {
                    "method has no parameters; expected `self`".to_string()
                });
                return None;
            }
            let expected_param_count = method.parameter.len() - 1;
            if args.len() != expected_param_count {
                note(reject_reason, || {
                    format!(
                        "method call has {} arg(s), expects {}",
                        args.len(),
                        expected_param_count
                    )
                });
                return None;
            }
            for (i, arg) in args.iter().enumerate() {
                let param_td = &method.parameter[i + 1].1;
                let arg_expr = program.expression.get(arg)?;
                if let Expr::Identifier(id) = arg_expr {
                    if let Some(arg_struct) = compound_locals.structs.get(&id).copied() {
                        match param_td {
                            TypeDecl::Identifier(s) | TypeDecl::Struct(s, _)
                                if *s == arg_struct
                                    && struct_layouts.contains_key(s) =>
                            {
                                continue;
                            }
                            _ => {
                                note(reject_reason, || {
                                    "method struct argument type mismatch".to_string()
                                });
                                return None;
                            }
                        }
                    }
                }
                // Scalar arg path
                let actual = check_expr(
                    program,
                    arg,
                    locals,
                    compound_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                )?;
                let want = match resolve_param_ty(param_td, substitutions, struct_layouts) {
                    Some(ParamTy::Scalar(s)) => s,
                    _ => {
                        note(reject_reason, || {
                            "method parameter type unsupported".to_string()
                        });
                        return None;
                    }
                };
                if actual != want {
                    note(reject_reason, || {
                        format!("method arg type mismatch: got {actual:?}, want {want:?}")
                    });
                    return None;
                }
            }
            callees.push(MonoCall {
                call_expr: *expr_ref,
                target: MonoTarget::Method(struct_name, method_name),
                mono_args: Vec::new(),
            });
            // Compute method's return type.
            match &method.return_type {
                Some(td) => {
                    let resolved = match td {
                        TypeDecl::Self_ => TypeDecl::Identifier(struct_name),
                        other => other.clone(),
                    };
                    match resolve_param_ty(&resolved, substitutions, struct_layouts) {
                        Some(ParamTy::Scalar(s)) => Some(s),
                        Some(ParamTy::Struct(_)) => {
                            // Struct-returning methods only flow through
                            // val/var rhs, similar to free-function struct
                            // returns. Reject in arbitrary positions.
                            note(reject_reason, || {
                                "struct-returning method must be the rhs of a val/var"
                                    .to_string()
                            });
                            None
                        }
                        Some(ParamTy::Tuple(_)) => {
                            note(reject_reason, || {
                                "tuple-returning method must be the rhs of a val/var"
                                    .to_string()
                            });
                            None
                        }
                        Some(ParamTy::Enum { .. }) => {
                            note(reject_reason, || {
                                "enum-returning method must be the rhs of a val/var"
                                    .to_string()
                            });
                            None
                        }
                        None => None,
                    }
                }
                None => Some(ScalarTy::Unit),
            }
        }
        Expr::FieldAccess(receiver, field_name) => {
            // Read access on a struct local: returns the field's scalar
            // type. Anything else (FieldAccess on a function call result,
            // nested FieldAccess, etc.) falls through to ineligible.
            let receiver_expr = program.expression.get(&receiver)?;
            let recv_name = match receiver_expr {
                Expr::Identifier(s) => s,
                _ => {
                    note(reject_reason, || {
                        "field access receiver must be a struct local".to_string()
                    });
                    return None;
                }
            };
            let struct_name = match compound_locals.structs.get(&recv_name).copied() {
                Some(s) => s,
                None => {
                    note(reject_reason, || {
                        "field access on a non-struct local".to_string()
                    });
                    return None;
                }
            };
            let field_ty = struct_layouts
                .get(&struct_name)
                .and_then(|l| l.field(field_name));
            if field_ty.is_none() {
                note(reject_reason, || "unknown field on struct".to_string());
            }
            field_ty
        }
        Expr::TupleAccess(tuple, idx) => {
            // Read access on a tuple local. The receiver must be an
            // identifier already bound to a known tuple shape; the
            // index must be in-range.
            let recv_expr = program.expression.get(&tuple)?;
            let recv_name = match recv_expr {
                Expr::Identifier(s) => s,
                _ => {
                    note(reject_reason, || {
                        "tuple access receiver must be a tuple local".to_string()
                    });
                    return None;
                }
            };
            let shape = match compound_locals.tuples.get(&recv_name).cloned() {
                Some(v) => v,
                None => {
                    note(reject_reason, || {
                        "tuple access on a non-tuple local".to_string()
                    });
                    return None;
                }
            };
            if idx >= shape.len() {
                note(reject_reason, || {
                    format!("tuple index {idx} out of bounds for shape {shape:?}")
                });
                return None;
            }
            Some(shape[idx])
        }
        Expr::Cast(inner, target) => {
            // Casts allowed: any-width int ↔ any-width int (sextend /
            // uextend / ireduce in codegen), and i64/u64 ↔ f64 (real
            // fcvt instructions). bool casts are intentionally
            // excluded.
            let inner_ty = check_expr(program, &inner, locals, compound_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            let target_ty = ScalarTy::from_type_decl(&target)?;
            let int_or_float = |t: ScalarTy| {
                matches!(t, ScalarTy::I64 | ScalarTy::U64 | ScalarTy::F64) || t.is_narrow_int()
            };
            if !int_or_float(inner_ty) {
                return None;
            }
            if !int_or_float(target_ty) {
                return None;
            }
            // Reject narrow-int <-> f64 for now: those require fcvt
            // pre-extending the narrow side, which the cast codegen
            // doesn't yet handle. Stay safe and fall back.
            let narrow_to_float = inner_ty.is_narrow_int() && target_ty == ScalarTy::F64;
            let float_to_narrow = inner_ty == ScalarTy::F64 && target_ty.is_narrow_int();
            if narrow_to_float || float_to_narrow {
                note(reject_reason, || {
                    "JIT cast between narrow int and f64 is not yet supported".to_string()
                });
                return None;
            }
            Some(target_ty)
        }
        // Everything else is unsupported in this iteration.
        other => {
            // Phase JE-1a: a `QualifiedIdentifier` whose head is a
            // JIT-eligible enum (non-generic, unit-only) corresponds
            // to a unit-variant constructor like `Color::Red`. The
            // tag layout is already in `enum_layouts`; the missing
            // piece is constructor + match codegen (Phase JE-1b).
            // Surface a precise "infra ready, codegen pending"
            // message instead of the generic "qualified identifier"
            // catch-all so the next phase knows which programs to
            // enable.
            let precise = match &other {
                Expr::QualifiedIdentifier(path)
                    if path.len() == 2
                        && enum_layout_for(path[0])
                            .and_then(|l| l.variant_tag(path[1]))
                            .is_some() =>
                {
                    "JIT enum support pending: unit-variant constructor codegen \
                     (Phase JE-1b will lower this via the existing tag layout)"
                        .to_string()
                }
                Expr::QualifiedIdentifier(path)
                    if !path.is_empty()
                        && enum_decl_lookup_by_name(program, path[0]).is_some() =>
                {
                    "JIT does not yet model enum values \
                     (constructors / match / methods)"
                        .to_string()
                }
                // Closures Phase 4: explicit reject reason. The JIT
                // doesn't model `Object::Closure` values — each
                // closure literal would need a captured-environment
                // representation + indirect-call dispatch the JIT
                // doesn't have. The interpreter handles closures
                // natively (Phase 3); JIT-eligible programs simply
                // fall back to interpretation when they contain a
                // closure literal.
                Expr::Closure { .. } => {
                    "JIT does not yet support closure / lambda values \
                     (interpreter handles them; AOT support is a later phase)"
                        .to_string()
                }
                _ => format!("uses unsupported expression {}", expr_kind_name(&other)),
            };
            note(reject_reason, move || precise);
            None
        }
    }
}

/// Shared eligibility check for plain function calls. Used by both
/// `Expr::Call(name, ExprList(args))` (the bare-name form) and
/// `Expr::AssociatedFunctionCall(module, name, args)` (the
/// module-qualified form, after the module-alias guard has confirmed
/// the qualifier doesn't refer to a struct). Locates the callee in
/// the program's function table, type-checks each argument against
/// the matching parameter (handling identifier-of-struct, identifier-
/// of-tuple-local, inline tuple literal, and scalar fall-throughs),
/// records the monomorphisation key, and returns the substituted
/// callee return type.
#[allow(clippy::too_many_arguments)]
fn check_plain_call(
    program: &Program,
    expr_ref: &ExprRef,
    name: DefaultSymbol,
    arg_list: &Vec<ExprRef>,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &mut CompoundLocals,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> Option<ScalarTy> {
    // Locate the callee in the program's function table.
    let callee = program.function.iter().find(|f| f.name == name).cloned();
    let callee = match callee {
        Some(f) => f,
        None => {
            note(reject_reason, || "calls an unknown function".to_string());
            return None;
        }
    };

    // `extern fn` callees: validate against the JIT extern dispatch
    // table. The table is keyed by interned symbol via the
    // thread-local set up by `analyze` (see `with_extern_dispatch`).
    // Names that aren't in the table fall back to the interpreter
    // call path.
    if callee.is_extern {
        let entry = match jit_extern_dispatch_for(name) {
            Some(e) => e,
            None => {
                note(reject_reason, || {
                    "calls extern fn not registered with the JIT".to_string()
                });
                return None;
            }
        };
        if arg_list.len() != entry.params.len() {
            note(reject_reason, || {
                format!(
                    "extern fn called with {} arg(s), expected {}",
                    arg_list.len(),
                    entry.params.len()
                )
            });
            return None;
        }
        for (a, want) in arg_list.iter().zip(entry.params.iter()) {
            match check_expr(
                program, a, locals, compound_locals,
                substitutions, struct_layouts, callees, ptr_read_hints,
                reject_reason,
            ) {
                Some(t) if t == *want => {}
                _ => {
                    note(reject_reason, || {
                        "extern fn argument type mismatch".to_string()
                    });
                    return None;
                }
            }
        }
        // Extern fns skip the monomorphisation pipeline; codegen
        // recognises the `is_extern` flag and emits the matching
        // helper / native op.
        return Some(entry.ret);
    }

    // Resolve each argument's type, allowing struct identifiers
    // when the callee's parameter at that position is a struct.
    // Generic substitutions are inferred only from scalar args;
    // generic-over-struct functions aren't supported in this
    // iteration.
    if arg_list.len() != callee.parameter.len() {
        note(reject_reason, || {
            format!(
                "call has {} arg(s), callee expects {}",
                arg_list.len(),
                callee.parameter.len()
            )
        });
        return None;
    }
    let mut scalar_arg_tys: Vec<ScalarTy> = Vec::with_capacity(arg_list.len());
    let mut callee_param_tys: Vec<ParamTy> = Vec::with_capacity(arg_list.len());
    for (a, (_, param_td)) in arg_list.iter().zip(callee.parameter.iter()) {
        let arg_expr = program.expression.get(a)?;
        // Phase JE-2d: enum-typed argument matching. Either the
        // arg is a direct identifier of an enum local, or it's a
        // unit / tuple constructor for the matching enum (the same
        // shapes the boundary expansion supports).
        if let Some(ParamTy::Enum { base_name: want_enum, payload_ty: want_payload }) =
            resolve_param_ty(param_td, substitutions, struct_layouts)
        {
            // Identifier of an enum local.
            if let Expr::Identifier(id) = arg_expr {
                if let Some(local_info) = compound_locals.enums.get(&id).copied() {
                    if local_info.base_name != want_enum {
                        note(reject_reason, || {
                            "enum argument's type does not match callee parameter".to_string()
                        });
                        return None;
                    }
                    // JE-5: per-monomorph payload type must agree
                    // (e.g. you can't pass `Opt<i64>` to a `Opt<u64>`
                    // parameter — both are `Opt` but the cranelift
                    // payload widths differ).
                    if local_info.payload_ty != want_payload {
                        note(reject_reason, || {
                            "enum argument's monomorph payload type does not match \
                             callee parameter".to_string()
                        });
                        return None;
                    }
                    callee_param_tys.push(ParamTy::Enum {
                        base_name: local_info.base_name,
                        payload_ty: local_info.payload_ty,
                    });
                    scalar_arg_tys.push(ScalarTy::Unit);
                    continue;
                }
            }
            // Inline unit / tuple constructor — reuse the
            // val/var rhs helper to validate the variant + payload.
            // Build a synthetic annotation hint from the param type
            // so generic-enum unit constructors can resolve T.
            if let Some(info) = check_enum_constructor_rhs(
                program, a, locals, compound_locals,
                substitutions, struct_layouts, callees, ptr_read_hints,
                reject_reason, Some(param_td),
            ) {
                if info.payload_ty != want_payload {
                    note(reject_reason, || {
                        "inline enum constructor's payload monomorph does not match \
                         callee parameter".to_string()
                    });
                    return None;
                }
                callee_param_tys.push(ParamTy::Enum {
                    base_name: want_enum,
                    payload_ty: want_payload,
                });
                scalar_arg_tys.push(ScalarTy::Unit);
                continue;
            }
            note(reject_reason, || {
                "enum argument must be a local identifier or a constructor for the matching enum"
                    .to_string()
            });
            return None;
        }
        if let Expr::Identifier(id) = arg_expr {
            if let Some(struct_name) = compound_locals.structs.get(&id).copied() {
                match param_td {
                    TypeDecl::Identifier(s) | TypeDecl::Struct(s, _)
                        if *s == struct_name && struct_layouts.contains_key(s) =>
                    {
                        callee_param_tys.push(ParamTy::Struct(struct_name));
                        scalar_arg_tys.push(ScalarTy::Unit);
                        continue;
                    }
                    _ => {
                        note(reject_reason, || {
                            "struct argument's type does not match callee parameter"
                                .to_string()
                        });
                        return None;
                    }
                }
            }
            if let Some(shape) = compound_locals.tuples.get(&id).cloned() {
                let want = match resolve_param_ty(param_td, substitutions, struct_layouts) {
                    Some(ParamTy::Tuple(ts)) => ts,
                    _ => {
                        note(reject_reason, || {
                            "tuple argument's type does not match callee parameter".to_string()
                        });
                        return None;
                    }
                };
                if want != shape {
                    note(reject_reason, || {
                        "tuple argument shape does not match callee parameter".to_string()
                    });
                    return None;
                }
                callee_param_tys.push(ParamTy::Tuple(shape));
                scalar_arg_tys.push(ScalarTy::Unit);
                continue;
            }
        }
        if let Expr::TupleLiteral(elements) = arg_expr {
            let want = match resolve_param_ty(param_td, substitutions, struct_layouts) {
                Some(ParamTy::Tuple(ts)) => ts,
                _ => {
                    note(reject_reason, || {
                        "inline tuple literal argument needs a tuple parameter".to_string()
                    });
                    return None;
                }
            };
            if elements.len() != want.len() {
                note(reject_reason, || {
                    "inline tuple literal argument arity does not match callee parameter".to_string()
                });
                return None;
            }
            let mut shape: Vec<ScalarTy> = Vec::with_capacity(elements.len());
            for e in &elements {
                let t = check_expr(
                    program, e, locals, compound_locals, substitutions,
                    struct_layouts, callees, ptr_read_hints, reject_reason,
                )?;
                shape.push(t);
            }
            if shape != want {
                note(reject_reason, || {
                    "inline tuple literal argument element types do not match callee parameter"
                        .to_string()
                });
                return None;
            }
            callee_param_tys.push(ParamTy::Tuple(shape));
            scalar_arg_tys.push(ScalarTy::Unit);
            continue;
        }
        let t = check_expr(
            program, a, locals, compound_locals, substitutions,
            struct_layouts, callees, ptr_read_hints, reject_reason,
        )?;
        scalar_arg_tys.push(t);
        callee_param_tys.push(ParamTy::Scalar(t));
    }

    let callee_subs = match infer_substitutions(
        &callee, &scalar_arg_tys, substitutions, reject_reason,
    ) {
        Some(s) => s,
        None => return None,
    };

    let mono_args: Vec<ScalarTy> = callee
        .generic_params
        .iter()
        .map(|g| callee_subs.get(g).copied().unwrap_or(ScalarTy::Unit))
        .collect();
    callees.push(MonoCall {
        call_expr: *expr_ref,
        target: MonoTarget::Function(name),
        mono_args,
    });

    let _ = callee_param_tys;

    match &callee.return_type {
        Some(td) => substitute_to_scalar(td, &callee_subs),
        None => Some(ScalarTy::Unit),
    }
}
