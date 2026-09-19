// SIMD (SIMD.md Phase 2): the 128-bit vector types, lane-wise
// operators, and the `__simd_*` intrinsics across all three
// backends.
//
// The semantics SIMD.md fixes are what these tests exist to pin, and
// each is a place the four engines could plausibly disagree:
//
//   * `__simd_reduce_add` folds lane 0 through n **in order**. A
//     pairwise tree would give a different f64 sum in cranelift than
//     the tree-walker's loop gives.
//   * Integer lanes **wrap** and never trap, unlike the scalar `u64 -`
//     / `/` which panic (RUNTIME-TRAP).
//   * A comparison produces an all-ones / all-zeros **mask**, not a
//     `bool`, so `__simd_all(a == b)` is the whole-vector question.
//   * `__simd_load(p, i)` addresses by **element**: lane `k` lives at
//     byte offset `(i + k) * lane_bytes`.
//
// The interpreter JIT takes its usual silent fallback (its `ScalarTy`
// has no vector), so it is not one of the lanes here.

use super::harness::*;

/// `assert_consistent` only proves the lanes agree, so each test also
/// pins the tree-walker's answer: without that, a change that made
/// every backend return 0 would still pass.
fn assert_simd(src: &str, stem: &str, expected: u64) {
    if skip_e2e() {
        return;
    }
    assert_eq!(
        interpreter_value(src),
        expected,
        "tree-walker value for {stem}"
    );
    assert_consistent(src, stem);
}

#[test]
fn simd_lane_wise_arithmetic_agrees() {
    let src = r#"
        fn main() -> u64 {
            val a: f64x2 = __simd_splat(1.5f64)
            val b: f64x2 = __simd_splat(2.0f64)
            val c = a * b + a
            val s: f64 = __simd_reduce_add(c)
            if s == 9.0f64 { 42u64 } else { 0u64 }
        }
    "#;
    assert_simd(src, "simd_lane_wise_arithmetic_agrees", 42u64);
}

#[test]
fn simd_reduce_folds_lanes_in_order() {
    // The fold order is observable in f32: adding a large lane first
    // and two tiny ones after loses them, while the reverse order
    // keeps one. Both orders are pinned so a backend that reassociates
    // (or builds a pairwise tree) fails here rather than silently
    // producing a different sum than the tree-walker.
    let src = r#"
        fn main() -> u64 {
            val big: f32x4 = __simd_splat(0f32)
            val v0 = __simd_insert(big, 0u64, 16777216f32)
            val v1 = __simd_insert(v0, 1u64, 1f32)
            val v2 = __simd_insert(v1, 2u64, 1f32)
            val forward: f32 = __simd_reduce_add(v2)

            val r0 = __simd_insert(big, 0u64, 1f32)
            val r1 = __simd_insert(r0, 1u64, 1f32)
            val r2 = __simd_insert(r1, 2u64, 16777216f32)
            val backward: f32 = __simd_reduce_add(r2)

            # Left-to-right: 16777216 + 1 + 1 rounds back to 16777216.
            # Right-to-left would have summed the ones first and kept 2.
            if forward == 16777216f32 && backward == 16777218f32 {
                21u64
            } else {
                0u64
            }
        }
    "#;
    assert_simd(src, "simd_reduce_folds_lanes_in_order", 21u64);
}

#[test]
fn simd_integer_lanes_wrap_without_trapping() {
    // The scalar `i32` add wraps too, but the point here is that no
    // guard is emitted per lane: a vector add of `i32::MAX + 1` lands
    // on `i32::MIN` on every backend rather than panicking.
    let src = r#"
        fn main() -> u64 {
            val m: i32x4 = __simd_splat(2147483647i32)
            val one: i32x4 = __simd_splat(1i32)
            val wrapped = m + one
            val lane: i32 = __simd_extract(wrapped, 0u64)
            if lane == -2147483648i32 { 33u64 } else { 0u64 }
        }
    "#;
    assert_simd(src, "simd_integer_lanes_wrap_without_trapping", 33u64);
}

