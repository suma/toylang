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
    val m1: u64 = hash_mix(1u64)
    val m2: u64 = hash_mix(2u64)
    val m3: u64 = hash_mix(3u64)
    if m1 != m2 { out = out + 1u64 }
    if (m1 & 15u64) != (m2 & 15u64) { out = out + 2u64 }
    if (m2 & 15u64) != (m3 & 15u64) { out = out + 4u64 }
    # a bijection: mixing is not allowed to fold two keys together
    if hash_mix(0u64) != m1 { out = out + 8u64 }
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
// `==` on every insert and lookup. `Hash` is a declared bound and is
// reported by the ordinary bound check, so the key here has one — what
// is left for this check is the `eq` nothing declares.
#[test]
fn a_dict_key_without_eq_is_rejected() {
    let src = r#"
struct P { x: i64 }

impl Hash for P {
    fn hash(self: Self) -> u64 { self.x as u64 }
}

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

// COLLECTIONS C1. The table is a separate array of indices, so the
// entries keep insertion order — including across a removal, which the
// old swap-remove broke (1, 2, 3 minus 1 used to iterate 3, 2).
#[test]
fn iteration_keeps_insertion_order_across_a_removal() {
    let src = r#"
fn main() -> u64 {
    var d: Dict<u64, u64> = Dict::new()
    d.insert(1u64, 10u64)
    d.insert(2u64, 20u64)
    d.insert(3u64, 30u64)
    d.remove(1u64)
    # 2 then 3, as inserted — not 3 then 2
    var out: u64 = 0u64
    for kv in d.iter() {
        out = out * 10u64 + kv.0
    }
    out
}
"#;
    assert_value(src, "dict_order_after_remove", 23u64);
}

// An update keeps the entry where it is, and a re-insert after a
// removal goes to the end.
#[test]
fn an_update_keeps_its_place_and_a_reinsert_goes_last() {
    let src = r#"
fn main() -> u64 {
    var d: Dict<u64, u64> = Dict::new()
    d.insert(1u64, 10u64)
    d.insert(2u64, 20u64)
    d.insert(3u64, 30u64)
    d.insert(1u64, 11u64)
    d.remove(2u64)
    d.insert(2u64, 22u64)
    var out: u64 = 0u64
    for kv in d.iter() {
        out = out * 10u64 + kv.0
    }
    out
}
"#;
    assert_value(src, "dict_order_update_and_reinsert", 132u64);
}

// Enough keys to grow the slot table several times (it starts at 8 and
// doubles past a 7/8 load), with every lookup checked afterwards. A
// rehash that dropped or duplicated an entry shows up here.
#[test]
fn the_table_survives_growing() {
    let src = r#"
fn main() -> u64 {
    var d: Dict<u64, u64> = Dict::new()
    var i: u64 = 0u64
    while i < 200u64 {
        d.insert(i * 7u64, i)
        i = i + 1u64
    }
    var hits: u64 = 0u64
    var j: u64 = 0u64
    while j < 200u64 {
        if d.get_or(j * 7u64, 999u64) == j { hits = hits + 1u64 }
        j = j + 1u64
    }
    # every key found, none of the gaps present, size intact
    var out: u64 = 0u64
    if hits == 200u64 { out = out + 1u64 }
    if d.contains_key(1u64) { out = out + 2u64 }
    if d.size() == 200u64 { out = out + 4u64 }
    out
}
"#;
    assert_value(src, "dict_growth", 5u64);
}

// `str` keys go through the runtime hash rather than the identity one.
// They used to all land in the same bucket (`Hash for str` returned a
// constant 0), which a table cannot survive.
#[test]
fn str_keys_find_their_own_entries() {
    let src = r#"
fn main() -> u64 {
    var d: Dict<str, u64> = Dict::new()
    d.insert("alpha", 1u64)
    d.insert("beta", 2u64)
    d.insert("gamma", 3u64)
    d.remove("beta")
    var out: u64 = 0u64
    out = out + d.get_or("alpha", 0u64)
    out = out + d.get_or("gamma", 0u64) * 10u64
    out = out + d.get_or("beta", 7u64) * 100u64
    if d.contains_key("beta") { out = out + 1000u64 }
    out
}
"#;
    assert_value(src, "dict_str_keys", 731u64);
}

// Removing a key that was never there changes nothing, and removing
// from an empty dict is not a probe into an unallocated table.
#[test]
fn removing_an_absent_key_is_a_no_op() {
    let src = r#"
fn main() -> u64 {
    var d: Dict<u64, u64> = Dict::new()
    var out: u64 = 0u64
    if d.remove(9u64) { out = out + 100u64 }
    d.insert(1u64, 10u64)
    if d.remove(9u64) { out = out + 200u64 }
    out = out + d.size() + d.get_or(1u64, 0u64)
    out
}
"#;
    assert_value(src, "dict_remove_absent", 11u64);
}

// BUMP-CHUNK-OVERSIZE, found while measuring the table above. The AOT /
// JIT runtime hands out memory from 1 MiB bump chunks, and a request
// larger than a chunk used to get a chunk-sized allocation anyway —
// the caller then wrote past it. 400,000 u64 is 3.2 MiB in one
// allocation; writing at the far end of it faulted. (A smaller
// overrun lands in whatever malloc had next and corrupts it quietly,
// which is why the size here is one that actually crashes.)
#[test]
fn an_allocation_larger_than_a_bump_chunk_is_whole() {
    let src = r#"
fn main() -> u64 {
    var v: Vec<u64> = Vec::with_capacity(400000u64)
    v.set_size(400000u64)
    v.set(0u64, 3u64)
    v.set(399999u64, 5u64)
    v.get(0u64) + v.get(399999u64)
}
"#;
    assert_value(src, "bump_oversized_allocation", 8u64);
}

// COLLECTIONS C2. `Set<T>` is its own struct rather than
// `Dict<T, ()>`, which the compiled lanes reject, so `insert` can
// answer the question a set's insert actually asks: was this new?
#[test]
fn set_dedups_and_keeps_insertion_order() {
    let src = r#"
fn main() -> u64 {
    var s: Set<u64> = Set::new()
    var out: u64 = 0u64
    if s.insert(3u64) { out = out + 1u64 }
    if s.insert(1u64) { out = out + 2u64 }
    # already there: not new, and it keeps its place
    if s.insert(3u64) { out = out + 4u64 }
    s.insert(2u64)
    s.remove(1u64)
    var order: u64 = 0u64
    for v in s.iter() {
        order = order * 10u64 + v
    }
    # 8 (new, new, not-new) with 3 then 2 surviving in insertion order
    out * 100u64 + order
}
"#;
    assert_value(src, "set_insert_order", 332u64);
}

// Enough elements to rehash several times, then every one read back,
// plus a gap that was never inserted.
#[test]
fn the_set_survives_growing() {
    let src = r#"
fn main() -> u64 {
    var s: Set<u64> = Set::new()
    var i: u64 = 0u64
    while i < 200u64 {
        s.insert(i * 3u64)
        i = i + 1u64
    }
    var hits: u64 = 0u64
    var j: u64 = 0u64
    while j < 200u64 {
        if s.contains(j * 3u64) { hits = hits + 1u64 }
        j = j + 1u64
    }
    var out: u64 = 0u64
    if hits == 200u64 { out = out + 1u64 }
    if s.contains(1u64) { out = out + 2u64 }
    if s.size() == 200u64 { out = out + 4u64 }
    if s.insert(0u64) { out = out + 8u64 }
    out
}
"#;
    assert_value(src, "set_growth", 5u64);
}

// `Set` and `Dict` carry two copies of the same probe / mixer / growth
// arithmetic — there is no cheap way to abstract "a table with an
// optional value column" here. This is what keeps the copies honest:
// fed the same keys in the same order, through the same growth and the
// same removals, the two have to iterate identically.
#[test]
fn a_set_and_a_dict_agree_on_order() {
    let src = r#"
fn main() -> u64 {
    var s: Set<u64> = Set::new()
    var d: Dict<u64, u64> = Dict::new()
    var i: u64 = 0u64
    while i < 40u64 {
        s.insert(i * 5u64)
        d.insert(i * 5u64, i)
        i = i + 1u64
    }
    s.remove(35u64)
    d.remove(35u64)
    s.remove(0u64)
    d.remove(0u64)

    var sset: u64 = 0u64
    for v in s.iter() {
        sset = sset * 31u64 + v
    }
    var sdict: u64 = 0u64
    for kv in d.iter() {
        sdict = sdict * 31u64 + kv.0
    }
    if sset == sdict { 1u64 } else { 0u64 }
}
"#;
    assert_value(src, "set_dict_same_order", 1u64);
}

#[test]
fn a_set_of_strs_uses_the_runtime_hash() {
    let src = r#"
fn main() -> u64 {
    var s: Set<str> = Set::new()
    s.insert("alpha")
    s.insert("beta")
    var out: u64 = 0u64
    if s.insert("alpha") { out = out + 1u64 }
    if s.contains("beta") { out = out + 2u64 }
    if s.remove("beta") { out = out + 4u64 }
    if s.contains("beta") { out = out + 8u64 }
    out + s.size()
}
"#;
    assert_value(src, "set_str_elements", 7u64);
}
