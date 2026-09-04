//! Compound bindings and fields, UTF-8 literals, str identity, the
//! stdlib allocator wrappers, and narrow-int AOT.

use super::harness::*;

#[test]
fn a_compound_field_binds_to_a_name() {
    // `val inner = o.i` used to fail with "val/var rhs produced no
    // value" on both compiled backends — struct values live in leaf
    // locals, not in the IR value graph, so the scalar let path had
    // nothing to store. The binding now adopts the field's own leaf
    // locals.
    //
    // Adoption, not copying: the interpreter shares the reference for
    // compound values, so a write through either name has to be
    // visible from the other. Both directions are exercised below,
    // which is also what makes this test meaningful — a copying
    // implementation passes the read-only half and fails here.
    let src = r#"
        struct Inner { a: u64, b: u64 }
        struct Mid { i: Inner, t: (u64, u64) }
        struct Outer { m: Mid, n: u64 }

        fn main() -> u64 {
            var o: Outer = Outer {
                m: Mid { i: Inner { a: 1u64, b: 2u64 }, t: (3u64, 4u64) },
                n: 5u64,
            }
            var mid: Mid = o.m
            val inner: Inner = o.m.i
            val pair: (u64, u64) = o.m.t
            o.m.i.a = 10u64        # write through the field, read via `inner`
            var deep: Inner = mid.i
            deep.b = 20u64         # write through the binding, read via the field
            inner.a + inner.b + pair.0 + pair.1 + o.m.i.b + o.n
        }
    "#;
    // 10 + 20 + 3 + 4 + 20 + 5 = 62. A copying implementation would
    // read the pre-write values and land elsewhere.
    assert_eq!(interpreter_value(src) & 0xff, 62);
    assert_consistent(src, "compound_field_binding");
}

#[test]
fn a_compound_binding_shares_storage_under_a_new_name() {
    // `var y: Inner = x` hit the same "val/var rhs produced no value"
    // wall as a field read, and takes the same fix: the new name
    // adopts the existing leaf locals. Sharing (not copying) is what
    // the interpreter does, so the write below is visible through
    // both names on every backend.
    let src = r#"
        struct Inner { a: u64 }
        fn main() -> u64 {
            var x: Inner = Inner { a: 1u64 }
            var y: Inner = x
            y.a = 9u64
            x.a
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 9);
    assert_consistent(src, "compound_binding_alias");
}

#[test]
fn a_compound_field_prints_and_interpolates() {
    // `println(o.i)` reported "println accepts only scalar values …
    // or identifiers referring to struct / tuple bindings"; the same
    // blind spot hit `__builtin_to_string` (interpolation) and, via
    // the `Display` rewrite `println(v)` -> `println(v.to_str())`,
    // any `String`-typed field. All three read the field's leaf tree
    // now, which is what an identifier binding already handed over.
    let src = r#"
        struct Inner { a: u64, b: u64 }
        struct Outer { i: Inner, t: (u64, bool), n: u64 }
        struct Named { name: String, n: u64 }

        fn main() -> u64 {
            val o: Outer = Outer { i: Inner { a: 1u64, b: 2u64 }, t: (7u64, true), n: 5u64 }
            var x: Named = Named { name: String::from_str("hi"), n: 3u64 }
            println(o.i)
            println(o.t)
            println(x.name)
            println("inner={o.i} tup={o.t} name={x.name}")
            0u64
        }
    "#;
    assert_eq!(
        interpreter_stdout(src, "compound_field_print_pin", true),
        "Inner { a: 1, b: 2 }\n(7, true)\nhi\ninner=Inner { a: 1, b: 2 } tup=(7, true) name=hi\n",
        "a compound field rendered differently than an equivalent binding"
    );
    assert_stdout_consistent(src, "compound_field_print");
}

#[test]
fn a_method_that_never_reads_its_receiver_still_compiles() {
    // The implicit `&self` / `&mut self` receiver is matched by token
    // text and never interned, so a program where **no source line
    // writes `self` at all** left the interner without the symbol —
    // and the receiver parameter then went unmaterialised while its
    // cranelift block param stayed (`param local not declared`), or
    // the next parameter bound to the receiver's type instead.
    //
    // Hence the deliberate shape here: not one method body mentions
    // `self`, and `add` takes a parameter after the receiver so a
    // dropped receiver shifts it onto the struct type. Adding any
    // `self.field` read anywhere in this source interns the symbol
    // and the whole program stops exercising the bug — which is why
    // it only ever showed up without the core modules, whose sources
    // interned `self` for everyone else.
    //
    // Driven through the no-core column directly: `assert_consistent`
    // falls back to the core-aware path on failure and would hide it.
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct S { a: u64, b: u64 }
        impl S {
            fn konst(&self) -> u64 { 7u64 }
            fn bump(&mut self) { }
            fn add(&self, n: u64) -> u64 { n + 1u64 }
        }
        fn main() -> u64 {
            var s = S { a: 1u64, b: 2u64 }
            s.bump()
            s.konst() + s.add(2u64)
        }
    "#;
    assert_eq!(
        try_compiler_exit_code(src, "receiver_unread_no_core", false),
        Some(10),
        "AOT compile without core modules dropped the implicit receiver"
    );
    assert_consistent(src, "receiver_unread");
}

#[test]
fn struct_fields_accept_non_literal_initialisers() {
    // A struct-typed field used to require a nested struct *literal*
    // on the rhs. Calls and existing bindings now write into the same
    // leaf locals, so all four shapes coexist in one literal.
    let src = r#"
        struct Inner { a: u64, b: u64 }
        struct Outer { i: Inner, n: u64 }

        fn make() -> Inner { Inner { a: 4u64, b: 5u64 } }

        fn build() -> Outer { Outer { i: make(), n: 1u64 } }

        fn main() -> u64 {
            val src: Inner = Inner { a: 10u64, b: 20u64 }
            val from_call: Outer = Outer { i: make(), n: 1u64 }
            val from_ident: Outer = Outer { i: src, n: 2u64 }
            val from_literal: Outer = Outer { i: Inner { a: 1u64, b: 1u64 }, n: 3u64 }
            val from_tail: Outer = build()
            from_call.i.a + from_call.i.b + from_call.n
              + from_ident.i.a + from_ident.i.b + from_ident.n
              + from_literal.i.a + from_literal.i.b + from_literal.n
              + from_tail.i.a + from_tail.i.b + from_tail.n
        }
    "#;
    // 10 + 32 + 5 + 10 = 57. A leaf landing in the wrong local shows
    // up as a different sum rather than a crash, so pin the value.
    assert_eq!(interpreter_value(src) & 0xff, 57);
    assert_consistent(src, "struct_field_non_literal_init");
}

#[test]
fn a_string_field_can_be_built_by_its_associated_function() {
    // The motivating case: `String` has no literal form — the only
    // way to make one is `String::new()` / `String::from_str(...)` —
    // so a struct with a `String` field could not be compiled at all.
    // `Vec::new()` covers the generic-template registry alongside it
    // (`String::from_str` resolves through the non-generic one).
    let src = r#"
        struct Named { name: String, n: u64 }
        struct Bag { v: Vec<u8>, n: u64 }

        fn main() -> u64 {
            var a: Named = Named { name: String::new(), n: 1u64 }
            a.name.push_char('x')
            a.name.push_char('y')
            var b: Named = Named { name: String::from_str("zzz"), n: 2u64 }
            var c: Bag = Bag { v: Vec::new(), n: 4u64 }
            c.v.push(1u8)
            a.name.len() + b.name.len() + c.v.size() + a.n + b.n + c.n
        }
    "#;
    // 2 + 3 + 1 + 1 + 2 + 4 = 13.
    assert_eq!(interpreter_value(src) & 0xff, 13);
    assert_consistent(src, "struct_field_string_init");
}

