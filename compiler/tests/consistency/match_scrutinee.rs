//! Method contracts, closure shadowing, call-shaped match scrutinees,
//! `val`-bound match, and implicit impl parameters.

use super::harness::*;

// Design-by-Contract: `requires` / `ensures` lower to plain
// `Branch` + `Panic` blocks, so a contract-satisfying program runs
// identically on all backends including the IR VM lane (the panic
// arm is simply never branched to). These pin the passing path 4-way.
#[test]
fn contract_requires_and_ensures_round_trip() {
    let src = r#"
        fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
            ensures result * b == a
        {
            a / b
        }
        fn main() -> u64 {
            val q: i64 = divide(20i64, 4i64)
            q as u64
        }
    "#;
    assert_consistent(src, "contract_requires_and_ensures");
}

#[test]
fn contract_multiple_requires_clauses_round_trip() {
    let src = r#"
        fn clamp_add(a: i64, b: i64) -> i64
            requires a >= 0i64
            requires b >= 0i64
            ensures result >= a
        {
            a + b
        }
        fn main() -> u64 {
            clamp_add(30i64, 12i64) as u64
        }
    "#;
    assert_consistent(src, "contract_multiple_requires");
}

#[test]
fn contract_method_requires_round_trip() {
    let src = r#"
        struct Counter { n: i64 }
        impl Counter {
            fn bumped(self: Self, by: i64) -> i64
                requires by > 0i64
                ensures result > self.n
            {
                self.n + by
            }
        }
        fn main() -> u64 {
            val c = Counter { n: 40i64 }
            c.bumped(2i64) as u64
        }
    "#;
    assert_consistent(src, "contract_method_requires");
}


// Lexical scoping of the callee name — a function-typed local
// binding shadows a same-named top-level function. All three
// backends resolve the callee through their own tables (type
// checker `visit_call`, interpreter `evaluate_function_call`,
// lowering `resolve_call_target`), so each had to be fixed
// independently; 3-way agreement pins the shared rule.
//
// Before the fix, lowering resolved the global first and emitted a
// direct `Call` to the wrong body — with the wrong arity once the
// closure captured anything, which surfaced as a cranelift verifier
// error rather than a wrong answer.
#[test]
fn closure_shadows_same_named_function_round_trip() {
    let src = r#"
        fn f(n: i64) -> i64 { n * 10i64 }
        fn main() -> i64 {
            val a = f(2i64)
            val f: fn (i64) -> i64 = fn(x: i64) -> i64 { x + 1i64 }
            val b = f(2i64)
            a + b
        }
    "#;
    assert_consistent(src, "closure_shadows_same_named_function");
}

#[test]
fn capturing_closure_shadows_same_named_function_round_trip() {
    let src = r#"
        fn f(n: i64) -> i64 { n * 10i64 }
        fn main() -> i64 {
            val base = 100i64
            val a = f(2i64)
            val f: fn (i64) -> i64 = fn(x: i64) -> i64 { x + base }
            val b = f(2i64)
            a + b
        }
    "#;
    assert_consistent(src, "capturing_closure_shadows_same_named_function");
}

#[test]
fn non_function_local_does_not_shadow_function_round_trip() {
    let src = r#"
        fn g(n: i64) -> i64 { n + 1i64 }
        fn main() -> i64 {
            val h = 5i64
            g(h)
        }
    "#;
    assert_consistent(src, "non_function_local_does_not_shadow_function");
}

// --- AOT-MATCH-SCRUTINEE-EXPAND ------------------------------------
//
// A `match` whose scrutinee is a call to a free function returning an
// enum. AOT used to reject these ("`match` on scalar scrutinee only
// supports i64 / u64 / bool, got enum#0") while the interpreter ran
// them, so the shape the iterator protocol desugars into only worked
// when the producer happened to be a method. A free function has no
// receiver, so unlike the method path there are no receiver leaves to
// pass and no `&mut self` writeback to route back.

#[test]
fn match_scrutinee_is_a_function_call_returning_an_enum() {
    let src = r#"
        enum Step { Go(i64), Stop }

        fn step(n: i64) -> Step {
            if n < 3i64 { Step::Go(n + 1i64) } else { Step::Stop }
        }

        fn main() -> i64 {
            var total: i64 = 0i64
            var i: i64 = 0i64
            while val Step::Go(next) = step(i) {
                total = total + next
                i = next
            }
            total
        }
    "#;
    assert_consistent(src, "match_scrut_call_enum");
}

