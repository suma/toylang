//! COMPILE-TIME-EVAL C3: run `const fn` calls while compiling and
//! leave literals behind.
//!
//! ## Where this sits, and why
//!
//! The pass runs in the driver — after type checking, before any
//! lowering — and rewrites the AST in place. Every backend therefore
//! sees a literal where the call was, exactly as `STRUCT-UPDATE` and
//! `NEWTYPE` desugar before lowering: no backend needs a line of code
//! for this feature, and no backend can disagree about the answer.
//!
//! The evaluator is the **tree-walker**, on purpose. This language
//! already runs one set of semantics on four engines, and a fifth
//! written for compile time is how "2u64 * 3u64 means something else
//! in a `const`" gets in. `CLAUDE.md` names the tree-walker as the
//! oracle whenever one is needed, and a compile-time fold is exactly
//! that. (`COMPILE_TIME_EVAL.md` C6 moves this onto the IR VM, which
//! removes even the possibility of drift, once the VM can be lifted
//! out of the interpreter crate.)
//!
//! Before this pass, `const D: u64 = double(21u64)` ran on the
//! tree-walker and failed to compile on the other three, because the
//! initialiser was folded by a *second*, weaker evaluator in
//! `compiler_lower::consts`. That is the drift this exists to end.
//!
//! ## Forced and opportunistic folds
//!
//! Two positions ask for a fold, and they treat failure differently:
//!
//! - **Forced** — a `const NAME: T = ...` initialiser. Every one of
//!   them: the value has to exist before the program runs, so a trap,
//!   a panic, a spent step budget, or a callee that is not a
//!   `const fn` is a **compile error**. Before this, an initialiser
//!   that traps was a run-time panic on the tree-walker and a
//!   silently wrapped value on the other three, because
//!   `compiler_lower::consts` folded it with the host's wrapping
//!   arithmetic and no guard.
//! - **Opportunistic** — an ordinary call with constant arguments.
//!   Folding it is an optimisation, so a failure just means the call
//!   stays and runs at run time, with whatever behaviour it would
//!   have had.
//!
//! Keeping the two apart is what lets `if false { boom(1u64) }` stay
//! legal: nothing forced that call, so nothing reports its trap
//! (`COMPILE_TIME_EVAL.md` 論点 6). It is also C++'s rule — a
//! `constexpr` function called outside a constant context is an
//! ordinary call.
//!
//! ## What can come back
//!
//! Scalars only: `bool`, the six narrow integer widths, `i64`, `u64`,
//! `f64`. That is not a policy but the shape of `compiler_ir::Const`,
//! which is where a folded value has to live for the compiled
//! backends. A `const fn` returning a `str`, a struct, or a `Vec` is
//! left alone by the opportunistic path and reported by the forced
//! one.

use std::collections::HashSet;

use frontend::ast::{Expr, ExprRef, File};
use frontend::type_checker::error::TypeCheckError;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::evaluation::EvaluationResult;
use crate::object::Object;

/// Loop iterations one fold may spend before it is called
/// non-terminating. Shared with nothing: `--check` trials get a much
/// smaller allowance because they run thousands of times, while a
/// fold runs once and a user who wrote a compile-time loop expects it
/// to finish.
const FOLD_STEP_BUDGET: u64 = 1_000_000;

/// Fold what can be folded, and report what had to be and could not.
///
/// Returns an empty vector for the overwhelming majority of programs:
/// with no `const fn` declared and no call in a `const` initialiser
/// there is nothing to do, and the scan that establishes that is a
/// pass over the function list.
pub fn fold_const_evaluations(
    program: &mut File,
    string_interner: &DefaultStringInterner,
) -> Vec<TypeCheckError> {
    let const_fns: HashSet<DefaultSymbol> = program
        .function
        .iter()
        .filter(|f| f.const_fn && !f.is_extern)
        .map(|f| f.name)
        .collect();

    if const_fns.is_empty() && program.consts.is_empty() {
        return Vec::new();
    }

    // MEMORY_PROFILING M0: the compiler's own allocations are not the
    // program's. A run zeroes the counters when it starts, but the
    // in-process lanes (`--test`, the consistency harness) read them
    // across a type-check, so the snapshot is restored rather than
    // relied upon.
    let profile_before = crate::heap::snapshot_profile();
    let result = evaluate(program, string_interner, &const_fns);
    crate::heap::restore_profile(profile_before);

    let (rewrites, errors) = result;
    for (expr_ref, literal) in rewrites {
        program.expression.update(&expr_ref, literal);
    }
    errors
}

