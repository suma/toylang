// `??` null-coalesce operator tests.
//
// The parser emits `Expr::NullCoalesce { lhs, rhs, .. }`; the type
// checker rewrites it (in place for val/var right-hand sides, in a
// post-pass everywhere else) into a lazy `val` + `match` block, so the
// default operand only evaluates on the `None` / `Err` path. Backends
// (interpreter, AOT, JIT) therefore see only the desugared form —
// these tests exercise the construct via the interpreter end-to-end,
// and the matching example (`interpreter/example/null_coalesce.t`) is
// swept across all three backends by `example_consistency`.

use crate::common::{assert_program_result_i64, assert_program_result_u64, core_modules_dir};

fn assert_output(src: &str, expected: &str) {
    let core = core_modules_dir();
    let mut parser = frontend::ParserWithInterner::new(src);
    parser.set_source_file("null_coalesce.t");
    let mut program = parser
        .parse_program()
        .map_err(|e| format!("Parse error: {e:?}"))
        .expect("parse failed");
    let interner = parser.get_string_interner();
    interpreter::check_typing_with_core_modules(
        &mut program,
        interner,
        Some(src),
        Some("null_coalesce.t"),
        std::slice::from_ref(&core),
    )
    .map_err(|errors| format!("Type check errors: {errors:?}"))
    .expect("type check failed");
    let (_, stdout) = interpreter::output::with_capture(|| {
        interpreter::execute_program(&program, interner, None, Some("null_coalesce.t"))
    });
    assert_eq!(stdout, expected, "stdout mismatch");
}

fn assert_type_error(src: &str, expected_hint: &str) {
    let core = core_modules_dir();
    let mut parser = frontend::ParserWithInterner::new(src);
    parser.set_source_file("null_coalesce.t");
    let mut program = parser
        .parse_program()
        .map_err(|e| format!("Parse error: {e:?}"))
        .expect("parse failed");
    let interner = parser.get_string_interner();
    let err = interpreter::check_typing_with_core_modules(
        &mut program,
        interner,
        Some(src),
        Some("null_coalesce.t"),
        std::slice::from_ref(&core),
    )
    .expect_err("expected a type error");
    let rendered = format!("{err:?}");
    assert!(
        rendered.contains(expected_hint),
        "expected the error to mention `{expected_hint}`, got: {rendered}"
    );
}

// ---------------------------------------------------------------------
// Option: Some takes the value, None takes the default.
// ---------------------------------------------------------------------

#[test]
fn null_coalesce_option_some_takes_value() {
    let src = r#"
        fn main() -> u64 {
            val o: Option<u64> = Option::Some(7u64)
            o ?? 0u64
        }
    "#;
    assert_program_result_u64(src, 7u64);
}

#[test]
fn null_coalesce_option_none_takes_default() {
    let src = r#"
        fn main() -> u64 {
            val o: Option<u64> = Option::None
            o ?? 42u64
        }
    "#;
    assert_program_result_u64(src, 42u64);
}

#[test]
fn null_coalesce_result_ok_takes_value() {
    let src = r#"
        fn main() -> u64 {
            val r: Result<u64, str> = Result::Ok(9u64)
            r ?? 0u64
        }
    "#;
    assert_program_result_u64(src, 9u64);
}

#[test]
fn null_coalesce_result_err_takes_default() {
    let src = r#"
        fn main() -> u64 {
            val r: Result<u64, str> = Result::Err("boom")
            r ?? 5u64
        }
    "#;
    assert_program_result_u64(src, 5u64);
}

// ---------------------------------------------------------------------
// Laziness: the default operand only evaluates when needed.
// ---------------------------------------------------------------------

#[test]
fn null_coalesce_default_is_lazy() {
    // The closure counts how often the default ran. `Some(7) ?? d()`
    // must leave the counter at 0; `None ?? d()` brings it to 1.
    let src = r#"
        fn main() -> u64 {
            var calls: u64 = 0u64
            val d = fn() -> u64 {
                calls = calls + 1u64
                99u64
            }
            val some: Option<u64> = Option::Some(7u64)
            val a = some ?? d()
            val none: Option<u64> = Option::None
            val b = none ?? d()
            a + b * 10u64 + calls * 100u64
        }
    "#;
    // a = 7, b = 99, calls = 1 → 7 + 990 + 100 = 1097.
    assert_program_result_u64(src, 1097u64);
}

// ---------------------------------------------------------------------
// Chaining and precedence.
// ---------------------------------------------------------------------

#[test]
fn null_coalesce_chains_right_associatively() {
    // `a ?? b ?? c` groups as `a ?? (b ?? c)`: both Some → 1, Some/None
    // mix → the first Some in the chain, all None → the final default.
    let src = r#"
        fn main() -> u64 {
            val some: Option<u64> = Option::Some(1u64)
            val none: Option<u64> = Option::None
            val x = some ?? some ?? 8u64
            val y = none ?? some ?? 8u64
            val z = none ?? none ?? 8u64
            x * 100u64 + y * 10u64 + z
        }
    "#;
    assert_program_result_u64(src, 118u64);
}

#[test]
fn null_coalesce_binds_tighter_than_equality() {
    // `a ?? b == c` groups as `(a ?? b) == c`. (The parenthesized
    // spelling is avoided here: a line starting with `(` joins the
    // previous expression as a call argument list.)
    let src = r#"
        fn main() -> bool {
            val none: Option<u64> = Option::None
            val lhs = none ?? 3u64
            val explicit = lhs == 3u64
            val implicit = none ?? 3u64 == 3u64
            println(explicit && implicit)
            explicit && implicit
        }
    "#;
    assert_output(src, "true\n");
}

// ---------------------------------------------------------------------
// Positions that bypass the val-right-hand-side route.
// ---------------------------------------------------------------------

#[test]
fn null_coalesce_in_condition_and_argument_positions() {
    let src = r#"
        fn show(v: u64) -> u64 {
            println(v)
            v
        }
        fn main() -> u64 {
            val none: Option<u64> = Option::None
            if none ?? 1u64 == 1u64 {
                show(none ?? 2u64)
            } else {
                0u64
            }
        }
    "#;
    assert_output(src, "2\n");
}

#[test]
fn null_coalesce_in_return_position() {
    let src = r#"
        fn pick(o: Option<i64>) -> i64 {
            return o ?? -7i64
        }
        fn main() -> i64 {
            pick(Option::Some(3i64)) + pick(Option::None)
        }
    "#;
    assert_program_result_i64(src, -4i64);
}

// ---------------------------------------------------------------------
// Errors: non-Option receivers and mismatched arms are type errors.
// ---------------------------------------------------------------------

#[test]
fn null_coalesce_rejects_non_option_receiver() {
    let src = r#"
        fn main() -> u64 {
            val n = 3u64
            n ?? 0u64
        }
    "#;
    assert_type_error(src, "requires Option");
}

#[test]
fn null_coalesce_rejects_mismatched_arms() {
    let src = r#"
        fn main() -> u64 {
            val o: Option<u64> = Option::None
            o ?? "str"
        }
    "#;
    assert_type_error(src, "incompatible types");
}
