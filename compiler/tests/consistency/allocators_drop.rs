//! Scope-bound allocator cleanup and generic RAII `Drop`.

use super::harness::*;

#[test]
fn arena_temporary_auto_cleanup_round_trip() {
    // Phase 5 (Design A scope-bound): `with allocator =
    // Arena::new() { ... }` releases the inline arena's tracked
    // allocations at scope exit — no explicit `arena.drop()`.
    // Linear exit, opening + closing two separate inline arenas,
    // and a `__builtin_current_allocator()` round-trip back to
    // the default sentinel after each `with` block. Pinned across
    // interpreter / JIT silent fallback / AOT.
    let src = r#"
        fn main() -> u64 {
            with allocator = Arena::new() {
                val p1: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p1) { return 1u64 }
                val p2: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p2) { return 2u64 }
            }
            val mid: Allocator = __builtin_current_allocator()
            if mid != __builtin_default_allocator() { return 3u64 }
            with allocator = Arena::new() {
                val p3: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p3) { return 4u64 }
            }
            val end: Allocator = __builtin_current_allocator()
            if end != __builtin_default_allocator() { return 5u64 }
            42u64
        }
    "#;
    assert_consistent(src, "arena_temporary_auto_cleanup_round_trip");
}

// `raw_builtin_arena_auto_cleanup_round_trip` was removed when the
// runtime arena / fixed_buffer infrastructure was retired. The same
// auto-cleanup contract is exercised by
// `arena_temporary_auto_cleanup_round_trip` and
// `fixed_buffer_temporary_auto_cleanup_round_trip` via the stdlib
// wrapper forms.

#[test]
fn arena_temporary_auto_cleanup_early_return_round_trip() {
    // Phase 5: early `return` from inside `with allocator =
    // Arena::new() { ... }` still pops the active stack AND
    // releases the inline arena slot. The AOT path emits the
    // matching `AllocPop` + `AllocArenaDrop` via
    // `emit_with_scope_cleanup` walking the
    // `with_scope_arena_drops` stack; the interpreter's `with`
    // arm runs `reset()` after the body returned (regardless of
    // whether the body returned an error or a value).
    let src = r#"
        fn helper() -> u64 {
            with allocator = Arena::new() {
                val p: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p) { return 9u64 }
                # Early return from inside the with-arena body.
                return 7u64
            }
            100u64
        }
        fn main() -> u64 {
            val r: u64 = helper()
            val cur: Allocator = __builtin_current_allocator()
            if cur != __builtin_default_allocator() { return 1u64 }
            if r != 7u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "arena_temporary_auto_cleanup_early_return_round_trip");
}

#[test]
fn fixed_buffer_temporary_auto_cleanup_round_trip() {
    // Phase 5 (FixedBuffer auto-cleanup): symmetric to the
    // Arena variant. `with allocator = FixedBuffer::new(16u64) {
    // ... }` returns a 16-byte quota allocator, and the slot is
    // released at scope exit (no explicit drop method needed).
    //
    // The body here exercises the quota:
    //   - first 8-byte alloc fits (8 used / 16)
    //   - second 8-byte alloc fits (16 used / 16)
    //   - third 1-byte alloc would push past the quota -> NULL
    // sum = 1 + 1 + 40 = 42 confirms all three branches fired.
    let src = r#"
        fn main() -> u64 {
            var sum: u64 = 0u64
            with allocator = FixedBuffer::new(16u64) {
                val p1: ptr = __builtin_heap_alloc(8u64)
                if !__builtin_ptr_is_null(p1) { sum = sum + 1u64 }
                val p2: ptr = __builtin_heap_alloc(8u64)
                if !__builtin_ptr_is_null(p2) { sum = sum + 1u64 }
                val p3: ptr = __builtin_heap_alloc(1u64)
                if __builtin_ptr_is_null(p3) { sum = sum + 40u64 }
            }
            val cur: Allocator = __builtin_current_allocator()
            if cur != __builtin_default_allocator() { return 99u64 }
            sum
        }
    "#;
    assert_consistent(src, "fixed_buffer_temporary_auto_cleanup_round_trip");
}

