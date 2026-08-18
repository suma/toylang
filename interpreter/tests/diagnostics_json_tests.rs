// LLM-LOOP P3 — structured diagnostics and applicable fixes.
//
// The claims worth pinning are not "the JSON has these keys" but the
// two properties a consumer actually depends on:
//
//   * a span resolves, byte for byte, to the source it blames
//   * applying a `machine-applicable` suggestion produces a program
//     that compiles
//
// The second one is what makes suggestions worth emitting at all. A
// suggestion that does not compile costs an agent a round trip *and*
// its trust in every later suggestion, so these tests apply the fixes
// and re-run rather than just inspecting the text.


use crate::common::{core_modules_dir, test_program};
use frontend::diagnostic::{Applicability, Diagnostic};

/// Type check `source` and return the structured diagnostics.
fn diagnose(source: &str) -> Vec<Diagnostic> {
    let mut parser = frontend::ParserWithInterner::new(source);
    parser.set_source_file("test.t");
    let mut program = parser.parse_program().expect("parse");
    let string_interner = parser.get_string_interner();
    let core = core_modules_dir();
    match interpreter::check_typing_diagnostics(
        &mut program,
        string_interner,
        Some(source),
        Some("test.t"),
        Some(core.as_path()),
    ) {
        Ok(()) => panic!("expected the program to fail type checking:\n{source}"),
        Err(diagnostics) => diagnostics,
    }
}

/// Text the diagnostic's span covers.
fn span_text<'a>(source: &'a str, diagnostic: &Diagnostic) -> &'a str {
    let span = diagnostic.span.expect("diagnostic should carry a span");
    &source[span.offset as usize..span.end_offset as usize]
}

/// Apply every machine-applicable suggestion, last span first so
/// earlier offsets stay valid.
fn apply_suggestions(source: &str, diagnostics: &[Diagnostic]) -> String {
    let mut edits: Vec<(usize, usize, &str)> = Vec::new();
    for d in diagnostics {
        for s in &d.suggestions {
            if s.applicability != Applicability::MachineApplicable {
                continue;
            }
            let span = s.effective_span(d.span).expect("suggestion needs a span");
            edits.push((span.offset as usize, span.end_offset as usize, &s.replacement));
        }
    }
    edits.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut out = source.to_string();
    for (start, end, replacement) in edits {
        out.replace_range(start..end, replacement);
    }
    out
}

#[test]
fn every_diagnostic_carries_a_stable_code() {
    let diagnostics = diagnose(
        "fn main() -> u64 {
            val a: bool = 1u64
            0u64
        }",
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "E0001");
    assert_eq!(diagnostics[0].severity.as_str(), "error");
}

#[test]
fn span_resolves_to_the_text_it_blames() {
    let source = "fn main() -> u64 {
            val a: bool = 1u64
            0u64
        }";
    let diagnostics = diagnose(source);
    assert_eq!(span_text(source, &diagnostics[0]), "1u64");
}

#[test]
fn undefined_call_span_covers_only_the_callee_name() {
    let source = "fn main() -> u64 {
            val z = no_such_thing(1u64)
            0u64
        }";
    let diagnostics = diagnose(source);
    assert_eq!(span_text(source, &diagnostics[0]), "no_such_thing");
}

#[test]
fn every_diagnostic_of_a_multi_error_program_is_structured() {
    let source = "fn helper(a: i64) -> i64 { a }
        fn main() -> u64 {
            val a: bool = 1u64
            val b = missing_fn(2u64)
            val c = helper(1u64)
            0u64
        }";
    let diagnostics = diagnose(source);
    assert_eq!(diagnostics.len(), 3, "{diagnostics:#?}");
    for d in &diagnostics {
        assert!(d.span.is_some(), "diagnostic without a span: {d:#?}");
        assert!(!d.code.is_empty());
    }
    // Reported in source order, same as the text form.
    let lines: Vec<u32> = diagnostics.iter().map(|d| d.span.unwrap().line).collect();
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted, "diagnostics out of source order");
}

// --- suggestions ------------------------------------------------------

#[test]
fn numeric_argument_mismatch_suggests_a_cast_that_compiles() {
    let source = "fn helper(a: i64) -> i64 { a }
fn main() -> u64 {
    val c = helper(1u64)
    c as u64
}";
    let diagnostics = diagnose(source);
    assert_eq!(diagnostics.len(), 1);
    let suggestion = diagnostics[0]
        .suggestions
        .first()
        .unwrap_or_else(|| panic!("expected a cast suggestion:\n{:#?}", diagnostics[0]));
    assert_eq!(suggestion.applicability, Applicability::MachineApplicable);
    assert_eq!(suggestion.replacement, "1u64 as i64");

    // The point of `machine-applicable`: applying it resolves the error.
    let fixed = apply_suggestions(source, &diagnostics);
    test_program(&fixed).unwrap_or_else(|e| panic!("suggested fix did not compile: {e}\n{fixed}"));
}