#[test]
fn a_tuple_field_accepts_the_same_initialisers_a_struct_field_does() {
    // Tuple-typed slots were the half of the literal path still stuck
    // on "must be a literal". They now share the rhs shapes struct
    // slots take: an existing binding, a tuple-typed field, and a
    // tuple-returning plain or method call.
    //
    // Every element carries a bool as well as a number, and the one
    // false is deliberate: a slot filled from the wrong source, or an
    // element pair landing swapped, shows up as a wrong sum rather
    // than a crash.
    let src = r#"
        struct Holder { t: (u64, bool), n: u64 }
        struct Src { pair: (u64, bool) }

        impl Src {
            fn get(&self) -> (u64, bool) { (self.pair.0 + 1u64, self.pair.1) }
        }

        fn make_pair() -> (u64, bool) { (7u64, true) }

        fn main() -> u64 {
            val src: Src = Src { pair: (3u64, true) }
            val existing: (u64, bool) = (5u64, false)
            val a: Holder = Holder { t: (1u64, true), n: 1u64 }
            val b: Holder = Holder { t: make_pair(), n: 2u64 }
            val c: Holder = Holder { t: existing, n: 3u64 }
            val d: Holder = Holder { t: src.pair, n: 4u64 }
            val e: Holder = Holder { t: src.get(), n: 5u64 }
            var total: u64 = a.t.0 + b.t.0 + c.t.0 + d.t.0 + e.t.0
                + a.n + b.n + c.n + d.n + e.n
            if a.t.1 { total = total + 100u64 }
            if b.t.1 { total = total + 100u64 }
            if c.t.1 { total = total + 1000u64 }
            if d.t.1 { total = total + 100u64 }
            if e.t.1 { total = total + 100u64 }
            total
        }
    "#;
    // 20 (first elements) + 15 (n) + 400 (four true bools, and *not*
    // the 1000 that `existing`'s false would trigger).
    assert_eq!(interpreter_value(src), 435);
    assert_consistent(src, "tuple_field_initialisers");
}

#[test]
fn a_tuple_enum_payload_can_be_built_by_a_call() {
    // Payload slots share the storage helper with struct fields on the
    // tuple side too.
    let src = r#"
        enum E { P((u64, bool)), None }
        fn make_pair() -> (u64, bool) { (7u64, true) }
        fn main() -> u64 {
            val e: E = E::P(make_pair())
            match e {
                E::P(t) => t.0,
                E::None => 0u64,
            }
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 7);
    assert_consistent(src, "tuple_payload_call_init");
}

#[test]
fn a_struct_field_can_be_built_by_a_method_call() {
    // The last rhs shape a struct-typed slot did not take. It needs
    // more than the plain-call path: the receiver's leaf scalars go in
    // front of the arguments, and a `&mut self` callee returns its
    // mutated receiver leaves *after* the result, so those slots have
    // to be appended to the destination list.
    //
    // `bump_and_copy` is the case that pins the writeback half:
    // `mutable.a` is read after the literal is built, so a lost
    // writeback changes the answer rather than crashing.
    let src = r#"
        struct Inner { a: u64, b: u64 }
        struct Outer { i: Inner, n: u64 }
        enum Holder { With(Inner), Empty }

        impl Inner {
            fn doubled(&self) -> Inner { Inner { a: self.a * 2u64, b: self.b * 2u64 } }
            fn plus(&self, k: u64) -> Inner { Inner { a: self.a + k, b: self.b + k } }
            fn bump_and_copy(&mut self, k: u64) -> Inner {
                self.a = self.a + k
                Inner { a: self.a, b: self.b }
            }
        }

        fn main() -> u64 {
            var src: Inner = Inner { a: 1u64, b: 2u64 }
            val o1: Outer = Outer { i: src.doubled(), n: 1u64 }
            val o2: Outer = Outer { i: src.plus(10u64), n: 2u64 }
            val h: Holder = Holder::With(src.doubled())
            var mutable: Inner = Inner { a: 5u64, b: 6u64 }
            val o3: Outer = Outer { i: mutable.bump_and_copy(3u64), n: 3u64 }
            val from_h: u64 = match h {
                Holder::With(i) => i.a + i.b,
                Holder::Empty => 0u64,
            }
            o1.i.a + o1.i.b + o2.i.a + o2.i.b + from_h + o3.i.a + mutable.a
        }
    "#;
    // 6 + 23 + 6 + 8 + 8 = 51, the last two terms being the returned
    // copy and the written-back receiver.
    assert_eq!(interpreter_value(src) & 0xff, 51);
    assert_consistent(src, "struct_field_method_init");
}

#[test]
fn a_string_field_can_be_built_by_a_method_call() {
    // The shape that motivated it: `String` values come out of methods
    // as often as associated functions.
    let src = r#"
        struct Named { name: String, n: u64 }
        fn main() -> u64 {
            val src: String = String::from_str("abc")
            var a: Named = Named { name: src.to_string(), n: 1u64 }
            a.name.push_char('!')
            a.name.len() + a.n
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 5);
    assert_consistent(src, "struct_field_string_method_init");
}

#[test]
fn an_enum_payload_can_be_built_by_a_call() {
    // Enum payload slots share the same storage helper as struct
    // fields, so they pick up the call shape too.
    let src = r#"
        struct Inner { a: u64, b: u64 }
        enum Holder { With(Inner), Empty }

        fn make() -> Inner { Inner { a: 4u64, b: 5u64 } }

        fn main() -> u64 {
            val h: Holder = Holder::With(make())
            match h {
                Holder::With(i) => i.a + i.b,
                Holder::Empty => 0u64,
            }
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 9);
    assert_consistent(src, "enum_payload_call_init");
}

#[test]
fn multibyte_utf8_string_literals_keep_their_bytes() {
    // The lexer used to push every inner byte of a string literal
    // `as char`, re-encoding each byte of a multi-byte scalar as its
    // own code point: `"♠"` (3 source bytes) reached every backend as
    // 6 mojibake bytes. **Agreement alone cannot see this** — all four
    // backends consumed the same broken literal — so this test pins
    // the values as well: byte lengths (`__builtin_str_len` counts
    // bytes) and equality against `\u{HEX}`, which decodes through a
    // separate lexer path and was always correct.
    let src = r#"
        fn main() -> u64 {
            var score: u64 = 0u64
            val spade = "♠"
            if __builtin_str_len(spade) == 3u64 { score = score + 1u64 }
            if spade == "\u{2660}" { score = score + 2u64 }
            if __builtin_str_len("日本語") == 9u64 { score = score + 4u64 }
            if __builtin_str_len("😀") == 4u64 { score = score + 8u64 }
            val mixed = "a♠b"
            if __builtin_str_len(mixed) == 5u64 { score = score + 16u64 }
            if mixed == "a".concat(spade).concat("b") { score = score + 32u64 }
            score
        }
    "#;
    assert_eq!(
        interpreter_value(src) & 0xff,
        63,
        "a multi-byte UTF-8 literal lost or gained bytes on the way in"
    );
    assert_consistent(src, "utf8_string_literals");
}

#[test]
fn multibyte_utf8_string_literals_print_as_written() {
    // The rendering half of the same bug: `println("♠")` emitted the
    // Latin-1 re-encoding. Pinned against the expected text, not just
    // across backends, for the reason above.
    let src = r#"
        fn main() -> u64 {
            println("a♠b")
            println("日本語 {1u64 + 1u64}")
            0u64
        }
    "#;
    assert_eq!(
        interpreter_stdout(src, "utf8_print_pin", false),
        "a♠b\n日本語 2\n",
        "a multi-byte UTF-8 literal was re-encoded on its way to stdout"
    );
    assert_stdout_consistent(src, "utf8_print");
}

