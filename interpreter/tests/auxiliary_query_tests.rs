// LLM-LOOP P7 — the queries that answer from the compiler's tables
// instead of from a run: type holes, `--api`, `--explain`.
//
// What these pin is the property each feature exists for, not its
// formatting:
//
//   * a hole reports the type the reader would have had to guess, and
//     reports *every* hole in one pass without poisoning the bindings
//     it answered for
//   * the type a hole reports parses back — pasting it over the `_`
//     compiles, which is the whole point of naming it
//   * `--api` lists what a module provides, contracts included
//   * every diagnostic code the compiler can print has an explanation


use crate::common::core_modules_dir;
use frontend::diagnostic::Diagnostic;

/// Type check `source` and return the structured diagnostics.
fn diagnose(source: &str) -> Vec<Diagnostic> {
    let mut parser = frontend::ParserWithInterner::new(source);
    parser.set_source_file("test.t");
    let mut program = parser.parse_program().expect("parse");
    let string_interner = parser.get_string_interner();
    let core = core_modules_dir();
    interpreter::check_typing_diagnostics(
        &mut program,
        string_interner,
        Some(source),
        Some("test.t"),
        std::slice::from_ref(&core),
    )
    .err()
    .unwrap_or_default()
}

fn holes(source: &str) -> Vec<String> {
    diagnose(source)
        .into_iter()
        .filter(|d| d.code == frontend::diagnostic::codes::TYPE_HOLE)
        .map(|d| d.message)
        .collect()
}

#[test]
fn hole_reports_the_inferred_type() {
    let source = r#"
fn add(a: i64, b: i64) -> i64 { a + b }
fn main() -> u64 {
    val total: _ = add(1i64, 2i64)
    0u64
}
"#;
    assert_eq!(holes(source), vec!["type hole: `total` has type `i64`"]);
}

#[test]
fn every_hole_in_a_file_is_answered_in_one_run() {
    // The point of the feature is that N holes cost one round trip, not
    // N. If a hole aborted its statement this would report one.
    let source = r#"
struct P { x: i64 }
fn main() -> u64 {
    val a: _ = 1i64
    val b: _ = true
    val c: _ = P { x: 1i64 }
    val d: _ = (1i64, "hi")
    0u64
}
"#;
    assert_eq!(
        holes(source),
        vec![
            "type hole: `a` has type `i64`",
            "type hole: `b` has type `bool`",
            "type hole: `c` has type `P`",
            "type hole: `d` has type `(i64, str)`",
        ]
    );
}

#[test]
fn a_hole_does_not_poison_later_uses_of_the_binding() {
    // The binding keeps its inferred type, so the answer is not buried
    // under follow-on errors it caused itself. Exactly one diagnostic.
    let source = r#"
fn main() -> u64 {
    val n: _ = 2i64
    val doubled: i64 = n * 2i64
    val tripled: i64 = doubled + n
    0u64
}
"#;
    let diagnostics = diagnose(source);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected only the hole, got: {:?}",
        diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    assert_eq!(diagnostics[0].code, frontend::diagnostic::codes::TYPE_HOLE);
}

#[test]
fn the_reported_type_can_be_pasted_over_the_hole() {
    // A name that does not parse would be worse than no name, so this
    // substitutes the reported type back and re-checks.
    let source = r#"
struct P { x: i64 }
fn main() -> u64 {
    val p: _ = P { x: 1i64 }
    0u64
}
"#;
    let message = holes(source).remove(0);
    let ty = message
        .rsplit_once("has type `")
        .and_then(|(_, rest)| rest.strip_suffix('`'))
        .expect("message names the type in backticks");
    let filled = source.replace("val p: _", &format!("val p: {ty}"));
    assert!(
        diagnose(&filled).is_empty(),
        "filling the hole with `{ty}` should compile, got: {:?}",
        diagnose(&filled)
    );
}

#[test]
fn hole_on_an_unsuffixed_literal_answers_a_spellable_type() {
    // NUMBER-HINT: the hole used to be answered while the literal was
    // still the internal `Number` placeholder, printing
    // `<Number: no source syntax>` — a name that does not parse, for a
    // feature whose whole point is that the reader pastes the answer.
    let source = r#"
fn main() -> u64 {
    val a = 42
    val h: _ = a
    0
}
"#;
    assert_eq!(holes(source), vec!["type hole: `h` has type `u64`"]);
}