#[test]
fn simd_comparison_produces_a_mask() {
    let src = r#"
        fn main() -> u64 {
            val a: f64x2 = __simd_splat(1.0f64)
            val b: f64x2 = __simd_splat(2.0f64)
            val lt = a < b
            val mixed = __simd_insert(a, 1u64, 5.0f64)
            val partial = mixed < b
            if __simd_all(lt) && __simd_any(partial) && !__simd_all(partial) {
                17u64
            } else {
                0u64
            }
        }
    "#;
    assert_simd(src, "simd_comparison_produces_a_mask", 17u64);
}

#[test]
fn simd_select_is_branch_free_lane_choice() {
    let src = r#"
        fn main() -> u64 {
            val a: i32x4 = __simd_splat(10i32)
            val b: i32x4 = __simd_splat(20i32)
            # lane 0 is 30, so `a < b` is false there and true elsewhere
            val a2 = __simd_insert(a, 0u64, 30i32)
            val mask = a2 < b
            val picked = __simd_select(mask, a2, b)
            val lane0: i32 = __simd_extract(picked, 0u64)
            val lane1: i32 = __simd_extract(picked, 1u64)
            if lane0 == 20i32 && lane1 == 10i32 { 55u64 } else { 0u64 }
        }
    "#;
    assert_simd(src, "simd_select_is_branch_free_lane_choice", 55u64);
}

#[test]
fn simd_select_across_lane_types_keeps_the_value_shape() {
    // A float vector's mask is an integer vector of the same lane
    // width, so `select` has two different types in play: the result
    // must take the *values*' shape, not the mask's. Getting this
    // backwards made `__simd_select(mask, a, b)` on `f64x2` come back
    // as `i64x2` (printing raw bit patterns) and made cranelift's
    // verifier reject the `bitselect`.
    let src = r#"
        fn main() -> u64 {
            val a: f64x2 = __simd_splat(1.5f64)
            val b: f64x2 = __simd_splat(2.5f64)
            val mask = a < b
            val picked = __simd_select(mask, a, b)
            val s: f64 = __simd_reduce_add(picked)
            if s == 3.0f64 { 47u64 } else { 0u64 }
        }
    "#;
    assert_simd(src, "simd_select_across_lane_types_keeps_the_value_shape", 47u64);
}

#[test]
fn simd_load_and_store_address_by_element() {
    // Written through `__builtin_ptr_write` at byte offsets 0 and 8,
    // read back as one `f64x2` starting at element 0 — the two
    // addressing conventions have to line up or the lanes come back
    // shifted.
    let src = r#"
        unsafe fn main() -> u64 {
            val p = __builtin_heap_alloc(64u64)
            __builtin_ptr_write(p, 0u64, 1.5f64)
            __builtin_ptr_write(p, 8u64, 2.5f64)
            val v: f64x2 = __simd_load(p, 0u64)
            val doubled = v + v
            __simd_store(p, 2u64, doubled)
            val back: f64 = __builtin_ptr_read::<f64>(p, 16u64)
            val back2: f64 = __builtin_ptr_read::<f64>(p, 24u64)
            __builtin_heap_free(p)
            if back == 3.0f64 && back2 == 5.0f64 { 61u64 } else { 0u64 }
        }
    "#;
    assert_simd(src, "simd_load_and_store_address_by_element", 61u64);
}

#[test]
fn simd_u8x16_bitwise_and_shift() {
    let src = r#"
        fn main() -> u64 {
            val v: u8x16 = __simd_splat(200u8)
            val masked = v & __simd_splat(15u8)
            val shifted = v >> 2u64
            val a: u8 = __simd_extract(masked, 0u64)
            val b: u8 = __simd_extract(shifted, 15u64)
            if a == 8u8 && b == 50u8 { 71u64 } else { 0u64 }
        }
    "#;
    assert_simd(src, "simd_u8x16_bitwise_and_shift", 71u64);
}

#[test]
fn simd_unary_operators_are_lane_wise() {
    let src = r#"
        fn main() -> u64 {
            val a: i32x4 = __simd_splat(5i32)
            val neg = -a
            val inv = ~a
            val n: i32 = __simd_extract(neg, 2u64)
            val i: i32 = __simd_extract(inv, 3u64)
            if n == -5i32 && i == -6i32 { 12u64 } else { 0u64 }
        }
    "#;
    assert_simd(src, "simd_unary_operators_are_lane_wise", 12u64);
}