#[test]
fn str_equality_compares_content_not_handles() {
    // `a == b` on two `str` values compares their bytes. It used to
    // compare the runtime handles everywhere except the tree-walker,
    // which are pointers: `"h".concat("i") == "hi"` was true on the
    // interpreter and false once compiled — a wrong answer that type
    // checked, and the kind of divergence `--all-backends` exists for.
    //
    // Covers both directions, both operators, empty strings, and
    // unequal lengths. The three deliberate false cases add 1000 each,
    // so any of them firing is unmistakable in the exit code.
    let src = r#"
        fn check() -> u64 {
            var score: u64 = 0u64
            val hi = "hi"
            val built = "h".concat("i")
            val other = "ho"
            val empty = ""
            val empty2 = "".concat("")
            val longer = "hii"

            if built == hi { score = score + 1u64 }
            if hi == hi { score = score + 2u64 }
            if empty == empty2 { score = score + 4u64 }
            if built != other { score = score + 8u64 }
            if hi != longer { score = score + 16u64 }
            if other == hi { score = score + 1000u64 }
            if built != hi { score = score + 1000u64 }
            if longer == hi { score = score + 1000u64 }
            score
        }

        fn main() -> u64 { check() }
    "#;
    // 1 + 2 + 4 + 8 + 16, and none of the 1000s.
    assert_consistent(src, "str_eq_content");
}

#[test]
fn str_equality_is_first_class_in_value_positions() {
    // R3 (2026-08-16): `str == str` used to type-check only inside
    // if/while conditions — the checker had no comparison rule for
    // `String` operands, so a value position (`val x: bool = a == b`)
    // was rejected with E0002 while the same expression in an `if`
    // sailed through unchecked (if conditions are not validated).
    // The comparison rule is now first-class; this pins the value
    // position on every backend. The false cases add 1000 each.
    let src = r#"
        fn main() -> u64 {
            var score: u64 = 0u64
            val hi = "hi"
            val built = "h".concat("i")
            val other = "ho"

            val equal: bool = built == hi
            val not_equal: bool = built != other
            val wrong: bool = built != hi
            if equal { score = score + 1u64 }
            if not_equal { score = score + 2u64 }
            if wrong { score = score + 1000u64 }
            score
        }
    "#;
    assert_consistent(src, "str_eq_value_pos");
}

// if/elif conditions were never type-checked — `visit_if_elif_else`
// validated only the branches, so `if 42u64 { ... }` sailed through
// and every expression inside a condition escaped validation (the
// same gap through which `str == str` used to reach the lowering
// without a checker rule). The condition check is on now; the tests
// below pin the rejection and the generic-equality rules that had to
// become first-class for the stdlib to keep checking (Dict::get's
// `existing == key` compares two same-`K` values, which the checker
// previously never saw because conditions were skipped).


#[test]
fn if_conditions_must_be_bool() {
    let errors = type_check_errors(
        "fn main() -> u64 {\n    if 42u64 { 1u64 } else { 0u64 }\n}\n",
    );
    let rendered = errors.join("\n");
    assert!(
        rendered.contains("expected bool, but got u64"),
        "unexpected diagnostics: {rendered}"
    );
    // elif conditions are checked the same way.
    let errors = type_check_errors(
        "fn main() -> u64 {\n    if true { 1u64 } elif 7u64 { 2u64 } else { 0u64 }\n}\n",
    );
    let rendered = errors.join("\n");
    assert!(
        rendered.contains("expected bool, but got u64"),
        "unexpected diagnostics: {rendered}"
    );
}

#[test]
fn generic_equality_is_instantiated_per_type() {
    // `a == b` on two same-`T` values had no checker rule (unchecked
    // conditions hid the absence), so `fn same<T>(a: T, b: T) -> bool`
    // was rejected outright. Now it type-checks and each instantiation
    // compares in the concrete type — including str, whose bytes must
    // be compared (STR-EQ), not the handles.
    let src = r#"
        fn same<T>(a: T, b: T) -> bool { a == b }
        fn main() -> u64 {
            var score: u64 = 0u64
            val a = 1u64
            val s1 = "x"
            val s2 = "x"
            val other = "y"
            if same(a, 1u64) { score = score + 1u64 }
            if same(s1, s2) { score = score + 2u64 }
            if !same(s1, other) { score = score + 4u64 }
            if !same(a, 2u64) { score = score + 8u64 }
            score
        }
    "#;
    assert_consistent(src, "generic_eq_per_type");
}

#[test]
fn generic_functions_instantiate_per_type_argument() {
    // A generic function called with two different concrete types
    // used to resolve the second call to the *first* instantiation:
    // `id(1u64)` then `id("hello")` called the u64 body with a str
    // handle and returned garbage on every backend. The lowering
    // registered instances under the bare template name, so the
    // bare-name lookup in `resolve_call_target` found the first one;
    // instances now live under the mangled name only and the generic
    // template is resolved before the plain lookup. The IR VM's
    // per-occurrence literal materialisation made the old wrong
    // answer visible (handles differed), while .rodata inlining hid
    // it in the compiled backends.
    let src = r#"
        fn id<T>(x: T) -> T { x }
        fn main() -> u64 {
            val a = id(1u64)
            val s = id("hello")
            println(s)
            a
        }
    "#;
    assert_stdout_consistent(src, "generic_two_instantiations");
}

#[test]
fn the_interpreter_jit_compares_str_content_too() {
    // `assert_consistent`'s lite path returns as soon as the
    // tree-walker, the compiler-side JIT, the AOT binary and the IR VM
    // agree — the interpreter's own JIT is only reached on the full
    // path, so the test above passes with its str-equality arm removed.
    // This one drives that column directly.
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn cmp() -> u64 {
            val a = "hi"
            val b = "h".concat("i")
            if a == b { 42u64 } else { 1u64 }
        }
        fn main() -> u64 { cmp() }
    "#;
    assert_eq!(
        jit_exit_code(src, "str_eq_interp_jit", true),
        42,
        "the interpreter JIT compared str handles rather than content"
    );
}

#[test]
fn aot_allocator_default_and_current_round_trip() {
    // #121 Phase B-min: `__builtin_default_allocator()` returns
    // the sentinel u64 = 0; `__builtin_current_allocator()` reads
    // the top of the runtime active-allocator stack
    // (the `toylang_rt` crate). The
    // `with allocator = expr { body }` scope emits push/pop calls
    // around the body so a `__builtin_current_allocator()` call
    // inside the body yields the pushed handle.
    //
    // Outside any `with`, default == current (both 0 = default
    // sentinel). Inside `with allocator = a { ... }`, current
    // equals a (the pushed handle). After the body exits, current
    // returns to default. Final exit 42 means every check matched.
    // Uses Allocator-to-Allocator `==` (the only comparison the
    // interpreter accepts on the opaque handle type).
    let src = r#"
        fn main() -> u64 {
            val outside_default = __builtin_default_allocator()
            val outside_current = __builtin_current_allocator()
            val inside_current = with allocator = outside_default {
                __builtin_current_allocator()
            }
            val after_current = __builtin_current_allocator()
            if outside_default == outside_current {
                if inside_current == outside_default {
                    if after_current == outside_default { 42u64 } else { 4u64 }
                } else { 3u64 }
            } else { 1u64 }
        }
    "#;
    assert_consistent(src, "aot_allocator_default_and_current_round_trip");
}

#[test]
fn stdlib_alloc_with_struct() {
    // STDLIB-alloc-trait: `core/std/allocator.t` defines wrapper
    // structs (`Global` / `Arena` / `FixedBuffer`) over the
    // primitive `Allocator` handle, plus `trait Alloc` that they
    // all impl. `with allocator = arena { ... }` accepts a
    // wrapper struct and auto-extracts its single
    // `Allocator`-typed field at lowering time, so user code
    // doesn't have to write `with allocator = arena.h { ... }`
    // (or call any `handle()` method).
    //
    // Coverage:
    //   - Arena: two-allocation `with` body, drop fires at scope exit
    //   - FixedBuffer(16): two 8-byte allocations succeed,
    //     third 1-byte allocation hits the quota → null
    //   - both wrapper kinds exercise the auto-extract path
    let src = r#"
        fn main() -> u64 {
            val arena = Arena::new()
            with allocator = arena {
                val p1: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p1) { return 1u64 }
            }

            val fb = FixedBuffer::new(16u64)
            with allocator = fb {
                val p2: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p2) { return 2u64 }
                val p3: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p3) { return 3u64 }
                val p4: ptr = __builtin_heap_alloc(1u64)
                if !__builtin_ptr_is_null(p4) { return 4u64 }
            }
            42u64
        }
    "#;
    assert_consistent(src, "stdlib_alloc_with_struct");
}

