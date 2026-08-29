//! Contract-driven property testing (LLM-LOOP P5).
//!
//! toylang already carries the two halves of a property test in the
//! source: `requires` says which inputs are legal, `ensures` says what
//! must hold afterwards. This reads them as a generator filter and an
//! oracle, so a counterexample can be produced without the author
//! writing a test at all.
//!
//! Three things make the output worth reading:
//!
//! * **Shrinking is not optional.** A raw counterexample of
//!   `a = -6148914691236517206, b = 3` and a shrunk one of
//!   `a = 1, b = 2` cost the reader very different amounts of thought.
//!   Reporting the first would just move the work.
//! * **A `requires` violation is not a failure.** It means the
//!   generator produced an input the function never promised to
//!   handle; those are discarded, and a run that discards nearly
//!   everything is reported as such rather than as a pass.
//! * **The seed is printed.** A property that fails one run in fifty is
//!   worthless if it cannot be replayed.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use frontend::ast::{File, Function, MethodFunction, Stmt, StmtRef};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::error::InterpreterError;
use crate::object::{Object, RcObject};
use crate::value::Value;

/// How many inputs to try per function before declaring it healthy.
const DEFAULT_CASES: usize = 200;

/// Give up after this many consecutive `requires` rejections — the
/// precondition is too narrow for uniform sampling to hit.
const MAX_DISCARD_RATIO: usize = 20;

/// Loop iterations one trial may execute before it is abandoned
/// (CHECK-NONTERMINATION).
///
/// A property trial calls a function with inputs nobody wrote it for.
/// `fn triangle(n: u64)` looping `while i <= n` is perfectly correct
/// and perfectly finite for every `n` a caller would pass, and takes
/// longer than the heat death of the universe for the `n = u64::MAX`
/// that uniform sampling hands it — so without a cap the checker
/// hangs on a *correct* program. The cap counts loop back-edges, not
/// wall-clock time, so `--check --seed=0x99` replays to the same
/// verdict on any machine under any load.
///
/// The first trial to exceed the budget ends that function's check,
/// so this is the ceiling per contracted function, not per case:
/// measured at ~0.5s in a debug build (the tree-walker runs ~200k
/// iterations/s there). Paid only by a function whose sampled inputs
/// run away, which is a function the author has to change anyway.
const TRIAL_STEP_BUDGET: u64 = 100_000;

/// Concrete types a generic receiver's type parameters may be
/// instantiated with, in preference order (DBC-CHECK-METHODS).
///
/// A method of a generic impl (`impl<T> Cell<T>`) has no concrete
/// instantiation in the source to borrow, so the checker picks one.
/// `i64` first: contracts overwhelmingly constrain signed integers,
/// and the receiver's fields are sampled under the same substitution,
/// so the body's operations line up unless it mixes fixed literals of
/// another width.
const GENERIC_UNIVERSE: [TypeDecl; 4] = [
    TypeDecl::Int64,
    TypeDecl::UInt64,
    TypeDecl::Float64,
    TypeDecl::Bool,
];

/// Bounding the receiver-construction recursion. Well-typed programs
/// cannot contain by-value type cycles (E0013), so this is a belt for
/// a pathological substitution (`Cell<Cell<Cell<…>>>`) rather than a
/// correctness requirement.
const MAX_RECEIVER_DEPTH: usize = 8;

