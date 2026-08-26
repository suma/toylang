// COMPILE-TIME-EVAL: `const fn f()` — a function the compiler may run
// while compiling, and the fold that runs it.
//
// C1 is the declaration and its eligibility check (a reachability walk
// with a different sink set than `never_allocates`); C3 is the driver
// pass that evaluates calls and leaves literals behind. The pair is
// what makes `const D: u64 = double(21u64)` mean the same thing on all
// four execution engines — before it, the tree-walker answered 42 and
// the other three refused to compile the program.

use crate::common::{assert_program_result_u64, test_program};

// --- C1: the declaration and its eligibility check ----------------

#[test]
fn a_pure_function_may_be_declared_const_fn() {
    assert_program_result_u64(
        r#"
        const fn double(n: u64) -> u64 { n * 2u64 }
        fn main() -> u64 { double(3u64) }
        "#,
        6,
    );
}

#[test]
fn a_const_fn_may_call_an_unannotated_function() {
    // Reachability, not a propagated attribute: the stdlib needs no
    // annotation pass for a `const fn` to use it.
    assert_program_result_u64(
        r#"
        fn helper(n: u64) -> u64 { n + 1u64 }
        const fn plus_one(n: u64) -> u64 { helper(n) }
        fn main() -> u64 { plus_one(41u64) }
        "#,
        42,
    );
}

#[test]
fn printing_is_refused_with_the_path() {
    let err = test_program(
        r#"
        const fn noisy(n: u64) -> u64 {
            println("hi")
            n
        }
        fn main() -> u64 { noisy(1u64) }
        "#,
    )
    .expect_err("a fold would write to the compiler's stdout");
    assert!(err.contains("E0017"), "{err}");
    assert!(err.contains("noisy -> println"), "{err}");
}

#[test]
fn allocating_is_refused_through_a_callee() {
    let err = test_program(
        r#"
        fn inner(n: u64) -> u64 {
            val p: ptr = __builtin_heap_alloc(32u64)
            n
        }
        const fn outer(n: u64) -> u64 { inner(n) }
        fn main() -> u64 { outer(1u64) }
        "#,
    )
    .expect_err("a folded value has to fit in a scalar constant");
    assert!(err.contains("E0017"), "{err}");
    assert!(err.contains("outer -> inner -> __builtin_heap_alloc"), "{err}");
}

#[test]
fn an_extern_call_gets_no_escape_hatch() {
    // `never_allocates` lets an author declare an `extern fn`
    // allocation-free. There is no equivalent here: an honest
    // declaration still gives the compiler no way to *call* the
    // implementation while compiling.
    let err = test_program(
        r#"
        never_allocates extern fn getchar() -> i32 from "c"
        const fn read() -> i32 { getchar() }
        fn main() -> u64 { 0u64 }
        "#,
    )
    .expect_err("there is nothing to run at compile time");
    assert!(err.contains("E0017"), "{err}");
    assert!(err.contains("extern fn"), "{err}");
}

#[test]
fn never_allocates_and_const_fn_compose_in_either_order() {
    assert_program_result_u64(
        r#"
        const never_allocates fn a(n: u64) -> u64 { n * 2u64 }
        never_allocates const fn b(n: u64) -> u64 { n * 3u64 }
        fn main() -> u64 { a(1u64) + b(1u64) }
        "#,
        5,
    );
}

#[test]
fn const_stays_a_declaration_keyword() {
    // `const` is one token doing two jobs; what follows it decides.
    assert_program_result_u64(
        r#"
        const N: u64 = 20u64
        const fn double(n: u64) -> u64 { n * 2u64 }
        fn main() -> u64 { double(N) + 2u64 }
        "#,
        42,
    );
}

// --- C3: the fold ------------------------------------------------

