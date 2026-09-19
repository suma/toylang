
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
    fn test_generic_trait_bound_accepts_matching_impl() {
        // `Iter<i64>` as a bound: the concrete impl `impl Iter<i64> for
        // Counter` satisfies it, and the trait's method resolves with the
        // type args substituted (`next` returns `Option<i64>`).
        let source = r#"
            trait Iter<T> {
                fn next(&mut self) -> Option<T>
            }
            struct Counter { n: i64 }
            impl Iter<i64> for Counter {
                fn next(&mut self) -> Option<i64> {
                    self.n = self.n + 1i64
                    Option::Some(self.n)
                }
            }
            fn collect<I: Iter<i64>>(it: I) -> i64 {
                val r = it.next()
                r.unwrap_or(0i64)
            }
            fn main() -> u64 {
                val c = Counter { n: 40i64 }
                val r: i64 = collect(c)
                assert_eq(r, 41i64)
                0u64
            }
        "#;
        let result = test_program(source);
        assert!(result.is_ok(), "generic trait bound should type check: {:?}", result.err());
    }

    #[test]
    fn test_generic_trait_bound_rejects_wrong_type_args() {
        // StrCounter implements `Iter<str>`, not `Iter<i64>`, so the
        // bound `I: Iter<i64>` must reject it — the trait name alone is
        // not enough to satisfy a generic-trait bound.
        let source = r#"
            trait Iter<T> {
                fn next(&mut self) -> Option<T>
            }
            struct StrCounter { n: i64 }
            impl Iter<str> for StrCounter {
                fn next(&mut self) -> Option<str> {
                    Option::Some("x")
                }
            }
            fn collect<I: Iter<i64>>(it: I) -> i64 {
                val r = it.next()
                r.unwrap_or(0i64)
            }
            fn main() -> u64 {
                val c = StrCounter { n: 0i64 }
                collect(c) as u64
            }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("bound violation") && err.contains("Iter<i64>"),
            "expected bound-violation error mentioning Iter<i64>, got: {}", err
        );
    }

    #[test]
    fn test_generic_trait_bound_passthrough() {
        // A bounded generic can forward to another function with the same
        // bound: `passthrough<X: Iter<i64>>` calling `collect<I: Iter<i64>>`
        // passes the bound through instead of requiring a concrete struct.
        let source = r#"
            trait Iter<T> {
                fn next(&mut self) -> Option<T>
            }
            struct Counter { n: i64 }
            impl Iter<i64> for Counter {
                fn next(&mut self) -> Option<i64> {
                    self.n = self.n + 1i64
                    Option::Some(self.n)
                }
            }
            fn collect<I: Iter<i64>>(it: I) -> i64 {
                val r = it.next()
                r.unwrap_or(0i64)
            }
            fn passthrough<X: Iter<i64>>(x: X) -> i64 {
                collect(x)
            }
            fn main() -> u64 {
                val c = Counter { n: 40i64 }
                val r: i64 = passthrough(c)
                assert_eq(r, 41i64)
                0u64
            }
        "#;
        let result = test_program(source);
        assert!(result.is_ok(), "generic trait bound passthrough should type check: {:?}", result.err());
    }

    #[test]
    fn test_into_rewrites_to_from_with_type_hint() {
        // From/Into: `"hi".into()` with a `String` annotation rewrites
        // to `String::from("hi")` at the call site (the blanket `Into`
        // side is derived, not written as an impl).
        let source = r#"
            fn main() -> u64 {
                val s: String = "hi".into()
                val n: u64 = s.size()
                n
            }
        "#;
        let result = test_program(source);
        assert!(result.is_ok(), "into() with String hint should run: {:?}", result.err());
        assert_eq!(result.unwrap().borrow().unwrap_uint64(), 2);
    }

    #[test]
    fn test_into_on_user_defined_struct() {
        // From/Into on a user type: `impl From<i64> for Kelvin` gives
        // `300i64.into() -> Kelvin`.
        let source = r#"
            struct Kelvin { k: i64 }
            impl From<i64> for Kelvin {
                fn from(value: i64) -> Kelvin {
                    val r: Kelvin = Kelvin { k: value }
                    r
                }
            }
            fn main() -> u64 {
                val t: Kelvin = 300i64.into()
                t.k as u64
            }
        "#;
        let result = test_program(source);
        assert!(result.is_ok(), "into() on user struct should run: {:?}", result.err());
        assert_eq!(result.unwrap().borrow().unwrap_uint64(), 300);
    }

    #[test]
    fn test_into_without_type_hint_is_rejected() {
        // No expected type -> no target for the `Into` derivation; the
        // call falls through to ordinary method dispatch which rejects
        // the unknown `into` method.
        let source = r#"
            fn main() -> u64 {
                val s = "hi".into()
                0u64
            }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("into"),
            "expected a diagnostic mentioning into, got: {}", err
        );
    }

    #[test]
    fn test_into_with_unimplemented_target_is_rejected() {
        // `u64` does not implement `From<str>`, so the rewrite must
        // not fire and the call is rejected.
        let source = r#"
            fn main() -> u64 {
                val n: u64 = "hi".into()
                n
            }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("into") || err.contains("method"),
            "expected a diagnostic mentioning into, got: {}", err
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
    fn test_function_may_return_a_reborrow() {
        // ELEMENT-BORROW E1: a reference may leave a function when it
        // is a **reborrow** of what the caller already holds. Handing
        // a parameter back is the simplest shape of that.
        let source = r#"
            fn pick(x: &u64) -> &u64 { x }
            fn main() -> u64 {
                val n = 7u64
                val r = pick(&n)
                r + 0u64
            }
        "#;
        let value = test_program(source).expect("a reborrow is allowed");
        assert_eq!(format!("{:?}", value), "RefCell { value: UInt64(7) }");
    }

    #[test]
    fn test_binding_a_borrow_is_allowed_but_it_cannot_escape() {
        // ELEMENT-BORROW E2: the binding is fine; outliving what it
        // names is not. The escape is the window rule (`[E0026]`),
        // which a borrow now travels under.
        let ok = r#"
            fn main() -> u64 {
                var a: u64 = 1u64
                val r: &u64 = &a
                r + 1u64
            }
        "#;
        let value = test_program(ok).expect("binding a borrow is allowed");
        assert_eq!(format!("{:?}", value), "RefCell { value: UInt64(2) }");

        let escaping = r#"
            fn dangle() -> &String {
                var v: Vec<String> = Vec::new()
                v.push(String::from_str("x"))
                val e = v.borrow(0u64)
                e
            }
            fn main() -> u64 { 0u64 }
        "#;
        let err = test_program(escaping).expect_err("expected an escape error");
        assert!(
            err.contains("E0026") || err.contains("window"),
            "expected the window-escape error, got: {}", err
        );
    }

    #[test]
    fn test_struct_field_of_ref_is_rejected() {
        // REF-Stage-2 (e): struct fields cannot hold references.
        // Without lifetimes a stored `&T` could outlive its
        // referent.
        let source = r#"
            struct Bad { r: &u64 }
            fn main() -> u64 { 0u64 }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("references cannot be stored in struct fields")
                || err.contains("declares a reference type"),
            "expected struct-field-ref escape error, got: {}", err
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

// =====================================================================
// A1: trait default method bodies. A trait method may carry a `{ ... }`
// body; impls that omit the method inherit the default. The default
// body is expanded into the impl AST by `expand_trait_defaults_in_pool`
// so backends see it as an ordinary inherent method.
// =====================================================================
mod default_body {
    use crate::common::test_program;

    #[test]
    fn default_body_inherited_when_impl_omits_method() {
        // Trait declares `value` and `doubled`; the default body for
        // `doubled` forwards through another trait method. Impl supplies
        // only `value`. Calling `doubled()` should dispatch through the
        // default and return `value * 2`.
        let source = r#"
            trait Num {
                fn value(self: Self) -> i64
                fn doubled(self: Self) -> i64 { self.value() + self.value() }
            }
            struct Cell { v: i64 }
            impl Num for Cell {
                fn value(self: Self) -> i64 { self.v }
            }
            fn main() -> i64 {
                val c = Cell { v: 7i64 }
                c.doubled()
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 14);
    }

    #[test]
    fn default_body_can_be_overridden_by_impl() {
        // Impl provides its own `doubled` body; the trait default must
        // be ignored. Default would return 14; override returns 100.
        let source = r#"
            trait Num {
                fn value(self: Self) -> i64
                fn doubled(self: Self) -> i64 { self.value() + self.value() }
            }
            struct Cell { v: i64 }
            impl Num for Cell {
                fn value(self: Self) -> i64 { self.v }
                fn doubled(self: Self) -> i64 { 100i64 }
            }
            fn main() -> i64 {
                val c = Cell { v: 7i64 }
                c.doubled()
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 100);
    }

    #[test]
    fn default_body_works_for_required_method_too() {
        // Even when the trait has only one method and provides a
        // default for it, omitting from the impl should still
        // produce a working dispatch.
        let source = r#"
            trait Tag {
                fn tag(self: Self) -> i64 { 42i64 }
            }
            struct Empty {}
            impl Tag for Empty {
            }
            fn main() -> i64 {
                val e = Empty {}
                e.tag()
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 42);
    }

    #[test]
    fn default_body_calls_another_default() {
        // Two defaults in the same trait, one of which calls the
        // other. After expansion both are inherent methods on the
        // impl, so the inter-default call resolves normally.
        let source = r#"
            trait Math {
                fn base(self: Self) -> i64
                fn doubled(self: Self) -> i64 { self.base() + self.base() }
                fn quadrupled(self: Self) -> i64 { self.doubled() + self.doubled() }
            }
            struct N { v: i64 }
            impl Math for N {
                fn base(self: Self) -> i64 { self.v }
            }
            fn main() -> i64 {
                val n = N { v: 3i64 }
                n.quadrupled()
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 12);
    }

    #[test]
    fn default_body_dispatch_via_bounded_generic() {
        // The default body must also be reachable when the call site
        // goes through a `<T: Trait>` bound rather than a direct
        // method call on the concrete type.
        let source = r#"
            trait Num {
                fn value(self: Self) -> i64
                fn doubled(self: Self) -> i64 { self.value() + self.value() }
            }
            struct Cell { v: i64 }
            impl Num for Cell {
                fn value(self: Self) -> i64 { self.v }
            }
            fn twice_doubled<T: Num>(x: T) -> i64 { x.doubled() }
            fn main() -> i64 {
                val c = Cell { v: 7i64 }
                twice_doubled(c)
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 14);
    }

    #[test]
    fn missing_method_without_default_still_rejected() {
        // Sanity: removing a default-less method from an impl must
        // still produce the missing-method error.
        let source = r#"
            trait Num {
                fn value(self: Self) -> i64
                fn doubled(self: Self) -> i64 { self.value() + self.value() }
            }
            struct Cell { v: i64 }
            impl Num for Cell {
            }
            fn main() -> u64 { 0u64 }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("missing method 'value'"),
            "expected missing-method error for `value`, got: {}", err
        );
    }
}

// =====================================================================
// ITER-PROTOCOL-TRAIT: generic trait declarations + impl-with-trait-args.
// =====================================================================
mod generic_traits {
    use crate::common::{test_program, assert_program_fails};

    #[test]
    fn generic_trait_compiles_alone() {
        // A bare `trait Iterator<T>` declaration alongside a struct
        // should type-check without any impls — the trait just
        // registers `T` as a generic parameter in the trait registry.
        let source = r#"
            trait Box<T> {
                fn unwrap(self: Self) -> T
            }
            struct Holder { v: i64 }
            fn main() -> i64 { 0i64 }
        "#;
        assert!(test_program(source).is_ok(), "expected ok");
    }

    #[test]
    fn generic_trait_impl_with_substitution() {
        // The impl supplies `<i64>` for the trait's `T`. The
        // substituted `next() -> Option<i64>` matches the impl
        // method's literal `Option<i64>`. End-to-end iteration
        // (driven by the parser-level `for x in iter { ... }`
        // desugaring) returns 0+1+2+3+4 = 10.
        let source = r#"
            trait Pull<T> {
                fn next(&mut self) -> Option<T>
            }
            struct Counter { current: i64, end: i64 }
            impl Counter {
                fn new(end: i64) -> Self {
                    Counter { current: 0i64, end: end }
                }
            }
            impl Pull<i64> for Counter {
                fn next(&mut self) -> Option<i64> {
                    if self.current >= self.end {
                        Option::None
                    } else {
                        val v = self.current
                        self.current = self.current + 1i64
                        Option::Some(v)
                    }
                }
            }
            fn main() -> i64 {
                var sum = 0i64
                var iter = Counter::new(5i64)
                for x in iter { sum = sum + x }
                sum
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 10);
    }

    #[test]
    fn generic_trait_return_type_mismatch_rejected() {
        // The impl claims `Pull<i64>` but `next` returns `bool` —
        // after substitution the trait demands `Option<i64>`, so
        // the conformance check must reject this with a clear
        // return-type-mismatch diagnostic.
        let source = r#"
            trait Pull<T> {
                fn next(&mut self) -> Option<T>
            }
            struct Counter { v: i64 }
            impl Counter {
                fn new() -> Self { Counter { v: 0i64 } }
            }
            impl Pull<i64> for Counter {
                fn next(&mut self) -> bool { true }
            }
            fn main() -> i64 { 0i64 }
        "#;
        assert_program_fails(source);
    }

    #[test]
    fn generic_trait_param_substitution_round_trip() {
        // A two-parameter generic trait. Both `T` and `E` get
        // substituted by the impl's args.
        let source = r#"
            trait Encode<T, E> {
                fn encode(self: Self, value: T) -> E
            }
            struct AsHex {}
            impl AsHex {
                fn new() -> Self { AsHex {} }
            }
            impl Encode<i64, u64> for AsHex {
                fn encode(self: Self, value: i64) -> u64 {
                    value as u64
                }
            }
            fn main() -> u64 {
                val a = AsHex::new()
                a.encode(42i64)
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn generic_trait_arg_count_mismatch_rejected() {
        // The trait declares one parameter; the impl supplies
        // two. Arity check should reject this.
        let source = r#"
            trait Box<T> {
                fn id(self: Self, v: T) -> T
            }
            struct Holder {}
            impl Box<i64, u64> for Holder {
                fn id(self: Self, v: i64) -> i64 { v }
            }
            fn main() -> i64 { 0i64 }
        "#;
        assert_program_fails(source);
    }

    #[test]
    fn generic_trait_can_be_implemented_via_stdlib_iterator() {
        // The stdlib's `core/std/iter.t::trait Iterator<T>` is
        // auto-loaded; user code can implement it directly. End-
        // to-end behaviour identical to the structural-only
        // shape that `for x in EXPR` already supports.
        let source = r#"
            struct Counter { current: i64, end: i64 }
            impl Counter {
                fn new(end: i64) -> Self {
                    Counter { current: 0i64, end: end }
                }
            }
            impl Iterator<i64> for Counter {
                fn next(&mut self) -> Option<i64> {
                    if self.current >= self.end {
                        Option::None
                    } else {
                        val v = self.current
                        self.current = self.current + 1i64
                        Option::Some(v)
                    }
                }
            }
            fn main() -> i64 {
                var sum = 0i64
                var iter = Counter::new(5i64)
                for x in iter { sum = sum + x }
                sum
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 10);
    }
}

// =====================================================================
// A2: multi-trait bounds (`<T: A + B>`). The parser promotes `+`-joined
// bounds to `TypeDecl::TraitIntersection([A, B, ...])`; the type checker
// requires the inferred concrete type to implement every trait in the
// intersection, and method dispatch on the bounded T tries each trait.
// =====================================================================
mod multi_bound {
    use crate::common::test_program;

    #[test]
    fn multi_bound_dispatch_through_both_traits() {
        // Dog implements Greet AND Named; the generic body calls one
        // method from each trait, exercising the OR-search in
        // method-call resolution and the AND-check in bound enforcement.
        let source = r#"
            trait Greet {
                fn greet(self: Self) -> i64
            }
            trait Named {
                fn id(self: Self) -> i64
            }
            struct Dog { tag: i64 }
            impl Greet for Dog {
                fn greet(self: Self) -> i64 { 1i64 }
            }
            impl Named for Dog {
                fn id(self: Self) -> i64 { self.tag }
            }
            fn describe<T: Greet + Named>(x: T) -> i64 {
                x.greet() + x.id()
            }
            fn main() -> i64 {
                val d = Dog { tag: 41i64 }
                describe(d)
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 42);
    }

    #[test]
    fn multi_bound_rejects_missing_trait() {
        // Cat implements only Greet; the bound demands `Greet + Named`.
        // The error should pinpoint Named as the missing trait.
        let source = r#"
            trait Greet {
                fn greet(self: Self) -> i64
            }
            trait Named {
                fn id(self: Self) -> i64
            }
            struct Cat { v: i64 }
            impl Greet for Cat {
                fn greet(self: Self) -> i64 { 1i64 }
            }
            fn describe<T: Greet + Named>(x: T) -> i64 {
                x.greet()
            }
            fn main() -> i64 {
                val c = Cat { v: 0i64 }
                describe(c)
            }
        "#;
        let err = test_program(source).expect_err("expected error");
        assert!(
            err.contains("bound violation") && err.contains("Named"),
            "expected Named to be flagged as the missing trait, got: {}",
            err
        );
    }

    #[test]
    fn multi_bound_order_independence() {
        // Switching the bound order `<T: B + A>` vs `<T: A + B>` must
        // produce the same dispatch behavior because intersection
        // semantics are commutative.
        let source = r#"
            trait A {
                fn a_val(self: Self) -> i64
            }
            trait B {
                fn b_val(self: Self) -> i64
            }
            struct S { v: i64 }
            impl A for S {
                fn a_val(self: Self) -> i64 { self.v }
            }
            impl B for S {
                fn b_val(self: Self) -> i64 { self.v + 1i64 }
            }
            fn ab<T: A + B>(x: T) -> i64 { x.a_val() + x.b_val() }
            fn ba<T: B + A>(x: T) -> i64 { x.a_val() + x.b_val() }
            fn main() -> i64 {
                val s = S { v: 10i64 }
                ab(s) + ba(s)
            }
        "#;
        let result = test_program(source).expect("expected ok");
        // ab = 10 + 11 = 21; ba = same = 21; total = 42
        assert_eq!(result.borrow().unwrap_int64(), 42);
    }

    #[test]
    fn multi_bound_three_traits() {
        // A bound list longer than two: `<T: A + B + C>`. Each trait
        // contributes one method that the body uses.
        let source = r#"
            trait A {
                fn a(self: Self) -> i64
            }
            trait B {
                fn b(self: Self) -> i64
            }
            trait C {
                fn c(self: Self) -> i64
            }
            struct S { v: i64 }
            impl A for S { fn a(self: Self) -> i64 { 10i64 } }
            impl B for S { fn b(self: Self) -> i64 { 20i64 } }
            impl C for S { fn c(self: Self) -> i64 { self.v } }
            fn sum<T: A + B + C>(x: T) -> i64 { x.a() + x.b() + x.c() }
            fn main() -> i64 {
                val s = S { v: 12i64 }
                sum(s)
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 42);
    }

    #[test]
    fn single_bound_still_works_after_intersection_landing() {
        // Regression: `<T: A>` (no `+`) must keep the
        // `Identifier(trait_sym)` form and continue to work, since the
        // multi-bound code path is gated on TraitIntersection.
        let source = r#"
            trait A {
                fn a(self: Self) -> i64
            }
            struct S { v: i64 }
            impl A for S { fn a(self: Self) -> i64 { self.v } }
            fn one<T: A>(x: T) -> i64 { x.a() }
            fn main() -> i64 {
                val s = S { v: 42i64 }
                one(s)
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 42);
    }
}

// =====================================================================
// A5-P1: `dyn Trait` trait objects (interpreter only). The type-checker
// allows `&Struct` to be passed where `&dyn Trait` is expected when the
// struct implements the trait, and dispatches `obj.method()` on a
// `&dyn Trait` receiver through the trait's signature table. Backends
// other than the tree-walker reject dyn at lower-time (P2/P3 will add
// AOT / JIT support).
// =====================================================================
mod dyn_trait {
    use crate::common::test_program;

    #[test]
    fn dyn_trait_dispatch_routes_through_concrete_impl() {
        // Two structs implementing the same trait; the function takes a
        // `&dyn Trait` and the dispatch returns each struct's own
        // contribution. Sum is 1 (Dog) + 2 (Cat) = 3.
        let source = r#"
            trait Animal {
                fn vol(self: Self) -> i64
            }
            struct Dog {}
            struct Cat {}
            impl Animal for Dog { fn vol(self: Self) -> i64 { 1i64 } }
            impl Animal for Cat { fn vol(self: Self) -> i64 { 2i64 } }
            fn pick(a: &dyn Animal) -> i64 { a.vol() }
            fn main() -> i64 {
                val d = Dog {}
                val c = Cat {}
                pick(d) + pick(c)
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 3);
    }

    #[test]
    fn dyn_trait_rejects_non_implementing_struct() {
        // Stone does not implement Animal; passing it through a
        // `&dyn Animal` parameter must be rejected by the type-checker.
        let source = r#"
            trait Animal {
                fn vol(self: Self) -> i64
            }
            struct Dog {}
            struct Stone { v: i64 }
            impl Animal for Dog { fn vol(self: Self) -> i64 { 1i64 } }
            fn pick(a: &dyn Animal) -> i64 { a.vol() }
            fn main() -> i64 {
                val s = Stone { v: 0i64 }
                pick(s)
            }
        "#;
        let err = test_program(source).expect_err("expected error");
        // Case-insensitive: the claim is that the argument is rejected,
        // not how the sentence reads.
        assert!(
            err.to_lowercase().contains("type mismatch") || err.contains("Type error"),
            "expected dyn-trait conformance rejection, got: {}",
            err
        );
    }

    #[test]
    fn dyn_trait_with_default_body() {
        // A trait default body is reachable via dyn dispatch. The impl
        // provides `value`; `doubled` is inherited from the trait
        // default. A5 reuses A1's expansion path, so backends see the
        // synthesized default as an ordinary inherent method, which
        // means dyn dispatch finds it through the regular registry.
        let source = r#"
            trait Num {
                fn value(self: Self) -> i64
                fn doubled(self: Self) -> i64 { self.value() + self.value() }
            }
            struct Cell { v: i64 }
            impl Num for Cell {
                fn value(self: Self) -> i64 { self.v }
            }
            fn use_dyn(n: &dyn Num) -> i64 { n.doubled() }
            fn main() -> i64 {
                val c = Cell { v: 21i64 }
                use_dyn(c)
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 42);
    }

    #[test]
    fn dyn_trait_explicit_borrow_form() {
        // The explicit `&value` borrow form at the call site is also
        // accepted; the auto-borrow path and the explicit form must
        // produce the same dispatch.
        let source = r#"
            trait Animal {
                fn vol(self: Self) -> i64
            }
            struct Dog {}
            impl Animal for Dog { fn vol(self: Self) -> i64 { 7i64 } }
            fn pick(a: &dyn Animal) -> i64 { a.vol() }
            fn main() -> i64 {
                val d = Dog {}
                pick(&d)
            }
        "#;
        let result = test_program(source).expect("expected ok");
        assert_eq!(result.borrow().unwrap_int64(), 7);
    }
}