/// Deterministic PRNG (SplitMix64). Small enough to keep the crate
/// dependency-free, and reproducible from a printed seed, which is the
/// only property that matters here.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Sample a value for `ty`, biased toward the edges that break code.
///
/// Uniform 64-bit noise almost never produces 0, 1, or -1, which is
/// where the interesting failures are; a third of draws are taken from
/// a small set of boundary values instead.
fn sample(rng: &mut Rng, ty: &TypeDecl) -> Option<Value> {
    let raw = rng.next_u64();
    let pick_edge = raw.is_multiple_of(3);
    Some(match ty {
        TypeDecl::Bool => Value::Bool(raw & 1 == 1),
        TypeDecl::Int64 => {
            if pick_edge {
                const EDGES: [i64; 6] = [0, 1, -1, 2, i64::MIN, i64::MAX];
                Value::Int64(EDGES[(raw >> 2) as usize % EDGES.len()])
            } else {
                Value::Int64((raw as i64) % 1_000)
            }
        }
        TypeDecl::UInt64 => {
            if pick_edge {
                const EDGES: [u64; 5] = [0, 1, 2, u64::MAX, u64::MAX - 1];
                Value::UInt64(EDGES[(raw >> 2) as usize % EDGES.len()])
            } else {
                Value::UInt64(raw % 1_000)
            }
        }
        TypeDecl::Float64 => {
            if pick_edge {
                const EDGES: [f64; 5] = [0.0, 1.0, -1.0, f64::MAX, f64::MIN];
                Value::Float64(EDGES[(raw >> 2) as usize % EDGES.len()])
            } else {
                Value::Float64((raw % 2_000) as f64 - 1_000.0)
            }
        }
        // SIMD-F32: same shape as the f64 sampler at single precision.
        TypeDecl::Float32 => {
            if pick_edge {
                const EDGES: [f32; 5] = [0.0, 1.0, -1.0, f32::MAX, f32::MIN];
                Value::Float32(EDGES[(raw >> 2) as usize % EDGES.len()])
            } else {
                Value::Float32((raw % 2_000) as f32 - 1_000.0)
            }
        }
        _ => return None,
    })
}

/// Sample a value for `ty` like `sample`, but also construct user
/// structs by recursively sampling their fields (DBC-CHECK-METHODS).
///
/// `subst` carries the concrete types chosen for a generic impl's type
/// parameters; it is applied to every field / parameter type before
/// sampling, so a receiver and its methods' arguments agree on one
/// instantiation. `depth` bounds nested structs; an unsupported leaf
/// (enum, `str`, `ptr`, arrays, …) fails the whole value, which the
/// caller reports as "cannot be generated yet".
fn sample_typed(
    rng: &mut Rng,
    shared: &crate::SharedRunData<'_>,
    ty: &TypeDecl,
    subst: &HashMap<DefaultSymbol, TypeDecl>,
    depth: usize,
) -> Option<Value> {
    let ty = ty.substitute_generics(subst);
    match &ty {
        TypeDecl::Struct(name, args) => {
            if depth >= MAX_RECEIVER_DEPTH {
                return None;
            }
            sample_struct_value(rng, shared, *name, args, subst, depth)
        }
        // The parser writes a bare user type name as `Identifier` in
        // some positions (TYPE-NAME-SPELLING); resolve it against the
        // struct registry the same way `Struct(name, args)` is.
        TypeDecl::Identifier(name) if shared.struct_definitions.contains_key(name) => {
            if depth >= MAX_RECEIVER_DEPTH {
                return None;
            }
            sample_struct_value(rng, shared, *name, &[], subst, depth)
        }
        other => sample(rng, other),
    }
}

/// Construct a struct value by sampling every field, recursively.
///
/// The result is a heap `Object::Struct` whose `type_args` carry the
/// concrete arguments — the same shape `evaluate_struct_literal`
/// builds, which is also what method dispatch (`get_method`) and
/// `to_display_string` key on.
fn sample_struct_value(
    rng: &mut Rng,
    shared: &crate::SharedRunData<'_>,
    name: DefaultSymbol,
    type_args: &[TypeDecl],
    subst: &HashMap<DefaultSymbol, TypeDecl>,
    depth: usize,
) -> Option<Value> {
    let entry = shared.struct_definitions.get(&name)?;
    let mut fields: HashMap<DefaultSymbol, RcObject> = HashMap::new();
    for (field_sym, field_ty) in &entry.fields {
        let value = sample_typed(rng, shared, field_ty, subst, depth + 1)?;
        fields.insert(*field_sym, value.into_rc());
    }
    Some(Value::Heap(Rc::new(RefCell::new(Object::Struct {
        type_name: name,
        fields: Box::new(fields),
        type_args: type_args.to_vec(),
    }))))
}