#[test]
fn stdlib_alloc_trait_methods() {
    // STDLIB-alloc-trait: `arena.alloc(8u64)` / `fb.alloc(...)`
    // dispatch through the `Alloc` trait. Each method body
    // delegates via `with allocator = self.h { __builtin_heap_alloc(size) }`,
    // so the actual allocation routes through the runtime
    // active-allocator stack and hits the right backend.
    //
    // Verifies arena unrestricted alloc + fixed_buffer quota
    // rejection through the trait method path (parallel to
    // `stdlib_alloc_with_struct` which exercises the `with`
    // path).
    let src = r#"
        fn main() -> u64 {
            val arena = Arena::new()
            val p1: ptr = arena.alloc(8u64)
            if __builtin_ptr_is_null(p1) { return 1u64 }
            val p2: ptr = arena.alloc(8u64)
            if __builtin_ptr_is_null(p2) { return 2u64 }

            val fb = FixedBuffer::new(8u64)
            val q1: ptr = fb.alloc(8u64)
            if __builtin_ptr_is_null(q1) { return 3u64 }
            val q2: ptr = fb.alloc(1u64)
            if !__builtin_ptr_is_null(q2) { return 4u64 }
            42u64
        }
    "#;
    assert_consistent(src, "stdlib_alloc_trait_methods");
}

// `aot_arena_drop_releases_and_reuses` and
// `aot_arena_and_fixed_buffer_allocators_round_trip` were removed
// when the runtime arena / fixed_buffer infrastructure was retired.
// The reset / reuse / quota-enforcement contracts are now exercised
// by `aot_arena_bytes_used_and_reset` and
// `aot_fixed_buffer_introspection` against the toylang stdlib.

#[test]
fn aot_with_allocator_early_return_pops_stack() {
    // An early `return` from inside a `with allocator = ...` body
    // must still emit the matching pop for every active scope.
    // Without this cleanup the active-allocator stack leaks the
    // pushed handle and the caller sees the wrong allocator
    // after the helper function returns.
    let src = r#"
        fn helper() -> u64 {
            val a = Arena::new()
            with allocator = a {
                return 7u64
            }
            0u64
        }

        fn main() -> u64 {
            val r = helper()
            val cur = __builtin_current_allocator()
            val def = __builtin_default_allocator()
            if cur != def { return 1u64 }
            if r != 7u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "aot_with_allocator_early_return_pops_stack");
}

#[test]
fn narrow_int_aot_round_trip() {
    // NUM-W-AOT (T5 follow-up to Phase 5): the AOT compiler now
    // models the narrow integer types. The previous version of
    // this test asserted that AOT *rejected* narrow-int code
    // with a precise diagnostic; since the IR / codegen
    // widening landed (T5), narrow-int programs compile and
    // run.
    //
    // Asserts AOT exit code matches the expected value (250 +
    // -1000_as_u32 = 4294966546, & 0xff = 18). Interpreter
    // path is also asserted via `assert_consistent` once that
    // path tolerates the same arithmetic. JIT remains silent-
    // fallback (NUM-W-JIT not yet done) — exit code through
    // the JIT helper still matches because it lowers through
    // the interpreter.
    let src = r#"
        fn main() -> u64 {
            val a: u8 = 200u8 + 50u8
            val b: i32 = -1000i32
            val c: u32 = b as u32
            a as u64 + c as u64
        }
    "#;
    assert_consistent(src, "narrow_int_aot_round_trip");
}

#[test]
fn narrow_int_array_packing_round_trip() {
    // NUM-W-AOT-pack Phase 1: homogeneous scalar element arrays
    // pack to the actual scalar byte size — `[u8; N]` to N
    // bytes, `[u16; N]` to 2N, `[u32; N]` to 4N, instead of the
    // previous uniform 8N. The lowering's leaf-index addressing
    // (byte_offset = leaf_idx * elem_stride_bytes) lands on the
    // correct narrow slot because `elem_stride_bytes` now
    // returns the per-width size for scalar element types.
    //
    // This test exercises read + write + loop sum across all six
    // narrow widths with const + runtime indexing through
    // interpreter / JIT (silent fallback) / AOT. Each backend
    // must agree on exit 42; any address-arithmetic regression
    // would surface as a wrong sum (the AOT cranelift `load.I8`
    // / `load.I16` / `load.I32` reads at a wrong offset and the
    // checksum wouldn't land on 42).
    let src = r#"
        fn main() -> u64 {
            var u8a: [u8; 4] = [10u8, 20u8, 30u8, 40u8]
            u8a[1] = 50u8
            var u16a: [u16; 4] = [100u16, 200u16, 300u16, 400u16]
            u16a[2] = 999u16
            var u32a: [u32; 4] = [1000u32, 2000u32, 3000u32, 4000u32]
            u32a[3] = 9999u32

            var i8a: [i8; 4] = [-1i8, -2i8, -3i8, -4i8]
            var i16a: [i16; 4] = [-100i16, -200i16, -300i16, -400i16]
            var i32a: [i32; 4] = [-1000i32, -2000i32, -3000i32, -4000i32]

            var su8: u64 = 0u64
            var i: u64 = 0u64
            while i < 4u64 {
                su8 = su8 + (u8a[i] as u64)
                i = i + 1u64
            }
            # 10 + 50 + 30 + 40 = 130
            if su8 != 130u64 { return 1u64 }

            if u16a[2] != 999u16 { return 2u64 }
            if u32a[3] != 9999u32 { return 3u64 }
            if i8a[0] != -1i8 { return 4u64 }
            if i16a[3] != -400i16 { return 5u64 }
            if i32a[1] != -2000i32 { return 6u64 }

            42u64
        }
    "#;
    // The tree-walker used to be left out here: the unsuffixed index
    // literals in `u8a[1] = 50u8` reached the evaluator still carrying
    // the `Number` placeholder and it died (TREE-WALKER-NUM-W). The
    // index of an indexed *assignment* was simply never visited by the
    // type checker — only the dict branch looked at it — so nothing
    // ever resolved it. Fixed while landing CLOSURE-CAPTURE E3, which
    // is where the same hole stopped being survivable.
    assert_consistent(src, "narrow_int_array_packing_round_trip");
}

#[test]
fn narrow_int_arithmetic_and_cast_interpreter() {
    // NUM-W Phase 3: exercises every narrow-int width through
    // arithmetic, comparison, the full cross-width cast matrix,
    // and `__builtin_sizeof`. Interpreter-only here — the JIT
    // and AOT backends don't yet recognise the new types
    // (Phases 4 / 5). When those phases land this test should
    // become an `assert_consistent` 3-way check.
    let src = r#"
        fn main() -> u64 {
            val u8v: u8 = 250u8 + 5u8
            val u16v: u16 = u8v as u16 + 1u16
            val u32v: u32 = u16v as u32 * 1000u32
            val i32v: i32 = -1i32
            val u32_from_i32: u32 = i32v as u32
            val i8v: i8 = (-100i64) as i8
            val sizes_ok: bool =
                __builtin_sizeof(u8v) == 1u64
                && __builtin_sizeof(u16v) == 2u64
                && __builtin_sizeof(u32v) == 4u64
                && __builtin_sizeof(i32v) == 4u64
                && __builtin_sizeof(i8v) == 1u64
            if u8v != 255u8 { 1u64 }
            elif u16v != 256u16 { 2u64 }
            elif u32v != 256000u32 { 3u64 }
            elif u32_from_i32 != 4294967295u32 { 4u64 }
            elif i8v != -100i8 { 5u64 }
            elif !sizes_ok { 6u64 }
            else { 42u64 }
        }
    "#;
    let interp = interpreter_value(src);
    assert_eq!(interp, 42, "interpreter expected 42, got {interp}");
    // JIT and AOT would fail today (no codegen for narrow
    // ints) — re-enable when Phases 4 / 5 land.
}

