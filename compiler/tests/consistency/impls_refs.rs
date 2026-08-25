//! Concrete-vs-generic impl dispatch, `String`, and the `&` / `&mut`
//! borrow stage (REF-Stage-2).

use super::harness::*;

#[test]
fn concrete_impl_dispatch_by_receiver_type_args() {
    // CONCRETE-IMPL Phase 2 (interpreter) + Phase 2b (compiler):
    // two `impl MarkerName for Container<X>` blocks with different
    // concrete `X` coexist in both interpreter and compiler method
    // registries. Instance method dispatch picks the matching impl
    // by reading the receiver's runtime / IR type args and looking
    // up the `(struct, method)` spec list with that key. Both
    // `Object::Struct.type_args` (interpreter) and
    // `StructDef.type_args` (compiler) are consulted, and a 3-tier
    // fallback (exact → empty-args → lone-spec) keeps the
    // single-impl baseline working unchanged.
    //
    // Expected exit: 8 + 64 = 72.
    let src = r#"
        struct Container<T> {
            value: u64
        }
        trait MarkerName {
            fn marker_name(self: Self) -> u64
        }
        impl<T> Container<T> {
            fn new() -> Self { Container { value: 0u64 } }
        }
        impl MarkerName for Container<u8> {
            fn marker_name(self: Self) -> u64 { 8u64 }
        }
        impl MarkerName for Container<i64> {
            fn marker_name(self: Self) -> u64 { 64u64 }
        }
        fn main() -> u64 {
            val a: Container<u8> = Container::new()
            val b: Container<i64> = Container::new()
            a.marker_name() + b.marker_name()
        }
    "#;
    assert_consistent(src, "concrete_impl_dispatch_by_receiver_type_args");
}

#[test]
fn concrete_inherent_impls_type_check_against_the_matching_signature() {
    // CONCRETE-IMPL-Phase-2c: the *type checker's* registry used to
    // hold one method per (struct, method) — the last impl registered
    // won for every receiver. With two inherent impls whose `get`
    // signatures differ (`-> u8` vs `-> i64`), `a.get()` on a
    // `C<u8>` receiver was typed with the `C<i64>` signature, and the
    // program was rejected with a bogus "expected u8, got i64" — even
    // though both runtimes had dispatched per-receiver since Phase 2b.
    // The registry is now multi-spec and dispatch reads the receiver's
    // type args, matching the runtime layers. Both impl orders are
    // pinned — the result must not depend on registration order.
    let src = r#"
        struct C<T> { v: T }
        impl C<u8> {
            fn get(self: Self) -> u8 { self.v }
        }
        impl C<i64> {
            fn get(self: Self) -> i64 { self.v }
        }
        fn main() -> i64 {
            val a: C<u8> = C { v: 1u8 }
            val b: C<i64> = C { v: 2i64 }
            val x: u8 = a.get()
            val y: i64 = b.get()
            (x as i64) + y
        }
    "#;
    assert_consistent(src, "concrete_impl_tycheck_u8_first");
    // Reverse registration order: the i64 impl first.
    let src = r#"
        struct C<T> { v: T }
        impl C<i64> {
            fn get(self: Self) -> i64 { self.v }
        }
        impl C<u8> {
            fn get(self: Self) -> u8 { self.v }
        }
        fn main() -> i64 {
            val a: C<u8> = C { v: 1u8 }
            val b: C<i64> = C { v: 2i64 }
            val x: u8 = a.get()
            val y: i64 = b.get()
            (x as i64) + y
        }
    "#;
    assert_consistent(src, "concrete_impl_tycheck_reversed");
}

