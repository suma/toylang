//! Walks the AST starting from `main` and collects every function that the
//! JIT can compile. A function is eligible when its signature uses only
//! `i64`/`u64`/`bool`/`Unit` and its body uses only the supported expression
//! and statement kinds (literals, locals, arithmetic, comparison, logical,
//! bitwise, unary, if/elif/else, while, for-range, val/var, assignment to
//! locals, return, calls to other eligible functions). Anything else makes
//! the entire reachable set ineligible — the caller silently falls back to
//! the tree-walking interpreter.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use frontend::ast::{BuiltinFunction, Expr, ExprRef, Function, Operator, Program, Stmt, StmtRef, UnaryOp};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

/// Records the *first* reason eligibility analysis rejected the program.
/// Subsequent rejections deeper in the recursion are ignored — the user
/// only needs the closest hint to the surface.
fn note(reason: &mut Option<String>, msg: impl FnOnce() -> String) {
    if reason.is_none() {
        *reason = Some(msg());
    }
}

/// JIT-supported scalar types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarTy {
    I64,
    U64,
    Bool,
    Unit,
    /// Heap pointer. Internally a u64 / cranelift I64 — distinct from
    /// `U64` for type checking but ABI-compatible.
    Ptr,
}

impl ScalarTy {
    pub fn from_type_decl(td: &TypeDecl) -> Option<Self> {
        match td {
            TypeDecl::Int64 => Some(ScalarTy::I64),
            TypeDecl::UInt64 => Some(ScalarTy::U64),
            TypeDecl::Bool => Some(ScalarTy::Bool),
            TypeDecl::Unit => Some(ScalarTy::Unit),
            TypeDecl::Ptr => Some(ScalarTy::Ptr),
            _ => None,
        }
    }
}

/// JIT-supported parameter / argument types. A `ParamTy::Struct` expands
/// into one cranelift parameter per scalar field at the ABI level.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ParamTy {
    Scalar(ScalarTy),
    /// Struct value, identified by its declared type name. Field layout
    /// is looked up via `EligibleSet::struct_layouts`.
    Struct(DefaultSymbol),
}

/// Signature of an eligible function in JIT-friendly form.
#[derive(Debug, Clone)]
pub struct FuncSignature {
    pub params: Vec<(DefaultSymbol, ParamTy)>,
    pub ret: ParamTy,
}

/// Identifies a single monomorphization of a function. The `Vec<ScalarTy>`
/// is the substitution list ordered by `Function::generic_params`; it's
/// empty for non-generic functions, so a non-generic function has exactly
/// one MonoKey `(name, vec![])`.
pub type MonoKey = (DefaultSymbol, Vec<ScalarTy>);

/// Field layout for a JIT-compatible struct: every field must be a JIT
/// scalar type (no nested structs in this iteration). Field names are
/// stored as `DefaultSymbol`s so they can be matched directly against
/// the symbols carried by `Expr::StructLiteral` and `Expr::FieldAccess`.
#[derive(Debug, Clone)]
pub struct StructLayout {
    pub fields: Vec<(DefaultSymbol, ScalarTy)>,
}

impl StructLayout {
    pub fn field(&self, name: DefaultSymbol) -> Option<ScalarTy> {
        self.fields
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, t)| *t)
    }
}

/// Result of eligibility analysis. Each MonoKey corresponds to one
/// cranelift function the runtime will compile.
pub struct EligibleSet {
    /// Each monomorphization key -> the source `Function` it came from.
    pub monomorphs: HashMap<MonoKey, Rc<Function>>,
    /// Each monomorphization key -> its concrete (substituted) signature.
    pub signatures: HashMap<MonoKey, FuncSignature>,
    /// For every user-defined function call expression, the
    /// monomorphization the JIT should target. Different call sites of
    /// the same generic function with different arg types get different
    /// MonoKeys here.
    pub call_targets: HashMap<ExprRef, MonoKey>,
    /// `__builtin_ptr_read(...)` is type-polymorphic at the language level —
    /// the interpreter picks the return type from the typed-slot store at
    /// runtime. The JIT instead requires the expected scalar at compile
    /// time, so eligibility records the type for every supported PtrRead
    /// position (Val/Var/Assign with a typed identifier on the LHS).
    /// Codegen reads back from this map to pick the right helper.
    /// Generic functions cannot use PtrRead: the same ExprRef would need
    /// distinct types per monomorph.
    pub ptr_read_hints: HashMap<ExprRef, ScalarTy>,
    /// Layout of every struct type the JIT understands. Built in a
    /// pre-pass over top-level `Stmt::StructDecl` declarations.
    pub struct_layouts: HashMap<DefaultSymbol, StructLayout>,
}

