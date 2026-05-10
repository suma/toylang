// Debug & verification builtins added 2026-05-10:
//
//   - `__builtin_source_file()` / `__builtin_source_line()` /
//     `__builtin_source_column()` — parser-level literal substitution
//     of the call site's location.
//   - `__builtin_dbg(EXPR)` — captures EXPR's source text, prints
//     "[file:line] text = value" to stdout, and returns the value.
//   - `assert_eq(a, b)` / `assert_ne(a, b)` — desugar to a value
//     comparison + `assert(cond, "...")` whose message includes the
//     source line and pretty-printed left/right operands.
//
// Each macro is rewritten in `frontend/src/parser/expr.rs::try_intercept_parser_macro`
// so the interpreter / AOT / JIT see only ordinary AST shapes; these
// tests therefore cover the desugar correctness via the
// tree-walking interpreter end-to-end.

mod common;

use common::{assert_program_fails, assert_program_result_u64, test_program};
use interpreter::object::Object;

// ---------------------------------------------------------------------
// __builtin_source_line / column / file
// ---------------------------------------------------------------------

#[test]
fn source_line_returns_call_site_line_as_u64() {
    // The `__builtin_source_line()` call lives on line 4 of the
    // source string below (the leading `\n` from `r#"` puts `fn` on
    // line 2, the open brace on the same line, and the val binding
    // on line 3 + the call on line 4).
    let src = "\nfn main() -> u64 {\n    val v = 1u64\n    __builtin_source_line()\n}\n";
    assert_program_result_u64(src, 4u64);
}

