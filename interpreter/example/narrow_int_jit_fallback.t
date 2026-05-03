# NUM-W Phase 4 — narrow integer JIT fallback fixture.
#
# The JIT codegen doesn't yet recognise the u8 / u16 / u32 /
# i8 / i16 / i32 widths added by NUM-W: every type-decl outside
# the i64 / u64 / f64 / bool / ptr / allocator set returns
# `None` from `ScalarTy::from_type_decl`, so a function that
# uses any narrow type fails JIT eligibility and the interpreter
# takes over. This file exercises that path end-to-end so the
# fallback is regression-tested rather than relying on
# accidental coverage.
#
# Programs touching narrow ints must keep producing the right
# value with `INTERPRETER_JIT=1` set — same as without.
# Expected exit: 142.

fn main() -> u64 {
    val a: u8 = 200u8 + 50u8        # wraps to 250
    val b: u16 = a as u16 - 100u16  # 150
    val c: i32 = -1i32              # -1, two's complement
    val d: u32 = c as u32           # 4294967295
    val sized: u64 = __builtin_sizeof(a) + __builtin_sizeof(b) + __builtin_sizeof(c) + __builtin_sizeof(d)
    # sized = 1 + 2 + 4 + 4 = 11

    if a != 250u8 { 1u64 }
    elif b != 150u16 { 2u64 }
    elif d != 4294967295u32 { 3u64 }
    elif sized != 11u64 { 4u64 }
    else { 142u64 }
}
