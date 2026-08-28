//! `Dict`, and closures from their first direct call through captures,
//! higher-order use, returns and struct fields.

use super::harness::*;

#[test]
fn return_inside_while_propagates() {
    // DICT-RETURN-WHILE fix (`evaluate_block::While` arm now
    // propagates Return / Break / Continue to the enclosing
    // block; `call_method` / `call_struct_method` convert
    // Return → Value at the function boundary so the signal
    // doesn't unwind past the callee). The pre-fix shape:
    //
    //   fn early(n: u64) -> u64 {
    //       var i = 0u64
    //       while i < n {
    //           if i == 5u64 { return 42u64 }
    //           i = i + 1u64
    //       }
    //       99u64
    //   }
    //
    // ...returned 99 (the post-loop value) instead of 42
    // because the while-loop arm in `evaluate_block` stored
    // the return result in `last` without surfacing it.
    //
    // The test exercises both paths the fix touches:
    //   - return from a free function's while loop (`early`).
    //   - return from a struct method's while loop (`Counter::find_first`).
    let src = r#"
        fn early(n: u64) -> u64 {
            var i: u64 = 0u64
            while i < n {
                if i == 5u64 { return 42u64 }
                i = i + 1u64
            }
            99u64
        }

        struct Counter { limit: u64 }
        impl Counter {
            fn find_first(self: Self, target: u64) -> u64 {
                var i: u64 = 0u64
                while i < self.limit {
                    if i == target { return i + 100u64 }
                    i = i + 1u64
                }
                999u64
            }
        }

        fn main() -> u64 {
            val a: u64 = early(10u64)         # 42
            val c = Counter { limit: 20u64 }
            val b: u64 = c.find_first(7u64)   # 107
            a + b                              # 149
        }
    "#;
    assert_consistent(src, "return_inside_while_propagates");
}

#[test]
fn dict_typed_slot_survives_geometric_growth() {
    // DICT-TYPED-SLOT-REALLOC pin (`6f60fb0` already added the
    // `typed_slots` migration in `interpreter/src/heap.rs::realloc`,
    // but the dict.t Phase 2 work flagged it as still-suspect
    // because the workaround had only ever exercised the first
    // `realloc(null, ...)` = `alloc()` path). This test forces
    // the dict past every geometric grow boundary (initial cap
    // 4, then 8, then 16, then 32) by inserting 33 typed
    // values (Object::Int64 keys + Object::UInt64 vals) and
    // reading every one back. If the migration were missing,
    // the post-growth `__builtin_ptr_read` would fall through
    // to the byte-buffer u64 path and the keys (Int64) would
    // come back as UInt64 — equality on the original signed
    // values would fail and `get_or` would return the default,
    // producing exit ≠ 42.
    //
    // Interpreter + JIT (silent fallback) only — AOT can't
    // currently lower `Dict::new()` (#159 / DICT-AOT-NEW).
    let src = r#"
        fn main() -> u64 {
            var d: Dict<i64, u64> = Dict::new()
            var i: i64 = 0i64
            while i < 33i64 {
                d.insert(i, (i as u64) * 10u64)
                i = i + 1i64
            }
            # Verify all 33 keys read back through the post-grow buffer.
            var j: i64 = 0i64
            var bad: bool = false
            while j < 33i64 {
                val expected: u64 = (j as u64) * 10u64
                val got: u64 = d.get_or(j, 99999u64)
                if got != expected { bad = true }
                j = j + 1i64
            }
            if bad { 1u64 } else { 42u64 }
        }
    "#;
    let interp = interpreter_value(src);
    assert_eq!(interp, 42, "interpreter expected 42 (typed_slots migrated through grow), got {interp}");
    let jit = jit_exit_code(src, "dict_typed_slot_growth_jit", true);
    assert_eq!(jit as u64, 42, "JIT expected 42, got {jit}");
}

