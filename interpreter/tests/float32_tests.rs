// `f32` — the single-precision float (SIMD-F32).
//
// The parser emits `Expr::Float32` / `TypeDecl::Float32` from the
// `f32` keyword and `1.5f32`-style literals; every lane treats it as
// a scalar with IEEE-754 single-precision semantics. Mixing f32 with
// f64 or an integer is a type error — cross-width moves go through
// `as`. These tests run the tree-walker end-to-end; the
// `compiler/tests/consistency/float32.rs` pins the 3-backend
// agreement and `interpreter/example/float32.t` feeds the sweep.

use crate::common::{assert_program_result_u64, core_modules_dir};

fn assert_type_error_message(src: &str, expected_hint: &str) {
    let core = core_modules_dir();
    let mut parser = frontend::ParserWithInterner::new(src);
    parser.set_source_file("float32.t");
    let mut program = parser
        .parse_program()
        .map_err(|e| format!("Parse error: {e:?}"))
        .expect("parse failed");
    let interner = parser.get_string_interner();
    let err = interpreter::check_typing_with_core_modules(
        &mut program,
        interner,
        Some(src),
        Some("float32.t"),
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
// Literals, arithmetic, comparisons.
// ---------------------------------------------------------------------

#[test]
fn f32_arithmetic_is_single_precision() {
    // `0.1f32 + 0.2f32 == 0.3f32` is true at single precision (and
    // false in f64) — the distinguishing semantics of the type.
    let src = r#"
        fn main() -> u64 {
            val a: f32 = 1.5f32
            val b: f32 = 2.25f32
            val c: f32 = a * b + 0.5f32
            val prec = 0.1f32 + 0.2f32 == 0.3f32
            if c == 3.875f32 && prec { 7u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 7u64);
}

#[test]
fn f32_unary_minus_and_comparison() {
    let src = r#"
        fn main() -> u64 {
            val a: f32 = 1.5f32
            val b: f32 = -a
            if a < 2f32 && b > -2f32 && b == -1.5f32 { 5u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 5u64);
}

// ---------------------------------------------------------------------
// Casts: f32 ↔ f64 and f32 → int.
// ---------------------------------------------------------------------

#[test]
fn f32_cast_matrix() {
    let src = r#"
        fn main() -> u64 {
            val w: f64 = 3.875f32 as f64
            val back: f32 = w as f32
            val i: u64 = 3.9f32 as u64
            val s: i64 = -2.7f32 as i64
            if back == 3.875f32 && i == 3u64 && s == -2i64 {
                w as u64
            } else {
                0u64
            }
        }
    "#;
    assert_program_result_u64(src, 3u64);
}

// ---------------------------------------------------------------------
// Compile-time evaluation and constants.
// ---------------------------------------------------------------------

#[test]
fn f32_const_folds() {
    let src = r#"
        const G: f32 = 1.5f32 * 2f32
        fn main() -> u64 {
            if G == 3.0f32 { 7u64 } else { 0u64 }
        }
    "#;
    assert_program_result_u64(src, 7u64);
}

// ---------------------------------------------------------------------
// Errors: mixed widths and non-float literals are rejected.
// ---------------------------------------------------------------------

#[test]
fn f32_rejects_mixed_width_arithmetic() {
    let src = r#"
        fn main() -> u64 {
            val a: f32 = 1.5f32
            val b: f64 = 2.25f64
            val c = a + b
            0u64
        }
    "#;
    assert_type_error_message(src, "Type mismatch");
}

#[test]
fn f32_rejects_bare_suffixless_literal() {
    // Floats keep the mandatory-suffix rule (`1.5` still lexes as
    // tuple access `1 . 5`); `1.5f32` is required.
    let src = r#"
        fn main() -> u64 {
            val a: f32 = 1.5
            0u64
        }
    "#;
    assert_type_error_message(src, "non-tuple");
}
