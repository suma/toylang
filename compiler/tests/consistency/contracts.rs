//! `requires` / `ensures`, `old()`, allocation budgets, and the guard
//! elision those preconditions buy (CONTRACT-ELISION).

use interpreter::RunOptions;

use super::harness::*;

/// ALLOC-CONTRACT. `old(expr)` is the value `expr` had on entry, so
/// every backend has to take the snapshot at the same point — after
/// `requires`, before the body — and read it back in the
/// postcondition. A backend that evaluated it late would silently
/// compare a value against itself.
#[test]
fn old_snapshot_match() {
    let src = r#"
        struct Counter { n: u64 }

        impl Counter {
            fn bump(&mut self, by: u64) -> u64
                ensures result == old(self.n) + by
            {
                self.n = self.n + by
                self.n
            }
        }

        fn bump_free(n: u64) -> u64
            ensures result == old(n) + 1u64
        {
            n + 1u64
        }

        fn main() -> u64 {
            println(bump_free(41u64))
            var c = Counter { n: 40u64 }
            c.bump(2u64)
        }
    "#;
    assert_consistent(src, "old_snapshot");
}

/// ALLOC-CONTRACT. A function that claims to be memory-neutral and
/// then allocates must be stopped by every backend — the counters the
/// clause reads are request-based precisely so they agree across all
/// three.
#[test]
fn allocation_contract_violation_stops_on_every_backend() {
    let src = r#"
        fn leaky(n: u64) -> u64
            ensures __builtin_live_bytes() == old(__builtin_live_bytes())
        {
            val p: ptr = __builtin_heap_alloc(64u64)
            n + 1u64
        }

        fn main() -> u64 { leaky(1u64) }
    "#;
    let core = core_modules_dir();

    let mut interp_opts = RunOptions::default();
    interp_opts.core_modules_dir = Some(core.as_path());
    assert!(
        interpreter::run_source(src, "alloc_contract.t", &interp_opts).is_err(),
        "the interpreter should refuse the allocating body"
    );

    let compiled = try_compiler_exit_code(src, "alloc_contract", true)
        .expect("the program should still compile — the contract is a runtime check");
    assert_ne!(compiled, 0, "compiled binary should exit non-zero");
}

/// ALLOC-CONTRACT. The satisfied form of the same contract has to
/// agree too, or the check would be worthless: a backend whose
/// counters drifted would fail a correct program.
#[test]
fn allocation_contract_satisfied_match() {
    let src = r#"
        fn scratch(n: u64) -> u64
            ensures __builtin_live_bytes() == old(__builtin_live_bytes())
        {
            val p: ptr = __builtin_heap_alloc(128u64)
            __builtin_ptr_write(p, 0u64, n)
            val v: u64 = __builtin_ptr_read(p, 0u64)
            __builtin_heap_free(p)
            v
        }

        fn main() -> u64 { scratch(7u64) }
    "#;
    assert_consistent(src, "alloc_contract_ok");
}

/// CONTRACT-ELISION. A `requires` that proves the divisor non-zero is
/// checked once on entry, so the per-operation divide-by-zero guard
/// underneath it can never fire. Counted rather than pattern-matched:
/// with three divisions in the body, the unguarded version tests the
/// divisor three times and the contracted one tests it once — in the
/// precondition.
#[test]
fn a_precondition_replaces_the_divide_by_zero_guard() {
    let contracted = r#"
        fn hot(a: u64, b: u64) -> u64
            requires b != 0u64
        {
            (a / b) + (a / b) + (a / b)
        }

        fn main() -> u64 { hot(9u64, 3u64) }
    "#;
    let plain = r#"
        fn hot(a: u64, b: u64) -> u64
        {
            (a / b) + (a / b) + (a / b)
        }

        fn main() -> u64 { hot(9u64, 3u64) }
    "#;

    let with_contract = lowered_function(&lowered_ir(contracted), "hot");
    let without = lowered_function(&lowered_ir(plain), "hot");
    assert_eq!(
        with_contract.matches("= ne ").count(),
        1,
        "the only zero test should be the precondition:\n{with_contract}"
    );
    assert_eq!(
        without.matches("= ne ").count(),
        3,
        "each division needs its own guard without a contract:\n{without}"
    );
}

