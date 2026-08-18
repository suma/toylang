// STDLIB-ORD: the `Ord` trait (`core/std/ord.t`) and `Vec<T>::sort`
// (`core/std/collections/vec.t`). Interpreter-side behaviour tests;
// the 3-backend agreement is pinned in
// `compiler/tests/consistency.rs` (`vec_sort_is_consistent_across_backends`).


use crate::common::{assert_program_result_u64, test_program};

#[test]
fn vec_sort_orders_u64_ascending() {
    assert_program_result_u64(
        r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(5u64)
            v.push(1u64)
            v.push(4u64)
            v.push(2u64)
            v.push(3u64)
            v.sort()
            val a: u64 = v.get(0u64)
            val b: u64 = v.get(1u64)
            val c: u64 = v.get(2u64)
            val d: u64 = v.get(3u64)
            val e: u64 = v.get(4u64)
            if a == 1u64 && b == 2u64 && c == 3u64 && d == 4u64 && e == 5u64 {
                1u64
            } else {
                0u64
            }
        }
        "#,
        1,
    );
}

#[test]
fn vec_sort_orders_i64_with_negatives() {
    assert_program_result_u64(
        r#"
        fn main() -> u64 {
            var v: Vec<i64> = Vec::new()
            v.push(3i64)
            v.push(-5i64)
            v.push(0i64)
            v.push(-2i64)
            v.push(7i64)
            v.sort()
            val a: i64 = v.get(0u64)
            val b: i64 = v.get(1u64)
            val c: i64 = v.get(2u64)
            val d: i64 = v.get(3u64)
            val e: i64 = v.get(4u64)
            if a == -5i64 && b == -2i64 && c == 0i64 && d == 3i64 && e == 7i64 {
                1u64
            } else {
                0u64
            }
        }
        "#,
        1,
    );
}

#[test]
fn vec_sort_orders_f64() {
    assert_program_result_u64(
        r#"
        fn main() -> u64 {
            var v: Vec<f64> = Vec::new()
            v.push(2.5f64)
            v.push(1.0f64)
            v.push(3.75f64)
            v.push(0.5f64)
            v.sort()
            val a: f64 = v.get(0u64)
            val b: f64 = v.get(1u64)
            val c: f64 = v.get(2u64)
            val d: f64 = v.get(3u64)
            if a == 0.5f64 && b == 1.0f64 && c == 2.5f64 && d == 3.75f64 {
                1u64
            } else {
                0u64
            }
        }
        "#,
        1,
    );
}

#[test]
fn vec_sort_orders_bool_false_first() {
    assert_program_result_u64(
        r#"
        fn main() -> u64 {
            var v: Vec<bool> = Vec::new()
            v.push(true)
            v.push(false)
            v.push(true)
            v.sort()
            val a: bool = v.get(0u64)
            val b: bool = v.get(1u64)
            val c: bool = v.get(2u64)
            if !a && b && c { 1u64 } else { 0u64 }
        }
        "#,
        1,
    );
}

#[test]
fn vec_sort_orders_strings_bytewise() {
    assert_program_result_u64(
        r#"
        fn main() -> u64 {
            var v: Vec<String> = Vec::new()
            val a: String = String::from_str("pear")
            v.push(a)
            val b: String = String::from_str("apple")
            v.push(b)
            val c: String = String::from_str("fig")
            v.push(c)
            val d: String = String::from_str("banana")
            v.push(d)
            v.sort()
            val r0: String = v.get(0u64)
            val r1: String = v.get(1u64)
            val r2: String = v.get(2u64)
            val r3: String = v.get(3u64)
            val want0: String = String::from_str("apple")
            val want1: String = String::from_str("banana")
            val want2: String = String::from_str("fig")
            val want3: String = String::from_str("pear")
            if r0 == want0 && r1 == want1 && r2 == want2 && r3 == want3 {
                1u64
            } else {
                0u64
            }
        }
        "#,
        1,
    );
}