/// Everything that needs the evaluation context, kept in one scope so
/// the immutable borrow of `program` ends before the rewrites land.
fn evaluate(
    program: &File,
    string_interner: &DefaultStringInterner,
    const_fns: &HashSet<DefaultSymbol>,
) -> (Vec<(ExprRef, Expr)>, Vec<TypeCheckError>) {
    let mut rewrites: Vec<(ExprRef, Expr)> = Vec::new();
    let mut errors: Vec<TypeCheckError> = Vec::new();

    let mut interner = string_interner.clone();
    let shared = match crate::SharedRunData::new(program, &mut interner) {
        Ok(shared) => shared,
        // The program does not even have a runnable shape; the
        // ordinary diagnostics say why, and folding is not the place
        // to repeat it.
        Err(_) => return (rewrites, errors),
    };
    let mut eval = crate::evaluation::EvaluationContext::new_with_shared(
        &program.statement,
        &program.expression,
        &mut interner,
        &shared,
    );
    eval.location_pool = Some(&program.location_pool);
    crate::initialize_module_environment(&mut eval, program);
    eval.set_step_budget(Some(FOLD_STEP_BUDGET));

    // Pass 1 — top-level consts, in declaration order (a later
    // initialiser may name an earlier one). All of them are forced:
    // the program cannot start without the value.
    //
    // A scalar result replaces the initialiser, so every backend gets
    // a literal and `compiler_lower::consts` has nothing left to
    // evaluate. A `str` (or any other non-scalar) is left as written —
    // there is no literal for it to become, and it was never the
    // shape that drifted.
    //
    // The loop stops at the first failure rather than reporting the
    // rest: consts chain, so one broken initialiser makes every later
    // one that names it fail too, and the cascade would bury the
    // cause.
    for decl in program.consts.iter() {
        let name = string_interner.resolve(decl.name).unwrap_or("?").to_string();
        let context = format!("const {name}");

        // Check the callees first: "not a `const fn`" is a far better
        // message than whatever the evaluator would say, and for a
        // call the evaluator cannot even reach (an `extern`) there
        // would be no message at all.
        if let Some(detail) = uncallable_reason(program, &decl.value, const_fns, string_interner) {
            errors.push(err_at(program, &decl.value, &context, detail));
            break;
        }

        let value = match eval.evaluate(&decl.value) {
            Ok(EvaluationResult::Value(v)) => v.into_rc(),
            Ok(_) => {
                errors.push(err_at(
                    program,
                    &decl.value,
                    &context,
                    "its initialiser does not produce a value".to_string(),
                ));
                break;
            }
            Err(e) => {
                errors.push(err_at(program, &decl.value, &context, describe(&e)));
                break;
            }
        };

        if let Some(literal) = literal_for(&value.borrow()) {
            rewrites.push((decl.value, literal));
        }
        eval.environment.set_val(decl.name, value.into());
    }

    // Pass 2 — opportunistic. Every `const fn` call whose arguments
    // are all literals, wherever it appears. A flat scan of the
    // expression pool rather than a walk of every body: the pool is
    // the complete set of expressions, and a fold is valid at any of
    // them.
    if !const_fns.is_empty() {
        for index in 0..program.expression.len() {
            let expr_ref = ExprRef(index as u32);
            let Some(Expr::Call(callee, args)) = program.expression.get(&expr_ref) else {
                continue;
            };
            if !const_fns.contains(&callee) || !all_literal_args(program, &args) {
                continue;
            }
            let Ok(EvaluationResult::Value(v)) = eval.evaluate(&expr_ref) else {
                // Opportunistic: a fold that traps, panics, or runs
                // out of budget leaves the call in place and lets run
                // time have the same behaviour it always had.
                continue;
            };
            if let Some(literal) = literal_for(&v.into_rc().borrow()) {
                rewrites.push((expr_ref, literal));
            }
        }
    }

    (rewrites, errors)
}

/// The literal a folded value becomes, or `None` when the value has no
/// literal form (a `str`, a struct, a `Vec`, unit).
fn literal_for(object: &Object) -> Option<Expr> {
    Some(match object {
        Object::Bool(true) => Expr::True,
        Object::Bool(false) => Expr::False,
        Object::Int64(v) => Expr::Int64(*v),
        Object::UInt64(v) => Expr::UInt64(*v),
        Object::Int8(v) => Expr::Int8(*v),
        Object::Int16(v) => Expr::Int16(*v),
        Object::Int32(v) => Expr::Int32(*v),
        Object::UInt8(v) => Expr::UInt8(*v),
        Object::UInt16(v) => Expr::UInt16(*v),
        Object::UInt32(v) => Expr::UInt32(*v),
        Object::Float64(v) => Expr::Float64(*v),
        _ => return None,
    })
}