/// Per-callsite monomorphization record. `call_expr` identifies the
/// `Expr::Call` AST node so codegen can map back to the right callee
/// FuncRef; `callee_name` and `mono_args` build the MonoKey.
#[derive(Debug, Clone)]
pub(crate) struct MonoCall {
    pub call_expr: ExprRef,
    pub callee_name: DefaultSymbol,
    pub mono_args: Vec<ScalarTy>,
}

pub fn analyze(
    program: &Program,
    main: &Rc<Function>,
    interner: &DefaultStringInterner,
) -> Result<EligibleSet, String> {
    let mut function_map: HashMap<DefaultSymbol, Rc<Function>> = HashMap::new();
    for f in &program.function {
        function_map.insert(f.name, f.clone());
    }

    // Pre-pass: build layouts for every struct whose fields are all
    // JIT-supported scalars. Anything else (nested struct fields,
    // generic structs, struct with arrays / strings, …) is silently
    // omitted; reads from such types would later reject anyway.
    let struct_layouts = collect_struct_layouts(program, interner);

    let mut visited: HashSet<MonoKey> = HashSet::new();
    let mut signatures: HashMap<MonoKey, FuncSignature> = HashMap::new();
    let mut monomorphs: HashMap<MonoKey, Rc<Function>> = HashMap::new();
    let mut call_targets: HashMap<ExprRef, MonoKey> = HashMap::new();
    let mut ptr_read_hints: HashMap<ExprRef, ScalarTy> = HashMap::new();
    // Work item: (function, substitution-vec ordered by func.generic_params).
    let mut stack: Vec<(Rc<Function>, Vec<ScalarTy>)> = vec![(main.clone(), Vec::new())];

    while let Some((func, subs_vec)) = stack.pop() {
        let key: MonoKey = (func.name, subs_vec.clone());
        if !visited.insert(key.clone()) {
            continue;
        }

        let fname_disp = display_mono(interner, &func, &subs_vec);

        // Generic bound checks (e.g. `<A: Allocator>`) require runtime
        // allocator handles which the JIT can't represent. Keep things
        // simple by rejecting bound generics.
        if !func.generic_bounds.is_empty() {
            return Err(format!(
                "function `{fname_disp}` has generic bounds (not supported in JIT)"
            ));
        }

        // The substitution list must agree with the function's generic
        // parameter count; mismatches are an analyzer bug, not a user
        // problem, so promote to a hard error.
        if subs_vec.len() != func.generic_params.len() {
            return Err(format!(
                "internal: monomorph for `{fname_disp}` expected {} substitutions, got {}",
                func.generic_params.len(),
                subs_vec.len()
            ));
        }
        let substitutions: HashMap<DefaultSymbol, ScalarTy> = func
            .generic_params
            .iter()
            .copied()
            .zip(subs_vec.iter().copied())
            .collect();

        let mut sig_reason: Option<String> = None;
        let sig = match function_signature(&func, &substitutions, &struct_layouts, &mut sig_reason) {
            Some(s) => s,
            None => {
                let detail = sig_reason.unwrap_or_else(|| "unsupported signature".into());
                return Err(format!("function `{fname_disp}`: {detail}"));
            }
        };

        let mut callees: Vec<MonoCall> = Vec::new();
        let mut body_reason: Option<String> = None;
        if !check_function_body(
            program,
            &func,
            &sig,
            &substitutions,
            &struct_layouts,
            &mut callees,
            &mut ptr_read_hints,
            &mut body_reason,
        ) {
            let detail = body_reason.unwrap_or_else(|| "unsupported feature".into());
            return Err(format!("function `{fname_disp}`: {detail}"));
        }

        signatures.insert(key.clone(), sig);
        monomorphs.insert(key.clone(), func.clone());

        for call in callees {
            let callee_fn = match function_map.get(&call.callee_name) {
                Some(f) => f.clone(),
                None => {
                    let cname = interner.resolve(call.callee_name).unwrap_or("<anon>");
                    return Err(format!(
                        "function `{fname_disp}` calls unknown / non-eligible function `{cname}`"
                    ));
                }
            };
            let callee_key: MonoKey = (call.callee_name, call.mono_args);
            call_targets.insert(call.call_expr, callee_key.clone());
            stack.push((callee_fn, callee_key.1));
        }
    }

    Ok(EligibleSet {
        monomorphs,
        signatures,
        call_targets,
        ptr_read_hints,
        struct_layouts,
    })
}