#[test]
fn vec_sort_orders_user_struct_with_impl_ord() {
    assert_program_result_u64(
        r#"
        struct Pt { x: i64, y: i64 }
        impl Ord for Pt {
            fn lt(self: Self, other: Self) -> bool {
                if self.x != other.x { self.x < other.x } else { self.y < other.y }
            }
        }
        fn main() -> u64 {
            var v: Vec<Pt> = Vec::new()
            val p1: Pt = Pt { x: 2i64, y: 9i64 }
            v.push(p1)
            val p2: Pt = Pt { x: 1i64, y: 5i64 }
            v.push(p2)
            val p3: Pt = Pt { x: 1i64, y: 3i64 }
            v.push(p3)
            val p4: Pt = Pt { x: 3i64, y: 1i64 }
            v.push(p4)
            v.sort()
            val a: Pt = v.get(0u64)
            val b: Pt = v.get(1u64)
            val c: Pt = v.get(2u64)
            val d: Pt = v.get(3u64)
            if a.x == 1i64 && a.y == 3i64
                && b.x == 1i64 && b.y == 5i64
                && c.x == 2i64 && c.y == 9i64
                && d.x == 3i64 && d.y == 1i64 {
                1u64
            } else {
                0u64
            }
        }
        "#,
        1,
    );
}

#[test]
fn empty_and_single_element_sorts_are_noops() {
    assert_program_result_u64(
        r#"
        fn main() -> u64 {
            var empty: Vec<u64> = Vec::new()
            empty.sort()
            var one: Vec<u64> = Vec::new()
            one.push(7u64)
            one.sort()
            val a: u64 = one.get(0u64)
            if empty.size() == 0u64 && a == 7u64 { 1u64 } else { 0u64 }
        }
        "#,
        1,
    );
}

#[test]
fn impl_ord_provides_the_lt_operator() {
    // `impl Ord` registers `lt`, which the `<` operator dispatch finds
    // by name — the same method serves both sort and `<`. (`>` / `<=`
    // / `>=` need their own `gt` / `le` / `ge` methods, per the
    // operator-overload convention.)
    assert_program_result_u64(
        r#"
        struct N { v: i64 }
        impl Ord for N {
            fn lt(self: Self, other: Self) -> bool {
                self.v < other.v
            }
        }
        fn main() -> u64 {
            val a: N = N { v: 1i64 }
            val b: N = N { v: 2i64 }
            if a < b && !(b < a) && !(a < a) { 1u64 } else { 0u64 }
        }
        "#,
        1,
    );
}

#[test]
fn generic_function_with_ord_bound_dispatches_lt() {
    assert_program_result_u64(
        r#"
        fn min<T: Ord>(a: T, b: T) -> T {
            if a.lt(b) { a } else { b }
        }
        fn main() -> u64 {
            val m: u64 = min(5u64, 3u64)
            val mi: i64 = min(-1i64, -4i64)
            if m == 3u64 && mi == -4i64 { 1u64 } else { 0u64 }
        }
        "#,
        1,
    );
}

#[test]
fn ord_impls_cover_all_primitive_widths() {
    assert_program_result_u64(
        r#"
        fn main() -> u64 {
            val u8a: u8 = 1u8
            val u8b: u8 = 2u8
            val u16a: u16 = 1u16
            val u16b: u16 = 2u16
            val u32a: u32 = 1u32
            val u32b: u32 = 2u32
            val i8a: i8 = -1i8
            val i8b: i8 = 1i8
            val i16a: i16 = -1i16
            val i16b: i16 = 1i16
            val i32a: i32 = -1i32
            val i32b: i32 = 1i32
            if u8a.lt(u8b) && u16a.lt(u16b) && u32a.lt(u32b)
                && i8a.lt(i8b) && i16a.lt(i16b) && i32a.lt(i32b)
                && (!u8b.lt(u8a)) {
                1u64
            } else {
                0u64
            }
        }
        "#,
        1,
    );
}

