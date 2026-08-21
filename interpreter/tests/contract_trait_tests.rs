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

#[test]
fn an_inherited_clause_is_reported_before_the_impls_own() {
    // Both sides may carry clauses; the trait's come first, so a
    // violation of the obligation the trait imposed is the one named.
    let src = r#"
        trait Shrink {
            fn shrink(self: Self, by: u64) -> u64
                requires by > 0u64
        }

        struct B { n: u64 }

        impl Shrink for B {
            fn shrink(self: Self, by: u64) -> u64
                requires by < 100u64
            {
                self.n - by
            }
        }

        fn main() -> u64 {
            val b = B { n: 1000u64 }
            b.shrink(REPLACE)
        }
    "#;
    let trait_broken = test_program(&src.replace("REPLACE", "0u64"))
        .expect_err("the trait clause must fire");
    assert!(trait_broken.contains("clause #1"), "{trait_broken}");

    let impl_broken = test_program(&src.replace("REPLACE", "200u64"))
        .expect_err("the impl clause must fire");
    assert!(impl_broken.contains("clause #2"), "{impl_broken}");
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
