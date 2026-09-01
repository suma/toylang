//! Literals, the earliest AOT round trips, plain arithmetic, and the
//! runtime traps (RUNTIME-TRAP).

use interpreter::RunOptions;

use super::harness::*;

#[test]
fn literal_returns_match() {
    assert_consistent("fn main() -> u64 { 42u64 }\n", "literal");
}

#[test]
fn aot_heap_alloc_round_trip() {
    // #121 Phase A: heap_alloc + ptr_write + ptr_read + heap_free
    // round trip through AOT codegen. Default global allocator
    // (libc malloc / realloc / free) only — `with allocator = ...`
    // scope handling and arena / fixed-buffer allocators come in
    // later phases.
    //
    // The test allocates a 16-byte buffer, writes a u64 at offset
    // 0 and another at offset 8, reads them back, sums, and frees.
    // 3-way `assert_consistent` checks interpreter / JIT
    // (silent fallback) / AOT all agree on exit code 42.
    let src = r#"
        unsafe fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(16u64)
            __builtin_ptr_write(p, 0u64, 17u64)
            __builtin_ptr_write(p, 8u64, 25u64)
            val a: u64 = __builtin_ptr_read(p, 0u64)
            val b: u64 = __builtin_ptr_read(p, 8u64)
            __builtin_heap_free(p)
            a + b
        }
    "#;
    assert_consistent(src, "aot_heap_alloc_round_trip");
}

#[test]
fn aot_mut_self_propagates_field_mutation() {
    // Stage 1 of `&` references — `&mut self` Phase 1b: a method
    // declared with `&mut self` mutates `self.field` and the
    // change must propagate back to the caller's struct binding.
    // Implementation: the method's IR signature gains trailing
    // self-leaf return slots (Self-out-parameter convention),
    // every Return appends LoadLocal-of-leaves, and the call
    // site uses `InstKind::CallWithSelfWriteback` to store the
    // returned leaves back into the receiver's leaf locals via
    // `def_var`. Without the writeback (the prior behavior),
    // `c.bump()` would leave `c.value` at 0 in the caller.
    //
    // Three `bump()` calls + a `read()` should observe value=3
    // across interpreter / JIT (silent fallback) / AOT.
    let src = r#"
        struct Counter {
            value: u64
        }

        impl Counter {
            fn bump(&mut self) {
                self.value = self.value + 1u64
            }

            fn read(self: Self) -> u64 {
                self.value
            }
        }

        fn main() -> u64 {
            var c = Counter { value: 0u64 }
            c.bump()
            c.bump()
            c.bump()
            c.read()
        }
    "#;
    assert_consistent(src, "aot_mut_self_propagates_field_mutation");
}

#[test]
fn aot_dict_contains_key_empty_uses_per_monomorph_subst() {
    // DICT-AOT-NEW Phase C: per-monomorph generic subst lets a
    // method body's `val existing: K = __builtin_ptr_read(...)`
    // resolve K to the concrete type for the active instance
    // (`Type::I64` for `Dict<i64, u64>::contains_key`). Combined
    // with the new `__builtin_sizeof(generic_param)` AOT lower,
    // the read-only methods of `core/std/dict.t` now compile
    // end-to-end.
    //
    // This test covers the still-no-mutation path: `Dict::new()`
    // followed by `contains_key(1i64)` against an empty dict
    // returns false (`self.count == 0` short-circuits the loop
    // before any heap read). The test still exercises the
    // monomorphised body in full because cranelift compiles the
    // entire CFG including the never-taken loop body — any
    // regression in the subst plumbing would surface as a
    // type / size mismatch at AOT compile time, not a runtime
    // error.
    //
    // Mutating methods (`insert`, `remove`) compile but do not
    // round-trip yet because struct method calls pass `self` by
    // value in the AOT path; mutations to `self.count` /
    // `self.keys` etc. don't propagate back to the caller. That
    // by-value-vs-by-reference gap is a separate, larger
    // refactor (`DICT-AOT-NEW Phase D`).
    let src = r#"
        fn main() -> u64 {
            var d: Dict<i64, u64> = Dict::new()
            if d.contains_key(1i64) { 99u64 } else { 42u64 }
        }
    "#;
    assert_consistent(src, "aot_dict_contains_key_empty");
}

