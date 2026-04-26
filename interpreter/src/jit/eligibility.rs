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

/// Signature of an eligible function in JIT-friendly form.
#[derive(Debug, Clone)]
pub struct FuncSignature {
    pub params: Vec<(DefaultSymbol, ScalarTy)>,
    pub ret: ScalarTy,
}

/// Identifies a single monomorphization of a function. The `Vec<ScalarTy>`
/// is the substitution list ordered by `Function::generic_params`; it's
/// empty for non-generic functions, so a non-generic function has exactly
/// one MonoKey `(name, vec![])`.
pub type MonoKey = (DefaultSymbol, Vec<ScalarTy>);

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
        let sig = match function_signature(&func, &substitutions, &mut sig_reason) {
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
    })
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
    reject_reason: &mut Option<String>,
) -> Option<FuncSignature> {
    let mut params = Vec::with_capacity(func.parameter.len());
    for (_, td) in &func.parameter {
        let st = match substitute_to_scalar(td, substitutions) {
            Some(s) => s,
            None => {
                note(reject_reason, || {
                    format!("parameter has unsupported type {td:?}")
                });
                return None;
            }
        };
        if st == ScalarTy::Unit {
            note(reject_reason, || {
                "parameter type Unit is not supported".to_string()
            });
            return None;
        }
        params.push((func.parameter[params.len()].0, st));
    }
    let ret = match &func.return_type {
        Some(td) => match substitute_to_scalar(td, substitutions) {
            Some(s) => s,
            None => {
                note(reject_reason, || {
                    format!("return type {td:?} is not supported")
                });
                return None;
            }
        },
        None => ScalarTy::Unit,
    };
    Some(FuncSignature { params, ret })
}

