//! The `?` operator, trait default bodies, multi-bounds, `From` /
//! `Into`, and `dyn Trait` dispatch.

use super::harness::*;

#[test]
fn try_op_result_ok_path_round_trip() {
    let src = r#"
        fn divide(a: u64, b: u64) -> Result<u64, u64> {
            if b == 0u64 {
                Result::Err(99u64)
            } else {
                Result::Ok(a / b)
            }
        }

        fn compute() -> Result<u64, u64> {
            val x = divide(100u64, 5u64)?
            val y = divide(20u64, 2u64)?
            Result::Ok(x + y)
        }

        fn main() -> u64 {
            val r = compute()
            match r {
                Result::Ok(v) => v,
                Result::Err(_) => 255u64,
            }
        }
    "#;
    assert_consistent(src, "try_op_result_ok_path");
}

#[test]
fn try_op_result_err_propagates_round_trip() {
    let src = r#"
        fn divide(a: u64, b: u64) -> Result<u64, u64> {
            if b == 0u64 {
                Result::Err(7u64)
            } else {
                Result::Ok(a / b)
            }
        }

        fn compute(d: u64) -> Result<u64, u64> {
            val x = divide(100u64, d)?
            Result::Ok(x + 1u64)
        }

        fn main() -> u64 {
            val r = compute(0u64)
            match r {
                Result::Ok(v) => v,
                Result::Err(e) => e,
            }
        }
    "#;
    assert_consistent(src, "try_op_result_err_propagates");
}

#[test]
fn try_op_option_some_path_round_trip() {
    let src = r#"
        fn first_positive(a: u64, b: u64) -> Option<u64> {
            if a > 0u64 {
                Option::Some(a)
            } elif b > 0u64 {
                Option::Some(b)
            } else {
                Option::None
            }
        }

        fn chain() -> Option<u64> {
            val x = first_positive(3u64, 0u64)?
            val y = first_positive(x, 7u64)?
            Option::Some(y + 1u64)
        }

        fn main() -> u64 {
            val r = chain()
            match r {
                Option::Some(v) => v,
                Option::None => 99u64,
            }
        }
    "#;
    assert_consistent(src, "try_op_option_some_path");
}

#[test]
fn try_op_option_none_propagates_round_trip() {
    let src = r#"
        fn first_positive(a: u64, b: u64) -> Option<u64> {
            if a > 0u64 {
                Option::Some(a)
            } elif b > 0u64 {
                Option::Some(b)
            } else {
                Option::None
            }
        }

        fn chain() -> Option<u64> {
            val x = first_positive(0u64, 0u64)?
            Option::Some(x + 1u64)
        }

        fn main() -> u64 {
            val r = chain()
            match r {
                Option::Some(v) => v,
                Option::None => 42u64,
            }
        }
    "#;
    assert_consistent(src, "try_op_option_none_propagates");
}

#[test]
fn trait_default_body_inherited_round_trip() {
    // A1: trait method `doubled` carries a default body that calls
    // `value`; impl provides only `value`. All three backends must
    // dispatch the inherited default and agree on the result.
    let src = r#"
        trait Num {
            fn value(self: Self) -> u64
            fn doubled(self: Self) -> u64 { self.value() + self.value() }
        }
        struct Cell { v: u64 }
        impl Num for Cell {
            fn value(self: Self) -> u64 { self.v }
        }
        fn main() -> u64 {
            val c = Cell { v: 7u64 }
            c.doubled()
        }
    "#;
    assert_consistent(src, "trait_default_inherited");
}

#[test]
fn trait_default_body_override_round_trip() {
    // A1: impl overrides the default; backends must respect the
    // override (return 100, not the default's 14).
    let src = r#"
        trait Num {
            fn value(self: Self) -> u64
            fn doubled(self: Self) -> u64 { self.value() + self.value() }
        }
        struct Cell { v: u64 }
        impl Num for Cell {
            fn value(self: Self) -> u64 { self.v }
            fn doubled(self: Self) -> u64 { 100u64 }
        }
        fn main() -> u64 {
            val c = Cell { v: 7u64 }
            c.doubled()
        }
    "#;
    assert_consistent(src, "trait_default_override");
}