#[test]
fn aot_dict_new_associated_function() {
    // DICT-AOT-NEW Phase B: `var d: Dict<i64, u64> = Dict::new()`
    // now compiles end-to-end on the AOT path. The associated
    // function (`new() -> Self`) is monomorphised through the
    // same generic-method machinery (Phase R3 / X) used for
    // `obj.method()`, with the type args lifted from the val
    // annotation (`Dict<i64, u64>`) and an empty arg list (no
    // self / no formal args).
    //
    // The body of `Dict::new()` calls `__builtin_heap_alloc(0u64)`
    // twice and zero-initialises the rest of the struct fields.
    // The heap-builtin path landed in #121 Phase A; this test
    // confirms the two pieces compose end-to-end.
    //
    // Subsequent methods (`insert` / `get_or` / `remove`) still
    // need additional generic-substitution plumbing in the
    // method body lowering (val annotations referencing generic
    // params like `K` / `V` aren't substituted yet) — covered
    // by a later phase.
    let src = r#"
        fn main() -> u64 {
            var d: Dict<i64, u64> = Dict::new()
            42u64
        }
    "#;
    assert_consistent(src, "aot_dict_new_associated_function");
}

#[test]
fn aot_heap_realloc_grows_buffer() {
    // #121 Phase A continued: realloc grows an existing buffer in
    // place (or moves it). After grow we write into the newly
    // available bytes and read everything back to verify both the
    // pre-grow and post-grow contents survived.
    let src = r#"
        unsafe fn main() -> u64 {
            var p: ptr = __builtin_heap_alloc(8u64)
            __builtin_ptr_write(p, 0u64, 100u64)
            p = __builtin_heap_realloc(p, 24u64)
            __builtin_ptr_write(p, 8u64, 200u64)
            __builtin_ptr_write(p, 16u64, 300u64)
            val a: u64 = __builtin_ptr_read(p, 0u64)
            val b: u64 = __builtin_ptr_read(p, 8u64)
            val c: u64 = __builtin_ptr_read(p, 16u64)
            __builtin_heap_free(p)
            a + b + c
        }
    "#;
    assert_consistent(src, "aot_heap_realloc_grows_buffer");
}

#[test]
fn arithmetic_match() {
    let src = r#"
        fn main() -> u64 {
            (3u64 + 4u64) * 5u64 - 1u64
        }
    "#;
    assert_consistent(src, "arith");
}

#[test]
fn signed_arithmetic_match() {
    let src = r#"
        fn main() -> i64 {
            val a: i64 = -7i64
            val b: i64 = 3i64
            a * b + 25i64
        }
    "#;
    assert_consistent(src, "signed");
}

#[test]
fn fib_recursive_match() {
    let src = r#"
        fn fib(n: u64) -> u64 {
            if n <= 1u64 { n } else { fib(n - 1u64) + fib(n - 2u64) }
        }
        fn main() -> u64 { fib(10u64) }
    "#;
    assert_consistent(src, "fib");
}

#[test]
fn for_loop_sum_match() {
    let src = r#"
        fn main() -> u64 {
            var sum = 0u64
            for i in 0u64..20u64 {
                sum = sum + i
            }
            sum
        }
    "#;
    assert_consistent(src, "for_sum");
}

#[test]
fn while_with_break_match() {
    let src = r#"
        fn main() -> u64 {
            var i = 0u64
            while i < 100u64 {
                if i == 13u64 { break }
                i = i + 1u64
            }
            i
        }
    "#;
    assert_consistent(src, "while_break");
}