/// Candidates a value shrinks to, simplest first.
///
/// Integers binary-search toward zero (`v`, `v/2`, `v/4`, … away from
/// the current value) rather than only halving. Halving alone gets
/// stuck: with `ensures result * b == a` over integer division, `9`
/// cannot reach a smaller counterexample via `0`, `4` or `8` — all
/// three satisfy the contract — so the report would have kept a value
/// three times larger than necessary.
fn shrink(value: &Value) -> Vec<Value> {
    match value {
        Value::Bool(true) => vec![Value::Bool(false)],
        Value::Bool(false) => vec![],
        Value::Int64(0) => vec![],
        Value::Int64(v) => {
            let mut out = Vec::new();
            if *v < 0 {
                // Sign is often incidental; try the positive twin first.
                // `wrapping_neg` on `i64::MIN` is a no-op — a candidate
                // identical to the current value would re-fail the trial,
                // be accepted, and stall the whole greedy loop on no-ops.
                let neg = v.wrapping_neg();
                if neg != *v {
                    out.push(Value::Int64(neg));
                }
            }
            let mut delta = *v;
            while delta != 0 {
                let candidate = v.wrapping_sub(delta);
                if candidate != *v {
                    out.push(Value::Int64(candidate));
                }
                delta /= 2;
            }
            out
        }
        Value::UInt64(0) => vec![],
        Value::UInt64(v) => {
            let mut out = Vec::new();
            let mut delta = *v;
            while delta != 0 {
                out.push(Value::UInt64(v - delta));
                delta /= 2;
            }
            out
        }
        Value::Float64(v) if *v == 0.0 => vec![],
        Value::Float64(v) => {
            let mut out = vec![Value::Float64(0.0)];
            if *v < 0.0 {
                out.push(Value::Float64(-v));
            }
            out.push(Value::Float64(v / 2.0));
            out
        }
        Value::Float32(v) if *v == 0.0 => vec![],
        Value::Float32(v) => {
            let mut out = vec![Value::Float32(0.0)];
            if *v < 0.0 {
                out.push(Value::Float32(-v));
            }
            out.push(Value::Float32(v / 2.0));
            out
        }
        // A struct receiver shrinks field by field: one candidate per
        // field whose value has a simpler form, so a counterexample
        // whose failure depends on `self` can be minimised too.
        Value::Heap(rc) => {
            let borrowed = rc.borrow();
            let Object::Struct { type_name, fields, type_args } = &*borrowed else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for (field_sym, field_rc) in fields.iter() {
                for candidate in shrink(&Value::from_rc(field_rc)) {
                    let mut new_fields = fields.as_ref().clone();
                    new_fields.insert(*field_sym, candidate.into_rc());
                    out.push(Value::Heap(Rc::new(RefCell::new(Object::Struct {
                        type_name: *type_name,
                        fields: Box::new(new_fields),
                        type_args: type_args.clone(),
                    }))));
                }
            }
            out
        }
        _ => vec![],
    }
}

fn render(value: &Value, interner: &DefaultStringInterner) -> String {
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Int64(v) => format!("{v}i64"),
        Value::UInt64(v) => format!("{v}u64"),
        Value::Float64(v) => format!("{v}f64"),
        Value::Float32(v) => format!("{v}f32"),
        // Struct receivers render like `println` would (field names
        // resolved through the interner), not as `Object` debug output
        // full of symbol ids.
        Value::Heap(rc) => rc.borrow().to_display_string(interner),
        other => format!("{other:?}"),
    }
}