#[test]
fn dict_user_space_round_trip() {
    // Originally Phase 2 of the user-space dict effort
    // (`core/std/dict.t`): exercises insert / get_or / overwrite
    // / contains_key / remove on the auto-loaded `Dict<i64, u64>`.
    // Promoted to a 3-way `assert_consistent` after `&mut self`
    // Phase 1c migrated `dict.t::insert` and `dict.t::remove` to
    // `&mut self`, which closes DICT-AOT-NEW Phase D — the
    // mutating methods now propagate `self.field = ...` writes
    // back to the caller's `d` binding via the AOT
    // Self-out-parameter writeback.
    //
    // Coverage:
    //   - insert into empty dict (allocation)
    //   - insert beyond initial capacity (geometric growth via
    //     heap_realloc)
    //   - update an existing key (overwrite branch)
    //   - get_or hit / miss
    //   - contains_key true / false
    //   - remove (swap-remove)
    //
    // Exit code 42 means every step matched the expected value.
    // Any digit 1..6 names the step that failed first.
    let src = r#"
        fn main() -> u64 {
            var d: Dict<i64, u64> = Dict::new()
            d.insert(1i64, 10u64)
            d.insert(2i64, 20u64)
            d.insert(3i64, 30u64)
            d.insert(4i64, 40u64)
            d.insert(5i64, 50u64)
            d.insert(2i64, 222u64)
            val a: u64 = d.get_or(1i64, 0u64)
            val b: u64 = d.get_or(2i64, 0u64)
            val c: u64 = d.get_or(5i64, 0u64)
            val miss: u64 = d.get_or(99i64, 7u64)
            val has: bool = d.contains_key(3i64)
            val no: bool = d.contains_key(99i64)
            val removed: bool = d.remove(3i64)
            val after_remove: bool = d.contains_key(3i64)
            if a != 10u64 { 1u64 }
            elif b != 222u64 { 2u64 }
            elif c != 50u64 { 3u64 }
            elif miss != 7u64 { 4u64 }
            elif has { if no { 5u64 } else { if removed { if after_remove { 6u64 } else { 42u64 } } else { 7u64 } } }
            else { 8u64 }
        }
    "#;
    assert_consistent(src, "dict_user_space_round_trip");
}

#[test]
fn dict_get_with_user_option_shadow() {
    // DICT-CROSS-MODULE-OPTION regression test:
    //
    // `core/std/dict.t::get(key) -> Option<V>` used to break when
    // user code declared its own `struct Option<T>`. The auto-load
    // integration silently dropped the stdlib `enum Option<T>` (the
    // user's same-named decl took precedence) but dict.t's body
    // still referenced `Option`, which now resolved to the user's
    // struct shape — `Option::Some(v)` failed with
    // "Associated function 'Some' not found for struct 'Option'".
    //
    // Fix: stdlib type names that the user shadows are re-interned
    // under `__std_<name>` during integration so dict.t's
    // `-> Option<V>` and `Option::Some(v)` keep resolving to the
    // stdlib enum (now `__std_Option`). User bare references to
    // `Option` still bind to the user's struct.
    //
    // This test lands a user `struct Option<T>` alongside a
    // `Dict<i64, u64>` and expects:
    //   1. The user's `Option` struct binding (`o.value`) keeps
    //      working — bare `Option` references resolve to the
    //      user's decl.
    //   2. `d.get(1)` returns the stdlib `Option<V>` (now
    //      registered as `__std_Option<V>` thanks to the alias),
    //      and inherent-method dispatch through method-call
    //      syntax (`r.is_some()`) reaches the stdlib impl on the
    //      aliased type. Users don't need to know the
    //      `__std_<name>` form to interop.
    //
    // Falls back to interpreter + JIT (silent fallback) — AOT
    // can't compile `Dict::new()` yet (#159 / DICT-AOT-NEW).
    let src = r#"
        struct Option<T> {
            value: T,
            is_some: bool
        }

        fn main() -> u64 {
            val o: Option<u64> = Option { value: 7u64, is_some: true }
            var d: Dict<i64, u64> = Dict::new()
            d.insert(1i64, 100u64)
            val r = d.get(1i64)
            val ok: bool = r.is_some()
            if ok { o.value } else { 0u64 }
        }
    "#;
    let interp = interpreter_value(src);
    assert_eq!(
        interp, 7,
        "interpreter expected 7 (user Option.value, gated on stdlib Option::is_some hit), got {interp}"
    );
    let jit = jit_exit_code(src, "dict_get_with_user_option_shadow_jit", true);
    assert_eq!(jit as u64, 7, "JIT expected 7, got {jit}");
}