/// CONTRACT-ELISION. Same for the u64 subtraction guard, whose
/// condition (`a >= b`) is exactly what the precondition states.
#[test]
fn a_precondition_replaces_the_underflow_guard() {
    let contracted = r#"
        fn hot(a: u64, b: u64) -> u64
            requires a >= b
        {
            (a - b) + (a - b)
        }

        fn main() -> u64 { hot(9u64, 3u64) }
    "#;
    let plain = r#"
        fn hot(a: u64, b: u64) -> u64
        {
            (a - b) + (a - b)
        }

        fn main() -> u64 { hot(9u64, 3u64) }
    "#;
    let with_contract = lowered_function(&lowered_ir(contracted), "hot");
    let without = lowered_function(&lowered_ir(plain), "hot");
    assert_eq!(
        with_contract.matches("= ge ").count(),
        1,
        "the only ordering test should be the precondition:\n{with_contract}"
    );
    assert_eq!(
        without.matches("= ge ").count(),
        2,
        "each subtraction needs its own guard without a contract:\n{without}"
    );
}

/// CONTRACT-ELISION, the safety property. `--release` does not emit
/// the preconditions, so nothing verifies them — and an unverified
/// contract must not be allowed to remove a memory-safety check. The
/// guards come back.
#[test]
fn release_keeps_the_guards_because_nothing_checks_the_contract() {
    let src = r#"
        fn hot(a: u64, b: u64) -> u64
            requires b != 0u64
        {
            (a / b) + (a / b)
        }

        fn main() -> u64 { hot(9u64, 3u64) }
    "#;
    let checked = lowered_function(&lowered_ir_with(src, false), "hot");
    let released = lowered_function(&lowered_ir_with(src, true), "hot");
    assert_eq!(
        checked.matches("= ne ").count(),
        1,
        "checked build: precondition only:\n{checked}"
    );
    assert_eq!(
        released.matches("= ne ").count(),
        2,
        "release build: no precondition, so both guards stay:\n{released}"
    );
}

/// CONTRACT-ELISION, the other safety property. A fact is only about
/// the *parameter*; once a `val` in the body takes the name over, the
/// guard site is reading something the contract never described.
#[test]
fn shadowing_the_parameter_brings_the_guard_back() {
    let src = r#"
        fn hot(a: u64, b: u64) -> u64
            requires b != 0u64
        {
            val first: u64 = a / b
            val b: u64 = first - first
            first + (a / b)
        }

        fn main() -> u64 { hot(9u64, 3u64) }
    "#;
    let ir = lowered_function(&lowered_ir(src), "hot");
    // Precondition, plus a guard on the division that reads the
    // shadowing binding. The first division, which still reads the
    // parameter, is elided.
    assert_eq!(
        ir.matches("= ne ").count(),
        2,
        "the division after the shadow must keep its guard:\n{ir}"
    );
}

/// CONTRACT-ELISION. The elision must not change what a program
/// computes, and a call that breaks the contract must still stop —
/// through the precondition rather than through the guard that is no
/// longer there.
#[test]
fn contracted_arithmetic_matches_across_backends() {
    let src = r#"
        fn div(a: i64, b: i64) -> i64
            requires b != 0i64
        {
            a / b
        }

        fn take(a: u64, b: u64) -> u64
            requires a >= b
        {
            a - b
        }

        fn main() -> i64 {
            println(div(10i64, 3i64))
            println(take(10u64, 3u64))
            div(-9i64, 3i64)
        }
    "#;
    assert_consistent(src, "contract_elision_ok");
}

#[test]
fn a_broken_precondition_still_stops_every_backend() {
    let src = r#"
        fn div(a: i64, b: i64) -> i64
            requires b != 0i64
        {
            a / b
        }

        fn main() -> i64 {
            var zero: i64 = 0i64
            div(10i64, zero)
        }
    "#;
    let core = core_modules_dir();
    let mut interp_opts = RunOptions::default();
    interp_opts.core_modules_dir = Some(core.as_path());
    assert!(
        interpreter::run_source(src, "contract_elision_bad.t", &interp_opts).is_err(),
        "the precondition should refuse the call"
    );

    let compiled = try_compiler_exit_code(src, "contract_elision_bad", true)
        .expect("the program should still compile");
    assert_ne!(compiled, 0, "compiled binary should exit non-zero");
}

/// DBC-TRAIT-INHERIT. A contract declared on a trait method has to
/// reach every backend the same way — it is copied onto the impl's
/// `MethodFunction` before lowering, so the three engines see one
/// method carrying both sets of clauses.
#[test]
fn trait_contract_reaches_every_backend() {
    let src = r#"
        trait Shrink {
            fn shrink(self: Self, by: u64) -> u64
                requires by > 0u64
                ensures result < 1000u64
        }

        struct B { n: u64 }

        impl Shrink for B {
            fn shrink(self: Self, by: u64) -> u64
                requires by < 100u64
            {
                self.n - by
            }
        }

        fn main() -> u64 {
            val b = B { n: 50u64 }
            b.shrink(8u64)
        }
    "#;
    assert_consistent(src, "trait_contract_ok");
}