/// What one function's property check produced.
#[derive(Debug, Clone)]
pub enum CheckOutcome {
    /// Every generated input that satisfied `requires` also satisfied
    /// `ensures`.
    ///
    /// `discarded` is how many the precondition turned away. A pass
    /// over 3 inputs and a pass over 200 are very different claims,
    /// and the caller has no other way to tell them apart
    /// (DBC-CHECK-CASES).
    Passed { cases: usize, discarded: usize },
    /// The precondition rejected nearly everything, so the pass means
    /// little. Reported separately rather than counted as success.
    Inconclusive { discarded: usize },
    /// A counterexample, already shrunk.
    Failed {
        /// Parameter name / value pairs.
        counterexample: Vec<(String, String)>,
        /// The diagnostic the contract produced.
        detail: String,
    },
    /// A generated input kept the body looping past the step budget,
    /// so the check could not finish (CHECK-NONTERMINATION).
    ///
    /// Neither a pass nor a failure: nothing falsified the contract,
    /// but nothing established it either. Reported on its own so a
    /// function whose inputs run away cannot masquerade as one that
    /// passed on the handful of trials that happened to terminate.
    Exhausted {
        /// Inputs that ran to completion before one ran away.
        cases: usize,
        /// Inputs `requires` turned away in the meantime.
        discarded: usize,
        /// The ceiling that was hit, in loop iterations.
        budget: u64,
    },
    /// Not checkable: no contracts, or a parameter this cannot sample.
    Skipped { reason: String },
}

#[derive(Debug, Clone)]
pub struct FunctionCheck {
    pub function: String,
    pub outcome: CheckOutcome,
}

/// Result of checking a whole file.
#[derive(Debug, Clone)]
pub struct CheckReport {
    pub seed: u64,
    pub checks: Vec<FunctionCheck>,
}

impl CheckReport {
    pub fn failed(&self) -> bool {
        self.checks
            .iter()
            .any(|c| matches!(c.outcome, CheckOutcome::Failed { .. }))
    }
}

/// Why a single trial ended.
enum Trial {
    /// Input satisfied `requires` and `ensures`.
    Ok,
    /// Input did not satisfy `requires`; not a failure.
    Discarded,
    /// `ensures` (or the body) rejected the input.
    Failed(String),
    /// The body ran past [`TRIAL_STEP_BUDGET`] loop iterations. Not a
    /// failure — the contract was never falsified, the checker simply
    /// could not wait for an answer (CHECK-NONTERMINATION).
    Exhausted,
}

fn run_trial(
    shared: &crate::SharedRunData<'_>,
    string_interner: &DefaultStringInterner,
    function: &Rc<Function>,
    args: &[Value],
) -> Trial {
    // A fresh context per trial: contracts can mutate globals and the
    // heap, and a counterexample that only reproduces after some other
    // trial ran is not a counterexample anyone can act on. The context
    // shares the program-derived maps with every other trial (TEST-PERF:
    // rebuilding them per trial costs ~600µs with the stdlib loaded).
    let outcome = crate::execute_function_with_values_shared(
        shared,
        string_interner,
        function.clone(),
        args,
        Some(TRIAL_STEP_BUDGET),
    );
    match outcome {
        Ok(_) => Trial::Ok,
        Err(InterpreterError::ContractViolation(v)) if v.kind == "requires" => Trial::Discarded,
        Err(InterpreterError::StepBudgetExceeded { .. }) => Trial::Exhausted,
        Err(e) => Trial::Failed(e.to_string()),
    }
}

/// One trial of a method: run it through the shared-context method
/// path so `requires` / `ensures` violations classify exactly like a
/// free function's (DBC-CHECK-METHODS).
///
/// `args` carries the receiver first when the method takes one
/// (`&self` / `&mut self` / `self: Self`), followed by the sampled
/// parameters — the same layout `shrink_counterexample` walks, so a
/// shrunk receiver is handed back here on the next trial.
fn run_method_trial(
    shared: &crate::SharedRunData<'_>,
    string_interner: &DefaultStringInterner,
    method: &Rc<MethodFunction>,
    args: &[Value],
) -> Trial {
    let first_param_is_self = method
        .parameter
        .first()
        .and_then(|(sym, _)| string_interner.resolve(*sym))
        .map(|name| name == "self")
        .unwrap_or(false);
    let has_receiver = method.has_self_param || first_param_is_self;
    let (self_obj, rest): (Option<RcObject>, &[Value]) = if has_receiver {
        match args.first() {
            Some(Value::Heap(rc)) => (Some(rc.clone()), &args[1..]),
            _ => {
                return Trial::Failed(
                    "internal error: method trial missing its receiver".to_string(),
                );
            }
        }
    } else {
        (None, args)
    };
    let outcome = crate::execute_method_with_values_shared(
        shared,
        string_interner,
        method.clone(),
        self_obj,
        rest,
        Some(TRIAL_STEP_BUDGET),
    );
    match outcome {
        Ok(_) => Trial::Ok,
        Err(InterpreterError::ContractViolation(v)) if v.kind == "requires" => Trial::Discarded,
        Err(InterpreterError::StepBudgetExceeded { .. }) => Trial::Exhausted,
        Err(e) => Trial::Failed(e.to_string()),
    }
}

