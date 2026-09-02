//! Collections groundwork (`design-docs/COLLECTIONS.md`): the hashing
//! the future open-addressing table is built on.
//!
//! The values matter across backends, not just within one: a key that
//! hashed differently in the interpreter and in a compiled binary would
//! land in a different slot of the same table, so these pin the
//! algorithm rather than just its shape.

use super::harness::*;

/// `assert_consistent` only asks the lanes to agree with each other,
/// which a wrong-but-uniform hash would satisfy. These tests pin the
/// value too: the constants below are canonical FNV-1a / splitmix64
/// output, so a rewrite of either algorithm has to be deliberate.
fn assert_value(source: &str, stem: &str, expected: u64) {
    assert_consistent(source, stem);
    if skip_e2e() {
        return;
    }
    let got = interpreter_value(source);
    assert_eq!(got, expected, "{stem}: value changed");
}

// `Hash for str` is FNV-1a in `toylang_rt` (`toy_str_hash`) and, for
// the interpreter, in `extern_io::str_hash`. `impl Hash for String`
// walks its own bytes in toylang with the same constants. All three
// have to produce the canonical FNV-1a value, or a `String` key and
// the `str` holding the same text would not find each other.
#[test]
fn str_and_string_hash_to_the_same_fnv1a_value() {
    let src = r#"
fn main() -> u64 {
    val a: u64 = "abc".hash()
    # the compiled lanes want a binding before a method call on a
    # constructor's result, so this is `val` + `.hash()` rather than a
    # chain
    val s: String = String::from_str("abc")
    val b: u64 = s.hash()
    var out: u64 = 0u64
    if a == b { out = out + 1u64 }
    if a == 16654208175385433931u64 { out = out + 2u64 }
    if "".hash() == 14695981039346656037u64 { out = out + 4u64 }
    if "abd".hash() != a { out = out + 8u64 }
    out
}
"#;
    assert_value(src, "hash_str_and_string_agree", 15u64);
}

// The mixer is what an open-addressing table applies before taking the
// low bits as a slot index, so the property under test is exactly that:
// neighbouring keys, whose identity hashes differ in one bit, have to
// land in different slots of a small table.
#[test]
fn mix_spreads_neighbouring_keys_across_low_bits() {
    let src = r#"
fn main() -> u64 {
    var out: u64 = 0u64
    val m1: u64 = mix(1u64)
    val m2: u64 = mix(2u64)
    val m3: u64 = mix(3u64)
    if m1 != m2 { out = out + 1u64 }
    if (m1 & 15u64) != (m2 & 15u64) { out = out + 2u64 }
    if (m2 & 15u64) != (m3 & 15u64) { out = out + 4u64 }
    # a bijection: mixing is not allowed to fold two keys together
    if mix(0u64) != m1 { out = out + 8u64 }
    out
}
"#;
    assert_value(src, "hash_mix_spreads_low_bits", 15u64);
}

// The contract `hash` actually promises: equal values hash equally.
// The narrow signed widths route through their unsigned width first,
// so `-5i8` must not sign-extend into a different bucket than the byte
// it really is.
#[test]
fn equal_values_hash_equally_across_widths() {
    let src = r#"
fn main() -> u64 {
    var out: u64 = 0u64
    if 7u64.hash() == 7u64.hash() { out = out + 1u64 }
    if (-5i8).hash() == 251u64 { out = out + 2u64 }
    if true.hash() != false.hash() { out = out + 4u64 }
    val k: String = String::from_str("k")
    if k.hash() == "k".hash() { out = out + 8u64 }
    out
}
"#;
    assert_value(src, "hash_equal_values_agree", 15u64);
}