#[test]
fn concrete_associated_call_picks_the_spec_from_the_annotation() {
    // The associated-function form has no receiver to read type args
    // off of; `C::make` exists under both `impl C<u8>` and
    // `impl C<i64>`, so the enclosing annotation (`val b: C<i64> = ...`)
    // is the only discriminator. Phase 2c threads the type hint into
    // the lookup so each binding picks the spec matching its
    // annotation. Without the hint (a bare call in expression
    // position) the ambiguity would be a compile error.
    let src = r#"
        struct C<T> { v: T }
        impl C<u8> {
            fn make(v: u8) -> C<u8> { C { v: v } }
        }
        impl C<i64> {
            fn make(v: i64) -> C<i64> { C { v: v } }
        }
        fn main() -> i64 {
            val a: C<u8> = C::make(1u8)
            val x: u8 = a.v
            val b: C<i64> = C::make(2i64)
            val y: i64 = b.v
            val total = x as i64 + y
            total
        }
    "#;
    assert_consistent(src, "concrete_associated_hint");
}

#[test]
fn concrete_impl_overrides_the_generic_impl_for_matching_receivers() {
    // CONCRETE-IMPL-Phase-2c generic-wildcard: the generic impl
    // (`impl<T> C<T>`) registers its target args as `[Generic(T)]`,
    // not the empty marker the old fallback looked for — so a
    // `C<i64>` receiver used to find no spec at all when a concrete
    // `impl C<u8>` coexisted (every layer rejected the overlap). Now
    // the wildcard tier makes the generic impl match any receiver the
    // concrete specs don't exactly cover: "concrete overrides
    // generic" is expressible, in the type checker and both
    // runtimes. Both registration orders are pinned.
    let src = r#"
        struct C<T> { v: T }
        impl C<u8> {
            fn get(self: Self) -> u8 { self.v }
        }
        impl<T> C<T> {
            fn get(self: Self) -> T { self.v }
        }
        fn main() -> i64 {
            val a: C<u8> = C { v: 1u8 }
            val b: C<i64> = C { v: 2i64 }
            val x: u8 = a.get()
            val y: i64 = b.get()
            (x as i64) + y
        }
    "#;
    assert_consistent(src, "concrete_overrides_generic");
    let src = r#"
        struct C<T> { v: T }
        impl<T> C<T> {
            fn get(self: Self) -> T { self.v }
        }
        impl C<u8> {
            fn get(self: Self) -> u8 { self.v }
        }
        fn main() -> i64 {
            val a: C<u8> = C { v: 1u8 }
            val b: C<i64> = C { v: 2i64 }
            val x: u8 = a.get()
            val y: i64 = b.get()
            (x as i64) + y
        }
    "#;
    assert_consistent(src, "concrete_overrides_generic_reversed");
}

#[test]
fn concrete_overrides_generic_for_compound_methods_and_associated_calls() {
    // The unified cross-registry dispatch must hold on every path:
    // compound-returning methods (`clone` — a val-rhs binding) and
    // associated calls (`make` — no receiver, the annotation picks
    // the concrete spec).
    let src = r#"
        struct C<T> { v: T }
        impl C<u8> {
            fn clone(self: Self) -> C<u8> { C { v: self.v } }
            fn make(v: u8) -> C<u8> { C { v: v } }
        }
        impl<T> C<T> {
            fn clone(self: Self) -> C<T> { C { v: self.v } }
            fn make(v: T) -> C<T> { C { v: v } }
        }
        fn main() -> i64 {
            val a: C<u8> = C::make(1u8)
            val b: C<i64> = C::make(2i64)
            val ca: C<u8> = a.clone()
            val cb: C<i64> = b.clone()
            (ca.v as i64) + cb.v
        }
    "#;
    assert_consistent(src, "concrete_overrides_generic_compound");
}

#[test]
fn a_paren_expression_on_a_new_line_stays_a_separate_statement() {
    // `b.v\n(x as i64)` used to parse as `b.v(x as i64)`: the postfix
    // chain continued across the newline and the type checker
    // reported "Method 'v' not found" — the field read `b.v` became a
    // call whose "argument" was the next statement. The `(` opening a
    // new line is a fresh expression (same disambiguation as `[` for
    // array literals); this program was previously un-runnable.
    let src = r#"
        struct Holder { v: i64 }
        fn main() -> i64 {
            val h = Holder { v: 7i64 }
            val x: i64 = 1i64
            val y: i64 = h.v
            (x + y) * 2i64
        }
    "#;
    assert_consistent(src, "newline_paren_is_not_a_method_call");
}