#[test]
fn narrow_int_hash_dispatch_interpreter() {
    // NUM-W Phase 6 (+ NUM-W-signed-hash follow-up):
    // `core/std/hash.t` declares `impl Hash for {u8, u16, u32,
    // i8, i16, i32}` so user code can dispatch `(7u8).hash()`
    // etc. through the same extension-trait method-registry
    // path the i64 / u64 impls already use. AOT silently skips
    // registering these (#161 / NUM-W-AOT); JIT silently falls
    // back (NUM-W-JIT). Interpreter-only here.
    //
    // Signed widths route through the matching unsigned width
    // (e.g. `(self as u8) as u64`) to avoid sign extension —
    // `(-5_i8).hash()` returns 251 (the byte pattern), not
    // 0xFFFFFFFFFFFFFFFB. This keeps all six results in a
    // sane range so the final sum doesn't depend on u64 wrap.
    //
    //   u8(7).hash()    = 7
    //   u16(100).hash() = 100
    //   u32(100000).hash() = 100000
    //   i8(-5).hash()   = 251           (0xFB)
    //   i16(-100).hash() = 65436         (0xFF9C)
    //   i32(-1000).hash() = 4294966296   (0xFFFFFC18)
    //   ----------------------------------
    //   sum            = 4295132090
    let src = r#"
        fn main() -> u64 {
            val a: u8 = 7u8
            val b: u16 = 100u16
            val c: u32 = 100000u32
            val d: i8 = -5i8
            val e: i16 = -100i16
            val f: i32 = -1000i32
            a.hash() + b.hash() + c.hash() + d.hash() + e.hash() + f.hash()
        }
    "#;
    let interp = interpreter_value(src);
    assert_eq!(interp, 4295132090, "interpreter expected 4295132090, got {interp}");
}

#[test]
fn hash_trait_dispatch_on_all_primitives() {
    // Phase 1 of the user-space dict effort (`core/std/hash.t`):
    // verifies the auto-loaded `Hash` extension trait dispatches
    // identically across interpreter / JIT-fallback / AOT for
    // every primitive impl. Sum lets us catch a per-backend
    // divergence anywhere in the chain rather than just the
    // first one. Expected: 7 (i64) + 100 (u64) + 1 (bool true)
    // + 0 (str placeholder) = 108.
    //
    // The str arm is intentionally a constant `0u64` rather than
    // a real `self.len()`-based hash — the AOT compiler doesn't
    // yet lower `BuiltinMethodCall::Len` on str. When that lands
    // (or when `__extern_str_hash` is wired), the str impl in
    // `core/std/hash.t` and this expected value should be
    // updated together.
    let src = r#"
        fn main() -> u64 {
            val a: i64 = 7i64
            val b: u64 = 100u64
            val c: bool = true
            val d: str = "hi"
            a.hash() + b.hash() + c.hash() + d.hash()
        }
    "#;
    assert_consistent(src, "hash_trait_dispatch_on_all_primitives");
}

// ---------------------------------------------------------------
// STRUCT-UPDATE: `P { x: 1i64, ..base }`
// ---------------------------------------------------------------

#[test]
fn struct_update_fills_omitted_fields_from_the_base() {
    // The type checker rewrites the update into an ordinary literal,
    // so what the three backends have to agree on is the field-by-field
    // copy: `x` and `z` from `a`, `y` as written.
    let src = r#"
        struct P { x: u64, y: u64, z: u64 }

        fn main() -> u64 {
            val a = P { x: 1u64, y: 2u64, z: 3u64 }
            val b = P { y: 20u64, ..a }
            b.x * 100u64 + b.y * 10u64 + b.z
        }
    "#;
    assert_consistent(src, "struct_update_fills_omitted_fields_from_the_base");
}

#[test]
fn struct_update_carries_a_struct_typed_field() {
    // The base fills `i`, a struct-typed field, by field access. That
    // shape used to be a lowering error on both compiled backends
    // ("cannot build a struct-typed value from FieldAccess") — the
    // tuple counterpart accepted it and the struct one did not, so a
    // hand-written `Outer { i: o.i, .. }` failed the same way.
    let src = r#"
        struct Inner { a: u64, b: u64 }
        struct Outer { i: Inner, n: u64 }

        fn main() -> u64 {
            val o = Outer { i: Inner { a: 1u64, b: 2u64 }, n: 3u64 }
            val u = Outer { n: 30u64, ..o }
            u.i.a + u.i.b + u.n
        }
    "#;
    assert_consistent(src, "struct_update_carries_a_struct_typed_field");
}

#[test]
fn struct_update_takes_self_as_its_base() {
    // A method whose tail expression is the update. That route reaches
    // the type checker through `check_expr_located`, which skips
    // `visit_expr` — where an un-intercepted update would have reached
    // the backends undesugared.
    let src = r#"
        struct P { x: u64, y: u64 }

        impl P {
            fn with_x(&self, nx: u64) -> P {
                P { x: nx, ..self }
            }
        }

        fn main() -> u64 {
            val a = P { x: 1u64, y: 2u64 }
            val b = a.with_x(7u64)
            b.x * 10u64 + b.y
        }
    "#;
    assert_consistent(src, "struct_update_takes_self_as_its_base");
}

#[test]
fn struct_update_on_a_generic_struct() {
    let src = r#"
        struct Wrap<T> { v: T, n: u64 }

        fn main() -> u64 {
            val w: Wrap<u64> = Wrap { v: 5u64, n: 1u64 }
            val u: Wrap<u64> = Wrap { n: 2u64, ..w }
            u.v * 10u64 + u.n
        }
    "#;
    assert_consistent(src, "struct_update_on_a_generic_struct");
}

#[test]
fn struct_update_copies_rather_than_aliases() {
    // Compound bindings alias in this language (`val q = p` names the
    // same leaf locals), so the thing worth pinning is that an update
    // does *not*: writing through the copy must leave the base alone
    // on every backend.
    let src = r#"
        struct P { x: u64, y: u64 }

        fn main() -> u64 {
            var a: P = P { x: 1u64, y: 2u64 }
            var b: P = P { y: 9u64, ..a }
            b.x = 99u64
            a.x * 100u64 + b.x
        }
    "#;
    assert_consistent(src, "struct_update_copies_rather_than_aliases");
}

// ---------------------------------------------------------------
// MATCH-STRUCT-ARM: composite tails in struct / tuple return position
// ---------------------------------------------------------------

#[test]
fn a_struct_returning_match_returns_the_arm_that_ran() {
    // Every arm used to lower its literal into its own locals and set
    // the pending-struct channel to whichever the lowering saw last;
    // the return then read that arm's locals whichever arm actually
    // ran. Any other arm therefore returned a zero-filled struct, with
    // no diagnostic and the same wrong answer from the IR VM, the JIT
    // and the AOT — they share this lowering. The tree-walker lane is
    // what tells them apart, which is why it now really is the
    // tree-walker.
    let src = r#"
        struct P { x: u64, y: u64 }

        fn pick(n: u64) -> P {
            match n {
                0u64 => P { x: 1u64, y: 2u64 },
                1u64 => P { x: 10u64, y: 20u64 },
                _ => P { x: 100u64, y: 200u64 }
            }
        }

        fn main() -> u64 {
            val a = pick(0u64)
            val b = pick(1u64)
            val c = pick(9u64)
            a.x + a.y + b.x + b.y + c.x + c.y
        }
    "#;
    // 3 + 30 + 300 = 333.
    assert_consistent(src, "struct_returning_match_arm");
}

