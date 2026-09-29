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
        std::slice::from_ref(&core),
    ) {
        Ok(_) => panic!("expected the program to fail type checking:\n{source}"),
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
            for edit in &s.edits {
                let span = edit.span.or(d.span).expect("suggestion needs a span");
                edits.push((span.offset as usize, span.end_offset as usize, &edit.replacement));
            }
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
    assert_eq!(suggestion.replacement().unwrap(), "1u64 as i64");

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
        std::slice::from_ref(&core),
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
    assert_eq!(suggestion.replacement().unwrap(), "5u64 as f64");

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
    assert_eq!(suggestion.replacement().unwrap(), "calculate_total");
    // Built before the error was anchored; resolution fills in the
    // primary span and the file, so a consumer never sees `None`.
    let edit = &suggestion.edits[0];
    assert_eq!(edit.span, diagnostics[0].span);
    assert_eq!(edit.file.as_deref(), Some("test.t"));

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
        span: Some(Span {
            file: frontend::source_map::FileId::ENTRY,
            line: 2,
            column: 5,
            offset: 22,
            end_offset: 26,
        }),
        origin_module: Some("helper".to_string()),
        suggestions: Vec::new(),
        related: Vec::new(),
        span_word: None,
        backtrace: Vec::new(),
    };

    let rendered = ErrorFormatter::new(local_source, "main.t").format_diagnostic(&diagnostic);
    assert!(rendered.contains("helper"), "{rendered}");
    assert!(
        !rendered.contains("0u64"),
        "quoted a line from the local file for a foreign span:\n{rendered}"
    );
}

// --- DEBUG-OBS D5: runtime failures on the JSON channel ------------
//
// 実測 8: `--format=json` covered parse and type-check failures
// only. The one thing left in plain text was the failure that happens
// while the program runs — the one an LLM loop reads most often.

/// Run a program expected to fail at runtime with JSON diagnostics on,
/// and return what landed on stderr.
fn runtime_json(source: &str) -> serde_json::Value {
    let core = crate::common::core_modules_dir();
    let mut options = interpreter::RunOptions::default();
    options.diagnostics_json = true;
    options.core_modules_dirs = std::slice::from_ref(&core);
    let (result, stderr) = interpreter::output::with_stderr_capture(|| {
        interpreter::run_source(source, "test.t", &options)
    });
    assert!(result.is_err(), "expected a runtime failure");
    serde_json::from_str(&stderr).unwrap_or_else(|e| panic!("not JSON ({e}):\n{stderr}"))
}

#[test]
fn a_panic_is_reported_as_json() {
    let value = runtime_json(
        "fn inner() -> u64 { panic(\"deep\") }
fn main() -> u64 { inner() }",
    );
    let d = &value[0];
    assert_eq!(d["severity"], "error");
    assert_eq!(d["code"], "E0019");
    assert_eq!(d["message"], "panic: deep");
    assert_eq!(d["file"], "test.t");
    assert_eq!(d["span"]["line"], 1);
    // The backtrace is data, not a string a consumer has to re-parse.
    assert_eq!(d["backtrace"][0]["function"], "inner");
    assert_eq!(d["backtrace"][0]["line"], 2);
    assert_eq!(d["backtrace"][1]["function"], "main");
    assert!(
        d["backtrace"][1].get("line").is_none(),
        "the entry frame was not called from anywhere: {d}"
    );
}

#[test]
fn a_contract_violation_gets_its_own_code() {
    let value = runtime_json(
        "fn half(n: u64) -> u64
    requires n % 2u64 == 0u64
{
    n / 2u64
}
fn main() -> u64 { half(3u64) }",
    );
    let d = &value[0];
    assert_eq!(d["code"], "E0020", "{d}");
    assert!(
        d["message"].as_str().unwrap_or("").contains("with n = 3"),
        "{d}"
    );
}

#[test]
fn a_failure_inside_the_stdlib_names_the_stdlib_file() {
    // DEBUG-OBS D2 put the file on the position; this is where a tool
    // reads it. `file` is the *failure's* file, not the entry one.
    let value = runtime_json(
        "fn main() -> u64 {
    val o: Option<u64> = Option::None
    val v: u64 = o.unwrap()
    v
}",
    );
    let d = &value[0];
    assert_eq!(d["file"], "core/std/option.t", "{d}");
    assert_eq!(d["backtrace"][0]["function"], "Option::unwrap", "{d}");
}

