// ALLOC-CONTRACT: `old(...)` in `ensures` clauses — the value an
// expression had on entry to the function.
//
// Interpreter-side behaviour; the three-backend agreement is pinned in
// `compiler/tests/consistency.rs` and by
// `interpreter/example/alloc_contract.t` through example_consistency.
// The `INTERPRETER_CONTRACTS` interaction (snapshots are not taken when
// postconditions are off) lives in `contract_mode_tests.rs`, which
// spawns the binary the way a user runs it.

use crate::common::{assert_program_result_u64, test_program};

#[test]
fn old_captures_the_entry_value_of_a_parameter() {
    assert_program_result_u64(
        r#"
        fn bump(n: u64) -> u64
            ensures result == old(n) + 1u64
        {
            n + 1u64
        }

        fn main() -> u64 { bump(41u64) }
        "#,
        42,
    );
}

#[test]
fn old_reads_the_receiver_before_the_body_mutates_it() {
    // The point of `old` on a `&mut self` method: without it there is
    // no way to state "this returns what the field became", because by
    // the time `ensures` runs the field already holds the new value.
    assert_program_result_u64(
        r#"
        struct Counter { n: u64 }

        impl Counter {
            fn bump(&mut self, by: u64) -> u64
                ensures result == old(self.n) + by
            {
                self.n = self.n + by
                self.n
            }
        }

        fn main() -> u64 {
            var c = Counter { n: 40u64 }
            c.bump(2u64)
        }
        "#,
        42,
    );
}

#[test]
fn a_wrong_old_postcondition_is_caught() {
    // Guards against the snapshot silently reading the *post* state,
    // which would make the clause above pass for the wrong reason: if
    // `old(n)` were the current `n`, this clause would hold.
    let err = test_program(
        r#"
        fn bump(n: u64) -> u64
            ensures result == old(n)
        {
            n + 1u64
        }

        fn main() -> u64 { bump(1u64) }
        "#,
    )
    .expect_err("the postcondition must fail");
    assert!(err.contains("Contract violation"), "{err}");
    assert!(err.contains("ensures"), "{err}");
}

#[test]
fn an_allocation_contract_passes_when_nothing_is_allocated() {
    assert_program_result_u64(
        r#"
        fn tidy(n: u64) -> u64
            ensures __builtin_live_bytes() == old(__builtin_live_bytes())
        {
            n + 1u64
        }

        fn main() -> u64 { tidy(41u64) }
        "#,
        42,
    );
}

#[test]
fn an_allocation_contract_catches_a_leak() {
    // The whole point of the feature: a function that claims to be
    // memory-neutral and is not stops, naming the clause.
    let err = test_program(
        r#"
        fn leaky(n: u64) -> u64
            ensures __builtin_live_bytes() == old(__builtin_live_bytes())
        {
            val p: ptr = __builtin_heap_alloc(64u64)
            n + 1u64
        }

        fn main() -> u64 { leaky(1u64) }
        "#,
    )
    .expect_err("the allocation contract must fail");
    assert!(err.contains("Contract violation"), "{err}");
}

#[test]
fn an_allocation_budget_bounds_what_a_function_may_request() {
    // The other shape: not "allocates nothing" but "allocates at most
    // this much", which is what a parser or a builder wants to promise.
    assert_program_result_u64(
        r#"
        fn bounded(n: u64) -> u64
            ensures __builtin_cumulative_bytes() - old(__builtin_cumulative_bytes()) <= 128u64
        {
            val p: ptr = __builtin_heap_alloc(64u64)
            n
        }

        fn main() -> u64 { bounded(7u64) }
        "#,
        7,
    );
}

#[test]
fn an_allocation_budget_catches_going_over() {
    let err = test_program(
        r#"
        fn over(n: u64) -> u64
            ensures __builtin_cumulative_bytes() - old(__builtin_cumulative_bytes()) <= 32u64
        {
            val p: ptr = __builtin_heap_alloc(64u64)
            n
        }

        fn main() -> u64 { over(7u64) }
        "#,
    )
    .expect_err("the budget must be enforced");
    assert!(err.contains("Contract violation"), "{err}");
}

#[test]
fn old_outside_an_ensures_clause_is_diagnosed() {
    for source in [
        // in a `requires`
        r#"
        fn f(n: u64) -> u64
            requires old(n) > 0u64
        { n }

        fn main() -> u64 { f(1u64) }
        "#,
        // in a body
        r#"
        fn f(n: u64) -> u64 {
            val x = old(n)
            x
        }

        fn main() -> u64 { f(1u64) }
        "#,
        // nested: the inner `old` snapshots the same instant, so it is
        // refused rather than accepted as a no-op
        r#"
        fn f(n: u64) -> u64
            ensures result == old(old(n))
        { n }

        fn main() -> u64 { f(1u64) }
        "#,
    ] {
        let err = test_program(source).expect_err("misplaced `old` must be refused");
        assert!(
            err.contains("`old(...)` is only meaningful in an `ensures` clause"),
            "{err}"
        );
    }
}

#[test]
fn old_is_still_available_as_an_ordinary_name() {
    // `old` is contextual: only a call spelled `old` inside an
    // `ensures` clause is the snapshot form, so a program that already
    // has a variable or a function by that name keeps working.
    assert_program_result_u64(
        r#"
        fn old(n: u64) -> u64 { n * 2u64 }

        fn main() -> u64 {
            val old: u64 = 21u64
            old + old(0u64)
        }
        "#,
        21,
    );
}