#[test]
fn a_trait_contract_violation_stops_every_backend() {
    let src = r#"
        trait Shrink {
            fn shrink(self: Self, by: u64) -> u64
                requires by > 0u64
        }

        struct B { n: u64 }

        impl Shrink for B {
            fn shrink(self: Self, by: u64) -> u64 {
                self.n - by
            }
        }

        fn main() -> u64 {
            val b = B { n: 10u64 }
            var zero: u64 = 0u64
            b.shrink(zero)
        }
    "#;
    let core = core_modules_dir();
    let mut interp_opts = RunOptions::default();
    interp_opts.core_modules_dir = Some(core.as_path());
    assert!(
        interpreter::run_source(src, "trait_contract.t", &interp_opts).is_err(),
        "the trait's precondition should refuse the call"
    );

    let compiled = try_compiler_exit_code(src, "trait_contract", true)
        .expect("the program should still compile");
    assert_ne!(compiled, 0, "compiled binary should exit non-zero");
}

/// ALLOC-CONTRACT-SUGAR. The point of the sugar is the numbers in the
/// diagnostic, so the numbers have to be the same wherever the
/// program runs. The sentence lives in two places — `compiler_ir` for
/// the interpreter and IR VM, and a hand-copied version in the no_std
/// `toylang_rt` for compiled binaries — and this is what catches them
/// drifting apart.
#[test]
fn an_allocation_budget_reports_the_same_numbers_on_every_backend() {
    for (clause, alloc, expected) in [
        ("retains(0u64)", "128u64", "retained 128 bytes, budget 0 bytes"),
        ("allocates(64u64)", "384u64", "requested 384 bytes, budget 64 bytes"),
    ] {
        let src = format!(
            r#"
        fn over(n: u64) -> u64
            ensures {clause}
        {{
            val p: ptr = __builtin_heap_alloc({alloc})
            n
        }}

        fn main() -> u64 {{ over(1u64) }}
    "#
        );

        let mut interp_opts = RunOptions::default();
        let core = core_modules_dir();
        interp_opts.core_modules_dir = Some(core.as_path());
        let interpreted = interpreter::run_source(&src, "budget.t", &interp_opts)
            .expect_err("the budget must be enforced");
        assert!(
            interpreted.to_string().contains(expected),
            "interpreter should report `{expected}`, got: {interpreted}"
        );

        let (code, stderr) =
            compiled_run_output(&src, "alloc_budget_msg").expect("the program should compile");
        assert_ne!(code, 0, "compiled binary should exit non-zero");
        assert!(
            stderr.contains(expected),
            "compiled binary should report `{expected}`, got: {stderr}"
        );
    }
}

/// ALLOC-CONTRACT-SUGAR. A satisfied budget must not disturb anything,
/// including the case the desugar exists to protect: a function whose
/// `live_bytes` ends up *below* where it started.
#[test]
fn satisfied_allocation_budgets_match() {
    let src = r#"
        fn scratch(n: u64) -> u64
            ensures allocates(256u64)
            ensures retains(0u64)
            ensures allocations(1u64)
        {
            val p: ptr = __builtin_heap_alloc(128u64)
            __builtin_ptr_write(p, 0u64, n)
            val v: u64 = __builtin_ptr_read(p, 0u64)
            __builtin_heap_free(p)
            v
        }

        fn freer(p: ptr) -> u64
            ensures retains(0u64)
        {
            __builtin_heap_free(p)
            7u64
        }

        fn main() -> u64 {
            println(scratch(35u64))
            val q: ptr = __builtin_heap_alloc(64u64)
            freer(q)
        }
    "#;
    assert_consistent(src, "alloc_budget_ok");
}

/// ALLOC-CONTRACT-SUGAR. The budget clause lowers to its own
/// terminator rather than the shared `panic #N`, which is what lets
/// the compiled binary report numbers at all.
#[test]
fn a_budget_clause_lowers_to_its_own_terminator() {
    let src = r#"
        fn leaky(n: u64) -> u64
            ensures retains(0u64)
        {
            val p: ptr = __builtin_heap_alloc(128u64)
            n
        }

        fn main() -> u64 { leaky(1u64) }
    "#;
    let ir = lowered_function(&lowered_ir(src), "leaky");
    assert!(
        ir.contains("panic_alloc_budget live_bytes"),
        "expected the budget terminator in:\n{ir}"
    );
}

/// MEM-COUNTER-INTERP-DRIFT. The counters report what the *program*
/// asked the allocator for, not what the language runtime spends
/// underneath to hold a string — and every engine has to agree on
/// that, because contracts read these numbers.
///
/// This used to differ: `println("n = {n}")` cost 24 bytes on the IR
/// VM (whose byte-uniform heap materialises the concatenation) and 0
/// on AOT (whose `toy_str_alloc` calls `malloc` directly), so the
/// same `ensures allocates(0u64)` passed on one engine and failed on
/// the other.
#[test]
fn string_interpolation_costs_the_same_everywhere() {
    let src = r#"
        fn probe(n: u64) -> u64 {
            val before: u64 = __builtin_cumulative_bytes()
            println("n = {n}")
            val after: u64 = __builtin_cumulative_bytes()
            after - before
        }

        fn main() -> u64 { probe(1u64) }
    "#;
    assert_consistent(src, "interp_counter");
}

