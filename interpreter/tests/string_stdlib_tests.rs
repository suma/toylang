// `core/std/string.t` / `core/std/collections/vec.t` stdlib API
// tests.
//
// Phase 0: `Vec<u8>::push_char` UTF-8 encoding (with the `char`
// alias upgraded from u8 to u32 in `core/std/char.t`).
// Phase 1: `Length` / `AsPtr` extension traits on `Vec<u8>`.
// Phase 2: `Substring` / `Trim` / `CaseConvert` extension traits
// on `Vec<u8>`.
// Deferred: `Concat<Other>` / `Contains<Needle>` (need AOT lower
// support for trait methods taking a struct argument).

mod common;

use common::{assert_program_fails, assert_program_result_u64};

// Returns each byte of the buffer packed into a u64 — bytes are
// laid out LSB-first so byte_0 ends up in the low 8 bits. The
// helper avoids needing a dedicated array assertion since every
// push_char test produces at most 4 bytes (well within u64).
fn pack_bytes_program(push_char_call: &str, expected_size: u64) -> String {
    format!(
        r#"
        fn main() -> u64 {{
            var s: Vec<u8> = Vec::new()
            {push_char_call}
            assert(s.size() == {expected_size}u64, "byte count mismatch")
            var packed: u64 = 0u64
            var i: u64 = 0u64
            while i < s.size() {{
                val b: u8 = s.get(i)
                packed = packed | ((b as u64) << (i * 8u64))
                i = i + 1u64
            }}
            packed
        }}
        "#
    )
}

#[test]
fn push_char_ascii_single_byte() {
    // 0x41 = 'A' is < 0x80, encoded as a single byte.
    let src = pack_bytes_program("s.push_char(0x41u32)", 1);
    assert_program_result_u64(&src, 0x41);
}

#[test]
fn push_char_two_byte_utf8() {
    // 0xE9 = 'é' (Latin small letter e with acute). 2-byte UTF-8.
    // Bytes: [0xC3, 0xA9] → packed LSB-first = 0xA9C3.
    let src = pack_bytes_program("s.push_char(0xE9u32)", 2);
    assert_program_result_u64(&src, 0xA9C3);
}

#[test]
fn push_char_three_byte_utf8() {
    // 0x3042 = 'あ' (Hiragana letter A). 3-byte UTF-8.
    // Bytes: [0xE3, 0x81, 0x82] → packed LSB-first = 0x8281E3.
    let src = pack_bytes_program("s.push_char(0x3042u32)", 3);
    assert_program_result_u64(&src, 0x82_81_E3);
}

#[test]
fn push_char_four_byte_utf8() {
    // 0x1F600 = '😀' (grinning face). 4-byte UTF-8.
    // Bytes: [0xF0, 0x9F, 0x98, 0x80] → packed LSB-first =
    // 0x80989FF0.
    let src = pack_bytes_program("s.push_char(0x1F600u32)", 4);
    assert_program_result_u64(&src, 0x80_98_9F_F0);
}

#[test]
fn push_char_char_literal_produces_utf8() {
    // Confirm char literal `'a'` lexes to UInt32 (codepoint 97)
    // and goes through the UTF-8 encoder. Single-byte path.
    let src = pack_bytes_program("s.push_char('a')", 1);
    assert_program_result_u64(&src, 0x61);
}

#[test]
fn push_char_unicode_escape_literal() {
    // `'\u{1F600}'` (😀) at parse time → Kind::UInt32(0x1F600).
    // Same expected bytes as `push_char_four_byte_utf8`.
    let src = pack_bytes_program(r"s.push_char('\u{1F600}')", 4);
    assert_program_result_u64(&src, 0x80_98_9F_F0);
}

#[test]
fn push_char_surrogate_low_panics() {
    // U+D800 is the first high surrogate — not a valid scalar.
    // push_char must panic via the assert guard.
    let src = r#"
        fn main() -> u64 {
            var s: Vec<u8> = Vec::new()
            s.push_char(0xD800u32)
            0u64
        }
    "#;
    assert_program_fails(src);
}

#[test]
fn push_char_surrogate_high_panics() {
    // U+DFFF is the last low surrogate.
    let src = r#"
        fn main() -> u64 {
            var s: Vec<u8> = Vec::new()
            s.push_char(0xDFFFu32)
            0u64
        }
    "#;
    assert_program_fails(src);
}

#[test]
fn push_char_out_of_range_panics() {
    // U+110000 is one past the Unicode max scalar.
    let src = r#"
        fn main() -> u64 {
            var s: Vec<u8> = Vec::new()
            s.push_char(0x110000u32)
            0u64
        }
    "#;
    assert_program_fails(src);
}

// ---------------------------------------------------------------
// Phase 1: `Length` / `AsPtr` for `Vec<u8>` (= `String`).
//
// `core/std/str.t` defines the `Length` and `AsPtr` traits that
// `str` already satisfies. `core/std/string.t` adds `impl
// Length for Vec<u8>` and `impl AsPtr for Vec<u8>` so user code
// can call `.len()` / `.as_ptr()` against either receiver shape
// (str / String) without caring which one a binding holds.
//
// The `Concat<Other>` / `Contains<Needle>` traits from the same
// session are **not** implemented yet — the AOT compiler currently
// rejects trait methods that take a struct (or `&struct`) argument
// with "method argument produced no value". That work is tracked
// separately.
// ---------------------------------------------------------------