#[test]
fn string_from_str_round_trip() {
    // `core/std/string.t::String::from_str(s)` copies the UTF-8
    // bytes of `s` into a fresh, heap-allocated `String` (a
    // wrapper around `Vec<u8>`). The trailing NUL terminator is
    // intentionally NOT copied. `String::len()` matches `s.len()`,
    // and `String::as_ptr()` exposes the underlying byte buffer.
    // Implementation uses `__builtin_mem_copy` for a single-call
    // bulk copy:
    //
    //   - AOT: libc memcpy(dest, src, n) — `s.as_ptr()` is a
    //     pointer into `.rodata`'s `[bytes][NUL][u64 len]`
    //     layout, so the source is real bytes; dest is a
    //     `heap_realloc`'d buffer.
    //   - Interpreter: `s.as_ptr()` writes typed_slot u8 entries
    //     and `HeapManager::copy_memory` (called by
    //     `__builtin_mem_copy`) is typed_slots-aware so the dest
    //     buffer ends up with the same per-byte u8 entries.
    //   - JIT: silent fallback (str scalar isn't modelled).
    //
    // Walks "hello" byte-by-byte via `__builtin_ptr_read` on the
    // pointer returned by `String::as_ptr()`, checking
    // 'h'=104 / 'e'=101 / 'l'=108 / 'l'=108 / 'o'=111 + len=5.
    let src = r#"
        fn main() -> u64 {
            val s: String = String::from_str("hello")
            val n: u64 = s.size()
            val p: ptr = s.as_ptr()
            val a: u8 = __builtin_ptr_read(p, 0u64)
            val b: u8 = __builtin_ptr_read(p, 1u64)
            val c: u8 = __builtin_ptr_read(p, 2u64)
            val d: u8 = __builtin_ptr_read(p, 3u64)
            val e: u8 = __builtin_ptr_read(p, 4u64)
            if n != 5u64 { 1u64 }
            elif a != 104u8 { 2u64 }
            elif b != 101u8 { 3u64 }
            elif c != 108u8 { 4u64 }
            elif d != 108u8 { 5u64 }
            elif e != 111u8 { 6u64 }
            else { 42u64 }
        }
    "#;
    assert_consistent(src, "string_from_str_round_trip");
}

#[test]
fn string_push_str_round_trip() {
    // REF-Stage-2 minimum subset: `String::push_str(&mut self,
    // other: &String)` lets a caller append one heap-managed
    // string onto another. The `&String` parameter type is parsed
    // as `TypeDecl::Ref(...)`, distinct from `String` in the type
    // system, but the call site can pass a bare `String` value
    // via auto-borrow (`s.push_str(b)` where `b: String`).
    //
    // Internally `push_str` delegates to
    // `Vec<u8>::extend_bytes(&mut self, src: ptr, count: u64)` —
    // a concrete-args impl on `Vec<u8>` that loops `__builtin_ptr_read` +
    // `self.push(b)`. That coexists with the generic
    // `impl<T> Vec<T>` thanks to CONCRETE-IMPL Phase 2.
    //
    // Builds "hello" + " " + "world" = "hello world" (len 11) and
    // spot-checks first / middle / last bytes via
    // `__builtin_ptr_read(s.as_ptr(), i)`. 3-way `assert_consistent`
    // pins interpreter / JIT silent fallback / AOT all see the
    // same exit code (42 on success).
    let src = r#"
        fn main() -> u64 {
            var s: String = String::from_str("hello")
            val sp: String = String::from_str(" ")
            val w: String = String::from_str("world")
            s.push_str(sp)
            s.push_str(w)
            val n: u64 = s.size()
            if n != 11u64 {
                return 1u64
            }
            val p: ptr = s.as_ptr()
            val first: u8 = __builtin_ptr_read(p, 0u64)
            val mid: u8 = __builtin_ptr_read(p, 5u64)
            val last: u8 = __builtin_ptr_read(p, 10u64)
            if first != 104u8 {
                return 2u64
            }
            if mid != 32u8 {
                return 3u64
            }
            if last != 100u8 {
                return 4u64
            }
            42u64
        }
    "#;
    assert_consistent(src, "string_push_str_round_trip");
}

