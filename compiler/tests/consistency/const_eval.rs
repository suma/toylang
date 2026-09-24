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

// --- C2: the fold inside a basic block ---------------------------

/// `lhs op rhs` computed twice inside one program: once from literals
/// (which C2 folds while lowering) and once through `var` bindings
/// holding the same values (which it cannot fold, since a local's
/// contents are not a block constant). The program returns 0 when the
/// two agree.
fn assert_ir_fold_matches_locals(ty: &str, lhs: &str, rhs: &str, op: &str, stem: &str) {
    let src = format!(
        "fn main() -> u64 {{\n    \
         var a: {ty} = {lhs}\n    \
         var b: {ty} = {rhs}\n    \
         val folded: {ty} = {lhs} {op} {rhs}\n    \
         val live: {ty} = a {op} b\n    \
         if folded == live {{ 0u64 }} else {{ 1u64 }}\n\
         }}\n"
    );
    assert_consistent(&src, stem);
    assert_eq!(
        interpreter_value(&src),
        0,
        "the folded answer differs from the computed one:\n{src}"
    );
}

#[test]
fn folding_wraps_where_the_language_wraps() {
    assert_ir_fold_matches_locals(
        "u64",
        "18446744073709551615u64",
        "3u64",
        "+",
        "c2_wrap_add",
    );
    assert_ir_fold_matches_locals(
        "u64",
        "18446744073709551615u64",
        "4u64",
        "*",
        "c2_wrap_mul",
    );
    assert_ir_fold_matches_locals("u8", "200u8", "3u8", "*", "c2_wrap_narrow");
}

#[test]
fn folding_keeps_signed_division_truncating() {
    assert_ir_fold_matches_locals("i64", "0i64 - 7i64", "3i64", "/", "c2_sdiv");
    assert_ir_fold_matches_locals("i64", "0i64 - 7i64", "3i64", "%", "c2_srem");
}

#[test]
fn folding_keeps_shifts_and_bitwise_ops() {
    assert_ir_fold_matches_locals("u64", "1u64", "40u64", "<<", "c2_shl");
    assert_ir_fold_matches_locals("u64", "18446744073709551615u64", "40u64", ">>", "c2_shr");
    assert_ir_fold_matches_locals("u64", "12u64", "10u64", "^", "c2_xor");
}

#[test]
fn folding_keeps_float_arithmetic() {
    assert_ir_fold_matches_locals("f64", "3.5f64", "0.1f64", "*", "c2_fmul");
    assert_ir_fold_matches_locals("f64", "1.0f64", "3.0f64", "/", "c2_fdiv");
}

#[test]
fn a_folded_expression_still_traps_where_the_language_traps() {
    // The other half of the rule: the fold *declines* where the
    // language traps, so RUNTIME-TRAP's guard survives and every
    // backend refuses the operation instead of inventing a value.
    // Written out rather than through `assert_consistent`, which
    // requires each backend to succeed before comparing.
    if skip_e2e() {
        return;
    }
    for (expr, stem) in [
        ("3u64 - 5u64", "c2_trap_underflow"),
        ("10u64 / 0u64", "c2_trap_div"),
        ("10u64 % 0u64", "c2_trap_rem"),
    ] {
        let src = format!("fn main() -> u64 {{ {expr} }}\n");
        let core = core_modules_dir();
        let mut opts = interpreter::RunOptions::default();
        opts.core_modules_dirs = std::slice::from_ref(&core);
        assert!(
            interpreter::run_source(&src, "trap.t", &opts).is_err(),
            "the interpreter should refuse `{expr}`"
        );
        let compiled = try_compiler_exit_code(&src, stem, true)
            .expect("the program still compiles — the guard is a run-time trap");
        assert_ne!(compiled, 0, "the compiled binary should exit non-zero for `{expr}`");
    }
}

// --- C5: a computed array length is a literal length --------------

#[test]
fn a_computed_array_length_matches_a_literal_one() {
    // `[i64; double(2u64)]` is resolved to `[i64; 4]` before lowering,
    // so the two spellings must behave identically on every backend.
    let computed = "const fn double(n: u64) -> u64 { n * 2u64 }\n\
                    fn main() -> u64 { val a: [i64; double(2u64)] = [1i64, 2i64, 3i64, 4i64]  (a[0] + a[3]) as u64 }\n";
    let literal = "fn main() -> u64 { val a: [i64; 4] = [1i64, 2i64, 3i64, 4i64]  (a[0] + a[3]) as u64 }\n";
    assert_consistent(computed, "c5_computed_length");
    assert_consistent(literal, "c5_literal_length");
    assert_eq!(
        interpreter_value(computed),
        interpreter_value(literal),
        "a computed length and the literal it resolves to must agree"
    );
}