#[test]
fn trait_default_body_calls_default_round_trip() {
    // A1: two defaults where one calls the other. After expansion both
    // are inherent methods on the impl; backends must agree on the
    // chained dispatch.
    let src = r#"
        trait Math {
            fn base(self: Self) -> u64
            fn doubled(self: Self) -> u64 { self.base() + self.base() }
            fn quadrupled(self: Self) -> u64 { self.doubled() + self.doubled() }
        }
        struct N { v: u64 }
        impl Math for N {
            fn base(self: Self) -> u64 { self.v }
        }
        fn main() -> u64 {
            val n = N { v: 3u64 }
            n.quadrupled()
        }
    "#;
    assert_consistent(src, "trait_default_chained");
}

#[test]
fn multi_bound_dispatch_round_trip() {
    // A2: `<T: A + B>` exercising one method from each trait. All three
    // backends must agree on the result of calling through the
    // intersection bound.
    let src = r#"
        trait A {
            fn a(self: Self) -> u64
        }
        trait B {
            fn b(self: Self) -> u64
        }
        struct S { v: u64 }
        impl A for S { fn a(self: Self) -> u64 { 10u64 } }
        impl B for S { fn b(self: Self) -> u64 { self.v } }
        fn both<T: A + B>(x: T) -> u64 { x.a() + x.b() }
        fn main() -> u64 {
            val s = S { v: 32u64 }
            both(s)
        }
    "#;
    assert_consistent(src, "multi_bound_dispatch");
}

#[test]
fn generic_trait_bound_dispatch_round_trip() {
    // TRAIT-BOUND: a generic-trait bound (`I: Iter<i64>`) is checked
    // against the impl's concrete type args at the call site, and the
    // trait method's return type is resolved with those args substituted.
    // All three backends must agree.
    let src = r#"
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
            collect(c) as u64
        }
    "#;
    assert_consistent(src, "generic_trait_bound_dispatch");
}

#[test]
fn generic_trait_bound_passthrough_round_trip() {
    // TRAIT-BOUND: a bounded generic forwards to another function with
    // the same generic-trait bound; the pass-through must satisfy the
    // callee's bound without naming a concrete struct.
    let src = r#"
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
            passthrough(c) as u64
        }
    "#;
    assert_consistent(src, "generic_trait_bound_passthrough");
}

#[test]
fn into_string_round_trip() {
    // From/Into: `"hi".into()` with a `String` annotation rewrites to
    // `String::from("hi")`. All three backends must run the rewritten
    // call and agree on the byte count.
    let src = r#"
        fn main() -> u64 {
            val s: String = "hi".into()
            s.size()
        }
    "#;
    assert_consistent(src, "into_string");
}

#[test]
fn into_user_struct_round_trip() {
    // From/Into on a user type: `300i64.into()` with a `Kelvin`
    // annotation dispatches to `impl From<i64> for Kelvin`.
    let src = r#"
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
    assert_consistent(src, "into_user_struct");
}

#[test]
fn try_cross_error_conversion_round_trip() {
    // From/Into `?` cross-error conversion: `inner()?` inside a
    // function returning `Result<i64, ErrWrap>` converts the `str`
    // error through `ErrWrap: From<str>` before re-returning it.
    // All three backends must agree on the converted payload.
    let src = r#"
        struct ErrWrap { code: u64 }

        impl From<str> for ErrWrap {
            fn from(value: str) -> ErrWrap {
                val r: ErrWrap = ErrWrap { code: 42u64 }
                r
            }
        }

        fn inner() -> Result<i64, str> {
            Result::Err("boom")
        }

        fn outer() -> Result<i64, ErrWrap> {
            val x = inner()?
            Result::Ok(x)
        }

        fn main() -> u64 {
            match outer() {
                Result::Err(w) => w.code,
                Result::Ok(v) => v as u64,
            }
        }
    "#;
    assert_consistent(src, "try_cross_error_conversion");
}

#[test]
fn multi_bound_three_traits_round_trip() {
    // A2: `<T: A + B + C>` — longer bound list across 3 backends.
    let src = r#"
        trait A { fn a(self: Self) -> u64 }
        trait B { fn b(self: Self) -> u64 }
        trait C { fn c(self: Self) -> u64 }
        struct S { v: u64 }
        impl A for S { fn a(self: Self) -> u64 { 1u64 } }
        impl B for S { fn b(self: Self) -> u64 { 2u64 } }
        impl C for S { fn c(self: Self) -> u64 { self.v } }
        fn sum<T: A + B + C>(x: T) -> u64 { x.a() + x.b() + x.c() }
        fn main() -> u64 {
            val s = S { v: 39u64 }
            sum(s)
        }
    "#;
    assert_consistent(src, "multi_bound_three_traits");
}