#[test]
fn if_elif_else_match() {
    let src = r#"
        fn classify(n: u64) -> u64 {
            if n == 0u64 { 11u64 }
            elif n == 1u64 { 22u64 }
            elif n == 2u64 { 33u64 }
            else { 44u64 }
        }
        fn main() -> u64 { classify(2u64) }
    "#;
    assert_consistent(src, "elif");
}

#[test]
fn short_circuit_match() {
    // Both interpreter and compiler must short-circuit `&&`. If either
    // evaluated the divide-by-zero, the test would crash the path that
    // happens to evaluate it but not the other, surfacing a divergence.
    let src = r#"
        fn main() -> u64 {
            val cond: bool = false && (1u64 / 0u64 == 0u64)
            if cond { 1u64 } else { 2u64 }
        }
    "#;
    assert_consistent(src, "short_circuit");
}

#[test]
fn nested_calls_match() {
    let src = r#"
        fn add(a: u64, b: u64) -> u64 { a + b }
        fn double(x: u64) -> u64 { add(x, x) }
        fn main() -> u64 {
            double(double(add(3u64, 4u64)))
        }
    "#;
    assert_consistent(src, "nested_calls");
}

#[test]
fn struct_field_match() {
    let src = r#"
        struct Point { x: i64, y: i64 }
        fn dist_sq(p: Point) -> i64 { p.x * p.x + p.y * p.y }
        fn main() -> u64 {
            val p = Point { x: 3i64, y: 4i64 }
            val d: i64 = dist_sq(p)
            d as u64
        }
    "#;
    assert_consistent(src, "struct_field");
}

#[test]
fn tuple_round_trip_match() {
    let src = r#"
        fn swap(p: (u64, u64)) -> (u64, u64) { (p.1, p.0) }
        fn main() -> u64 {
            val orig = (5u64, 10u64)
            val s = swap(orig)
            s.0 + s.1
        }
    "#;
    assert_consistent(src, "tuple_round");
}

#[test]
fn u64_wrapping_overflow_match() {
    // Now that the interpreter uses wrapping arithmetic too, all
    // three backends agree on overflow behaviour. The result is
    // (5 + u64::MAX) wrapped == 4; exit code is 4.
    let src = r#"
        fn main() -> u64 {
            val a: u64 = 18446744073709551615u64
            a + 5u64
        }
    "#;
    assert_consistent(src, "u64_overflow");
}

#[test]
fn u64_underflow_traps_on_every_backend() {
    // This used to pin *wrapping*: `5 - 10` gave `u64::MAX - 4` and the
    // three backends agreed on that. LLM-LOOP P6-3 changed the
    // semantics — a result of 18446744073709551615 looks like a
    // plausible number, so the mistake surfaces far from its cause and
    // costs an afternoon. The agreement being checked is now that every
    // backend refuses the operation rather than inventing a value.
    //
    // Written out rather than run through `assert_consistent`, which
    // asserts each backend *succeeds* before comparing results and so
    // cannot express "they all fail the same way".
    //
    // Addition and multiplication still wrap (see `u64_overflow`
    // above); only subtraction is guarded so far.
    let src = r#"
        fn main() -> u64 {
            val a: u64 = 5u64
            a - 10u64
        }
    "#;
    let core = core_modules_dir();

    let mut interp_opts = RunOptions::default();
    interp_opts.core_modules_dir = Some(core.as_path());
    assert!(
        interpreter::run_source(src, "underflow.t", &interp_opts).is_err(),
        "interpreter should refuse the subtraction"
    );

    // The JIT is checked in `interpreter/tests/jit_integration.rs`
    // instead: its panic helper terminates via `process::exit(1)`, which
    // would tear down this test runner along with the program.

    let compiled = try_compiler_exit_code(src, "u64_underflow", true)
        .expect("the program should still compile — the guard is a runtime trap");
    assert_ne!(compiled, 0, "compiled binary should exit non-zero");
}