#[test]
fn hole_on_a_literal_reports_the_type_the_program_gives_it() {
    // The answer must be the type the literal actually ends up with,
    // not the default it would have taken had nothing claimed it. Here
    // `f`'s parameter claims `a`, so the hole says `i64`.
    let source = r#"
fn f(x: i64) -> i64 { x }
fn main() -> u64 {
    val a = 42
    val h: _ = a
    println(f(a))
    0
}
"#;
    assert_eq!(holes(source), vec!["type hole: `h` has type `i64`"]);
}

#[test]
fn var_bindings_get_holes_too() {
    // `val` and `var` take separate paths through the checker and have
    // been out of step before, so both are pinned.
    let source = r#"
fn main() -> u64 {
    var m: _ = 3u64
    0u64
}
"#;
    assert_eq!(holes(source), vec!["type hole: `m` has type `u64`"]);
}

#[test]
fn underscore_is_still_an_ordinary_identifier_elsewhere() {
    // `_` reaches the parser as a plain identifier, so confining the
    // hole to the annotation position must leave match wildcards and
    // `_`-prefixed names alone.
    let source = r#"
enum E { A, B }
fn main() -> u64 {
    val _unused: u64 = 1u64
    val e = E::A
    match e {
        E::A => 0u64,
        _ => 1u64,
    }
}
"#;
    assert!(diagnose(source).is_empty(), "{:?}", diagnose(source));
}

#[test]
fn a_hole_outside_a_binding_annotation_is_rejected() {
    // Nothing to infer from in a parameter position, so it must not
    // silently parse into something meaningless.
    let source = "fn f(x: _) -> u64 { 0u64 }\nfn main() -> u64 { 0u64 }\n";
    let mut parser = frontend::ParserWithInterner::new(source);
    parser.set_source_file("test.t");
    assert!(parser.parse_program().is_err(), "`_` should not parse as a parameter type");
}

// --- `--api` --------------------------------------------------------

fn api(source: &str) -> String {
    let mut parser = frontend::ParserWithInterner::new(source);
    parser.set_source_file("test.t");
    let program = parser.parse_program().expect("parse");
    frontend::api::render(&program, parser.get_string_interner(), Some(source))
}

#[test]
fn api_lists_the_shapes_a_module_provides() {
    let source = r#"
pub struct Point { pub x: i64, y: i64 }

pub trait Named {
    fn name(&self) -> str
}

impl Named for Point {
    fn name(&self) -> str { "point" }
}

pub fn origin() -> Point { Point { x: 0i64, y: 0i64 } }
fn helper(a: u64) -> u64 { a }
"#;
    let out = api(source);
    assert!(out.contains("pub struct Point {"), "{out}");
    assert!(out.contains("    pub x: i64"), "{out}");
    // Private members are listed, distinguished by the missing `pub`.
    assert!(out.contains("\n    y: i64"), "{out}");
    assert!(out.contains("pub trait Named {"), "{out}");
    assert!(out.contains("impl Named for Point {"), "{out}");
    assert!(out.contains("    fn name(&self) -> str"), "{out}");
    assert!(out.contains("pub fn origin() -> Point"), "{out}");
    assert!(out.contains("\nfn helper(a: u64) -> u64"), "{out}");
    // Bodies are not part of the answer.
    assert!(!out.contains("\"point\""), "{out}");
}

#[test]
fn api_quotes_contracts_in_full() {
    // A contract says what a function refuses and guarantees, which a
    // caller cannot infer from the types. The clause's own AST node is
    // located at its operator, so quoting from that alone would print
    // `!= 0i64`; this pins the whole predicate.
    let source = r#"
pub fn divide(a: i64, b: i64) -> i64
    requires b != 0i64
    ensures result * b == a
{
    a / b
}
"#;
    let out = api(source);
    assert!(out.contains("requires b != 0i64"), "{out}");
    assert!(out.contains("ensures result * b == a"), "{out}");
}

