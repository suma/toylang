// RUNTIME-LIB P0-B: `parse::to_u64` / `to_i64` / `to_f64` / `to_bool`
// (`core/std/parse.t`).
//
// The pair of `__builtin_to_string`: without these, a program can
// print a number but not read one back, so `io::read_line` and
// `io::arg` dead-end at `str`.
//
// The grammar is deliberately narrow, and most of what is worth
// testing is what it *refuses* — surrounding whitespace, a `0x`
// prefix, a bare sign, `inf` — because each of those is a place where
// a permissive parser would quietly turn bad input into a number.

use crate::common::assert_program_result_u64;

/// Run one parse and report the outcome as a code: `Ok(v)` gives
/// `v` (offset by the caller), and each `ParseError` variant its own
/// number, so a single `u64` result distinguishes all four.
fn parse_code(call: &str, on_ok: &str) -> u64 {
    let src = format!(
        r#"fn main() -> u64 {{
            val r = {call}
            match r {{
                Result::Ok(v) => {on_ok},
                Result::Err(ParseError::Empty) => 901u64,
                Result::Err(ParseError::Invalid) => 902u64,
                Result::Err(ParseError::Overflow) => 903u64,
            }}
        }}"#
    );
    let mut program = None;
    interpreter::output::with_capture(|| {
        program = Some(crate::common::test_program(&src));
    });
    let result = program.expect("captured run").expect("run");
    let value = result.borrow().clone();
    match value {
        interpreter::object::Object::UInt64(n) => n,
        other => panic!("expected a u64 result, got {other:?}"),
    }
}

const EMPTY: u64 = 901;
const INVALID: u64 = 902;
const OVERFLOW: u64 = 903;