#[test]
fn fixed_buffer_temporary_auto_cleanup_early_return_round_trip() {
    // Phase 5: early `return` from inside `with allocator =
    // FixedBuffer::new(cap) { ... }` still pops the active stack
    // and releases the fixed_buffer slot. AOT routes through
    // `emit_with_scope_cleanup` walking
    // `with_scope_arena_drops` (now `WithScopeCleanup` enum-
    // typed) and emits `AllocPop` + `AllocFixedBufferDrop` on
    // the `return` path; interpreter's `Expr::With` arm runs
    // `reset()` after the body completes regardless of the exit
    // mode.
    let src = r#"
        fn helper() -> u64 {
            with allocator = FixedBuffer::new(8u64) {
                val p: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p) { return 9u64 }
                return 7u64
            }
            100u64
        }
        fn main() -> u64 {
            val r: u64 = helper()
            val cur: Allocator = __builtin_current_allocator()
            if cur != __builtin_default_allocator() { return 1u64 }
            if r != 7u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "fixed_buffer_temporary_auto_cleanup_early_return_round_trip");
}

#[test]
fn drop_trait_named_binding_round_trip() {
    // Both `Arena` and `FixedBuffer` impl the stdlib `Drop` trait
    // (`core/std/drop.t`). Named bindings get `drop()` auto-called
    // at scope exit through the Phase 5 RAII path, so the toylang
    // body's tracking-array release fires without an explicit call.
    // The `with allocator = Arena::new() { ... }` temporary form
    // also fires `drop()` at scope exit via the inline-temporary
    // auto-drop hook.
    let src = r#"
        fn main() -> u64 {
            val arena = Arena::new()
            with allocator = arena {
                val p: ptr = __builtin_heap_alloc(8u64)
                if __builtin_ptr_is_null(p) { return 1u64 }
            }

            val fb = FixedBuffer::new(8u64)
            with allocator = fb {
                val q: ptr = __builtin_heap_alloc(4u64)
                if __builtin_ptr_is_null(q) { return 2u64 }
            }
            42u64
        }
    "#;
    assert_consistent(src, "drop_trait_named_binding_round_trip");
}

#[test]
fn generic_raii_drop_lifo_round_trip() {
    // Phase 5 (汎用 RAII, AOT補完): user struct with `impl Drop`
    // gets `drop()` auto-called at scope exit in LIFO order
    // across all three backends. The Marker.drop body mutates a
    // shared cell via `__builtin_ptr_write` so the drop order
    // is encoded as a base-10 sequence (last-bound drops first
    // → its id ends up in the most-significant digit at exit).
    //
    // - 21 = b(2) dropped before a(1) — function exit linear
    //   path runs LIFO drops via `pop_and_emit_drops` (AOT)
    //   / `run_and_pop_drop_scope` (interp).
    // - JIT silent fallback to interpreter inherits the same
    //   semantics.
    let src = r#"
        struct Marker { id: u64, log: ptr }
        impl Drop for Marker {
            unsafe fn drop(&mut self) {
                val cur: u64 = __builtin_ptr_read::<u64>(self.log, 0u64)
                __builtin_ptr_write(self.log, 0u64, cur * 10u64 + self.id)
            }
        }
        unsafe fn run(log: ptr) {
            val a = Marker { id: 1u64, log: log }
            val b = Marker { id: 2u64, log: log }
        }
        unsafe fn main() -> u64 {
            val log: ptr = __builtin_heap_alloc(8u64)
            __builtin_ptr_write(log, 0u64, 0u64)
            run(log)
            val recorded: u64 = __builtin_ptr_read::<u64>(log, 0u64)
            recorded
        }
    "#;
    assert_consistent(src, "generic_raii_drop_lifo_round_trip");
}

#[test]
fn generic_raii_drop_on_early_return_round_trip() {
    // Phase 5 (汎用 RAII): early `return` from inside the
    // function body triggers auto-drop of bindings introduced
    // before the return, in LIFO order.  Result `43` =
    // b(4) → a(3).  AOT path goes through `terminate_return`
    // calling `emit_drop_scopes_to_depth(0)` before the return
    // is materialised; interpreter path goes through
    // `evaluate_block`'s `run_and_pop_drop_scope` on the
    // `Ok(Return(_))` path.
    let src = r#"
        struct Marker { id: u64, log: ptr }
        impl Drop for Marker {
            unsafe fn drop(&mut self) {
                val cur: u64 = __builtin_ptr_read::<u64>(self.log, 0u64)
                __builtin_ptr_write(self.log, 0u64, cur * 10u64 + self.id)
            }
        }
        unsafe fn run(log: ptr) -> u64 {
            val a = Marker { id: 3u64, log: log }
            val b = Marker { id: 4u64, log: log }
            return 7u64
        }
        unsafe fn main() -> u64 {
            val log: ptr = __builtin_heap_alloc(8u64)
            __builtin_ptr_write(log, 0u64, 0u64)
            val r: u64 = run(log)
            val recorded: u64 = __builtin_ptr_read::<u64>(log, 0u64)
            if r != 7u64 { return 1u64 }
            recorded
        }
    "#;
    assert_consistent(src, "generic_raii_drop_on_early_return_round_trip");
}

