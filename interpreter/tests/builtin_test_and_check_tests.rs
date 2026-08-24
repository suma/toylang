// LLM-LOOP P4 / P5 — built-in tests and contract-driven property checks.
//
// P4: `test "name" { ... }` blocks, run by `--test`, with `assert_eq`
// reporting both values.
// P5: `requires` read as a generator filter and `ensures` as an oracle,
// producing a shrunk counterexample without the author writing a test.

use crate::common;

use crate::common::core_modules_dir;
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
    check_with_cases(source, seed, 200)
}

/// `check` with an explicit case budget. Only worth spelling out when
/// the test is about *rejected* inputs: the checker keeps generating
/// until `cases * MAX_DISCARD_RATIO` (20x) inputs have been discarded,
/// so a `requires` nothing satisfies costs 20 generations per case and
/// nothing else in the suite comes close.
fn check_with_cases(
    source: &str,
    seed: u64,
    cases: usize,
) -> interpreter::property::CheckReport {
    let core = core_modules_dir();
    interpreter::property::check_source(
        source,
        "check.t",
        &options(core.as_path()),
        seed,
        Some(cases),
    )
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
fn a_pass_records_how_much_was_actually_tried() {
    // DBC-CHECK-CASES: a pass over 200 inputs and a pass over one are
    // very different claims. The outcome carries both halves so the
    // caller can tell them apart — `--check` prints a THIN line for
    // the second kind, which used to look exactly like the first.
    let broad = check(
        "fn double(x: i64) -> i64
            requires x > 0i64
            ensures  result == x * 2i64
        {
            x * 2i64
        }
        fn main() -> i64 { double(1i64) }",
        0x1234,
    );
    match outcome_for(&broad, "double") {
        CheckOutcome::Passed { cases, discarded } => {
            assert_eq!(*cases, 200, "the whole budget should have been used");
            assert!(
                *cases * 10 > *discarded,
                "{cases} case(s) against {discarded} discards should not read as thin"
            );
        }
        other => panic!("expected a pass, got {other:?}"),
    }

    let narrow = check(
        "fn exact(x: i64) -> i64
            requires x == 42i64
            ensures  result == 43i64
        {
            x + 1i64
        }
        fn main() -> i64 { exact(42i64) }",
        0x1234,
    );
    match outcome_for(&narrow, "exact") {
        CheckOutcome::Passed { cases, discarded } => {
            assert!(
                *cases * 10 < *discarded,
                "a pass resting on {cases} case(s) against {discarded} discards should be \
                 recognisable as thin"
            );
        }
        // Drawing 42 is luck; with none the run is inconclusive, which
        // is already reported as its own thing.
        CheckOutcome::Inconclusive { .. } => {}
        other => panic!("expected a pass or inconclusive, got {other:?}"),
    }
}

#[test]
fn an_unsatisfiable_requires_is_inconclusive_rather_than_passing() {
    // Every input rejected means the `ensures` was never exercised.
    // Calling that a pass would be a lie.
    //
    // 10 cases, not the usual 200: every generated input is discarded
    // here, so the run costs the full 20x discard budget either way —
    // 4000 generations to reach the same verdict 200 do.
    let report = check_with_cases(
        "fn impossible(x: i64) -> i64
            requires x > 0i64
            requires x < 0i64
            ensures  result == x
        {
            x
        }
        fn main() -> i64 { 0i64 }",
        0x1234,
        10,
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

// --- DBC-CHECK-METHODS: methods are checked too ---------------------

#[test]
fn a_method_with_contracts_is_checked() {
    let report = check(
        "struct Point {
            x: i64,
            y: i64,
        }
        impl Point {
            fn dist_sq(self: Self) -> i64
                ensures result == self.x * self.x + self.y * self.y
            {
                self.x * self.x + self.y * self.y
            }
        }
        fn main() -> u64 { 0u64 }",
        0x1234,
    );
    match outcome_for(&report, "Point::dist_sq") {
        CheckOutcome::Passed { cases, .. } => assert_eq!(*cases, 200),
        other => panic!("expected a pass, got {other:?}"),
    }
}

#[test]
fn a_false_method_ensures_produces_a_counterexample() {
    let report = check(
        "struct Point {
            x: i64,
            y: i64,
        }
        impl Point {
            fn wrong(self: Self) -> i64
                ensures result == 0i64
            {
                self.x + self.y
            }
        }
        fn main() -> u64 { 0u64 }",
        0x1234,
    );
    match outcome_for(&report, "Point::wrong") {
        CheckOutcome::Failed { counterexample, detail } => {
            let rendered = format!("{counterexample:?}");
            assert!(rendered.contains("self"), "receiver missing: {rendered}");
            assert!(detail.contains("ensures"), "{detail}");
        }
        other => panic!("expected a counterexample, got {other:?}"),
    }
}

#[test]
fn a_method_receiver_is_shrunk_like_an_argument() {
    // The failure only depends on `self.x`, so the counterexample must
    // come with the smallest `self.x` that still fails — 1, not the
    // raw draw.
    let report = check(
        "struct Big {
            x: i64,
        }
        impl Big {
            fn fail(&self) -> i64
                ensures result < 1i64
            {
                self.x
            }
        }
        fn main() -> u64 { 0u64 }",
        0x1234,
    );
    let CheckOutcome::Failed { counterexample, .. } = outcome_for(&report, "Big::fail") else {
        panic!("expected a counterexample");
    };
    let rendered = counterexample
        .iter()
        .map(|(name, value)| format!("{name} = {value}"))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(rendered.contains("self = Big { x: 1 }"), "{rendered}");
}

