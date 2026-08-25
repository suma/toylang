// STRUCT-UPDATE: `P { x: 1i64, ..base }`.
//
// The parser cannot fill the omitted fields — the struct may be
// declared later in the file, or imported — so it emits
// `Expr::StructUpdate` and the type checker rewrites it once it knows
// the field list. Backends only ever see the ordinary literal, which
// is why these tests read like tests of a hand-written one.
//
// The 3-backend agreement is pinned separately, in
// `compiler/tests/consistency/compound_values.rs`.

use crate::common::{assert_program_result_i64, test_program};

#[test]
fn omitted_fields_come_from_the_base() {
    assert_program_result_i64(
        r#"
        struct P { x: i64, y: i64, z: i64 }

        fn main() -> i64 {
            val a = P { x: 1i64, y: 2i64, z: 3i64 }
            val b = P { y: 20i64, ..a }
            b.x * 100i64 + b.y * 10i64 + b.z
        }
        "#,
        // x and z from `a`, y written: 100 + 200 + 3.
        303,
    );
}

#[test]
fn a_base_only_update_copies_every_field() {
    assert_program_result_i64(
        r#"
        struct P { x: i64, y: i64 }

        fn main() -> i64 {
            val a = P { x: 4i64, y: 5i64 }
            val b = P { ..a }
            b.x * 10i64 + b.y
        }
        "#,
        45,
    );
}

#[test]
fn the_update_is_a_new_value_not_an_alias() {
    // Writing through the copy must not reach the base. A desugar that
    // aliased the base instead of building a literal would report 99
    // for both.
    assert_program_result_i64(
        r#"
        struct P { x: i64, y: i64 }

        fn main() -> i64 {
            var a: P = P { x: 1i64, y: 2i64 }
            var b: P = P { y: 9i64, ..a }
            b.x = 99i64
            a.x * 100i64 + b.x
        }
        "#,
        // `a.x` untouched at 1, `b.x` overwritten to 99.
        199,
    );
}

#[test]
fn the_base_can_be_a_field_path() {
    assert_program_result_i64(
        r#"
        struct Inner { a: i64, b: i64 }
        struct Outer { i: Inner, n: i64 }

        fn main() -> i64 {
            val o = Outer { i: Inner { a: 1i64, b: 2i64 }, n: 3i64 }
            val u = Inner { a: 40i64, ..o.i }
            u.a + u.b
        }
        "#,
        42,
    );
}

#[test]
fn a_struct_typed_field_is_carried_over() {
    assert_program_result_i64(
        r#"
        struct Inner { a: i64, b: i64 }
        struct Outer { i: Inner, n: i64 }

        fn main() -> i64 {
            val o = Outer { i: Inner { a: 1i64, b: 2i64 }, n: 3i64 }
            val u = Outer { n: 30i64, ..o }
            u.i.a + u.i.b + u.n
        }
        "#,
        33,
    );
}

#[test]
fn self_can_be_the_base_inside_a_method() {
    // The tail expression of a body reaches the type checker through
    // `check_expr_located`, not `visit_expr` — the route where an
    // un-intercepted struct update would have reached the backends
    // undesugared and died with "unexpected expr".
    assert_program_result_i64(
        r#"
        struct P { x: i64, y: i64 }

        impl P {
            fn with_x(&self, nx: i64) -> P {
                P { x: nx, ..self }
            }
        }

        fn main() -> i64 {
            val a = P { x: 1i64, y: 2i64 }
            val b = a.with_x(7i64)
            b.x * 10i64 + b.y
        }
        "#,
        72,
    );
}

#[test]
fn a_generic_struct_keeps_its_type_argument() {
    assert_program_result_i64(
        r#"
        struct Wrap<T> { v: T, n: i64 }

        fn main() -> i64 {
            val w: Wrap<i64> = Wrap { v: 5i64, n: 1i64 }
            val u: Wrap<i64> = Wrap { n: 2i64, ..w }
            u.v * 10i64 + u.n
        }
        "#,
        52,
    );
}

#[test]
fn a_tuple_struct_can_be_copied_from_a_base() {
    // Tuple structs desugar to fields named "0", "1", ..., which the
    // update fills like any other name. Writing one explicitly is a
    // different matter — a literal's field name has to lex as an
    // identifier, so `Pair { 0: ..., ..p }` is not sayable and this
    // form only copies.
    assert_program_result_i64(
        r#"
        struct Pair(i64, i64)

        fn main() -> i64 {
            val p = Pair(3i64, 4i64)
            val q = Pair { ..p }
            q.0 * 10i64 + q.1
        }
        "#,
        34,
    );
}

#[test]
fn a_side_effecting_base_is_evaluated_once() {
    // The non-path base keeps its `val __su_N` temporary, so `bump()`
    // runs once even though it fills two fields.
    assert_program_result_i64(
        r#"
        struct P { x: i64, y: i64, z: i64 }

        fn bump() -> P {
            println("called")
            P { x: 1i64, y: 2i64, z: 3i64 }
        }

        fn main() -> i64 {
            val u = P { x: 10i64, ..bump() }
            u.x + u.y + u.z
        }
        "#,
        15,
    );
}

#[test]
fn a_base_of_another_struct_type_is_rejected() {
    let err = test_program(
        r#"
        struct P { x: i64, y: i64 }
        struct Q { x: i64 }

        fn main() -> i64 {
            val q = Q { x: 1i64 }
            val p = P { y: 2i64, ..q }
            p.x
        }
        "#,
    )
    .expect_err("a `Q` base for a `P` literal must not type check");
    assert!(
        err.contains("expected P") && err.contains("got Q"),
        "the error should name both structs, got: {err}"
    );
}

#[test]
fn a_field_written_twice_over_is_the_written_one() {
    // A name that appears explicitly is never filled from the base,
    // whichever order the declaration lists it in.
    assert_program_result_i64(
        r#"
        struct P { x: i64, y: i64, z: i64 }

        fn main() -> i64 {
            val a = P { x: 1i64, y: 2i64, z: 3i64 }
            val b = P { z: 30i64, x: 10i64, ..a }
            b.x * 100i64 + b.y * 10i64 + b.z
        }
        "#,
        10 * 100 + 2 * 10 + 30,
    );
}

#[test]
fn the_base_must_be_last() {
    let err = test_program(
        r#"
        struct P { x: i64, y: i64 }

        fn main() -> i64 {
            val a = P { x: 1i64, y: 2i64 }
            val b = P { ..a, x: 5i64 }
            b.x
        }
        "#,
    )
    .expect_err("`..base` before a field must be a parse error");
    assert!(
        err.contains("must be the last item"),
        "unexpected error: {err}"
    );
}

#[test]
fn an_unknown_written_field_names_itself() {
    let err = test_program(
        r#"
        struct P { x: i64, y: i64 }

        fn main() -> i64 {
            val a = P { x: 1i64, y: 2i64 }
            val b = P { z: 9i64, ..a }
            b.x
        }
        "#,
    )
    .expect_err("`z` is not a field of `P`");
    assert!(
        err.contains("Unknown field 'z' in struct 'P'"),
        "the error should spell both names, got: {err}"
    );
}
