// PATTERN-STRUCT: `match p { Point { x: 0i64, y } => ... }`.
//
// Enum, tuple, literal, range, `@`, or-patterns and guards were all
// there; destructuring a struct was the one shape missing, so a match
// on a struct had to go through field accesses in the arm body.
//
// The compiled backends do not lower struct patterns, the same as
// tuple patterns — see `AOT_UNSUPPORTED` in
// `compiler/tests/example_consistency.rs`.

use crate::common::{assert_program_result_i64, test_program};

#[test]
fn a_field_pattern_selects_the_arm() {
    assert_program_result_i64(
        r#"
        struct Point { x: i64, y: i64 }

        fn classify(p: Point) -> i64 {
            match p {
                Point { x: 0i64, y } => y * 100i64,
                Point { x, y } => x + y,
            }
        }

        fn main() -> i64 {
            classify(Point { x: 0i64, y: 7i64 }) + classify(Point { x: 3i64, y: 4i64 })
        }
        "#,
        707,
    );
}

#[test]
fn the_shorthand_binds_the_field_to_its_own_name() {
    assert_program_result_i64(
        r#"
        struct Point { x: i64, y: i64 }

        fn main() -> i64 {
            val p = Point { x: 5i64, y: 6i64 }
            match p {
                Point { x, y } => x * y,
            }
        }
        "#,
        30,
    );
}

#[test]
fn rest_ignores_the_fields_not_named() {
    assert_program_result_i64(
        r#"
        struct Config { host: str, port: i64, debug: bool }

        fn describe(c: Config) -> i64 {
            match c {
                Config { port: 0i64, .. } => 0i64 - 1i64,
                Config { debug: true, port, .. } => port * 2i64,
                Config { port, .. } => port,
            }
        }

        fn main() -> i64 {
            describe(Config { host: "a", port: 0i64, debug: false })
                + describe(Config { host: "b", port: 10i64, debug: true })
                + describe(Config { host: "c", port: 99i64, debug: false })
        }
        "#,
        118,
    );
}

#[test]
fn a_field_left_out_without_rest_is_refused() {
    // A pattern that silently ignores what it does not mention reads
    // as complete when it is not, and a field added later would slip
    // past every existing pattern.
    let err = test_program(
        r#"
        struct Point { x: i64, y: i64 }

        fn main() -> i64 {
            val p = Point { x: 1i64, y: 2i64 }
            match p {
                Point { x } => x,
            }
        }
        "#,
    )
    .expect_err("the missing field must be reported");
    assert!(err.contains("does not mention y"), "{err}");
    assert!(err.contains("`..`"), "{err}");
}

#[test]
fn an_unknown_field_is_refused() {
    let err = test_program(
        r#"
        struct Point { x: i64, y: i64 }

        fn main() -> i64 {
            val p = Point { x: 1i64, y: 2i64 }
            match p {
                Point { x, z } => x,
            }
        }
        "#,
    )
    .expect_err("the unknown field must be reported");
    assert!(err.contains("has no field `z`"), "{err}");
}

#[test]
fn a_pattern_naming_another_struct_is_refused() {
    let err = test_program(
        r#"
        struct Point { x: i64, y: i64 }
        struct Other { x: i64, y: i64 }

        fn main() -> i64 {
            val p = Point { x: 1i64, y: 2i64 }
            match p {
                Other { x, y } => x + y,
            }
        }
        "#,
    )
    .expect_err("the struct name must agree");
    assert!(err.contains("names `Other`"), "{err}");
}

#[test]
fn a_match_whose_arms_can_all_fail_is_non_exhaustive() {
    // A struct has one shape, so an arm whose field patterns always
    // match covers everything. Without such an arm the match can fall
    // through.
    let err = test_program(
        r#"
        struct Point { x: i64, y: i64 }

        fn main() -> i64 {
            val p = Point { x: 5i64, y: 5i64 }
            match p {
                Point { x: 0i64, y: 0i64 } => 1i64,
                Point { x: 1i64, y: 1i64 } => 2i64,
            }
        }
        "#,
    )
    .expect_err("no arm covers every value");
    assert!(err.contains("non-exhaustive match on a struct"), "{err}");
}

#[test]
fn an_irrefutable_struct_pattern_is_exhaustive_on_its_own() {
    assert_program_result_i64(
        r#"
        struct Point { x: i64, y: i64 }

        fn main() -> i64 {
            val p = Point { x: 4i64, y: 5i64 }
            match p {
                Point { x, y } => x + y,
            }
        }
        "#,
        9,
    );
}

#[test]
fn a_guard_still_applies_to_a_struct_arm() {
    assert_program_result_i64(
        r#"
        struct Point { x: i64, y: i64 }

        fn main() -> i64 {
            val p = Point { x: 4i64, y: 5i64 }
            match p {
                Point { x, y } if x > y => 1i64,
                Point { x, y } => x + y,
            }
        }
        "#,
        9,
    );
}

#[test]
fn nested_patterns_work_inside_a_field() {
    // A field pattern is any pattern, including another struct
    // pattern, so the two compose.
    assert_program_result_i64(
        r#"
        struct Inner { v: i64 }
        struct Outer { inner: Inner, tag: i64 }

        fn total(o: Outer) -> i64 {
            match o {
                Outer { inner: Inner { v: 0i64 }, tag } => tag,
                Outer { inner: Inner { v }, tag } => v * tag,
            }
        }

        fn main() -> i64 {
            total(Outer { inner: Inner { v: 0i64 }, tag: 7i64 })
                + total(Outer { inner: Inner { v: 3i64 }, tag: 5i64 })
        }
        "#,
        22,
    );
}

#[test]
fn if_val_accepts_a_struct_pattern() {
    assert_program_result_i64(
        r#"
        struct Point { x: i64, y: i64 }

        fn main() -> i64 {
            val p = Point { x: 0i64, y: 9i64 }
            if val Point { x: 0i64, y } = p {
                y
            } else {
                0i64
            }
        }
        "#,
        9,
    );
}
