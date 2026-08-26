//! COMPILE-TIME-EVAL C3 + C6: run `const fn` calls while compiling and
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
//! The evaluator is the **IR VM** (`compiler_vm`), the same engine the
//! run-time fast path uses (COMPILE-TIME-EVAL C6). The fold lowers the
//! type-checked program — with the initialiser under evaluation moved
//! into a synthetic wrapper function — runs it through
//! `compiler_vm::run_function`, and rewrites the result into a
//! literal. Because the fold and the runtime execute the *same lowered
//! code* — same trap guards, same operator table, same RUNTIME-TRAP
//! semantics — a compile-time answer and a run-time answer cannot
//! differ by construction. Before C6 the evaluator was the
//! tree-walker: a second implementation of the same language, which is
//! exactly how a `const` and a run-time call used to disagree.
//!
//! Before C3, `const D: u64 = double(21u64)` ran on the tree-walker
//! and failed to compile on the other three, because the initialiser
//! was folded by a *second*, weaker evaluator in
//! `compiler_lower::consts`. That drift is what this pass exists to
//! end.
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
//!   silently wrapped value on the other three.
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
//!
//! ## Why the fold lowers the whole program
//!
//! `compiler_lower::lower_program` requires every top-level `const`
//! initialiser to be readable by `consts.rs` (a literal, or a
//! reference to an earlier const). While the fold is running, the
//! consts it has not reached yet still contain calls — so the fold
//! temporarily **stubs** them with a placeholder literal for the
//! lowering, and refuses to fold anything that would read a stub
//! (see [`stub_references`]). The initialiser under evaluation
//! travels in the wrapper function instead of its const slot, so its
//! real expression is what gets lowered and run. A program that does
//! not lower at all gets no fold and no fold-specific diagnostics —
//! the ordinary backends say why, and the run-time path still works.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use compiler_ir::{Module, Type};
use compiler_lower::ContractMessages;
use compiler_vm::slot::RawSlot;
use frontend::ast::{Expr, ExprRef, File, Function, Node, Stmt, StmtRef, TestCase, Visibility};
use frontend::type_checker::error::TypeCheckError;
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ir_vm::host::InterpreterHost;

/// Loop iterations one fold may spend before it is called
/// non-terminating. Shared with nothing: `--check` trials get a much
/// smaller allowance because they run thousands of times, while a
/// fold runs once and a user who wrote a compile-time loop expects it
/// to finish.
const FOLD_STEP_BUDGET: u64 = 1_000_000;

/// What one fold pass found.
#[derive(Default)]
pub struct FoldReport {
    /// A value that had to be known at compile time and was not.
    pub errors: Vec<TypeCheckError>,
    /// COMPILE-TIME-EVAL C4: a call whose arguments are all constants
    /// and whose `requires` the fold watched fail. Nothing here knows
    /// whether the call is reached, so it is reported rather than
    /// refused.
    pub warnings: Vec<TypeCheckError>,
}

/// Fold what can be folded, and report what had to be and could not.
///
/// Returns nothing for the overwhelming majority of programs: with no
/// `const fn` declared and no `const` at all there is nothing to do,
/// and the scan that establishes that is a pass over two lists.
pub fn fold_const_evaluations(
    program: &mut File,
    string_interner: &DefaultStringInterner,
) -> FoldReport {
    let const_fns: HashSet<DefaultSymbol> = program
        .function
        .iter()
        .filter(|f| f.const_fn && !f.is_extern)
        .map(|f| f.name)
        .collect();

    if const_fns.is_empty() && program.consts.is_empty() {
        return FoldReport::default();
    }

    // The fold's VM runs need a runtime state (heap manager +
    // allocator stack) to reach for; install a fresh one for the pass
    // and tear it down afterwards.
    //
    // MEMORY_PROFILING M0: the compiler's own allocations are not the
    // program's. A run zeroes the counters when it starts, but the
    // in-process lanes (`--test`, the consistency harness) read them
    // across a type-check, so the snapshot is restored rather than
    // relied upon.
    let profile_before = crate::heap::snapshot_profile();
    crate::runtime_state::RT.with(|s| {
        *s.borrow_mut() = Some(crate::runtime_state::RuntimeState::new());
    });
    let report = evaluate(program, string_interner, &const_fns);
    crate::runtime_state::RT.with(|s| {
        *s.borrow_mut() = None;
    });
    crate::heap::restore_profile(profile_before);

    report
}