#[test]
fn simd_reduce_min_max_and_bitwise_agree() {
    let src = r#"
        fn main() -> u64 {
            val base: i32x4 = __simd_splat(6i32)
            val v0 = __simd_insert(base, 0u64, 3i32)
            val v1 = __simd_insert(v0, 1u64, 12i32)
            val lo: i32 = __simd_reduce_min(v1)
            val hi: i32 = __simd_reduce_max(v1)
            val and: i32 = __simd_reduce_and(v1)
            val or: i32 = __simd_reduce_or(v1)
            if lo == 3i32 && hi == 12i32 && and == 0i32 && or == 15i32 {
                29u64
            } else {
                0u64
            }
        }
    "#;
    assert_simd(src, "simd_reduce_min_max_and_bitwise_agree", 29u64);
}

#[test]
fn simd_crosses_function_boundaries_as_one_value() {
    // Unlike a struct, a vector is not decomposed into leaves at the
    // call boundary — it goes through as one SSA value in every
    // compiled lane.
    let src = r#"
        fn scale(v: f64x2, by: f64) -> f64x2 {
            v * __simd_splat_by(by)
        }
        fn __simd_splat_by(x: f64) -> f64x2 {
            val out: f64x2 = __simd_splat(x)
            out
        }
        fn main() -> u64 {
            val a: f64x2 = __simd_splat(2.0f64)
            val scaled = scale(a, 3.0f64)
            val s: f64 = __simd_reduce_add(scaled)
            if s == 12.0f64 { 88u64 } else { 0u64 }
        }
    "#;
    assert_simd(src, "simd_crosses_function_boundaries_as_one_value", 88u64);
}

#[test]
fn simd_printing_agrees_across_backends() {
    // A vector prints as its type name applied to its lanes, with
    // each lane spelled exactly as that scalar would be on its own —
    // including the `.0` an integral float keeps.
    let src = r#"
        fn main() -> u64 {
            val a: f64x2 = __simd_splat(1.5f64)
            val b = __simd_insert(a, 1u64, 4f64)
            println(b)
            val i: i32x4 = __simd_splat(-3i32)
            println(i)
            val u: u8x16 = __simd_splat(7u8)
            println(u)
            val f: f32x4 = __simd_splat(0.25f32)
            println(f)
            println("interp {b}")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "simd_printing_agrees_across_backends");
}

// ---------------------------------------------------------------------
// Stdlib kernels (SIMD.md 戦略 B).
//
// `String::eq`, `String::to_upper` / `to_lower`, and
// `String::contains` process 16 bytes at a time. Every input here is
// deliberately longer than 16 bytes, with a tail that is not a
// multiple of 16 -- the rest of the suite works on short strings and
// never reaches the vector path at all, so a chunk/tail boundary bug
// would go unnoticed.
// ---------------------------------------------------------------------

#[test]
fn stdlib_string_eq_spans_chunks_and_tail() {
    // 43 bytes: two full 16-byte chunks plus an 11-byte tail. The
    // difference in the third pair sits in the tail, and the one in
    // the fourth sits inside the first chunk, so both halves of the
    // kernel have to be right.
    let src = r#"
        fn main() -> u64 {
            val a = String::from_str("the quick brown fox jumps over the lazy dog")
            val b = String::from_str("the quick brown fox jumps over the lazy dog")
            val tail = String::from_str("the quick brown fox jumps over the lazy dig")
            val head = String::from_str("The quick brown fox jumps over the lazy dog")
            val longer = String::from_str("the quick brown fox jumps over the lazy dogs")
            val empty = String::from_str("")
            val empty2 = String::from_str("")
            if a == b && !(a == tail) && !(a == head) && !(a == longer) && empty == empty2 {
                91u64
            } else {
                0u64
            }
        }
    "#;
    assert_simd(src, "stdlib_string_eq_spans_chunks_and_tail", 91u64);
}

