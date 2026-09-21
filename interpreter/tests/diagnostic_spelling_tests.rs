//! DIAG-SYMBOL-NAME: what the reader actually sees.
//!
//! `frontend/tests/diagnostic_spelling_tests.rs` checks the rule at the
//! source: no hand-built type-checker message formats a value with
//! `{:?}`. This file checks it at the other end, through the real
//! rendering path — parse, check, and render with the interner in hand,
//! the way `interpreter` does for a user.
//!
//! Both are needed. The source scan cannot see a leak that arrives
//! through a shared helper, and this corpus cannot see a site no
//! program in it happens to reach.

use crate::common::test_program;

/// Spellings that only Debug produces. `SymbolU32` is the headline —
/// an interned id where a name belongs — and the rest are `TypeDecl`'s
/// variant names, which mean nothing to someone who writes `u64`.
const DEBUG_LEAKS: &[&str] = &[
    "SymbolU32",
    "Struct(",
    "Enum(",
    "Identifier(",
    "UInt64",
    "Int64",
    "Float64",
    "UInt8",
    "Bool,",
];

/// Type-check `source`, which must fail, and return the rendered
/// diagnostics.
fn diagnostics_for(source: &str) -> String {
    match test_program(source) {
        Ok(_) => panic!("expected a type error, but the program checked:\n{source}"),
        Err(rendered) => rendered,
    }
}

/// Every case: the program, and the words its diagnostic must contain.
/// The expectations name *user* spellings — `P`, `u64` — which is the
/// whole point.
fn cases() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    vec![
        (
            "associated function on a struct",
            r#"struct P { x: i64 }
               fn main() -> u64 { val p = P::new()
                 0u64 }"#,
            vec!["Associated function 'new'", "struct 'P'"],
        ),
        (
            "impl method return type",
            r#"struct Q { a: i64 }
               trait T { fn f(self: Self) -> u64 }
               impl T for Q { fn f(self: Self) -> Q { self } }
               fn main() -> u64 { 0u64 }"#,
            vec!["return type mismatch", "expected u64", "found Q"],
        ),
        (
            "impl method parameter type",
            r#"struct Q { a: i64 }
               trait T { fn f(self: Self, v: u64) -> u64 }
               impl T for Q { fn f(self: Self, v: Q) -> u64 { 0u64 } }
               fn main() -> u64 { 0u64 }"#,
            vec!["parameter #2 type mismatch", "expected u64", "found Q"],
        ),
        (
            "duplicate struct field",
            r#"struct P { x: i64, x: u64 }
               fn main() -> u64 { 0u64 }"#,
            vec!["Duplicate field 'x'", "struct 'P'"],
        ),
        (
            "array elements of different types",
            r#"struct P { x: i64 }
               fn main() -> u64 {
                 val a = [P { x: 1i64 }, 2u64]
                 0u64 }"#,
            vec!["has type u64", "first element has type P"],
        ),
        (
            "cast from a struct",
            r#"struct P { x: i64 }
               fn main() -> u64 {
                 val p = P { x: 1i64 }
                 val q = p as u64
                 0u64 }"#,
            vec!["Cannot cast P to u64"],
        ),
        (
            "index into a struct with no __getitem__",
            r#"struct P { x: i64 }
               fn main() -> u64 {
                 val p = P { x: 1i64 }
                 val v = p[0u64]
                 0u64 }"#,
            vec!["Cannot index into type P"],
        ),
        (
            "`?` on something that is neither Result nor Option",
            r#"struct P { x: i64 }
               fn main() -> u64 {
                 val p = P { x: 1i64 }
                 val v = p?
                 0u64 }"#,
            vec!["`?` requires Result<T, E> or Option<T>, got P"],
        ),
        (
            "match on a type that cannot be a scrutinee",
            r#"fn main() -> u64 {
                 val f = 1.5f64
                 match f { _ => 0u64, } }"#,
            vec!["match scrutinee must be", "got f64"],
        ),
        (
            "match arms of different types",
            r#"struct P { x: i64 }
               fn main() -> u64 {
                 val c = 1u64
                 val r = match c { 0u64 => P { x: 1i64 }, _ => 5u64, }
                 0u64 }"#,
            vec!["arm 0 is P", "arm 1 is u64"],
        ),
        (
            "struct pattern against a primitive",
            r#"struct P { x: i64 }
               fn main() -> u64 {
                 val c = 1u64
                 match c { P { x } => 0u64, _ => 1u64, } }"#,
            vec!["struct pattern requires a struct value, got u64"],
        ),
        (
            "`with allocator` on a non-allocator",
            r#"fn main() -> u64 { with allocator = 1u64 { 0u64 } }"#,
            vec!["requires an Allocator value, but got u64"],
        ),
        (
            "closure body of the wrong type",
            r#"struct P { x: i64 }
               fn main() -> u64 {
                 val f = fn() -> u64 { P { x: 1i64 } }
                 0u64 }"#,
            vec!["closure body returns P", "declared return type is u64"],
        ),
        (
            "trait bound violated at the call site",
            r#"struct P { x: i64 }
               fn main() -> u64 {
                 var v: Vec<P> = Vec::new()
                 v.sort()
                 0u64 }"#,
            vec!["bound violation", "expected Ord", "got P"],
        ),
        (
            "a code point too wide for the annotated type",
            r#"fn main() -> u64 {
                 val b: u8 = '\u{1F600}'
                 b as u64 }"#,
            vec!["128512", "u8"],
        ),
    ]
}

#[test]
fn diagnostics_spell_names_and_types_as_they_were_written() {
    for (what, source, expected) in cases() {
        let rendered = diagnostics_for(source);
        for want in expected {
            assert!(
                rendered.contains(want),
                "{what}: diagnostic does not mention {want:?}\n--- got ---\n{rendered}"
            );
        }
    }
}

#[test]
fn no_diagnostic_shows_an_interned_id_or_a_debug_type_name() {
    for (what, source, _) in cases() {
        let rendered = diagnostics_for(source);
        for leak in DEBUG_LEAKS {
            assert!(
                !rendered.contains(leak),
                "{what}: diagnostic leaks the Debug spelling {leak:?} — \
                 use `resolve_symbol_name` / `type_name_for_error`\n\
                 --- got ---\n{rendered}"
            );
        }
    }
}

/// A reserved word written where a name goes says which word, and
/// where it belongs.
///
/// Three positions reported it three ways, none of them useful:
/// `fn f(to: u64)` said `ParenClose` (the real error was swallowed by
/// the parameter loop's recovery), `val to = 3u64` said "reserved
/// keyword 'keyword'" — the catch-all arm of a hand-written match —
/// and a struct field said "expected field name" without saying why
/// the name was refused.
#[test]
fn a_keyword_used_as_a_name_says_which_keyword() {
    let cases = [
        ("fn f(to: u64) -> u64 { 1u64 }", "a parameter name"),
        ("fn main() -> u64 { val to = 3u64  1u64 }", "the name of a binding"),
        ("struct S { to: u64 }\nfn main() -> u64 { 0u64 }", "a field name"),
    ];
    for (source, position) in cases {
        let err = test_program(source).expect_err("a keyword is not a name");
        assert!(
            err.contains("`to` is a keyword"),
            "the word should be named ({position}): {err}"
        );
        assert!(
            err.contains(position),
            "and the position said plainly: {err}"
        );
        assert!(
            err.contains("range keyword"),
            "`to` has an obvious neighbour, so the message offers it: {err}"
        );
        assert!(
            !err.contains("ParserError {") && !err.contains("GenericError {"),
            "and no struct dump reaches the reader: {err}"
        );
    }
}