#[test]
fn match_scrutinee_is_a_call_with_computed_arguments() {
    // The arguments are lowered by the same path as any other call, so
    // this pins that they are evaluated rather than skipped.
    let src = r#"
        enum E { A(i64), B }

        fn f(n: i64, m: i64) -> E { E::A(n + m) }

        fn main() -> i64 {
            val k: i64 = 3i64
            match f(k * 2i64, k + 1i64) {
                E::A(v) => v,
                E::B => 0i64,
            }
        }
    "#;
    assert_consistent(src, "match_scrut_call_args");
}

#[test]
fn match_scrutinee_is_a_call_returning_a_stdlib_enum() {
    // `Option` / `Result` come from the core modules, so this also
    // exercises the module-qualified lookup inside the new path.
    let src = r#"
        fn find(n: i64) -> Option<i64> {
            if n > 0i64 { Option::Some(n * 2i64) } else { Option::None }
        }

        fn halve(a: i64) -> Result<i64, i64> {
            if a % 2i64 == 0i64 { Result::Ok(a / 2i64) } else { Result::Err(a) }
        }

        fn main() -> i64 {
            val a: i64 = match find(21i64) {
                Option::Some(v) => v,
                Option::None => 0i64,
            }
            # Tail position rather than a `val` binding: a match whose
            # arms *all* bind a payload cannot be used as a val rhs
            # yet, independently of the scrutinee (see
            # MATCH-LET-RHS-PAYLOAD-INFER in todo.md).
            match halve(a) {
                Result::Ok(v) => v,
                Result::Err(e) => e,
            }
        }
    "#;
    assert_consistent(src, "match_scrut_call_stdlib_enum");
}

#[test]
fn match_scrutinee_is_a_generic_function_call() {
    // Resolution goes through `resolve_call_target`, so a generic
    // callee monomorphises here the same as at any other call site.
    let src = r#"
        fn wrap<T>(x: T) -> Option<T> { Option::Some(x) }

        fn main() -> i64 {
            match wrap(9i64) {
                Option::Some(v) => v,
                Option::None => 0i64,
            }
        }
    "#;
    assert_consistent(src, "match_scrut_generic_call");
}

#[test]
fn match_scrutinee_call_writes_back_a_mut_argument() {
    // A free function has no `self` writeback, but a `&mut T`
    // parameter still has to be routed back to the caller's local.
    let src = r#"
        enum E { A(i64), B }

        fn bump(c: &mut i64) -> E {
            c = c + 1i64
            E::A(c)
        }

        fn main() -> i64 {
            var n: i64 = 4i64
            val r: i64 = match bump(&mut n) {
                E::A(v) => v,
                E::B => 0i64,
            }
            r + n
        }
    "#;
    assert_consistent(src, "match_scrut_call_mut_arg");
}

#[test]
fn match_scrutinee_call_returning_a_scalar_still_takes_the_scalar_path() {
    // The enum arm must not swallow calls that return a scalar — those
    // still go through the literal-comparison path below it.
    let src = r#"
        fn f(n: i64) -> i64 { n * 2i64 }
        fn p(n: i64) -> bool { n > 0i64 }

        fn main() -> i64 {
            val a: i64 = match f(3i64) {
                6i64 => 60i64,
                _ => 0i64,
            }
            val b: i64 = match p(1i64) {
                true => 1i64,
                false => 0i64,
            }
            a + b
        }
    "#;
    assert_consistent(src, "match_scrut_call_scalar");
}

// --- MATCH-LET-RHS-PAYLOAD-INFER -----------------------------------
//
// `val x = match e { A(v) => v, B(w) => w }` — a match used as a
// val/var right-hand side where *every* arm body is a name its own
// pattern binds. The lowering pass infers the binding's slot type with
// `value_scalar`, which is `&self` and so cannot introduce the pattern
// bindings and recurse the way the lowering-time `arm_body_type` does.
// It gave up, and the whole `val` was rejected with "could not infer
// scalar type for val/var rhs" even though lowering would have handled
// it. One arm with a literal body was enough to hide the problem,
// which is why the plain `Option::None => 0i64` shape always worked.
//
// The last residual — a *method-call* scrutinee (`val x = match
// h.get() { ... }`) — was resolved by teaching `scrutinee_enum_id` to
// peek through method calls with the same receiver-binding +
// method-registry lookup `value_scalar`'s `MethodCall` arm already
// used. It needs no `&mut self`: the registry is data lowered once per
// program, not state created during resolution.

