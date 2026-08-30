//! RUNTIME-LIB P0-B: `core/std/parse.t` across the backends.
//!
//! The integer and bool parsers are pure toylang, so what is really
//! being checked there is that the stdlib code lowers — but `to_f64`
//! crosses an extern boundary into two different implementations
//! (Rust's `str::parse` in the interpreter, libc `strtod` in the
//! compiled runtime). They agree because the *grammar* is decided in
//! toylang before either is called and both conversions are correctly
//! rounded; these tests are what keeps that true.

use super::harness::{assert_consistent, assert_stdout_consistent};

#[test]
fn integers_parse_to_the_same_values_everywhere() {
    let src = r#"
        fn code(s: str) -> u64 {
            val r = parse::to_u64(s)
            match r {
                Result::Ok(v) => v,
                Result::Err(ParseError::Empty) => 901u64,
                Result::Err(ParseError::Invalid) => 902u64,
                Result::Err(ParseError::Overflow) => 903u64,
            }
        }

        fn main() -> u64 {
            # 42 + 7 + invalid + overflow + empty, summed so one
            # disagreement anywhere changes the answer.
            code("42") + code("+7") + code("-1") + code("18446744073709551616") + code("")
        }
    "#;
    assert_consistent(src, "parse_u64_codes");
}

#[test]
fn the_signed_boundary_values_agree() {
    let src = r#"
        fn main() -> u64 {
            val min = parse::to_i64("-9223372036854775808")
            val max = parse::to_i64("9223372036854775807")
            val over = parse::to_i64("9223372036854775808")
            var code: u64 = 0u64
            match min {
                Result::Ok(v) => { if v == -9223372036854775808i64 { code = code + 1u64 } }
                Result::Err(_) => { code = code + 10u64 }
            }
            match max {
                Result::Ok(v) => { if v == 9223372036854775807i64 { code = code + 2u64 } }
                Result::Err(_) => { code = code + 20u64 }
            }
            match over {
                Result::Ok(_) => { code = code + 40u64 }
                Result::Err(ParseError::Overflow) => { code = code + 4u64 }
                Result::Err(_) => { code = code + 80u64 }
            }
            code
        }
    "#;
    assert_consistent(src, "parse_i64_bounds");
}

#[test]
fn parsed_floats_print_identically() {
    // The rendering is the check: two correctly rounded conversions of
    // the same decimal string must produce the same bits, and the
    // shortest-round-trip printing makes a one-ulp difference visible.
    let src = r#"
        fn show(s: str) {
            val r = parse::to_f64(s)
            match r {
                Result::Ok(v) => { println("{s} -> {v}") }
                Result::Err(e) => { println("{s} -> {e}") }
            }
        }

        fn main() -> u64 {
            show("1.5")
            show("0.1")
            show("3.141592653589793")
            show("2.5e3")
            show("25E-1")
            show("-0.000001")
            show("123456789.123456789")
            show("1e999")
            show("inf")
            show(" 1.5")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "parse_f64_render");
}

#[test]
fn a_parsed_number_survives_the_round_trip_from_to_string() {
    // The property that makes the pair useful: whatever the language
    // prints, it can read back.
    let src = r#"
        fn main() -> u64 {
            val n: u64 = 1234567u64
            val text: str = "{n}"
            val back = parse::to_u64(text)
            val f: f64 = 0.1f64 + 0.2f64
            val ftext: str = "{f}"
            val fback = parse::to_f64(ftext)
            var code: u64 = 0u64
            match back {
                Result::Ok(v) => { if v == n { code = code + 1u64 } }
                Result::Err(_) => { code = code + 10u64 }
            }
            match fback {
                Result::Ok(v) => { if v == f { code = code + 2u64 } }
                Result::Err(_) => { code = code + 20u64 }
            }
            code
        }
    "#;
    assert_consistent(src, "parse_round_trip");
}
