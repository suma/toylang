// CHAR-LITERAL-NUM: `'a'` is a `u32`, and a position that wants
// another integer width may have it.
//
// The two halves are deliberately in tension: a char literal is
// *held* as `u32` (a code point is 32 bits, and `char` is the alias
// for that), but a string's bytes are `u8`, so requiring an `as`
// cast at every comparison would mean writing `48u8` with the
// character in a comment — which is what this codebase did before.
// The exception is narrow: only a literal written as a character
// moves. A suffixed literal keeps the NUM-W rule, because its suffix
// already named its type.

use crate::common::{assert_program_result_u64, test_program};

#[test]
fn a_char_literal_is_u32_when_nothing_asks_otherwise() {
    assert_program_result_u64(
        r#"fn main() -> u64 {
            val c = 'a'
            val size: u64 = __builtin_sizeof(c)
            val as_u64: u64 = c as u64
            size * 1000u64 + as_u64
        }"#,
        4097, // 4 bytes, code point 97
    );
}

#[test]
fn a_position_naming_another_integer_type_gets_it() {
    assert_program_result_u64(
        r#"fn main() -> u64 {
            val b: u8 = '0'
            val h: u16 = 'A'
            val i: i64 = '\n'
            val u: u64 = 'z'
            (b as u64) + (h as u64) + (i as u64) + u
        }"#,
        48 + 65 + 10 + 122,
    );
}

#[test]
fn a_byte_can_be_compared_against_a_character() {
    // The reason the exception exists: `s.get(i)` is a `u8`.
    assert_program_result_u64(
        r#"fn main() -> u64 {
            val s = String::from_str("hi9")
            var code: u64 = 0u64
            if s.get(0u64) == 'h' { code = code + 1u64 }
            if s.get(2u64) >= '0' && s.get(2u64) <= '9' { code = code + 2u64 }
            val digit: u64 = (s.get(2u64) - '0') as u64
            code + digit * 10u64
        }"#,
        93, // 1 + 2 + 9 * 10
    );
}

#[test]
fn byte_iteration_yields_bytes_and_still_compares_against_characters() {
    // `String::iter()` is a *byte* iterator (`u8` per step) — the
    // char-level API is `push_char`, which takes the `u32` code
    // point. Both sides meet a char literal without a cast.
    assert_program_result_u64(
        r#"fn main() -> u64 {
            val s = String::from_str("hello")
            var ells: u64 = 0u64
            for b in s.iter() {
                if b == 'l' { ells = ells + 1u64 }
            }
            var out = String::new()
            out.push_char('A')
            ells * 10u64 + out.size()
        }"#,
        21, // two `l`s, one byte pushed
    );
}

#[test]
fn a_char_literal_argument_takes_the_parameter_type() {
    assert_program_result_u64(
        r#"fn as_byte(b: u8) -> u64 { b as u64 }
        fn as_code(c: char) -> u64 { c as u64 }

        fn main() -> u64 {
            as_byte('A') + as_code('A')
        }"#,
        130,
    );
}

#[test]
fn a_code_point_that_does_not_fit_is_refused() {
    // The same report an out-of-range integer literal gets.
    let err = test_program(
        r#"fn main() -> u64 {
            val b: u8 = '\u{1F600}'
            b as u64
        }"#,
    )
    .expect_err("a 4-byte code point is not a byte");
    assert!(err.contains("128512"), "{err}");
    assert!(err.contains("UInt8"), "{err}");
}

#[test]
fn a_suffixed_literal_is_still_strict() {
    // The exception is for characters, not for `u32` in general:
    // `42u32` said what it was, so nothing is left to decide.
    let err = test_program(
        r#"fn main() -> u64 {
            val b: u8 = 42u32
            b as u64
        }"#,
    )
    .expect_err("a suffixed literal keeps the NUM-W rule");
    assert!(err.contains("u8") && err.contains("u32"), "{err}");
}

#[test]
fn escapes_carry_their_code_point() {
    assert_program_result_u64(
        r#"fn main() -> u64 {
            val nl: u8 = '\n'
            val tab: u8 = '\t'
            val quote: u8 = '\''
            val hex: u8 = '\x41'
            val uni: u32 = '\u{1F600}'
            (nl as u64) + (tab as u64) + (quote as u64) + (hex as u64) + (uni as u64)
        }"#,
        10 + 9 + 39 + 65 + 128512,
    );
}

#[test]
fn the_narrowing_reaches_stdlib_free_functions() {
    // `core/std/parse.t` scans bytes against `'0'` / `'9'` / `'+'`,
    // in free functions rather than `impl` blocks. Those bodies were
    // not type-checked until this landed, so the literals stayed
    // `u32` and the comparison failed at run time — the same hole
    // would have swallowed a `?` or a `Display` insertion there.
    assert_program_result_u64(
        r#"fn main() -> u64 {
            val n = parse::to_u64("+9")
            val bad = parse::to_u64("x")
            var code: u64 = 0u64
            match n {
                Result::Ok(v) => { code = code + v }
                Result::Err(_) => { code = code + 100u64 }
            }
            match bad {
                Result::Ok(_) => { code = code + 200u64 }
                Result::Err(_) => { code = code + 1u64 }
            }
            code
        }"#,
        10,
    );
}

#[test]
fn a_character_works_as_a_match_pattern_on_its_own_width() {
    // `match` over a `u32` scrutinee: the pattern is a char literal
    // of the same width. (A `u8` scrutinee is a separate gap — the
    // checker only accepts bool / i64 / u64 / str scrutinees today.)
    assert_program_result_u64(
        r#"fn main() -> u64 {
            val c: u64 = 'b' as u64
            match c {
                97u64 => 1u64,
                98u64 => 2u64,
                _ => 3u64,
            }
        }"#,
        2,
    );
}
