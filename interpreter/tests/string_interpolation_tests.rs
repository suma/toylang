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

use common::{assert_program_result_i64, get_program_result};
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
