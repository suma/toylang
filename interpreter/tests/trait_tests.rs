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
    fn test_prelude_i64_abs() {
        // Step E: `i64.abs()` resolves through the prelude's
        // `impl Abs for i64 { fn abs(self) -> Self { __extern_abs_i64(self) } }`
        // — same user-facing surface as the legacy
        // `BuiltinMethod::I64Abs` path, but routed through the
        // extension-trait machinery + extern dispatch tables. No
        // explicit `import` is needed since the prelude is always
        // integrated.
        let source = r#"
            fn main() -> u64 {
                val n: i64 = -42i64
                n.abs() as u64
            }
        "#;
        let result = test_program(source);
        assert!(result.is_ok(), "prelude i64.abs() should run: {:?}", result.err());
        assert_eq!(result.unwrap().borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_prelude_f64_abs_and_sqrt() {
        // Same coverage as the i64 test on the f64 side. `(-7.5).abs() +
        // 81.sqrt() = 7.5 + 9 = 16.5`, cast to u64 → 16.
        let source = r#"
            fn main() -> u64 {
                val x: f64 = -7.5f64
                val y: f64 = 81f64
                (x.abs() + y.sqrt()) as u64
            }
        "#;
        let result = test_program(source);
        assert!(result.is_ok(), "prelude f64 methods should run: {:?}", result.err());
        assert_eq!(result.unwrap().borrow().unwrap_uint64(), 16);
    }

    #[test]
    fn test_extension_trait_method_dispatch_on_primitive() {
        // Step B of the extension-trait work: a user `impl Trait for
        // <PrimitiveType>` method is callable through the regular
        // `receiver.method(args)` syntax. The interpreter resolves
        // the canonical primitive name (`"i64"` / `"f64"`) to a
        // symbol and looks it up in the same `method_registry` as
        // struct methods. Both i64 and f64 sides exercise `Self`
        // resolution + chained calls.
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
            fn main() -> u64 {
                val a: i64 = 7i64
                val c: i64 = a.neg().neg()       # 7
                val x: f64 = 3.5f64
                val y: f64 = x.neg().neg()       # 3.5
                (c + (y as i64) + 5i64) as u64    # 7 + 3 + 5 = 15
            }
        "#;
        let result = test_program(source);
        assert!(
            result.is_ok(),
            "extension-trait method dispatch on primitive should run: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap().borrow().unwrap_uint64(), 15);
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
    fn test_trait_with_mut_self_rejects_non_mut_impl() {
        // Stage 1 of `&` references: the trait writes the receiver
        // contract; an impl that promises less mutation
        // (`self: Self`) when the trait demands `&mut self` is
        // rejected so users can't silently subvert the trait's
        // mutability promise.
        let source = r#"
            trait Bumpable {
                fn bump(&mut self)
            }
            struct Counter { value: u64 }
            impl Bumpable for Counter {
                fn bump(self: Self) {
                    self.value = self.value + 1u64
                }
            }
            fn main() -> u64 { 0u64 }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("receiver kind mismatch")
                || err.contains("self-parameter mismatch"),
            "expected receiver-kind diagnostic; got: {}", err
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

    #[test]
    fn test_mut_borrow_of_val_binding_is_rejected() {
        // REF-Stage-2 (f): `&mut <name>` is only valid against a
        // `var`-declared local. Attempting to borrow a `val` binding
        // mutably must be a type error so the source location stays
        // honest about which bindings can be mutated through a ref.
        let source = r#"
            fn take(x: &mut u64) -> u64 { 0u64 }
            fn main() -> u64 {
                val a: u64 = 1u64
                take(&mut a)
            }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("cannot borrow") && err.contains("mutable"),
            "expected immutable-binding-borrow error, got: {}", err
        );
    }

    #[test]
    fn test_auto_borrow_into_mut_ref_is_rejected() {
        // REF-Stage-2 (f): `T -> &mut T` auto-borrow is intentionally
        // not allowed. The caller must write `&mut <name>` so the
        // mutability is visible at the call site (mirrors Rust).
        let source = r#"
            fn take(x: &mut u64) -> u64 { 0u64 }
            fn main() -> u64 {
                var a: u64 = 1u64
                # Missing explicit `&mut`; auto-borrow into &mut T is rejected.
                take(a)
            }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("type mismatch") || err.contains("Type") || err.contains("argument"),
            "expected arg-type error for missing `&mut`, got: {}", err
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