#[test]
fn val_bound_match_infers_from_payload_bindings() {
    let src = r#"
        enum E { A(i64), B(i64) }

        fn produce() -> E { E::A(7i64) }

        fn main() -> i64 {
            val bound = E::B(4i64)
            val from_binding: i64 = match bound {
                E::A(v) => v,
                E::B(w) => w,
            }
            val from_call: i64 = match produce() {
                E::A(v) => v,
                E::B(w) => w,
            }
            from_binding + from_call
        }
    "#;
    assert_consistent(src, "val_match_payload_infer");
}

#[test]
fn val_bound_match_infers_without_an_annotation() {
    // The annotation is not what rescues it — the inference has to
    // stand on its own.
    let src = r#"
        enum E { A(i64), B(i64) }

        fn produce() -> E { E::B(4i64) }

        fn main() -> i64 {
            val x = match produce() {
                E::A(v) => v,
                E::B(w) => w,
            }
            x
        }
    "#;
    assert_consistent(src, "val_match_payload_infer_noann");
}

#[test]
fn val_bound_match_picks_the_payload_slot_the_body_names() {
    // `P::Pt(_, y) => y` takes the *second* payload, so the slot has to
    // come from the sub-pattern's position rather than always slot 0.
    let src = r#"
        enum P { Pt(i64, i64), Z(i64) }

        fn produce() -> P { P::Pt(1i64, 8i64) }

        fn main() -> i64 {
            val y: i64 = match produce() {
                P::Pt(_, second) => second,
                P::Z(z) => z,
            }
            y
        }
    "#;
    assert_consistent(src, "val_match_payload_slot");
}

#[test]
fn val_bound_match_distinguishes_two_instantiations_of_one_generic_enum() {
    // The enum is identified from the scrutinee, not from the pattern's
    // enum name: `Option<i64>` and `Option<u64>` share a base name and
    // are separate interned enums with different payload types. Reading
    // the type off the name would pick whichever was interned first.
    let src = r#"
        fn signed() -> Option<i64> { Option::Some(0i64 - 3i64) }
        fn unsigned() -> Option<u64> { Option::Some(70u64) }

        fn main() -> i64 {
            val a: i64 = match signed() {
                Option::Some(v) => v,
                Option::None => 0i64,
            }
            val b: u64 = match unsigned() {
                Option::Some(w) => w,
                Option::None => 0u64,
            }
            a + (b as i64)
        }
    "#;
    assert_consistent(src, "val_match_generic_instantiations");
}

#[test]
fn val_bound_match_on_a_stdlib_result_binds_both_arms() {
    let src = r#"
        fn halve(a: i64) -> Result<i64, i64> {
            if a % 2i64 == 0i64 { Result::Ok(a / 2i64) } else { Result::Err(a) }
        }

        fn main() -> i64 {
            val b: i64 = match halve(42i64) {
                Result::Ok(v) => v,
                Result::Err(e) => e,
            }
            b
        }
    "#;
    assert_consistent(src, "val_match_result_both_arms");
}

// The residual: scrutinees that are *method calls*. Every earlier test
// above has an identifier or function call on the left of `match`; a
// method call used to send `value_scalar` (which cannot resolve a
// method target without the `&mut self` machinery) back with nothing,
// so the val was rejected despite lowering being able to handle it.

#[test]
fn val_bound_match_on_a_method_call_scrutinee() {
    let src = r#"
        enum E { A(i64), B(i64) }

        struct Holder { n: i64 }

        impl Holder {
            fn get(self: Self) -> E { E::B(self.n) }
        }

        fn main() -> i64 {
            val h = Holder { n: 5i64 }
            val x: i64 = match h.get() {
                E::A(v) => v,
                E::B(w) => w,
            }
            x
        }
    "#;
    assert_consistent(src, "val_match_method_scrutinee");
}

#[test]
fn val_bound_match_on_a_mut_method_scrutinee_keeps_the_writeback() {
    // `&mut self` methods write the mutated receiver back after the
    // call; the loop below would return the wrong total if the
    // scrutinee path dropped the writeback half.
    let src = r#"
        enum E { A(i64), B(i64) }

        struct Counter { n: i64 }

        impl Counter {
            fn step(&mut self) -> E {
                self.n = self.n + 1i64
                if self.n % 2i64 == 0i64 { E::A(self.n) } else { E::B(self.n) }
            }
        }

        fn main() -> i64 {
            var c = Counter { n: 0i64 }
            var total = 0i64
            for i in 0u64 to 4u64 {
                val x: i64 = match c.step() {
                    E::A(v) => v,
                    E::B(w) => w,
                }
                total = total + x
            }
            total
        }
    "#;
    assert_consistent(src, "val_match_mut_method_writeback");
}