/// Everything that needs the fold's evaluation context, kept in one
/// scope so the temporary mutations of `program` (the stubs, the
/// wrapper) are balanced on every path.
///
/// Each fold result is written into the expression pool **immediately**
/// — an earlier const's rewritten initialiser is what the next
/// const's fold sees, exactly as the tree-walker's environment held
/// the earlier consts' values.
fn evaluate(
    program: &mut File,
    string_interner: &DefaultStringInterner,
    const_fns: &HashSet<DefaultSymbol>,
) -> FoldReport {
    let mut report = FoldReport::default();

    // Lowering needs a mutable interner (contract messages, the
    // wrapper names below); a clone carries every symbol the program
    // already references.
    let mut interner_owned = string_interner.clone();
    let contract_msgs = ContractMessages::intern(&mut interner_owned);
    let host = InterpreterHost;

    // The placeholder every not-yet-literal const initialiser is
    // stubbed with while the fold lowers.
    let stub_ref = program.expression.add(Expr::UInt64(0));

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
    for idx in 0..program.consts.len() {
        let name = string_interner
            .resolve(program.consts[idx].name)
            .unwrap_or("?")
            .to_string();
        let context = format!("const {name}");
        let decl_value = program.consts[idx].value;

        // A literal initialiser is already its own value.
        if is_const_literal(program, decl_value) {
            continue;
        }

        // Check the callees first: "not a `const fn`" is a far better
        // message than whatever the evaluator would say, and for a
        // call the evaluator cannot even reach (an `extern`) there
        // would be no message at all.
        if let Some(detail) = uncallable_reason(program, &decl_value, const_fns, string_interner) {
            report.errors.push(err_at(program, &decl_value, &context, detail));
            break;
        }

        // The consts whose value the fold does not know yet (from
        // `idx` on, plus earlier non-literal ones like a `str`) are
        // stubbed in the fold's lowering. Anything the fold will run
        // must not read one: a function body may name a const declared
        // later than the one being folded — the initialiser may not —
        // and at fold time that later const has no value yet. This is
        // the C3 tree-walker's "undefined variable" for the same
        // situation, reported up front instead of as a wrong number.
        let stub_syms = stub_syms_for(program, idx);
        if let Some(sym) = stub_references(program, &decl_value, &stub_syms) {
            let what = string_interner.resolve(sym).unwrap_or("?");
            report.errors.push(err_at(
                program,
                &decl_value,
                &context,
                format!(
                    "it (or a function it calls) reads `{what}`, whose value is not known \
                     while compiling — consts are evaluated in declaration order"
                ),
            ));
            break;
        }

        // Lower once, run the wrapper, fold the result.
        let wrapper_sym = crate::ir_vm::const_wrapper_symbol(&mut interner_owned, idx);
        let Some(module) = lower_for_fold(
            program,
            &interner_owned,
            &contract_msgs,
            &stub_ref,
            &stub_syms,
            Some((idx, decl_value, wrapper_sym)),
        ) else {
            // The program does not lower at all; the ordinary backends
            // say why, and nothing here can be folded. Reporting a
            // const error would blame the wrong pass — and the
            // run-time path still evaluates this initialiser.
            break;
        };
        let Some(wrapper_id) = module.lookup_function(None, wrapper_sym) else {
            continue; // defensive: the wrapper was declared, so this is unreachable
        };
        match compiler_vm::run_function(
            &module,
            Some(&interner_owned),
            &host,
            wrapper_id,
            Vec::new(),
            Some(FOLD_STEP_BUDGET),
        ) {
            Ok(slots) => {
                let ret_ty = module.function(wrapper_id).return_type;
                if let Some(literal) = slot_to_literal(&slots, ret_ty) {
                    program.expression.update(&decl_value, literal);
                }
                // A non-scalar result (str, struct, Vec) has no
                // literal form; the initialiser stays as written and
                // whatever engine runs the program evaluates it.
            }
            // COMPILE-TIME-EVAL C4: a `requires` the fold watched
            // fail, with the offending values in the message.
            Err(message) if message == "requires violation" => {
                report.errors.push(err_at(
                    program,
                    &decl_value,
                    &context,
                    requires_violation_detail(program, &decl_value, string_interner),
                ));
                break;
            }
            Err(message) => {
                report.errors.push(err_at(program, &decl_value, &context, describe(&message)));
                break;
            }
        }
    }

    // Pass 2 — opportunistic. Every `const fn` call whose arguments
    // are all literals, wherever it appears. A flat scan of the
    // expression pool rather than a walk of every body: the pool is
    // the complete set of expressions, and a fold is valid at any of
    // them.
    //
    // Runs only when pass 1 got through: a broken initialiser already
    // fails the compile, and the program's consts are then in a state
    // the fold's lowering cannot represent faithfully.
    if !const_fns.is_empty() && report.errors.is_empty() {
        // Pass 1 folded the foldable consts to literals; the stubs
        // are whatever is left (a `str` const, say). Recompute the set
        // so the lowering reads real values wherever pass 1 produced
        // one.
        let stub_syms = stub_syms_for(program, program.consts.len());
        let Some(module) = lower_for_fold(
            program,
            &interner_owned,
            &contract_msgs,
            &stub_ref,
            &stub_syms,
            None,
        ) else {
            return report;
        };
        for index in 0..program.expression.len() {
            let expr_ref = ExprRef(index as u32);
            let Some(Expr::Call(callee, args)) = program.expression.get(&expr_ref) else {
                continue;
            };
            if !const_fns.contains(&callee) || !all_literal_args(program, &args) {
                continue;
            }
            // A generic `const fn` has no plain FuncId in the module;
            // its bodies are monomorphised on demand, which this pass
            // does not chase. The call is left alone.
            let Some(func_id) = module.lookup_function(None, callee) else {
                continue;
            };
            // The callee's bodies must not read a stubbed const —
            // the placeholder would fold into the answer.
            if stub_references_stmt(program, callee_body_stmt(program, callee), &stub_syms) {
                continue;
            }
            let Some(arg_slots) = literal_arg_slots(program, &args) else {
                continue;
            };
            match compiler_vm::run_function(
                &module,
                Some(&interner_owned),
                &host,
                func_id,
                arg_slots,
                Some(FOLD_STEP_BUDGET),
            ) {
                Ok(slots) => {
                    let ret_ty = module.function(func_id).return_type;
                    if let Some(literal) = slot_to_literal(&slots, ret_ty) {
                        program.expression.update(&expr_ref, literal);
                    }
                }
                // COMPILE-TIME-EVAL C4: a precondition the compiler
                // watched fail, with every argument a constant, fails
                // on every run that reaches this call. Whether one
                // does is exactly what this pass cannot see, so it is
                // reported and the call is left alone.
                Err(message) if message == "requires violation" => {
                    report.warnings.push(broken_precondition(
                        program,
                        &expr_ref,
                        string_interner,
                        callee,
                        &args,
                    ));
                }
                // Anything else — a trap, a panic, a spent budget —
                // leaves the call in place and lets run time have the
                // behaviour it always had.
                Err(_) => continue,
            }
        }
    }

    report
}