#[test]
fn enum_str_payload_round_trip() {
    // Result<u64, str> exercises the new str-payload enum support
    // in the AOT compiler. Previously rejected with "unsupported
    // payload type str"; now lowers via the same scalar machinery
    // strings already use (Type::Str = i64-sized opaque pointer
    // into .rodata). interpreter / JIT (silent fallback) / AOT
    // must all agree on exit 99 (the Err arm fires).
    let src = r#"
        fn main() -> u64 {
            val r: Result<u64, str> = Result::Err("boom")
            match r {
                Result::Ok(v) => v,
                Result::Err(_) => 99u64,
            }
        }
    "#;
    assert_consistent(src, "enum_str_payload_round_trip");
}

// Closures Phase 5a — non-capturing closure literal lifted to a
// synthesized top-level function. The `val name = fn(...)` form
// gets a fresh FuncId; subsequent `name(args)` direct-call sites
// resolve through `closure_bindings` and emit a regular `Call`.
// JIT and interpreter handle the same source through their own
// closure paths (Phase 4 silent fallback for JIT, Phase 3
// `Object::Closure` for interpreter); 3-way agreement pins the
// shared semantics.
#[test]
fn closure_phase5_non_capturing_direct_call_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val add_two = fn(x: i64) -> i64 { x + 2i64 }
            add_two(40i64)
        }
    "#;
    assert_consistent(src, "closure_phase5_non_capturing_direct_call");
}

#[test]
fn closure_phase5_multi_param_direct_call_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val sum3 = fn(a: i64, b: i64, c: i64) -> i64 { a + b + c }
            sum3(10i64, 20i64, 12i64)
        }
    "#;
    assert_consistent(src, "closure_phase5_multi_param_direct_call");
}

#[test]
fn closure_phase5_zero_arg_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val k = fn() -> u64 { 42u64 }
            k()
        }
    "#;
    assert_consistent(src, "closure_phase5_zero_arg");
}

#[test]
fn closure_phase5_call_then_bind_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val mul = fn(x: i64, y: i64) -> i64 { x * y }
            val r = mul(6i64, 7i64)
            r
        }
    "#;
    assert_consistent(src, "closure_phase5_call_then_bind");
}

// Closures Phase 5b — HOF + closure-as-argument. The fn-typed
// parameter `f: (i64) -> i64` lowers to a `Type::U64` slot bound
// as `Binding::FunctionPtr`; a body-level `f(x)` dispatches via
// `InstKind::CallIndirect` against the recorded signature. The
// caller passes the closure either as a binding-name identifier
// (`apply(add_two, x)`) — emits `FuncAddr` to surface the lifted
// FuncId's runtime address — or as an inline literal
// (`apply(fn(x) -> ..., x)`) which lifts on-the-fly through
// `lift_closure_inline`.
#[test]
fn closure_phase5b_hof_with_closure_binding_round_trip() {
    let src = r#"
        fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }

        fn main() -> i64 {
            val add_two = fn(x: i64) -> i64 { x + 2i64 }
            apply(add_two, 40i64)
        }
    "#;
    assert_consistent(src, "closure_phase5b_hof_with_closure_binding");
}

#[test]
fn closure_phase5b_hof_with_inline_closure_literal_round_trip() {
    let src = r#"
        fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }

        fn main() -> i64 {
            apply(fn(x: i64) -> i64 { x * 2i64 }, 21i64)
        }
    "#;
    assert_consistent(src, "closure_phase5b_hof_with_inline_closure_literal");
}

#[test]
fn closure_phase5b_hof_called_twice_round_trip() {
    // Confirms the fn-pointer parameter is callable any number
    // of times in the body — `CallIndirect` re-imports the
    // signature each time but the cranelift `SigRef` cache makes
    // this O(1).
    let src = r#"
        fn apply_twice(f: (i64) -> i64, x: i64) -> i64 {
            f(f(x))
        }

        fn main() -> i64 {
            val plus_three = fn(x: i64) -> i64 { x + 3i64 }
            apply_twice(plus_three, 36i64)
        }
    "#;
    assert_consistent(src, "closure_phase5b_hof_called_twice");
}