#[test]
fn ref_stage2_explicit_borrow_and_mut_ref_round_trip() {
    // REF-Stage-2 (a)+(d)+(f): explicit `&value` / `&mut value`
    // borrow expressions + `&T` / `&mut T` parameter types
    // outside the `&self` receiver position. With (f) landed,
    // `&mut T` parameters require an **explicit** `&mut <var>`
    // at the call site (no auto-borrow into `&mut`), and the
    // operand of `&mut` must itself be a `var`-declared local.
    //
    //   - `len_of(&String)` is called both with auto-borrow
    //     (`len_of(s)`) and with explicit borrow (`len_of(&s)`),
    //     pinning that the type system accepts both forms for
    //     immutable references.
    //   - `first_byte_mut(&mut String)` exercises the new
    //     annotation; the explicit `&mut s` borrow is the only
    //     accepted call form. With erasure still in place at
    //     lowering, the body intentionally only reads the
    //     buffer (true mutation propagation is a future phase).
    //   - `&mut T` actual passed to a `&T` parameter (downgrade)
    //     is also exercised via `len_of(&mut s)`.
    //
    // 3-way `assert_consistent` across interpreter / JIT /
    // AOT — all should agree on exit code 42.
    let src = r#"
        fn len_of(s: &String) -> u64 {
            s.size()
        }

        fn first_byte(s: &String) -> u8 {
            val b: u8 = __builtin_ptr_read(s.as_ptr(), 0u64)
            b
        }

        fn first_byte_mut(s: &mut String) -> u8 {
            val b: u8 = __builtin_ptr_read(s.as_ptr(), 0u64)
            b
        }

        fn main() -> u64 {
            var s: String = String::from_str("hello")
            # auto-borrow: bare String -> &String (immutable only)
            if len_of(s) != 5u64 { return 1u64 }
            # explicit borrow expression
            if len_of(&s) != 5u64 { return 2u64 }
            # &mut T actual downgraded to &T expected
            if len_of(&mut s) != 5u64 { return 3u64 }
            # &mut T parameter, called with explicit &mut value (the
            # only accepted form post-(f); auto-borrow into &mut is rejected)
            if first_byte_mut(&mut s) != 104u8 { return 4u64 }
            # nested: explicit &expr in a chain
            if first_byte(&s) != 104u8 { return 5u64 }
            42u64
        }
    "#;
    assert_consistent(src, "ref_stage2_explicit_borrow_and_mut_ref_round_trip");
}

#[test]
fn ref_stage2_scalar_mut_ref_propagates_mutation_round_trip() {
    // REF-Stage-2 (b)+(c)+(g)+(i): scalar `&mut T` parameter
    // mutation propagates back to the caller's `var` binding
    // across all three backends.
    //   - AOT: pointer-passing via `AddressOf` + `LoadRef` /
    //     `StoreRef`; the caller's local lives in a cranelift
    //     `StackSlot` so the callee writes through the address.
    //   - Interpreter: post-call writeback. Each `&mut <name>`
    //     call argument records the caller-side identifier, the
    //     function body runs against a mutable parameter binding,
    //     and `evaluate_function_call` snapshots the post-body
    //     value before `exit_block` and copies it back into the
    //     caller's binding.
    //   - JIT: the JIT skips on `&mut T` parameter functions
    //     (no `Type::Ref` modelling yet) and falls back to the
    //     interpreter, so it inherits the writeback path.
    //
    // Returns 42 (= 41 + 1).
    let src = r#"
        fn inc(x: &mut u64) {
            x = x + 1u64
        }
        fn main() -> u64 {
            var n: u64 = 41u64
            inc(&mut n)
            inc(&mut n)
            n - 1u64
        }
    "#;
    assert_consistent(src, "ref_stage2_scalar_mut_ref_propagates_mutation_round_trip");
}