/// The other half of the same rule: an allocation the program makes
/// explicitly is still counted, on every engine. Without this the fix
/// above could have been "stop counting anything".
#[test]
fn an_explicit_allocation_is_still_counted_everywhere() {
    let src = r#"
        fn probe(n: u64) -> u64 {
            val before: u64 = __builtin_cumulative_bytes()
            val p: ptr = __builtin_heap_alloc(32u64)
            __builtin_heap_free(p)
            val after: u64 = __builtin_cumulative_bytes()
            after - before
        }

        fn main() -> u64 { probe(1u64) }
    "#;
    assert_consistent(src, "explicit_counter");
}

/// A budget clause over a function that interpolates must therefore
/// hold everywhere, rather than depending on which engine ran it.
#[test]
fn an_allocation_budget_survives_interpolation() {
    let src = r#"
        fn report(n: u64) -> u64
            ensures allocates(0u64)
        {
            println("n = {n}")
            n
        }

        fn main() -> u64 { report(7u64) }
    "#;
    assert_consistent(src, "budget_with_interp");
}

/// CONTRACT-ELISION. `requires i < 4u64` on a four-element array
/// states exactly what the bounds guard would test, and it is checked
/// once on entry — so the per-access guard goes. Counted rather than
/// matched: two accesses need two guards without the contract and
/// none with it, leaving only the precondition's own comparison.
#[test]
fn a_precondition_replaces_the_bounds_guard() {
    let contracted = r#"
        fn hot(i: u64) -> u64
            requires i < 4u64
        {
            val arr = [1u64, 2u64, 3u64, 4u64]
            arr[i] + arr[i]
        }

        fn main() -> u64 { hot(1u64) }
    "#;
    let plain = r#"
        fn hot(i: u64) -> u64
        {
            val arr = [1u64, 2u64, 3u64, 4u64]
            arr[i] + arr[i]
        }

        fn main() -> u64 { hot(1u64) }
    "#;
    let with_contract = lowered_function(&lowered_ir(contracted), "hot");
    let without = lowered_function(&lowered_ir(plain), "hot");
    assert_eq!(
        with_contract.matches("= lt ").count(),
        1,
        "the only bound test should be the precondition:\n{with_contract}"
    );
    assert_eq!(
        without.matches("= lt ").count(),
        2,
        "each access needs its own guard without a contract:\n{without}"
    );
}

/// CONTRACT-ELISION. A bound that does not cover the array keeps the
/// guard — `i < 8u64` says nothing useful about a four-element array.
#[test]
fn a_bound_wider_than_the_array_keeps_the_guard() {
    let src = r#"
        fn hot(i: u64) -> u64
            requires i < 8u64
        {
            val arr = [1u64, 2u64, 3u64, 4u64]
            arr[i]
        }

        fn main() -> u64 { hot(1u64) }
    "#;
    let ir = lowered_function(&lowered_ir(src), "hot");
    assert_eq!(
        ir.matches("= lt ").count(),
        2,
        "precondition plus the guard it does not justify removing:\n{ir}"
    );
}

/// CONTRACT-ELISION, the safety property for the bounds case: a call
/// that breaks the precondition still stops, through the precondition
/// rather than through the guard that is no longer there.
#[test]
fn a_broken_index_precondition_still_stops_every_backend() {
    let src = r#"
        fn contracted(i: u64) -> u64
            requires i < 4u64
        {
            val arr = [1u64, 2u64, 3u64, 4u64]
            arr[i]
        }

        fn main() -> u64 {
            var bad: u64 = 9u64
            contracted(bad)
        }
    "#;
    let core = core_modules_dir();
    let mut interp_opts = RunOptions::default();
    interp_opts.core_modules_dir = Some(core.as_path());
    assert!(
        interpreter::run_source(src, "index_contract.t", &interp_opts).is_err(),
        "the precondition should refuse the call"
    );

    let compiled = try_compiler_exit_code(src, "index_contract", true)
        .expect("the program should still compile");
    assert_ne!(compiled, 0, "compiled binary should exit non-zero");
}