#[test]
fn a_method_requires_rejection_is_discarded() {
    // `requires self.n < 100u64` turns away roughly a third of the
    // sampled receivers (the edge values 0..=2 and u64::MAX-1..); the
    // pass must survive the discards.
    let report = check(
        "struct Counter {
            n: u64,
        }
        impl Counter {
            fn inc(&mut self) -> u64
                requires self.n < 100u64
                ensures  result == self.n + 1u64
            {
                self.n + 1u64
            }
        }
        fn main() -> u64 { 0u64 }",
        0x1234,
    );
    match outcome_for(&report, "Counter::inc") {
        CheckOutcome::Passed { cases, discarded } => {
            assert_eq!(*cases, 200, "the whole budget should have been used");
            assert!(*discarded > 0, "a receiver must have been rejected");
        }
        other => panic!("expected a pass, got {other:?}"),
    }
}

#[test]
fn an_associated_function_is_checked() {
    // No receiver to generate — the other parameters alone carry the
    // contract.
    let report = check(
        "struct Counter {
            n: u64,
        }
        impl Counter {
            fn from(v: u64) -> Counter
                ensures result.n == v
            {
                Counter { n: v }
            }
        }
        fn main() -> u64 { 0u64 }",
        0x1234,
    );
    match outcome_for(&report, "Counter::from") {
        CheckOutcome::Passed { cases, .. } => assert_eq!(*cases, 200),
        other => panic!("expected a pass, got {other:?}"),
    }
}

#[test]
fn a_generic_method_is_checked() {
    // `impl<T> Cell<T>` has no concrete instantiation in the source;
    // the checker substitutes the first sampleable type and checks
    // against it.
    let report = check(
        "struct Cell<T> {
            value: T,
        }
        impl<T> Cell<T> {
            fn get(&self) -> T
                ensures result == self.value
            {
                self.value
            }
        }
        fn main() -> u64 { 0u64 }",
        0x1234,
    );
    match outcome_for(&report, "Cell::get") {
        CheckOutcome::Passed { cases, .. } => assert_eq!(*cases, 200),
        other => panic!("expected a pass, got {other:?}"),
    }
}

#[test]
fn methods_without_contracts_are_skipped() {
    let report = check(
        "struct Point {
            x: i64,
        }
        impl Point {
            fn plain(&self) -> i64 { self.x }
        }
        fn main() -> u64 { 0u64 }",
        0x1234,
    );
    assert!(
        matches!(
            outcome_for(&report, "Point::plain"),
            CheckOutcome::Skipped { .. }
        ),
        "{:?}",
        outcome_for(&report, "Point::plain")
    );
}

#[test]
fn methods_on_types_without_a_generatable_receiver_are_skipped() {
    // Enum receivers (and primitive extension-trait targets) cannot be
    // generated yet; the skip must say so rather than pass silently.
    let report = check(
        "enum Box_ {
            Empty,
            Full(i64),
        }
        impl Box_ {
            fn kind(&self) -> i64
                ensures result >= 0i64
            {
                1i64
            }
        }
        fn main() -> u64 { 0u64 }",
        0x1234,
    );
    match outcome_for(&report, "Box_::kind") {
        CheckOutcome::Skipped { reason } => {
            assert!(reason.contains("not a struct"), "{reason}");
        }
        other => panic!("expected a skip, got {other:?}"),
    }
}