#[test]
fn closure_phase5b_hof_passes_closure_through_round_trip() {
    // Forwards a fn-typed parameter from one HOF to another —
    // `lower_expr::Expr::Identifier` for a `Binding::FunctionPtr`
    // emits LoadLocal to surface the U64 address, which the
    // outer call's arg evaluator passes through as a value.
    let src = r#"
        fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }

        fn run_via(g: (i64) -> i64, x: i64) -> i64 {
            apply(g, x)
        }

        fn main() -> i64 {
            val plus_one = fn(x: i64) -> i64 { x + 1i64 }
            run_via(plus_one, 41i64)
        }
    "#;
    assert_consistent(src, "closure_phase5b_hof_passes_closure_through");
}

// Closures Phase 6 — capturing closure direct call. The
// `val name = fn(...) { ... + cap }` form lifts to a synthesized
// fn whose IR signature carries an implicit `env: U64` first
// parameter. `MakeClosure` allocates an env on the heap
// (layout: `[fn_ptr][cap0][cap1]...`) and the binding's
// env_ptr is prepended to the user-visible args at every call
// site. Captures are loaded inside the body via
// `PtrRead(env, +8 + i*8)`.
#[test]
fn closure_phase6_single_capture_direct_call_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val n: i64 = 10i64
            val add_n = fn(x: i64) -> i64 { x + n }
            add_n(32i64)
        }
    "#;
    assert_consistent(src, "closure_phase6_single_capture_direct_call");
}

#[test]
fn closure_phase6_multi_capture_direct_call_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val a: i64 = 5i64
            val b: i64 = 7i64
            val sum_offset = fn(x: i64) -> i64 { x + a + b }
            sum_offset(30i64)
        }
    "#;
    assert_consistent(src, "closure_phase6_multi_capture_direct_call");
}

#[test]
fn closure_phase6_capture_snapshot_independent_of_post_capture_mutation() {
    // Primitives are captured by value at lift time — `MakeClosure`
    // stores the current binding's loaded value into the env, so
    // a subsequent reassignment of the outer `n` doesn't affect
    // the closure's behaviour. Mirrors the interpreter Phase 3
    // semantics.
    let src = r#"
        fn main() -> i64 {
            var n: i64 = 10i64
            val add_n = fn(x: i64) -> i64 { x + n }
            n = 100i64
            add_n(32i64)
        }
    "#;
    assert_consistent(src, "closure_phase6_capture_snapshot");
}

#[test]
fn closure_phase6_capture_called_twice_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val k: i64 = 1i64
            val plus_k = fn(x: i64) -> i64 { x + k }
            val r1 = plus_k(20i64)
            val r2 = plus_k(21i64)
            r1 + r2
        }
    "#;
    assert_consistent(src, "closure_phase6_capture_called_twice");
}

// Closures Phase 6b — unified env-based ABI. Every closure value
// is an env_ptr (Type::U64) pointing at `[fn_ptr][captures...]`,
// even non-capturing closures (env layout = `[fn_ptr]`, 8 bytes).
// CallIndirect loads fn_ptr from env+0 and prepends env to the
// user-visible args. This unification lets capturing closures
// flow through HOF parameters: the same indirect-call machinery
// handles both non-capturing and capturing call sites.
#[test]
fn closure_phase6b_capturing_closure_via_hof_round_trip() {
    let src = r#"
        fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }

        fn main() -> i64 {
            val n: i64 = 10i64
            val add_n = fn(x: i64) -> i64 { x + n }
            apply(add_n, 32i64)
        }
    "#;
    assert_consistent(src, "closure_phase6b_capturing_via_hof");
}

#[test]
fn closure_phase6b_capturing_inline_literal_via_hof_round_trip() {
    // Inline closure literal that captures from the outer scope
    // and is passed straight to a HOF — exercises both
    // `lift_closure_inline` (env build) and CallIndirect dispatch
    // in a single expression position.
    let src = r#"
        fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }

        fn main() -> i64 {
            val n: i64 = 10i64
            apply(fn(x: i64) -> i64 { x + n }, 32i64)
        }
    "#;
    assert_consistent(src, "closure_phase6b_capturing_inline_via_hof");
}