#[test]
fn unsigned_integers_read_back_what_to_string_writes() {
    assert_eq!(parse_code(r#"parse::to_u64("0")"#, "v"), 0);
    assert_eq!(parse_code(r#"parse::to_u64("42")"#, "v"), 42);
    assert_eq!(parse_code(r#"parse::to_u64("+42")"#, "v"), 42);
    // u64::MAX, the boundary the accumulator has to reach exactly.
    assert_eq!(
        parse_code(
            r#"parse::to_u64("18446744073709551615")"#,
            "if v == 18446744073709551615u64 { 1u64 } else { 0u64 }"
        ),
        1
    );
}

#[test]
fn an_unsigned_parse_refuses_what_it_cannot_represent() {
    assert_eq!(parse_code(r#"parse::to_u64("")"#, "v"), EMPTY);
    assert_eq!(parse_code(r#"parse::to_u64("18446744073709551616")"#, "v"), OVERFLOW);
    // A negative number is not a `u64`: refused rather than wrapped.
    assert_eq!(parse_code(r#"parse::to_u64("-1")"#, "v"), INVALID);
    assert_eq!(parse_code(r#"parse::to_u64("12a")"#, "v"), INVALID);
    assert_eq!(parse_code(r#"parse::to_u64("+")"#, "v"), INVALID);
    // No trimming: a caller who wanted it can call `.trim()`, but a
    // caller who wanted strictness cannot undo it.
    assert_eq!(parse_code(r#"parse::to_u64(" 42")"#, "v"), INVALID);
    assert_eq!(parse_code(r#"parse::to_u64("42 ")"#, "v"), INVALID);
    // Source-literal syntax is not input syntax.
    assert_eq!(parse_code(r#"parse::to_u64("0xFF")"#, "v"), INVALID);
    assert_eq!(parse_code(r#"parse::to_u64("1_000")"#, "v"), INVALID);
}

#[test]
fn signed_integers_keep_the_value_whose_positive_form_does_not_fit() {
    assert_eq!(parse_code(r#"parse::to_i64("7")"#, "v as u64"), 7);
    assert_eq!(parse_code(r#"parse::to_i64("+7")"#, "v as u64"), 7);
    assert_eq!(
        parse_code(r#"parse::to_i64("-7")"#, "if v == 0i64 - 7i64 { 1u64 } else { 0u64 }"),
        1
    );
    // i64::MIN: read as a magnitude first, so it is not lost to the
    // fact that its positive form is out of range.
    assert_eq!(
        parse_code(
            r#"parse::to_i64("-9223372036854775808")"#,
            "if v == -9223372036854775808i64 { 1u64 } else { 0u64 }"
        ),
        1
    );
    assert_eq!(
        parse_code(
            r#"parse::to_i64("9223372036854775807")"#,
            "if v == 9223372036854775807i64 { 1u64 } else { 0u64 }"
        ),
        1
    );
    assert_eq!(parse_code(r#"parse::to_i64("9223372036854775808")"#, "v as u64"), OVERFLOW);
    assert_eq!(parse_code(r#"parse::to_i64("-9223372036854775809")"#, "v as u64"), OVERFLOW);
    assert_eq!(parse_code(r#"parse::to_i64("-")"#, "v as u64"), INVALID);
}

#[test]
fn floats_accept_the_forms_the_language_itself_writes() {
    assert_eq!(
        parse_code(r#"parse::to_f64("1.5")"#, "if v == 1.5f64 { 1u64 } else { 0u64 }"),
        1
    );
    assert_eq!(
        parse_code(r#"parse::to_f64("-2.5")"#, "if v == 0.0f64 - 2.5f64 { 1u64 } else { 0u64 }"),
        1
    );
    // A bare integer is a float too, as is a leading or trailing dot.
    assert_eq!(parse_code(r#"parse::to_f64("3")"#, "if v == 3.0f64 { 1u64 } else { 0u64 }"), 1);
    assert_eq!(parse_code(r#"parse::to_f64(".5")"#, "if v == 0.5f64 { 1u64 } else { 0u64 }"), 1);
    assert_eq!(parse_code(r#"parse::to_f64("5.")"#, "if v == 5.0f64 { 1u64 } else { 0u64 }"), 1);
    // Exponents, which the language has no literal syntax for.
    assert_eq!(
        parse_code(r#"parse::to_f64("2.5e3")"#, "if v == 2500.0f64 { 1u64 } else { 0u64 }"),
        1
    );
    assert_eq!(
        parse_code(r#"parse::to_f64("25E-1")"#, "if v == 2.5f64 { 1u64 } else { 0u64 }"),
        1
    );
}

#[test]
fn a_float_too_large_to_represent_is_an_error_not_an_infinity() {
    assert_eq!(parse_code(r#"parse::to_f64("1e999")"#, "0u64"), OVERFLOW);
    assert_eq!(parse_code(r#"parse::to_f64("-1e999")"#, "0u64"), OVERFLOW);
    // Underflow is not an error: the nearest representable value is 0.
    assert_eq!(parse_code(r#"parse::to_f64("1e-999")"#, "if v == 0.0f64 { 1u64 } else { 0u64 }"), 1);
}

#[test]
fn floats_refuse_the_spellings_a_c_strtod_would_take() {
    assert_eq!(parse_code(r#"parse::to_f64("")"#, "0u64"), EMPTY);
    assert_eq!(parse_code(r#"parse::to_f64("inf")"#, "0u64"), INVALID);
    assert_eq!(parse_code(r#"parse::to_f64("nan")"#, "0u64"), INVALID);
    assert_eq!(parse_code(r#"parse::to_f64("0x1p3")"#, "0u64"), INVALID);
    assert_eq!(parse_code(r#"parse::to_f64(" 1.5")"#, "0u64"), INVALID);
    assert_eq!(parse_code(r#"parse::to_f64("1.5f64")"#, "0u64"), INVALID);
    assert_eq!(parse_code(r#"parse::to_f64(".")"#, "0u64"), INVALID);
    assert_eq!(parse_code(r#"parse::to_f64("1e")"#, "0u64"), INVALID);
    assert_eq!(parse_code(r#"parse::to_f64("1e+")"#, "0u64"), INVALID);
}

#[test]
fn booleans_are_the_two_words_and_nothing_else() {
    assert_eq!(parse_code(r#"parse::to_bool("true")"#, "if v { 1u64 } else { 0u64 }"), 1);
    assert_eq!(parse_code(r#"parse::to_bool("false")"#, "if v { 1u64 } else { 2u64 }"), 2);
    assert_eq!(parse_code(r#"parse::to_bool("")"#, "0u64"), EMPTY);
    assert_eq!(parse_code(r#"parse::to_bool("True")"#, "0u64"), INVALID);
    assert_eq!(parse_code(r#"parse::to_bool("1")"#, "0u64"), INVALID);
    assert_eq!(parse_code(r#"parse::to_bool("yes")"#, "0u64"), INVALID);
}

#[test]
fn the_error_says_what_went_wrong() {
    // `ParseError` renders through `Display`, so a program can report
    // the reason without matching on it.
    assert_program_result_u64(
        r#"fn main() -> u64 {
            val r = parse::to_u64("nope")
            val text = match r {
                Result::Ok(_) => "ok",
                Result::Err(e) => e.to_str(),
            }
            if text == "invalid number" { 1u64 } else { 0u64 }
        }"#,
        1,
    );
}