#[test]
fn stdlib_case_folding_spans_chunks_and_tail() {
    // Non-letters (digits, punctuation, spaces) must pass through the
    // lane-wise fold untouched, and the 5-byte tail is handled by the
    // scalar loop.
    let src = r#"
        fn main() -> u64 {
            val mixed = String::from_str("Hello, World! 123 The Quick Brown Fox xyzXY")
            val up = mixed.to_ascii_upper()
            val lo = mixed.to_ascii_lower()
            val want_up = String::from_str("HELLO, WORLD! 123 THE QUICK BROWN FOX XYZXY")
            val want_lo = String::from_str("hello, world! 123 the quick brown fox xyzxy")
            val empty = String::from_str("")
            val eu = empty.to_ascii_upper()
            if up == want_up && lo == want_lo && eu.len() == 0u64 {
                93u64
            } else {
                0u64
            }
        }
    "#;
    assert_simd(src, "stdlib_case_folding_spans_chunks_and_tail", 93u64);
}

#[test]
fn stdlib_contains_skips_whole_chunks() {
    // The needle's first byte is absent from the first 30 bytes, so
    // the memchr-style skip advances 16 at a time before the naive
    // compare ever runs. A match that starts inside the final partial
    // chunk exercises the non-vector path of the same loop.
    let src = r#"
        fn find(h: &String, s: str) -> bool {
            val needle = String::from_str(s)
            h.contains(needle)
        }
        fn main() -> u64 {
            val h = String::from_str("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaZq tail")
            val plain = String::from_str("the quick brown fox jumps over the lazy dog")
            if find(h, "Zq")
                && find(h, "tail")
                && !find(h, "Zz")
                && find(plain, "quick")
                && find(plain, "dog")
                && !find(plain, "cat")
                && find(plain, "")
            {
                95u64
            } else {
                0u64
            }
        }
    "#;
    assert_simd(src, "stdlib_contains_skips_whole_chunks", 95u64);
}

#[test]
fn stdlib_split_skips_whole_chunks() {
    // The separator's first byte is absent from the first 30 bytes,
    // so the skip advances 16 at a time before the naive compare
    // runs. `start` must not move while skipping, or the parts come
    // out with the wrong boundaries -- which is the whole risk of
    // adding a skip to a loop that also tracks a slice origin.
    let src = r#"
        fn part_len(h: &String, s: str, k: u64) -> u64 {
            val sep = String::from_str(s)
            val parts = h.split(sep)
            val p: &String = parts.borrow(k)
            p.len()
        }
        fn count(h: &String, s: str) -> u64 {
            val sep = String::from_str(s)
            val parts = h.split(sep)
            parts.size()
        }
        fn main() -> u64 {
            val a = String::from_str("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaZq tail Zq end")
            val b = String::from_str("a,b,c")
            val c = String::from_str("no-separator-here-at-all-long-enough")
            if count(a, "Zq") == 3u64
                && part_len(a, "Zq", 0u64) == 30u64
                && part_len(a, "Zq", 1u64) == 6u64
                && part_len(a, "Zq", 2u64) == 4u64
                && count(b, ",") == 3u64
                && count(c, "|") == 1u64
            {
                97u64
            } else {
                0u64
            }
        }
    "#;
    assert_simd(src, "stdlib_split_skips_whole_chunks", 97u64);
}

// ---------------------------------------------------------------
// `__simd_bitmask` / `__simd_swizzle` / `__simd_bitcast`
// ---------------------------------------------------------------

#[test]
fn simd_bitmask_says_which_lanes_matched() {
    // The point of the intrinsic: `__simd_any` says *whether* a byte
    // is in the chunk, `__simd_bitmask` plus `trailing_zeros` says
    // *where* -- which is what turns the stdlib's memchr from a
    // per-byte rescan into a jump.
    let src = r#"
        fn main() -> u64 {
            val a: u8x16 = __simd_splat(1u8)
            val b = __simd_insert(__simd_insert(a, 3u64, 9u8), 7u64, 9u8)
            val hits = __simd_bitmask(b == __simd_splat(9u8))
            val first = hits.trailing_zeros()
            if hits == 136u64 && first == 3u32 { 51u64 } else { 0u64 }
        }
    "#;
    assert_simd(src, "simd_bitmask_says_which_lanes_matched", 51u64);
}