fn collect_struct_layouts(
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

/// Format a monomorphization for diagnostic output, e.g. `id<i64>`.
fn display_mono(
    interner: &DefaultStringInterner,
    func: &Function,
    mono_args: &[ScalarTy],
) -> String {
    let name = interner.resolve(func.name).unwrap_or("<anon>");
    if mono_args.is_empty() {
        name.to_string()
    } else {
        let parts: Vec<String> = mono_args.iter().map(|t| format!("{t:?}")).collect();
        format!("{name}<{}>", parts.join(", "))
    }
}

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
fn infer_substitutions(
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
        // Struct param positions skip generic inference and scalar
        // matching — the caller has already validated that the arg's
        // struct type lines up with the callee's declared type.
        if matches!(
            param_td,
            TypeDecl::Identifier(_) | TypeDecl::Struct(_, _)
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

fn function_signature(
    func: &Function,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    reject_reason: &mut Option<String>,
) -> Option<FuncSignature> {
    let mut params = Vec::with_capacity(func.parameter.len());
    for (_, td) in &func.parameter {
        let pt = match resolve_param_ty(td, substitutions, struct_layouts) {
            Some(p) => p,
            None => {
                note(reject_reason, || {
                    format!("parameter has unsupported type {td:?}")
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
        params.push((func.parameter[params.len()].0, pt));
    }
    // Return type. Scalars and structs are both allowed; struct returns
    // expand into cranelift multi-returns (one cranelift return per
    // field) at the ABI layer.
    let ret = match &func.return_type {
        Some(td) => match resolve_param_ty(td, substitutions, struct_layouts) {
            Some(p) => p,
            None => {
                note(reject_reason, || {
                    format!("return type {td:?} is not supported")
                });
                return None;
            }
        },
        None => ParamTy::Scalar(ScalarTy::Unit),
    };
    Some(FuncSignature { params, ret })
}

/// Resolve a TypeDecl into a JIT parameter type, considering both scalar
/// substitutions (for generic monomorphs) and known struct layouts.
pub(crate) fn resolve_param_ty(
    td: &TypeDecl,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
) -> Option<ParamTy> {
    if let Some(s) = substitute_to_scalar(td, substitutions) {
        return Some(ParamTy::Scalar(s));
    }
    match td {
        TypeDecl::Identifier(s) | TypeDecl::Struct(s, _)
            if struct_layouts.contains_key(s) =>
        {
            Some(ParamTy::Struct(*s))
        }
        _ => None,
    }
}

/// Walks a function body to confirm it only uses supported constructs and
/// reports every callee found via `callees`. Returns false on the first
/// unsupported construct.
fn check_function_body(
    program: &Program,
    func: &Function,
    sig: &FuncSignature,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    // Generic functions are forbidden from using `__builtin_ptr_read`
    // because the hint table is keyed by ExprRef, which is shared across
    // monomorphs of the same function body. Reject early so the diagnostic
    // is clearer than a per-arm rejection deep inside the body.
    if !func.generic_params.is_empty() && body_has_ptr_read(program, &func.code) {
        note(reject_reason, || {
            "generic functions cannot use __builtin_ptr_read in JIT".to_string()
        });
        return false;
    }
    let mut locals: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
    let mut struct_locals: HashMap<DefaultSymbol, DefaultSymbol> = HashMap::new();
    for (n, t) in &sig.params {
        match t {
            ParamTy::Scalar(s) => {
                locals.insert(*n, *s);
            }
            ParamTy::Struct(struct_name) => {
                struct_locals.insert(*n, *struct_name);
            }
        }
    }

    // For struct-returning functions, the body's terminal expression
    // must produce a struct value (Identifier of a struct local, or a
    // StructLiteral). check_expr rejects struct literals in arbitrary
    // positions, so we process the leading statements normally and then
    // validate the trailing expression by hand.
    if let ParamTy::Struct(struct_name) = &sig.ret {
        return check_struct_returning_body(
            program,
            &func.code,
            *struct_name,
            &mut locals,
            &mut struct_locals,
            substitutions,
            struct_layouts,
            callees,
            ptr_read_hints,
            reject_reason,
        );
    }

    check_stmt(
        program,
        &func.code,
        &mut locals,
        &mut struct_locals,
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
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
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
            struct_locals,
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
            match struct_locals.get(&name).copied() {
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
                struct_locals,
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
        note(reject_reason, || {
            "struct literal references a struct without a JIT-eligible scalar layout".to_string()
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
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
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
        struct_locals,
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

/// Validate every field of a struct literal against the registered
/// layout. Records callees / ptr_read hints encountered while typing the
/// individual field initializers.
#[allow(clippy::too_many_arguments)]
fn check_struct_literal_fields(
    program: &Program,
    value_ref: &ExprRef,
    struct_name: DefaultSymbol,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let layout = match struct_layouts.get(&struct_name) {
        Some(l) => l.clone(),
        None => {
            note(reject_reason, || {
                "struct layout missing in JIT analysis".to_string()
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
            struct_locals,
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
        Stmt::For(_, s, e, body) => {
            walk_expr_for_ptr_read(program, &s, found);
            walk_expr_for_ptr_read(program, &e, found);
            walk_expr_for_ptr_read(program, &body, found);
        }
        Stmt::While(c, body) => {
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
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
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
            check_expr(program, &e, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason).is_some()
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
                    struct_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                ) {
                    return false;
                }
                struct_locals.insert(name, struct_name);
                return true;
            }
            // Special-case: a struct-returning function call also lands as
            // a fresh struct local. Validate the call site (and its args)
            // through the normal Call eligibility path.
            if let Some(struct_name) = check_struct_returning_call(
                program,
                &value,
                locals,
                struct_locals,
                substitutions,
                struct_layouts,
                callees,
                ptr_read_hints,
                reject_reason,
            ) {
                struct_locals.insert(name, struct_name);
                return true;
            }
            let declared_hint = type_decl.as_ref().and_then(ScalarTy::from_type_decl);
            // If both the annotation and the RHS are PtrRead-shaped, record
            // the expected return type before recursing so check_expr can
            // accept the otherwise type-polymorphic builtin.
            if let Some(t) = declared_hint {
                register_ptr_read_hint(program, &value, t, ptr_read_hints);
            }
            let val_ty = match check_expr(program, &value, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            let declared = match type_decl {
                Some(td) => match ScalarTy::from_type_decl(&td) {
                    Some(t) => t,
                    None => return false,
                },
                None => val_ty,
            };
            if declared != val_ty {
                return false;
            }
            if declared == ScalarTy::Unit {
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
                        struct_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    ) {
                        return false;
                    }
                    struct_locals.insert(name, struct_name);
                    return true;
                }
                if let Some(struct_name) = check_struct_returning_call(
                    program,
                    &v,
                    locals,
                    struct_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                ) {
                    struct_locals.insert(name, struct_name);
                    return true;
                }
            }
            let declared = match (type_decl.as_ref(), value) {
                (Some(td), _) => match ScalarTy::from_type_decl(td) {
                    Some(t) => t,
                    None => return false,
                },
                (None, Some(v)) => match check_expr(program, &v, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                    Some(t) => t,
                    None => return false,
                },
                (None, None) => return false,
            };
            if let Some(v) = value {
                if type_decl.is_some() {
                    register_ptr_read_hint(program, &v, declared, ptr_read_hints);
                }
                let val_ty = match check_expr(program, &v, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                    Some(t) => t,
                    None => return false,
                };
                if val_ty != declared {
                    return false;
                }
            }
            if declared == ScalarTy::Unit {
                return false;
            }
            locals.insert(name, declared);
            true
        }
        Stmt::Return(value) => {
            if let Some(v) = value {
                check_expr(program, &v, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason).is_some()
            } else {
                true
            }
        }
        Stmt::Break | Stmt::Continue => true,
        Stmt::For(var, start, end, block) => {
            let start_ty = match check_expr(program, &start, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            let end_ty = match check_expr(program, &end, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
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
                check_expr(program, &block, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason).is_some();
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
        Stmt::While(cond, block) => {
            let cond_ty = match check_expr(program, &cond, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            if cond_ty != ScalarTy::Bool {
                return false;
            }
            check_expr(program, &block, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason).is_some()
        }
        // No struct / impl / enum declarations are tolerated inside an
        // eligible function body. Top-level decls live outside of any
        // function so they don't affect us here.
        Stmt::StructDecl { .. } | Stmt::ImplBlock { .. } | Stmt::EnumDecl { .. } => false,
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
    struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
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
        Expr::True | Expr::False => Some(ScalarTy::Bool),
        Expr::Identifier(sym) => locals.get(&sym).copied(),
        Expr::Binary(op, lhs, rhs) => {
            let lt = check_expr(program, &lhs, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            let rt = check_expr(program, &rhs, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            if lt != rt {
                return None;
            }
            match op {
                Operator::IAdd | Operator::ISub | Operator::IMul | Operator::IDiv => {
                    if matches!(lt, ScalarTy::I64 | ScalarTy::U64) {
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
                    if matches!(lt, ScalarTy::I64 | ScalarTy::U64) {
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
                    if matches!(lt, ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Bool) {
                        Some(lt)
                    } else {
                        None
                    }
                }
                Operator::LeftShift | Operator::RightShift => {
                    if matches!(lt, ScalarTy::I64 | ScalarTy::U64) {
                        Some(lt)
                    } else {
                        None
                    }
                }
            }
        }
        Expr::Unary(op, operand) => {
            let t = check_expr(program, &operand, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
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
                    // already, but be defensive: only allow signed ints.
                    if t == ScalarTy::I64 {
                        Some(ScalarTy::I64)
                    } else {
                        None
                    }
                }
            }
        }
        Expr::Block(stmts) => {
            let mut last_ty = ScalarTy::Unit;
            let mut snapshot = locals.clone();
            for s in &stmts {
                let stmt = program.statement.get(s)?;
                if let Stmt::Expression(e) = &stmt {
                    last_ty = check_expr(program, e, &mut snapshot, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                } else {
                    if !check_stmt(program, s, &mut snapshot, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    last_ty = ScalarTy::Unit;
                }
            }
            Some(last_ty)
        }
        Expr::IfElifElse(cond, if_block, elif_pairs, else_block) => {
            let ct = check_expr(program, &cond, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            if ct != ScalarTy::Bool {
                return None;
            }
            let then_ty = check_expr(program, &if_block, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            for (ec, eb) in &elif_pairs {
                let et = check_expr(program, ec, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                if et != ScalarTy::Bool {
                    return None;
                }
                let bt = check_expr(program, eb, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                if bt != then_ty {
                    return None;
                }
            }
            let else_ty = check_expr(program, &else_block, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            // Allow if-without-else: the parser inserts an empty Block whose
            // type is Unit. Permit it only when both branches are Unit.
            if else_ty == then_ty {
                Some(then_ty)
            } else if then_ty == ScalarTy::Unit && else_ty == ScalarTy::Unit {
                Some(ScalarTy::Unit)
            } else {
                None
            }
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
                        struct_locals,
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
                    let struct_name = match struct_locals.get(&recv_name).copied() {
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
                        struct_locals,
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
        Expr::Call(name, args_ref) => {
            let args_expr = program.expression.get(&args_ref)?;
            let arg_list = match args_expr {
                Expr::ExprList(v) => v,
                _ => return None,
            };

            // Locate the callee in the program's function table.
            let callee = program.function.iter().find(|f| f.name == name).cloned();
            let callee = match callee {
                Some(f) => f,
                None => {
                    note(reject_reason, || "calls an unknown function".to_string());
                    return None;
                }
            };

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
                // Determine the declared param type up front so we can
                // distinguish scalar vs struct expected shape. Generic
                // params resolve via inference below.
                let arg_expr = program.expression.get(a)?;
                if let Expr::Identifier(id) = arg_expr {
                    if let Some(struct_name) = struct_locals.get(&id).copied() {
                        // Struct argument: callee's param must be a
                        // matching struct type.
                        match param_td {
                            TypeDecl::Identifier(s) | TypeDecl::Struct(s, _)
                                if *s == struct_name && struct_layouts.contains_key(s) =>
                            {
                                callee_param_tys.push(ParamTy::Struct(struct_name));
                                scalar_arg_tys.push(ScalarTy::Unit); // placeholder
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
                }
                // Fall back to scalar typing.
                let t = check_expr(
                    program,
                    a,
                    locals,
                    struct_locals,
                    substitutions,
                    struct_layouts,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                )?;
                scalar_arg_tys.push(t);
                callee_param_tys.push(ParamTy::Scalar(t));
            }

            // Infer substitutions for any generic params from the scalar
            // arg types. Struct args contribute placeholders that
            // `infer_substitutions` skips because the callee's param
            // type is concrete (not Generic).
            let callee_subs = match infer_substitutions(
                &callee,
                &scalar_arg_tys,
                substitutions,
                reject_reason,
            ) {
                Some(s) => s,
                None => return None,
            };

            // Build the ordered substitution vec (MonoKey tail) and record
            // the call site so codegen can resolve it later.
            let mono_args: Vec<ScalarTy> = callee
                .generic_params
                .iter()
                .map(|g| callee_subs.get(g).copied().unwrap_or(ScalarTy::Unit))
                .collect();
            callees.push(MonoCall {
                call_expr: *expr_ref,
                callee_name: name,
                mono_args,
            });

            // The struct-arg placeholders we put in `scalar_arg_tys` keep
            // the inferer happy; they must agree with the callee's
            // declared (non-generic) struct types, which we already
            // verified above.
            let _ = callee_param_tys;

            // Compute callee's substituted return type. Struct returns
            // aren't supported yet — substitute_to_scalar returns None
            // for `TypeDecl::Identifier(struct)` so such calls reject.
            match &callee.return_type {
                Some(td) => substitute_to_scalar(td, &callee_subs),
                None => Some(ScalarTy::Unit),
            }
        }
        Expr::BuiltinCall(func, args) => {
            // Type-check each argument against an expected ScalarTy.
            let check_args = |expected: &[ScalarTy],
                              args: &Vec<ExprRef>,
                              locals: &mut HashMap<DefaultSymbol, ScalarTy>,
                              struct_locals: &mut HashMap<DefaultSymbol, DefaultSymbol>,
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
                        struct_locals,
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
                BuiltinFunction::Print | BuiltinFunction::Println => {
                    if args.len() != 1 {
                        return None;
                    }
                    let t = check_expr(program, &args[0], locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                    if !matches!(t, ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Bool) {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::HeapAlloc => {
                    if !check_args(&[ScalarTy::U64], &args, locals, struct_locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Ptr)
                }
                BuiltinFunction::HeapFree => {
                    if !check_args(&[ScalarTy::Ptr], &args, locals, struct_locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::HeapRealloc => {
                    if !check_args(&[ScalarTy::Ptr, ScalarTy::U64], &args, locals, struct_locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Ptr)
                }
                BuiltinFunction::PtrIsNull => {
                    if !check_args(&[ScalarTy::Ptr], &args, locals, struct_locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Bool)
                }
                BuiltinFunction::MemCopy | BuiltinFunction::MemMove => {
                    if !check_args(
                        &[ScalarTy::Ptr, ScalarTy::Ptr, ScalarTy::U64],
                        &args,
                        locals,
                    struct_locals,
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
                    struct_locals,
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
                    struct_locals,
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
                        struct_locals,
                        substitutions,
                        struct_layouts,
                        callees,
                        ptr_read_hints,
                        reject_reason,
                    )?;
                    if !matches!(
                        t,
                        ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Bool | ScalarTy::Ptr
                    ) {
                        note(reject_reason, || {
                            format!("__builtin_sizeof of {t:?} is not supported in JIT")
                        });
                        return None;
                    }
                    Some(ScalarTy::U64)
                }
                BuiltinFunction::PtrWrite => {
                    if args.len() != 3 {
                        return None;
                    }
                    let p = check_expr(program, &args[0], locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                    let off = check_expr(program, &args[1], locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
                    let v = check_expr(program, &args[2], locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
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
                other => {
                    note(reject_reason, || {
                        format!("uses unsupported builtin {other:?}")
                    });
                    None
                }
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
            let struct_name = match struct_locals.get(&recv_name).copied() {
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
        Expr::Cast(inner, target) => {
            // Match the interpreter: only i64 ↔ u64 (or identity for those
            // two) is permitted. bool casts are intentionally excluded.
            let inner_ty = check_expr(program, &inner, locals, struct_locals, substitutions, struct_layouts, callees, ptr_read_hints, reject_reason)?;
            let target_ty = ScalarTy::from_type_decl(&target)?;
            if !matches!(inner_ty, ScalarTy::I64 | ScalarTy::U64) {
                return None;
            }
            if !matches!(target_ty, ScalarTy::I64 | ScalarTy::U64) {
                return None;
            }
            Some(target_ty)
        }
        // Everything else is unsupported in this iteration.
        other => {
            note(reject_reason, || {
                format!("uses unsupported expression {}", expr_kind_name(&other))
            });
            None
        }
    }
}

/// Short human-readable name of an Expr variant for reject messages.
fn expr_kind_name(e: &Expr) -> &'static str {
    match e {
        Expr::Number(_) => "untyped numeric literal",
        Expr::Null => "null",
        Expr::ExprList(_) => "expression list",
        Expr::String(_) => "string literal",
        Expr::ArrayLiteral(_) => "array literal",
        Expr::FieldAccess(_, _) => "field access",
        Expr::MethodCall(_, _, _) => "method call",
        Expr::StructLiteral(_, _) => "struct literal",
        Expr::QualifiedIdentifier(_) => "qualified identifier",
        Expr::BuiltinMethodCall(_, _, _) => "builtin method call",
        Expr::SliceAccess(_, _) => "slice access",
        Expr::SliceAssign(_, _, _, _) => "slice assign",
        Expr::AssociatedFunctionCall(_, _, _) => "associated function call",
        Expr::DictLiteral(_) => "dict literal",
        Expr::TupleLiteral(_) => "tuple literal",
        Expr::TupleAccess(_, _) => "tuple access",
        Expr::With(_, _) => "`with allocator` block",
        Expr::Match(_, _) => "match expression",
        Expr::Range(_, _) => "range value",
        _ => "expression",
    }
}