/// Lower `program` for the fold: every const named in `stub_syms` has
/// its initialiser temporarily replaced with `stub_ref`, and (when
/// `wrapper` is given) that const's real initialiser travels in a
/// synthetic zero-argument function registered as a `test` block, so
/// the lowering treats it as an entry point and lowers its body.
///
/// The program is restored (stubs and wrapper both) before returning;
/// the returned module is what the fold runs.
fn lower_for_fold(
    program: &mut File,
    interner: &DefaultStringInterner,
    contract_msgs: &ContractMessages,
    stub_ref: &ExprRef,
    stub_syms: &HashSet<DefaultSymbol>,
    wrapper: Option<(usize, ExprRef, DefaultSymbol)>,
) -> Option<Module> {
    // 1. Stub every const the fold does not know yet.
    let mut saved: Vec<(usize, ExprRef)> = Vec::new();
    for (j, c) in program.consts.iter().enumerate() {
        if stub_syms.contains(&c.name) {
            saved.push((j, c.value));
        }
    }
    for (j, _) in &saved {
        program.consts[*j].value = *stub_ref;
    }
    // 2. The wrapper, when this lowering is for one const's value.
    if let Some((idx, original, sym)) = wrapper {
        let body = program.statement.add(Stmt::Expression(original));
        let function = Rc::new(Function {
            node: Node::new(0, 0),
            name: sym,
            generic_params: Vec::new(),
            generic_bounds: HashMap::new(),
            parameter: Vec::new(),
            return_type: Some(program.consts[idx].type_decl.clone()),
            requires: Vec::new(),
            ensures: Vec::new(),
            old_exprs: Vec::new(),
            ensures_kinds: Vec::new(),
            never_allocates: false,
            const_fn: false,
            code: body,
            is_extern: false,
            extern_link: None,
            visibility: Visibility::Private,
        });
        program.function.push(Rc::clone(&function));
        program.tests.push(TestCase {
            name: format!("__ctfe_{idx}"),
            function: sym,
            line: 0,
        });
    }
    // 3. Lower.
    let module = compiler_lower::lower_program(program, interner, contract_msgs, false).ok();
    // 4. Restore (wrapper first, then the stubs).
    if wrapper.is_some() {
        program.tests.pop();
        program.function.pop();
    }
    for (j, value) in saved {
        program.consts[j].value = value;
    }
    module
}