#[test]
fn string_len_matches_byte_count() {
    // `Vec<u8>::len()` (via `impl Length for Vec<u8>`) returns the
    // same value as `.size()`.
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("hello world")
            assert(s.len() == 11u64, "len mismatch")
            assert(s.len() == s.size(), "len() == size()")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn string_len_with_multibyte_utf8() {
    // The byte length of a multi-byte UTF-8 string is bigger than
    // its logical character count. `é` (U+00E9) occupies 2 bytes,
    // `あ` (U+3042) 3 bytes, `😀` (U+1F600) 4 bytes — total 9 bytes.
    let src = r#"
        fn main() -> u64 {
            var s: Vec<u8> = Vec::new()
            s.push_char(0xE9u32)
            s.push_char(0x3042u32)
            s.push_char(0x1F600u32)
            assert(s.len() == 9u64, "expected 9 bytes")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn string_as_ptr_round_trip() {
    // `s.as_ptr()` returns the heap pointer; round-trip through
    // `__builtin_ptr_read` to confirm bytes match.
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("ABC")
            val p: ptr = s.as_ptr()
            val b0: u8 = __builtin_ptr_read(p, 0u64)
            val b1: u8 = __builtin_ptr_read(p, 1u64)
            val b2: u8 = __builtin_ptr_read(p, 2u64)
            assert(b0 == 0x41u8, "byte 0")
            assert(b1 == 0x42u8, "byte 1")
            assert(b2 == 0x43u8, "byte 2")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn str_and_string_share_len_method_name() {
    // The whole point of putting `Length` on both receivers — the
    // same `.len()` call works whether the binding is a `str`
    // literal or a heap-allocated `String`.
    let src = r#"
        fn main() -> u64 {
            val a = "hello"
            val b: String = Vec::from_str("hello")
            assert(a.len() == b.len(), "str.len() == String.len()")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

// ---------------------------------------------------------------
// Phase 2: `Substring` / `Trim` / `CaseConvert` for `Vec<u8>`.
// ---------------------------------------------------------------

#[test]
fn string_substring_basic() {
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("hello world")
            val sub: String = s.substring(6u64, 11u64)
            val expected: String = Vec::from_str("world")
            assert(sub.eq(expected), "substring mismatch")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn string_substring_empty_range() {
    // start == end returns an empty buffer (size 0).
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("hello")
            val sub: String = s.substring(2u64, 2u64)
            assert(sub.size() == 0u64, "empty substring size 0")
            assert(sub.is_empty(), "empty substring is_empty")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn string_substring_full_range() {
    // 0..len returns a copy of the whole buffer.
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("hello")
            val sub: String = s.substring(0u64, s.len())
            assert(sub.eq(s), "full-range substring equals self")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn string_substring_inverted_range_panics() {
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("hello")
            val sub: String = s.substring(3u64, 1u64)
            sub.size()
        }
    "#;
    assert_program_fails(src);
}

#[test]
fn string_substring_out_of_range_panics() {
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("hi")
            val sub: String = s.substring(0u64, 10u64)
            sub.size()
        }
    "#;
    assert_program_fails(src);
}

#[test]
fn string_trim_strips_both_ends() {
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("  hi  ")
            val t: String = s.trim()
            val expected: String = Vec::from_str("hi")
            assert(t.eq(expected), "trim both ends")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn string_trim_only_whitespace_returns_empty() {
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("   \t\n\r  ")
            val t: String = s.trim()
            assert(t.size() == 0u64, "trim of all-whitespace is empty")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn string_trim_no_whitespace_unchanged() {
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("hello")
            val t: String = s.trim()
            assert(t.eq(s), "trim of clean string equals self")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn string_to_upper_ascii() {
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("Hello, World!")
            val u: String = s.to_upper()
            val expected: String = Vec::from_str("HELLO, WORLD!")
            assert(u.eq(expected), "to_upper ascii")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn string_to_lower_ascii() {
    let src = r#"
        fn main() -> u64 {
            val s: String = Vec::from_str("Hello, World!")
            val l: String = s.to_lower()
            val expected: String = Vec::from_str("hello, world!")
            assert(l.eq(expected), "to_lower ascii")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn string_case_convert_preserves_high_bit_bytes() {
    // 0xC3 0xA9 = UTF-8 'é'. Both bytes have the high bit set;
    // case fold should leave them unchanged so multi-byte
    // sequences pass through.
    let src = r#"
        fn main() -> u64 {
            var s: Vec<u8> = Vec::new()
            s.push_char(0xE9u32)
            val u: Vec<u8> = s.to_upper()
            val l: Vec<u8> = s.to_lower()
            assert(u.size() == 2u64, "upper preserves byte count")
            assert(l.size() == 2u64, "lower preserves byte count")
            assert(u.get(0u64) == 0xC3u8, "upper byte 0 unchanged")
            assert(u.get(1u64) == 0xA9u8, "upper byte 1 unchanged")
            assert(l.get(0u64) == 0xC3u8, "lower byte 0 unchanged")
            assert(l.get(1u64) == 0xA9u8, "lower byte 1 unchanged")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}

#[test]
fn push_char_appends_to_existing_buffer() {
    // push_char on a non-empty buffer keeps prior content intact
    // and appends the encoded bytes after.
    let src = r#"
        fn main() -> u64 {
            var s: Vec<u8> = Vec::from_str("hi")
            s.push_char(0x21u32)
            assert(s.size() == 3u64, "size after push_char")
            assert(s.get(0u64) == 104u8, "byte 0 = 'h'")
            assert(s.get(1u64) == 105u8, "byte 1 = 'i'")
            assert(s.get(2u64) == 33u8, "byte 2 = '!'")
            42u64
        }
    "#;
    assert_program_result_u64(src, 42);
}