/// CONST-ARRAY: a `const` table is read-only data, and every lane
/// reads the same bytes out of it.
///
/// The compiled lanes used to refuse the declaration outright ("only
/// literal values and references to earlier consts are supported"),
/// so a table had to be a `Vec` the program filled in at run time —
/// which is an allocation, and why `sha256`'s round constants kept
/// `compress` from being `never_allocates`.
#[test]
fn a_const_array_is_read_the_same_by_every_lane() {
    let src = r#"
        const K: [u32; 6] = [11u32, 22u32, 33u32, 44u32, 55u32, 66u32]
        const W: [i64; 3] = [-1i64, 0i64, 7i64]
        const B: [u8; 4] = [1u8, 2u8, 4u8, 8u8]

        fn at(i: u64) -> u32 { K[i] }

        fn main() -> u64 {
            # A constant index, folded, and a runtime one through a call.
            println(K[0u64])
            println(at(5u64))
            # Narrow elements keep their own width, so the table is
            # packed rather than one 8-byte slot per entry.
            var total: u64 = 0u64
            var i: u64 = 0u64
            while i < 4u64 {
                total = total + B[i] as u64
                i = i + 1u64
            }
            println(total)
            # A signed element is sign-extended, not read as a huge
            # unsigned — the bytes are the element's width.
            println(W[0u64])
            0u64
        }
    "#;
    assert_renders(src, "const_array_reads", "11\n66\n15\n-1\n");
}

/// The table is data, not a program: reading one allocates nothing.
#[test]
fn a_const_array_allocates_nothing() {
    let src = r#"
        const K: [u64; 4] = [2u64, 3u64, 5u64, 7u64]

        never_allocates fn product() -> u64 {
            var p: u64 = 1u64
            var i: u64 = 0u64
            while i < 4u64 {
                p = p * K[i]
                i = i + 1u64
            }
            p
        }

        fn main() -> u64 {
            val before: u64 = __builtin_live_bytes()
            println(product())
            println(__builtin_live_bytes() - before)
            0u64
        }
    "#;
    // `never_allocates` is the compile-time half of the claim and the
    // counter is the run-time half. Both, because the first proves no
    // allocating call is reachable and the second proves the lanes
    // agree about what the table itself costs.
    assert_renders(src, "const_array_allocation_free", "210\n0\n");
}

/// An index past the end panics, with the message and the position
/// every other array access gives.
#[test]
fn a_const_array_index_is_bounds_checked() {
    let src = r#"
        const K: [u64; 3] = [1u64, 2u64, 3u64]

        fn at(i: u64) -> u64 { K[i] }

        fn main() -> u64 {
            # Nothing on stdout first: the lanes flush a panicking
            # program's output differently, and this test is about
            # the panic.
            println(at(7u64))
            0u64
        }
    "#;
    assert_diagnostic_consistent(src, "const_array_out_of_bounds");
}

/// CONST-UNSUFFIXED-INIT: the declared type names the initializer's
/// type, so a suffix-less literal and a character literal take it the
/// way a `val` annotation makes them. This used to type-check and then
/// fail at run time on an `Expr::Number` the checker left behind; the
/// char literal was rejected outright (`declared as u8 but initializer
/// has type u32`).
#[test]
fn a_const_initializer_takes_its_declared_type() {
    let src = r#"
        const J: u64 = 4
        const K: i32 = -3
        const M: u8 = 'a'
        const S: u64 = 2 + 3
        const W: u16 = 0xFF
        const T: [u32; 3] = [1, 2, 3]

        fn main() -> u64 {
            println("{J} {K} {M} {S} {W} {T[2]}")
            J + S
        }
    "#;
    assert_renders(src, "const_unsuffixed_init", "4 -3 97 5 255 3\n");
}

/// A const table is already a value, so the fold never stubs it. It
/// used to be stubbed with a scalar `0` while any other const was
/// folded, which made the `T[1]` in `main` unlowerable: the fold gave
/// up without a word and the computed `B` reached the compiled lanes
/// unfolded ("cannot evaluate the initialiser for `const B`").
#[test]
fn a_const_table_does_not_stop_the_fold_of_its_neighbours() {
    let src = r#"
        const A: u64 = 5u64
        const B: i64 = -(A as i64) * 2i64
        const T: [u32; 3] = [1u32, 2u32, 3u32]

        fn main() -> u64 {
            println("{B} {T[1]}")
            A
        }
    "#;
    assert_renders(src, "const_table_beside_fold", "-10 2\n");
}