#[test]
fn a_const_initialiser_may_call_a_const_fn() {
    // The program that used to answer 42 on the tree-walker and fail
    // to compile everywhere else.
    assert_program_result_u64(
        r#"
        const fn double(n: u64) -> u64 { n * 2u64 }
        const D: u64 = double(21u64)
        fn main() -> u64 { D }
        "#,
        42,
    );
}

#[test]
fn a_const_initialiser_may_build_on_an_earlier_const() {
    assert_program_result_u64(
        r#"
        const fn double(n: u64) -> u64 { n * 2u64 }
        const A: u64 = double(3u64)
        const B: u64 = double(A)
        fn main() -> u64 { B }
        "#,
        12,
    );
}

#[test]
fn a_const_initialiser_calling_an_unannotated_function_is_refused() {
    let err = test_program(
        r#"
        fn plain(n: u64) -> u64 { n * 2u64 }
        const D: u64 = plain(21u64)
        fn main() -> u64 { D }
        "#,
    )
    .expect_err("the initialiser has to be known before the program runs");
    assert!(err.contains("E0017"), "{err}");
    assert!(err.contains("not declared `const fn`"), "{err}");
}

#[test]
fn a_forced_fold_that_traps_is_a_compile_error() {
    let err = test_program(
        r#"
        const fn boom(n: u64) -> u64 { n / 0u64 }
        const D: u64 = boom(21u64)
        fn main() -> u64 { D }
        "#,
    )
    .expect_err("a value that cannot exist is not worth compiling");
    assert!(err.contains("E0017"), "{err}");
    assert!(err.contains("division by zero"), "{err}");
}

#[test]
fn a_forced_fold_that_breaks_a_contract_is_a_compile_error() {
    // The `requires` is checked while compiling, with the offending
    // value in the message — DbC's half of the motivation.
    let err = test_program(
        r#"
        const fn half(n: u64) -> u64 requires n % 2u64 == 0u64 { n / 2u64 }
        const D: u64 = half(3u64)
        fn main() -> u64 { D }
        "#,
    )
    .expect_err("the precondition does not hold for a constant argument");
    assert!(err.contains("E0017"), "{err}");
    assert!(err.contains("requires"), "{err}");
    assert!(err.contains("n = 3"), "{err}");
}

#[test]
fn a_non_terminating_fold_spends_a_budget_and_stops() {
    let err = test_program(
        r#"
        const fn spin(n: u64) -> u64 {
            var i: u64 = 0u64
            while true { i = i + 1u64 }
            n
        }
        const D: u64 = spin(1u64)
        fn main() -> u64 { D }
        "#,
    )
    .expect_err("the compiler does not get to hang");
    assert!(err.contains("E0017"), "{err}");
    assert!(err.contains("loop iterations"), "{err}");
}

#[test]
fn an_unforced_fold_that_would_trap_leaves_the_call_alone() {
    // The rule that keeps folding out of the reachability business:
    // nothing forced this call, the branch never runs, and the
    // program is legal. C++ draws the same line — a `constexpr`
    // function called outside a constant context is an ordinary call.
    assert_program_result_u64(
        r#"
        const fn boom(n: u64) -> u64 { n / 0u64 }
        fn main() -> u64 {
            if false { boom(1u64) } else { 7u64 }
        }
        "#,
        7,
    );
}

#[test]
fn folding_covers_every_scalar_width() {
    assert_program_result_u64(
        r#"
        const fn narrow(n: u8) -> u8 { n * 2u8 }
        const fn signed(n: i64) -> i64 { 0i64 - n }
        const fn flag(n: u64) -> bool { n > 1u64 }
        const A: u8 = narrow(3u8)
        const B: i64 = signed(5i64)
        const C: bool = flag(2u64)
        fn main() -> u64 {
            var total: u64 = A as u64
            total = total + ((0i64 - B) as u64)
            if C { total = total + 1u64 }
            total
        }
        "#,
        12,
    );
}

// --- C4: the contract connection ---------------------------------

