// String interpolation tests — `"hello {name}, sum={a + b}"`.
//
// Implementation: lexer detects `{...}` segments inside string
// literals and emits `Kind::InterpolatedString(parts)`. The parser
// desugars the token at parse time into a chain of `.concat()`
// calls with each `{expr}` lifted through `__builtin_to_string(...)`:
//
//     "lit0" .concat( __builtin_to_string( e0 ) ) .concat( "lit1" )
//            .concat( __builtin_to_string( e1 ) ) .concat( ... )
//
// Empty literal segments are filtered so adjacent `{a}{b}` doesn't
// produce a `"".concat(...)` step. `{{` / `}}` lex to literal
// `{` / `}` (Rust convention).

mod common;

use common::{assert_program_result_i64, get_program_result, test_program};
use interpreter::object::Object;

/// Run a toylang program returning a string and read it back. The
/// runtime represents string values two ways:
///   - `Object::String(owned)` — produced by every `.concat(...)`
///     in the desugared interpolation chain (and by user code that
///     builds strings at runtime).
///   - `Object::ConstString(symbol)` — produced for plain string
///     literals when no interpolation / runtime concatenation
///     happens. The symbol resolves through the interpreter's
///     string interner.
///
/// The helper accepts both. The interpreter binary holds the
/// interner internally so we can't resolve a symbol from outside;
/// instead, the no-interpolation regression test below uses
/// `assert_program_result_i64` indirection (length check) rather
/// than asserting an exact symbol-string round-trip.
fn run_returns_owned_string(src: &str) -> String {
    let result = get_program_result(src);
    let s = match &*result.borrow() {
        Object::String(s) => s.clone(),
        Object::ConstString(_) => panic!(
            "expected owned Object::String (interpolation must produce one via .concat()), \
             got ConstString — string interpolation didn't run"
        ),
        other => panic!("expected Object::String, got {:?}", other),
    };
    s
}

#[test]
fn interpolation_with_identifier_argument() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val name = "world"
            "hello {name}"
        }"#,
    );
    assert_eq!(s, "hello world");
}

#[test]
fn interpolation_with_arithmetic_expression() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val x: i64 = 7i64
            val y: i64 = 35i64
            "sum is {x + y}"
        }"#,
    );
    assert_eq!(s, "sum is 42");
}

#[test]
fn interpolation_with_multiple_segments() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val name = "Alice"
            val age: i64 = 30i64
            "name={name}, age={age}, next={age + 1i64}"
        }"#,
    );
    assert_eq!(s, "name=Alice, age=30, next=31");
}

#[test]
fn interpolation_at_start_of_string() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val name = "Bob"
            "{name} arrived"
        }"#,
    );
    assert_eq!(s, "Bob arrived");
}

#[test]
fn interpolation_at_end_of_string() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val n: i64 = 42i64
            "answer = {n}"
        }"#,
    );
    assert_eq!(s, "answer = 42");
}

#[test]
fn interpolation_only_no_surrounding_text() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val n: i64 = 42i64
            "{n}"
        }"#,
    );
    assert_eq!(s, "42");
}

#[test]
fn interpolation_adjacent_expressions() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val a = "x"
            val b = "y"
            "{a}{b}"
        }"#,
    );
    assert_eq!(s, "xy");
}

#[test]
fn interpolation_double_brace_escapes_literal() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val n: i64 = 7i64
            "value {{is {n}}}"
        }"#,
    );
    assert_eq!(s, "value {is 7}");
}

#[test]
fn interpolation_with_bool_expression() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val x: i64 = 5i64
            "positive: {x > 0i64}"
        }"#,
    );
    assert_eq!(s, "positive: true");
}

#[test]
fn interpolation_with_function_call() {
    let s = run_returns_owned_string(
        r#"fn double(x: i64) -> i64 { x * 2i64 }
        fn main() -> str {
            val n: i64 = 21i64
            "doubled = {double(n)}"
        }"#,
    );
    assert_eq!(s, "doubled = 42");
}