#[test]
fn val_bound_match_on_self_method_scrutinee_inside_a_method() {
    // `self.get()` — the receiver is the implicit `self` parameter,
    // which is an identifier like any other at lowering time.
    let src = r#"
        enum E { A(i64), B(i64) }

        struct Holder { n: i64 }

        impl Holder {
            fn get(self: Self) -> E { E::B(self.n) }
            fn twice(self: Self) -> i64 {
                val x: i64 = match self.get() {
                    E::A(v) => v,
                    E::B(w) => w,
                }
                x * 2i64
            }
        }

        fn main() -> i64 {
            val h = Holder { n: 5i64 }
            h.twice()
        }
    "#;
    assert_consistent(src, "val_match_self_method_scrutinee");
}

#[test]
fn val_bound_match_on_a_generic_method_scrutinee() {
    // The scrutinee method goes through the generic-method template
    // instantiation (`Result::map<U>`), not a plain registry hit — the
    // peek has to find the *instantiated* return type.
    let src = r#"
        fn halve(a: i64) -> Result<i64, i64> {
            if a % 2i64 == 0i64 { Result::Ok(a / 2i64) } else { Result::Err(a) }
        }

        fn main() -> i64 {
            val r: Result<i64, i64> = halve(42i64)
            val mapped = r.map(fn(x: i64) -> i64 { x + 1i64 })
            val b: i64 = match mapped {
                Result::Ok(v) => v,
                Result::Err(e) => e,
            }
            b
        }
    "#;
    assert_consistent(src, "val_match_generic_method_scrutinee");
}

#[test]
fn val_bound_match_without_annotation_on_a_method_call_scrutinee() {
    // The annotation is not what makes the inference work; the slot
    // type has to come from the payload's declared type alone.
    let src = r#"
        enum E { A(i64), B(i64) }

        struct Holder { n: i64 }

        impl Holder {
            fn get(self: Self) -> E { E::A(self.n) }
        }

        fn main() -> i64 {
            val h = Holder { n: 7i64 }
            val x = match h.get() {
                E::A(v) => v,
                E::B(w) => w,
            }
            x
        }
    "#;
    assert_consistent(src, "val_match_method_scrutinee_noann");
}

// The other half of the residual: a *field-access* receiver
// (`h.inner.get()`). `resolve_method_target` used to accept only bare
// identifier receivers, so the match scrutinee fell through to the
// scalar path and was rejected with "match on scalar scrutinee only
// supports i64 / u64 / bool" — for a value that is an enum. The
// receiver's leaf locals live inside the parent binding, so the
// synthesized struct binding must point at those same locals for the
// call args *and* the `&mut self` writeback to land correctly.

#[test]
fn val_bound_match_on_a_field_access_method_scrutinee() {
    let src = r#"
        enum E { A(i64), B(i64) }

        struct Inner { n: i64 }

        struct Holder { inner: Inner }

        impl Inner {
            fn get(self: Self) -> E { E::A(self.n) }
        }

        fn main() -> i64 {
            val h = Holder { inner: Inner { n: 6i64 } }
            val x: i64 = match h.inner.get() {
                E::A(v) => v,
                E::B(w) => w,
            }
            x
        }
    "#;
    assert_consistent(src, "val_match_field_scrutinee");
}

#[test]
fn val_bound_match_on_a_mut_field_access_scrutinee_keeps_the_writeback() {
    // `&mut self` on `h.inner` writes back into the *parent's* leaf
    // locals; if the synthesized receiver binding pointed anywhere
    // else, the loop below would return a stale total.
    let src = r#"
        enum E { A(i64), B(i64) }

        struct Inner { n: i64 }

        struct Holder { inner: Inner }

        impl Inner {
            fn step(&mut self) -> E {
                self.n = self.n + 1i64
                if self.n % 2i64 == 0i64 { E::A(self.n) } else { E::B(self.n) }
            }
        }

        fn main() -> i64 {
            var h = Holder { inner: Inner { n: 0i64 } }
            var total = 0i64
            for i in 0u64 to 4u64 {
                val x: i64 = match h.inner.step() {
                    E::A(v) => v,
                    E::B(w) => w,
                }
                total = total + x
            }
            total
        }
    "#;
    assert_consistent(src, "val_match_field_mut_writeback");
}