#[test]
fn simd_bitmask_reads_the_high_bit_not_non_zero() {
    // `__simd_any` asks whether a lane is non-zero; `__simd_bitmask`
    // gathers the lane's **most significant** bit. Lanes of `1` make
    // the two answers differ, and every backend has to pick the same
    // one -- the machine instructions gather the MSB, so that is the
    // definition. A float lane's MSB is its sign bit.
    let src = r#"
        fn main() -> u64 {
            val ones: i32x4 = __simd_splat(1i32)
            val neg: i32x4 = __simd_splat(0i32 - 1i32)
            val f: f64x2 = __simd_splat(0f64 - 1.5f64)
            if __simd_bitmask(ones) == 0u64
                && __simd_any(ones)
                && __simd_bitmask(neg) == 15u64
                && __simd_bitmask(f) == 3u64
            {
                53u64
            } else {
                0u64
            }
        }
    "#;
    assert_simd(src, "simd_bitmask_reads_the_high_bit_not_non_zero", 53u64);
}

#[test]
fn simd_swizzle_looks_up_a_byte_table() {
    // Runtime indices, unlike the constant lane of `__simd_extract`:
    // this is the 16-entry lookup a hex or base64 encoder needs. An
    // index of 16 or more selects zero, which is the rule both
    // `pshufb` (as cranelift normalises it) and NEON's `tbl` follow --
    // pin it, because a backend that wrapped modulo 16 instead would
    // still look plausible.
    let src = r#"
        fn main() -> u64 {
            val base: u8x16 = __simd_splat(0u8)
            val t = __simd_insert(
                __simd_insert(__simd_insert(base, 0u64, 10u8), 1u64, 20u8),
                2u64, 30u8)
            val ib: u8x16 = __simd_splat(2u8)
            val idx = __simd_insert(__simd_insert(ib, 0u64, 1u8), 15u64, 99u8)
            val out = __simd_swizzle(t, idx)
            val l0: u8 = __simd_extract(out, 0u64)
            val l1: u8 = __simd_extract(out, 1u64)
            val l15: u8 = __simd_extract(out, 15u64)
            if l0 == 20u8 && l1 == 30u8 && l15 == 0u8 { 55u64 } else { 0u64 }
        }
    "#;
    assert_simd(src, "simd_swizzle_looks_up_a_byte_table", 55u64);
}

#[test]
fn simd_bitcast_keeps_the_bytes_in_little_endian_order() {
    // A bitcast moves no bits: it is `__simd_store` followed by
    // `__simd_load` at the other type. The lane *order* is the part
    // that could differ between engines, so the test reads a value
    // back whose bytes are all distinct positions.
    let src = r#"
        fn main() -> u64 {
            val a: u8x16 = __simd_splat(1u8)
            val b = __simd_insert(a, 3u64, 9u8)
            val w: i32x4 = __simd_bitcast(b)
            val lane0: i32 = __simd_extract(w, 0u64)
            val back: u8x16 = __simd_bitcast(w)
            val byte3: u8 = __simd_extract(back, 3u64)
            val byte0: u8 = __simd_extract(back, 0u64)

            val f: f64x2 = __simd_splat(1.0f64)
            val bits: i64x2 = __simd_bitcast(f)
            val exponent: i64 = __simd_extract(bits, 0u64)

            if lane0 == 151060737i32
                && byte3 == 9u8
                && byte0 == 1u8
                && exponent == 4607182418800017408i64
            {
                57u64
            } else {
                0u64
            }
        }
    "#;
    assert_simd(
        src,
        "simd_bitcast_keeps_the_bytes_in_little_endian_order",
        57u64,
    );
}