#[test]
fn plain_string_literal_remains_const_string() {
    // Regression: a literal with no `{...}` segment must still
    // tokenize as the plain `Kind::String` path, not get rewired
    // through interpolation. Plain literals stay as
    // `Object::ConstString` (interned via the string interner)
    // for memory efficiency, while interpolation always produces
    // an owned `Object::String` via the desugared `.concat()`
    // chain. This test pins both halves.
    let result = get_program_result(
        r#"fn main() -> str {
            "plain string with no braces"
        }"#,
    );
    let is_const = matches!(&*result.borrow(), Object::ConstString(_));
    assert!(is_const, "plain literal should remain ConstString, got {:?}", result.borrow());
}

#[test]
fn interpolation_can_be_passed_to_println() {
    // Doesn't assert stdout (the test harness doesn't capture it),
    // but exercises the desugaring → method-call path through the
    // builtin `println` argument slot. Just checking that the
    // program runs to completion and returns the expected value.
    assert_program_result_i64(
        r#"fn main() -> i64 {
            val name = "world"
            val n: i64 = 42i64
            println("hello {name}, n={n}")
            n
        }"#,
        42,
    );
}

#[test]
fn interpolation_inside_concat_chain() {
    // Whole interpolation chain participates in further postfix
    // method calls — `.to_upper()` should receive the concat
    // result and process it correctly.
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val name = "world"
            "hello {name}".to_upper()
        }"#,
    );
    assert_eq!(s, "HELLO WORLD");
}

// ---------------------------------------------------------------
// STR-INTERP-COMPOUND: struct values can now be interpolated.
// AOT side uses `ConstStrBytes` for format prefixes + per-field
// `toy_to_string_<ty>` + `toy_str_concat` chain. Interpreter
// already routes through `Object::to_display_string`, which the
// AOT output matches byte-for-byte (alphabetical field order,
// `TypeName { name: value, ... }`).
// ---------------------------------------------------------------

#[test]
fn interpolation_with_struct_value() {
    let s = run_returns_owned_string(
        r#"
        struct Point { x: i64, y: i64 }
        fn main() -> str {
            val p: Point = Point { x: 3i64, y: 5i64 }
            "p = {p}"
        }
        "#,
    );
    assert_eq!(s, "p = Point { x: 3, y: 5 }");
}

#[test]
fn interpolation_with_struct_alphabetical_field_order() {
    // Declaration order is `(z, a)`; output must be sorted
    // alphabetically (`a` before `z`) to match the interpreter's
    // `Object::to_display_string` ordering.
    let s = run_returns_owned_string(
        r#"
        struct Mixed { z: i64, a: i64 }
        fn main() -> str {
            val m: Mixed = Mixed { z: 9i64, a: 1i64 }
            "{m}"
        }
        "#,
    );
    assert_eq!(s, "Mixed { a: 1, z: 9 }");
}

#[test]
fn interpolation_with_tuple_value() {
    // STR-INTERP-COMPOUND-EXTEND tuple branch.
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val t: (i64, u64) = (3i64, 5u64)
            "t = {t}"
        }"#,
    );
    assert_eq!(s, "t = (3, 5)");
}

#[test]
fn interpolation_with_single_element_tuple() {
    // Trailing-comma form `(elem,)` — Rust convention, matches
    // the interpreter's tuple display.
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val t: (i64,) = (42i64,)
            "{t}"
        }"#,
    );
    assert_eq!(s, "(42,)");
}

#[test]
fn interpolation_with_nested_struct() {
    // STR-INTERP-COMPOUND-EXTEND nested-compound branch:
    // `Outer { inner: Inner { ... }, ... }` recurses through
    // `emit_struct_format` for the inner struct.
    let s = run_returns_owned_string(
        r#"
        struct Inner { x: i64, y: i64 }
        struct Outer { inner: Inner, n: u64 }
        fn main() -> str {
            val o: Outer = Outer { inner: Inner { x: 3i64, y: 5i64 }, n: 7u64 }
            "{o}"
        }
        "#,
    );
    assert_eq!(s, "Outer { inner: Inner { x: 3, y: 5 }, n: 7 }");
}

#[test]
fn interpolation_with_struct_mixed_scalar_types() {
    // struct fields restricted to i64 / u64 (the AOT lower
    // already supports more, but the parser/type checker
    // currently rejects narrow ints / f64 / bool in struct
    // field positions — see "field type in struct" diagnostic).
    let s = run_returns_owned_string(
        r#"
        struct Cell { count: u64, total: i64 }
        fn main() -> str {
            val c: Cell = Cell { count: 7u64, total: 42i64 }
            "{c}"
        }
        "#,
    );
    assert_eq!(s, "Cell { count: 7, total: 42 }");
}