#[test]
fn source_column_advances_for_calls_on_same_line() {
    // Pin the *relative* ordering of two calls on the same line: the
    // second call's column is strictly greater than the first's. The
    // exact column depends on whether the parser captures the
    // identifier's position or the following `(`, so the absolute
    // values are intentionally not asserted.
    let src = r#"
        fn pair() -> (u64, u64) {
            (__builtin_source_column(), __builtin_source_column())
        }

        fn main() -> u64 {
            val (a, b) = pair()
            if b > a { 1u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 1u64);
}

#[test]
fn source_file_returns_test_filename() {
    // Tests in `common.rs` set the parser source file to "test.t".
    let src = r#"
        fn main() -> u64 {
            val f = __builtin_source_file()
            f.len()
        }
    "#;
    // "test.t" has 6 bytes.
    assert_program_result_u64(src, 6u64);
}

// ---------------------------------------------------------------------
// __builtin_dbg
// ---------------------------------------------------------------------

#[test]
fn dbg_returns_inner_value_unchanged_for_u64() {
    let src = r#"
        fn main() -> u64 {
            __builtin_dbg(42u64)
        }
    "#;
    assert_program_result_u64(src, 42u64);
}

#[test]
fn dbg_returns_value_of_arithmetic_expr() {
    let src = r#"
        fn main() -> u64 {
            val x = __builtin_dbg(10u64 + 32u64)
            x
        }
    "#;
    assert_program_result_u64(src, 42u64);
}

#[test]
fn dbg_propagates_value_through_function_call() {
    let src = r#"
        fn double(n: u64) -> u64 { n * 2u64 }

        fn main() -> u64 {
            val r = double(__builtin_dbg(21u64))
            r
        }
    "#;
    assert_program_result_u64(src, 42u64);
}

#[test]
fn dbg_works_for_bool_value() {
    let src = r#"
        fn main() -> u64 {
            val b = __builtin_dbg(1u64 < 2u64)
            if b { 1u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 1u64);
}

#[test]
fn dbg_nested_calls_use_distinct_synthetic_names() {
    // Two `__builtin_dbg` calls in the same scope must not collide on
    // the synthesized `__dbg_<n>` binding. The counter increment makes
    // each call its own `__dbg_<n>`.
    let src = r#"
        fn main() -> u64 {
            __builtin_dbg(__builtin_dbg(7u64) + __builtin_dbg(8u64))
        }
    "#;
    assert_program_result_u64(src, 15u64);
}

// ---------------------------------------------------------------------
// assert_eq / assert_ne — happy paths
// ---------------------------------------------------------------------

#[test]
fn assert_eq_passes_for_equal_u64() {
    let src = r#"
        fn main() -> u64 {
            assert_eq(40u64 + 2u64, 42u64)
            0u64
        }
    "#;
    assert_program_result_u64(src, 0u64);
}

#[test]
fn assert_eq_passes_for_equal_strings() {
    let src = r#"
        fn main() -> u64 {
            assert_eq("hello", "hello")
            0u64
        }
    "#;
    assert_program_result_u64(src, 0u64);
}

#[test]
fn assert_ne_passes_for_unequal_values() {
    let src = r#"
        fn main() -> u64 {
            assert_ne(1u64, 2u64)
            assert_ne("foo", "bar")
            0u64
        }
    "#;
    assert_program_result_u64(src, 0u64);
}

// ---------------------------------------------------------------------
// assert_eq / assert_ne — failure paths
// ---------------------------------------------------------------------

#[test]
fn assert_eq_panics_on_mismatch() {
    let src = r#"
        fn main() -> u64 {
            assert_eq(1u64, 2u64)
            0u64
        }
    "#;
    let result = test_program(src);
    let err = result.expect_err("expected panic from assert_eq");
    assert!(
        err.contains("left == right"),
        "panic message should mention `left == right`, got: {err}"
    );
    assert!(
        err.contains("left:  1") && err.contains("right: 2"),
        "panic message should include both operands, got: {err}"
    );
}

#[test]
fn assert_ne_panics_on_match() {
    let src = r#"
        fn main() -> u64 {
            assert_ne(7u64, 7u64)
            0u64
        }
    "#;
    let result = test_program(src);
    let err = result.expect_err("expected panic from assert_ne");
    assert!(
        err.contains("left != right"),
        "panic message should mention `left != right`, got: {err}"
    );
}

#[test]
fn assert_eq_message_includes_source_line() {
    // Place the failing assert on a known line so we can pin the
    // line-number in the panic message.
    let src = "\nfn main() -> u64 {\n    assert_eq(1u64, 2u64)\n    0u64\n}\n";
    let result = test_program(src);
    let err = result.expect_err("expected panic");
    assert!(
        err.contains("line 3"),
        "panic message should include source line 3, got: {err}"
    );
}

// ---------------------------------------------------------------------
// Sanity: dbg/assert_eq/source_* don't break ordinary programs.
// ---------------------------------------------------------------------

#[test]
fn debug_builtins_compose_with_existing_program() {
    let src = r#"
        fn fact(n: u64) -> u64 {
            if n == 0u64 {
                1u64
            } else {
                n * fact(n - 1u64)
            }
        }

        fn main() -> u64 {
            val n = __builtin_dbg(5u64)
            val f = fact(n)
            assert_eq(f, 120u64)
            f
        }
    "#;
    assert_program_result_u64(src, 120u64);
}

// ---------------------------------------------------------------------
// Negative: unknown macro name still falls through to "function not
// found" (i.e. our intercept didn't accidentally swallow other names).
// ---------------------------------------------------------------------

#[test]
fn unknown_underscore_underscore_builtin_name_still_errors() {
    let src = r#"
        fn main() -> u64 {
            __builtin_does_not_exist()
        }
    "#;
    // The intercept only catches the four names registered in
    // `BuiltinFunctionSymbols`; anything else falls through to the
    // ordinary call dispatch and ultimately reaches the runtime as a
    // `FunctionNotFound` (or type-check) error.
    assert_program_fails(src);
}

// Suppress unused-import warning when only some helpers are used.
#[allow(dead_code)]
fn _force_object_use(_o: &Object) {}