#[test]
fn val_bound_match_on_a_nested_field_access_scrutinee() {
    // The chain resolves through several levels of nesting before the
    // method's receiver appears.
    let src = r#"
        enum E { A(i64), B(i64) }

        struct Deep { n: i64 }

        struct Mid { deep: Deep }

        struct Top { mid: Mid }

        impl Deep {
            fn get(self: Self) -> E { E::B(self.n) }
        }

        fn main() -> i64 {
            val t = Top { mid: Mid { deep: Deep { n: 9i64 } } }
            val x: i64 = match t.mid.deep.get() {
                E::A(v) => v,
                E::B(w) => w,
            }
            x
        }
    "#;
    assert_consistent(src, "val_match_nested_field_scrutinee");
}

#[test]
fn compound_method_on_a_field_receiver_binds_with_val() {
    // `resolve_method_target` is shared with the compound-returning
    // val-rhs path — a field receiver must resolve there too, or the
    // upgrade below fails with "no method".
    let src = r#"
        struct Inner { n: i64 }

        struct Holder { inner: Inner }

        impl Inner {
            fn make(n: i64) -> Inner { Inner { n: n } }
            fn bump(self: Self) -> Inner { Inner { n: self.n + 1i64 } }
        }

        fn main() -> i64 {
            val h = Holder { inner: Inner::make(5i64) }
            val upgraded: Inner = h.inner.bump()
            upgraded.n
        }
    "#;
    assert_consistent(src, "compound_val_rhs_field_receiver");
}

// --- stdlib higher-order methods on generic enums -------------------
//
// `Option::map` / `Result::map` / `map_err` / `unwrap_or_else` ran on
// the interpreter but could not be lowered, so they were effectively
// interpreter-only. Three separate gaps stacked up:
//
//   1. `resolve_method_target` bailed out on an *enum* receiver in its
//      generic-method branch, so every caller that resolves a target
//      before choosing a call shape reported "cannot use a
//      compound-returning method in expression position; bind the
//      result with `val`" — to code that had already done that.
//   2. a method-only generic param mentioned solely inside a
//      function-typed parameter (`map<U>(f: fn (T) -> U)`) could not be
//      inferred: the argument's IR type is a bare U64 pointer.
//   3. that same `fn (T) -> U` parameter was lowered without applying
//      the active monomorphisation, so it arrived as
//      `Function([Generic(T)], Generic(U))`.

#[test]
fn option_map_round_trips_through_every_backend() {
    let src = r#"
        fn main() -> i64 {
            val some: Option<i64> = Option::Some(20i64)
            val mapped = some.map(fn(x: i64) -> i64 { x + 1i64 })
            val from_some: i64 = match mapped {
                Option::Some(v) => v,
                Option::None => 0i64,
            }

            val none: Option<i64> = Option::None
            val untouched = none.map(fn(x: i64) -> i64 { x + 1i64 })
            val from_none: i64 = match untouched {
                Option::Some(v) => v,
                Option::None => 100i64,
            }

            from_some + from_none
        }
    "#;
    assert_consistent(src, "option_map_backends");
}

#[test]
fn option_map_changes_the_payload_type() {
    // `U` differs from `T`, which is the whole point of `map` and the
    // case that needs the method-only param inferred from the
    // closure's declared return type.
    let src = r#"
        fn main() -> i64 {
            val some: Option<i64> = Option::Some(3i64)
            val flagged = some.map(fn(x: i64) -> bool { x > 1i64 })
            match flagged {
                Option::Some(v) => if v { 1i64 } else { 0i64 },
                Option::None => 0i64,
            }
        }
    "#;
    assert_consistent(src, "option_map_retype");
}

#[test]
fn result_map_and_map_err_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val ok: Result<i64, i64> = Result::Ok(20i64)
            val doubled = ok.map(fn(x: i64) -> i64 { x * 2i64 })
            val from_ok: i64 = match doubled {
                Result::Ok(v) => v,
                Result::Err(e) => e,
            }

            val err: Result<i64, i64> = Result::Err(5i64)
            val relabelled = err.map_err(fn(e: i64) -> i64 { e + 1i64 })
            val from_err: i64 = match relabelled {
                Result::Ok(v) => v,
                Result::Err(e) => e,
            }

            from_ok + from_err
        }
    "#;
    assert_consistent(src, "result_map_backends");
}