// ---------------------------------------------------------------
// STR-INTERP-FMT: format specs (`"{x:.2}"`).
//
// The spec is packed at parse time into the second argument of
// `__builtin_format`, so these tests also cover the packing: a wrong
// bit layout shows up as wrong text. The AOT / JIT agreement is
// pinned separately in `compiler/tests/consistency.rs`.
// ---------------------------------------------------------------

#[test]
fn precision_controls_f64_decimals() {
    // The reason the feature exists: without a spec there is no way
    // to choose how many decimals an f64 shows.
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val pi: f64 = 3.14159265f64
            "{pi:.2}|{pi:.5}|{pi}"
        }"#,
    );
    assert_eq!(s, "3.14|3.14159|3.14159265");
}

#[test]
fn width_and_alignment_pad_the_rendered_value() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val n: u64 = 42u64
            val t: str = "ok"
            "[{n:6}][{n:<6}][{n:^6}][{t:6}][{t:>6}]"
        }"#,
    );
    // Numbers default to right-aligned, text to left-aligned.
    assert_eq!(s, "[    42][42    ][  42  ][ok    ][    ok]");
}

#[test]
fn zero_padding_goes_after_the_sign() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val neg: i64 = -42i64
            val pos: u64 = 42u64
            "{neg:06}|{pos:06}|{neg:6}"
        }"#,
    );
    assert_eq!(s, "-00042|000042|   -42");
}

#[test]
fn radix_types_render_the_alternate_bases() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val n: u64 = 255u64
            "{n:x}|{n:X}|{n:b}|{n:o}"
        }"#,
    );
    assert_eq!(s, "ff|FF|11111111|377");
}

#[test]
fn negative_values_render_twos_complement_at_their_own_width() {
    // A narrow int must not widen to 16 hex digits — the runtime
    // masks to the value's own width.
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val a: i32 = -1i32
            val b: i8 = -1i8
            val c: i64 = -1i64
            "{a:x}|{b:x}|{c:x}"
        }"#,
    );
    assert_eq!(s, "ffffffff|ff|ffffffffffffffff");
}

#[test]
fn bool_takes_width_and_alignment() {
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val flag: bool = true
            "[{flag:7}][{flag:>7}]"
        }"#,
    );
    assert_eq!(s, "[true   ][   true]");
}

#[test]
fn an_empty_spec_is_the_default_rendering() {
    // `"{x:}"` lowers to a plain `__builtin_to_string` — the spec
    // asks for nothing, so it costs nothing.
    let s = run_returns_owned_string(
        r#"fn main() -> str {
            val n: u64 = 7u64
            "{n:}|{n}"
        }"#,
    );
    assert_eq!(s, "7|7");
}

#[test]
fn colons_inside_the_expression_are_not_spec_separators() {
    // A struct literal's field colon sits at brace depth 1, and `::`
    // is consumed as a pair, so neither starts a spec.
    let s = run_returns_owned_string(
        r#"struct P {
            x: i64
        }
        enum Color {
            Red,
            Blue,
        }
        fn main() -> str {
            val c: Color = Color::Red
            "{P { x: 2i64 }}|{c}"
        }"#,
    );
    assert_eq!(s, "P { x: 2 }|Color::Red");
}

#[test]
fn a_malformed_spec_is_reported_at_parse_time() {
    let err = test_program(
        r#"fn main() -> u64 {
            val x: f64 = 1.5f64
            println("{x:.q}")
            0u64
        }"#,
    )
    .expect_err("`.q` is not a precision");
    assert!(
        err.contains("invalid format spec") && err.contains("{x:.q}"),
        "error should quote the offending spec, got: {err}"
    );
}

#[test]
fn a_spec_on_a_compound_value_is_a_type_error() {
    // Width / radix have no defined meaning for a value that renders
    // through a recursive field walk, so it is rejected rather than
    // silently ignored.
    let err = test_program(
        r#"struct P {
            x: i64
        }
        fn main() -> u64 {
            val p: P = P { x: 1i64 }
            println("{p:5}")
            0u64
        }"#,
    )
    .expect_err("a struct cannot carry a format spec");
    assert!(
        err.contains("format spec applies to primitives only"),
        "error should explain the primitive-only rule, got: {err}"
    );
}