/// CONTRACT-ELISION. The signed bounds guard tests two things — that
/// the index is not negative (adjusting it if it is, since `arr[-1]`
/// is the last element) and that it stays below the length. A
/// precondition that states both halves (`i >= 0` and `i < N`)
/// replaces the whole guard, adjustment included.
#[test]
fn a_signed_index_precondition_replaces_the_bounds_guard() {
    let contracted = r#"
        fn hot(i: i64) -> i64
            requires i >= 0i64
            requires i < 4i64
        {
            val arr = [1i64, 2i64, 3i64, 4i64]
            arr[i] + arr[i]
        }

        fn main() -> i64 { hot(1i64) }
    "#;
    let plain = r#"
        fn hot(i: i64) -> i64
        {
            val arr = [1i64, 2i64, 3i64, 4i64]
            arr[i] + arr[i]
        }

        fn main() -> i64 { hot(1i64) }
    "#;
    let with_contract = lowered_function(&lowered_ir(contracted), "hot");
    let without = lowered_function(&lowered_ir(plain), "hot");
    assert_eq!(
        with_contract.matches("= lt ").count(),
        1,
        "only the upper-bound precondition should test ordering:\n{with_contract}"
    );
    assert_eq!(
        without.matches("= lt ").count(),
        4,
        "signed guard per access is two comparisons; two accesses need four:\n{without}"
    );
}

/// CONTRACT-ELISION, the negative half of the signed guard. `i < N`
/// alone does not rule out a negative index, and the guard's
/// negative-adjustment path is part of the semantics being elided —
/// so without a non-negativity fact the guard must stay.
#[test]
fn a_signed_bound_without_nonneg_keeps_the_guard() {
    let src = r#"
        fn hot(i: i64) -> i64
            requires i < 4i64
        {
            val arr = [1i64, 2i64, 3i64, 4i64]
            arr[i]
        }

        fn main() -> i64 { hot(1i64) }
    "#;
    let ir = lowered_function(&lowered_ir(src), "hot");
    assert_eq!(
        ir.matches("= lt ").count(),
        3,
        "precondition plus the signed guard's two comparisons:\n{ir}"
    );
}

/// CONTRACT-ELISION, the safety property for the signed bounds case:
/// a negative index that the precondition rejects must still stop
/// every backend.
#[test]
fn a_broken_signed_index_precondition_still_stops_every_backend() {
    let src = r#"
        fn contracted(i: i64) -> i64
            requires i >= 0i64
            requires i < 4i64
        {
            val arr = [1i64, 2i64, 3i64, 4i64]
            arr[i]
        }

        fn main() -> i64 { contracted(-1i64) }
    "#;
    let core = core_modules_dir();
    let mut interp_opts = RunOptions::default();
    interp_opts.core_modules_dir = Some(core.as_path());
    assert!(
        interpreter::run_source(src, "signed_index_contract.t", &interp_opts).is_err(),
        "the precondition should refuse the call"
    );

    let compiled = try_compiler_exit_code(src, "signed_index_contract", true)
        .expect("the program should still compile");
    assert_ne!(compiled, 0, "compiled binary should exit non-zero");
}

/// CONTRACT-ELISION. `requires b != -1i64` kills the `rhs == -1` half
/// of the signed `MIN / -1` guard, whose two comparisons per division
/// (`rhs == -1`, `lhs == MIN`) can then never fire. The divide-by-zero
/// guard is untouched: `b != -1` says nothing about `b == 0`.
#[test]
fn a_precondition_replaces_the_div_overflow_guard() {
    let contracted = r#"
        fn hot(a: i64, b: i64) -> i64
            requires b != -1i64
        {
            (a / b) + (a / b)
        }

        fn main() -> i64 { hot(10i64, 3i64) }
    "#;
    let plain = r#"
        fn hot(a: i64, b: i64) -> i64
        {
            (a / b) + (a / b)
        }

        fn main() -> i64 { hot(10i64, 3i64) }
    "#;
    let with_contract = lowered_function(&lowered_ir(contracted), "hot");
    let without = lowered_function(&lowered_ir(plain), "hot");
    assert_eq!(
        with_contract.matches("= eq ").count(),
        0,
        "no overflow guard remains:\n{with_contract}"
    );
    assert_eq!(
        without.matches("= eq ").count(),
        4,
        "two comparisons per division, two divisions:\n{without}"
    );
    assert_eq!(
        with_contract.matches("= ne ").count(),
        3,
        "precondition plus one divide-by-zero guard per division:\n{with_contract}"
    );
}

/// CONTRACT-ELISION, the safety property for `MIN / -1`: the call the
/// precondition rejects must still stop every backend, through the
/// precondition rather than through the guard that is no longer there.
#[test]
fn a_broken_minus_one_precondition_still_stops_every_backend() {
    let src = r#"
        fn contracted(a: i64, b: i64) -> i64
            requires b != -1i64
        {
            a / b
        }

        fn main() -> i64 { contracted(-9223372036854775808i64, -1i64) }
    "#;
    let core = core_modules_dir();
    let mut interp_opts = RunOptions::default();
    interp_opts.core_modules_dir = Some(core.as_path());
    assert!(
        interpreter::run_source(src, "overflow_contract.t", &interp_opts).is_err(),
        "the precondition should refuse the call"
    );

    let compiled = try_compiler_exit_code(src, "overflow_contract", true)
        .expect("the program should still compile");
    assert_ne!(compiled, 0, "compiled binary should exit non-zero");
}