/// Check every contracted function *and method* in `program`.
pub fn check_program(
    program: &File,
    interner: &mut DefaultStringInterner,
    seed: u64,
    cases: usize,
) -> CheckReport {
    let shared = crate::SharedRunData::new(program, interner)
        .expect("shared run data should build for a type-checked program");
    let mut checks = Vec::new();
    for function in &program.function {
        let name = interner.resolve(function.name).unwrap_or("<unknown>").to_string();
        if name.starts_with("__test_") {
            continue;
        }
        checks.push(FunctionCheck {
            outcome: check_function(&shared, interner, function, seed, cases),
            function: name,
        });
    }

    // DBC-CHECK-METHODS: methods carry contracts too, and a
    // `requires` / `ensures` on one is as much of an oracle as a free
    // function's. Impl blocks live in the statement pool (integrated
    // stdlib impls included); uncontracted methods are skipped cheaply
    // by `check_method`, and the same `(target, method)` pair
    // appearing in both an inherent and a trait impl is checked once.
    let mut seen: HashSet<(DefaultSymbol, DefaultSymbol)> = HashSet::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        let Some(stmt) = program.statement.get(&stmt_ref) else {
            continue;
        };
        let Stmt::ImplBlock {
            target_type,
            target_type_args,
            methods,
            ..
        } = &stmt
        else {
            continue;
        };
        let target_name = interner.resolve(*target_type).unwrap_or("<unknown>").to_string();
        for method in methods {
            if !seen.insert((*target_type, method.name)) {
                continue;
            }
            let method_name = interner.resolve(method.name).unwrap_or("<unknown>").to_string();
            checks.push(FunctionCheck {
                outcome: check_method(
                    &shared,
                    interner,
                    method,
                    &target_name,
                    *target_type,
                    target_type_args,
                    seed,
                    cases,
                ),
                function: format!("{target_name}::{method_name}"),
            });
        }
    }
    CheckReport { seed, checks }
}