#[test]
fn ref_stage2_field_mut_borrow_propagates_round_trip() {
    // REF-Stage-2 (iii): field-level mutable borrow.
    // `&mut p.x` resolves to the leaf scalar local of `Point.x`
    // in AOT (`AddressOf` against the per-field local) and to
    // the captured parent struct's `Rc<RefCell<Object::Struct>>`
    // in the interpreter (post-call `borrow_mut` overwrites the
    // field). The type-checker accepts the new lvalue shape now
    // that `find_borrow_lvalue_root` walks `FieldAccess` chains
    // to the root binding.
    //
    // The test mutates one field (x) twice and reads back both
    // x and y to pin that the unrelated field stayed put.
    let src = r#"
        struct Point { x: u64, y: u64 }
        fn add_in_place(target: &mut u64, delta: u64) {
            target = target + delta
        }
        fn main() -> u64 {
            var p: Point = Point { x: 10u64, y: 20u64 }
            add_in_place(&mut p.x, 30u64)
            add_in_place(&mut p.x, 2u64)
            if p.y != 20u64 { return 1u64 }
            p.x
        }
    "#;
    assert_consistent(src, "ref_stage2_field_mut_borrow_propagates_round_trip");
}

#[test]
fn ref_stage2_tuple_mut_borrow_propagates_round_trip() {
    // REF-Stage-2 (iii) — tuple variant. `&mut t.0` resolves to
    // the leaf scalar local of element 0 in AOT
    // (`AddressOf` against the per-element local) and to the
    // captured parent tuple's `Rc<RefCell<Object::Tuple>>` in
    // the interpreter (post-call `borrow_mut` overwrites
    // `elements[index]`).
    //
    // Mutates element 0 twice and checks element 1 stayed put,
    // mirroring the struct-field test for parity.
    let src = r#"
        fn add_in_place(target: &mut u64, delta: u64) {
            target = target + delta
        }
        fn main() -> u64 {
            var t: (u64, u64) = (10u64, 20u64)
            add_in_place(&mut t.0, 30u64)
            add_in_place(&mut t.0, 2u64)
            if t.1 != 20u64 { return 1u64 }
            t.0
        }
    "#;
    assert_consistent(src, "ref_stage2_tuple_mut_borrow_propagates_round_trip");
}

#[test]
fn ref_stage2_nested_chain_mut_borrow_round_trip() {
    // REF-Stage-2 (iii-deep): nested field / tuple chains as
    // `&mut <chain>` operands. Three flavours in one program:
    //   1. `&mut o.inner.value` — struct -> struct -> scalar
    //   2. `&mut p.a.0`         — struct -> tuple -> scalar
    //   3. `&mut t.0.x`         — tuple  -> struct -> scalar
    // The AOT path uses `resolve_field_chain` (not the older
    // bare-identifier-only `resolve_tuple_element_local`) to
    // walk to the leaf scalar local. The interpreter writeback
    // works automatically because `evaluate(<chain>.0)` /
    // `evaluate(<chain>.field)` already returns the parent's
    // shared `Rc<RefCell<Object>>`, and the existing field /
    // tuple writeback arms then store into it.
    let src = r#"
        struct Inner { value: u64 }
        struct Outer { inner: Inner, tag: u64 }
        struct Pair { a: (u64, u64), tag: u64 }
        struct Point { x: u64, y: u64 }
        fn add_in_place(target: &mut u64, delta: u64) {
            target = target + delta
        }
        fn main() -> u64 {
            var o: Outer = Outer { inner: Inner { value: 5u64 }, tag: 100u64 }
            add_in_place(&mut o.inner.value, 7u64)
            if o.tag != 100u64 { return 1u64 }
            if o.inner.value != 12u64 { return 2u64 }

            var p: Pair = Pair { a: (3u64, 8u64), tag: 999u64 }
            add_in_place(&mut p.a.0, 10u64)
            if p.tag != 999u64 { return 3u64 }
            if p.a.0 != 13u64 { return 4u64 }

            var t: (Point, u64) = (Point { x: 1u64, y: 200u64 }, 777u64)
            add_in_place(&mut t.0.x, 16u64)
            if t.1 != 777u64 { return 5u64 }
            if t.0.y != 200u64 { return 6u64 }
            if t.0.x != 17u64 { return 7u64 }

            42u64
        }
    "#;
    assert_consistent(src, "ref_stage2_nested_chain_mut_borrow_round_trip");
}