/// The consts whose real value is unknown at fold time of `idx`:
/// every const from `idx` on (not yet evaluated), plus earlier consts
/// whose initialiser is not a literal (a `str` const, which the fold
/// cannot turn into a literal). These are the ones the fold's
/// lowering stubs, and the ones a folded body must not read.
fn stub_syms_for(program: &File, idx: usize) -> HashSet<DefaultSymbol> {
    program
        .consts
        .iter()
        .enumerate()
        .filter(|(j, c)| *j >= idx || !is_const_literal(program, c.value))
        .map(|(_, c)| c.name)
        .collect()
}

/// Whether `compiler_lower::consts` can read this initialiser as-is:
/// the literal set its evaluator accepts. Everything else — a name, a
/// call, an expression, even a narrow literal — must be folded (or
/// stubbed) before lowering.
fn is_const_literal(program: &File, value: ExprRef) -> bool {
    matches!(
        program.expression.get(&value),
        Some(Expr::Int64(_) | Expr::UInt64(_) | Expr::Float64(_) | Expr::True | Expr::False)
    )
}

/// The literal a folded value becomes, or `None` when the value has
/// no literal form (a `str`, a struct, a `Vec`, unit). `ty` is the
/// function's IR return type — the slot alone does not say what it
/// holds.
fn slot_to_literal(slots: &[RawSlot], ty: Type) -> Option<Expr> {
    let slot = slots.first()?;
    Some(match ty {
        Type::Bool => {
            if unsafe { slot.bool } {
                Expr::True
            } else {
                Expr::False
            }
        }
        Type::I64 => Expr::Int64(unsafe { slot.i64 }),
        Type::U64 => Expr::UInt64(unsafe { slot.u64 }),
        Type::I8 => Expr::Int8(unsafe { slot.i64 as i8 }),
        Type::U8 => Expr::UInt8(unsafe { slot.u64 as u8 }),
        Type::I16 => Expr::Int16(unsafe { slot.i64 as i16 }),
        Type::U16 => Expr::UInt16(unsafe { slot.u64 as u16 }),
        Type::I32 => Expr::Int32(unsafe { slot.i64 as i32 }),
        Type::U32 => Expr::UInt32(unsafe { slot.u64 as u32 }),
        Type::F64 => Expr::Float64(unsafe { slot.f64 }),
        _ => return None,
    })
}

