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


// COLLECTIONS C0(a). `==` between two values of a type parameter needs
// no `Eq` bound — a container method written that way dispatches to
// whatever `eq` the element type has. What has to be caught is the
// element type that has none: before this, the program type-checked and
// died at run time with `expected Struct(SymbolU32 { value: 60 }, []),
// found Struct(SymbolU32 { value: 60 }, [])` — the same type printed as
// a mismatch.
#[test]
fn a_type_argument_without_eq_is_rejected_at_the_call_site() {
    let src = r#"
struct P { x: i64 }
struct Bag<T> { v: Vec<T> }

impl<T> Bag<T> {
    fn contains(&self, needle: T) -> bool {
        val e: T = self.v.get(0u64)
        e == needle
    }
}

fn main() -> u64 {
    var b: Bag<P> = Bag { v: Vec::new() }
    b.v.push(P { x: 1i64 })
    # a struct literal in an `if` condition collides with the block
    # brace, so the probe is a binding
    val probe: P = P { x: 1i64 }
    if b.contains(probe) { 1u64 } else { 0u64 }
}
"#;
    let errors = type_check_errors(src);
    assert!(
        errors.iter().any(|e| {
            e.contains("contains") && e.contains("`==`") && e.contains("`P` has no `eq`")
        }),
        "expected the missing `eq` on `P` to be reported at the call, got: {errors:?}"
    );
}

// The stdlib case the check exists for: a `Dict` key is compared with
// `==` on every insert and lookup.
#[test]
fn a_dict_key_without_eq_is_rejected() {
    let src = r#"
struct P { x: i64 }

fn main() -> u64 {
    var d: Dict<P, u64> = Dict::new()
    d.insert(P { x: 1i64 }, 5u64)
    d.get_or(P { x: 1i64 }, 0u64)
}
"#;
    let errors = type_check_errors(src);
    assert!(
        errors.iter().any(|e| e.contains("insert") && e.contains("`P` has no `eq`")),
        "expected the `Dict` key to be rejected, got: {errors:?}"
    );
}

// An enum cannot answer `==` at all (overloading is a struct feature),
// so the advice has to point somewhere else than "write an `eq`".
#[test]
fn an_enum_type_argument_is_told_to_match_instead() {
    let src = r#"
enum Color { Red, Green }

fn same<T>(a: T, b: T) -> bool { a == b }

fn main() -> u64 {
    val answer: bool = same(Color::Red, Color::Green)
    if answer { 1u64 } else { 0u64 }
}
"#;
    let errors = type_check_errors(src);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("`Color` is an enum") && e.contains("match on the variants")),
        "expected the enum advice, got: {errors:?}"
    );
}

// The other half: a type that does have `eq` still instantiates, and
// the comparison still dispatches to it (the `eq` here looks at one
// field only, so a structural comparison would answer differently).
#[test]
fn a_type_argument_with_eq_still_works_on_every_lane() {
    let src = r#"
struct Key { a: i64, b: i64 }

impl Key {
    fn eq(&self, other: &Key) -> bool { self.a == other.a }
}

struct Bag<T> { v: Vec<T> }

impl<T> Bag<T> {
    fn contains(&self, needle: T) -> bool {
        var i: u64 = 0u64
        while i < self.v.size() {
            val e: T = self.v.get(i)
            if e == needle { return true }
            i = i + 1u64
        }
        false
    }
}

fn main() -> u64 {
    var b: Bag<Key> = Bag { v: Vec::new() }
    b.v.push(Key { a: 1i64, b: 2i64 })
    val same_a: Key = Key { a: 1i64, b: 99i64 }
    val other_a: Key = Key { a: 5i64, b: 2i64 }
    var out: u64 = 0u64
    if b.contains(same_a) { out = out + 1u64 }
    if b.contains(other_a) { out = out + 2u64 }
    out
}
"#;
    assert_value(src, "eq_dispatch_through_a_type_parameter", 1u64);
}