#[test]
fn simd_shuffle_permutes_two_vectors() {
    // The mask indexes `a` followed by `b`, so an index at or above
    // the lane count reaches into the second vector. Interleaving
    // the low halves is the case a codec needs and the one that
    // catches a backend that treats the mask as two separate
    // selections.
    let src = r#"
        fn main() -> u64 {
            val a0: i32x4 = __simd_splat(10i32)
            val a = __simd_insert(
                __simd_insert(__simd_insert(a0, 1u64, 11i32), 2u64, 12i32),
                3u64, 13i32)
            val b0: i32x4 = __simd_splat(20i32)
            val b = __simd_insert(
                __simd_insert(__simd_insert(b0, 1u64, 21i32), 2u64, 22i32),
                3u64, 23i32)

            val zip = __simd_shuffle(a, b, [0u64, 4u64, 1u64, 5u64])
            val l0: i32 = __simd_extract(zip, 0u64)
            val l1: i32 = __simd_extract(zip, 1u64)
            val l3: i32 = __simd_extract(zip, 3u64)

            val rev = __simd_shuffle(a, a, [3u64, 2u64, 1u64, 0u64])
            val r0: i32 = __simd_extract(rev, 0u64)
            val r3: i32 = __simd_extract(rev, 3u64)

            if l0 == 10i32 && l1 == 20i32 && l3 == 21i32
                && r0 == 13i32 && r3 == 10i32
            {
                61u64
            } else {
                0u64
            }
        }
    "#;
    assert_simd(src, "simd_shuffle_permutes_two_vectors", 61u64);
}

#[test]
fn simd_shuffle_works_on_every_lane_width() {
    // Cranelift's `shuffle` is an `I8X16` instruction taking *byte*
    // indices, so every lane wider than a byte becomes a run of
    // consecutive bytes. Get that widening wrong and an `f64x2` swap
    // still "works" on the tree-walker while the compiled lanes
    // return halves of two different lanes -- which is why both ends
    // of the width range are here.
    let src = r#"
        fn main() -> u64 {
            val f0: f64x2 = __simd_splat(1.5f64)
            val f = __simd_insert(f0, 1u64, 2.5f64)
            val g: f64x2 = __simd_splat(9.5f64)
            val swapped = __simd_shuffle(f, g, [3u64, 0u64])
            val s0: f64 = __simd_extract(swapped, 0u64)
            val s1: f64 = __simd_extract(swapped, 1u64)

            val u0: u8x16 = __simd_splat(7u8)
            val u = __simd_insert(u0, 5u64, 99u8)
            val z: u8x16 = __simd_splat(0u8)
            val picked = __simd_shuffle(u, z, [
                5u64, 5u64, 16u64, 5u64, 5u64, 5u64, 5u64, 5u64,
                5u64, 5u64, 5u64, 5u64, 5u64, 5u64, 5u64, 5u64])
            val p0: u8 = __simd_extract(picked, 0u64)
            val p2: u8 = __simd_extract(picked, 2u64)

            if s0 == 9.5f64 && s1 == 1.5f64 && p0 == 99u8 && p2 == 0u8 {
                63u64
            } else {
                0u64
            }
        }
    "#;
    assert_simd(src, "simd_shuffle_works_on_every_lane_width", 63u64);
}

#[test]
fn simd_shuffle_mask_is_folded_in_every_position() {
    // The mask is folded in the *parser*, not in the type checker's
    // rewrite prologue, because a bare expression statement, a binary
    // operand and an `if` condition never reach that prologue. Each
    // of those positions appears here: before the fold moved, three
    // of them were rejected as "no mask" while the same call in an
    // argument position compiled.
    let src = r#"
        fn main() -> u64 {
            val a0: i32x4 = __simd_splat(1i32)
            val a = __simd_insert(a0, 1u64, 4i32)
            __simd_shuffle(a, a, [1u64, 0u64, 1u64, 0u64])
            val sum = __simd_shuffle(a, a, [1u64, 1u64, 1u64, 1u64]) + a
            val total: i32 = __simd_reduce_add(sum)
            if __simd_all(__simd_shuffle(a, a, [0u64, 1u64, 2u64, 3u64]) == a)
                && total == 23i32
            {
                65u64
            } else {
                0u64
            }
        }
    "#;
    assert_simd(src, "simd_shuffle_mask_is_folded_in_every_position", 65u64);
}