/// CONTRACT-ELISION, transitivity: `a >= b` and `b >= c` chain into
/// `a >= c`, which the underflow guard of `a - c` would otherwise test
/// itself. Without the middle fact the guard stays.
#[test]
fn a_transitive_chain_replaces_the_underflow_guard() {
    let chained = r#"
        fn hot(a: u64, b: u64, c: u64) -> u64
            requires a >= b
            requires b >= c
        {
            (a - c) + (a - c)
        }

        fn main() -> u64 { hot(9u64, 5u64, 2u64) }
    "#;
    let partial = r#"
        fn hot(a: u64, b: u64, c: u64) -> u64
            requires a >= b
        {
            (a - c) + (a - c)
        }

        fn main() -> u64 { hot(9u64, 5u64, 2u64) }
    "#;
    let with_chain = lowered_function(&lowered_ir(chained), "hot");
    let without = lowered_function(&lowered_ir(partial), "hot");
    assert_eq!(
        with_chain.matches("= ge ").count(),
        2,
        "the two preconditions only:\n{with_chain}"
    );
    assert_eq!(
        without.matches("= ge ").count(),
        3,
        "one precondition plus two guards `a - c` cannot justify:\n{without}"
    );
}

/// CONTRACT-ELISION, transitivity of the upper bound: `j <= i` and
/// `i < N` chain into `j < N`, which is exactly what the bounds guard
/// on `arr[j]` would test.
#[test]
fn a_below_fact_propagates_through_a_chain() {
    let chained = r#"
        fn hot(i: u64, j: u64) -> u64
            requires j <= i
            requires i < 4u64
        {
            val arr = [1u64, 2u64, 3u64, 4u64]
            arr[j]
        }

        fn main() -> u64 { hot(3u64, 2u64) }
    "#;
    let direct = r#"
        fn hot(i: u64, j: u64) -> u64
            requires i < 4u64
        {
            val arr = [1u64, 2u64, 3u64, 4u64]
            arr[j]
        }

        fn main() -> u64 { hot(3u64, 2u64) }
    "#;
    let with_chain = lowered_function(&lowered_ir(chained), "hot");
    let without = lowered_function(&lowered_ir(direct), "hot");
    assert_eq!(
        with_chain.matches("= lt ").count(),
        1,
        "the upper-bound precondition only:\n{with_chain}"
    );
    assert_eq!(
        without.matches("= lt ").count(),
        2,
        "precondition plus the guard `j` alone has no fact for:\n{without}"
    );
}

/// CONTRACT-ELISION, non-negativity through a chain: `j >= 0` and
/// `j <= i` chain into `i >= 0`, the missing half of the signed bounds
/// elision on `arr[i]`.
#[test]
fn a_signed_index_fact_propagates_through_a_chain() {
    let chained = r#"
        fn hot(i: i64, j: i64) -> i64
            requires j >= 0i64
            requires j <= i
            requires i < 4i64
        {
            val arr = [1i64, 2i64, 3i64, 4i64]
            arr[i]
        }

        fn main() -> i64 { hot(3i64, 2i64) }
    "#;
    let direct = r#"
        fn hot(i: i64, j: i64) -> i64
            requires j >= 0i64
            requires i < 4i64
        {
            val arr = [1i64, 2i64, 3i64, 4i64]
            arr[i]
        }

        fn main() -> i64 { hot(3i64, 2i64) }
    "#;
    let with_chain = lowered_function(&lowered_ir(chained), "hot");
    let without = lowered_function(&lowered_ir(direct), "hot");
    assert_eq!(
        with_chain.matches("= lt ").count(),
        1,
        "the upper-bound precondition only:\n{with_chain}"
    );
    assert_eq!(
        without.matches("= lt ").count(),
        3,
        "precondition plus the signed guard's two comparisons:\n{without}"
    );
}

/// CONTRACT-ELISION, the safety property for the transitive chains: a
/// call that breaks the chain still stops every backend.
#[test]
fn a_broken_chain_precondition_still_stops_every_backend() {
    let src = r#"
        fn contracted(i: u64, j: u64) -> u64
            requires j <= i
            requires i < 4u64
        {
            val arr = [1u64, 2u64, 3u64, 4u64]
            arr[j]
        }

        fn main() -> u64 { contracted(5u64, 5u64) }
    "#;
    let core = core_modules_dir();
    let mut interp_opts = RunOptions::default();
    interp_opts.core_modules_dir = Some(core.as_path());
    assert!(
        interpreter::run_source(src, "chain_contract.t", &interp_opts).is_err(),
        "the precondition should refuse the call"
    );

    let compiled = try_compiler_exit_code(src, "chain_contract", true)
        .expect("the program should still compile");
    assert_ne!(compiled, 0, "compiled binary should exit non-zero");
}