// --- CHECK-NONTERMINATION: the step budget ---------------------------

#[test]
fn an_input_that_never_finishes_is_reported_not_waited_for() {
    // `triangle` is correct and terminates for every `n` a caller
    // would pass. It is the *sampler* that reaches `n = u64::MAX`,
    // where `while i <= n` runs for longer than anyone will wait — so
    // before the budget this hung `--check` on a program with no bug
    // in it at all.
    let report = check(
        "fn triangle(n: u64) -> u64
            ensures result >= 0u64
        {
            var total: u64 = 0u64
            var i: u64 = 1u64
            while i <= n {
                total = total + i
                i = i + 1u64
            }
            total
        }
        fn main() -> u64 { triangle(3u64) }",
        0x99,
    );
    match outcome_for(&report, "triangle") {
        CheckOutcome::Exhausted { budget, .. } => assert!(*budget > 0),
        other => panic!("expected the budget to be reported, got {other:?}"),
    }
}

#[test]
fn a_requires_that_bounds_the_loop_makes_the_check_finish() {
    // The other half of the report: EXHAUSTED tells the author to
    // bound the inputs, and doing so has to actually work. Same
    // function, same seed, one added clause.
    let report = check(
        "fn triangle(n: u64) -> u64
            requires n <= 1000u64
            ensures result >= 0u64
        {
            var total: u64 = 0u64
            var i: u64 = 1u64
            while i <= n {
                total = total + i
                i = i + 1u64
            }
            total
        }
        fn main() -> u64 { triangle(3u64) }",
        0x99,
    );
    match outcome_for(&report, "triangle") {
        CheckOutcome::Passed { cases, .. } => assert_eq!(*cases, 200),
        other => panic!("expected a pass, got {other:?}"),
    }
}

#[test]
fn a_for_loop_over_a_sampled_range_is_bounded_too() {
    // The other back-edge. A range ending at a parameter is
    // unremarkable in source and unbounded under sampling, exactly
    // like the `while` above.
    let report = check(
        "fn sum_to(n: u64) -> u64
            ensures result >= 0u64
        {
            var total: u64 = 0u64
            for i in 0u64 to n {
                total = total + i
            }
            total
        }
        fn main() -> u64 { sum_to(3u64) }",
        0x99,
    );
    assert!(
        matches!(outcome_for(&report, "sum_to"), CheckOutcome::Exhausted { .. }),
        "{:?}",
        outcome_for(&report, "sum_to")
    );
}

#[test]
fn the_budget_is_deterministic_across_runs() {
    // The budget counts loop iterations rather than elapsed time
    // precisely so that `--check --seed=X` keeps its promise to
    // replay. A wall-clock cut would make this test flaky by design.
    let source = "fn triangle(n: u64) -> u64
            ensures result >= 0u64
        {
            var total: u64 = 0u64
            var i: u64 = 1u64
            while i <= n {
                total = total + i
                i = i + 1u64
            }
            total
        }
        fn main() -> u64 { triangle(3u64) }";
    let (a, b) = (check(source, 0x7777), check(source, 0x7777));
    let (
        CheckOutcome::Exhausted { cases: ca, discarded: da, budget: ba },
        CheckOutcome::Exhausted { cases: cb, discarded: db, budget: bb },
    ) = (outcome_for(&a, "triangle"), outcome_for(&b, "triangle"))
    else {
        panic!("expected both runs to hit the budget");
    };
    assert_eq!((ca, da, ba), (cb, db, bb));
}

#[test]
fn ordinary_execution_is_not_capped() {
    // The budget belongs to `--check`, which calls functions with
    // inputs nobody wrote them for. A program the user asked to run
    // gets to loop as long as it likes, well past the budget.
    common::assert_program_result_u64(
        "fn main() -> u64 {
            var total: u64 = 0u64
            var i: u64 = 0u64
            while i < 300000u64 {
                total = total + 1u64
                i = i + 1u64
            }
            total
        }",
        300000,
    );
}