#[test]
fn dyn_trait_empty_struct_round_trip() {
    // A5-P2-MVP-A: `&dyn Trait` dispatch on an empty struct. All
    // three backends must agree:
    // - interpreter (A5-P1, type-erased method registry)
    // - cranelift JIT (silent fallback to interpreter — Dyn type
    //   is rejected by eligibility, so this leg actually runs the
    //   interpreter too)
    // - AOT (P2-MVP-A: fat pointer + vtable + CallIndirectFn)
    let src = r#"
        trait Animal {
            fn sound(self: Self) -> i64
        }
        struct Dog {}
        impl Animal for Dog {
            fn sound(self: Self) -> i64 { 7i64 }
        }
        fn describe(a: &dyn Animal) -> i64 {
            a.sound()
        }
        fn main() -> u64 {
            val d = Dog {}
            describe(d) as u64
        }
    "#;
    assert_consistent(src, "dyn_empty_struct");
}

#[test]
fn dyn_trait_heterogeneous_dispatch_round_trip() {
    // A5-P2-MVP-A: same `&dyn Trait` parameter, two different
    // concrete empty structs. Vtable per-impl is exercised: Dog
    // dispatches to Dog::tone(), Cat dispatches to Cat::tone(),
    // sum is 1 + 2 = 3.
    let src = r#"
        trait Animal {
            fn tone(self: Self) -> i64
        }
        struct Dog {}
        struct Cat {}
        impl Animal for Dog { fn tone(self: Self) -> i64 { 1i64 } }
        impl Animal for Cat { fn tone(self: Self) -> i64 { 2i64 } }
        fn pick(a: &dyn Animal) -> i64 {
            a.tone()
        }
        fn main() -> u64 {
            val d = Dog {}
            val c = Cat {}
            (pick(d) + pick(c)) as u64
        }
    "#;
    assert_consistent(src, "dyn_hetero_dispatch");
}

#[test]
fn dyn_trait_scalar_field_round_trip() {
    // A5-P2-MVP-B: `&dyn Trait` dispatch on a struct with one scalar
    // field. The fat pointer's data_ptr now references a caller-frame
    // stack slot holding the field value; the dispatched thunk reads
    // it back via PtrRead before forwarding to the impl method.
    let src = r#"
        trait Num {
            fn get(self: Self) -> u64
        }
        struct Cell { v: u64 }
        impl Num for Cell {
            fn get(self: Self) -> u64 { self.v }
        }
        fn use_dyn(n: &dyn Num) -> u64 {
            n.get()
        }
        fn main() -> u64 {
            val c = Cell { v: 42u64 }
            use_dyn(c)
        }
    "#;
    assert_consistent(src, "dyn_scalar_field");
}

#[test]
fn dyn_trait_two_scalar_fields_round_trip() {
    // A5-P2-MVP-B: struct with two scalar fields of mixed types.
    // Tests the natural-sum byte offset accounting (i64=8, u64=8
    // → leaf2 at offset 8) for both coercion-site PtrWrite and
    // thunk-side PtrRead. Result = x + y = 10 + 32 = 42.
    let src = r#"
        trait Pair {
            fn sum(self: Self) -> u64
        }
        struct Pt { x: u64, y: u64 }
        impl Pair for Pt {
            fn sum(self: Self) -> u64 { self.x + self.y }
        }
        fn use_dyn(p: &dyn Pair) -> u64 {
            p.sum()
        }
        fn main() -> u64 {
            val p = Pt { x: 10u64, y: 32u64 }
            use_dyn(p)
        }
    "#;
    assert_consistent(src, "dyn_two_scalar_fields");
}

#[test]
fn dyn_trait_heterogeneous_field_round_trip() {
    // A5-P2-MVP-B: two concrete types with different scalar fields
    // both routed through the same `&dyn Trait` param. Cell uses
    // i64, Pad uses u64 — different per-impl thunks but the
    // dispatch site sees a uniform call signature.
    let src = r#"
        trait Show {
            fn payload(self: Self) -> u64
        }
        struct Cell { v: u64 }
        struct Pad { w: u64 }
        impl Show for Cell { fn payload(self: Self) -> u64 { self.v } }
        impl Show for Pad  { fn payload(self: Self) -> u64 { self.w + 1u64 } }
        fn pick(s: &dyn Show) -> u64 { s.payload() }
        fn main() -> u64 {
            val c = Cell { v: 5u64 }
            val p = Pad { w: 7u64 }
            pick(c) + pick(p)
        }
    "#;
    assert_consistent(src, "dyn_hetero_field");
}

