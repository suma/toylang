//! Enum construction written straight into an argument.
//!
//! ENUM-VARIANT-ARG / ENUM-ARG-NEST. Before this, `take(Color::Red)`
//! and `area(Shape::Circle(3i64))` were both "compiler MVP cannot
//! lower expression yet" on the compiled lanes while the tree-walker
//! ran them, so every `Option`-taking API forced a `val` binding at
//! each call site. The unit form parses as a `QualifiedIdentifier`
//! and the tuple form as an `AssociatedFunctionCall`; neither had a
//! home in argument position, only on a `val` RHS.
//!
//! The generic case is the reason this matters beyond convenience:
//! `take(Option::None)` carries nothing to infer `T` from, so the
//! instantiation has to come from the callee's declared parameter
//! type.

use super::harness::*;

/// The unit form (`QualifiedIdentifier`) in argument position.
#[test]
fn a_unit_variant_is_an_argument() {
    let src = r#"
        enum Color { Red, Green, Blue }

        fn rank(c: Color) -> u64 {
            match c {
                Color::Red => 1u64,
                Color::Green => 2u64,
                Color::Blue => 4u64,
            }
        }

        fn main() -> u64 {
            rank(Color::Red) + rank(Color::Green) + rank(Color::Blue)
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 7);
    assert_consistent(src, "enum_arg_unit_variant");
}

/// The tuple form (`AssociatedFunctionCall`) in argument position,
/// mixed with the unit form so both arms of the match are reached.
#[test]
fn a_tuple_variant_is_an_argument() {
    let src = r#"
        enum Shape { Circle(i64), Rect(i64, i64), Point }

        fn area(s: Shape) -> i64 {
            match s {
                Shape::Circle(r) => r * r,
                Shape::Rect(w, h) => w * h,
                Shape::Point => 0i64,
            }
        }

        fn main() -> i64 {
            area(Shape::Circle(3i64)) + area(Shape::Rect(2i64, 5i64)) + area(Shape::Point)
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 19);
    assert_consistent(src, "enum_arg_tuple_variant");
}

/// The case the item was filed for: a generic enum whose type
/// argument only the parameter slot knows. `Option::None` says
/// nothing about `T`.
#[test]
fn a_generic_unit_variant_takes_its_instantiation_from_the_slot() {
    let src = r#"
        fn or_else(o: Option<i64>, d: i64) -> i64 {
            match o {
                Option::Some(v) => v,
                Option::None => d,
            }
        }

        fn main() -> i64 {
            or_else(Option::None, 7i64) + or_else(Option::Some(3i64), 0i64)
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 10);
    assert_consistent(src, "enum_arg_generic_unit_variant");
}

/// An enum argument to a *method*, and to an associated function —
/// the two call shapes that lower their arguments through a
/// different entry point than a free call.
#[test]
fn an_enum_argument_reaches_methods_and_associated_functions() {
    let src = r#"
        enum Op { Add(u64), Nop }

        struct Acc { total: u64 }

        impl Acc {
            fn start() -> Acc { Acc { total: 0u64 } }

            fn apply(&self, op: Op) -> Acc {
                match op {
                    Op::Add(n) => Acc { total: self.total + n },
                    Op::Nop => Acc { total: self.total },
                }
            }

            fn of(op: Op) -> Acc {
                val zero = Acc { total: 0u64 }
                val out = zero.apply(op)
                out
            }
        }

        fn main() -> u64 {
            val a = Acc::of(Op::Add(5u64))
            val b = a.apply(Op::Add(3u64))
            val c = b.apply(Op::Nop)
            c.total
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 8);
    assert_consistent(src, "enum_arg_method_and_assoc");
}

/// Nested: an enum construction as the payload of another enum
/// construction, itself in argument position.
#[test]
fn an_enum_argument_nests() {
    let src = r#"
        fn depth(o: Option<Option<u64>>) -> u64 {
            match o {
                Option::Some(inner) => {
                    match inner {
                        Option::Some(v) => v,
                        Option::None => 1u64,
                    }
                }
                Option::None => 100u64,
            }
        }

        fn main() -> u64 {
            depth(Option::Some(Option::Some(9u64))) + depth(Option::Some(Option::None))
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 10);
    assert_consistent(src, "enum_arg_nested");
}
