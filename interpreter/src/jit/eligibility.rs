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

use frontend::ast::{Expr, ExprRef, Function, Operator, Program, Stmt, StmtRef, UnaryOp};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

/// JIT-supported scalar types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarTy {
    I64,
    U64,
    Bool,
    Unit,
}

impl ScalarTy {
    pub fn from_type_decl(td: &TypeDecl) -> Option<Self> {
        match td {
            TypeDecl::Int64 => Some(ScalarTy::I64),
            TypeDecl::UInt64 => Some(ScalarTy::U64),
            TypeDecl::Bool => Some(ScalarTy::Bool),
            TypeDecl::Unit => Some(ScalarTy::Unit),
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

/// Result of eligibility analysis: every transitively reachable function
/// from `main` together with its scalar signature. The caller compiles each
/// of these in turn.
pub struct EligibleSet {
    pub functions: HashMap<DefaultSymbol, Rc<Function>>,
    pub signatures: HashMap<DefaultSymbol, FuncSignature>,
}

pub fn analyze(
    program: &Program,
    main: &Rc<Function>,
) -> Option<EligibleSet> {
    let mut function_map: HashMap<DefaultSymbol, Rc<Function>> = HashMap::new();
    for f in &program.function {
        function_map.insert(f.name, f.clone());
    }

    let mut visited: HashSet<DefaultSymbol> = HashSet::new();
    let mut signatures: HashMap<DefaultSymbol, FuncSignature> = HashMap::new();
    let mut eligible_funcs: HashMap<DefaultSymbol, Rc<Function>> = HashMap::new();
    let mut stack: Vec<Rc<Function>> = vec![main.clone()];

    while let Some(func) = stack.pop() {
        if !visited.insert(func.name) {
            continue;
        }

        let sig = match function_signature(&func) {
            Some(s) => s,
            None => return None,
        };

        // Generic functions are not supported.
        if !func.generic_params.is_empty() {
            return None;
        }

        let mut callees: Vec<DefaultSymbol> = Vec::new();
        if !check_function_body(program, &func, &sig, &mut callees) {
            return None;
        }

        signatures.insert(func.name, sig);
        eligible_funcs.insert(func.name, func.clone());

        for callee in callees {
            if let Some(callee_fn) = function_map.get(&callee) {
                stack.push(callee_fn.clone());
            } else {
                // Unknown callee (could be a method, builtin, or something we
                // don't recognize). Bail out.
                return None;
            }
        }
    }

    Some(EligibleSet {
        functions: eligible_funcs,
        signatures,
    })
}

fn function_signature(func: &Function) -> Option<FuncSignature> {
    let mut params = Vec::with_capacity(func.parameter.len());
    for (name, td) in &func.parameter {
        let st = ScalarTy::from_type_decl(td)?;
        if st == ScalarTy::Unit {
            return None;
        }
        params.push((*name, st));
    }
    let ret = match &func.return_type {
        Some(td) => ScalarTy::from_type_decl(td)?,
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
    callees: &mut Vec<DefaultSymbol>,
) -> bool {
    let mut locals: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
    for (n, t) in &sig.params {
        locals.insert(*n, *t);
    }
    check_stmt(program, &func.code, &mut locals, callees)
}

fn check_stmt(
    program: &Program,
    stmt_ref: &StmtRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    callees: &mut Vec<DefaultSymbol>,
) -> bool {
    let stmt = match program.statement.get(stmt_ref) {
        Some(s) => s,
        None => return false,
    };
    match stmt {
        Stmt::Expression(e) => check_expr(program, &e, locals, callees).is_some(),
        Stmt::Val(name, type_decl, value) => {
            let val_ty = match check_expr(program, &value, locals, callees) {
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
                (None, Some(v)) => match check_expr(program, &v, locals, callees) {
                    Some(t) => t,
                    None => return false,
                },
                (None, None) => return false,
            };
            if let Some(v) = value {
                let val_ty = match check_expr(program, &v, locals, callees) {
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
                check_expr(program, &v, locals, callees).is_some()
            } else {
                true
            }
        }
        Stmt::Break | Stmt::Continue => true,
        Stmt::For(var, start, end, block) => {
            let start_ty = match check_expr(program, &start, locals, callees) {
                Some(t) => t,
                None => return false,
            };
            let end_ty = match check_expr(program, &end, locals, callees) {
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
            let body_ok = check_expr(program, &block, locals, callees).is_some();
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
            let cond_ty = match check_expr(program, &cond, locals, callees) {
                Some(t) => t,
                None => return false,
            };
            if cond_ty != ScalarTy::Bool {
                return false;
            }
            check_expr(program, &block, locals, callees).is_some()
        }
        // No struct / impl / enum declarations are tolerated inside an
        // eligible function body. Top-level decls live outside of any
        // function so they don't affect us here.
        Stmt::StructDecl { .. } | Stmt::ImplBlock { .. } | Stmt::EnumDecl { .. } => false,
    }
}

/// Returns the type produced by the expression, or `None` if the expression
/// uses an unsupported construct. As a side effect, populates `callees` with
/// names of user-defined functions invoked by this expression.
pub(crate) fn check_expr(
    program: &Program,
    expr_ref: &ExprRef,
    locals: &mut HashMap<DefaultSymbol, ScalarTy>,
    callees: &mut Vec<DefaultSymbol>,
) -> Option<ScalarTy> {
    let expr = program.expression.get(expr_ref)?;
    match expr {
        Expr::Int64(_) => Some(ScalarTy::I64),
        Expr::UInt64(_) => Some(ScalarTy::U64),
        Expr::True | Expr::False => Some(ScalarTy::Bool),
        Expr::Identifier(sym) => locals.get(&sym).copied(),
        Expr::Binary(op, lhs, rhs) => {
            let lt = check_expr(program, &lhs, locals, callees)?;
            let rt = check_expr(program, &rhs, locals, callees)?;
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
            let t = check_expr(program, &operand, locals, callees)?;
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
                    last_ty = check_expr(program, e, &mut snapshot, callees)?;
                } else {
                    if !check_stmt(program, s, &mut snapshot, callees) {
                        return None;
                    }
                    last_ty = ScalarTy::Unit;
                }
            }
            Some(last_ty)
        }
        Expr::IfElifElse(cond, if_block, elif_pairs, else_block) => {
            let ct = check_expr(program, &cond, locals, callees)?;
            if ct != ScalarTy::Bool {
                return None;
            }
            let then_ty = check_expr(program, &if_block, locals, callees)?;
            for (ec, eb) in &elif_pairs {
                let et = check_expr(program, ec, locals, callees)?;
                if et != ScalarTy::Bool {
                    return None;
                }
                let bt = check_expr(program, eb, locals, callees)?;
                if bt != then_ty {
                    return None;
                }
            }
            let else_ty = check_expr(program, &else_block, locals, callees)?;
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
            let rhs_ty = check_expr(program, &rhs, locals, callees)?;
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
            for a in &arg_list {
                check_expr(program, a, locals, callees)?;
            }
            callees.push(name);
            // The caller will resolve the callee's return type after the
            // function is itself confirmed eligible. For now, assume the
            // callee resolves correctly; if it doesn't, the outer analysis
            // bails out, in which case eligibility is denied for the whole
            // program. Use Unit as a placeholder — but this would break type
            // checks in containing expressions, so look up the callee's
            // declared return type from the program directly.
            for f in &program.function {
                if f.name == name {
                    return f
                        .return_type
                        .as_ref()
                        .and_then(ScalarTy::from_type_decl)
                        .or(Some(ScalarTy::Unit));
                }
            }
            None
        }
        Expr::Cast(inner, target) => {
            // Match the interpreter: only i64 ↔ u64 (or identity for those
            // two) is permitted. bool casts are intentionally excluded.
            let inner_ty = check_expr(program, &inner, locals, callees)?;
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
        Expr::Number(_)
        | Expr::Null
        | Expr::ExprList(_)
        | Expr::String(_)
        | Expr::ArrayLiteral(_)
        | Expr::FieldAccess(_, _)
        | Expr::MethodCall(_, _, _)
        | Expr::StructLiteral(_, _)
        | Expr::QualifiedIdentifier(_)
        | Expr::BuiltinMethodCall(_, _, _)
        | Expr::BuiltinCall(_, _)
        | Expr::SliceAccess(_, _)
        | Expr::SliceAssign(_, _, _, _)
        | Expr::AssociatedFunctionCall(_, _, _)
        | Expr::DictLiteral(_)
        | Expr::TupleLiteral(_)
        | Expr::TupleAccess(_, _)
        | Expr::With(_, _)
        | Expr::Match(_, _)
        | Expr::Range(_, _) => None,
    }
}
