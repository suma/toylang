// Iterator protocol tests — `for x in EXPR { body }`.
//
// `EXPR` is any value whose type provides `fn next(&mut self) -> Option<T>`
// (structural / duck-typed; the parser desugars at parse time so no
// `trait Iterator<T>` declaration is required — generic-trait support
// is on the deferred list, see `core/std/iter.t`).
//
// The desugaring (in `frontend/src/parser/stmt.rs::desugar_for_in_iterator`):
//
//     for x in EXPR { body }
//   ⇒ {
//         var __iter_for_<n> = EXPR
//         while true {
//             match __iter_for_<n>.next() {
//                 Option::Some(x) => { body; continue },
//                 Option::None    => { break },
//             }
//         }
//     }
//
// Bare integer ranges (`for i in 0..N` / `for i in 0 to N`) keep
// their dedicated `Stmt::For` integer fast path and don't flow
// through this protocol.


use crate::common::{assert_program_result_i64, assert_program_result_u64};

const COUNTER_PRELUDE: &str = "
struct Counter {
    current: i64,
    end: i64,
}
impl Counter {
    fn new(end: i64) -> Self {
        Counter { current: 0i64, end: end }
    }
    fn next(&mut self) -> Option<i64> {
        if self.current >= self.end {
            Option::None
        } else {
            val v = self.current
            self.current = self.current + 1i64
            Option::Some(v)
        }
    }
}
";

fn program_with_counter(main_body: &str) -> String {
    format!("{COUNTER_PRELUDE}\nfn main() -> i64 {{\n{main_body}\n}}\n")
}

#[test]
fn iterator_basic_sums_to_ten() {
    // 0+1+2+3+4 = 10
    assert_program_result_i64(
        &program_with_counter(
            "    var sum = 0i64
    var iter = Counter::new(5i64)
    for x in iter { sum = sum + x }
    sum",
        ),
        10,
    );
}

#[test]
fn iterator_break_terminates_early() {
    // first five values yielded, then break — sum 0+1+2+3+4 = 10
    assert_program_result_i64(
        &program_with_counter(
            "    var sum = 0i64
    var iter = Counter::new(100i64)
    for x in iter {
        if x >= 5i64 { break }
        sum = sum + x
    }
    sum",
        ),
        10,
    );
}

#[test]
fn iterator_continue_skips_iteration() {
    // 0..10 keeping evens: 0+2+4+6+8 = 20
    assert_program_result_i64(
        &program_with_counter(
            "    var sum = 0i64
    var iter = Counter::new(10i64)
    for x in iter {
        if x % 2i64 == 1i64 { continue }
        sum = sum + x
    }
    sum",
        ),
        20,
    );
}

#[test]
fn iterator_return_propagates_from_for_body() {
    assert_program_result_i64(
        &format!(
            "{COUNTER_PRELUDE}
fn first_ge(threshold: i64) -> i64 {{
    var iter = Counter::new(100i64)
    for x in iter {{
        if x >= threshold {{ return x }}
    }}
    -1i64
}}
fn main() -> i64 {{
    first_ge(7i64)
}}
"
        ),
        7,
    );
}

#[test]
fn iterator_nested_two_loops() {
    // sum_{i,j in 0..3} i*j
    //  = (0+0+0)+(0+1+2)+(0+2+4) = 9
    assert_program_result_i64(
        &program_with_counter(
            "    var total = 0i64
    var outer = Counter::new(3i64)
    for i in outer {
        var inner = Counter::new(3i64)
        for j in inner { total = total + i * j }
    }
    total",
        ),
        9,
    );
}

#[test]
fn iterator_zero_iterations_when_immediately_none() {
    assert_program_result_i64(
        &program_with_counter(
            "    var sum = 0i64
    var iter = Counter::new(0i64)
    for x in iter { sum = sum + x + 1i64 }
    sum",
        ),
        0,
    );
}

#[test]
fn integer_range_fast_path_still_works() {
    // Regression: the existing `for i in 0..N` and `for i in 0 to N`
    // forms must continue to use the dedicated `Stmt::For` integer
    // path (no iterator desugaring).
    assert_program_result_u64(
        "fn main() -> u64 {
            var sum = 0u64
            for i in 0u64..5u64 { sum = sum + i }
            sum
        }",
        10,
    );
    assert_program_result_u64(
        "fn main() -> u64 {
            var sum = 0u64
            for i in 0u64 to 5u64 { sum = sum + i }
            sum
        }",
        10,
    );
}

// --- STDLIB-ITER: the standard collections iterate -------------------
//
// `for x in v.iter()` / `d.iter()` / `s.iter()` go through the same
// structural protocol as the user-defined `Counter` above — the
// stdlib now ships the `next(&mut self) -> Option<T>` methods the
// desugaring looks for.

#[test]
fn vec_iter_yields_each_element() {
    assert_program_result_u64(
        "fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(1u64)
            v.push(2u64)
            v.push(3u64)
            var sum = 0u64
            for x in v.iter() { sum = sum + x }
            sum
        }",
        6,
    );
}

#[test]
fn vec_iter_yields_boxed_elements_and_frees_them() {
    // Iterating `Vec<Box<i64>>`: the yielded `Box` is an alias of the
    // stored value, so reads work and every slot is freed exactly once
    // when the vec dies (DROP-GLUE).
    assert_program_result_u64(
        "fn main() -> u64 {
            var v: Vec<Box<i64>> = Vec::new()
            val b1: Box<i64> = Box::new(1i64)
            v.push(b1)
            val b2: Box<i64> = Box::new(2i64)
            v.push(b2)
            var sum = 0i64
            for x in v.iter() { sum = sum + x.get() }
            sum as u64
        }",
        3,
    );
}