#[test]
fn a_str_literal_address_is_not_the_programs_allocation() {
    // MEM-COUNTER-INTERP-DRIFT: the counters report what the program
    // asked the allocator for. A `str` lives in `.rodata` on the
    // compiled lanes, so handing out its address costs nothing there —
    // but the tree-walker has to materialise the bytes somewhere, and
    // it used to charge the program `len + 1` for every `as_ptr`,
    // never returning it. `String::from_str` goes through exactly that
    // path, so a loop of them drifted up on one lane and stayed flat
    // on the others, and no steady-state promise could be checked
    // against the tree-walker (which is the `--check` oracle).
    let src = r#"
        fn main() -> u64 {
            val before = __builtin_live_bytes()
            var i = 0u64
            while i < 4u64 {
                val s = String::from_str("0123456789")
                i = i + 1u64
            }
            val after = __builtin_live_bytes()
            if after != before { return 1u64 }
            7u64
        }
    "#;
    assert_consistent(src, "a_str_literal_address_is_not_the_programs_allocation");
}

#[test]
fn a_match_arm_names_the_payload_it_does_not_copy_it() {
    // MATCH-PAYLOAD-COPY: the arm used to bind a *copy* of the
    // payload, with drop glue of its own, while the scrutinee kept
    // its own. Two owners of one resource is invisible for a heap
    // block -- `free` is idempotent on a heap that never reuses an
    // address -- and fatal for anything the OS hands back once, which
    // is how a server closed a connection it had just accepted.
    //
    // The counter is the visible half: writing through the arm's name
    // has to reach the value the scrutinee holds, and the resource
    // has to be released once, not twice.
    let src = r#"
        struct Handle { open: bool }

        impl Handle {
            fn new() -> Self {
                val p = __builtin_heap_alloc(16u64)
                __builtin_heap_free(p)
                Handle { open: true }
            }
            fn shut(&mut self) -> u64 {
                if !self.open { return 1u64 }
                self.open = false
                0u64
            }
        }

        fn main() -> u64 {
            val held: Result<Handle, u64> = Result::Ok(Handle::new())
            var closed_twice = 0u64
            match held {
                Result::Ok(h) => {
                    var one = h
                    closed_twice = closed_twice + one.shut()
                }
                Result::Err(e) => { return 90u64 }
            }
            # The arm named the scrutinee's payload, so the flag it
            # cleared is the one the scrutinee still holds: shutting it
            # again answers "already shut".
            match held {
                Result::Ok(h2) => {
                    var two = h2
                    closed_twice = closed_twice + two.shut()
                }
                Result::Err(e) => { return 91u64 }
            }
            closed_twice
        }
    "#;
    assert_consistent(src, "a_match_arm_names_the_payload_it_does_not_copy_it");
}

#[test]
fn cloning_a_vector_of_strings_leaves_the_original_whole() {
    // ELEMENT-BORROW: `Vec::clone` used to read each element with
    // `get`, which hands back a value sharing the element's buffer.
    // The binding freed it one iteration later, with the vector still
    // pointing at it -- invisible on a heap that never reuses an
    // address, and wrong in every accounting of it.
    //
    // The live-byte count is the visible half: cloning two strings
    // must *add* their bytes, not swap them.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<String> = Vec::new()
            v.push(String::from_str("hello"))
            v.push(String::from_str("world"))

            val before = __builtin_live_bytes()
            val copy = v.clone()
            val after = __builtin_live_bytes()
            if after <= before { return 1u64 }
            # The ten bytes of the two elements are still the original's,
            # and the copy has ten of its own.
            if after - before < 10u64 { return 2u64 }

            val a: &String = v.borrow(0u64)
            val b: &String = copy.borrow(1u64)
            if a.len() != 5u64 { return 3u64 }
            if b.len() != 5u64 { return 4u64 }
            0u64
        }
    "#;
    assert_consistent(src, "cloning_a_vector_of_strings_leaves_the_original_whole");
}