/// The call's argument literals as VM slots. `None` when an argument
/// is not a literal — the fold only runs calls it can prove
/// constant.
fn literal_arg_slots(program: &File, args: &ExprRef) -> Option<Vec<RawSlot>> {
    let items = expr_list_items(program, args)?;
    items.iter().map(|item| literal_slot(program, item)).collect()
}

fn literal_slot(program: &File, item: &ExprRef) -> Option<RawSlot> {
    match program.expression.get(item) {
        Some(Expr::Int64(v)) => Some(RawSlot::from_i64(v)),
        Some(Expr::UInt64(v)) => Some(RawSlot::from_u64(v)),
        Some(Expr::Float64(v)) => Some(RawSlot::from_f64(v)),
        Some(Expr::True) => Some(RawSlot::from_bool(true)),
        Some(Expr::False) => Some(RawSlot::from_bool(false)),
        Some(Expr::Int8(v)) => Some(RawSlot::from_i64(v as i64)),
        Some(Expr::Int16(v)) => Some(RawSlot::from_i64(v as i64)),
        Some(Expr::Int32(v)) => Some(RawSlot::from_i64(v as i64)),
        Some(Expr::UInt8(v)) => Some(RawSlot::from_u64(v as u64)),
        Some(Expr::UInt16(v)) => Some(RawSlot::from_u64(v as u64)),
        Some(Expr::UInt32(v)) => Some(RawSlot::from_u64(v as u64)),
        _ => None,
    }
}

/// A literal's text, the way the tree-walker's contract reports
/// render a value ("3", "true", ...).
fn literal_text(program: &File, item: &ExprRef) -> Option<String> {
    match program.expression.get(item) {
        Some(Expr::Int64(v)) => Some(v.to_string()),
        Some(Expr::UInt64(v)) => Some(v.to_string()),
        Some(Expr::Float64(v)) => Some(v.to_string()),
        Some(Expr::True) => Some("true".to_string()),
        Some(Expr::False) => Some("false".to_string()),
        Some(Expr::Int8(v)) => Some(v.to_string()),
        Some(Expr::Int16(v)) => Some(v.to_string()),
        Some(Expr::Int32(v)) => Some(v.to_string()),
        Some(Expr::UInt8(v)) => Some(v.to_string()),
        Some(Expr::UInt16(v)) => Some(v.to_string()),
        Some(Expr::UInt32(v)) => Some(v.to_string()),
        _ => None,
    }
}

fn expr_list_items(program: &File, args: &ExprRef) -> Option<Vec<ExprRef>> {
    match program.expression.get(args) {
        Some(Expr::ExprList(items)) => Some(items.clone()),
        _ => None,
    }
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
    items.iter().all(|item| literal_slot(program, item).is_some())
}

/// The statement the fold runs when it calls `callee` — its body —
/// for the stub-reference check. `find` picks the first same-named
/// function, which for a name the fold knows is the user's:
/// user-authored functions precede integrated stdlib ones in
/// `program.function`.
fn callee_body_stmt(program: &File, callee: DefaultSymbol) -> Option<StmtRef> {
    program
        .function
        .iter()
        .find(|f| f.name == callee && !f.is_extern)
        .map(|f| f.code)
}