/// CONTRACT-ELISION. The three new shapes — signed index, `MIN / -1`,
/// transitive chains — must not change what a program computes; every
/// backend has to agree on the elided versions.
#[test]
fn contract_elision_signed_shapes_match_across_backends() {
    let src = r#"
        fn signed(i: i64) -> i64
            requires i >= 0i64
            requires i < 4i64
        {
            val arr = [1i64, 2i64, 3i64, 4i64]
            arr[i]
        }

        fn overflow(b: i64) -> i64
            requires b != -1i64
        {
            (100i64 / b) + (100i64 / b)
        }

        fn chain(a: u64, b: u64, c: u64) -> u64
            requires a >= b
            requires b >= c
        {
            (a - c) + (a - c)
        }

        fn bound(i: u64, j: u64) -> u64
            requires j <= i
            requires i < 4u64
        {
            val arr = [1u64, 2u64, 3u64, 4u64]
            arr[j]
        }

        fn main() -> u64 {
            println(signed(1i64))
            println(overflow(2i64))
            println(chain(9u64, 5u64, 2u64))
            println(bound(3u64, 2u64))
            0u64
        }
    "#;
    assert_consistent(src, "contract_elision_signed_shapes");
}

// ---------------------------------------------------------------------
// CONTRACT-ELISION, control-flow half: the facts a branch condition or
// a loop range states about the code they guard.
//
// A `requires` is not the only place a program says what it knows —
// `if b != 0u64 { a / b }` says the same thing, and says it where most
// code actually says it. These facts differ from a precondition's in
// one way that matters: they hold under `--release` too, because the
// branch is evaluated either way.
// ---------------------------------------------------------------------

/// The guard the division would emit tests exactly what the `if`
/// already tested.
#[test]
fn a_branch_that_rules_out_zero_elides_the_division_guard() {
    let src = r#"
        fn hot(a: u64, b: u64) -> u64 {
            if b != 0u64 { a / b } else { 0u64 }
        }

        fn main() -> u64 { hot(9u64, 3u64) }
    "#;
    let ir = lowered_function(&lowered_ir(src), "hot");
    // One `ne`: the condition itself. A second would be the guard.
    assert_eq!(
        ir.matches("= ne ").count(),
        1,
        "the division's guard tests what the branch tested:\n{ir}"
    );
}

/// The `else` branch knows the condition failed, which is where the
/// same program is just as often written.
#[test]
fn the_else_branch_reads_the_condition_the_other_way_round() {
    let src = r#"
        fn hot(a: u64, b: u64) -> u64 {
            if b == 0u64 { 0u64 } else { a / b }
        }

        fn main() -> u64 { hot(9u64, 3u64) }
    "#;
    let ir = lowered_function(&lowered_ir(src), "hot");
    assert_eq!(
        ir.matches("= ne ").count(),
        0,
        "reaching the else means `b` is non-zero:\n{ir}"
    );
}

/// `if a < b { 0u64 } else { a - b }` — the underflow guard is the
/// negation of the test that got here.
#[test]
fn the_else_branch_rules_out_underflow() {
    let src = r#"
        fn take(a: u64, b: u64) -> u64 {
            if a < b { 0u64 } else { a - b }
        }

        fn main() -> u64 { take(9u64, 3u64) }
    "#;
    let ir = lowered_function(&lowered_ir(src), "take");
    assert_eq!(
        ir.matches("= ge ").count(),
        0,
        "reaching the else means `a >= b`:\n{ir}"
    );
}

/// The safety property. A parameter cannot change, but a local can —
/// and a fact about a binding the branch reassigns would be true on
/// entry and stale at the guard site.
#[test]
fn assigning_the_tested_binding_brings_the_guard_back() {
    let src = r#"
        fn hot(a: u64, b: u64) -> u64 {
            var d: u64 = b
            if d != 0u64 {
                d = d - d
                a / d
            } else { 0u64 }
        }

        fn main() -> u64 { hot(9u64, 3u64) }
    "#;
    let ir = lowered_function(&lowered_ir(src), "hot");
    assert_eq!(
        ir.matches("= ne ").count(),
        2,
        "the branch reassigns `d`, so the guard stays:\n{ir}"
    );
}

