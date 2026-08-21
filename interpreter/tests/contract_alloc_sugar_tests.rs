// ALLOC-CONTRACT-SUGAR: `allocates(N)` / `retains(N)` /
// `allocations(N)` in an `ensures` clause.
//
// The sugar exists for the diagnostic more than for the length: a
// hand-written `__builtin_live_bytes() == old(__builtin_live_bytes())`
// can only report "clause #1 evaluated to false", leaving the reader
// to run `--profile=mem` and compare by hand. These tests pin the
// numbers that come out instead.
//
// Backend agreement — including the wording, which is duplicated into
// the no_std runtime — is pinned in `compiler/tests/consistency.rs`.

use crate::common::{assert_program_result_u64, test_program};

#[test]
fn retains_reports_what_was_leaked() {
    let err = test_program(
        r#"
        fn leaky(n: u64) -> u64
            ensures retains(0u64)
        {
            val p: ptr = __builtin_heap_alloc(128u64)
            n
        }

        fn main() -> u64 { leaky(1u64) }
        "#,
    )
    .expect_err("the budget must be enforced");
    assert!(err.contains("retained 128 bytes"), "{err}");
    assert!(err.contains("budget 0 bytes"), "{err}");
}

#[test]
fn allocates_reports_what_was_requested() {
    // `allocates` counts requests, so freeing does not help — this
    // body hands everything back and still blows the budget.
    let err = test_program(
        r#"
        fn over(n: u64) -> u64
            ensures allocates(64u64)
        {
            val p: ptr = __builtin_heap_alloc(384u64)
            __builtin_heap_free(p)
            n
        }

        fn main() -> u64 { over(1u64) }
        "#,
    )
    .expect_err("the budget must be enforced");
    assert!(err.contains("requested 384 bytes"), "{err}");
    assert!(err.contains("budget 64 bytes"), "{err}");
}

#[test]
fn allocations_reports_the_count() {
    let err = test_program(
        r#"
        fn many(n: u64) -> u64
            ensures allocations(1u64)
        {
            val a: ptr = __builtin_heap_alloc(8u64)
            val b: ptr = __builtin_heap_alloc(8u64)
            val c: ptr = __builtin_heap_alloc(8u64)
            n
        }

        fn main() -> u64 { many(1u64) }
        "#,
    )
    .expect_err("the budget must be enforced");
    assert!(err.contains("made 3 allocations"), "{err}");
    assert!(err.contains("budget 1"), "{err}");
}

#[test]
fn the_two_byte_axes_answer_different_questions() {
    // A function that allocates and frees satisfies `retains` and
    // violates `allocates` — which is exactly why the sugar cannot be
    // one word.
    assert_program_result_u64(
        r#"
        fn scratch(n: u64) -> u64
            ensures retains(0u64)
        {
            val p: ptr = __builtin_heap_alloc(128u64)
            __builtin_heap_free(p)
            n
        }

        fn main() -> u64 { scratch(42u64) }
        "#,
        42,
    );

    let err = test_program(
        r#"
        fn scratch(n: u64) -> u64
            ensures allocates(0u64)
        {
            val p: ptr = __builtin_heap_alloc(128u64)
            __builtin_heap_free(p)
            n
        }

        fn main() -> u64 { scratch(42u64) }
        "#,
    )
    .expect_err("`allocates(0)` means nothing was requested at all");
    assert!(err.contains("requested 128 bytes"), "{err}");
}

#[test]
fn a_function_that_frees_more_than_it_takes_does_not_underflow() {
    // The desugar is `counter() <= old(counter()) + N`, not
    // `counter() - old(counter()) <= N`. Written the second way — the
    // way one writes it by hand — this panics with a u64 underflow,
    // because `live_bytes` ends up below where it started.
    assert_program_result_u64(
        r#"
        fn freer(p: ptr) -> u64
            ensures retains(0u64)
        {
            __builtin_heap_free(p)
            7u64
        }

        fn main() -> u64 {
            val q: ptr = __builtin_heap_alloc(64u64)
            freer(q)
        }
        "#,
        7,
    );
}

#[test]
fn a_budget_inside_a_larger_predicate_is_not_a_budget_clause() {
    // `ensures retains(0u64) && result > 0u64` is a plain predicate
    // that happens to contain a budget. Reporting it as a budget
    // violation would name the wrong half, so it falls back to the
    // generic message.
    let err = test_program(
        r#"
        fn leaky(n: u64) -> u64
            ensures retains(0u64) && n > 0u64
        {
            val p: ptr = __builtin_heap_alloc(128u64)
            n
        }

        fn main() -> u64 { leaky(1u64) }
        "#,
    )
    .expect_err("the clause must still fail");
    assert!(err.contains("evaluated to false"), "{err}");
    assert!(!err.contains("retained 128 bytes"), "{err}");
}

#[test]
fn the_names_stay_available_outside_a_contract() {
    // Contextual, like `old`: a program with its own `allocates`
    // function keeps it.
    assert_program_result_u64(
        r#"
        fn allocates(n: u64) -> u64 { n * 2u64 }
        fn retains(n: u64) -> u64 { n + 1u64 }

        fn main() -> u64 {
            val a: u64 = allocates(20u64)
            val b: u64 = retains(1u64)
            a + b
        }
        "#,
        42,
    );
}

#[test]
fn a_budget_outside_an_ensures_clause_is_diagnosed() {
    let err = test_program(
        r#"
        fn f(n: u64) -> u64 {
            val x = retains(0u64)
            n
        }

        fn main() -> u64 { f(1u64) }
        "#,
    )
    .expect_err("the sugar only means something in a postcondition");
    assert!(err.contains("retains"), "{err}");
}