fn check_function(
    shared: &crate::SharedRunData<'_>,
    interner: &DefaultStringInterner,
    function: &Rc<Function>,
    seed: u64,
    cases: usize,
) -> CheckOutcome {
    if function.requires.is_empty() && function.ensures.is_empty() {
        return CheckOutcome::Skipped {
            reason: "no `requires` / `ensures` to check against".to_string(),
        };
    }
    if function.ensures.is_empty() {
        return CheckOutcome::Skipped {
            reason: "only `requires`; nothing to falsify without an `ensures` oracle".to_string(),
        };
    }
    if function.is_extern {
        return CheckOutcome::Skipped { reason: "extern fn".to_string() };
    }

    // Seed per function so adding a function upstream does not change
    // every other function's inputs.
    let mut rng = Rng(seed ^ name_hash(interner.resolve(function.name).unwrap_or("")));

    let mut discarded = 0usize;
    let mut executed = 0usize;
    let discard_budget = cases * MAX_DISCARD_RATIO;

    while executed < cases && discarded < discard_budget {
        let mut args = Vec::with_capacity(function.parameter.len());
        for (_, ty) in &function.parameter {
            match sample(&mut rng, ty) {
                Some(v) => args.push(v),
                None => {
                    return CheckOutcome::Skipped {
                        reason: format!("parameter of type {ty:?} cannot be generated yet"),
                    };
                }
            }
        }
        match run_trial(shared, interner, function, &args) {
            Trial::Ok => executed += 1,
            Trial::Discarded => discarded += 1,
            // CHECK-NONTERMINATION: stop the whole function here
            // rather than spending the remaining cases. The inputs
            // are drawn from one distribution, so an input that runs
            // away means the distribution reaches a region this
            // function cannot answer for — trying 199 more costs a
            // budget each and changes nothing about the verdict.
            Trial::Exhausted => {
                return CheckOutcome::Exhausted {
                    cases: executed,
                    discarded,
                    budget: TRIAL_STEP_BUDGET,
                };
            }
            Trial::Failed(detail) => {
                let mut args = args;
                let mut detail = detail;
                shrink_counterexample(&mut args, &mut detail, |candidate| {
                    run_trial(shared, interner, function, candidate)
                });
                let counterexample = function
                    .parameter
                    .iter()
                    .map(|(name, _)| interner.resolve(*name).unwrap_or("?").to_string())
                    .zip(args.iter().map(|v| render(v, interner)))
                    .collect();
                return CheckOutcome::Failed { counterexample, detail };
            }
        }
    }

    if executed == 0 {
        CheckOutcome::Inconclusive { discarded }
    } else {
        CheckOutcome::Passed { cases: executed, discarded }
    }
}