#[test]
fn dict_iter_yields_key_value_tuples() {
    assert_program_result_u64(
        "fn main() -> u64 {
            var d: Dict<i64, i64> = Dict::new()
            d.insert(1i64, 10i64)
            d.insert(2i64, 20i64)
            var sum = 0i64
            for kv in d.iter() {
                val (k, v) = kv
                sum = sum + v / k
            }
            sum as u64
        }",
        20,
    );
}

#[test]
fn string_iter_yields_bytes() {
    assert_program_result_u64(
        "fn main() -> u64 {
            val s = String::from_str(\"abc\")
            var sum = 0u64
            for b in s.iter() { sum = sum + (b as u64) }
            sum
        }",
        97 + 98 + 99,
    );
}

#[test]
fn generic_tuple_payload_variant_constructs() {
    // The frontend fix behind `DictIter::next`'s `Option<(K, V)>`
    // return: `Option::Some((k, v))` inside a generic function used to
    // fail with a `Generic(K)` vs `Identifier(K)` conflict.
    assert_program_result_i64(
        "fn make<K, V>(k: K, v: V) -> Option<(K, V)> {
            Option::Some((k, v))
        }

        fn main() -> i64 {
            val o: Option<(i64, i64)> = make(3i64, 4i64)
            match o {
                Option::Some(pair) => {
                    val (a, b) = pair
                    a + b
                }
                Option::None => 0i64,
            }
        }",
        7,
    );
}

// RANGE-FOR: `for i in r` over a range value is rewritten by the
// checker into `for i in r.start..r.end`; the three-lane behaviour is
// pinned in `compiler/tests/consistency/range_values.rs`. What is left
// here is the checker's side of it.

#[test]
fn range_value_iterates_without_being_consumed() {
    let src = r#"
fn main() -> u64 {
    val r = 1u64..4u64
    var t: u64 = 0u64
    for i in r { t = t + i }
    for i in r { t = t + i }
    t
}
"#;
    assert_program_result_u64(src, 12);
}

#[test]
fn range_value_has_only_start_and_end() {
    let src = r#"
fn main() -> u64 {
    val r = 1u64..4u64
    r.len
}
"#;
    let err = crate::common::test_program(src).expect_err("`r.len` is not a field of a range");
    assert!(err.contains("len"), "{err}");
}

// MAP-OWNED-ITEM-LEAK: a `map` adapter whose closure is handed values
// that own nothing makes each item itself, so the loop's item is its
// only owner. It used to be taken for an element the container keeps
// (NEXT-ITEM-ALIAS) and leaked one `String` per iteration.
const MAP_LABEL: &str = "fn label(x: u64) -> String {
    var s = String::from_str(\"n\")
    s.push_char('0' + x as u32)
    s
}
fn three() -> Vec<u64> {
    var v: Vec<u64> = Vec::new()
    v.push(1u64)
    v.push(2u64)
    v.push(3u64)
    v
}
";

#[test]
fn a_made_item_is_freed_each_iteration() {
    assert_program_result_u64(
        &format!(
            "{MAP_LABEL}
            fn first_long(v: &Vec<u64>) -> u64 {{
                val it = v.iter()
                val m = it.map(label)
                for s in m {{ if s.len() > 1u64 {{ return s.len() }} }}
                0u64
            }}
            fn bound_one(v: &Vec<u64>) -> u64 {{
                val it = v.iter()
                var m = it.map(label)
                val one = m.next()
                match one {{ Option::Some(s) => s.len(), Option::None => 0u64 }}
            }}
            fn bytes_mapped() -> u64 {{
                val text = String::from_str(\"ab\")
                var sm = text.iter().map(fn(b: u8) -> String {{ label(b as u64 - 96u64) }})
                var n = 0u64
                while val Option::Some(s) = sm.next() {{ n = n + s.len() }}
                n
            }}
            fn main() -> u64 {{
                val v = three()
                val before = __builtin_live_bytes()
                var total = 0u64
                for s in v.iter().map(label) {{ total = total + s.len() }}
                val it = v.iter()
                val m = it.map(label)
                for s in m {{ if s.len() == 2u64 {{ break }} }}
                total = total + bound_one(&v) + first_long(&v) + bytes_mapped()
                (__builtin_live_bytes() - before) * 100u64 + total
            }}"
        ),
        // 2 bytes each: 3 in the loop, the bound one, `first_long`, 2 bytes.
        3 * 2 + 2 + 2 + 2 * 2,
    );
}

/// Handed on, the item belongs to its new owner and stays readable.
#[test]
fn a_made_item_moves_into_a_container() {
    assert_program_result_u64(
        &format!(
            "{MAP_LABEL}
            fn main() -> u64 {{
                val v = three()
                var keep: Vec<String> = Vec::new()
                for s in v.iter().map(label) {{ keep.push(s) }}
                keep.get(0u64).get(1u64) as u64 + keep.get(2u64).get(1u64) as u64
            }}"
        ),
        u64::from(b'1') + u64::from(b'3'),
    );
}

/// An item the closure may have been handed is still an element: a
/// `map` over owning values does not free what the `Vec` keeps.
#[test]
fn a_mapped_owning_element_is_still_the_containers() {
    assert_program_result_u64(
        "fn same(s: String) -> String { s }
        fn main() -> u64 {
            var w: Vec<String> = Vec::new()
            w.push(String::from_str(\"hello\"))
            w.push(String::from_str(\"hi\"))
            val before = __builtin_live_bytes()
            var n = 0u64
            for s in w.iter().map(same) { n = n + s.len() }
            (before - __builtin_live_bytes()) * 100u64 + n + w.get(0u64).get(0u64) as u64
        }",
        7 + u64::from(b'h'),
    );
}