#[test]
fn ref_stage2_array_index_mut_borrow_round_trip() {
    // REF-Stage-2 (iii-index): array element mutable borrow
    // `&mut arr[i]`. The AOT path uses a new
    // `InstKind::ArrayElemAddr { slot, index, elem_ty }` that
    // codegens to `iadd(stack_addr(slot, 0), index *
    // elem_stride_bytes)` against the per-array stack slot;
    // the resulting `Type::U64` pointer hands off to the same
    // `LoadRef` / `StoreRef` machinery scalar address-of uses.
    // The interpreter writeback captures the parent
    // `Object::Array` Rc + the resolved usize index at call
    // time, then `borrow_mut` + indexed assignment after the
    // call.
    //
    // Mutates index 1 twice (one accumulating delta) and pins
    // that the unrelated indices 0/2 stayed put.
    let src = r#"
        fn add_in_place(target: &mut u64, delta: u64) {
            target = target + delta
        }
        fn main() -> u64 {
            var arr = [10u64, 20u64, 30u64]
            add_in_place(&mut arr[1u64], 20u64)
            add_in_place(&mut arr[1u64], 2u64)
            if arr[0u64] != 10u64 { return 1u64 }
            if arr[2u64] != 30u64 { return 2u64 }
            arr[1u64]
        }
    "#;
    assert_consistent(src, "ref_stage2_array_index_mut_borrow_round_trip");
}

#[test]
fn ref_stage2_compound_mut_ref_propagates_round_trip() {
    // REF-Stage-2 (ii): compound `&mut T` parameter mutation
    // propagates back to the caller's binding across all three
    // backends.
    //   - AOT generalises the Stage-1 self-writeback convention:
    //     each `&mut <compound>` parameter contributes its leaf
    //     scalar types to `Function::self_writeback_types` at
    //     declaration time (forward-call safety) and matching
    //     leaf locals at body lowering. The call site emits
    //     `CallWithSelfWriteback` with caller-side leaves as
    //     `self_dests` so the trailing return values flow back
    //     into the caller's `Binding::Struct` fields.
    //   - Interpreter / JIT need no extra wiring: struct values
    //     ride `Rc<RefCell<Object::Struct>>`, so the parameter
    //     binding shares the cell with the caller's local and
    //     `p.x = ...` inside the body is observable on both
    //     sides.
    //
    // The test mutates one field through the `&mut Point`
    // parameter twice (once with an early return / no-op
    // branch to make sure unrelated control flow doesn't kill
    // the writeback path) and pins both fields after.
    let src = r#"
        struct Point { x: u64, y: u64 }
        fn shift_x(p: &mut Point, dx: u64) {
            p.x = p.x + dx
        }
        fn main() -> u64 {
            var p: Point = Point { x: 10u64, y: 20u64 }
            shift_x(&mut p, 30u64)
            shift_x(&mut p, 2u64)
            if p.y != 20u64 { return 1u64 }
            p.x
        }
    "#;
    assert_consistent(src, "ref_stage2_compound_mut_ref_propagates_round_trip");
}