#[test]
fn a_type_error_in_a_module_names_the_module_file_and_line() {
    // DEBUG-OBS D2, the type-check half of
    // `a_failure_inside_the_stdlib_names_the_stdlib_file`.
    //
    // The line used to be recomputed from the offset against the
    // *entry* file's text, so an error on line 5 of a module was
    // reported as line 3 — and the number moved when the entry file
    // was edited, which is the tell that it was never the module's
    // line at all.
    let dir = std::env::temp_dir().join(format!(
        "toy-modline-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("temp module dir");
    let module = dir.join("modline.t");
    // The error is on line 5. The leading blank lines are the point:
    // they make the module's numbering differ from any other file's.
    std::fs::write(
        &module,
        "\n\n\npub fn modline_boom(n: u64) -> u64 {\n    val bad: u64 = \"nope\"\n    n\n}\n",
    )
    .expect("write module");

    let mut options = interpreter::RunOptions::default();
    options.diagnostics_json = true;
    let roots = [dir.clone()];
    options.core_modules_dirs = &roots;
    let (result, stderr) = interpreter::output::with_stderr_capture(|| {
        interpreter::run_source(
            "fn main() -> u64 { modline::modline_boom(1u64) }",
            "test.t",
            &options,
        )
    });
    let _ = std::fs::remove_dir_all(&dir);
    assert!(result.is_err(), "expected the module to fail type checking");

    let value: serde_json::Value = serde_json::from_str(&stderr)
        .unwrap_or_else(|e| panic!("not JSON ({e}):\n{stderr}"));
    let d = &value[0];
    assert_eq!(d["code"], "E0001", "{d}");
    assert!(
        d["file"].as_str().unwrap_or("").ends_with("modline.t"),
        "the diagnostic should name the module's own file: {d}"
    );
    assert_eq!(d["span"]["line"], 5, "{d}");
}

#[test]
fn a_parallel_loop_that_prints_is_refused() {
    // CONCURRENCY A1: interleaved output is not the same output, and
    // a lane that ran the iterations at once would stop agreeing
    // with one that did not.
    let core = crate::common::core_modules_dir();
    let mut options = interpreter::RunOptions::default();
    options.diagnostics_json = true;
    options.core_modules_dirs = std::slice::from_ref(&core);
    let (result, stderr) = interpreter::output::with_stderr_capture(|| {
        interpreter::run_source(
            "fn main() -> u64 {\n    parallel for i in 0u64..4u64 {\n        println(i)\n    }\n    0u64\n}",
            "test.t",
            &options,
        )
    });
    assert!(result.is_err(), "printing inside a parallel body should be refused");
    let value: serde_json::Value = serde_json::from_str(&stderr)
        .unwrap_or_else(|e| panic!("not JSON ({e}):\n{stderr}"));
    let d = &value[0];
    assert_eq!(d["code"], "E0029", "{d}");
    // The caret sits on the modifier, not past the closing brace:
    // a `Stmt::For` records where the parser finished.
    assert_eq!(d["span"]["line"], 2, "{d}");
}

/// The three shapes A2-b-2 had to add to E0029, each refused where
/// every lane can see it.
///
/// They are order dependencies, not lowering limits: an accumulator
/// reads what the previous iteration wrote, and `break` / `return`
/// stop iterations that may already be running. Refusing them in the
/// type checker is what keeps the tree-walker from happily running a
/// program the compiled lanes cannot.
#[test]
fn a_parallel_loop_that_depends_on_order_is_refused() {
    let core = crate::common::core_modules_dir();
    let cases = [
        (
            "an accumulator",
            "fn main() -> u64 {\n    var acc = 0u64\n    parallel for i in 0u64..4u64 {\n        acc = acc + i\n    }\n    println(acc)\n    0u64\n}",
            4,
        ),
        (
            "break",
            "fn main() -> u64 {\n    parallel for i in 0u64..4u64 {\n        if i > 2u64 { break }\n    }\n    0u64\n}",
            2,
        ),
        (
            "return",
            "fn main() -> u64 {\n    parallel for i in 0u64..4u64 {\n        if i > 2u64 { return 1u64 }\n    }\n    0u64\n}",
            3,
        ),
    ];
    for (what, src, line) in cases {
        let mut options = interpreter::RunOptions::default();
        options.diagnostics_json = true;
        options.core_modules_dirs = std::slice::from_ref(&core);
        let (result, stderr) = interpreter::output::with_stderr_capture(|| {
            interpreter::run_source(src, "test.t", &options)
        });
        assert!(result.is_err(), "{what} inside a parallel body should be refused");
        let value: serde_json::Value = serde_json::from_str(&stderr)
            .unwrap_or_else(|e| panic!("not JSON ({e}):\n{stderr}"));
        let d = &value[0];
        assert_eq!(d["code"], "E0029", "{what}: {d}");
        assert_eq!(d["span"]["line"], line, "{what}: {d}");
    }
}

/// ... and the one that is a lowering limit rather than an order
/// dependency: a `var` the body declares itself is per iteration, so
/// assigning to it is fine.
#[test]
fn a_parallel_body_may_assign_to_what_it_declares() {
    let core = crate::common::core_modules_dir();
    let mut options = interpreter::RunOptions::default();
    options.core_modules_dirs = std::slice::from_ref(&core);
    let result = interpreter::run_source(
        "fn main() -> u64 {\n    parallel for i in 0u64..4u64 {\n        var seen = i\n        seen = seen + 1u64\n    }\n    0u64\n}",
        "test.t",
        &options,
    );
    assert!(result.is_ok(), "a binding made inside the body is the body's own: {result:?}");
}

// LLM-TOOLING L0: a name declared twice in one file. A second `fn`
// used to pass the checker and panic in `compiler_ir`; a second
// `struct` / `const` silently replaced the first.

#[test]
fn a_second_declaration_of_a_name_is_e0031_on_the_name() {
    for (source, noun) in [
        ("fn f() -> u64 { 1u64 }\nfn f() -> u64 { 2u64 }\nfn main() -> u64 { f() }", "function"),
        ("struct S { a: u64 }\nstruct S { b: u64 }\nfn main() -> u64 { 0u64 }", "type"),
        ("struct S { a: u64 }\nenum S { B }\nfn main() -> u64 { 0u64 }", "type"),
        ("enum S { A }\nenum S { B }\nfn main() -> u64 { 0u64 }", "type"),
        ("trait S { fn m(&self) -> u64 }\ntrait S { fn n(&self) -> u64 }\nfn main() -> u64 { 0u64 }", "trait"),
        ("const S: u64 = 1u64\nconst S: u64 = 2u64\nfn main() -> u64 { S }", "constant"),
    ] {
        let diagnostics = diagnose(source);
        assert_eq!(diagnostics.len(), 1, "{source}\n{diagnostics:#?}");
        let d = &diagnostics[0];
        assert_eq!(d.code, "E0031", "{source}");
        assert_eq!(d.span.unwrap().line, 2, "{source}");
        let name = if noun == "function" { "f" } else { "S" };
        assert_eq!(span_text(source, d), name, "{source}");
        assert!(d.message.contains(noun) && d.message.contains("line 1"), "{}", d.message);
    }
}

#[test]
fn the_first_declaration_wins_and_the_rest_of_the_file_is_still_checked() {
    // `S { a: .. }` matches the first struct, so it is not a second
    // error; the unrelated mismatch in `main` is still reported.
    let source = "struct S { a: u64 }
fn g() -> u64 { 0u64 }
struct S { b: u64 }
fn main() -> u64 {
    val s = S { a: 1u64 }
    val x: bool = 1u64
    s.a
}";
    let diagnostics = diagnose(source);
    let codes: Vec<&str> = diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(codes, ["E0031", "E0001"], "{diagnostics:#?}");
}

#[test]
fn a_function_named_like_a_type_is_not_a_duplicate() {
    // Functions and types are different namespaces.
    let source = "struct P { a: u64 }
fn P() -> u64 { 0u64 }
fn main() -> u64 {
    val x: bool = 1u64
    0u64
}";
    let diagnostics = diagnose(source);
    assert!(diagnostics.iter().all(|d| d.code != "E0031"), "{diagnostics:#?}");
}

// LLM-TOOLING #1: every machine-applicable suggestion, applied, gives a
// program that checks. Each entry below has only fixable mistakes, so
// the property is "apply everything once, then no diagnostics at all".
// A new suggestion belongs here with the smallest program that shows it.

/// Parse, then type check. The diagnostics of whichever stage failed,
/// with their edits resolved; empty when the program is fine.
fn check_all(source: &str) -> Vec<Diagnostic> {
    let mut parser = frontend::ParserWithInterner::new(source);
    parser.set_source_file("test.t");
    let outcome = parser.parse_program_multiple_errors();
    if !outcome.errors.is_empty() {
        return outcome
            .errors
            .iter()
            .map(|e| {
                let mut d = Diagnostic::from_parser_error(e, "test.t");
                d.resolve_edits();
                d
            })
            .collect();
    }
    let mut program = outcome.result.expect("a program when there are no errors");
    let string_interner = parser.get_string_interner();
    let core = core_modules_dir();
    match interpreter::check_typing_diagnostics(
        &mut program,
        string_interner,
        Some(source),
        Some("test.t"),
        std::slice::from_ref(&core),
    ) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics,
    }
}

const FIXABLE: &[(&str, &str)] = &[
    (
        "else if -> elif",
        "fn f(a: u64) -> u64 {
    if a > 1u64 { 1u64 } else if a > 0u64 { 2u64 } else { 3u64 }
}
fn main() -> u64 { f(1u64) }",
    ),
    (
        "float literal suffix, several on one line",
        "fn f(a: u64) -> f64 {
    if a > 1u64 { 1.5 } else { 1_000.25 }
}
fn main() -> u64 { val x = f(1u64)
    0u64 }",
    ),
    (
        "float literal suffix follows an `f32` declaration",
        "const K: f32 = 2.5
fn main() -> u64 {
    val a: f32 = 1.5
    val b = 0.25
    0u64
}",
    ),
    (
        "unsafe fn, free function and method",
        "struct B { p: ptr }
impl B {
    fn first(&self) -> u64 { __builtin_ptr_read::<u64>(self.p, 0u64) }
}
pub never_allocates fn peek(p: ptr) -> u64 {
    __builtin_ptr_read::<u64>(p, 0u64)
}
fn main() -> u64 { 0u64 }",
    ),
    (
        "owning element: borrow, with and without an annotation",
        "fn main() -> u64 {
    var v: Vec<String> = Vec::new()
    v.push(String::from_str(\"a\"))
    val e: String = v.get(0u64)
    val g = v.get(0u64)
    e.len() + g.len()
}",
    ),
    (
        "misspelled field in a literal, a field access and a method",
        "struct Point { xpos: u64, ypos: u64 }
impl Point { fn magnitude(&self) -> u64 { self.xpos } }
fn main() -> u64 {
    val ypoz = 3u64
    val p = Point { xpos: ypoz, ypoz: 2u64 }
    val q = Point { xpos: 1u64, ypos: 2u64 }
    val a = q.xpoz
    val b = q.magnitdue()
    a + b
}",
    ),
    (
        "misspelled function and a numeric cast",
        "fn calculate_total(a: u64) -> u64 { a }
fn main() -> u64 {
    val n = calculate_totl(3u64)
    val c: u8 = n
    0u64
}",
    ),
];

