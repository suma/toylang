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

use std::rc::Rc;

use frontend::ast::{File, Function};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultStringInterner;

use crate::error::InterpreterError;
use crate::value::Value;

/// How many inputs to try per function before declaring it healthy.
const DEFAULT_CASES: usize = 200;

/// Give up after this many consecutive `requires` rejections — the
/// precondition is too narrow for uniform sampling to hit.
const MAX_DISCARD_RATIO: usize = 20;

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
        _ => return None,
    })
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
                out.push(Value::Int64(v.wrapping_neg()));
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
        _ => vec![],
    }
}

fn render(value: &Value) -> String {
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Int64(v) => format!("{v}i64"),
        Value::UInt64(v) => format!("{v}u64"),
        Value::Float64(v) => format!("{v}f64"),
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
}

fn run_trial(
    program: &File,
    interner: &DefaultStringInterner,
    function: &Rc<Function>,
    args: &[Value],
) -> Trial {
    // A fresh context per trial: contracts can mutate globals and the
    // heap, and a counterexample that only reproduces after some other
    // trial ran is not a counterexample anyone can act on.
    let outcome = crate::execute_function_with_values(program, interner, function.clone(), args);
    match outcome {
        Ok(_) => Trial::Ok,
        Err(InterpreterError::ContractViolation { kind: "requires", .. }) => Trial::Discarded,
        Err(e) => Trial::Failed(e.to_string()),
    }
}

/// Check every contracted function in `program`.
pub fn check_program(
    program: &File,
    interner: &DefaultStringInterner,
    seed: u64,
    cases: usize,
) -> CheckReport {
    let mut checks = Vec::new();
    for function in &program.function {
        let name = interner.resolve(function.name).unwrap_or("<unknown>").to_string();
        if name.starts_with("__test_") {
            continue;
        }
        checks.push(FunctionCheck {
            outcome: check_function(program, interner, function, seed, cases),
            function: name,
        });
    }
    CheckReport { seed, checks }
}

fn check_function(
    program: &File,
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
    let name_hash = interner
        .resolve(function.name)
        .unwrap_or("")
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
        });
    let mut rng = Rng(seed ^ name_hash);

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
        match run_trial(program, interner, function, &args) {
            Trial::Ok => executed += 1,
            Trial::Discarded => discarded += 1,
            Trial::Failed(detail) => {
                let (args, detail) = shrink_counterexample(program, interner, function, args, detail);
                let counterexample = function
                    .parameter
                    .iter()
                    .map(|(name, _)| interner.resolve(*name).unwrap_or("?").to_string())
                    .zip(args.iter().map(render))
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

/// Greedily replace each argument with a simpler one that still fails.
///
/// One pass per argument, simplest candidate first, repeated until
/// nothing improves. Enough to turn machine noise into something a
/// reader can hold in their head, which is the entire point.
fn shrink_counterexample(
    program: &File,
    interner: &DefaultStringInterner,
    function: &Rc<Function>,
    mut args: Vec<Value>,
    mut detail: String,
) -> (Vec<Value>, String) {
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
                if let Trial::Failed(d) = run_trial(program, interner, function, &trial_args) {
                    args = trial_args;
                    detail = d;
                    improved = true;
                    break;
                }
            }
        }
    }
    (args, detail)
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
        session.string_interner(),
        seed,
        cases.unwrap_or(DEFAULT_CASES),
    ))
}
