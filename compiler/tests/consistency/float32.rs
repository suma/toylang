// SIMD-F32: the single-precision primitive across all three backends.
//
// `f32` is a first-class scalar: literals (`1.5f32`), lane-wise IEEE
// arithmetic, comparisons, unary minus, the `as` cast matrix (f32 ↔
// f64 ↔ every integer width), `const` initialisers, and print
// formatting ("always a decimal point" — `1.0f32` prints `1.0`).
// The interpreter JIT takes its usual silent fallback; the other
// three lanes must agree byte-for-byte.

use super::harness::*;

#[test]
fn f32_arithmetic_and_literals_agree() {
    // f32 bit patterns are not wired through the process exit code,
    // so the result is compared in-language and reported as u64.
    let src = r#"
        fn compute() -> f32 {
            val a: f32 = 1.5f32
            val b: f32 = 2.25f32
            a * b + 0.5f32 - 1f32
        }
        fn main() -> u64 {
            if compute() == 2.875f32 { 44u64 } else { 0u64 }
        }
    "#;
    assert_consistent(src, "f32_arithmetic_and_literals_agree");
}

#[test]
fn f32_precision_is_single_not_double() {
    // `0.1f32 + 0.2f32 == 0.3f32` is TRUE at single precision (the
    // f64 experiment famously fails this). Pinning it here proves the
    // backend evaluates in f32, not by promoting through f64.
    let src = r#"
        fn main() -> u64 {
            if 0.1f32 + 0.2f32 == 0.3f32 { 7u64 } else { 0u64 }
        }
    "#;
    assert_consistent(src, "f32_precision_is_single_not_double");
}

#[test]
fn f32_comparison_and_unary_minus_agree() {
    let src = r#"
        fn main() -> u64 {
            val a: f32 = 1.5f32
            val b: f32 = -a
            if a < 2f32 && b > -2f32 { (a - b) as u64 } else { 0u64 }
        }
    "#;
    assert_consistent(src, "f32_comparison_and_unary_minus_agree");
}

#[test]
fn f32_cast_matrix_round_trips() {
    // f32 ↔ f64 promote / demote and f32 → int (saturating) through
    // every compiled lane.
    let src = r#"
        fn main() -> u64 {
            val w: f64 = 3.875f32 as f64
            val back: f32 = w as f32
            val big: f32 = 3000000000f32
            val i: u64 = big as u64
            val tiny: i64 = back as i64
            val same: u64 = if back == 3.875f32 { 1u64 } else { 0u64 }
            same * 1_000_000u64 + i + tiny as u64
        }
    "#;
    assert_consistent(src, "f32_cast_matrix_round_trips");
}

#[test]
fn f32_const_initialiser_folds() {
    // The driver-layer CTFE folds `1.5f32 * 2f32` to `3.0f32` before
    // lowering, so every backend sees the same literal.
    let src = r#"
        const G: f32 = 1.5f32 * 2f32
        fn main() -> u64 {
            if G == 3.0f32 { 7u64 } else { 0u64 }
        }
    "#;
    assert_consistent(src, "f32_const_initialiser_folds");
}

#[test]
fn f32_print_formatting_agrees() {
    let src = r#"
        fn main() -> u64 {
            println(1.0f32)
            println(0.5f32)
            println(-2.25f32)
            0u64
        }
    "#;
    assert_consistent(src, "f32_print_formatting_agrees");
}

#[test]
fn an_f32_field_does_not_stop_the_drop_glue() {
    // The leaf-type list the glue signature is built from was written
    // before `f32` existed, and nothing added it — so a struct with
    // an `f32` field could not be a `Vec` element on the compiled
    // lanes at all ("drop glue: unsupported leaf type f32"), even
    // though the leaf owns nothing, exactly like `f64`.
    //
    // The `Box` is what makes this a drop-glue question: the glue has
    // to walk *past* the float leaves to reach it.
    let src = r#"
        struct S { a: f32, b: Box<i64>, c: f32 }

        fn main() -> i64 {
            var v: Vec<S> = Vec::new()
            var i: u64 = 0u64
            while i < 3u64 {
                val one = S { a: 1.5f32, b: Box::new(i as i64), c: 2.5f32 }
                v.push(one)
                i = i + 1u64
            }
            var total: i64 = 0i64
            var k: u64 = 0u64
            while k < v.size() {
                val e: &S = v.borrow(k)
                total = total + e.b.get()
                k = k + 1u64
            }
            println(total)
            0i64
        }
    "#;
    assert_renders(src, "f32_leaf_drop_glue", "3\n");
    // And the boxes are freed: the glue reached them.
    let report = memory_profile_report(src, "prof_f32_leaf_drop_glue");
    assert!(
        report.contains("live_bytes        0"),
        "something leaked:\n{report}"
    );
}

#[test]
fn f32_travels_by_reference_and_lives_in_an_array() {
    // SIMD-F32 added `f32` as a scalar, and three hand-written type
    // lists were not told. Each is the same kind of list, and each
    // failed differently:
    //
    // * `&f32` / `&mut f32` — the list that decides which parameters
    //   are passed as an address disagreed with the one that binds
    //   them on the callee side, by exactly this type. The two are
    //   documented as having to give the same answer, and the
    //   disagreement was not a diagnostic: cranelift's verifier
    //   panicked with "declared type of variable var0 doesn't match
    //   type of value v0".
    // * `[f32; N]` — refused as an element type, while
    //   `elem_stride_bytes` had given f32 a native 4-byte stride all
    //   along.
    //
    // They share one definition now (`is_scalar_pointee`).
    let src = r#"
        struct P { x: f32, y: f32 }

        fn take(v: &f32) -> f32 { v * 2.0f32 }

        fn bump(v: &mut f32) { v = v + 1.0f32 }

        fn main() -> u64 {
            var one: f32 = 1.5f32
            println(take(&one))
            bump(&mut one)
            println(one)

            # A field chain ending in an f32 leaf.
            val p = P { x: 3.0f32, y: 4.0f32 }
            println(take(&p.y))

            # An array of them, read, written and summed.
            var arr: [f32; 4] = [1.0f32, 2.0f32, 3.0f32, 4.0f32]
            arr[0u64] = 10.5f32
            var sum: f32 = 0.0f32
            for i in 0u64..4u64 {
                sum = sum + arr[i]
            }
            println(sum)
            println(take(&arr[2u64]))
            0u64
        }
    "#;
    assert_renders(src, "f32_by_reference", "3.0\n2.5\n8.0\n19.5\n6.0\n");
}

#[test]
fn a_field_of_an_array_element_can_be_printed() {
    // `val x = ps[i].q` lowered fine and `println(ps[i].q)` did not:
    // the print path asked `resolve_field_chain`, which refuses a
    // chain rooted at anything but a bare identifier, and propagated
    // that refusal instead of falling through to the value path —
    // which lowers exactly this to one leaf load (DATA-ORIENTED).
    let src = r#"
        struct V { p: u64, q: f32 }

        fn main() -> u64 {
            var ps: [V; 2] = [V { p: 1u64, q: 2.5f32 }, V { p: 3u64, q: 4.5f32 }]
            println(ps[1u64].p)
            println(ps[0u64].q)
            var qs: soa [V; 2] = [V { p: 5u64, q: 6.5f32 }, V { p: 7u64, q: 8.5f32 }]
            println(qs[1u64].p)
            println(qs[0u64].q)
            0u64
        }
    "#;
    assert_renders(src, "print_array_element_field", "3\n2.5\n7\n6.5\n");
}