#[test]
fn api_renders_generic_and_reference_types_as_written() {
    let source = r#"
pub struct Holder<T> { item: T }

impl Holder<T> {
    fn get(&self) -> T { self.item }
    fn replace(&mut self, other: &Holder<T>) {}
}
"#;
    let out = api(source);
    assert!(out.contains("pub struct Holder<T> {"), "{out}");
    // `impl Holder<T>` declares `T` implicitly, re-using the struct's
    // parameter, so its methods carry it exactly as the explicit
    // `impl<T> Holder<T>` form does.
    assert!(out.contains("fn get<T>(&self) -> T"), "{out}");
    assert!(out.contains("other: &Holder<T>"), "{out}");
}

// --- `--explain` ----------------------------------------------------

#[test]
fn every_code_a_diagnostic_can_carry_has_an_explanation() {
    // The compiler prints a code on every diagnostic. One with no prose
    // behind it makes `--explain` answer "unknown code" for something
    // the compiler just printed, which is worse than having no code.
    for code in frontend::diagnostic::codes::ALL {
        assert!(
            frontend::explain::explain(code).is_some(),
            "no explanation for {code}"
        );
    }
}

// --- CLI surface (D6 stdin, P7 query flags) -------------------------

/// Run the interpreter binary, feeding `source` on stdin when given.
fn cli(args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let core = core_modules_dir();
    let mut child = Command::new(env!("CARGO_BIN_EXE_interpreter"))
        .args(args)
        .arg("--core-modules")
        .arg(&core)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn interpreter");
    if let Some(source) = stdin {
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(source.as_bytes())
            .expect("write source");
    }
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_program_can_arrive_on_stdin() {
    // D6: a throwaway program should not need a throwaway file. `-` is
    // the input, not an unknown flag.
    let (status, stdout, stderr) = cli(&["-"], Some("fn main() -> u64 {\n    println(\"hi\")\n    3u64\n}\n"));
    assert_eq!(status, 3, "stderr: {stderr}");
    assert_eq!(stdout, "hi\n");
}

#[test]
fn stdin_diagnostics_name_the_input_rather_than_a_dash() {
    let (_, _, stderr) = cli(&["-"], Some("fn main() -> u64 {\n    val x: bool = 1u64\n    0u64\n}\n"));
    assert!(stderr.contains("<stdin>:2"), "stderr: {stderr}");
}

#[test]
fn explain_answers_for_a_code_and_lists_them_all() {
    let (status, stdout, _) = cli(&["--explain", "E0011"], None);
    assert_eq!(status, 0);
    assert!(stdout.starts_with("E0011: "), "{stdout}");

    let (status, listing, _) = cli(&["--explain"], None);
    assert_eq!(status, 0);
    for code in frontend::diagnostic::codes::ALL {
        assert!(listing.contains(*code), "listing omits {code}:\n{listing}");
    }

    // An unknown code must fail rather than print nothing and succeed.
    let (status, _, stderr) = cli(&["--explain", "E9999"], None);
    assert_ne!(status, 0);
    assert!(stderr.contains("no such diagnostic code"), "{stderr}");
}

#[test]
fn api_reads_a_module_from_a_path_or_stdin() {
    let (status, from_stdin, stderr) = cli(
        &["--api", "-"],
        Some("pub fn twice(n: u64) -> u64 { n * 2u64 }\n"),
    );
    assert_eq!(status, 0, "stderr: {stderr}");
    assert!(from_stdin.contains("pub fn twice(n: u64) -> u64"), "{from_stdin}");
}

#[test]
fn diagnostics_from_real_programs_carry_explainable_codes() {
    // Guards the other direction: a code that reaches a reader must be
    // one `--explain` knows.
    let programs = [
        "fn main() -> u64 {\n    val x: bool = 1u64\n    0u64\n}\n",
        "fn main() -> u64 {\n    val x = nope(1u64)\n    0u64\n}\n",
        "fn main() -> u64 {\n    val xs = [1u64, true]\n    0u64\n}\n",
        "fn main() -> u64 {\n    val n: _ = 1i64\n    0u64\n}\n",
    ];
    for source in programs {
        let diagnostics = diagnose(source);
        assert!(!diagnostics.is_empty(), "expected a diagnostic for:\n{source}");
        for d in diagnostics {
            assert!(
                frontend::explain::explain(d.code).is_some(),
                "code {} has no explanation (from:\n{source})",
                d.code
            );
        }
    }
}
