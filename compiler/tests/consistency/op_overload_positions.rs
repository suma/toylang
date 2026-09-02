//! OP-OVERLOAD-CHAIN: an overloaded operator outside a `val`'s rhs.
//!
//! The interpreter accepted every position; the compiled lanes
//! accepted exactly one. Measured on 2026-08-29 with `struct V` and
//! `fn add(&self, other: &V) -> V`:
//!
//! | written as | interpreter | AOT / JIT |
//! |---|---|---|
//! | `val r: V = a + b` | ok | ok |
//! | `val r: V = a + b + c` | ok | `arith lhs must be a bare identifier (MVP)` |
//! | `val r: V = a + V { .. }` | ok | `arith rhs must be a bare identifier (MVP)` |
//! | `(a + b).x` | ok | `field-access chains rooted at a bare identifier` |
//! | `take(a + b)` | ok | `binary lhs produced no value` |
//! | `if (a + b) == c` | ok | `binary lhs produced no value` |
//!
//! One cause under all five: the operator's result is a struct, and a
//! struct lives in leaf locals rather than in an SSA value. The only
//! position that allocated those locals was a `val` RHS, so every
//! other one had nowhere to put the answer — and each site reported
//! that in its own vocabulary, none of which named the rule.
//!
//! The operands were the other half. Requiring a bare identifier on
//! each side is what made a chain and a literal operand fail; routing
//! them through the ordinary compound-argument path accepts a
//! binding, a literal, a call, and another overloaded operator, which
//! is what a chain is.

use super::harness::*;

/// The chain and the literal operand — the two the documentation
/// listed as deliberately out of scope.
#[test]
fn operators_chain_and_take_literal_operands() {
    let src = r#"
        struct V { x: i64 }

        impl V {
            fn add(&self, other: &V) -> V { V { x: self.x + other.x } }
            fn neg(&self) -> V { V { x: 0i64 - self.x } }
        }

        fn main() -> i64 {
            val a = V { x: 1i64 }
            val b = V { x: 2i64 }
            val c = V { x: 4i64 }
            val chain: V = a + b + c
            val literal: V = a + V { x: 8i64 }
            val negated: V = -(a + b)
            chain.x + literal.x + negated.x
        }
    "#;
    // 7 + 9 + (-3).
    assert_eq!(interpreter_value(src) & 0xff, 13);
    assert_consistent(src, "op_overload_chain_and_literal");
}

/// The result's field, and an argument slot — the two positions that
/// used to answer with a sentence about bare identifiers.
#[test]
fn an_operator_result_is_a_field_root_and_an_argument() {
    let src = r#"
        struct V { x: i64 }

        impl V {
            fn add(&self, other: &V) -> V { V { x: self.x + other.x } }
        }

        fn take(v: V) -> i64 { v.x }

        fn main() -> i64 {
            val a = V { x: 1i64 }
            val b = V { x: 2i64 }
            val c = V { x: 4i64 }
            (a + b).x + take(a + b) + (a + b + c).x + take(a + b + c)
        }
    "#;
    // 3 + 3 + 7 + 7.
    assert_eq!(interpreter_value(src) & 0xff, 20);
    assert_consistent(src, "op_overload_field_and_arg");
}

/// A condition — the `==` overload with operands that are themselves
/// overloaded operators.
#[test]
fn an_operator_result_is_a_condition() {
    let src = r#"
        struct V { x: i64 }

        impl V {
            fn add(&self, other: &V) -> V { V { x: self.x + other.x } }
            fn eq(&self, other: &V) -> bool { self.x == other.x }
        }

        fn main() -> i64 {
            val a = V { x: 1i64 }
            val b = V { x: 2i64 }
            val three = V { x: 3i64 }
            var acc: i64 = 0i64
            if (a + b) == three { acc = acc + 1i64 }
            if (a + b) == a { acc = acc + 10i64 }
            if (a + b) == (a + b) { acc = acc + 100i64 }
            acc
        }
    "#;
    // The first and third hold; the second does not.
    assert_eq!(interpreter_value(src) & 0xff, 101);
    assert_consistent(src, "op_overload_condition");
}

/// A struct with more than one leaf, so the operand flattening has to
/// carry every field in declaration order rather than getting away
/// with a single value.
#[test]
fn a_multi_field_struct_chains_correctly() {
    let src = r#"
        struct P { x: i64, y: i64 }

        impl P {
            fn add(&self, other: &P) -> P {
                P { x: self.x + other.x, y: self.y + other.y }
            }
            fn sub(&self, other: &P) -> P {
                P { x: self.x - other.x, y: self.y - other.y }
            }
        }

        fn weigh(p: P) -> i64 { p.x * 10i64 + p.y }

        fn main() -> i64 {
            val a = P { x: 1i64, y: 2i64 }
            val b = P { x: 3i64, y: 4i64 }
            val c = P { x: 1i64, y: 1i64 }
            weigh(a + b - c)
        }
    "#;
    // (1+3-1, 2+4-1) = (3, 5) -> 35.
    assert_eq!(interpreter_value(src) & 0xff, 35);
    assert_consistent(src, "op_overload_multi_field");
}