#[test]
fn a_struct_returning_if_chain_returns_the_branch_that_ran() {
    // Same defect through the `if` walker, elifs included — each is a
    // separate branch that has to write the same locals.
    let src = r#"
        struct P { x: u64, y: u64 }

        fn pick(n: u64) -> P {
            if n == 0u64 { P { x: 1u64, y: 2u64 } }
            elif n == 1u64 { P { x: 10u64, y: 20u64 } }
            elif n == 2u64 { P { x: 100u64, y: 200u64 } }
            else { P { x: 1000u64, y: 2000u64 } }
        }

        fn main() -> u64 {
            val a = pick(0u64)
            val b = pick(1u64)
            val c = pick(2u64)
            val d = pick(3u64)
            a.x + b.x + c.x + d.x
        }
    "#;
    // 1 + 10 + 100 + 1000 = 1111 -> 1111 & 0xff.
    assert_consistent(src, "struct_returning_if_chain");
}

#[test]
fn a_tuple_returning_composite_returns_the_branch_that_ran() {
    // Tuples ride the same pending-value channel and had the same
    // hole.
    let src = r#"
        fn pick(n: u64) -> (u64, u64) {
            if n == 0u64 { (1u64, 2u64) } else { (30u64, 40u64) }
        }

        fn main() -> u64 {
            val a = pick(0u64)
            val b = pick(1u64)
            a.0 + a.1 + b.0 + b.1
        }
    "#;
    // 3 + 70 = 73.
    assert_consistent(src, "tuple_returning_composite");
}

#[test]
fn a_composite_tail_carries_compound_fields_and_bindings() {
    // The branches are not all literals: one comes from a binding, one
    // from a call, and the struct has a struct-typed field — the leaf
    // shapes `store_struct_value_into_fields` has to cover once the
    // walker routes each branch into the shared target.
    let src = r#"
        struct Inner { a: u64, b: u64 }
        struct Outer { i: Inner, n: u64 }

        fn mk(v: u64) -> Inner { Inner { a: v, b: v + 1u64 } }

        fn pick(n: u64) -> Outer {
            val held = Inner { a: 7u64, b: 8u64 }
            match n {
                0u64 => Outer { i: held, n: 1u64 },
                1u64 => Outer { i: mk(20u64), n: 2u64 },
                _ => {
                    val other = mk(50u64)
                    Outer { i: other, n: 3u64 }
                }
            }
        }

        fn main() -> u64 {
            val a = pick(0u64)
            val b = pick(1u64)
            val c = pick(9u64)
            a.i.a + a.n + b.i.a + b.n + c.i.a + c.n
        }
    "#;
    // (7+1) + (20+2) + (50+3) = 83.
    assert_consistent(src, "composite_tail_compound_fields");
}

#[test]
fn a_method_returning_a_struct_from_a_composite_tail() {
    // Methods go through the same function-body lowering, and `self`
    // is a live binding inside every branch.
    let src = r#"
        struct P { x: u64, y: u64 }

        impl P {
            fn pick(&self, n: u64) -> P {
                if n == 0u64 { P { x: self.x, y: 0u64 } } else { P { x: 0u64, y: self.y } }
            }
        }

        fn main() -> u64 {
            val p = P { x: 3u64, y: 5u64 }
            val a = p.pick(0u64)
            val b = p.pick(1u64)
            a.x * 10u64 + a.y + b.x + b.y
        }
    "#;
    // 30 + 0 + 0 + 5 = 35.
    assert_consistent(src, "method_struct_composite_tail");
}

#[test]
fn a_guarded_arm_returning_a_struct() {
    // A guard inserts an extra block between the pattern test and the
    // arm body; the body still has to write the shared target.
    let src = r#"
        struct P { x: u64, y: u64 }

        fn pick(n: u64) -> P {
            match n {
                k if k > 10u64 => P { x: 5u64, y: 5u64 },
                0u64 => P { x: 1u64, y: 1u64 },
                _ => P { x: 2u64, y: 2u64 }
            }
        }

        fn main() -> u64 {
            val a = pick(20u64)
            val b = pick(0u64)
            val c = pick(3u64)
            a.x * 100u64 + b.x * 10u64 + c.x
        }
    "#;
    // 500 + 10 + 2 = 512 -> 512 & 0xff == 0.
    assert_consistent(src, "guarded_arm_struct");
}

#[test]
fn a_diverging_branch_in_a_struct_returning_composite() {
    // Not every branch produces a value: one panics, one leaves
    // through `return`. Both reach the merge from nowhere, so the
    // walker has to accept them rather than demand a struct — the
    // scalar path always did, and routing struct returns through a
    // pre-allocated target must not lose it.
    let src = r#"
        struct P { x: u64, y: u64 }

        fn or_die(n: u64) -> P {
            if n == 0u64 { P { x: 1u64, y: 2u64 } } else { panic("nope") }
        }

        fn early(n: u64) -> P {
            val held = P { x: 7u64, y: 8u64 }
            if n == 0u64 { return held } else { P { x: 1u64, y: 2u64 } }
        }

        fn main() -> u64 {
            val a = or_die(0u64)
            val b = early(0u64)
            val c = early(1u64)
            a.x + b.x + c.x
        }
    "#;
    // 1 + 7 + 1 = 9.
    assert_consistent(src, "diverging_branch_struct_composite");
}

// ---------------------------------------------------------------
// COMPOUND-BLOCK-RHS: composite `val` right-hand sides
// ---------------------------------------------------------------

#[test]
fn a_struct_producing_if_chain_can_be_a_val_rhs() {
    // `val/var rhs produced no value` until the binding pre-allocates
    // its fields and every branch writes those. The `if`-chain half of
    // the enum machinery's struct counterpart.
    let src = r#"
        struct P { x: u64, y: u64 }

        fn main() -> u64 {
            val a = if true { P { x: 1u64, y: 2u64 } } else { P { x: 30u64, y: 40u64 } }
            val b = if false { P { x: 1u64, y: 2u64 } } else { P { x: 30u64, y: 40u64 } }
            a.x + a.y + b.x + b.y
        }
    "#;
    // 3 + 70 = 73.
    assert_consistent(src, "struct_if_chain_val_rhs");
}

#[test]
fn a_struct_producing_match_can_be_a_val_rhs() {
    let src = r#"
        struct P { x: u64, y: u64 }

        fn main() -> u64 {
            val n = 1u64
            val p = match n {
                0u64 => P { x: 10u64, y: 11u64 },
                _ => P { x: 20u64, y: 21u64 }
            }
            p.x + p.y
        }
    "#;
    // 41.
    assert_consistent(src, "struct_match_val_rhs");
}

#[test]
fn a_block_rhs_binds_the_struct_its_tail_produces() {
    // The plain-block half: leading statements run, the tail supplies
    // the value. This is also the shape a struct update with a
    // side-effecting base desugars to.
    let src = r#"
        struct P { x: u64, y: u64 }

        fn mk(v: u64) -> P { P { x: v, y: v + 1u64 } }

        fn main() -> u64 {
            val p = {
                val t = mk(50u64)
                P { x: t.x, y: t.y }
            }
            p.x + p.y
        }
    "#;
    // 101.
    assert_consistent(src, "block_val_rhs_struct");
}

#[test]
fn a_struct_update_with_a_call_base_runs_on_every_backend() {
    // STRUCT-UPDATE's non-path base keeps a temporary, which puts a
    // block holding a struct literal on the rhs — the shape this
    // change made bindable. The base must still be evaluated exactly
    // once however many fields it fills, which is what the temporary
    // is for; `a_side_effecting_base_is_evaluated_once` in
    // `interpreter/tests/struct_update_tests.rs` pins the count.
    let src = r#"
        struct P { x: u64, y: u64, z: u64 }

        fn defaults() -> P { P { x: 1u64, y: 2u64, z: 3u64 } }

        fn main() -> u64 {
            val u = P { x: 10u64, ..defaults() }
            u.x + u.y + u.z
        }
    "#;
    // 10 + 2 + 3 = 15.
    assert_consistent(src, "struct_update_call_base");
}