#[test]
fn closure_phase6b_capturing_called_via_two_hops_round_trip() {
    // HOF→HOF forward of a capturing closure. The inner HOF
    // (`apply`) sees `g` as a fn-typed parameter (Binding::
    // FunctionPtr); reading `g` in expression position loads
    // the env_ptr U64; passing it to `apply(g, x)` goes through
    // the same CallIndirect path again.
    let src = r#"
        fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }

        fn run_via(g: (i64) -> i64, x: i64) -> i64 {
            apply(g, x)
        }

        fn main() -> i64 {
            val k: i64 = 7i64
            val plus_k = fn(x: i64) -> i64 { x + k }
            run_via(plus_k, 35i64)
        }
    "#;
    assert_consistent(src, "closure_phase6b_capturing_via_two_hops");
}

// Closures Phase 6c — narrow int captures (u8/u16/u32/i8/i16/i32).
// Each capture occupies an 8-byte slot in the env tuple for
// pointer-aligned addressing, but uses a width-aware load at
// body entry (driven by `PtrRead.elem_ty`). MakeClosure's
// `store` is width-polymorphic on the value type.
#[test]
fn closure_phase6c_narrow_int_capture_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val n: i32 = 10i32
            val add_n = fn(x: i32) -> i32 { x + n }
            val r = add_n(32i32)
            r as i64
        }
    "#;
    assert_consistent(src, "closure_phase6c_narrow_int_capture");
}

#[test]
fn closure_phase6c_u8_capture_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val n: u8 = 10u8
            val add_n = fn(x: u8) -> u8 { x + n }
            val r = add_n(32u8)
            r as i64
        }
    "#;
    assert_consistent(src, "closure_phase6c_u8_capture");
}

// Closures Phase 6d — closure return value. A function whose
// return type is `(T1, T2) -> R` lifts the inline closure body
// to a top-level fn (Phase 5b's lift_closure_inline path) and
// surfaces the env_ptr as the return value (Type::U64). The
// caller's `val name = make_adder(...)` binds `name` as a
// `Binding::FunctionPtr` so a subsequent `name(args)` dispatches
// through the env-aware CallIndirect.
#[test]
fn closure_phase6d_closure_return_value_round_trip() {
    let src = r#"
        fn make_adder(n: i64) -> (i64) -> i64 {
            fn(x: i64) -> i64 { x + n }
        }

        fn main() -> i64 {
            val add5 = make_adder(5i64)
            add5(37i64)
        }
    "#;
    assert_consistent(src, "closure_phase6d_closure_return_value");
}

#[test]
fn closure_phase6d_two_returned_closures_with_independent_captures_round_trip() {
    // Each `make_adder(n)` call produces an independent env
    // tuple — the captures must not alias across the two
    // returned closures.
    let src = r#"
        fn make_adder(n: i64) -> (i64) -> i64 {
            fn(x: i64) -> i64 { x + n }
        }

        fn main() -> i64 {
            val add5 = make_adder(5i64)
            val add10 = make_adder(10i64)
            val r1 = add5(37i64)
            val r2 = add10(32i64)
            r1 + r2 - 42i64
        }
    "#;
    assert_consistent(src, "closure_phase6d_two_returned_closures");
}

// Closures Phase 8 — closure stored in a struct field, called
// via `obj.field(args)`. Type-checker resolves the field-call
// because no method named `field` exists on the struct;
// runtime dispatches through the same env-based CallIndirect
// machinery the HOF parameter path uses (Phase 6b ABI).
#[test]
fn closure_phase8_struct_field_holds_closure_round_trip() {
    let src = r#"
        struct Calculator {
            op: fn (i64, i64) -> i64,
        }

        fn main() -> i64 {
            val c = Calculator {
                op: fn(a: i64, b: i64) -> i64 { a + b },
            }
            c.op(20i64, 22i64)
        }
    "#;
    assert_consistent(src, "closure_phase8_struct_field_holds_closure");
}

#[test]
fn closure_phase8_struct_field_capturing_closure_round_trip() {
    // The closure stored in `inc` captures `n` from the
    // outer scope — exercises the full Phase 6b env-aware
    // CallIndirect through a field-call dispatch.
    let src = r#"
        struct Counter {
            inc: fn (i64) -> i64,
        }

        fn main() -> i64 {
            val n: i64 = 10i64
            val c = Counter {
                inc: fn(x: i64) -> i64 { x + n },
            }
            c.inc(32i64)
        }
    "#;
    assert_consistent(src, "closure_phase8_struct_field_capturing_closure");
}

