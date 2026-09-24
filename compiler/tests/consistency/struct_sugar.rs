//! STRUCT-SUGAR-GAP: the struct forms a pattern had and construction
//! and binding lacked -- the field shorthand `P { x, y }` and the
//! destructuring `val P { x, y } = e`. Both are parser desugarings, so
//! the lanes see a plain struct literal and plain field reads.

use super::harness::*;

/// Shorthand, destructuring with renames / `..` / nesting, `var`, and a
/// struct inside a tuple pattern. The owned `String` field is freed
/// once.
#[test]
fn struct_shorthand_and_destructuring() {
    let src = r#"
        struct P { x: i64, y: i64 }
        struct U { name: String, id: u64, at: P }
        fn mk() -> U { U { name: String::from_str("ann"), id: 5u64, at: P { x: 3i64, y: 4i64 } } }
        fn main() -> u64 {
            val x = 1i64
            val y = 2i64
            val p = P { x, y }
            val q = P {
                x,
                y: 9i64,
            }
            val P { x: a, y } = q
            val U { name, at: P { x: ax, .. }, .. } = mk()
            var (P { x: m, .. }, k) = (p, 4u64)
            m = m + 10i64
            k = k + 1u64
            println("{p.x} {p.y} {a} {y} {name} {ax} {m} {k}")
            0u64
        }
    "#;
    assert_renders(src, "struct_sugar", "1 2 1 9 ann 3 11 5\n");
    memory_profiles_agree(src, "struct_sugar_mem");
}

/// A destructuring is checked like a `match` arm: the struct's name,
/// and every field unless `..`.
#[test]
fn a_struct_destructuring_is_checked_like_a_pattern() {
    let src = r#"
        struct P { x: i64, y: i64 }
        struct Z { x: i64, y: i64 }
        fn main() -> u64 {
            val p = P { x: 1i64, y: 2i64 }
            val P { x } = p
            val Z { x: b, .. } = p
            0u64
        }
    "#;
    let errors = type_check_errors(src).join("\n");
    assert!(errors.contains("does not mention y"), "{errors}");
    assert!(errors.contains("struct pattern names `Z`, but the value is `P`"), "{errors}");
}
