// Lexical errors must be reported as diagnostics that point at the
// offending literal — not swallowed (the old behaviour: the lexer's
// `Error::Unmatch` became "end of input", the literal vanished from
// the token stream, and the damage surfaced as an unrelated error on
// a different line) and not as the parse cascade around them.
//
// The location assertions are the point: "the error names the line of
// the bad literal" is the property that was broken.

mod common;

use common::test_program;
use frontend::parser::error::ParserError;

/// Parse `source` and return every reported parse error, in source
/// order. Panics when the parse succeeds — these tests are about
/// failures.
fn parse_errors(source: &str) -> Vec<ParserError> {
    let mut session = compiler_core::CompilerSession::new();
    match session.parse_program_all_errors(source, "test.t") {
        Ok(_) => panic!("expected the parse to fail:\n{source}"),
        Err(errors) => errors,
    }
}

/// The one reported error, asserted to sit at `(line, column)`.
fn single_lex_error(source: &str, line: u32, column: u32) -> ParserError {
    let errors = parse_errors(source);
    assert_eq!(errors.len(), 1, "expected exactly one error:\n{errors:?}");
    let error = errors.into_iter().next().unwrap();
    assert_eq!(
        (error.location.line, error.location.column),
        (line, column),
        "the error must point at the literal, not the collateral damage"
    );
    error
}

#[test]
fn a_non_ascii_hex_escape_points_at_the_literal_line() {
    // The motivating case: the lex error used to be swallowed, the
    // literal dropped out of the token stream, and the type checker
    // reported the *next* statement (`__builtin_str_len(s)` with `s`
    // bound to a different type) as a mismatch on the wrong line.
    let error = single_lex_error(
        "fn main() -> u64 {\n    val s: str = \"bad \\x80 byte\"\n    __builtin_str_len(s)\n}",
        2,
        18,
    );
    let text = error.to_string();
    assert!(text.contains("\\x80"), "the escape should be named: {text}");
    assert!(text.contains("UTF-8"), "the fix should be named: {text}");
}

#[test]
fn an_unknown_escape_points_at_the_literal() {
    let error = single_lex_error(
        "fn main() -> u64 {\n    val s: str = \"bad \\q byte\"\n    0u64\n}",
        2,
        18,
    );
    assert!(error.to_string().contains("unknown escape"), "{error:?}");
}

#[test]
fn a_truncated_hex_escape_reports_the_lex_error_not_the_cascade() {
    // The literal is skipped, so the parse also trips over the now
    // empty `val s =` — but the lex error is the one reported, at the
    // literal's own position.
    let error = single_lex_error(
        "fn main() -> u64 {\n    val s = \"hex \\x\"\n    s\n}",
        2,
        13,
    );
    assert!(error.to_string().contains("E0012"), "{error:?}");
    assert!(error.to_string().contains("hex"), "{error:?}");
}

#[test]
fn an_unterminated_string_is_named_as_such() {
    // The literal runs to end of input, so the parse also trips on
    // the now-missing `}` — that cascade is on a different line and
    // stays. The lex error is the one that points at the string.
    let errors = parse_errors(
        "fn main() -> u64 {\n    val s = \"abc\n    0u64\n}",
    );
    let lex = errors
        .iter()
        .find(|e| e.to_string().contains("E0012"))
        .expect("the lex error must be reported");
    assert_eq!((lex.location.line, lex.location.column), (2, 13), "{errors:?}");
    assert!(
        lex.to_string().contains("unterminated string"),
        "{lex:?}"
    );
}

#[test]
fn an_unterminated_interpolation_is_named_as_such() {
    let error = single_lex_error(
        "fn main() -> u64 {\n    val s = \"a {b\"\n    0u64\n}",
        2,
        13,
    );
    assert!(
        error.to_string().contains("interpolation"),
        "{error:?}"
    );
}

#[test]
fn an_unmatched_character_is_reported_where_it_sits() {
    // `$` matches no rule at all. Before the fix the token stream
    // stopped there — everything after it silently vanished.
    let error = single_lex_error(
        "fn main() -> u64 {\n    val x = 1u64 + $ 2u64\n    x\n}",
        2,
        20,
    );
    assert!(error.to_string().contains("unrecognized character"), "{error:?}");
}

#[test]
fn digits_followed_by_letters_is_a_lex_error_at_the_token() {
    let error = single_lex_error(
        "fn main() -> u64 {\n    val x = 123abc\n    0u64\n}",
        2,
        13,
    );
    assert!(error.to_string().contains("invalid number"), "{error:?}");
}

#[test]
fn the_reported_error_carries_the_e0012_code() {
    // The code is what `--explain E0012` answers to; without it the
    // diagnostic is a dead end for a tool that reads codes.
    let error = single_lex_error(
        "fn main() -> u64 {\n    val s = \"\\x80\"\n    0u64\n}",
        2,
        13,
    );
    assert!(error.to_string().contains("E0012"), "{error:?}");
}

#[test]
fn the_old_misleading_type_error_is_gone() {
    // End-to-end through the single-error path (`test_program` →
    // `parse_program`): the failure must be the lex error, not "Type
    // mismatch: expected String, but got UInt64" on the next line.
    let err = test_program(
        "fn main() -> u64 {\n    val s: str = \"bad \\x80 byte\"\n    __builtin_str_len(s)\n}",
    )
    .unwrap_err();
    assert!(err.contains("UTF-8"), "expected the lex error:\n{err}");
    assert!(!err.contains("UInt64"), "old misleading type error:\n{err}");
}

#[test]
fn lex_errors_reach_the_json_diagnostics_path() {
    let mut options = interpreter::RunOptions::default();
    options.diagnostics_json = true;
    let (result, stderr) = interpreter::output::with_stderr_capture(|| {
        interpreter::run_source(
            "fn main() -> u64 {\n    val s = \"bad \\q\"\n    s\n}",
            "test.t",
            &options,
        )
    });
    assert!(result.is_err(), "the parse must fail");
    assert!(
        stderr.contains("\"code\": \"E0012\""),
        "JSON diagnostics must carry the lex code:\n{stderr}"
    );
    assert!(
        stderr.contains("\"line\": 2"),
        "JSON diagnostics must point at the literal's line:\n{stderr}"
    );
    assert!(
        stderr.contains("unknown escape"),
        "JSON diagnostics must carry the message:\n{stderr}"
    );
}