#[test]
fn dyn_trait_nested_struct_round_trip() {
    // A5-P2-MVP-C: `&dyn Trait` dispatch on a struct whose field
    // is itself a struct. Recursive `flatten_struct_locals` and
    // `flatten_compound_leaf_types` agree on the leaf order, so
    // the same `(byte_offset, leaf_ty)` list drives the
    // coercion-site PtrWrite and the thunk PtrRead. Result =
    // inner.v + tag = 100 + 7 = 107.
    let src = r#"
        trait Show {
            fn read(self: Self) -> i64
        }
        struct Inner { v: i64 }
        struct Outer { inner: Inner, tag: u64 }
        impl Show for Outer {
            fn read(self: Self) -> i64 { self.inner.v + self.tag as i64 }
        }
        fn use_dyn(s: &dyn Show) -> i64 {
            s.read()
        }
        fn main() -> u64 {
            val o = Outer { inner: Inner { v: 100i64 }, tag: 7u64 }
            use_dyn(o) as u64
        }
    "#;
    assert_consistent(src, "dyn_nested_struct");
}

#[test]
fn dyn_trait_mut_self_round_trip() {
    // A5-P2-MVP-C: `&mut dyn Trait` writeback. The trait method
    // `bump(&mut self)` mutates the struct; the per-impl thunk
    // captures the writeback via `CallWithSelfWriteback` and
    // writes it back to `data_ptr` (the caller's stack slot).
    // After the outer call, the dispatch site reads the slot
    // leaves back into the caller's struct binding, so the
    // second `bump()` sees the first call's mutation
    // (a = 11, b = 12, a + b = 23).
    let src = r#"
        trait Counter {
            fn bump(&mut self) -> i64
        }
        struct Tick { n: i64 }
        impl Counter for Tick {
            fn bump(&mut self) -> i64 {
                self.n = self.n + 1i64
                self.n
            }
        }
        fn pump(c: &mut dyn Counter) -> i64 {
            c.bump()
        }
        fn main() -> u64 {
            var t = Tick { n: 10i64 }
            val a = pump(&mut t)
            val b = pump(&mut t)
            (a + b) as u64
        }
    "#;
    assert_consistent(src, "dyn_mut_self");
}

#[test]
fn dyn_trait_struct_return_round_trip() {
    // A5-P2-MVP-D: `&dyn Trait` dispatch where the trait method
    // returns a struct. The thunk's `Call(impl)` becomes
    // `CallStruct` so cranelift's multi-result call lands in
    // pre-allocated leaf locals, the thunk's `Return` emits them
    // all, and the caller's `CallIndirectFnStruct` fans the
    // results into the let-binding's per-field locals.
    // p.x + p.y = self.v + (self.v + 1) = 2*v + 1 = 21 for v=10.
    let src = r#"
        trait Make {
            fn build(self: Self) -> Pair
        }
        struct Pair { x: i64, y: i64 }
        struct Cell { v: i64 }
        impl Make for Cell {
            fn build(self: Self) -> Pair {
                Pair { x: self.v, y: self.v + 1i64 }
            }
        }
        fn use_dyn(m: &dyn Make) -> i64 {
            val p = m.build()
            p.x + p.y
        }
        fn main() -> u64 {
            val c = Cell { v: 10i64 }
            use_dyn(c) as u64
        }
    "#;
    assert_consistent(src, "dyn_struct_return");
}

#[test]
fn dyn_trait_tuple_return_round_trip() {
    // A5-P2-MVP-E: `&dyn Trait` dispatch where the trait method
    // returns a tuple. The thunk's `Call(impl)` becomes
    // `CallTuple` so cranelift's multi-result lands in
    // pre-allocated leaf locals; the caller's
    // `CallIndirectFnTuple` fans the results into the
    // let-binding's per-element locals via the standard
    // `pending_tuple_value` channel. Result = 10 + 42 = 52.
    let src = r#"
        trait Paired {
            fn get_pair(self: Self) -> (i64, u64)
        }
        struct Maker { x: i64 }
        impl Paired for Maker {
            fn get_pair(self: Self) -> (i64, u64) {
                (self.x, 42u64)
            }
        }
        fn extract(b: &dyn Paired) -> u64 {
            val (a, c) = b.get_pair()
            (a as u64) + c
        }
        fn main() -> u64 {
            val m = Maker { x: 10i64 }
            extract(m)
        }
    "#;
    assert_consistent(src, "dyn_tuple_return");
}