#[test]
fn a_tuple_producing_composite_can_be_a_val_rhs() {
    let src = r#"
        fn mk(v: u64) -> (u64, u64) { (v, v + 1u64) }

        fn main() -> u64 {
            val a = if true { (1u64, 2u64) } else { (30u64, 40u64) }
            val b = if false { mk(5u64) } else { mk(50u64) }
            a.0 + a.1 + b.0 + b.1
        }
    "#;
    // 3 + 101 = 104.
    assert_consistent(src, "tuple_composite_val_rhs");
}

#[test]
fn a_composite_val_rhs_covers_branches_that_do_not_produce() {
    // A branch may panic instead of producing; detection must not read
    // that as "not a struct" and give up on the whole rhs.
    let src = r#"
        struct P { x: u64, y: u64 }

        fn main() -> u64 {
            val p = if true { P { x: 5u64, y: 6u64 } } else { panic("nope") }
            p.x + p.y
        }
    "#;
    // 11.
    assert_consistent(src, "composite_val_rhs_diverging_branch");
}

#[test]
fn a_composite_val_rhs_nests_and_carries_compound_fields() {
    // A `match` inside an `if`, a struct-typed field filled from a
    // call, and a `var` written through afterwards — the binding owns
    // its locals, so the write must not reach whatever the branch
    // built from.
    let src = r#"
        struct Inner { a: u64, b: u64 }
        struct Outer { i: Inner, n: u64 }

        fn mk(v: u64) -> Inner { Inner { a: v, b: v + 1u64 } }

        fn main() -> u64 {
            val n = 1u64
            var o: Outer = if true {
                match n {
                    0u64 => Outer { i: mk(1u64), n: 10u64 },
                    _ => Outer { i: mk(2u64), n: 20u64 }
                }
            } else {
                Outer { i: mk(3u64), n: 30u64 }
            }
            o.n = 100u64
            o.i.a + o.i.b + o.n
        }
    "#;
    // 2 + 3 + 100 = 105.
    assert_consistent(src, "composite_val_rhs_nested");
}

#[test]
fn a_generic_struct_composite_val_rhs_takes_its_args_from_the_annotation() {
    // The pre-allocated target has to be the *monomorphised* instance,
    // and an associated-call-free composite has nothing but the
    // annotation to pick it from — the same rule
    // `resolve_struct_instance` applies everywhere else.
    let src = r#"
        struct Wrap<T> { v: T, n: u64 }

        fn main() -> u64 {
            val w: Wrap<u64> = if true { Wrap { v: 1u64, n: 2u64 } } else { Wrap { v: 3u64, n: 4u64 } }
            w.v + w.n
        }
    "#;
    // 3.
    assert_consistent(src, "generic_struct_composite_val_rhs");
}

#[test]
fn a_composite_val_rhs_binds_an_associated_call_and_still_drops() {
    // `Box::new(..)` in a branch is an associated call, so detection
    // has to recognise that shape too — and the binding owns whatever
    // the taken branch built, so it needs the same auto-drop
    // registration a literal rhs gets. Without it the box would leak
    // in the compiled backends while the tree-walker freed it, which
    // is a divergence the exit code alone would not show.
    let src = r#"
        fn main() -> u64 {
            val n = {
                val b: Box<i64> = if true { Box::new(7i64) } else { Box::new(8i64) }
                b.get()
            }
            val after = __builtin_live_bytes()
            (n as u64) + after
        }
    "#;
    // 7 when the box was freed on the way out of the inner scope, 15
    // when it leaked — so the allocation counter, not just the value,
    // is what this pins.
    assert_consistent(src, "composite_val_rhs_associated_call");
}

// --- UNIT-TYPE-ARG: `()` as a generic type argument -----------------
//
// `Result<(), E>` — the signature of every operation that answers only
// "did it work" — could not be lowered on any compiled lane. Two gates
// refused it: `lower_param_or_return_type` rejected a `Type::Unit`
// type argument, and `is_supported_enum_payload` rejected a `()`
// payload. The layout side was already fine —
// `flatten_compound_leaf_types` gives `Type::Unit` zero leaves, which
// is exactly what a unit variant already contributes — so what was
// missing was permission, plus a `PayloadSlot::Unit` that holds no
// local so the storage's flat value list stays aligned with the
// function boundary's.
//
// `core/std/net.t` is where this was hit: `set_blocking` / `close` /
// `shutdown_write` / `take_error` all wanted `Result<(), NetError>`.

/// Both arms of a `Result<(), E>`, an `Option<()>`, and a `()`-payload
/// result threaded through another function.
#[test]
fn unit_can_be_a_generic_type_argument() {
    let src = r#"
        enum E { Bad, Worse }

        fn check(n: i64) -> Result<(), E> {
            if n < 0i64 { Result::Err(E::Bad) }
            elif n == 0i64 { Result::Err(E::Worse) }
            else { Result::Ok(()) }
        }

        fn maybe(n: i64) -> Option<()> {
            if n > 0i64 { Option::Some(()) } else { Option::None }
        }

        # The `()` result crossing a second function boundary, which is
        # where a payload slot that wrongly held a local would put every
        # later value one position out.
        fn forward(n: i64) -> Result<u64, E> {
            val c = check(n)
            match c {
                Result::Ok(_) => Result::Ok(42u64),
                Result::Err(e) => Result::Err(e),
            }
        }

        fn main() -> u64 {
            val a = check(1i64)
            val ra = match a { Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }
            val b = check(-1i64)
            val rb = match b {
                Result::Ok(_) => 0u64,
                Result::Err(e) => match e { E::Bad => 2u64, E::Worse => 9u64 },
            }
            val c = maybe(5i64)
            val rc = match c { Option::Some(_) => 4u64, Option::None => 0u64 }
            val d = maybe(-5i64)
            val rd = match d { Option::Some(_) => 0u64, Option::None => 8u64 }
            val e = forward(3i64)
            val re = match e { Result::Ok(v) => v, Result::Err(x) => 0u64 }
            val f = forward(0i64)
            val rf = match f { Result::Ok(v) => 0u64, Result::Err(x) => 16u64 }
            ra + rb + rc + rd + re + rf
        }
    "#;
    // 1 + 2 + 4 + 8 + 42 + 16.
    assert_eq!(interpreter_value(src) & 0xff, 73);
    assert_consistent(src, "unit_type_arg");
}

/// A `()` payload prints as `()`, so `Ok(())` is not mistaken for a
/// payload-less `Ok` — and every lane spells it the same way.
#[test]
fn a_unit_payload_prints_as_unit() {
    let src = r#"
        enum E { Bad }
        fn main() -> u64 {
            val a: Result<(), E> = Result::Ok(())
            println(a)
            val b: Option<()> = Option::Some(())
            println(b)
            val c: Option<()> = Option::None
            println(c)
            0u64
        }
    "#;
    assert_eq!(
        interpreter_stdout(src, "unit_payload_print", true),
        "Result<(), E>::Ok(())\nOption<()>::Some(())\nOption<()>::None\n"
    );
    assert_stdout_consistent(src, "unit_payload_print");
}

// --- COMPOUND-BLOCK-RHS: unwrapping a compound out of a `match` -----
//
// `val s = match r { Result::Ok(s) => s, Result::Err(e) => ... }` is
// how every `Result`-returning constructor is used, and it did not
// lower: "val/var rhs produced no value".
//
// Two things defeated `detect_struct_result`. An arm body that is a
// bare *pattern-bound* name is not in `self.bindings` at detection
// time — arm bindings only exist once the arm is being lowered — so
// the success arm looked like an unknown identifier. And an error arm
// that leaves through `return` was not recognised as diverging the way
// `panic(...)` already was, so it constrained the answer instead of
// standing aside.
//
// Neither is recoverable from the arm alone, but both are recoverable:
// the scrutinee's enum says what that variant's payload is at that
// position, and a `return` is a `return`.

