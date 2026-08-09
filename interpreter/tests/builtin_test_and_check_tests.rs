// LLM-LOOP P4 / P5 — built-in tests and contract-driven property checks.
//
// P4: `test "name" { ... }` blocks, run by `--test`, with `assert_eq`
// reporting both values.
// P5: `requires` read as a generator filter and `ensures` as an oracle,
// producing a shrunk counterexample without the author writing a test.

mod common;

use common::core_modules_dir;
use interpreter::property::CheckOutcome;
use interpreter::RunOptions;

fn options(core: &std::path::Path) -> RunOptions<'_> {
    let mut o = RunOptions::default();
    o.core_modules_dir = Some(core);
    o
}

// --- P4: `test` blocks -----------------------------------------------

fn run_tests(source: &str) -> Vec<interpreter::TestOutcome> {
    let core = core_modules_dir();
    interpreter::run_tests_from_source(source, "test.t", &options(core.as_path()))
        .expect("program should type check")
}

#[test]
fn a_passing_test_block_reports_no_failure() {
    let outcomes = run_tests(
        "fn add(a: i64, b: i64) -> i64 { a + b }
        test \"add sums\" {
            assert_eq(add(1i64, 2i64), 3i64)
        }
        fn main() -> u64 { 0u64 }",
    );
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].name, "add sums");
    assert!(outcomes[0].failure.is_none(), "{:?}", outcomes[0]);
}

#[test]
fn a_failing_test_reports_both_values_and_the_line() {
    let outcomes = run_tests(
        "fn add(a: i64, b: i64) -> i64 { a + b }
        test \"add is wrong\" {
            assert_eq(add(1i64, 2i64), 4i64)
        }
        fn main() -> u64 { 0u64 }",
    );
    let failure = outcomes[0]
        .failure
        .as_deref()
        .unwrap_or_else(|| panic!("expected a failure: {:?}", outcomes[0]));
    // The point of `assert_eq` over `assert`: the two values are in the
    // diagnostic, so no print-and-rerun cycle is needed.
    assert!(failure.contains("left:  3"), "{failure}");
    assert!(failure.contains("right: 4"), "{failure}");
    assert!(failure.contains("test.t:3:"), "should cite the line:\n{failure}");
}

#[test]
fn tests_are_independent_of_each_other() {
    // Each test gets its own evaluation context; one blowing up must
    // not decide the next one's outcome.
    let outcomes = run_tests(
        "test \"first fails\" {
            assert_eq(1i64, 2i64)
        }
        test \"second passes\" {
            assert_eq(2i64, 2i64)
        }
        fn main() -> u64 { 0u64 }",
    );
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes[0].failure.is_some());
    assert!(outcomes[1].failure.is_none(), "{:?}", outcomes[1]);
}

#[test]
fn test_blocks_do_not_run_during_a_normal_execution() {
    // A test that would fail must not affect `main`.
    common::assert_program_result_u64(
        "test \"would fail\" {
            assert_eq(1i64, 2i64)
        }
        fn main() -> u64 { 42u64 }",
        42,
    );
}

#[test]
fn test_is_still_usable_as_an_ordinary_identifier() {
    // `test` is contextual, not a keyword: only `test "..." {` at top
    // level starts a block. Reserving the word would break existing
    // programs that use it as a name.
    common::assert_program_result_u64(
        "fn test(n: u64) -> u64 { n + 1u64 }
        fn main() -> u64 {
            val test = 41u64
            test(test)
        }",
        42,
    );
}

// --- P5: contract-driven property checks -----------------------------

fn check(source: &str, seed: u64) -> interpreter::property::CheckReport {
    let core = core_modules_dir();
    interpreter::property::check_source(source, "check.t", &options(core.as_path()), seed, Some(200))
        .expect("program should type check")
}

fn outcome_for<'a>(
    report: &'a interpreter::property::CheckReport,
    name: &str,
) -> &'a CheckOutcome {
    &report
        .checks
        .iter()
        .find(|c| c.function == name)
        .unwrap_or_else(|| panic!("no check for `{name}`"))
        .outcome
}