/// Warnings do not stop the program, so they are collected from the
/// structured type-check result rather than from a failure.
fn warnings_for(source: &str) -> Vec<String> {
    let mut session = compiler_core::CompilerSession::new();
    let mut program = session
        .parse_program_all_errors(source, "test.t")
        .expect("parse");
    let core = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../core"));
    interpreter::check_typing_diagnostics(
        &mut program,
        session.string_interner_mut(),
        Some(source),
        Some("test.t"),
        Some(core.as_path()),
    )
    .expect("the program type-checks; the findings are warnings")
    .iter()
    .map(|d| format!("[{}] {}", d.code, d.message))
    .collect()
}

#[test]
fn an_impure_contract_predicate_is_warned_about() {
    // COMPILE_TIME_EVAL.md 実測 6: this program printed `checking`
    // with `INTERPRETER_CONTRACTS=off`, because the switch is read by
    // the tree-walker while the default engine has the checks lowered
    // into the IR. A contract has to be a statement about the program
    // rather than a part of it.
    let warnings = warnings_for(
        r#"
        fn noisy(n: u64) -> bool {
            println("checking")
            n > 0u64
        }
        fn f(n: u64) -> u64 requires noisy(n) { n }
        fn main() -> u64 { f(3u64) }
        "#,
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("E0018"), "{warnings:?}");
    assert!(warnings[0].contains("f` -> noisy -> println"), "{warnings:?}");
}

#[test]
fn a_pure_contract_predicate_is_not_warned_about() {
    // Reachability, not annotation: `is_even` needs no `const fn` to
    // be an acceptable predicate.
    assert!(
        warnings_for(
            r#"
            fn is_even(n: u64) -> bool { n % 2u64 == 0u64 }
            fn half(n: u64) -> u64 requires is_even(n) { n / 2u64 }
            fn main() -> u64 { half(8u64) }
            "#,
        )
        .is_empty()
    );
}

#[test]
fn reading_the_allocation_counters_from_a_contract_stays_legal() {
    // ALLOC-CONTRACT exists to let a contract talk about memory. A
    // counter read changes nothing, so it is not an effect.
    assert!(
        warnings_for(
            r#"
            fn quiet(n: u64) -> u64
                ensures __builtin_live_bytes() == old(__builtin_live_bytes())
            { n }
            fn main() -> u64 { quiet(1u64) }
            "#,
        )
        .is_empty()
    );
}

#[test]
fn a_constant_call_that_breaks_its_precondition_is_warned_about() {
    let warnings = warnings_for(
        r#"
        const fn half(n: u64) -> u64 requires n % 2u64 == 0u64 { n / 2u64 }
        fn main() -> u64 { half(3u64) }
        "#,
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("E0018"), "{warnings:?}");
    assert!(warnings[0].contains("breaks its own precondition"), "{warnings:?}");
    // The value the predicate saw, so the reader need not work it out.
    assert!(warnings[0].contains("n = 3"), "{warnings:?}");
}

#[test]
fn a_constant_call_that_keeps_its_precondition_is_quiet() {
    assert!(
        warnings_for(
            r#"
            const fn half(n: u64) -> u64 requires n % 2u64 == 0u64 { n / 2u64 }
            fn main() -> u64 { half(4u64) }
            "#,
        )
        .is_empty()
    );
}

#[test]
fn the_same_failure_in_a_const_initialiser_is_an_error() {
    // The forced / opportunistic split: a `const` has to have a value,
    // so there the identical failure stops the compile (E0017) rather
    // than warning (E0018).
    let err = test_program(
        r#"
        const fn half(n: u64) -> u64 requires n % 2u64 == 0u64 { n / 2u64 }
        const D: u64 = half(3u64)
        fn main() -> u64 { D }
        "#,
    )
    .expect_err("a const initialiser cannot be left for run time");
    assert!(err.contains("E0017"), "{err}");
}