// STDLIB-ORD: `impl<T: Ord> Vec<T>` bounds are enforced at the call
// site. Before this, dispatch simply failed later — at run time in
// the interpreter ("Method 'lt' not found"), at compile time in AOT.

#[test]
fn vec_sort_on_non_ord_element_is_a_type_error() {
    let err = test_program(
        r#"
        struct P {
            x: i64
        }
        fn main() -> u64 {
            var v: Vec<P> = Vec::new()
            v.push(P { x: 3i64 })
            v.sort()
            0u64
        }
        "#,
    )
    .expect_err("sorting a Vec of a type without `impl Ord` must not type-check");
    assert!(
        err.contains("bound violation") && err.contains("Ord") && err.contains("sort"),
        "error should name the violated `Ord` bound on `sort`, got: {err}"
    );
}

#[test]
fn vec_sort_in_unbounded_generic_is_a_type_error() {
    // The caller's own `T` carries no bound, so it cannot satisfy
    // `impl<T: Ord>` — the same rule Rust applies.
    let err = test_program(
        r#"
        fn sorted_size<T>(v: Vec<T>) -> u64 {
            v.sort()
            v.size()
        }
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(1u64)
            sorted_size(v)
        }
        "#,
    )
    .expect_err("`v.sort()` under an unbounded `T` must not type-check");
    assert!(
        err.contains("bound violation") && err.contains("Ord"),
        "error should name the violated `Ord` bound, got: {err}"
    );
}

#[test]
fn vec_sort_in_ord_bounded_generic_is_accepted() {
    // Pass-through: the caller declares the same bound, so the
    // receiver satisfies it without being concrete.
    assert_program_result_u64(
        r#"
        fn sorted_size<T: Ord>(v: Vec<T>) -> u64 {
            v.sort()
            v.size()
        }
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(5u64)
            v.push(2u64)
            sorted_size(v)
        }
        "#,
        2,
    );
}

#[test]
fn user_struct_with_impl_ord_still_sorts() {
    // The negative tests above must not have made the positive
    // case stricter: a struct that *does* implement `Ord` passes
    // the same call-site check.
    assert_program_result_u64(
        r#"
        struct Item {
            key: u64
        }
        impl Ord for Item {
            fn lt(self: Self, other: Self) -> bool {
                self.key < other.key
            }
        }
        fn main() -> u64 {
            var v: Vec<Item> = Vec::new()
            v.push(Item { key: 3u64 })
            v.push(Item { key: 1u64 })
            v.sort()
            val first: Item = v.get(0u64)
            first.key
        }
        "#,
        1,
    );
}

#[test]
fn enum_impl_bound_is_enforced_on_the_receiver() {
    // The same rule on a generic *enum* receiver: `impl<T: Ord>
    // Holder<T>` is only reachable for payload types with `Ord`.
    let program = |payload: &str, value: &str| {
        format!(
            r#"
            struct P {{
                x: i64
            }}
            enum Holder<T> {{
                Val(T),
                Empty,
            }}
            impl<T: Ord> Holder<T> {{
                fn is_empty(self: Self) -> bool {{
                    match self {{
                        Holder::Val(_) => false,
                        Holder::Empty => true,
                    }}
                }}
            }}
            fn main() -> u64 {{
                val h: Holder<{payload}> = Holder::Val({value})
                if h.is_empty() {{ 1u64 }} else {{ 0u64 }}
            }}
            "#
        )
    };
    let err = test_program(&program("P", "P { x: 1i64 }"))
        .expect_err("`Holder<P>` must not reach an `impl<T: Ord>` method");
    assert!(
        err.contains("bound violation") && err.contains("Ord"),
        "error should name the violated `Ord` bound, got: {err}"
    );
    assert_program_result_u64(&program("u64", "7u64"), 0);
}