#[test]
fn a_false_ensures_produces_a_counterexample() {
    // `result * b == a` does not hold for integer division.
    let report = check(
        "fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
            ensures  result * b == a
        {
            a / b
        }
        fn main() -> i64 { divide(20i64, 4i64) }",
        0x1234,
    );
    match outcome_for(&report, "divide") {
        CheckOutcome::Failed { counterexample, detail } => {
            assert_eq!(counterexample.len(), 2);
            assert!(detail.contains("ensures"), "{detail}");
        }
        other => panic!("expected a counterexample, got {other:?}"),
    }
}

#[test]
fn the_counterexample_is_shrunk_to_small_values() {
    // Reporting the raw failing draw would just move the work onto the
    // reader; shrinking is what makes the output actionable.
    let report = check(
        "fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
            ensures  result * b == a
        {
            a / b
        }
        fn main() -> i64 { divide(20i64, 4i64) }",
        0x1234,
    );
    let CheckOutcome::Failed { counterexample, .. } = outcome_for(&report, "divide") else {
        panic!("expected a counterexample");
    };
    for (name, rendered) in counterexample {
        let magnitude: i64 = rendered
            .trim_end_matches("i64")
            .parse()
            .unwrap_or_else(|_| panic!("unparsable value for {name}: {rendered}"));
        assert!(
            magnitude.abs() < 100,
            "`{name}` was not shrunk: {rendered} (whole example: {counterexample:?})"
        );
    }
}

#[test]
fn a_true_ensures_passes() {
    let report = check(
        "fn double(x: i64) -> i64
            ensures result == x + x
        {
            x * 2i64
        }
        fn main() -> i64 { double(2i64) }",
        0x1234,
    );
    assert!(
        matches!(outcome_for(&report, "double"), CheckOutcome::Passed { .. }),
        "{:?}",
        outcome_for(&report, "double")
    );
}

#[test]
fn a_requires_rejection_is_not_reported_as_a_failure() {
    // Inputs the function never promised to handle are discarded, not
    // counted against it.
    let report = check(
        "fn only_positive(x: i64) -> i64
            requires x > 0i64
            ensures  result == x
        {
            x
        }
        fn main() -> i64 { only_positive(1i64) }",
        0x1234,
    );
    assert!(
        matches!(outcome_for(&report, "only_positive"), CheckOutcome::Passed { .. }),
        "{:?}",
        outcome_for(&report, "only_positive")
    );
}

#[test]
fn an_unsatisfiable_requires_is_inconclusive_rather_than_passing() {
    // Every input rejected means the `ensures` was never exercised.
    // Calling that a pass would be a lie.
    let report = check(
        "fn impossible(x: i64) -> i64
            requires x > 0i64
            requires x < 0i64
            ensures  result == x
        {
            x
        }
        fn main() -> i64 { 0i64 }",
        0x1234,
    );
    assert!(
        matches!(
            outcome_for(&report, "impossible"),
            CheckOutcome::Inconclusive { .. }
        ),
        "{:?}",
        outcome_for(&report, "impossible")
    );
}

#[test]
fn functions_without_an_ensures_are_skipped() {
    let report = check(
        "fn plain(x: i64) -> i64 { x }
        fn main() -> i64 { plain(1i64) }",
        0x1234,
    );
    assert!(
        matches!(outcome_for(&report, "plain"), CheckOutcome::Skipped { .. }),
        "{:?}",
        outcome_for(&report, "plain")
    );
}

#[test]
fn the_same_seed_reproduces_the_same_counterexample() {
    // The seed is printed precisely so a failure can be replayed; if it
    // did not determine the result, printing it would be theatre.
    let source = "fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
            ensures  result * b == a
        {
            a / b
        }
        fn main() -> i64 { divide(20i64, 4i64) }";
    let first = check(source, 0xABCD);
    let second = check(source, 0xABCD);
    let (CheckOutcome::Failed { counterexample: a, .. }, CheckOutcome::Failed { counterexample: b, .. }) =
        (outcome_for(&first, "divide"), outcome_for(&second, "divide"))
    else {
        panic!("expected both runs to fail");
    };
    assert_eq!(a, b, "same seed produced different counterexamples");
}