#[test]
fn closure_phase8_struct_with_two_closure_fields_round_trip() {
    // Two closure fields stored independently; each is called
    // through its own field-call dispatch.
    let src = r#"
        struct Pair {
            add: fn (i64, i64) -> i64,
            sub: fn (i64, i64) -> i64,
        }

        fn main() -> i64 {
            val p = Pair {
                add: fn(a: i64, b: i64) -> i64 { a + b },
                sub: fn(a: i64, b: i64) -> i64 { a - b },
            }
            p.add(20i64, 30i64) + p.sub(0i64, 8i64)
        }
    "#;
    assert_consistent(src, "closure_phase8_struct_with_two_closure_fields");
}

#[test]
fn closure_phase6c_all_narrow_widths_captured_round_trip() {
    // Captures all six narrow widths in a single closure to
    // confirm the per-width store/load pairings line up
    // independently — each capture lives in its own 8-byte
    // env slot regardless of its width.
    let src = r#"
        fn main() -> i64 {
            val a: i8 = 1i8
            val b: i16 = 2i16
            val c: i32 = 3i32
            val d: u8 = 4u8
            val e: u16 = 5u16
            val f: u32 = 6u32
            val sum = fn(x: i64) -> i64 {
                x + (a as i64) + (b as i64) + (c as i64)
                  + (d as i64) + (e as i64) + (f as i64)
            }
            sum(21i64)
        }
    "#;
    assert_consistent(src, "closure_phase6c_all_narrow_widths");
}

// CLOSURE-CAPTURE E0/E1 — writes to a captured binding.
//
// The closure tests above cover *reads* only (phases 5a/5b/6b/6c),
// which is how a counter closure came to answer `1, 1, 0` on the IR
// VM, both JITs and AOT while the tree-walker rejected it at run time
// with a reason that was not true. The rejection now lives in the
// shared frontend, so the disagreement cannot come back one engine at
// a time.
#[test]
fn writing_to_a_captured_binding_is_rejected_before_any_engine_runs() {
    let src = r#"
fn main() -> u64 {
    var count: u64 = 0u64
    val bump = fn() -> u64 { count = count + 1u64  count }
    bump()
    bump()
    count
}
"#;
    let errors = type_check_errors(src);
    assert!(
        errors.iter().any(|e| e.contains("count") && e.contains("closure")),
        "expected the write to `count` to be rejected as a capture, got: {errors:?}"
    );
}

// Reading a capture and writing to a binding the closure owns are
// both still legal, and every engine agrees on the answer. Pinned
// together with the rejection above so a future mutable-capture
// phase (E3) cannot widen the rule past what it means to widen.
#[test]
fn closure_local_write_over_a_captured_read_round_trip() {
    let src = r#"
fn main() -> u64 {
    val step: u64 = 2u64
    val f = fn(x: u64) -> u64 {
        var acc: u64 = x
        acc = acc + step
        acc
    }
    f(1u64) + f(10u64)
}
"#;
    assert_consistent(src, "closure_local_write_over_captured_read");
}

// CLOSURE-CAPTURE E2 — writing through a capture. A captured compound
// kept its cell, so `p.x = ...` reached the outer binding on the three
// interpreter engines while the two compiled ones reported `p` as an
// undefined identifier. The shape of the value decided the meaning;
// the rule is now the same for both shapes.
#[test]
fn writing_through_a_captured_compound_is_rejected_before_any_engine_runs() {
    let src = r#"
struct P { x: i64 }
fn main() -> i64 {
    var p = P { x: 1i64 }
    val f = fn() -> i64 { p.x = p.x + 1i64  p.x }
    f()
}
"#;
    let errors = type_check_errors(src);
    assert!(
        errors.iter().any(|e| e.contains("p.x") && e.contains("captured")),
        "expected the write through `p` to be rejected as a capture, got: {errors:?}"
    );
}

// Reading *through* a capture is not pinned across engines here: the
// compiled lanes cannot capture a compound at all yet, and report `p`
// as an undefined identifier (CLOSURE-CAPTURE E5). The interpreter
// engines do run it. Pin it when E5 lands rather than recording the
// gap as if it were the design.