#[test]
fn applying_every_machine_applicable_suggestion_gives_a_program_that_checks() {
    for (what, source) in FIXABLE {
        let mut current = source.to_string();
        // A fix can reveal a mistake the first one hid (a parse error
        // stops type checking), so apply until nothing is offered.
        for _round in 0..4 {
            let diagnostics = check_all(&current);
            if diagnostics.is_empty() {
                break;
            }
            let fixable = diagnostics.iter().filter(|d| {
                d.suggestions.iter().any(|s| s.applicability == Applicability::MachineApplicable)
            });
            assert!(
                fixable.count() == diagnostics.len(),
                "{what}: a diagnostic without a fix:\n{diagnostics:#?}\n---\n{current}"
            );
            for d in &diagnostics {
                for s in &d.suggestions {
                    for e in &s.edits {
                        assert_eq!(e.file.as_deref(), Some("test.t"), "{what}: {e:?}");
                        assert!(e.span.is_some(), "{what}: unresolved edit {e:?}");
                    }
                }
            }
            current = apply_suggestions(&current, &diagnostics);
        }
        let remaining = check_all(&current);
        assert!(remaining.is_empty(), "{what}: still failing after the fixes:\n{remaining:#?}\n---\n{current}");
    }
}

#[test]
fn a_misspelling_is_not_offered_when_two_names_are_equally_close() {
    // `magnitude_a` and `magnitude_b` are both one edit from
    // `magnitude_c`: picking one would be a guess.
    let source = "struct P { magnitude_a: u64, magnitude_b: u64 }
fn main() -> u64 {
    val p = P { magnitude_a: 1u64, magnitude_b: 2u64 }
    p.magnitude_c
}";
    let diagnostics = check_all(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert!(diagnostics[0].suggestions.is_empty(), "{diagnostics:#?}");
}

// LLM-TOOLING #3: `related` names the other place a diagnostic is
// about, and like the primary span it resolves byte for byte.

fn related_texts<'a>(source: &'a str, d: &Diagnostic) -> Vec<(&'a str, u32, String)> {
    d.related
        .iter()
        .map(|r| {
            let span = r.span.expect("related span");
            assert_eq!(r.file.as_deref(), Some("test.t"), "{r:?}");
            (&source[span.offset as usize..span.end_offset as usize], span.line, r.message.clone())
        })
        .collect()
}

