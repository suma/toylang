// LLM-LOOP P2 — every diagnostic carries a position, and that position
// is one the reader can trust.
//
// Three separate failures were in play before P2:
//   1. the message body was prefixed with `line:column:offset:`, which
//      duplicated the header and leaked a byte offset
//   2. the caret was guessed by string-matching the message against the
//      source line, so it landed on the wrong token or a fixed `^^`
//   3. plenty of diagnostics carried no location at all, and errors
//      raised inside an imported module carried one that pointed into a
//      *different file* while being rendered against this one

mod common;

use common::test_program;

fn diagnostics(source: &str) -> String {
    match test_program(source) {
        Ok(_) => panic!("expected the program to fail type checking:\n{source}"),
        Err(e) => e,
    }
}

/// Every reported diagnostic must carry a file:line:column header.
fn assert_all_located(diags: &str) {
    assert!(
        !diags.contains("Error: ["),
        "a diagnostic was reported with no location:\n{diags}"
    );
    assert!(diags.contains("Error at"), "no diagnostics at all:\n{diags}");
}

#[test]
fn message_body_carries_no_internal_coordinates() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val a: bool = 1u64
            0u64
        }",
    );
    // The header is `test.t:2:27:`; the *message* must not repeat a
    // position, and must never contain a raw byte offset.
    let message = diags
        .split("Type mismatch")
        .nth(1)
        .expect("expected a type mismatch diagnostic");
    assert!(
        !message.contains("offset"),
        "byte offset leaked into the message:\n{diags}"
    );
    // `2:27:41:` style prefixes: three colon-separated numbers in a row.
    assert!(
        !diags.contains(":41:") && !diags.contains(":40:"),
        "internal coordinates still in the message:\n{diags}"
    );
}

#[test]
fn caret_width_matches_the_offending_token() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val a: bool = 1u64
            0u64
        }",
    );
    // `1u64` is four characters, so four carets -- not the old fixed
    // two, and not the whole line.
    assert!(
        diags.contains("^^^^ [E0001] Type mismatch"),
        "expected a 4-wide caret over `1u64`:\n{diags}"
    );
}

#[test]
fn undefined_call_is_anchored_at_the_callee_name() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val z = not_a_function(1u64)
            0u64
        }",
    );
    // 14 characters of `not_a_function`, and the column must be the
    // start of the name rather than the `(` that follows it.
    assert!(
        diags.contains("^^^^^^^^^^^^^^ [E0003] Function 'not_a_function' not found"),
        "caret should cover the callee name:\n{diags}"
    );
}

#[test]
fn argument_mismatch_is_anchored_at_the_argument() {
    let diags = diagnostics(
        "fn takes_i64(a: i64) -> i64 { a }
        fn main() -> u64 {
            val c = takes_i64(1u64)
            0u64
        }",
    );
    // The offending value is the argument, not the callee: pointing at
    // `takes_i64` would not say which argument to change.
    assert!(
        diags.contains("^^^^ [E0010] Type error"),
        "caret should cover the argument `1u64`:\n{diags}"
    );
}

#[test]
fn binding_mismatch_is_anchored_at_the_initializer() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val flag: bool = 7u64
            0u64
        }",
    );
    assert!(
        diags.contains("^^^^ [E0001] Type mismatch"),
        "caret should cover the initializer `7u64`:\n{diags}"
    );
}

// --- location coverage ------------------------------------------------
//
// One test per diagnostic shape that used to escape with no position.

#[test]
fn bare_call_statement_error_is_located() {
    assert_all_located(&diagnostics("fn main() -> u64 { no_such_function() }"));
}

#[test]
fn impl_method_return_mismatch_is_located() {
    assert_all_located(&diagnostics(
        "struct P { x: u64 }
        impl P {
            fn get(self: Self) -> bool { self.x }
        }
        fn main() -> u64 { 0u64 }",
    ));
}

#[test]
fn array_index_type_error_is_located() {
    assert_all_located(&diagnostics(
        "fn main() -> u64 {
            val a = [1u64, 2u64]
            a[true]
        }",
    ));
}

#[test]
fn non_exhaustive_match_is_located() {
    assert_all_located(&diagnostics(
        "enum E { A, B }
        fn main() -> u64 {
            val e = E::A
            match e { E::A => 0u64 }
        }",
    ));
}

#[test]
fn non_bool_requires_clause_is_located() {
    assert_all_located(&diagnostics(
        "fn g(n: u64) -> u64
            requires n
        { n }
        fn main() -> u64 { g(1u64) }",
    ));
}

#[test]
fn while_condition_type_error_is_located() {
    assert_all_located(&diagnostics(
        "fn main() -> u64 {
            while 1u64 { }
            0u64
        }",
    ));
}

#[test]
fn struct_literal_field_mismatch_is_located() {
    assert_all_located(&diagnostics(
        "struct P { x: u64 }
        fn main() -> u64 {
            val p = P { x: true }
            0u64
        }",
    ));
}