/// Check one impl-block method (DBC-CHECK-METHODS).
///
/// Same rules as [`check_function`] — contracts required, an `ensures`
/// to falsify against, sampled inputs filtered by `requires` — plus a
/// generated receiver: a struct value built by recursively sampling
/// the target type's fields. Generic impls have no concrete
/// instantiation in the source, so the checker tries each entry of
/// [`GENERIC_UNIVERSE`] as a uniform substitution and keeps the first
/// whose receiver and parameters all sample. Methods on types that
/// cannot be generated (enums, primitives, `ptr`-fielded stdlib
/// structs) are skipped with a reason, not silently.
fn check_method(
    shared: &crate::SharedRunData<'_>,
    interner: &DefaultStringInterner,
    method: &Rc<MethodFunction>,
    target_name: &str,
    target_symbol: DefaultSymbol,
    impl_type_args: &[TypeDecl],
    seed: u64,
    cases: usize,
) -> CheckOutcome {
    if method.requires.is_empty() && method.ensures.is_empty() {
        return CheckOutcome::Skipped {
            reason: "no `requires` / `ensures` to check against".to_string(),
        };
    }
    if method.ensures.is_empty() {
        return CheckOutcome::Skipped {
            reason: "only `requires`; nothing to falsify without an `ensures` oracle".to_string(),
        };
    }

    let qualified = format!(
        "{target_name}::{}",
        interner.resolve(method.name).unwrap_or("<unknown>")
    );
    let mut rng = Rng(seed ^ name_hash(&qualified));

    // The receiver: `&self` / `&mut self` never appears in
    // `method.parameter` (the parser only sets `has_self_param`);
    // `self: Self` is a regular first parameter named `self`.
    let first_param_is_self = method
        .parameter
        .first()
        .and_then(|(sym, _)| interner.resolve(*sym))
        .map(|name| name == "self")
        .unwrap_or(false);
    let has_receiver = method.has_self_param || first_param_is_self;

    // Parameters excluding the receiver, for both instantiation
    // probing and sampling below.
    let params: Vec<(DefaultSymbol, TypeDecl)> = if first_param_is_self {
        method.parameter.iter().skip(1).cloned().collect()
    } else {
        method.parameter.clone()
    };

    let (self_type_args, subst) = if has_receiver {
        match instantiate_receiver(&mut rng, shared, target_symbol, impl_type_args, &params) {
            Some((args, subst)) => (args, subst),
            None => {
                let reason = if shared.struct_definitions.contains_key(&target_symbol) {
                    format!("cannot generate a receiver of type `{target_name}`")
                } else {
                    format!("cannot generate a receiver for `{target_name}` (not a struct)")
                };
                return CheckOutcome::Skipped { reason };
            }
        }
    } else {
        (Vec::new(), HashMap::new())
    };

    let mut discarded = 0usize;
    let mut executed = 0usize;
    let discard_budget = cases * MAX_DISCARD_RATIO;

    while executed < cases && discarded < discard_budget {
        // A fresh receiver per trial: a contract like
        // `requires self.n < 100u64` is only exercised if the state
        // varies, and a counterexample must reproduce on its own.
        let mut trial_args: Vec<Value> = Vec::with_capacity(params.len() + 1);
        if has_receiver {
            match sample_struct_value(
                &mut rng,
                shared,
                target_symbol,
                &self_type_args,
                &subst,
                0,
            ) {
                Some(v) => trial_args.push(v),
                None => {
                    return CheckOutcome::Skipped {
                        reason: format!("cannot generate a receiver of type `{target_name}`"),
                    };
                }
            }
        }
        for (_, ty) in &params {
            match sample_typed(&mut rng, shared, ty, &subst, 0) {
                Some(v) => trial_args.push(v),
                None => {
                    return CheckOutcome::Skipped {
                        reason: format!("parameter of type {ty:?} cannot be generated yet"),
                    };
                }
            }
        }
        match run_method_trial(shared, interner, method, &trial_args) {
            Trial::Ok => executed += 1,
            Trial::Discarded => discarded += 1,
            Trial::Exhausted => {
                return CheckOutcome::Exhausted {
                    cases: executed,
                    discarded,
                    budget: TRIAL_STEP_BUDGET,
                };
            }
            Trial::Failed(detail) => {
                let mut detail = detail;
                shrink_counterexample(&mut trial_args, &mut detail, |candidate| {
                    run_method_trial(shared, interner, method, candidate)
                });
                let names: Vec<String> = if has_receiver {
                    vec!["self".to_string()]
                        .into_iter()
                        .chain(params.iter().map(|(name, _)| {
                            interner.resolve(*name).unwrap_or("?").to_string()
                        }))
                        .collect()
                } else {
                    params
                        .iter()
                        .map(|(name, _)| interner.resolve(*name).unwrap_or("?").to_string())
                        .collect()
                };
                let counterexample = names
                    .into_iter()
                    .zip(trial_args.iter().map(|v| render(v, interner)))
                    .collect();
                return CheckOutcome::Failed { counterexample, detail };
            }
        }
    }

    if executed == 0 {
        CheckOutcome::Inconclusive { discarded }
    } else {
        CheckOutcome::Passed { cases: executed, discarded }
    }
}

/// Pick the concrete instantiation for a method receiver.
///
/// Returns `(type_args, substitution)` where the substitution maps the
/// struct's generic parameters to concrete types — it is what the
/// per-trial receiver and parameter sampling apply. For a generic impl
/// (`impl<T> Cell<T>` → symbolic `target_type_args`) each entry of
/// [`GENERIC_UNIVERSE`] is tried as a uniform substitution until both
/// the receiver and every parameter sample; concrete impls
/// (`impl FromStr for Vec<u8>`) keep their args. The receiver value
/// itself is sampled afresh per trial, so a precondition on its fields
/// sees varied states.
fn instantiate_receiver(
    rng: &mut Rng,
    shared: &crate::SharedRunData<'_>,
    target_symbol: DefaultSymbol,
    impl_type_args: &[TypeDecl],
    params: &[(DefaultSymbol, TypeDecl)],
) -> Option<(Vec<TypeDecl>, HashMap<DefaultSymbol, TypeDecl>)> {
    let entry = shared.struct_definitions.get(&target_symbol)?;
    let mut probes_ok = |concrete_args: &[TypeDecl], subst: &HashMap<DefaultSymbol, TypeDecl>| {
        sample_struct_value(rng, shared, target_symbol, concrete_args, subst, 0).is_some()
            && params
                .iter()
                .all(|(_, ty)| sample_typed(rng, shared, ty, subst, 0).is_some())
    };
    if entry.generic_params.is_empty() {
        let subst = HashMap::new();
        if probes_ok(&[], &subst) {
            return Some((Vec::new(), subst));
        }
        return None;
    }
    for candidate in GENERIC_UNIVERSE {
        let concrete_args: Vec<TypeDecl> = if impl_type_args.is_empty() {
            vec![candidate.clone()]
        } else {
            impl_type_args
                .iter()
                .map(|a| match a {
                    TypeDecl::Generic(_) => candidate.clone(),
                    other => other.clone(),
                })
                .collect()
        };
        let subst: HashMap<DefaultSymbol, TypeDecl> = entry
            .generic_params
            .iter()
            .copied()
            .zip(concrete_args.iter().cloned())
            .collect();
        if probes_ok(&concrete_args, &subst) {
            return Some((concrete_args, subst));
        }
    }
    None
}