/// Walks a function body to confirm it only uses supported constructs and
/// reports every callee found via `callees`. Returns false on the first
/// unsupported construct.
fn check_function_body(
    program: &Program,
    func: &Function,
    sig: &FuncSignature,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
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
    for (n, t) in &sig.params {
        locals.insert(*n, *t);
    }
    check_stmt(
        program,
        &func.code,
        &mut locals,
        substitutions,
        callees,
        ptr_read_hints,
        reject_reason,
    )
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
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
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
            check_expr(program, &e, locals, substitutions, callees, ptr_read_hints, reject_reason).is_some()
        }
        Stmt::Val(name, type_decl, value) => {
            let declared_hint = type_decl.as_ref().and_then(ScalarTy::from_type_decl);
            // If both the annotation and the RHS are PtrRead-shaped, record
            // the expected return type before recursing so check_expr can
            // accept the otherwise type-polymorphic builtin.
            if let Some(t) = declared_hint {
                register_ptr_read_hint(program, &value, t, ptr_read_hints);
            }
            let val_ty = match check_expr(program, &value, locals, substitutions, callees, ptr_read_hints, reject_reason) {
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
            let declared = match (type_decl.as_ref(), value) {
                (Some(td), _) => match ScalarTy::from_type_decl(td) {
                    Some(t) => t,
                    None => return false,
                },
                (None, Some(v)) => match check_expr(program, &v, locals, substitutions, callees, ptr_read_hints, reject_reason) {
                    Some(t) => t,
                    None => return false,
                },
                (None, None) => return false,
            };
            if let Some(v) = value {
                if type_decl.is_some() {
                    register_ptr_read_hint(program, &v, declared, ptr_read_hints);
                }
                let val_ty = match check_expr(program, &v, locals, substitutions, callees, ptr_read_hints, reject_reason) {
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
                check_expr(program, &v, locals, substitutions, callees, ptr_read_hints, reject_reason).is_some()
            } else {
                true
            }
        }
        Stmt::Break | Stmt::Continue => true,
        Stmt::For(var, start, end, block) => {
            let start_ty = match check_expr(program, &start, locals, substitutions, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            let end_ty = match check_expr(program, &end, locals, substitutions, callees, ptr_read_hints, reject_reason) {
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
                check_expr(program, &block, locals, substitutions, callees, ptr_read_hints, reject_reason).is_some();
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
            let cond_ty = match check_expr(program, &cond, locals, substitutions, callees, ptr_read_hints, reject_reason) {
                Some(t) => t,
                None => return false,
            };
            if cond_ty != ScalarTy::Bool {
                return false;
            }
            check_expr(program, &block, locals, substitutions, callees, ptr_read_hints, reject_reason).is_some()
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
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
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
            let lt = check_expr(program, &lhs, locals, substitutions, callees, ptr_read_hints, reject_reason)?;
            let rt = check_expr(program, &rhs, locals, substitutions, callees, ptr_read_hints, reject_reason)?;
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
            let t = check_expr(program, &operand, locals, substitutions, callees, ptr_read_hints, reject_reason)?;
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
                    last_ty = check_expr(program, e, &mut snapshot, substitutions, callees, ptr_read_hints, reject_reason)?;
                } else {
                    if !check_stmt(program, s, &mut snapshot, substitutions, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    last_ty = ScalarTy::Unit;
                }
            }
            Some(last_ty)
        }
        Expr::IfElifElse(cond, if_block, elif_pairs, else_block) => {
            let ct = check_expr(program, &cond, locals, substitutions, callees, ptr_read_hints, reject_reason)?;
            if ct != ScalarTy::Bool {
                return None;
            }
            let then_ty = check_expr(program, &if_block, locals, substitutions, callees, ptr_read_hints, reject_reason)?;
            for (ec, eb) in &elif_pairs {
                let et = check_expr(program, ec, locals, substitutions, callees, ptr_read_hints, reject_reason)?;
                if et != ScalarTy::Bool {
                    return None;
                }
                let bt = check_expr(program, eb, locals, substitutions, callees, ptr_read_hints, reject_reason)?;
                if bt != then_ty {
                    return None;
                }
            }
            let else_ty = check_expr(program, &else_block, locals, substitutions, callees, ptr_read_hints, reject_reason)?;
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
            // Only assignment to an identifier (a previously declared local)
            // is supported.
            let lhs_expr = program.expression.get(&lhs)?;
            let name = match lhs_expr {
                Expr::Identifier(s) => s,
                _ => return None,
            };
            let lhs_ty = locals.get(&name).copied()?;
            let rhs_ty = check_expr(program, &rhs, locals, substitutions, callees, ptr_read_hints, reject_reason)?;
            if rhs_ty != lhs_ty {
                return None;
            }
            Some(ScalarTy::Unit)
        }
        Expr::Call(name, args_ref) => {
            let args_expr = program.expression.get(&args_ref)?;
            let arg_list = match args_expr {
                Expr::ExprList(v) => v,
                _ => return None,
            };
            let mut arg_tys: Vec<ScalarTy> = Vec::with_capacity(arg_list.len());
            for a in &arg_list {
                let t = check_expr(
                    program,
                    a,
                    locals,
                    substitutions,
                    callees,
                    ptr_read_hints,
                    reject_reason,
                )?;
                arg_tys.push(t);
            }

            // Locate the callee in the program's function table.
            let callee = program.function.iter().find(|f| f.name == name).cloned();
            let callee = match callee {
                Some(f) => f,
                None => {
                    note(reject_reason, || "calls an unknown function".to_string());
                    return None;
                }
            };

            // Infer substitutions for any generic params from arg types.
            let callee_subs = match infer_substitutions(
                &callee,
                &arg_tys,
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

            // Compute callee's substituted return type.
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
                    match check_expr(program, a, locals, substitutions, callees, ptr_read_hints, reject_reason) {
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
                    let t = check_expr(program, &args[0], locals, substitutions, callees, ptr_read_hints, reject_reason)?;
                    if !matches!(t, ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Bool) {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::HeapAlloc => {
                    if !check_args(&[ScalarTy::U64], &args, locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Ptr)
                }
                BuiltinFunction::HeapFree => {
                    if !check_args(&[ScalarTy::Ptr], &args, locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Unit)
                }
                BuiltinFunction::HeapRealloc => {
                    if !check_args(&[ScalarTy::Ptr, ScalarTy::U64], &args, locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Ptr)
                }
                BuiltinFunction::PtrIsNull => {
                    if !check_args(&[ScalarTy::Ptr], &args, locals, callees, ptr_read_hints, reject_reason) {
                        return None;
                    }
                    Some(ScalarTy::Bool)
                }
                BuiltinFunction::MemCopy | BuiltinFunction::MemMove => {
                    if !check_args(
                        &[ScalarTy::Ptr, ScalarTy::Ptr, ScalarTy::U64],
                        &args,
                        locals,
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
                        substitutions,
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
                    let p = check_expr(program, &args[0], locals, substitutions, callees, ptr_read_hints, reject_reason)?;
                    let off = check_expr(program, &args[1], locals, substitutions, callees, ptr_read_hints, reject_reason)?;
                    let v = check_expr(program, &args[2], locals, substitutions, callees, ptr_read_hints, reject_reason)?;
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
        Expr::Cast(inner, target) => {
            // Match the interpreter: only i64 ↔ u64 (or identity for those
            // two) is permitted. bool casts are intentionally excluded.
            let inner_ty = check_expr(program, &inner, locals, substitutions, callees, ptr_read_hints, reject_reason)?;
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
