mod common;

// =====================================================================
// Trait Tests — basic trait declaration, impl-trait blocks, trait-bounded
// generics, and conformance checking. Mirrors the structure of the
// existing generics_tests / collections_tuple_struct_tests files.
// =====================================================================

mod basic {
    use crate::common::test_program;

    #[test]
    fn test_trait_decl_compiles() {
        // A bare trait declaration alongside a struct should type check
        // without needing any impls; the trait simply registers in the
        // context.
        let source = r#"
            trait Greet {
                fn greet(self: Self) -> str
            }
            struct Dog { name: str }
            fn main() -> u64 { 0u64 }
        "#;
        assert!(test_program(source).is_ok(), "expected ok");
    }

    #[test]
    fn test_impl_trait_method_dispatch() {
        // The impl-trait method should be callable directly on a value
        // of the implementing struct, just like an inherent method.
        let source = r#"
            trait Greet {
                fn greet(self: Self) -> str
            }
            struct Dog { name: str }
            impl Greet for Dog {
                fn greet(self: Self) -> str { "Woof!" }
            }
            fn main() -> u64 {
                val d = Dog { name: "Rex" }
                val s = d.greet()
                0u64
            }
        "#;
        assert!(test_program(source).is_ok(), "expected ok");
    }

    #[test]
    fn test_trait_bounded_generic_dispatch() {
        // A generic function bounded by a trait can call the trait's
        // methods on the bounded parameter; at the call site the bound
        // is satisfied by the concrete struct's impl-trait block.
        let source = r#"
            trait Greet {
                fn greet(self: Self) -> str
            }
            struct Dog { name: str }
            impl Greet for Dog {
                fn greet(self: Self) -> str { "Woof!" }
            }
            fn announce<T: Greet>(x: T) -> str { x.greet() }
            fn main() -> u64 {
                val d = Dog { name: "Rex" }
                val s = announce(d)
                0u64
            }
        "#;
        assert!(test_program(source).is_ok(), "expected ok");
    }

    #[test]
    fn test_multiple_structs_implementing_trait() {
        // Two structs implementing the same trait can both satisfy the
        // bound at separate call sites.
        let source = r#"
            trait Greet {
                fn greet(self: Self) -> str
            }
            struct Dog { name: str }
            struct Cat { name: str }
            impl Greet for Dog {
                fn greet(self: Self) -> str { "Woof!" }
            }
            impl Greet for Cat {
                fn greet(self: Self) -> str { "Meow!" }
            }
            fn announce<T: Greet>(x: T) -> str { x.greet() }
            fn main() -> u64 {
                val d = Dog { name: "Rex" }
                val c = Cat { name: "Whiskers" }
                val sd = announce(d)
                val sc = announce(c)
                0u64
            }
        "#;
        assert!(test_program(source).is_ok(), "expected ok");
    }

    #[test]
    fn test_extension_trait_parses_for_primitive_target() {
        // Step A of the extension-trait work: `impl Trait for i64`
        // / `impl Trait for f64` etc. parse + type-check. The body
        // can use `Self` which resolves to the matching primitive
        // (`Self == i64` here, so the `0i64 - self` expression
        // type-checks). The method itself is not yet *callable* —
        // dispatch (Step B+) wires `x.neg()` up to this body.
        let source = r#"
            trait Negate {
                fn neg(self: Self) -> Self
            }
            impl Negate for i64 {
                fn neg(self: Self) -> Self {
                    0i64 - self
                }
            }
            impl Negate for f64 {
                fn neg(self: Self) -> Self {
                    0f64 - self
                }
            }
            fn main() -> u64 { 7u64 }
        "#;
        assert!(
            test_program(source).is_ok(),
            "extension-trait impls on primitives should parse + type-check"
        );
    }
}

mod errors {
    use crate::common::test_program;

    #[test]
    fn test_missing_method_in_impl_is_rejected() {
        let source = r#"
            trait Greet {
                fn greet(self: Self) -> str
            }
            struct Dog { name: str }
            impl Greet for Dog {
            }
            fn main() -> u64 { 0u64 }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("missing method 'greet'"),
            "expected missing-method error, got: {}", err
        );
    }

    #[test]
    fn test_signature_mismatch_in_impl_is_rejected() {
        // The impl returns u64 instead of the trait's str, so conformance
        // should fail.
        let source = r#"
            trait Greet {
                fn greet(self: Self) -> str
            }
            struct Dog { name: str }
            impl Greet for Dog {
                fn greet(self: Self) -> u64 { 0u64 }
            }
            fn main() -> u64 { 0u64 }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("return type mismatch"),
            "expected return-type-mismatch error, got: {}", err
        );
    }

    #[test]
    fn test_unimplementing_struct_violates_bound() {
        // Frog never implements Greet, so passing it to `announce` is a
        // bound violation at the call site.
        let source = r#"
            trait Greet {
                fn greet(self: Self) -> str
            }
            struct Frog { color: str }
            fn announce<T: Greet>(x: T) -> str { x.greet() }
            fn main() -> u64 {
                val f = Frog { color: "green" }
                val s = announce(f)
                0u64
            }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("bound violation") && err.contains("Greet"),
            "expected bound-violation error mentioning Greet, got: {}", err
        );
    }

    #[test]
    fn test_duplicate_trait_decl_is_rejected() {
        let source = r#"
            trait Greet {
                fn greet(self: Self) -> str
            }
            trait Greet {
                fn other(self: Self) -> u64
            }
            fn main() -> u64 { 0u64 }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("already defined"),
            "expected duplicate-trait error, got: {}", err
        );
    }

    #[test]
    fn test_duplicate_method_in_trait_is_rejected() {
        let source = r#"
            trait Greet {
                fn greet(self: Self) -> str
                fn greet(self: Self) -> u64
            }
            fn main() -> u64 { 0u64 }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("duplicate method"),
            "expected duplicate-method error, got: {}", err
        );
    }
}

mod multi_method {
    use crate::common::test_program;

    #[test]
    fn test_trait_with_multiple_methods() {
        // A trait declares two methods; the impl provides both.
        let source = r#"
            trait Counter {
                fn step(self: Self) -> u64
                fn label(self: Self) -> str
            }
            struct Tick { n: u64 }
            impl Counter for Tick {
                fn step(self: Self) -> u64 { self.n + 1u64 }
                fn label(self: Self) -> str { "tick" }
            }
            fn main() -> u64 {
                val t = Tick { n: 5u64 }
                val s = t.step()
                s
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_uint64(), 6);
    }
}
