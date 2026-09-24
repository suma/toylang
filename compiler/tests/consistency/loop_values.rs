//! BREAK-WITH-VALUE: `break <value>` makes a `loop` an expression.
//!
//! The parser desugars a value loop into a hidden `var` of
//! `Option<T>`, a `while true`, and a `match` that takes the value out;
//! the type checker gives the `var` its type from the first `break`
//! that names one. So the lanes see only constructs they already run,
//! and what these tests pin is that the desugaring means the same thing
//! on each of them.

use super::harness::*;

/// The shapes a value loop comes in: bound with `val`, as a function's
/// tail, a struct value, a suffix-less literal taking the annotation,
/// and a `break @label v` out of an inner `while`.
#[test]
fn a_loop_is_the_value_its_break_carries() {
    let src = r#"
        struct P { x: i64, y: i64 }
        fn first_even(v: &Vec<u64>) -> Option<u64> {
            var i = 0u64
            loop {
                if i == v.size() { break Option::None }
                val x = v.get(i)
                if x % 2u64 == 0u64 { break Option::Some(x) }
                i = i + 1u64
            }
        }
        fn find_idx(n: u64) -> u64 {
            var i = 0u64
            val k = loop {
                i = i + 1u64
                if i * i > n { break i }
            }
            k
        }
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(3u64)
            v.push(8u64)
            val fe = first_even(&v) ?? 99u64
            val idx = find_idx(10u64)
            val signed: i64 = loop { break -5 }
            var j = 0i64
            val p = loop {
                j = j + 1i64
                if j == 3i64 { break P { x: j, y: j * 2i64 } }
            }
            var a = 0u64
            val s = @outer: loop {
                var b = 0u64
                while b < 10u64 {
                    b = b + 1u64
                    if a * b == 12u64 { break @outer String::from_str("found") }
                }
                a = a + 1u64
            }
            println("{fe} {idx} {signed} {p.x} {p.y} {s} {a}")
            0u64
        }
    "#;
    assert_renders(src, "loop_values", "8 4 -5 3 6 found 2\n");
}

/// An owned value leaves the loop with its owner: allocated twice,
/// freed twice, on every lane.
#[test]
fn an_owned_loop_value_is_freed_once() {
    let src = r#"
        fn pick(n: u64) -> String {
            var i = 0u64
            loop {
                i = i + 1u64
                if i == n { break String::from_str("hit") }
            }
        }
        fn main() -> u64 {
            var i = 0u64
            val s = loop {
                i = i + 1u64
                if i == 2u64 { break String::from_str("two") }
            }
            val t = pick(3u64)
            println("{s} {t}")
            0u64
        }
    "#;
    assert_renders(src, "loop_value_owned", "two hit\n");
    memory_profiles_agree(src, "loop_value_owned_mem");
}

/// A `break` whose value names no type (`Option::None`) leaves the
/// question to the annotation.
#[test]
fn an_annotation_types_a_loop_whose_breaks_do_not() {
    let src = r#"
        fn main() -> u64 {
            val a: Option<u64> = loop { break Option::None }
            a ?? 7u64
        }
    "#;
    assert_consistent(src, "loop_value_annotated");
    let errors = type_check_errors(
        r#"
        fn main() -> u64 {
            val a = loop { break Option::None }
            0u64
        }
    "#,
    );
    assert!(
        errors.iter().any(|e| e.contains("cannot tell the type of this `loop`'s value")),
        "{errors:?}"
    );
}