#[test]
fn option_unwrap_or_else_takes_a_zero_argument_closure() {
    // The function-typed parameter has no parameters at all, so the
    // substitution has to reach the return type on its own.
    let src = r#"
        fn main() -> i64 {
            val none: Option<i64> = Option::None
            val fallback: i64 = none.unwrap_or_else(fn() -> i64 { 9i64 })
            val some: Option<i64> = Option::Some(4i64)
            val kept: i64 = some.unwrap_or_else(fn() -> i64 { 9i64 })
            fallback + kept
        }
    "#;
    assert_consistent(src, "option_unwrap_or_else_backends");
}

// --- implicit impl type parameters ---------------------------------
//
// `impl Container<T>` re-uses the parameter `struct Container<T>`
// declared — the form the language reference documents. It did not
// work: `T` was parsed as a concrete type argument named `T`, so the
// reference's own example failed to type check ("Cannot unify
// Identifier(T) with Int64") and only the explicit `impl<T>
// Container<T>` compiled.
//
// The distinguishing rule is the *declaration*: a type argument
// becomes a parameter only if the struct / enum lists that name. `u8`
// in `impl Vec<u8>` lexes as a type keyword and can never match, so
// the concrete-args form is unaffected — which is what the
// `two_concrete_impls` test below guards, because the first attempt at
// this adopted the declaration wholesale and turned those impls into
// generic templates, losing their methods.

#[test]
fn implicit_impl_type_parameter_on_a_struct() {
    // Verbatim from docs/language.md.
    let src = r#"
        struct Container<T> {
            value: T
        }

        impl Container<T> {
            fn new(v: T) -> Self {
                Container { value: v }
            }
            fn get(self: Self) -> T {
                self.value
            }
        }

        fn main() -> i64 {
            val c: Container<i64> = Container::new(7i64)
            c.get()
        }
    "#;
    assert_consistent(src, "implicit_impl_struct");
}

#[test]
fn implicit_impl_type_parameter_on_an_enum() {
    let src = r#"
        enum Box<T> { Put(T), Empty }

        impl Box<T> {
            fn or(self: Self, fallback: T) -> T {
                match self {
                    Box::Put(v) => v,
                    Box::Empty => fallback,
                }
            }
        }

        fn main() -> i64 {
            val present: Box<i64> = Box::Put(4i64)
            val absent: Box<i64> = Box::Empty
            present.or(0i64) + absent.or(3i64)
        }
    "#;
    assert_consistent(src, "implicit_impl_enum");
}

#[test]
fn implicit_impl_type_parameter_carries_into_a_higher_order_method() {
    // The shape that motivated this: `f: fn (T) -> U` needs `T` to be
    // a parameter, not a type named `T`, before the function-typed
    // parameter can be lowered at all.
    let src = r#"
        enum Box<T> { Put(T), Empty }

        impl Box<T> {
            fn map<U>(self: Self, f: fn (T) -> U) -> Box<U> {
                match self {
                    Box::Put(v) => Box::Put(f(v)),
                    Box::Empty => Box::Empty,
                }
            }
        }

        fn main() -> i64 {
            val b: Box<i64> = Box::Put(4i64)
            val doubled = b.map(fn(x: i64) -> i64 { x * 2i64 })
            match doubled {
                Box::Put(v) => v,
                Box::Empty => 0i64,
            }
        }
    "#;
    assert_consistent(src, "implicit_impl_hof");
}

#[test]
fn two_concrete_impls_of_one_generic_struct_still_dispatch_separately() {
    // CONCRETE-IMPL: `impl C<u8>` and `impl C<i64>` are two distinct
    // specs, not templates. Treating the declaration's `T` as present
    // here would collapse them.
    let src = r#"
        struct C<T> { v: T }

        impl C<u8> { fn tag(self: Self) -> i64 { 1i64 } }
        impl C<i64> { fn tag(self: Self) -> i64 { 2i64 } }

        fn main() -> i64 {
            val a: C<i64> = C { v: 5i64 }
            a.tag()
        }
    "#;
    assert_consistent(src, "two_concrete_impls");
}

#[test]
fn explicit_and_implicit_impl_parameter_lists_agree() {
    // The two spellings have to produce the same program.
    let implicit = r#"
        struct Holder<T> { item: T }
        impl Holder<T> { fn get(self: Self) -> T { self.item } }
        fn main() -> i64 {
            val h: Holder<i64> = Holder { item: 11i64 }
            h.get()
        }
    "#;
    let explicit = r#"
        struct Holder<T> { item: T }
        impl<T> Holder<T> { fn get(self: Self) -> T { self.item } }
        fn main() -> i64 {
            val h: Holder<i64> = Holder { item: 11i64 }
            h.get()
        }
    "#;
    assert_consistent(implicit, "impl_param_implicit");
    assert_consistent(explicit, "impl_param_explicit");
}