#[test]
fn dyn_trait_enum_return_round_trip() {
    // A5-P2-MVP-E: `&dyn Trait` dispatch where the trait method
    // returns an enum (`Option<i64>`). The thunk uses `CallEnum`
    // to capture `[tag, Some_payload, ...]`; the caller's
    // `CallIndirectFnEnum` fans the results into a fresh
    // `EnumStorage` via the `pending_enum_value` channel. Result
    // = 42 (Some branch).
    let src = r#"
        trait Optional {
            fn maybe(self: Self) -> Option<i64>
        }
        struct Wrapper { n: i64 }
        impl Optional for Wrapper {
            fn maybe(self: Self) -> Option<i64> {
                if self.n > 0i64 {
                    Option::Some(self.n)
                } else {
                    Option::None
                }
            }
        }
        fn check(w: &dyn Optional) -> i64 {
            val r = w.maybe()
            match r {
                Option::Some(v) => v,
                Option::None => 0i64,
            }
        }
        fn main() -> u64 {
            val w = Wrapper { n: 42i64 }
            check(w) as u64
        }
    "#;
    assert_consistent(src, "dyn_enum_return");
}

#[test]
fn dyn_trait_mut_self_struct_return_round_trip() {
    // A5-P2-MVP-F: `&mut dyn Trait` dispatch where the trait
    // method is `&mut self` AND returns a struct. The thunk
    // routes through `CallWithSelfWritebackCompound` to capture
    // both the user-visible return leaves and the writeback
    // leaves in one call, then PtrWrites the writeback half
    // back to `data_ptr` and returns the user half through the
    // multi-value Return terminator. The caller's `&mut dyn`
    // drain (MVP-C) reads the mutated leaves out of the slot
    // after the outer call so the second `step()` sees the
    // first call's increment.
    // After two steps starting from n=10:
    //   step1: n=11, Pair{x:11, y:22}
    //   step2: n=12, Pair{x:12, y:24}
    //   sum = 11 + 22 + 12 + 24 = 69
    let src = r#"
        trait Pump {
            fn step(&mut self) -> Pair
        }
        struct Pair { x: i64, y: i64 }
        struct Cell { n: i64 }
        impl Pump for Cell {
            fn step(&mut self) -> Pair {
                self.n = self.n + 1i64
                Pair { x: self.n, y: self.n * 2i64 }
            }
        }
        fn drive(c: &mut dyn Pump) -> i64 {
            val p1 = c.step()
            val p2 = c.step()
            p1.x + p1.y + p2.x + p2.y
        }
        fn main() -> u64 {
            var c = Cell { n: 10i64 }
            drive(&mut c) as u64
        }
    "#;
    assert_consistent(src, "dyn_mut_self_struct_return");
}

// ERROR_MODEL E2: `?` written as a statement, and `?` on a
// `Result<(), E>`.
//
// The existing tests above all bind the result (`val x = f()?`), which
// is the only shape the desugar used to reach: a bare `f()?` type-checked
// and then died at run time with `unexpected expr: Try`, and a
// `Result<(), E>` had no working shape at all. The four combinations
// below — statement / tail position, unit / non-unit success type, free
// function / method — are pinned together because they share one rewrite.

#[test]
fn try_op_in_statement_position_round_trip() {
    let src = r#"
        enum E { Bad }

        fn step(n: u64) -> Result<u64, E> {
            if n == 0u64 { Result::Err(E::Bad) } else { Result::Ok(n) }
        }

        fn run(n: u64) -> Result<u64, E> {
            step(n)?
            val v = step(n + 1u64)?
            Result::Ok(v)
        }

        fn main() -> u64 {
            val ok = run(3u64)
            val a = match ok {
                Result::Ok(v) => v,
                Result::Err(_) => 200u64,
            }
            val bad = run(0u64)
            val b = match bad {
                Result::Ok(_) => 100u64,
                Result::Err(_) => 9u64,
            }
            a + b
        }
    "#;
    // run(3) discards a 3 and binds a 4; run(0) fails at the discarded
    // call, so the statement `?` really does propagate.
    assert_consistent(src, "try_op_statement_position");
}