/// "its initialiser calls `half`, whose `requires` clause #1 is false
/// for a constant argument, with n = 3" — the forced-path report for
/// a `requires` violation the fold watched fail. The violating call
/// is not identified by the VM, so the first requires-constrained
/// call in the initialiser stands in; the common single-call
/// initialiser is exact.
fn requires_violation_detail(
    program: &File,
    expr_ref: &ExprRef,
    interner: &DefaultStringInterner,
) -> String {
    let mut found: Option<String> = None;
    walk(program, expr_ref, &mut |expr| {
        if found.is_some() {
            return;
        }
        if let Expr::Call(callee, args) = expr {
            let Some(f) = program.function.iter().find(|f| f.name == *callee) else {
                return;
            };
            if f.requires.is_empty() {
                return;
            }
            found = Some(format!(
                "its initialiser calls `{}`, whose `requires` clause #1 is false for a \
                 constant argument{}",
                interner.resolve(*callee).unwrap_or("?"),
                call_site_values(program, &f.parameter, args, interner),
            ));
        }
    });
    found.unwrap_or_else(|| "its initialiser breaks a `requires` clause".to_string())
}

/// `, with n = 3` — the values a call site passes, for a diagnostic.
/// Empty when an argument is not a literal.
fn call_site_values(
    program: &File,
    params: &[(DefaultSymbol, TypeDecl)],
    args: &ExprRef,
    interner: &DefaultStringInterner,
) -> String {
    let Some(items) = expr_list_items(program, args) else {
        return String::new();
    };
    let rendered: Vec<String> = params
        .iter()
        .zip(items.iter())
        .filter_map(|((name, _), item)| {
            let value = literal_text(program, item)?;
            Some(format!("{} = {}", interner.resolve(*name).unwrap_or("?"), value))
        })
        .collect();
    if rendered.is_empty() {
        String::new()
    } else {
        format!(", with {}", rendered.join(", "))
    }
}