/// The success arm is the bound name and the error arm leaves through
/// `return` — the shape `TcpStream::connect` and every other
/// `Result`-returning constructor is used with. No annotation.
#[test]
fn a_struct_can_be_unwrapped_out_of_a_result_by_match() {
    let src = r#"
        struct P { v: i64 }
        enum E { Bad }

        fn mk(n: i64) -> Result<P, E> {
            if n < 0i64 {
                Result::Err(E::Bad)
            } else {
                val p = P { v: n }
                Result::Ok(p)
            }
        }

        fn main() -> u64 {
            val r = mk(7i64)
            var p = match r {
                Result::Ok(q) => q,
                Result::Err(e) => { return 1u64 }
            }
            # A `var` so the binding has to be real storage rather than
            # an alias of the payload.
            p.v = p.v + 1i64
            p.v as u64
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 8);
    assert_consistent(src, "compound_out_of_result");
}

/// The error arm produces a struct of its own rather than diverging,
/// so both arms have to agree — and the bound name still has to be
/// resolved through the scrutinee to know that they do.
#[test]
fn both_arms_of_a_match_may_produce_the_same_struct() {
    let src = r#"
        struct P { v: i64 }
        enum E { Bad }

        fn mk(n: i64) -> Result<P, E> {
            if n < 0i64 {
                Result::Err(E::Bad)
            } else {
                val p = P { v: n }
                Result::Ok(p)
            }
        }

        fn main() -> u64 {
            val good = mk(7i64)
            val a = match good {
                Result::Ok(q) => q,
                Result::Err(e) => P { v: 100i64 },
            }
            val bad = mk(-1i64)
            val b = match bad {
                Result::Ok(q) => q,
                Result::Err(e) => P { v: 100i64 },
            }
            (a.v + b.v) as u64
        }
    "#;
    // 7 from the payload, 100 from the fallback.
    assert_eq!(interpreter_value(src) & 0xff, 107);
    assert_consistent(src, "compound_both_arms");
}

/// A compound return is flattened into one cranelift return slot per
/// leaf, and past the target's return registers cranelift refused the
/// signature outright ("Too many return values to fit in registers"),
/// so a struct this wide could not be returned at all on either
/// compiled lane. Ten leaves clears the widest limit among the
/// supported targets (8, on aarch64).
#[test]
fn a_struct_wider_than_the_return_registers_comes_back_whole() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64, e: u64,
            f: u64, g: u64, h: u64, i: u64, j: u64,
        }

        fn make(n: u64) -> Wide {
            Wide {
                a: n, b: 2u64, c: 3u64, d: 4u64, e: 5u64,
                f: 6u64, g: 7u64, h: 8u64, i: 9u64, j: 10u64,
            }
        }

        fn main() -> u64 {
            val w = make(1u64)
            # Every leaf, so a return area that only carried the first
            # few would show up as a wrong sum rather than a crash.
            w.a + w.b + w.c + w.d + w.e + w.f + w.g + w.h + w.i + w.j
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 55);
    assert_consistent(src, "wide_struct_return");
}

/// The leaf count is what matters, not the field count: nesting is
/// what produces a wide leaf list out of few fields. The leaves are
/// mixed types on purpose — integer and float leaves draw on separate
/// return registers, so it is the nine integer ones that overflow
/// here while the three floats still fit.
#[test]
fn a_wide_return_may_nest_and_mix_leaf_types() {
    let src = r#"
        struct Inner { p: u64, q: i64, r: f64 }
        struct Wide {
            a: u64, b: i64, c: f64, d: bool,
            e: Inner, f: Inner,
            g: u64, h: u64,
        }

        fn make(n: u64) -> Wide {
            Wide {
                a: n, b: 2i64, c: 3.5f64, d: true,
                e: Inner { p: 10u64, q: -1i64, r: 0.5f64 },
                f: Inner { p: 20u64, q: -2i64, r: 1.5f64 },
                g: 99u64, h: 100u64,
            }
        }

        fn main() -> u64 {
            val w = make(1u64)
            var acc = w.a + w.e.p + w.f.p + w.g + w.h
            if w.d { acc = acc + 1u64 }
            acc = acc + (w.c + w.e.r + w.f.r) as u64
            acc + (w.b - w.e.q - w.f.q) as u64
        }
    "#;
    // 1 + 10 + 20 + 99 + 100 = 230, +1 for `d`, +5 for 3.5+0.5+1.5,
    // +5 for 2-(-1)-(-2).
    assert_eq!(interpreter_value(src) & 0xff, 241);
    assert_consistent(src, "wide_nested_return");
}

/// Tuples and enum payloads reach codegen as the same flattened leaf
/// list a struct does, and each has its own call lowering
/// (`CallTuple` / `CallEnum`), so each needs its own witness that the
/// return area carries the whole value.
#[test]
fn wide_tuple_and_enum_returns_survive_the_return_area() {
    let src = r#"
        enum Big {
            Many(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64),
            Nil,
        }

        fn tup(n: u64) -> (u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) {
            (n, 1u64, 2u64, 3u64, 4u64, 5u64, 6u64, 7u64, 8u64, 9u64)
        }

        fn en(n: u64) -> Big {
            if n > 0u64 {
                Big::Many(n, 1u64, 2u64, 3u64, 4u64, 5u64, 6u64, 7u64, 8u64, 9u64)
            } else {
                Big::Nil
            }
        }

        fn main() -> u64 {
            val t = tup(10u64)
            val s = t.0 + t.1 + t.2 + t.3 + t.4 + t.5 + t.6 + t.7 + t.8 + t.9
            val e = en(1u64)
            val p = match e {
                Big::Many(a, b, c, d, f, g, h, i, j, k) => {
                    a + b + c + d + f + g + h + i + j + k
                }
                Big::Nil => 0u64,
            }
            s + p
        }
    "#;
    // 10 + 45 from the tuple, 1 + 45 from the payload.
    assert_eq!(interpreter_value(src) & 0xff, 101);
    assert_consistent(src, "wide_tuple_enum_return");
}

/// The two call shapes that append their own return slots: a `&mut
/// self` method hands back the mutated receiver's leaves alongside
/// the value, and a `dyn` call goes through a vtable rather than a
/// direct symbol. Both push the slot count further past the registers.
#[test]
fn wide_returns_work_through_writeback_and_dyn_dispatch() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64, e: u64,
            f: u64, g: u64, h: u64, i: u64, j: u64,
        }

        fn sum(w: Wide) -> u64 {
            w.a + w.b + w.c + w.d + w.e + w.f + w.g + w.h + w.i + w.j
        }

        struct Counter { n: u64 }

        impl Counter {
            fn bump(&mut self) -> Wide {
                self.n = self.n + 1u64
                Wide {
                    a: self.n, b: 2u64, c: 3u64, d: 4u64, e: 5u64,
                    f: 6u64, g: 7u64, h: 8u64, i: 9u64, j: 10u64,
                }
            }
        }

        trait Maker {
            fn build(self: Self) -> Wide
        }

        struct M { k: u64 }

        impl Maker for M {
            fn build(self: Self) -> Wide {
                Wide {
                    a: self.k, b: 2u64, c: 3u64, d: 4u64, e: 5u64,
                    f: 6u64, g: 7u64, h: 8u64, i: 9u64, j: 10u64,
                }
            }
        }

        fn via_dyn(m: &dyn Maker) -> u64 {
            val w = m.build()
            sum(w)
        }

        fn main() -> u64 {
            var c = Counter { n: 0u64 }
            val w1 = c.bump()
            val w2 = c.bump()
            val m = M { k: 3u64 }
            # 55 + 56 witnesses the writeback: the second call has to
            # see the increment the first one made.
            sum(w1) + sum(w2) + via_dyn(&m) + c.n
        }
    "#;
    // 55 + 56 + 57 + 2 = 170.
    assert_eq!(interpreter_value(src) & 0xff, 170);
    assert_consistent(src, "wide_return_writeback_dyn");
}