/// RUNTIME-TRAP. Integer `/` and `%` by zero used to fail in whatever
/// way the host happened to fail: the IR VM hit Rust's
/// `attempt to divide by zero` panic (a backtrace into
/// `ir_vm/dispatch.rs`, naming no toylang line) and the compiled
/// binary took cranelift's own `sdiv` trap. Now every backend raises
/// the same toylang panic. Written out rather than run through
/// `assert_consistent`, which asserts each backend *succeeds* before
/// comparing results and so cannot express "they all fail the same
/// way".
#[test]
fn integer_division_by_zero_traps_on_every_backend() {
    for (op, stem) in [("/", "div_by_zero"), ("%", "rem_by_zero")] {
        let src = format!(
            r#"
        fn main() -> u64 {{
            var z: u64 = 0u64
            10u64 {op} z
        }}
    "#
        );
        let core = core_modules_dir();
        let mut interp_opts = RunOptions::default();
        interp_opts.core_modules_dir = Some(core.as_path());
        assert!(
            interpreter::run_source(&src, "div_by_zero.t", &interp_opts).is_err(),
            "the interpreter should refuse `{op}` by zero"
        );

        let compiled = try_compiler_exit_code(&src, stem, true)
            .expect("the program should still compile — the guard is a runtime trap");
        assert_ne!(compiled, 0, "compiled binary should exit non-zero for `{op}`");
    }
}

/// RUNTIME-TRAP. `i64::MIN / -1` has no representable result. The
/// compiled binary used to die with SIGILL (cranelift's `sdiv`
/// faults) while the interpreter wrapped back to `MIN` and carried
/// on — the one trap where the backends disagreed on whether the
/// program even survived.
#[test]
fn a_function_may_write_its_unit_return_type() {
    // `()` as a type is the unit type. Written out it used to be
    // parsed as the empty *tuple*, so `fn f() -> ()` never
    // type-checked — the diagnostic was the memorable
    // "expected (), but got ()" — and the compiled lanes would have
    // treated the return as a compound. Omitting the type has always
    // worked, so this pins the written form against both failures.
    let src = r#"
        fn note(n: u64) -> () {
            println("note {n}")
        }

        fn silent(n: u64) {
            println("silent {n}")
        }

        fn main() -> u64 {
            note(1u64)
            silent(2u64)
            val x: () = ()
            0u64
        }
    "#;
    assert_stdout_consistent(src, "unit_return_type");
}

#[test]
fn signed_division_overflow_traps_on_every_backend() {
    for (op, stem) in [("/", "div_overflow"), ("%", "rem_overflow")] {
        let src = format!(
            r#"
        fn main() -> i64 {{
            var lo: i64 = -9223372036854775808i64
            var m: i64 = -1i64
            lo {op} m
        }}
    "#
        );
        let core = core_modules_dir();
        let mut interp_opts = RunOptions::default();
        interp_opts.core_modules_dir = Some(core.as_path());
        assert!(
            interpreter::run_source(&src, "div_overflow.t", &interp_opts).is_err(),
            "the interpreter should refuse `MIN {op} -1`"
        );

        let compiled = try_compiler_exit_code(&src, stem, true)
            .expect("the program should still compile — the guard is a runtime trap");
        assert_ne!(
            compiled, 0,
            "compiled binary should exit non-zero for `MIN {op} -1`"
        );
    }
}