/// `for i in 0u64..4u64` over a `[T; 4]` states exactly what the index
/// guard would test.
#[test]
fn a_literal_bounded_loop_elides_the_index_guard() {
    let src = r#"
        fn sum() -> u64 {
            val arr: [u64; 4] = [1u64, 2u64, 3u64, 4u64]
            var total: u64 = 0u64
            for i in 0u64..4u64 {
                total = total + arr[i]
            }
            total
        }

        fn main() -> u64 { sum() }
    "#;
    let ir = lowered_function(&lowered_ir(src), "sum");
    // One `lt`: the loop header. A second would be the bounds check.
    assert_eq!(
        ir.matches("= lt ").count(),
        1,
        "the loop bound is the array bound:\n{ir}"
    );
}

/// The same shape with a range that runs past the array keeps its
/// guard — and the program still stops, which is the point.
#[test]
fn a_loop_that_can_overrun_keeps_the_index_guard() {
    let src = r#"
        fn sum() -> u64 {
            val arr: [u64; 4] = [1u64, 2u64, 3u64, 4u64]
            var total: u64 = 0u64
            for i in 0u64..8u64 {
                total = total + arr[i]
            }
            total
        }

        fn main() -> u64 { sum() }
    "#;
    let ir = lowered_function(&lowered_ir(src), "sum");
    assert_eq!(
        ir.matches("= lt ").count(),
        2,
        "the range reaches past the array, so the guard stays:\n{ir}"
    );
}

/// A signed loop starting below zero cannot be proved non-negative, so
/// the negative-adjustment path — which the elision would remove along
/// with the guard — has to stay.
#[test]
fn a_signed_loop_from_a_negative_start_keeps_the_guard() {
    let src = r#"
        fn sum() -> i64 {
            val arr: [i64; 4] = [1i64, 2i64, 3i64, 4i64]
            var total: i64 = 0i64
            for i in -2i64..2i64 {
                total = total + arr[i]
            }
            total
        }

        fn main() -> i64 { sum() }
    "#;
    let ir = lowered_function(&lowered_ir(src), "sum");
    assert!(
        ir.contains("= lt ") && ir.matches("= lt ").count() > 1,
        "a negative index still has to be adjusted and checked:\n{ir}"
    );
}

/// Control-flow facts hold with the contracts switched off: the branch
/// runs either way. This is what separates them from `requires`, whose
/// facts vanish under `--release` along with the check that earned
/// them.
#[test]
fn a_branch_fact_survives_release() {
    let src = r#"
        fn hot(a: u64, b: u64) -> u64 {
            if b != 0u64 { a / b } else { 0u64 }
        }

        fn main() -> u64 { hot(9u64, 3u64) }
    "#;
    let released = lowered_function(&lowered_ir_with(src, true), "hot");
    assert_eq!(
        released.matches("= ne ").count(),
        1,
        "release build: the branch is still there, so the fact is too:\n{released}"
    );
}

/// The elision must not change what a program computes, and the traps
/// it does not remove must still fire on every backend.
#[test]
fn control_flow_elision_matches_across_backends() {
    let src = r#"
        fn guarded_div(a: u64, b: u64) -> u64 {
            if b != 0u64 { a / b } else { 0u64 }
        }

        fn guarded_take(a: u64, b: u64) -> u64 {
            if a < b { 0u64 } else { a - b }
        }

        fn indexed() -> u64 {
            val arr: [u64; 4] = [10u64, 20u64, 30u64, 40u64]
            var total: u64 = 0u64
            for i in 0u64..4u64 {
                total = total + arr[i]
            }
            total
        }

        fn main() -> u64 {
            println(guarded_div(9u64, 0u64))
            println(guarded_div(9u64, 3u64))
            println(guarded_take(3u64, 9u64))
            println(guarded_take(9u64, 3u64))
            indexed()
        }
    "#;
    assert_consistent(src, "control_flow_elision");
}

/// The trap that is still needed still stops every backend: the loop
/// runs past the array, so the guard the elision left in place is the
/// one that fires.
#[test]
fn an_overrunning_loop_still_stops_every_backend() {
    let src = r#"
        fn main() -> u64 {
            val arr: [u64; 4] = [1u64, 2u64, 3u64, 4u64]
            var total: u64 = 0u64
            for i in 0u64..8u64 {
                total = total + arr[i]
            }
            total
        }
    "#;
    let core = core_modules_dir();
    let mut interp_opts = RunOptions::default();
    interp_opts.core_modules_dir = Some(core.as_path());
    assert!(
        interpreter::run_source(src, "loop_overrun.t", &interp_opts).is_err(),
        "the index guard should still fire"
    );

    let compiled = try_compiler_exit_code(src, "loop_overrun", true)
        .expect("the program should still compile");
    assert_ne!(compiled, 0, "compiled binary should exit non-zero");
}