// --- allocator behaviour that the backends do NOT share -------------
//
// Recorded, not endorsed — the same spirit as
// `u64_addition_still_wraps`. `assert_consistent` cannot express this
// because the whole point is that the backends disagree.
//
// The interpreter's `HeapManager` is a bump allocator: `next_addr`
// only moves forward and a `free` returns nothing to it. The AOT path
// is libc `malloc`, which hands the block straight back. So a program
// can observe which backend it is running on.
//
// This is why MEMORY_PROFILING defines every counter on the sizes and
// order the program *requested*, never on addresses or region layout:
// an address-derived metric could not be made to agree here, and a
// fragmentation number computed from the interpreter would describe
// the bump allocator rather than the program.

#[test]
fn const_patterns_compare_on_every_lane() {
    // MATCH-CONST-PATTERN: the type checker rewrites a const named in a
    // pattern to a literal pattern, so the backends only ever see the
    // literal form. Pinned at the top level, in a payload position, and
    // through a const that names another const.
    let src = r#"
        const K: u64 = 3u64
        const J: u64 = 4u64
        const L: u64 = K
        fn classify(n: u64) -> u64 {
            match n {
                K => 10u64,
                J => 20u64,
                _ => 0u64,
            }
        }
        fn nested(o: Option<u64>) -> u64 {
            match o {
                Option::Some(L) => 7u64,
                Option::Some(v) => v,
                Option::None => 0u64,
            }
        }
        fn main() -> u64 {
            val flat = classify(3u64) + classify(4u64) + classify(5u64)
            val deep = nested(Option::Some(3u64)) + nested(Option::Some(9u64))
            flat + deep
        }
    "#;
    assert_consistent(src, "const_patterns_compare");
}

#[test]
fn narrow_integers_can_be_matched_on_every_lane() {
    // CHAR-LITERAL-MATCH: a `u8` / `u32` / `i8` scrutinee, with char
    // literals narrowed to the byte they name (`'m'`), suffixed narrow
    // literals, `|`, ranges (half-open, so `'0'..':'` is the ten
    // digits), a payload position, and a `u8` covered by ranges alone
    // -- 256 values is small enough to be exhaustive without `_`.
    let src = r#"
        fn unit(c: u8) -> u64 {
            match c {
                'm' => 60u64,
                'h' => 3600u64,
                'd' => 86400u64,
                _ => 1u64,
            }
        }
        fn digit(c: u8) -> u64 {
            match c {
                '0'..':' => (c - '0') as u64,
                _ => 99u64,
            }
        }
        fn kind(k: u32) -> u64 {
            match k {
                1u32 => 10u64,
                2u32 | 3u32 => 20u64,
                _ => 0u64,
            }
        }
        fn signed(v: i8) -> u64 {
            match v {
                -1i8 => 1u64,
                0i8..10i8 => 2u64,
                _ => 3u64,
            }
        }
        fn all_bytes(b: u8) -> u64 {
            match b {
                0u8..128u8 => 1u64,
                128u8..255u8 => 2u64,
                255u8 => 3u64,
            }
        }
        fn payload(o: Option<u8>) -> u64 {
            match o {
                Option::Some('x') => 5u64,
                Option::Some(_) => 6u64,
                Option::None => 7u64,
            }
        }
        fn main() -> u64 {
            println("{unit('m')} {unit('h')} {unit('d')} {unit('z')}")
            println("{digit('7')} {digit('a')}")
            println("{kind(1u32)} {kind(3u32)} {kind(9u32)}")
            println("{signed(-1i8)} {signed(5i8)} {signed(-9i8)}")
            println("{all_bytes(5u8)} {all_bytes(200u8)} {all_bytes(255u8)}")
            println("{payload(Option::Some('x'))} {payload(Option::Some('y'))} {payload(Option::None)}")
            0u64
        }
    "#;
    assert_renders(
        src,
        "narrow_integer_match",
        "60 3600 86400 1\n7 99\n10 20 0\n1 2 3\n1 2 3\n5 6 7\n",
    );
}