#[test]
fn a_duplicate_points_back_at_the_first_declaration() {
    let source = "fn area() -> u64 { 1u64 }\nfn area() -> u64 { 2u64 }\nfn main() -> u64 { area() }";
    let diagnostics = check_all(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(related_texts(source, &diagnostics[0]), [("area", 1, "first defined here".to_string())]);
}

#[test]
fn a_use_after_move_points_at_the_move() {
    let source = "struct B { v: Vec<u64> }
fn main() -> u64 {
    val v: Vec<u64> = Vec::new()
    val b = B { v: v }
    v.size()
}";
    let diagnostics = check_all(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E0014");
    assert_eq!(related_texts(source, &diagnostics[0]), [("v", 4, "moved here".to_string())]);
}

#[test]
fn an_argument_mismatch_points_at_the_parameter() {
    let source = "fn g(count: u64) -> u64 { count }\nfn main() -> u64 { g(true) }";
    let diagnostics = check_all(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    let related = related_texts(source, &diagnostics[0]);
    assert_eq!(related.len(), 1, "{related:?}");
    assert_eq!((related[0].0, related[0].1), ("count", 1));
}

#[test]
fn an_impl_that_does_not_match_its_trait_points_at_both_methods() {
    let source = "trait Area {
    fn area(&self) -> u64
}
struct Sq { side: u64 }
impl Area for Sq {
    fn area(&self) -> i64 { 1i64 }
}
fn main() -> u64 { 0u64 }";
    let diagnostics = check_all(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    let d = &diagnostics[0];
    assert_eq!(d.code, "E0038");
    assert_eq!((span_text(source, d), d.span.unwrap().line), ("area", 6));
    let related = related_texts(source, d);
    assert_eq!((related[0].0, related[0].1), ("area", 2), "{related:?}");
}

// LLM-TOOLING #6: one run reports what one run can see.

fn diagnose_failed_parse(source: &str) -> Vec<Diagnostic> {
    let core = core_modules_dir();
    interpreter::diagnose_parse_failure(source, "test.t", std::slice::from_ref(&core))
}

#[test]
fn parse_errors_come_with_the_type_errors_of_what_parsed() {
    let source = "struct P { x: u64 }
fn f(a: u64) -> u64 {
    val b: bool = a
    if a > 1u64 { 1u64 } else if a > 0u64 { 2u64 } else { 3u64 }
}
fn g() -> u64 { calculate_totl(1u64) }
fn calculate_total(a: u64) -> u64 { a }
fn h() -> u64 { val p = P { x: 1u64, z: 2u64 }
    p.x }
fn main() -> u64 { val z = 1.5
    0u64 }";
    let diagnostics = diagnose_failed_parse(source);
    let codes: Vec<&str> = diagnostics.iter().map(|d| d.code).collect();
    // `val b: bool = a` sits in `f`, which holds the `else if`: its
    // errors are not reported, whatever they are.
    assert_eq!(codes, ["E0033", "E0003", "E0010", "E0034"], "{diagnostics:#?}");
}

#[test]
fn a_parse_error_outside_every_function_reports_the_parse_errors_alone() {
    let source = "struct P { x: u64 y: u64 }
fn g() -> u64 { calculate_totl(1u64) }
fn main() -> u64 { 0u64 }";
    let diagnostics = diagnose_failed_parse(source);
    assert!(diagnostics.iter().all(|d| d.code == "E0032"), "{diagnostics:#?}");
    assert!(!diagnostics.is_empty());
}

#[test]
fn an_error_downstream_of_another_is_not_reported() {
    // `p` is Unknown after the bad literal; `p.w` and `p ?? 3u64` would
    // each report about that, not about anything written wrong.
    let source = "struct P { x: u64 }
fn main() -> u64 {
    val p = P { x: 1u64, z: 2u64 }
    val a = p.w
    val o = p ?? 3u64
    0u64
}";
    let diagnostics = check_all(source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert!(diagnostics[0].message.starts_with("Unknown field 'z'"));
}