#[test]
fn ref_stage2_enum_mut_ref_propagates_round_trip() {
    // REF-Stage-2 (ii-enum): `&mut Enum` parameter mutation
    // propagates back to the caller's binding across all three
    // backends.
    //   - AOT: `collect_compound_writeback_dests` now flattens
    //     `Binding::Enum` (tag local + per-variant payload locals)
    //     into the writeback dest list, and the body-time
    //     writeback-leaves loop adds enum bindings to
    //     `Function::self_writeback_types` so the call site uses
    //     `CallWithSelfWriteback` and routes the trailing return
    //     leaves back into the caller's enum storage.
    //   - Interpreter / JIT: enum values share `Rc<RefCell<Object>>`
    //     so reassignment inside the body is observable on both
    //     sides automatically (same as struct/tuple).
    //
    // Exercises both unit-variant -> tuple-variant transitions and
    // tuple-variant payload swaps so the tag and payload both make
    // a round trip through the writeback shape.
    let src = r#"
        enum Box {
            Empty,
            Filled(u64),
            Pair(u64, u64),
        }

        fn fill(b: &mut Box, v: u64) {
            b = Box::Filled(v)
        }

        fn pair_it(b: &mut Box, a: u64, c: u64) {
            b = Box::Pair(a, c)
        }

        fn main() -> u64 {
            var b: Box = Box::Empty
            fill(&mut b, 10u64)
            pair_it(&mut b, 30u64, 12u64)
            match b {
                Box::Empty => 99u64,
                Box::Filled(x) => x,
                Box::Pair(x, y) => x + y,
            }
        }
    "#;
    assert_consistent(src, "ref_stage2_enum_mut_ref_propagates_round_trip");
}

#[test]
fn ref_stage2_let_rhs_struct_return_with_mut_writeback_round_trip() {
    // REF-Stage-2 (ii-let-rhs / struct return): a struct-returning
    // call that also takes a `&mut <compound>` parameter.
    //
    // Previously the let-rhs Call->Struct path emitted a bare
    // `CallStruct` with only the struct field dests, while the
    // callee's cranelift signature already had the writeback
    // leaves appended — the mismatch tripped a `block0 is not
    // sealed` panic at codegen time. Now `lower_let` appends
    // `collect_compound_writeback_dests` to the CallStruct dests
    // so the trailing writeback values flow back into the
    // caller's `&mut <var>` binding.
    let src = r#"
        struct Point { x: u64, y: u64 }

        fn shift_x(p: &mut Point, dx: u64) -> Point {
            p.x = p.x + dx
            Point { x: p.x, y: p.y }
        }

        fn main() -> u64 {
            var p = Point { x: 10u64, y: 5u64 }
            val snap: Point = shift_x(&mut p, 32u64)
            if p.x != 42u64 { return 1u64 }
            if snap.x != 42u64 { return 2u64 }
            if snap.y != 5u64 { return 3u64 }
            p.x
        }
    "#;
    assert_consistent(src, "ref_stage2_let_rhs_struct_return_with_mut_writeback_round_trip");
}

#[test]
fn ref_stage2_let_rhs_tuple_return_with_mut_writeback_round_trip() {
    // REF-Stage-2 (ii-let-rhs / tuple return): same shape as the
    // struct-return case but for tuple-returning calls. Pinned
    // separately because CallTuple uses its own lower path.
    let src = r#"
        struct Point { x: u64, y: u64 }

        fn shift_and_pair(p: &mut Point, dx: u64) -> (u64, u64) {
            p.x = p.x + dx
            val out: (u64, u64) = (p.x, p.y)
            out
        }

        fn main() -> u64 {
            var p = Point { x: 10u64, y: 32u64 }
            val pair: (u64, u64) = shift_and_pair(&mut p, 30u64)
            if p.x != 40u64 { return 1u64 }
            if pair.0 != 40u64 { return 2u64 }
            if pair.1 != 32u64 { return 3u64 }
            pair.0 + pair.1 - 30u64
        }
    "#;
    assert_consistent(src, "ref_stage2_let_rhs_tuple_return_with_mut_writeback_round_trip");
}