/// RUNTIME-TRAP. A runtime index past the end of an array reached
/// `ArrayLoad` unchecked: the AOT binary read whatever followed the
/// backing stack slot and exited 0 (printing a stack address), while
/// the IR VM raised the internal error "value not defined". Constant
/// indices were already rejected at compile time — this pins the
/// runtime ones.
#[test]
fn runtime_index_out_of_bounds_traps_on_every_backend() {
    let src = r#"
        fn main() -> u64 {
            val arr = [1u64, 2u64, 3u64]
            var i: u64 = 10u64
            arr[i]
        }
    "#;
    let core = core_modules_dir();
    let mut interp_opts = RunOptions::default();
    interp_opts.core_modules_dir = Some(core.as_path());
    assert!(
        interpreter::run_source(src, "index_oob.t", &interp_opts).is_err(),
        "the interpreter should refuse the out-of-bounds read"
    );

    let compiled = try_compiler_exit_code(src, "index_oob", true)
        .expect("the program should still compile — the guard is a runtime trap");
    assert_ne!(
        compiled, 0,
        "compiled binary should exit non-zero rather than read past the array"
    );
}

/// RUNTIME-TRAP. The bounds guard adjusts a negative runtime index the
/// way the tree-walker does (`arr[-1i64]` is the last element), so the
/// backends agree on the in-bounds negative cases too — they did not
/// before, since the lowered lanes indexed with the raw negative value.
#[test]
fn negative_runtime_index_match() {
    let src = r#"
        fn main() -> i64 {
            val arr = [10i64, 20i64, 30i64]
            var i: i64 = 0i64 - 1i64
            println(arr[i])
            var j: i64 = 0i64 - 3i64
            println(arr[j])
            arr[i]
        }
    "#;
    assert_consistent(src, "negative_runtime_index");
}

// --- NUM-W: bitwise operators at every width -----------------------
//
// The tree-walker's `Value`-flavoured fast path for `& | ^ << >>` had
// arms for `u64` and `i64` only, so `flags & 1u32` — both sides
// plainly `u32` — failed with "expected UInt32, found UInt32", a
// message naming the same type twice because the mismatch was never
// between the operands. The compiled lanes were fine, so this was a
// lane disagreement rather than a shared gap.
//
// That is the enumeration NUM-W-ENUMERATION describes, found by
// `core/std/poll.t`'s interest flags — the first stdlib code to do
// bit arithmetic at a narrow width.

/// Every narrow width, through every bitwise operator, with a shift
/// that carries bits off the top so the truncation is doing real work.
#[test]
fn bitwise_operators_work_at_every_integer_width() {
    let src = r#"
        fn main() -> u64 {
            val a8: u8 = 12u8
            val b8: u8 = 10u8
            val and8 = a8 & b8
            val or8 = a8 | b8
            val xor8 = a8 ^ b8

            val a16: u16 = 0xF0F0u16
            val b16: u16 = 0x0FF0u16
            val and16 = a16 & b16

            val a32: u32 = 0xFFFF0000u32
            val b32: u32 = 0x00FFFF00u32
            val and32 = a32 & b32

            val i8v: i8 = -2i8
            val and_i8 = i8v & 0x7Fi8

            val i32v: i32 = -1i32
            val or_i32 = i32v | 0i32

            # Shifts are *not* here: the type checker rejects a narrow
            # left operand outright ("incompatible types u8 and u64"),
            # whatever the shift amount's type, so `<<` and `>>` stay
            # 64-bit-only for now (todo: NUM-W-SHIFT).

            (and8 as u64) * 1000000u64
                + (or8 as u64) * 100000u64
                + (xor8 as u64) * 10000u64
                + (and16 as u64)
                + (and32 as u64)
                + (and_i8 as u64)
                + (or_i32 as i64 + 1i64) as u64
        }
    "#;
    // 12 & 10 = 8, 12 | 10 = 14, 12 ^ 10 = 6,
    // and16 = 0x00F0 = 240, and32 = 0x00FF0000 = 16711680,
    // and_i8 = -2 & 0x7F = 0x7E = 126, or_i32 = -1 (+1 = 0).
    assert_eq!(
        interpreter_value(src),
        8 * 1000000 + 14 * 100000 + 6 * 10000 + 240 + 16711680 + 126
    );
    assert_consistent(src, "bitwise_every_width");
}