#[test]
fn a_str_built_at_run_time_matches_its_literal_arm() {
    // The compiled lanes compared a `str` scrutinee against a literal
    // arm with `BinOp::Eq` -- the runtime handles, which are pointers.
    // A literal scrutinee shares the interned pointer, so the existing
    // tests passed; a `str` read out of a `String` never matched any
    // arm on JIT / AOT while the interpreter matched it. Same fix `==`
    // already had: compare the bytes (`StrEq`). Top level, `|`, and a
    // payload position.
    let src = r#"
        fn classify(s: str) -> u64 {
            match s {
                "from" => 1u64,
                "to" | "until" => 2u64,
                _ => 0u64,
            }
        }
        fn nested(o: Option<str>) -> u64 {
            match o {
                Option::Some("to") => 5u64,
                Option::Some(_) => 6u64,
                Option::None => 7u64,
            }
        }
        fn main() -> u64 {
            val whole = String::from_str("from=5 until=9")
            val a = whole.substring(0u64, 4u64)
            val b = whole.substring(7u64, 12u64)
            val t = String::from_str("to")
            val x = classify(a.to_str())
            val y = classify(b.to_str())
            val z = nested(Option::Some(t.to_str()))
            println("{x} {y} {z}")
            0u64
        }
    "#;
    assert_renders(src, "str_match_runtime_value", "1 2 5\n");
}

#[test]
fn enum_discriminants_cast_on_every_lane() {
    // ENUM-DISCRIMINANT: `e as T` becomes a match over the variants'
    // numbers (a bare path folds to the literal), so the backends see
    // nothing new. Auto-numbering continues from an explicit value
    // (`Apache = 10`, so `Epoch` is 11; `Neg = -1`, so `Zero` is 0), a
    // char literal is a number, and the operand can be a local, a call,
    // a field, or sit inside an arithmetic expression.
    let src = r#"
        enum Kind { Plain, Syslog, Datetime, Apache = 10, Epoch }
        enum Signed { Neg = -1, Zero, Pos }
        enum Byte { A = 'a', B }
        fn pick(n: u64) -> Kind {
            if n == 0u64 { Kind::Plain } elif n == 1u64 { Kind::Apache } else { Kind::Epoch }
        }
        struct Rec { kind: Kind, n: u64 }
        fn main() -> u64 {
            val k = pick(1u64)
            val a = k as u32
            val b = pick(2u64) as u64
            val c = Kind::Datetime as u8
            val r = Rec { kind: Kind::Syslog, n: 1u64 }
            val d = r.kind as u64
            val e = 100u64 + (pick(0u64) as u64)
            val s = Signed::Neg as i64
            val t = Byte::B as u8
            println("{a} {b} {c} {d} {e} {s} {t}")
            0u64
        }
    "#;
    assert_renders(src, "enum_discriminant_cast", "10 11 2 1 100 -1 98\n");
}

#[test]
fn enum_struct_variants_on_every_lane() {
    // ENUM-STRUCT-VARIANT: the type checker turns `E::A { .. }` into the
    // tuple-variant construction and `E::A { x, .. }` into the positional
    // pattern, so the backends see nothing new. Pinned: fields written out
    // of order and across lines, a literal sub-pattern with `..`, `if val`,
    // and a struct variant nested in an `Option`.
    let src = r#"
        enum Rec {
            Syslog { host: u64, tag: u64 },
            Apache { status: u64, bytes: u64, client: u64 },
            Plain,
        }
        fn make(n: u64) -> Rec {
            if n == 0u64 {
                Rec::Syslog { tag: 7u64, host: 3u64 }
            } elif n == 1u64 {
                Rec::Apache {
                    status: 404u64,
                    bytes: 10u64,
                    client: 9u64,
                }
            } else {
                Rec::Plain
            }
        }
        fn describe(r: Rec) -> u64 {
            match r {
                Rec::Syslog { host, tag } => host * 100u64 + tag,
                Rec::Apache { status: 404u64, .. } => 1u64,
                Rec::Apache { status, bytes, .. } => status + bytes,
                Rec::Plain => 0u64,
            }
        }
        fn main() -> u64 {
            val a = describe(make(0u64))
            val b = describe(make(1u64))
            val c = describe(make(2u64))
            val r = Rec::Apache { status: 200u64, bytes: 5u64, client: 1u64 }
            val d = describe(r)
            val e = if val Rec::Syslog { host, .. } = make(0u64) { host } else { 99u64 }
            val o: Option<Rec> = Option::Some(Rec::Syslog { host: 1u64, tag: 2u64 })
            val f = match o {
                Option::Some(Rec::Syslog { tag, .. }) => tag,
                _ => 0u64,
            }
            println("{a} {b} {c} {d} {e} {f}")
            0u64
        }
    "#;
    assert_renders(src, "enum_struct_variant", "307 1 0 205 3 2\n");
}