#[test]
fn try_op_on_unit_result_round_trip() {
    let src = r#"
        enum E { Bad }

        struct Sink { seen: u64 }

        impl Sink {
            fn accept(&mut self, n: u64) -> Result<(), E> {
                if n == 0u64 { return Result::Err(E::Bad) }
                self.seen = self.seen + n
                Result::Ok(())
            }
        }

        fn fill(s: &mut Sink, n: u64) -> Result<u64, E> {
            s.accept(n)?
            s.accept(n + 1u64)?
            Result::Ok(s.seen)
        }

        fn main() -> u64 {
            var s = Sink { seen: 0u64 }
            val r = fill(&mut s, 5u64)
            val total = match r {
                Result::Ok(v) => v,
                Result::Err(_) => 200u64,
            }
            var t = Sink { seen: 0u64 }
            val bad = fill(&mut t, 0u64)
            val code = match bad {
                Result::Ok(_) => 100u64,
                Result::Err(_) => 1u64,
            }
            total + code
        }
    "#;
    // 5 + 6 = 11 on the success path, plus 1 for the failing one.
    assert_consistent(src, "try_op_unit_result");
}

#[test]
fn a_type_can_convert_from_several_error_types() {
    // ERROR_MODEL E1. Two `From` impls on one aggregate error type is
    // how a program that can fail in more than one way is written; the
    // second impl used to replace the first in the method registry, so
    // this did not type-check at all.
    let src = r#"
        enum Io { Missing }
        enum Parse { NotANumber }

        enum AppError { FromIo(Io), FromParse(Parse) }

        impl From<Io> for AppError {
            fn from(value: Io) -> Self { AppError::FromIo(value) }
        }
        impl From<Parse> for AppError {
            fn from(value: Parse) -> Self { AppError::FromParse(value) }
        }

        fn read(ok: bool) -> Result<u64, Io> {
            if ok { Result::Ok(4u64) } else { Result::Err(Io::Missing) }
        }

        fn parse(ok: bool) -> Result<u64, Parse> {
            if ok { Result::Ok(3u64) } else { Result::Err(Parse::NotANumber) }
        }

        fn load(read_ok: bool, parse_ok: bool) -> Result<u64, AppError> {
            val a = read(read_ok)?
            val b = parse(parse_ok)?
            Result::Ok(a + b)
        }

        fn code(read_ok: bool, parse_ok: bool) -> u64 {
            val r = load(read_ok, parse_ok)
            match r {
                Result::Ok(v) => v,
                Result::Err(e) => {
                    match e {
                        AppError::FromIo(_) => 10u64,
                        AppError::FromParse(_) => 20u64,
                    }
                }
            }
        }

        fn main() -> u64 {
            # Both conversions survive, and each `?` picks the impl
            # that matches the error it is carrying.
            code(true, true) + code(false, true) + code(true, false)
        }
    "#;
    // 7 + 10 + 20 = 37.
    assert_consistent(src, "several_from_impls");
}

#[test]
fn from_is_selected_by_the_argument_type() {
    // The explicit-call side of the same fix: `E::from(x)` picks its
    // impl by what `x` is, not by which impl was registered last.
    let src = r#"
        enum E { N(u64), B(bool) }

        impl From<u64> for E {
            fn from(value: u64) -> Self { E::N(value) }
        }
        impl From<bool> for E {
            fn from(value: bool) -> Self { E::B(value) }
        }

        fn main() -> u64 {
            val a: E = E::from(5u64)
            val b: E = E::from(true)
            val x = match a { E::N(n) => n, E::B(_) => 0u64 }
            val y = match b { E::N(_) => 0u64, E::B(f) => if f { 7u64 } else { 1u64 } }
            x + y
        }
    "#;
    assert_consistent(src, "from_selected_by_argument");
}


// STDLIB-TRAIT-BASE B1 / B3 / B4: what a bound is for.
//
// `Ord` was the one trait usable through a bound, and the reason was
// accidental: `lt` returns `bool` and takes no `&mut`, so it dodged
// both holes. A trait method returning `Self` could not be called
// through a type parameter at all, and a `&mut T` parameter could not
// be inferred at the call site — which is every trait anyone would
// want to write next.

#[test]
fn a_trait_method_returning_self_is_callable_through_a_bound() {
    let src = r#"
        struct P { v: i64 }
        impl Clone for P {
            fn clone(&self) -> Self { P { v: self.v } }
        }

        fn dup<T: Clone>(v: T) -> T {
            val c: T = v.clone()
            c
        }

        fn main() -> u64 {
            val a: u64 = 7u64
            val b: u64 = dup(a)
            val p = P { v: 5i64 }
            val q: P = dup(p)
            val n: i64 = q.v
            b + (n as u64)
        }
    "#;
    // 7 + 5. The primitive goes through `impl Clone for u64`, the
    // struct through the user's own impl, and both reach the same
    // generic body.
    assert_consistent(src, "clone_through_bound");
}

