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
    // The tree-walker keeps one spec per (struct, associated fn) name,
    // so `C::make` under both `impl C<u8>` and `impl C<i64>` resolves
    // to neither. Recorded as TREE-WALKER-CONCRETE-IMPL in
    // design-docs/todo.md.
    assert_consistent_without_tree_walker(
        src,
        "concrete_associated_hint",
        "annotation-driven associated-function dispatch across concrete impls",
    );
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
        unsafe fn main() -> u64 {
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
        unsafe fn main() -> u64 {
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
        unsafe fn len_of(s: &String) -> u64 {
            s.size()
        }

        unsafe fn first_byte(s: &String) -> u8 {
            val b: u8 = __builtin_ptr_read(s.as_ptr(), 0u64)
            b
        }

        unsafe fn first_byte_mut(s: &mut String) -> u8 {
            val b: u8 = __builtin_ptr_read(s.as_ptr(), 0u64)
            b
        }

        unsafe fn main() -> u64 {
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
    // Two failure modes the new `param_ref_pointee`-aware
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
    // the IR via `param_ref_pointee`, then `lower_call_args_with_target`
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

#[test]
fn scalar_ref_arguments_reach_a_method_and_survive_having_no_home() {
    // METHOD-ARG-AUTOBORROW. The `T` -> `&T` auto-borrow the frontend
    // approves was materialised only at free-function call sites, and
    // only for an argument that was a bare identifier bound to a
    // scalar local. Two shapes therefore passed a *value* into a slot
    // the callee reads through a pointer:
    //
    //   a.plus(y)          — every method call, even a plain binding
    //   plus(22i64, ..)    — any argument with no address of its own
    //
    // The tree-walker was right throughout (it erases references), so
    // this reproduced as a wrong answer on the IR VM — the engine the
    // interpreter actually runs — and a segfault once compiled, which
    // is why it looked like two separate bugs.
    let src = r#"
        struct W { v: i64 }

        impl W {
            fn plus(&self, other: &i64) -> i64 { self.v + other }
        }

        fn plus(a: &i64, b: &i64) -> i64 { a + b }

        fn via_chain(a: &i64) -> i64 { plus(a, a) }

        fn main() -> i64 {
            val w: W = W { v: 20i64 }
            val other: W = W { v: 22i64 }
            val y: i64 = 22i64
            val n: i64 = 21i64

            # Method calls: a binding, a literal, and a field read.
            if w.plus(y) != 42i64 { return 1i64 }
            if w.plus(22i64) != 42i64 { return 2i64 }
            if w.plus(other.v) != 42i64 { return 3i64 }
            if w.plus(&y) != 42i64 { return 4i64 }

            # Free functions: the literal / computed cases were broken
            # here too, while the bare-identifier one already worked.
            if plus(20i64, 22i64) != 42i64 { return 5i64 }
            if plus(n + 1i64, n) != 42i64 { return 6i64 }
            if plus(y, y - 2i64) != 42i64 { return 7i64 }

            # A reference forwarded through another `&T` parameter must
            # still be forwarded, not addressed a second time.
            if via_chain(n) != 42i64 { return 8i64 }
            if via_chain(&n) != 42i64 { return 9i64 }

            42i64
        }
    "#;
    assert_consistent(src, "scalar_ref_arguments_reach_a_method_and_survive_having_no_home");
}

#[test]
fn scalar_ref_arguments_keep_their_width() {
    // The spilled temporary has to be as wide as the pointee, not the
    // pointer: `&u8` reads one byte back out, `&f64` eight. Getting
    // this from the callee's own declaration is the reason
    // `param_ref_pointee` carries a type rather than a flag.
    let src = r#"
        fn take_u8(x: &u8) -> u8 { x + 1u8 }
        fn take_i16(x: &i16) -> i16 { x + 1i16 }
        fn take_f64(x: &f64) -> f64 { x + 1.5f64 }
        fn take_bool(x: &bool) -> bool { !x }

        fn main() -> u64 {
            if take_u8(41u8) != 42u8 { return 1u64 }
            if take_i16(0i16 - 2i16) != (0i16 - 1i16) { return 2u64 }
            if take_f64(40.5f64) != 42.0f64 { return 3u64 }
            if take_bool(false) != true { return 4u64 }
            42u64
        }
    "#;
    assert_consistent(src, "scalar_ref_arguments_keep_their_width");
}

// FROM-INTO-ENUM-ERR: the `?` cross-error conversion into an ENUM
// error type. The desugar emits `val e: MyErr = MyErr::from(s)`, which
// previously only the interpreter lowered (the AOT / JIT read the enum
// qualifier as a variant construction and died on "unknown enum
// variant `MyErr::from`"). The let-rhs dispatch now tells variants
// from associated functions and routes `from` through the same
// `CallEnum` machinery as any enum-returning call. The struct-error
// counterpart (`ErrWrap: From<str>`) already ran on every lane and
// rides along here as the control group.
#[test]
fn try_cross_error_into_enum_target_is_consistent_across_backends() {
    let src = r#"
        enum MyErr {
            Fail(u64),
        }

        impl From<str> for MyErr {
            fn from(value: str) -> MyErr {
                val wrapped: u64 = if value == "empty" { 7u64 } else { 9u64 }
                MyErr::Fail(wrapped)
            }
        }

        struct ErrWrap { code: u64 }

        impl From<str> for ErrWrap {
            fn from(value: str) -> ErrWrap {
                ErrWrap { code: 42u64 }
            }
        }

        fn inner(ok: bool) -> Result<i64, str> {
            if ok {
                Result::Ok(11i64)
            } else {
                Result::Err("empty")
            }
        }

        fn outer_enum(ok: bool) -> Result<i64, MyErr> {
            val x = inner(ok)?
            Result::Ok(x)
        }

        fn outer_struct(ok: bool) -> Result<i64, ErrWrap> {
            val x = inner(ok)?
            Result::Ok(x)
        }

        fn main() -> i64 {
            # Ok path flows through both conversions untouched.
            val a = match outer_enum(true) {
                Result::Ok(v) => v,
                Result::Err(_) => -1i64,
            }
            # Err path converts str -> MyErr::Fail(7).
            val b = match outer_enum(false) {
                Result::Err(MyErr::Fail(v)) => v as i64,
                Result::Ok(_) => -2i64,
            }
            # Err path converts str -> ErrWrap { code: 42 }.
            val c = match outer_struct(false) {
                Result::Err(w) => w.code as i64,
                Result::Ok(_) => -3i64,
            }
            a + b + c
        }
    "#;
    // Expected exit: 11 (Ok passthrough) + 7 (enum conversion) + 42
    // (struct conversion) = 60.
    assert_consistent(src, "try_cross_error_into_enum_target");
}

#[test]
fn getitem_setitem_magic_methods_3_backend() {
    // POINTER P2: the two frontend holes that made `p[i]` a
    // type-checker-only illusion — (a) the arity gate counted
    // `parameter` slots, so the `&self` short form was rejected
    // while `self: Self` passed, and (b) a generic struct's
    // declared `__getitem__` return type came back as
    // `Generic(T)`. With both fixed, the compiled lanes also
    // needed the dispatch itself: `p[i]` / `p[i] = v` on a struct
    // binding lower as `__getitem__` / `__setitem__` calls (the
    // tree-walker always dispatched this way).
    let src = r#"
        struct Slot<T> { v: T }

        impl<T> Slot<T> {
            unsafe fn __getitem__(&self, index: u64) -> T {
                self.v
            }
            unsafe fn __setitem__(&mut self, index: u64, value: T) {
                self.v = value
            }
        }

        struct Bytes {
            data: ptr,
            len: u64,
        }

        impl Bytes {
            unsafe fn __getitem__(&self, i: u64) -> u64 {
                val v: u64 = __builtin_ptr_read(self.data, i)
                v
            }
            unsafe fn __setitem__(&mut self, i: u64, value: u64) {
                __builtin_ptr_write(self.data, i, value)
            }
        }

        unsafe fn main() -> u64 {
            # &self receiver + generic return substitution.
            val s: Slot<u64> = Slot { v: 40u64 }
            if s[2u64] != 40u64 { return 1u64 }
            # &mut self setitem mutating the caller's binding.
            var m: Slot<u64> = Slot { v: 1u64 }
            m[0u64] = 7u64
            if m[0u64] != 7u64 { return 2u64 }
            # Heap-backed container: the shape `Ptr<T>` wants.
            # The index here is a raw byte offset (the method body
            # decides), mirroring `__builtin_ptr_read`'s contract.
            var b: Bytes = Bytes { data: __builtin_heap_alloc(16u64), len: 16u64 }
            b[8u64] = 55u64
            if b[8u64] != 55u64 { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "getitem_setitem_magic_methods_3_backend");
}

#[test]
fn stdlib_ptr_typed_window_3_backend() {
    // POINTER P3: `core/std/ptr.t`'s `Ptr<T>` — a typed window over
    // raw memory, implemented as an ordinary struct + impl on the raw
    // builtins (no backend special-casing, the `Box<T>` approach).
    // The stride comes from `__builtin_sizeof::<T>()`, the read/write
    // shape from the pointee type, and `p[i]` / `p[i] = v` are the
    // `__getitem__` / `__setitem__` sugar. Instantiations cover both
    // `Ptr::alloc` shapes: T from the `val` annotation (no T-bearing
    // argument exists) and T from the receiver.
    let src = r#"
        fn sum_through_window(p: Ptr<u64>, count: u64) -> u64 {
            var total: u64 = 0u64
            var i: u64 = 0u64
            while i < count {
                total = total + p[i]
                i = i + 1u64
            }
            total
        }

        fn main() -> u64 {
            val p: Ptr<u64> = Ptr::alloc(4u64)
            p.set(0u64, 7u64)
            p[1u64] = 9u64
            p[2u64] = 11u64
            p[3u64] = 13u64
            if p[0u64] != 7u64 { return 1u64 }
            if p[1u64] != 9u64 { return 2u64 }
            # offset: a window 2 elements forward, same allocation.
            val q: Ptr<u64> = p.offset(2u64)
            if q[0u64] != 11u64 { return 3u64 }
            if sum_through_window(p, 4u64) != 40u64 { return 4u64 }
            # as_raw hands the address to the raw builtins.
            if __builtin_ptr_is_null(p.as_raw()) { return 5u64 }
            # A second instantiation: the stride follows the type arg.
            val t: Ptr<i64> = Ptr::alloc(2u64)
            t.set(0u64, -5i64)
            t.set(1u64, 3i64)
            if t[0u64] != -5i64 { return 6u64 }
            if t[1u64] != 3i64 { return 7u64 }
            42u64
        }
    "#;
    assert_consistent(src, "stdlib_ptr_typed_window_3_backend");
}

#[test]
fn stdlib_span_typed_window_3_backend() {
    // POINTER P4: `core/std/span.t`'s `Span<T>` — a bounds-checked
    // view over a `Ptr<T>` window. The struct holds a `Ptr<T>` field
    // (a generic struct as a field type with the outer `T` as its
    // argument), element math goes through `self.data.addr`, and a
    // span crosses function boundaries by value the way a slice
    // would. Escape is deliberately unchecked (POINTER.md 選択肢 1).
    let src = r#"
        fn sum(s: Span<u64>) -> u64 {
            var total: u64 = 0u64
            var i: u64 = 0u64
            while i < s.len() {
                total = total + s[i]
                i = i + 1u64
            }
            total
        }

        fn main() -> u64 {
            val p: Ptr<u64> = Ptr::alloc(4u64)
            p.set(0u64, 7u64)
            p.set(1u64, 9u64)
            p.set(2u64, 11u64)
            p.set(3u64, 13u64)
            val s: Span<u64> = Span::from_parts(p, 4u64)
            if s.get(0u64) != 7u64 { return 1u64 }
            s.set(1u64, 20u64)
            # The span and the window share memory.
            if p[1u64] != 20u64 { return 2u64 }
            if s.len() != 4u64 { return 3u64 }
            if s.is_empty() { return 4u64 }
            if sum(s) != 51u64 { return 5u64 }
            # A sub-window: offset the pointer, shrink the count.
            val p2: Ptr<u64> = p.offset(2u64)
            val tail: Span<u64> = Span::from_parts(p2, 2u64)
            if tail[0u64] != 11u64 { return 6u64 }
            if sum(tail) != 24u64 { return 7u64 }
            # A second instantiation: the stride follows the type arg.
            val fp: Ptr<f64> = Ptr::alloc(2u64)
            val fs: Span<f64> = Span::from_parts(fp, 2u64)
            if fs.len() != 2u64 { return 8u64 }
            42u64
        }
    "#;
    assert_consistent(src, "stdlib_span_typed_window_3_backend");
}

#[test]
fn option_of_typed_ptr_3_backend() {
    // POINTER P5: `Ptr<T>` is non-null by construction, so absence
    // is `Option<Ptr<T>>` — the `next: ptr` + `has_next: bool`
    // pairing a raw-`ptr` list needs dies, and the match arm is the
    // truth. The enum payload carries a struct whose field is a
    // raw `ptr`, the struct is recursive through the option, and an
    // enum-typed field read feeds a call argument directly (the
    // compiled lanes expand the field's EnumStorage leaves in
    // argument position). `Option<Ptr<T>>` is 16 bytes: u64 tag +
    // payload, no niche optimisation (the enum layout is fixed).
    let src = r#"
        struct Node {
            v: i64,
            next: Option<Ptr<Node>>,
        }

        fn total(n: Option<Ptr<Node>>) -> i64 {
            match n {
                Option::None => 0i64,
                Option::Some(p) => {
                    val node: Node = p.get(0u64)
                    node.v + total(node.next)
                }
            }
        }

        fn cons(v: i64, rest: Option<Ptr<Node>>) -> Ptr<Node> {
            val p: Ptr<Node> = Ptr::alloc(1u64)
            p.set(0u64, Node { v: v, next: rest })
            p
        }

        fn main() -> u64 {
            val empty: Option<Ptr<Node>> = Option::None
            # Non-null: an empty request still yields an address.
            val z: Ptr<u64> = Ptr::alloc(0u64)
            if __builtin_ptr_is_null(z.as_raw()) { return 1u64 }
            val n1: Ptr<Node> = cons(1i64, empty)
            val some_n1: Option<Ptr<Node>> = Option::Some(n1)
            val n2: Ptr<Node> = cons(2i64, some_n1)
            val some_n2: Option<Ptr<Node>> = Option::Some(n2)
            val head: Ptr<Node> = cons(3i64, some_n2)
            val some_head: Option<Ptr<Node>> = Option::Some(head)
            if total(some_head) != 6i64 { return 2u64 }
            if total(empty) != 0i64 { return 3u64 }
            val node: Node = head.get(0u64)
            val has_next: u64 = match node.next {
                Option::None => 0u64,
                Option::Some(_) => 1u64,
            }
            if has_next != 1u64 { return 4u64 }
            42u64
        }
    "#;
    assert_consistent(src, "option_of_typed_ptr_3_backend");
}

// --- IMPL-BLOCK-VISIBILITY: an impl method sees every other impl -----------
//
// Method bodies used to be checked and registered in one sweep, block
// by block in statement order, so a method was visible only to blocks
// that came *after* its own. `integrate_modules` appends the stdlib
// behind the user's statements, which meant a user `impl` could not
// reach the stdlib at all — `Vec::new()` inside one was "Associated
// function 'new' not found for struct", and a `Span<u8>` parameter had
// no methods. The identical code in a free function worked, because
// free functions are checked in a later pass with everything already
// registered, and that asymmetry is what kept this hidden: the stdlib
// itself is nearly all impl blocks, and each one only ever needed the
// blocks above it.
//
// Registration is now its own pass over every block, ahead of any body.

/// The stdlib, reached from inside a user `impl` method: an
/// associated function, a method on the value it returns, and a
/// generic stdlib struct as a parameter type.
#[test]
fn an_impl_method_can_use_stdlib_types() {
    let src = r#"
        struct Bag { tag: u64 }

        impl Bag {
            fn collect(&self) -> u64 {
                var v: Vec<u64> = Vec::new()
                v.push(7u64)
                v.push(35u64)
                var total: u64 = 0u64
                for x in v.iter() {
                    total = total + x
                }
                total
            }

            # A stdlib struct as a parameter type: the receiver is the
            # parameter, not `self`, which is the shape `TcpStream::read`
            # (NETWORK_IO N1) needs and the one that failed first.
            fn measure(&self, s: String) -> u64 {
                s.len()
            }
        }

        fn main() -> u64 {
            val b = Bag { tag: 0u64 }
            val s = String::from_str("abc")
            b.collect() + b.measure(s)
        }
    "#;
    // 7 + 35 + 3.
    assert_eq!(interpreter_value(src) & 0xff, 45);
    assert_consistent(src, "impl_method_uses_stdlib");
}

/// Two user impl blocks calling each other, in both directions. The
/// backward one is what a single registering sweep could never do —
/// `First` is declared before `Second`, so `Second`'s methods did not
/// exist yet when `First`'s body was checked.
#[test]
fn impl_blocks_can_call_each_other_in_either_direction() {
    let src = r#"
        struct First { n: u64 }
        struct Second { n: u64 }

        impl First {
            fn forward(&self) -> u64 {
                val s = Second { n: self.n }
                s.doubled()
            }
            fn base(&self) -> u64 { self.n }
        }

        impl Second {
            fn doubled(&self) -> u64 { self.n * 2u64 }
            fn backward(&self) -> u64 {
                val f = First { n: self.n }
                f.base() + 1u64
            }
        }

        fn main() -> u64 {
            val f = First { n: 10u64 }
            val s = Second { n: 4u64 }
            f.forward() + s.backward()
        }
    "#;
    // 20 + 5.
    assert_eq!(interpreter_value(src) & 0xff, 25);
    assert_consistent(src, "impl_blocks_mutual");
}