#[test]
fn ref_stage2_let_rhs_enum_return_with_mut_writeback_round_trip() {
    // REF-Stage-2 (ii-let-rhs / enum return): same shape as the
    // struct/tuple cases but for enum-returning calls. CallEnum
    // dests cover (tag, payload-leaves...), with writeback leaves
    // appended so both the enum return and the &mut Point param
    // make it back into the caller's bindings.
    let src = r#"
        struct Point { x: u64, y: u64 }

        enum Status {
            Ok(u64),
            Bad,
        }

        fn shift_and_status(p: &mut Point, dx: u64) -> Status {
            p.x = p.x + dx
            Status::Ok(p.x)
        }

        fn main() -> u64 {
            var p = Point { x: 10u64, y: 0u64 }
            val st: Status = shift_and_status(&mut p, 32u64)
            if p.x != 42u64 { return 1u64 }
            match st {
                Status::Ok(v) => v,
                Status::Bad => 99u64,
            }
        }
    "#;
    assert_consistent(src, "ref_stage2_let_rhs_enum_return_with_mut_writeback_round_trip");
}

#[test]
fn ref_stage2_method_mut_arg_writeback_round_trip() {
    // REF-Stage-2 (ii-method): method-call passing `&mut <var>`
    // for a compound parameter. Previously the AOT method-call
    // path emitted a plain `Call` (no writeback) so the
    // mutation never made it back to the caller. Now
    // `lower_method_call` builds `self_dests` from the receiver
    // (when `&mut self`) plus `collect_compound_writeback_dests_slice`
    // for compound-`&mut T` args and emits
    // `CallWithSelfWriteback`. Pre-population of method
    // `self_writeback_types` (in both the eager and the
    // generic-method instantiation path) lets the call site see
    // the right shape even when the method body hasn't been
    // lowered yet.
    let src = r#"
        struct Point { x: u64, y: u64 }
        struct Mover { delta: u64 }

        impl Mover {
            fn shift(self: Self, p: &mut Point) {
                p.x = p.x + self.delta
            }
        }

        fn main() -> u64 {
            var p = Point { x: 10u64, y: 0u64 }
            val m = Mover { delta: 30u64 }
            m.shift(&mut p)
            val m2 = Mover { delta: 2u64 }
            m2.shift(&mut p)
            p.x
        }
    "#;
    assert_consistent(src, "ref_stage2_method_mut_arg_writeback_round_trip");
}

#[test]
fn ref_stage2_immutable_ref_scalar_chain_round_trip() {
    // REF-Stage-2 (iv): ref-of-ref scalar chains and `T -> &T`
    // auto-borrow at the AOT call boundary.
    //
    // Two failure modes the new `param_is_ref`-aware
    // `lower_call_args_with_target` fixes:
    //   1. Forwarding `RefScalar` bindings: in
    //      `outer(x: &u64) { inner(x) }`, `x` is a RefScalar
    //      holding a pointer. The frontend auto-derefs `x` in
    //      value position, so before the fix lowering emitted
    //      LoadRef(x) and passed the dereferenced value where
    //      `inner` expected a pointer (segfault).
    //   2. T -> &T auto-borrow: passing a `T` value (`Scalar`
    //      binding) to a `&T` parameter previously emitted
    //      LoadLocal of the value; codegen handed it to the
    //      callee as if it were a pointer (segfault).
    //
    // The fix marks `&T` / `&mut T` params on every Function in
    // the IR via `param_is_ref`, then `lower_call_args_with_target`
    // peeks the flag per-arg and emits AddressOf (for Scalar) or
    // forwards the existing pointer (for RefScalar) instead of
    // dereferencing.
    let src = r#"
        fn double(x: &u64) -> u64 {
            x + x
        }

        fn read_via_chain(x: &u64) -> u64 {
            double(x) + 0u64
        }

        fn main() -> u64 {
            val n = 21u64
            val a = double(&n)
            val b = double(n)
            val c = read_via_chain(&n)
            if a != 42u64 { return 1u64 }
            if b != 42u64 { return 2u64 }
            if c != 42u64 { return 3u64 }
            a
        }
    "#;
    assert_consistent(src, "ref_stage2_immutable_ref_scalar_chain_round_trip");
}