/// COMPILE-TIME-EVAL C4: a constant call that breaks its own
/// `requires`. The values the predicate saw come from the call site —
/// the callee's parameter names paired with the literal arguments.
fn broken_precondition(
    program: &File,
    expr_ref: &ExprRef,
    interner: &DefaultStringInterner,
    callee: DefaultSymbol,
    args: &ExprRef,
) -> TypeCheckError {
    let name = interner.resolve(callee).unwrap_or("?").to_string();
    let bindings: Vec<(String, String)> = program
        .function
        .iter()
        .find(|f| f.name == callee)
        .map(|f| {
            let items = expr_list_items(program, args).unwrap_or_default();
            f.parameter
                .iter()
                .zip(items.iter())
                .filter_map(|((pname, _), item)| {
                    literal_text(program, item).map(|v| {
                        (interner.resolve(*pname).unwrap_or("?").to_string(), v)
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let values = if bindings.is_empty() {
        String::new()
    } else {
        let rendered: Vec<String> = bindings
            .iter()
            .map(|(name, value)| format!("{name} = {value}"))
            .collect();
        format!(", with {}", rendered.join(", "))
    };
    let detail = format!("`requires` clause #{}{}", 1, values);
    let mut error = TypeCheckError::broken_precondition(name, detail);
    if let Some(location) = program.location_pool.get_expr_location(expr_ref) {
        error = error.with_location(*location);
    }
    error
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

// ---------------------------------------------------------------------------
// The stub-reference walk
// ---------------------------------------------------------------------------

/// Whether anything reachable from `root` names one of `stub_syms` —
/// a const whose initialiser the fold's lowering stubbed. Follows the
/// call graph the way `reachability.rs` does: free functions by name,
/// methods by name (every same-named body, since the owning type is
/// not tracked here). A call that resolves to no body (a generic
/// template, a closure) is not followed — this check is a net, not a
/// proof.
fn stub_references(
    program: &File,
    root: &ExprRef,
    stub_syms: &HashSet<DefaultSymbol>,
) -> Option<DefaultSymbol> {
    let mut walker = StubWalker::new(program, stub_syms);
    walker.walk_expr(root)
}

/// Statement-rooted variant of [`stub_references`], used for the
/// opportunistic path's callee bodies.
fn stub_references_stmt(
    program: &File,
    root: Option<StmtRef>,
    stub_syms: &HashSet<DefaultSymbol>,
) -> bool {
    let Some(root) = root else {
        return false;
    };
    let mut walker = StubWalker::new(program, stub_syms);
    walker.walk_stmt(&root).is_some()
}

struct StubWalker<'a> {
    program: &'a File,
    stub_syms: &'a HashSet<DefaultSymbol>,
    by_name: HashMap<DefaultSymbol, usize>,
    by_method_name: HashMap<DefaultSymbol, Vec<StmtRef>>,
    seen: HashSet<usize>,
    seen_methods: HashSet<DefaultSymbol>,
}

impl<'a> StubWalker<'a> {
    fn new(program: &'a File, stub_syms: &'a HashSet<DefaultSymbol>) -> Self {
        let mut by_name: HashMap<DefaultSymbol, usize> = HashMap::new();
        for (i, f) in program.function.iter().enumerate() {
            by_name.entry(f.name).or_insert(i);
        }
        let mut by_method_name: HashMap<DefaultSymbol, Vec<StmtRef>> = HashMap::new();
        for index in 0..program.statement.len() {
            if let Some(Stmt::ImplBlock { methods, .. }) =
                program.statement.get(&StmtRef(index as u32))
            {
                for m in methods {
                    by_method_name.entry(m.name).or_default().push(m.code);
                }
            }
        }
        StubWalker {
            program,
            stub_syms,
            by_name,
            by_method_name,
            seen: HashSet::new(),
            seen_methods: HashSet::new(),
        }
    }

    fn walk_stmt(&mut self, stmt_ref: &StmtRef) -> Option<DefaultSymbol> {
        let stmt = self.program.statement.get(stmt_ref)?;
        match stmt {
            Stmt::Expression(e) | Stmt::Val(_, _, e) => self.walk_expr(&e),
            Stmt::Var(_, _, e) => e.and_then(|e| self.walk_expr(&e)),
            Stmt::Return(e) => e.and_then(|e| self.walk_expr(&e)),
            Stmt::For(_, _, start, end, body) => self
                .walk_expr(&start)
                .or_else(|| self.walk_expr(&end))
                .or_else(|| self.walk_expr(&body)),
            Stmt::While(_, cond, body) => self
                .walk_expr(&cond)
                .or_else(|| self.walk_expr(&body)),
            _ => None,
        }
    }

    fn walk_expr(&mut self, expr_ref: &ExprRef) -> Option<DefaultSymbol> {
        let expr = self.program.expression.get(expr_ref)?;
        match expr {
            Expr::Identifier(sym) if self.stub_syms.contains(&sym) => Some(sym),
            Expr::Call(callee, args) => self
                .walk_expr(&args)
                .or_else(|| self.enter(&callee)),
            Expr::MethodCall(receiver, method, args) => self
                .walk_expr(&receiver)
                .or_else(|| self.walk_all(&args))
                .or_else(|| self.enter_method(&method)),
            Expr::AssociatedFunctionCall(_, function, args) => self
                .walk_all(&args)
                .or_else(|| self.enter_method(&function)),
            Expr::Binary(_, lhs, rhs) => self.walk_expr(&lhs).or_else(|| self.walk_expr(&rhs)),
            Expr::Unary(_, operand) => self.walk_expr(&operand),
            Expr::Block(stmts) => stmts.iter().find_map(|s| self.walk_stmt(s)),
            Expr::IfElifElse(cond, then_block, elifs, else_block) => self
                .walk_expr(&cond)
                .or_else(|| self.walk_expr(&then_block))
                .or_else(|| {
                    elifs
                        .iter()
                        .find_map(|(c, b)| self.walk_expr(c).or_else(|| self.walk_expr(b)))
                })
                .or_else(|| self.walk_expr(&else_block)),
            Expr::Match(scrutinee, arms) => self.walk_expr(&scrutinee).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.guard
                        .and_then(|g| self.walk_expr(&g))
                        .or_else(|| self.walk_expr(&arm.body))
                })
            }),
            Expr::Assign(lhs, rhs) => self.walk_expr(&lhs).or_else(|| self.walk_expr(&rhs)),
            Expr::ExprList(items) | Expr::ArrayLiteral(items) | Expr::TupleLiteral(items) => {
                self.walk_all(&items)
            }
            Expr::StructLiteral(_, fields) => {
                fields.iter().find_map(|(_, v)| self.walk_expr(v))
            }
            Expr::DictLiteral(entries) => entries
                .iter()
                .find_map(|(k, v)| self.walk_expr(k).or_else(|| self.walk_expr(v))),
            Expr::FieldAccess(obj, _) | Expr::TupleAccess(obj, _) | Expr::Cast(obj, _) => {
                self.walk_expr(&obj)
            }
            Expr::BuiltinMethodCall(receiver, _, args) => self
                .walk_expr(&receiver)
                .or_else(|| self.walk_all(&args)),
            Expr::SliceAccess(obj, info) => self.walk_expr(&obj).or_else(|| {
                info.start
                    .and_then(|s| self.walk_expr(&s))
                    .or_else(|| info.end.and_then(|e| self.walk_expr(&e)))
            }),
            Expr::SliceAssign(obj, start, end, value) => self
                .walk_expr(&obj)
                .or_else(|| start.and_then(|s| self.walk_expr(&s)))
                .or_else(|| end.and_then(|e| self.walk_expr(&e)))
                .or_else(|| self.walk_expr(&value)),
            Expr::With(allocator, body) => self
                .walk_expr(&allocator)
                .or_else(|| self.walk_expr(&body)),
            Expr::Range(start, end) => self
                .walk_expr(&start)
                .or_else(|| self.walk_expr(&end)),
            _ => None,
        }
    }

    fn walk_all(&mut self, items: &[ExprRef]) -> Option<DefaultSymbol> {
        items.iter().find_map(|e| self.walk_expr(e))
    }

    fn enter(&mut self, callee: &DefaultSymbol) -> Option<DefaultSymbol> {
        let index = self.by_name.get(callee).copied()?;
        if !self.seen.insert(index) {
            // Recursion: the cycle adds no new reachable code.
            return None;
        }
        let function = self.program.function[index].clone();
        let result = if function.is_extern {
            None
        } else {
            self.walk_stmt(&function.code)
        };
        self.seen.remove(&index);
        result
    }

    fn enter_method(&mut self, name: &DefaultSymbol) -> Option<DefaultSymbol> {
        if !self.seen_methods.insert(*name) {
            return None;
        }
        let result = self
            .by_method_name
            .get(name)
            .cloned()
            .unwrap_or_default()
            .iter()
            .find_map(|body| self.walk_stmt(body));
        self.seen_methods.remove(name);
        result
    }
}

/// A one-clause description of a failed fold, phrased to follow
/// "`const D` must be known at compile time, but ...".
fn describe(message: &str) -> String {
    format!("evaluating it failed: {message}")
}

fn err_at(program: &File, expr_ref: &ExprRef, context: &str, detail: String) -> TypeCheckError {
    let mut error = TypeCheckError::const_eval(context.to_string(), detail);
    if let Some(location) = program.location_pool.get_expr_location(expr_ref) {
        error = error.with_location(*location);
    }
    error
}