/// FNV-1a-ish mix of a callable name; per-callable seed offset so
/// adding one function or method upstream does not change every other
/// check's inputs.
fn name_hash(name: &str) -> u64 {
    name.bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
        })
}

/// Greedily replace each argument with a simpler one that still fails.
///
/// One pass per argument, simplest candidate first, repeated until
/// nothing improves. Enough to turn machine noise into something a
/// reader can hold in their head, which is the entire point. The
/// `trial` closure keeps this shared between free functions and
/// methods (whose first "argument" is the receiver).
///
/// Only `Trial::Failed` accepts a candidate, so a candidate whose run
/// exhausts the step budget is rejected like one that passes: it did
/// not reproduce the failure, so it cannot replace the argument that
/// did (CHECK-NONTERMINATION).
fn shrink_counterexample<F>(
    args: &mut Vec<Value>,
    detail: &mut String,
    trial: F,
) where
    F: Fn(&[Value]) -> Trial,
{
    let mut improved = true;
    // Bounded so a pathological shrink cannot outlast the check itself.
    let mut rounds = 0;
    while improved && rounds < 32 {
        improved = false;
        rounds += 1;
        for i in 0..args.len() {
            for candidate in shrink(&args[i]) {
                let mut trial_args = args.clone();
                trial_args[i] = candidate;
                if let Trial::Failed(d) = trial(&trial_args) {
                    *args = trial_args;
                    *detail = d;
                    improved = true;
                    break;
                }
            }
        }
    }
}

/// Parse, type check, and property-check `source`.
pub fn check_source(
    source: &str,
    filename: &str,
    options: &crate::RunOptions<'_>,
    seed: u64,
    cases: Option<usize>,
) -> Result<CheckReport, String> {
    let formatter = crate::error_formatter::ErrorFormatter::new(source, filename);
    let mut session = compiler_core::CompilerSession::new();
    let mut program = match session.parse_program_all_errors(source, filename) {
        Ok(p) => p,
        Err(errors) => {
            if options.diagnostics_json {
                let diagnostics: Vec<frontend::diagnostic::Diagnostic> = errors
                    .iter()
                    .map(|e| frontend::diagnostic::Diagnostic::from_parser_error(e, filename))
                    .collect();
                crate::emit_diagnostics_json(&diagnostics);
            } else {
                formatter.display_parse_errors(&errors);
            }
            return Err(format!("{} parse error(s)", errors.len()));
        }
    };
    if let Err(diagnostics) = crate::check_typing_diagnostics(
        &mut program,
        session.string_interner_mut(),
        Some(source),
        Some(filename),
        options.core_modules_dir,
    ) {
        let rendered: Vec<String> = diagnostics
            .iter()
            .map(|d| formatter.format_diagnostic(d))
            .collect();
        formatter.display_type_check_errors(&rendered);
        return Err(format!("{} type-check error(s)", diagnostics.len()));
    }
    Ok(check_program(
        &program,
        session.string_interner_mut(),
        seed,
        cases.unwrap_or(DEFAULT_CASES),
    ))
}
