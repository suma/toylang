// SIMD — 128-bit vectors in the tree-walker (SIMD.md Phase 2).
//
// These run the tree-walker end-to-end, which matters because it is
// the oracle the other three engines are checked against: it has its
// own lane implementation (`evaluation/simd.rs`) rather than sharing
// `compiler_lower` with the IR VM / AOT / JIT.
// `compiler/tests/consistency/simd.rs` pins the 3-backend agreement,
// and `interpreter/example/simd.t` feeds the example sweep.

use crate::common::{assert_program_result_u64, core_modules_dir};

fn assert_type_error_message(src: &str, expected_hint: &str) {
    let core = core_modules_dir();
    let mut parser = frontend::ParserWithInterner::new(src);
    parser.set_source_file("simd.t");
    let mut program = parser
        .parse_program()
        .map_err(|e| format!("Parse error: {e:?}"))
        .expect("parse failed");
    let interner = parser.get_string_interner();
    let err = interpreter::check_typing_with_core_modules(
        &mut program,
        interner,
        Some(src),
        Some("simd.t"),
        std::slice::from_ref(&core),
    )
    .expect_err("expected a type error");
    let rendered = format!("{err:?}");
    assert!(
        rendered.contains(expected_hint),
        "expected the error to mention `{expected_hint}`, got: {rendered}"
    );
}

// ---------------------------------------------------------------------
// Lane-wise operators.
// ---------------------------------------------------------------------

#[test]
fn lane_wise_arithmetic_and_reduction() {
    let src = r#"
        fn main() -> u64 {
            val a: f64x2 = __simd_splat(1.5f64)
            val b: f64x2 = __simd_splat(2.0f64)
            val s: f64 = __simd_reduce_add(a * b + a)
            if s == 9.0f64 { 5u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 5u64);
}

#[test]
fn comparison_produces_a_mask_not_a_bool() {
    // Sixteen lanes give sixteen answers, so `==` is a mask and the
    // whole-vector question is `__simd_all`.
    let src = r#"
        fn main() -> u64 {
            val a: u8x16 = __simd_splat(3u8)
            val b = __simd_insert(a, 7u64, 9u8)
            val same = a == b
            if __simd_any(same) && !__simd_all(same) { 6u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 6u64);
}

#[test]
fn integer_lanes_wrap_instead_of_trapping() {
    let src = r#"
        fn main() -> u64 {
            val m: i32x4 = __simd_splat(-2147483648i32)
            val one: i32x4 = __simd_splat(1i32)
            val lane: i32 = __simd_extract(m - one, 0u64)
            if lane == 2147483647i32 { 9u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 9u64);
}

#[test]
fn shift_takes_a_scalar_amount_per_lane() {
    let src = r#"
        fn main() -> u64 {
            val v: i64x2 = __simd_splat(6i64)
            val up = v << 3u64
            val down = v >> 1u64
            val a: i64 = __simd_extract(up, 1u64)
            val b: i64 = __simd_extract(down, 0u64)
            if a == 48i64 && b == 3i64 { 11u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 11u64);
}

// ---------------------------------------------------------------------
// Intrinsics.
// ---------------------------------------------------------------------

#[test]
fn load_and_store_address_by_element() {
    // Lane `k` lives at byte offset `(i + k) * lane_bytes` — the one
    // place `__simd_*` departs from `__builtin_ptr_read`, whose
    // offset is a byte count.
    let src = r#"
        unsafe fn main() -> u64 {
            val p = __builtin_heap_alloc(64u64)
            __builtin_ptr_write(p, 0u64, 10i32)
            __builtin_ptr_write(p, 4u64, 20i32)
            __builtin_ptr_write(p, 8u64, 30i32)
            __builtin_ptr_write(p, 12u64, 40i32)
            val v: i32x4 = __simd_load(p, 0u64)
            val total: i32 = __simd_reduce_add(v)
            __builtin_heap_free(p)
            if total == 100i32 { 13u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 13u64);
}

#[test]
fn store_round_trips_through_ptr_read() {
    let src = r#"
        unsafe fn main() -> u64 {
            val p = __builtin_heap_alloc(64u64)
            val v: f64x2 = __simd_splat(2.5f64)
            __simd_store(p, 1u64, v)
            val a: f64 = __builtin_ptr_read::<f64>(p, 8u64)
            val b: f64 = __builtin_ptr_read::<f64>(p, 16u64)
            __builtin_heap_free(p)
            if a == 2.5f64 && b == 2.5f64 { 15u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 15u64);
}

#[test]
fn reduce_folds_lanes_left_to_right() {
    // The order is part of the language: adding the huge lane first
    // swallows the two small ones, which a pairwise tree would not.
    let src = r#"
        fn main() -> u64 {
            val zero: f32x4 = __simd_splat(0f32)
            val v0 = __simd_insert(zero, 0u64, 16777216f32)
            val v1 = __simd_insert(v0, 1u64, 1f32)
            val v2 = __simd_insert(v1, 2u64, 1f32)
            val s: f32 = __simd_reduce_add(v2)
            if s == 16777216f32 { 19u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 19u64);
}

#[test]
fn select_keeps_the_value_lane_type() {
    // `f64x2`'s mask is `i64x2`; the result takes the values' shape.
    let src = r#"
        fn main() -> u64 {
            val a: f64x2 = __simd_splat(1.0f64)
            val b: f64x2 = __simd_splat(4.0f64)
            val picked = __simd_select(a < b, a, b)
            val s: f64 = __simd_reduce_add(picked)
            if s == 2.0f64 { 23u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 23u64);
}

#[test]
fn vectors_cross_function_boundaries_as_one_value() {
    let src = r#"
        fn double(v: i32x4) -> i32x4 {
            v + v
        }
        fn main() -> u64 {
            val a: i32x4 = __simd_splat(5i32)
            val total: i32 = __simd_reduce_add(double(a))
            if total == 40i32 { 27u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 27u64);
}

// ---------------------------------------------------------------------
// Diagnostics.
// ---------------------------------------------------------------------

#[test]
fn integer_division_on_lanes_is_rejected() {
    let src = r#"
        fn main() -> u64 {
            val a: i32x4 = __simd_splat(4i32)
            val b: i32x4 = __simd_splat(2i32)
            val c = a / b
            0u64
        }
    "#;
    assert_type_error_message(src, "is not defined on `i32x4`");
}

#[test]
fn mixing_a_vector_with_a_scalar_is_rejected() {
    let src = r#"
        fn main() -> u64 {
            val a: f64x2 = __simd_splat(1.0f64)
            val b = a + 1.0f64
            0u64
        }
    "#;
    assert_type_error_message(src, "__simd_splat");
}

#[test]
fn a_lane_index_must_be_a_literal_in_range() {
    let src = r#"
        fn main() -> u64 {
            val a: f64x2 = __simd_splat(1.0f64)
            val x: f64 = __simd_extract(a, 5u64)
            0u64
        }
    "#;
    assert_type_error_message(src, "out of range");
}

#[test]
fn splat_without_a_named_vector_type_is_rejected() {
    let src = r#"
        fn main() -> u64 {
            val a = __simd_splat(1.0f64)
            0u64
        }
    "#;
    assert_type_error_message(src, "no lane-type suffix");
}

#[test]
fn logical_not_on_a_vector_is_rejected() {
    let src = r#"
        fn main() -> u64 {
            val a: f64x2 = __simd_splat(1.0f64)
            val b = !a
            0u64
        }
    "#;
    assert_type_error_message(src, "invert a mask with `~`");
}