/// Are all of a call's arguments literals?
///
/// Literals only — not names. A name in a body could be a local that
/// happens to share a top-level const's name, and folding it against
/// the const's value would be a miscompile. The `const` initialisers
/// of pass 1 have no locals to shadow anything, which is why they can
/// afford to be more generous.
fn all_literal_args(program: &File, args: &ExprRef) -> bool {
    let Some(Expr::ExprList(items)) = program.expression.get(args) else {
        return false;
    };
    items.iter().all(|item| {
        matches!(
            program.expression.get(item),
            Some(
                Expr::True
                    | Expr::False
                    | Expr::Int64(_)
                    | Expr::UInt64(_)
                    | Expr::Int8(_)
                    | Expr::Int16(_)
                    | Expr::Int32(_)
                    | Expr::UInt8(_)
                    | Expr::UInt16(_)
                    | Expr::UInt32(_)
                    | Expr::Float64(_)
            )
        )
    })
}

/// Why this expression cannot be evaluated at compile time, judged by
/// the calls it makes rather than by running it. `None` means every
/// call in it names a `const fn`.
fn uncallable_reason(
    program: &File,
    expr_ref: &ExprRef,
    const_fns: &HashSet<DefaultSymbol>,
    interner: &DefaultStringInterner,
) -> Option<String> {
    let mut reason = None;
    walk(program, expr_ref, &mut |expr| {
        if reason.is_some() {
            return;
        }
        reason = match expr {
            Expr::Call(callee, _) if !const_fns.contains(callee) => Some(format!(
                "it calls `{}`, which is not declared `const fn`",
                interner.resolve(*callee).unwrap_or("?")
            )),
            Expr::MethodCall(_, name, _) | Expr::AssociatedFunctionCall(_, name, _) => {
                Some(format!(
                    "it calls `{}`, and only free functions can be declared `const fn`",
                    interner.resolve(*name).unwrap_or("?")
                ))
            }
            _ => None,
        };
    });
    reason
}

/// Visit `expr_ref` and every expression under it.
///
/// Deliberately shallow about statements: a `const` initialiser is an
/// expression, and the only block it could contain would come from a
/// closure, which cannot be folded anyway.
fn walk(program: &File, expr_ref: &ExprRef, visit: &mut dyn FnMut(&Expr)) {
    let Some(expr) = program.expression.get(expr_ref) else {
        return;
    };
    visit(&expr);
    let mut children: Vec<ExprRef> = Vec::new();
    match &expr {
        Expr::Call(_, args) => children.push(*args),
        Expr::MethodCall(receiver, _, args) => {
            children.push(*receiver);
            children.extend(args.iter().copied());
        }
        Expr::AssociatedFunctionCall(_, _, args) | Expr::BuiltinCall(_, args) => {
            children.extend(args.iter().copied())
        }
        Expr::Binary(_, lhs, rhs) => children.extend([*lhs, *rhs]),
        Expr::Unary(_, operand) => children.push(*operand),
        Expr::Cast(inner, _) => children.push(*inner),
        Expr::ExprList(items)
        | Expr::ArrayLiteral(items)
        | Expr::TupleLiteral(items) => children.extend(items.iter().copied()),
        Expr::StructLiteral(_, fields) => children.extend(fields.iter().map(|(_, v)| *v)),
        Expr::FieldAccess(obj, _) | Expr::TupleAccess(obj, _) => children.push(*obj),
        Expr::BuiltinMethodCall(receiver, _, args) => {
            children.push(*receiver);
            children.extend(args.iter().copied());
        }
        Expr::IfElifElse(cond, then_block, elifs, else_block) => {
            children.extend([*cond, *then_block, *else_block]);
            for (c, b) in elifs {
                children.extend([*c, *b]);
            }
        }
        _ => {}
    }
    for child in children {
        walk(program, &child, visit);
    }
}

/// A one-clause description of a failed fold, phrased to follow
/// "`const D` must be known at compile time, but ...".
fn describe(error: &crate::error::InterpreterError) -> String {
    match error {
        crate::error::InterpreterError::StepBudgetExceeded { steps } => format!(
            "evaluating it ran past {steps} loop iterations, so the compiler stopped \
             rather than assume it finishes"
        ),
        other => format!("evaluating it failed: {other}"),
    }
}

fn err_at(program: &File, expr_ref: &ExprRef, context: &str, detail: String) -> TypeCheckError {
    let mut error = TypeCheckError::const_eval(context.to_string(), detail);
    if let Some(location) = program.location_pool.get_expr_location(expr_ref) {
        error = error.with_location(*location);
    }
    error
}
