// DBC-TRAIT-INHERIT: a contract declared on a `trait` method reaches
// the `impl` that provides the method.
//
// Before this, a `requires` on a trait method did nothing unless the
// impl happened to repeat it verbatim — the clause read as an
// obligation the trait imposed and was in fact inert. The exception
// was a method with a default body, which inherited its clauses
// through `synthesize_default_method`; the two paths now agree.
//
// Backend agreement is pinned in `compiler/tests/consistency.rs`.

use crate::common::{assert_program_result_u64, test_program};

#[test]
fn a_trait_precondition_applies_to_an_impl_that_omits_it() {
    let err = test_program(
        r#"
        trait Shrink {
            fn shrink(self: Self, by: u64) -> u64
                requires by > 0u64
        }

        struct B { n: u64 }

        impl Shrink for B {
            fn shrink(self: Self, by: u64) -> u64 {
                self.n - by
            }
        }

        fn main() -> u64 {
            val b = B { n: 10u64 }
            b.shrink(0u64)
        }
        "#,
    )
    .expect_err("the trait's precondition must apply");
    assert!(err.contains("Contract violation"), "{err}");
    assert!(err.contains("requires"), "{err}");
}

#[test]
fn a_trait_postcondition_applies_to_an_impl_that_omits_it() {
    let err = test_program(
        r#"
        trait Grow {
            fn grow(self: Self, by: u64) -> u64
                ensures result > 0u64
        }

        struct G { n: u64 }

        impl Grow for G {
            fn grow(self: Self, by: u64) -> u64 {
                self.n * by
            }
        }

        fn main() -> u64 {
            val g = G { n: 5u64 }
            g.grow(0u64)
        }
        "#,
    )
    .expect_err("the trait's postcondition must apply");
    assert!(err.contains("Contract violation"), "{err}");
    assert!(err.contains("ensures"), "{err}");
}

/// DBC-LISKOV. A precondition is what callers are told to satisfy, and
/// a caller holding `&dyn Shrink` can read the trait's clauses and
/// nothing else. An impl that adds one of its own breaks calls that
/// were written correctly, so it is refused.
#[test]
fn an_impl_may_not_strengthen_the_precondition() {
    let err = test_program(
        r#"
        trait Shrink {
            fn shrink(&self, by: u64) -> u64
                requires by > 0u64
        }

        struct B { n: u64 }

        impl Shrink for B {
            fn shrink(&self, by: u64) -> u64
                requires by < 100u64
            {
                self.n - by
            }
        }

        fn use_it(s: &dyn Shrink) -> u64 { s.shrink(200u64) }

        fn main() -> u64 {
            val b = B { n: 1000u64 }
            use_it(&b)
        }
        "#,
    )
    .expect_err("the added precondition must be refused");
    assert!(err.contains("E0023"), "{err}");
    assert!(err.contains("`shrink`"), "{err}");
    // The impl is still registered as implementing the trait, so the
    // `&dyn` coercion does not fail on top of it: one error, not two.
    assert!(!err.contains("E0001"), "{err}");
}

/// The same rule with no contract on the trait at all — the case that
/// actually occurs. A trait that says nothing lets callers pass
/// anything the types allow, so any clause the impl adds is stronger
/// than that.
#[test]
fn an_impl_may_not_add_a_precondition_where_the_trait_has_none() {
    let err = test_program(
        r#"
        trait Shrink {
            fn shrink(&self, by: u64) -> u64
        }

        struct B { n: u64 }

        impl Shrink for B {
            fn shrink(&self, by: u64) -> u64
                requires by < 100u64
            {
                self.n - by
            }
        }

        fn main() -> u64 {
            val b = B { n: 1000u64 }
            b.shrink(1u64)
        }
        "#,
    )
    .expect_err("a precondition on an uncontracted trait method must be refused");
    assert!(err.contains("E0023"), "{err}");
    assert!(err.contains("asks for nothing"), "{err}");
}

/// The other direction stays open: promising *more* than the trait did
/// breaks nobody, so an impl may add `ensures`. Both sets are checked,
/// the trait's first.
#[test]
fn an_impl_may_strengthen_the_postcondition() {
    assert_program_result_u64(
        r#"
        trait Grow {
            fn grow(self: Self, by: u64) -> u64
                ensures result > 0u64
        }

        struct G { n: u64 }

        impl Grow for G {
            fn grow(self: Self, by: u64) -> u64
                ensures result < 100u64
            {
                self.n * by
            }
        }

        fn main() -> u64 {
            val g = G { n: 6u64 }
            g.grow(7u64)
        }
        "#,
        42,
    );
}

/// ...and the impl's own postcondition is really checked, after the
/// trait's.
#[test]
fn an_added_postcondition_is_checked_too() {
    let err = test_program(
        r#"
        trait Grow {
            fn grow(self: Self, by: u64) -> u64
                ensures result > 0u64
        }

        struct G { n: u64 }

        impl Grow for G {
            fn grow(self: Self, by: u64) -> u64
                ensures result < 100u64
            {
                self.n * by
            }
        }

        fn main() -> u64 {
            val g = G { n: 50u64 }
            g.grow(7u64)
        }
        "#,
    )
    .expect_err("the impl's own postcondition must fire");
    assert!(err.contains("Contract violation"), "{err}");
    assert!(err.contains("clause #2"), "{err}");
}

#[test]
fn a_satisfied_contract_leaves_the_call_alone() {
    assert_program_result_u64(
        r#"
        trait Shrink {
            fn shrink(self: Self, by: u64) -> u64
                requires by > 0u64
                ensures result < 1000u64
        }

        struct B { n: u64 }

        impl Shrink for B {
            fn shrink(self: Self, by: u64) -> u64 {
                self.n - by
            }
        }

        fn main() -> u64 {
            val b = B { n: 50u64 }
            b.shrink(8u64)
        }
        "#,
        42,
    );
}

#[test]
fn renaming_a_parameter_under_a_contracted_trait_is_refused() {
    // A clause is an expression over parameter names, so it cannot be
    // carried onto an impl that spells them differently. Silently
    // dropping the contract is what this whole feature exists to stop,
    // so the rename is refused instead.
    let err = test_program(
        r#"
        trait Grow {
            fn grow(self: Self, by: u64) -> u64
                requires by > 0u64
        }

        struct G { n: u64 }

        impl Grow for G {
            fn grow(self: Self, amount: u64) -> u64 {
                self.n * amount
            }
        }

        fn main() -> u64 {
            val g = G { n: 5u64 }
            g.grow(1u64)
        }
        "#,
    )
    .expect_err("the rename must be refused");
    assert!(err.contains("renames parameter"), "{err}");
    assert!(err.contains("`by`"), "{err}");
}

#[test]
fn an_uncontracted_trait_still_allows_renaming() {
    // The rule is scoped to traits that actually declare a contract —
    // renaming a parameter is otherwise perfectly ordinary.
    assert_program_result_u64(
        r#"
        trait Grow {
            fn grow(self: Self, by: u64) -> u64
        }

        struct G { n: u64 }

        impl Grow for G {
            fn grow(self: Self, amount: u64) -> u64 {
                self.n * amount
            }
        }

        fn main() -> u64 {
            val g = G { n: 6u64 }
            g.grow(7u64)
        }
        "#,
        42,
    );
}