#[test]
fn a_cast_suggestion_is_either_correct_or_absent_whatever_the_argument_looks_like() {
    // The literal case above passed while every other argument shape was
    // broken, because a literal is the one expression whose recorded
    // location happens to be its full extent. An identifier was located
    // at the *next* token, so the edit came out as `f(a) as i64` — which
    // compiles, resolves nothing, and was advertised as
    // machine-applicable. A binary operand was located at its operator,
    // so `a + b` produced `+ as i64`.
    //
    // So the property is per-shape: whatever suggestion is offered must
    // resolve the diagnostic, and a shape we cannot quote correctly must
    // offer none at all. Silence is free; a wrong edit is not.
    // `expect_fix` is asserted, not merely observed: a shape silently
    // losing its suggestion is a regression too, and "no suggestion"
    // otherwise passes this test trivially.
    let cases = [
        ("identifier", true, "fn f(n: i64) -> i64 { n }\nfn main() -> u64 {\n    val a: u64 = 1u64\n    val x = f(a)\n    0u64\n}"),
        ("binary", true, "fn f(n: i64) -> i64 { n }\nfn main() -> u64 {\n    val a: u64 = 1u64\n    val x = f(a + 2u64)\n    0u64\n}"),
        ("nested binary", true, "fn f(n: i64) -> i64 { n }\nfn main() -> u64 {\n    val a: u64 = 1u64\n    val x = f(a * 2u64 + 3u64)\n    0u64\n}"),
        ("second argument", true, "fn f(n: i64, m: i64) -> i64 { n }\nfn main() -> u64 {\n    val a: u64 = 1u64\n    val x = f(1i64, a)\n    0u64\n}"),
        ("field access", true, "struct P { x: u64 }\nfn f(n: i64) -> i64 { n }\nfn main() -> u64 {\n    val p = P { x: 1u64 }\n    val y = f(p.x)\n    0u64\n}"),
        ("tuple access", true, "fn f(n: i64) -> i64 { n }\nfn main() -> u64 {\n    val t = (1u64, 2u64)\n    val y = f(t.0)\n    0u64\n}"),
        ("index", true, "fn f(n: i64) -> i64 { n }\nfn main() -> u64 {\n    val a: [u64; 2] = [1u64, 2u64]\n    val y = f(a[0])\n    0u64\n}"),
        ("method call", true, "fn f(n: u64) -> u64 { n }\nfn main() -> u64 {\n    val n: i64 = -3i64\n    val y = f(n.abs())\n    0u64\n}"),
        ("cast", true, "fn f(n: i64) -> i64 { n }\nfn main() -> u64 {\n    val a: f64 = 1.0f64\n    val x = f(a as u64)\n    0u64\n}"),
        ("unary", true, "fn f(n: u64) -> u64 { n }\nfn main() -> u64 {\n    val a: i64 = 1i64\n    val y = f(-a)\n    0u64\n}"),
        // A call is located at its callee so that "function not found"
        // points at the name (P2). That span cannot be quoted as a
        // value -- `g as i64()` -- so no suggestion is offered.
        ("call result", false, "fn g() -> u64 { 3u64 }\nfn f(n: i64) -> i64 { n }\nfn main() -> u64 {\n    val x = f(g())\n    0u64\n}"),
    ];
    for (shape, expect_fix, source) in cases {
        let diagnostics = diagnose(source);
        assert!(!diagnostics.is_empty(), "{shape}: expected a diagnostic");
        let has_fix = diagnostics.iter().any(|d| !d.suggestions.is_empty());
        assert_eq!(
            has_fix, expect_fix,
            "{shape}: expected a suggestion? {expect_fix}, got {has_fix}:\n{diagnostics:#?}"
        );
        if !has_fix {
            continue;
        }
        let fixed = apply_suggestions(source, &diagnostics);
        test_program(&fixed).unwrap_or_else(|e| {
            panic!("{shape}: the suggested fix does not compile: {e}\n{fixed}")
        });
        // Compiling is necessary but not sufficient — `f(a) as i64`
        // compiled too. The diagnostic itself has to be gone.
        assert!(
            diagnose_ok(&fixed),
            "{shape}: the suggested fix compiles but the diagnostic remains:\n{fixed}"
        );
    }
}