#[test]
fn a_value_named_twice_is_dropped_once_on_every_lane() {
    // MATCH-MOVE-OUT-DOUBLE-DROP: each shape below names one value twice
    // -- a binding and its alias -- and used to drop it twice on every
    // lane (a second `close` for a descriptor). One `drop N` line per
    // value, at the point its last owner lets go.
    let src = r#"
        struct H { id: u64 }
        impl Drop for H {
            fn drop(&mut self) { println("drop {self.id}") }
        }
        fn keep(h: H) -> u64 {
            var v: Vec<H> = Vec::with_capacity(1u64)
            v.push(h)
            v.size()
        }
        fn extracted() {
            val made: Result<H, u64> = Result::Ok(H { id: 1u64 })
            var conn = match made {
                Result::Ok(c) => c,
                Result::Err(e) => { panic("no") }
            }
            println("using {conn.id}")
        }
        fn extracted_and_moved() {
            val made: Result<H, u64> = Result::Ok(H { id: 2u64 })
            var conn = match made {
                Result::Ok(c) => c,
                Result::Err(e) => { panic("no") }
            }
            val n = keep(conn)
            println("kept {n}")
        }
        fn moved_in_the_arm() {
            val made: Option<H> = Option::Some(H { id: 3u64 })
            val n = match made {
                Option::Some(c) => keep(c),
                Option::None => 0u64,
            }
            println("kept {n}")
        }
        fn alias_moved() {
            val a = H { id: 4u64 }
            val b = a
            val n = keep(b)
            println("kept {n}")
        }
        fn main() -> u64 {
            extracted()
            extracted_and_moved()
            moved_in_the_arm()
            alias_moved()
            0u64
        }
    "#;
    assert_renders(
        src,
        "value_named_twice_dropped_once",
        "using 1\ndrop 1\ndrop 2\nkept 1\ndrop 3\nkept 1\ndrop 4\nkept 1\n",
    );
}

#[test]
fn a_value_lent_to_a_reading_callee_is_dropped_by_the_caller() {
    // BY-VALUE-PARAM-NO-DROP: a callee that only reads a by-value
    // parameter (`reads`, and `passes_on`, which only hands it to
    // `reads`) lends it, so the caller keeps the drop -- the value used
    // to be freed by nobody. A callee that stores it (`keeps`) or
    // closes it (`closes`, through a `&mut self` method) still takes
    // it. Every value is closed exactly once, on every lane.
    let src = r#"
        struct H { id: u64, open: bool }
        impl H {
            fn shut(&mut self) {
                if self.open { println("close {self.id}") }
                self.open = false
            }
            fn id_of(&self) -> u64 { self.id }
        }
        impl Drop for H {
            fn drop(&mut self) { self.shut() }
        }
        fn reads(h: H) -> u64 { h.id_of() + h.id }
        fn keeps(h: H) -> u64 {
            var v: Vec<H> = Vec::with_capacity(1u64)
            v.push(h)
            v.size()
        }
        fn closes(h: H) -> u64 {
            var m = h
            m.shut()
            0u64
        }
        fn passes_on(h: H) -> u64 { reads(h) }
        fn main() -> u64 {
            val a = H { id: 1u64, open: true }
            val x = reads(a)
            println("after reads {x}")
            val b = H { id: 2u64, open: true }
            val y = keeps(b)
            println("after keeps {y}")
            val c = H { id: 3u64, open: true }
            val z = closes(c)
            println("after closes {z}")
            val d = H { id: 4u64, open: true }
            val w = passes_on(d)
            println("after passes_on {w}")
            0u64
        }
    "#;
    assert_renders(
        src,
        "lent_param_dropped_by_caller",
        "after reads 2\nclose 2\nafter keeps 1\nclose 3\nafter closes 0\nafter passes_on 8\nclose 4\nclose 1\n",
    );
}

/// #121: a program that never pushes an allocator heap-allocates
/// through the default one at every operation, so the lowering names
/// it (`alloc=static(0)`) and codegen skips asking the runtime stack.
/// One `with` anywhere keeps every operation on the stack
/// (`alloc=ambient`) -- a function cannot see the `with` its caller is
/// inside. Both programs answer the same on every lane.
#[test]
fn heap_operations_name_the_default_allocator_when_nothing_pushes_one() {
    let plain = r#"
        fn main() -> u64 {
            val b: Box<u64> = Box::new(3u64)
            b.get()
        }
    "#;
    let ir = lowered_ir(plain);
    assert!(ir.contains("alloc=static(0)"), "{ir}");
    assert!(!ir.contains("alloc=ambient"), "{ir}");
    assert_consistent(plain, "alloc_static_default");

    let scoped = r#"
        fn make() -> u64 {
            val b: Box<u64> = Box::new(4u64)
            b.get()
        }
        fn main() -> u64 {
            var r = 0u64
            with allocator = Arena::new() {
                r = make()
            }
            r
        }
    "#;
    let ir = lowered_ir(scoped);
    assert!(ir.contains("alloc=ambient"), "{ir}");
    assert!(!ir.contains("alloc=static"), "{ir}");
    assert_consistent(scoped, "alloc_ambient_with");
}
