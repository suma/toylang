//! `toy test` — running a package's `test` blocks (TEST_TOOL.md D3).
//!
//! The tests live wherever the code does. TEST-TOOL T0 made a module
//! able to hold a `test` block at all — before it, integration failed
//! on the string concatenation `assert_eq` desugars to, so the only
//! file that could hold a test was the entry, which is the one file in
//! the language that is not a module. That is why `poc/logsearch` had
//! 5,000 lines and no tests.
//!
//! What this adds on top is the running: a whole package at once
//! rather than a file at a time, a name filter, `--list`, and a JSON
//! form for anything reading the results rather than looking at them.
//! One process, because `toy` holds the interpreter as a crate and a
//! spawn is ~30 ms — fifty test files used to be fifty spawns.

use std::path::{Path, PathBuf};

use interpreter::RunOptions;

use crate::package::Package;

/// How to report.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

pub struct Options {
    /// Substring filter on the test name. Empty runs everything.
    pub filter: Option<String>,
    pub list_only: bool,
    pub format: Format,
    pub verbose: bool,
}

/// One `test` block's result, flattened for the report.
struct Outcome {
    name: String,
    file: String,
    line: u32,
    failure: Option<String>,
}

pub fn run(pkg: &Package, opts: &Options) -> Result<(), String> {
    let files = discover(pkg)?;
    if files.is_empty() {
        return Err(format!(
            "no `.t` files found in `{}`\n  looked in tests/, src/ and the package root",
            pkg.root.display()
        ));
    }

    let mut outcomes: Vec<Outcome> = Vec::new();
    // A module's tests come along with whichever program integrates
    // it, and `tests/a.t` and the entry both integrate `src/`. Without
    // this the same block is run and reported once per program that
    // pulled it in -- three tests became five. Keyed by where the
    // block is written, which is the only thing that identifies it.
    let mut seen: std::collections::HashSet<(String, u32, String)> =
        std::collections::HashSet::new();
    let started = std::time::Instant::now();
    for file in &files {
        if opts.verbose {
            eprintln!("toy: running tests in {}", file.display());
        }
        for outcome in run_one(pkg, file, opts)? {
            let key = (outcome.file.clone(), outcome.line, outcome.name.clone());
            if seen.insert(key) {
                outcomes.push(outcome);
            }
        }
    }
    let elapsed = started.elapsed();

    if opts.list_only {
        for o in &outcomes {
            println!("{}  ({}:{})", o.name, o.file, o.line);
        }
        println!("{} test(s)", outcomes.len());
        return Ok(());
    }

    match opts.format {
        Format::Json => report_json(&outcomes),
        Format::Text => report_text(&outcomes, elapsed),
    }
    if outcomes.iter().any(|o| o.failure.is_some()) {
        std::process::exit(1);
    }
    Ok(())
}

/// Which files to run.
///
/// `tests/*.t` and the entry. **Not** `src/**` on its own: a module's
/// tests come along with it when the entry is compiled, since the
/// module roots include `src/`, and running the module as its own
/// program would compile it a second time. A file under `tests/` is
/// its own program — that is what the directory is for.
fn discover(pkg: &Package) -> Result<Vec<PathBuf>, String> {
    let mut out: Vec<PathBuf> = Vec::new();
    let tests_dir = pkg.root.join("tests");
    if tests_dir.is_dir() {
        collect_t_files(&tests_dir, &mut out)?;
    }
    if pkg.entry.is_file() {
        out.push(pkg.entry.clone());
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn collect_t_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let read = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read `{}`: {e}", dir.display()))?;
    for entry in read {
        let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_t_files(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("t") {
            out.push(path);
        }
    }
    Ok(())
}

fn run_one(pkg: &Package, file: &Path, opts: &Options) -> Result<Vec<Outcome>, String> {
    let source = std::fs::read_to_string(file)
        .map_err(|e| format!("cannot read `{}`: {e}", file.display()))?;
    let display = display_path(pkg, file);
    let mut options = RunOptions::default();
    options.core_modules_dirs = &pkg.module_roots;
    let outcomes = interpreter::run_tests_from_source(&source, &display, &options)?;
    Ok(outcomes
        .into_iter()
        .filter(|o| match &opts.filter {
            Some(f) => o.name.contains(f.as_str()),
            None => true,
        })
        .map(|o| Outcome {
            name: o.name,
            // A test carried in from a module names its own file; one
            // written in this file does not, and the file is this one.
            file: o.file.unwrap_or_else(|| display.clone()),
            line: o.line,
            failure: o.failure,
        })
        .collect())
}

/// Paths relative to the package root, so a report is the same
/// wherever the package is checked out.
fn display_path(pkg: &Package, file: &Path) -> String {
    file.strip_prefix(&pkg.root)
        .unwrap_or(file)
        .to_string_lossy()
        .into_owned()
}

/// Failure-first, the shape `--test` already had: a passing run is a
/// single line, and a failure is the diagnostic in full.
fn report_text(outcomes: &[Outcome], elapsed: std::time::Duration) {
    let failed: Vec<&Outcome> = outcomes.iter().filter(|o| o.failure.is_some()).collect();
    for o in &failed {
        eprintln!("FAILED  {} ({}:{})", o.name, o.file, o.line);
        for line in o.failure.as_deref().unwrap_or("").lines() {
            eprintln!("    {line}");
        }
    }
    let passed = outcomes.len() - failed.len();
    println!(
        "{passed} passed, {} failed   {:.2} s",
        failed.len(),
        elapsed.as_secs_f64()
    );
}

/// The machine form. Hand-written rather than pulled through serde:
/// four fields, and `toy` has no other reason to take the dependency.
fn report_json(outcomes: &[Outcome]) {
    println!("[");
    for (i, o) in outcomes.iter().enumerate() {
        let comma = if i + 1 == outcomes.len() { "" } else { "," };
        let failure = match &o.failure {
            Some(f) => format!("\"{}\"", escape(f)),
            None => "null".to_string(),
        };
        println!(
            "  {{\"name\": \"{}\", \"file\": \"{}\", \"line\": {}, \"failure\": {failure}}}{comma}",
            escape(&o.name),
            escape(&o.file),
            o.line
        );
    }
    println!("]");
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