/// Whether `source` type checks cleanly.
fn diagnose_ok(source: &str) -> bool {
    let mut parser = frontend::ParserWithInterner::new(source);
    parser.set_source_file("test.t");
    let Ok(mut program) = parser.parse_program() else {
        return false;
    };
    let string_interner = parser.get_string_interner();
    let core = core_modules_dir();
    interpreter::check_typing_diagnostics(
        &mut program,
        string_interner,
        Some(source),
        Some("test.t"),
        Some(core.as_path()),
    )
    .is_ok()
}

#[test]
fn binding_annotation_mismatch_suggests_a_cast_that_compiles() {
    // `u64` into an `i64` slot is accepted, so the mismatch has to be
    // one the checker actually rejects: integer into `f64`.
    let source = "fn main() -> u64 {
    val d: f64 = 5u64
    d as u64
}";
    let diagnostics = diagnose(source);
    let suggestion = diagnostics[0]
        .suggestions
        .first()
        .unwrap_or_else(|| panic!("expected a cast suggestion:\n{:#?}", diagnostics[0]));
    assert_eq!(suggestion.replacement, "5u64 as f64");

    let fixed = apply_suggestions(source, &diagnostics);
    test_program(&fixed).unwrap_or_else(|e| panic!("suggested fix did not compile: {e}\n{fixed}"));
}

#[test]
fn misspelled_function_suggests_the_real_name_and_compiles() {
    let source = "fn calculate_total(a: u64) -> u64 { a }
fn main() -> u64 { calculate_totl(3u64) }";
    let diagnostics = diagnose(source);
    let suggestion = diagnostics[0]
        .suggestions
        .first()
        .unwrap_or_else(|| panic!("expected a did-you-mean suggestion:\n{:#?}", diagnostics[0]));
    assert_eq!(suggestion.replacement, "calculate_total");
    // Built before the error was anchored, so it inherits the primary span.
    assert!(suggestion.span.is_none());

    let fixed = apply_suggestions(source, &diagnostics);
    test_program(&fixed).unwrap_or_else(|e| panic!("suggested fix did not compile: {e}\n{fixed}"));
}

#[test]
fn no_cast_is_suggested_when_no_cast_would_help() {
    // `u64` to `bool` is not an `as` cast, so offering one would send
    // the reader to a second error rather than a fix.
    let diagnostics = diagnose(
        "fn main() -> u64 {
            val a: bool = 1u64
            0u64
        }",
    );
    assert!(
        diagnostics[0].suggestions.is_empty(),
        "suggested an inapplicable cast: {:#?}",
        diagnostics[0]
    );
}

#[test]
fn an_unrecognisable_name_gets_no_suggestion() {
    let diagnostics = diagnose(
        "fn calculate_total(a: u64) -> u64 { a }
        fn main() -> u64 { zzzzzzzzzz(3u64) }",
    );
    assert!(
        diagnostics[0].suggestions.is_empty(),
        "guessed at an unrelated name: {:#?}",
        diagnostics[0]
    );
}

#[test]
fn diagnostics_serialise_to_json() {
    let diagnostics = diagnose(
        "fn helper(a: i64) -> i64 { a }
        fn main() -> u64 {
            val c = helper(1u64)
            0u64
        }",
    );
    let json = serde_json::to_string(&diagnostics).expect("serialise");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("round trip");
    let first = &parsed[0];
    assert_eq!(first["severity"], "error");
    assert_eq!(first["code"], "E0001");
    assert_eq!(first["file"], "test.t");
    assert_eq!(first["suggestions"][0]["applicability"], "machine-applicable");
    assert_eq!(first["suggestions"][0]["replacement"], "1u64 as i64");
}

#[test]
fn a_module_flagged_diagnostic_is_never_rendered_against_the_local_file() {
    // A span from another file must never be presented as if it indexed
    // the file being compiled -- that is the failure mode P2 fixed, and
    // it has to survive the P3 rendering path too.
    use frontend::diagnostic::{Severity, Span};
    use interpreter::error_formatter::ErrorFormatter;

    let local_source = "fn main() -> u64 {\n    0u64\n}\n";
    let diagnostic = Diagnostic {
        severity: Severity::Error,
        code: "E0001",
        message: "Type mismatch: expected Bool, but got UInt64".to_string(),
        file: "main.t".to_string(),
        // Offsets into *another* file that happen to be valid here.
        span: Some(Span { line: 2, column: 5, offset: 22, end_offset: 26 }),
        origin_module: Some("helper".to_string()),
        suggestions: Vec::new(),
    };

    let rendered = ErrorFormatter::new(local_source, "main.t").format_diagnostic(&diagnostic);
    assert!(rendered.contains("helper"), "{rendered}");
    assert!(
        !rendered.contains("0u64"),
        "quoted a line from the local file for a foreign span:\n{rendered}"
    );
}