#[test]
fn a_generic_function_can_take_a_mutable_borrow() {
    // `Cannot unify &mut T with &mut P`: the unifier had no row for a
    // reference at all. `&T` failed more quietly — the call was
    // accepted and the return type came back `Unknown`, surfacing
    // later as an error somewhere else entirely.
    let src = r#"
        struct P { n: u64 }
        trait Bump { fn bump(&mut self) }
        impl Bump for P {
            fn bump(&mut self) { self.n = self.n + 1u64 }
        }

        fn go<T: Bump>(v: &mut T) { v.bump() }

        fn main() -> u64 {
            var p = P { n: 5u64 }
            go(&mut p)
            go(&mut p)
            p.n
        }
    "#;
    assert_consistent(src, "generic_mut_borrow");
}

#[test]
fn a_generic_function_can_take_a_shared_borrow_of_a_compound() {
    let src = r#"
        struct P { v: i64 }
        impl Clone for P {
            fn clone(&self) -> Self { P { v: self.v } }
        }

        fn dup<T: Clone>(v: &T) -> T {
            val c: T = v.clone()
            c
        }

        fn main() -> u64 {
            val p = P { v: 5i64 }
            val q: P = dup(&p)
            val n: i64 = q.v
            n as u64
        }
    "#;
    assert_consistent(src, "generic_shared_borrow");
}

#[test]
fn cloning_a_string_gives_an_independent_buffer() {
    // The point of `Clone` in this language: `val b = a` on a compound
    // is an alias, and putting `a` in a container moves it. A clone is
    // the other thing you can hand over — with its own allocation,
    // freed on its own.
    let src = r#"
        fn main() -> u64 {
            var s = String::from_str("hi")
            val t: String = s.clone()
            s.push_str("!")
            println(s)
            println(t)
            s.len() + t.len()
        }
    "#;
    assert_stdout_consistent(src, "string_clone_independent");
}

// STDLIB-TRAIT-BASE B2: the stdlib's iterators name the trait.
//
// Sixteen of them existed with `fn next(&mut self) -> Option<T>` as an
// inherent method and none said `impl Iterator<T>`, because the `for`
// loop desugar is structural and never had to ask. The cost was that a
// function taking an iterator could not be written at all: you could
// take a `Vec<i64>`, but not the result of `v.iter().filter(...)`.
//
// The methods were *moved* rather than added -- writing one in both an
// inherent and a trait impl silently discards a body, which the
// diagnostic below now refuses.

#[test]
fn a_function_can_take_any_stdlib_iterator() {
    let src = r#"
        fn total<I: Iterator<u64>>(it: I) -> u64 {
            var sum: u64 = 0u64
            for x in it { sum = sum + x }
            sum
        }

        fn count<I: Iterator<u8>>(it: I) -> u64 {
            var n: u64 = 0u64
            for b in it { n = n + 1u64 }
            n
        }

        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(1u64)
            v.push(2u64)
            v.push(3u64)
            # Bound first: the compiled lanes take a generic function's
            # type arguments from bindings, not from a call in the
            # argument slot.
            val vi = v.iter()
            val a = total(vi)
            val s = String::from_str("hello")
            val si = s.iter()
            val b = count(si)
            a + b
        }
    "#;
    // 6 + 5. Two different stdlib iterators, one bound each.
    assert_consistent(src, "iterator_bound_stdlib");
}

#[test]
fn for_loops_over_stdlib_iterators_still_work() {
    // The move must be invisible to existing programs: the desugar
    // looks for `next`, not for a trait.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(10u64)
            v.push(20u64)
            var sum: u64 = 0u64
            for x in v.iter() { sum = sum + x }
            val s = String::from_str("abc")
            var bytes: u64 = 0u64
            for b in s.iter() { bytes = bytes + 1u64 }
            var chars: u64 = 0u64
            for c in s.chars() { chars = chars + 1u64 }
            var d: Deque<u64> = Deque::new()
            d.push_back(5u64)
            var dq: u64 = 0u64
            for x in d.iter() { dq = dq + x }
            sum + bytes + chars + dq
        }
    "#;
    // 30 + 3 + 3 + 5 = 41.
    assert_consistent(src, "iterator_for_loops_still_work");
}

