//! COMPILE-TIME-EVAL C0: the compiler's answer and the program's
//! answer are the same answer.
//!
//! This language already runs one set of semantics on four engines,
//! and compile-time evaluation is a fifth place the same expression
//! gets a value. The lane below computes each expression twice from
//! one body — once folded before lowering (a `const fn` called from a
//! `const` initialiser) and once left to run time (the same body,
//! unannotated, called from `main`) — and requires every backend to
//! agree with every other *and* the two ways to agree with each
//! other.
//!
//! The lane cannot pass vacuously. `compiler_lower::consts` still
//! cannot evaluate a call, so if the driver's fold did not happen the
//! folded half would not compile at all on the JIT and AOT paths, and
//! `assert_consistent` would fail rather than quietly compare two
//! run-time answers.
//!
//! The cases are chosen where a hand-written compile-time evaluator
//! would most plausibly drift from the run time it is supposed to
//! predict: wrapping `+` / `*`, truncated signed division and
//! remainder, narrow-width wrapping, and float rounding.

use super::harness::*;

/// `body` computed both ways, with `params` bound to `args`.
fn assert_fold_matches_runtime(params: &str, args: &str, body: &str, stem: &str) {
    let folded = format!(
        "const fn compute({params}) -> u64 {{ {body} }}\n\
         const R: u64 = compute({args})\n\
         fn main() -> u64 {{ R }}\n"
    );
    let live = format!(
        "fn compute({params}) -> u64 {{ {body} }}\n\
         fn main() -> u64 {{ compute({args}) }}\n"
    );
    assert_consistent(&folded, &format!("{stem}_folded"));
    assert_consistent(&live, &format!("{stem}_live"));
    assert_eq!(
        interpreter_value(&folded),
        interpreter_value(&live),
        "compile-time and run-time answers differ for `{body}`"
    );
}

#[test]
fn plain_arithmetic_folds_to_the_same_value() {
    assert_fold_matches_runtime("n: u64", "20u64", "n * 2u64 + 1u64", "ctfe_arith");
}

#[test]
fn unsigned_addition_wraps_the_same_way() {
    // RUNTIME-TRAP: `+` and `*` wrap rather than trap, in every build
    // profile. A fold that used the host's checked arithmetic would
    // turn this into a compiler crash, and one that used saturating
    // arithmetic would return a different number.
    assert_fold_matches_runtime(
        "n: u64, m: u64",
        "18446744073709551615u64, 3u64",
        "n + m",
        "ctfe_wrap_add",
    );
}

#[test]
fn unsigned_multiplication_wraps_the_same_way() {
    assert_fold_matches_runtime(
        "n: u64, m: u64",
        "18446744073709551615u64, 4u64",
        "n * m",
        "ctfe_wrap_mul",
    );
}

#[test]
fn signed_division_truncates_the_same_way() {
    // Truncated, not floored: `-7 / 3 == -2` and `-7 % 3 == -1`.
    // Offset by 100 so the result is a valid exit code either way.
    assert_fold_matches_runtime(
        "a: i64, b: i64",
        "0i64 - 7i64, 3i64",
        "(a / b + 100i64) as u64",
        "ctfe_sdiv",
    );
}

#[test]
fn signed_remainder_matches() {
    assert_fold_matches_runtime(
        "a: i64, b: i64",
        "0i64 - 7i64, 3i64",
        "(a % b + 100i64) as u64",
        "ctfe_srem",
    );
}

#[test]
fn narrow_widths_wrap_at_their_own_width() {
    assert_fold_matches_runtime("a: u8, b: u8", "200u8, 3u8", "(a * b) as u64", "ctfe_narrow");
}

#[test]
fn float_arithmetic_matches() {
    assert_fold_matches_runtime(
        "x: f64, y: f64",
        "3.5f64, 4.0f64",
        "(x * y) as u64",
        "ctfe_float",
    );
}

#[test]
fn recursion_folds() {
    assert_fold_matches_runtime(
        "n: u64",
        "10u64",
        "if n <= 1u64 { n } else { compute(n - 1u64) + compute(n - 2u64) }",
        "ctfe_recursion",
    );
}

#[test]
fn a_loop_folds() {
    assert_fold_matches_runtime(
        "n: u64",
        "9u64",
        "var total: u64 = 0u64  var i: u64 = 1u64  while i <= n { total = total + i  i = i + 1u64 }  total",
        "ctfe_loop",
    );
}

#[test]
fn a_bool_result_folds() {
    assert_fold_matches_runtime(
        "n: u64",
        "5u64",
        "if n > 3u64 { 1u64 } else { 0u64 }",
        "ctfe_bool",
    );
}

#[test]
fn a_fold_reads_an_earlier_const() {
    // The chain a single-pass fold would get wrong: `B` is only
    // computable once `A` is, and the initialisers are evaluated in
    // declaration order.
    let src = "const fn double(n: u64) -> u64 { n * 2u64 }\n\
               const A: u64 = double(3u64)\n\
               const B: u64 = double(A) + A\n\
               fn main() -> u64 { B }\n";
    assert_consistent(src, "ctfe_const_chain");
    assert_eq!(interpreter_value(src), 18);
}