#[test]
fn writing_one_method_twice_is_refused_before_it_runs() {
    // The registries replace on a matching key, so one of the two
    // bodies disappears. That used to be found at run time, after the
    // type check had passed.
    let errs = type_check_errors(
        r#"
        struct Counter { n: i64 }
        impl Counter {
            fn next(&mut self) -> Option<i64> { Option::Some(self.n) }
        }
        impl Iterator<i64> for Counter {
            fn next(&mut self) -> Option<i64> { Option::None }
        }
        fn main() -> u64 { 0u64 }
        "#,
    );
    let joined = errs.join("\n");
    assert!(
        joined.contains("two impls of `next`"),
        "a doubly-written method should be refused at check time:\n{joined}"
    );
}

// STDLIB-TRAIT-BASE B5 (mechanism): a type argument that only the
// return position can name.
//
// Every layer read type arguments from the *arguments*, which leaves
// nothing to read when a parameter appears solely in the return type.
// `T::default()` was `[E0003] Struct 'T' not found` in the checker,
// `cannot infer type arguments` in the monomorphiser, and
// `Associated function 'default' not found for struct 'T'` in the
// tree-walker -- three reports of one missing source of evidence.

#[test]
fn a_type_parameter_can_be_named_by_the_binding_alone() {
    let src = r#"
        trait Spawn { fn spawn() -> Self }

        struct P { v: i64 }
        struct Q { v: i64 }

        impl Spawn for P { fn spawn() -> Self { P { v: 3i64 } } }
        impl Spawn for Q { fn spawn() -> Self { Q { v: 40i64 } } }

        fn make<T: Spawn>() -> T {
            val c: T = T::spawn()
            c
        }

        fn main() -> u64 {
            # Nothing in the call says which `T`; the annotation does,
            # and each instantiation has to reach its own impl.
            val p: P = make()
            val q: Q = make()
            val a: i64 = p.v
            val b: i64 = q.v
            (a + b) as u64
        }
    "#;
    assert_consistent(src, "type_arg_from_binding");
}

#[test]
fn an_argument_still_wins_over_the_binding() {
    // The annotation is evidence of last resort: reaching for it
    // whenever a hint exists broke inference that was already working,
    // because a call site's type hint is not always the expected
    // return type -- it also carries numeric-literal context into the
    // arguments.
    let src = r#"
        fn first<T>(a: T, b: T) -> T { a }

        fn main() -> u64 {
            val x: u64 = first(7u64, 9u64)
            val y: i64 = first(1i64, 2i64)
            x + (y as u64)
        }
    "#;
    assert_consistent(src, "argument_beats_binding");
}

// STDLIB-TRAIT-BASE B5: `Default` and the two methods that needed it.

#[test]
fn default_answers_for_a_type_with_no_value_in_hand() {
    let src = r#"
        struct P { v: i64 }
        impl Default for P { fn default() -> Self { P { v: 9i64 } } }

        fn make<T: Default>() -> T {
            val c: T = T::default()
            c
        }

        fn main() -> u64 {
            # Primitives reach the stdlib impls, the struct its own,
            # and nothing in either call says which.
            val a: u64 = make()
            val b: i64 = make()
            val f: bool = make()
            val p: P = make()
            println(a)
            println(b)
            println(f)
            val n: i64 = p.v
            println(n)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "default_trait");
}

#[test]
fn resize_fills_with_the_element_types_default() {
    // `resize` has to produce values for slots no element exists in
    // yet, so the type is the only thing that can answer. Its bound
    // lives on its own impl block, so an ordinary `Vec<T>` is
    // unaffected.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(7u64)
            v.resize(4u64)
            println(v.size())
            println(v.get(0u64))
            println(v.get(3u64))
            # Shrinking drops the tail and keeps the capacity.
            v.resize(1u64)
            println(v.size())
            0u64
        }
    "#;
    assert_stdout_consistent(src, "vec_resize_default");
}

#[test]
fn get_or_default_answers_for_a_key_that_is_not_there() {
    // The counting shape: no value to hand `get_or`, only a type.
    let src = r#"
        fn main() -> u64 {
            var counts: Dict<str, u64> = Dict::new()
            val first = counts.get_or_default("apple")
            println(first)
            counts.insert("apple", first + 1u64)
            val second = counts.get_or_default("apple")
            println(second)
            val missing = counts.get_or_default("pear")
            println(missing)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "dict_get_or_default");
}
