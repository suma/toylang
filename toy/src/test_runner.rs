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
    /// Run the blocks natively (TEST-TOOL T1) rather than on the IR
    /// VM. The default, because the lane that ships is the one worth
    /// testing — the bugs a real program hits are backend-specific.
    pub aot: bool,
    pub release: bool,
    /// TEST-TOOL T5: record the golden files instead of checking
    /// them. Reaches the program as `TOY_BLESS`, which
    /// `testing::assert_golden` reads.
    pub bless: bool,
}

/// One `test` block's result, flattened for the report.
struct Outcome {
    name: String,
    file: String,
    line: u32,
    failure: Option<String>,
}

/// One test as the compiled lane needs to see it.
struct Planned {
    name: String,
    file: String,
    line: u32,
    expect_panic: Option<Option<String>>,
}

pub fn run(pkg: &Package, opts: &Options) -> Result<(), String> {
    // A golden path in a test (`tests/golden/one.bin`) is written
    // relative to the package, so that is where tests run from — on
    // both lanes, since the in-process one inherits `toy`'s directory
    // and the compiled one is spawned.
    let restore = std::env::current_dir().ok();
    if std::env::set_current_dir(&pkg.root).is_err() {
        return Err(format!("cannot enter `{}`", pkg.root.display()));
    }
    let result = run_in_package(pkg, opts);
    if let Some(dir) = restore {
        let _ = std::env::set_current_dir(dir);
    }
    result
}

fn run_in_package(pkg: &Package, opts: &Options) -> Result<(), String> {
    // The IR VM lane runs inside this process, so the variable has to
    // be here rather than on a child. Set once, before anything is
    // run, in a tool that is single-threaded at this point.
    if opts.bless {
        unsafe { std::env::set_var("TOY_BLESS", "1") };
    }
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

/// Marker the compiled driver prints before each test, on stderr.
/// Spelled in `compiler_lower::install_test_driver`; matched here.
const MARKER: &str = "__toy_test:";

/// Compile `file` with a test entry and run it.
///
/// **The lane stops at the first failure.** An assertion failure is a
/// panic and a panic ends the process, so the marker stream tells us
/// which test was running when it died and everything after it never
/// ran. Reporting every failure in one pass needs a process per test
/// (`TEST_TOOL.md` T4's shape); this is the cheap form, and the
/// report says which tests did not get a turn.
fn run_one_aot(pkg: &Package, file: &Path, opts: &Options) -> Result<Vec<Outcome>, String> {
    let planned = plan(pkg, file, opts)?;
    if planned.is_empty() {
        return Ok(Vec::new());
    }
    // TEST-TOOL T4: a `panics` test ends the process, so it cannot
    // share a driver with tests that have to run after it. Each gets
    // its own binary — the design's shape, on the premise that there
    // are few of them.
    let (panicking, plain): (Vec<&Planned>, Vec<&Planned>) =
        planned.iter().partition(|p| p.expect_panic.is_some());
    let mut out = Vec::with_capacity(planned.len());
    for p in &panicking {
        out.push(run_one_panics_aot(pkg, file, opts, p)?);
    }
    if plain.is_empty() {
        return Ok(out);
    }
    out.extend(run_plain_aot(pkg, file, opts, &plain)?);
    Ok(out)
}

/// A single `panics` test, in a binary of its own.
fn run_one_panics_aot(
    pkg: &Package,
    file: &Path,
    opts: &Options,
    test: &Planned,
) -> Result<Outcome, String> {
    let names = [test.name.clone()];
    let exe = compile_driver(pkg, file, opts, Some(&names), &sanitise(&test.name))?;
    let run = std::process::Command::new(&exe)
        .envs(bless_env(opts))
        .output()
        .map_err(|e| format!("cannot run `{}`: {e}", exe.display()))?;
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    let text: String = stderr
        .lines()
        .filter(|l| !l.starts_with(MARKER))
        .collect::<Vec<_>>()
        .join("\n");
    let failure = if run.status.success() {
        Some(format!("expected `{}` to panic, but it returned", test.name))
    } else {
        match &test.expect_panic {
            Some(Some(wanted)) if !text.contains(wanted.as_str()) => Some(format!(
                "expected a panic containing `{wanted}`, but it said:\n{text}"
            )),
            _ => None,
        }
    };
    Ok(Outcome {
        name: test.name.clone(),
        file: test.file.clone(),
        line: test.line,
        failure,
    })
}

/// `TOY_BLESS` for the compiled lane, which is a child process.
fn bless_env(opts: &Options) -> Vec<(String, String)> {
    if opts.bless {
        vec![("TOY_BLESS".to_string(), "1".to_string())]
    } else {
        Vec::new()
    }
}

/// Turn a test name into something that can be a file name.
fn sanitise(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// Compile `file` with a test entry, optionally holding one test.
fn compile_driver(
    pkg: &Package,
    file: &Path,
    opts: &Options,
    only: Option<&[String]>,
    stem: &str,
) -> Result<PathBuf, String> {
    let profile = crate::package::Profile::of(opts.release);
    let exe = pkg.test_exe_path(profile, stem);
    if let Some(parent) = exe.parent() {
        pkg.ensure_dir(parent)?;
    }
    let mut options = compiler::options::CompilerOptions::new(file.to_path_buf());
    options.output = Some(exe.clone());
    options.release = opts.release;
    options.core_modules_dirs = pkg.module_roots.clone();
    options.link_cache_dir = Some(pkg.link_cache_dir());
    options.test_mode = true;
    options.test_only = only.map(|names| names.to_vec());
    compiler::compile_file(&options)?;
    Ok(exe)
}

/// The tests that do not expect a panic, in one driver.
fn run_plain_aot(
    pkg: &Package,
    file: &Path,
    opts: &Options,
    expected: &[&Planned],
) -> Result<Vec<Outcome>, String> {
    // Name the ones to include rather than the ones to skip: the
    // driver runs a set, and "everything except the panicking tests"
    // is that set.
    let names: Vec<String> = expected.iter().map(|p| p.name.clone()).collect();
    let exe = compile_driver(
        pkg,
        file,
        opts,
        Some(&names),
        file.file_stem().and_then(|s| s.to_str()).unwrap_or("tests"),
    )?;
    let out = std::process::Command::new(&exe)
        .envs(bless_env(opts))
        .output()
        .map_err(|e| format!("cannot run `{}`: {e}", exe.display()))?;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    // Every marker that was printed is a test that started. The last
    // one is the one that was running when the process ended; it
    // passed only if the process exited cleanly.
    let started: Vec<String> = stderr
        .lines()
        .filter_map(|l| l.strip_prefix(MARKER))
        .map(|s| s.to_string())
        .collect();
    let failure_text: String = stderr
        .lines()
        .filter(|l| !l.starts_with(MARKER))
        .collect::<Vec<_>>()
        .join("\n");
    let ok = out.status.success();

    let mut result = Vec::with_capacity(expected.len());
    for p in expected {
        let position = started.iter().position(|s| *s == p.name);
        let failure = match position {
            // Never started: an earlier test ended the process.
            None => Some("not run: an earlier test ended the process".to_string()),
            // Started, and it is the last one, and we died: this is it.
            Some(i) if !ok && i + 1 == started.len() => Some(failure_text.clone()),
            Some(_) => None,
        };
        result.push(Outcome {
            name: p.name.clone(),
            file: p.file.clone(),
            line: p.line,
            failure,
        });
    }
    Ok(result)
}

/// The test blocks `file` declares, without running them. The AOT
/// path needs this to know what the driver will run and which of them
/// expect a panic.
fn plan(pkg: &Package, file: &Path, opts: &Options) -> Result<Vec<Planned>, String> {
    let source = std::fs::read_to_string(file)
        .map_err(|e| format!("cannot read `{}`: {e}", file.display()))?;
    let display = display_path(pkg, file);
    let mut options = RunOptions::default();
    options.core_modules_dirs = &pkg.module_roots;
    Ok(interpreter::list_tests_from_source(&source, &display, &options)?
        .into_iter()
        .filter(|o| match &opts.filter {
            Some(f) => o.name.contains(f.as_str()),
            None => true,
        })
        .map(|o| Planned {
            name: o.name,
            file: o.file.unwrap_or_else(|| display.clone()),
            line: o.line,
            expect_panic: o.expect_panic,
        })
        .collect())
}

fn run_one(pkg: &Package, file: &Path, opts: &Options) -> Result<Vec<Outcome>, String> {
    // `--list` never needs to run anything, whichever lane is asked
    // for, and the AOT path needs the listing anyway to know what the
    // driver will run.
    if opts.list_only {
        return list_one(pkg, file, opts);
    }
    if opts.aot {
        return run_one_aot(pkg, file, opts);
    }
    run_one_vm(pkg, file, opts)
}

/// The test blocks in `file`, discovered by type-checking it. Nothing
/// is executed.
fn list_one(pkg: &Package, file: &Path, opts: &Options) -> Result<Vec<Outcome>, String> {
    let mut listed = run_one_vm(pkg, file, &Options { list_only: true, ..clone_opts(opts) })?;
    for o in &mut listed {
        o.failure = None;
    }
    Ok(listed)
}

fn clone_opts(opts: &Options) -> Options {
    Options {
        filter: opts.filter.clone(),
        list_only: opts.list_only,
        format: opts.format,
        verbose: opts.verbose,
        aot: opts.aot,
        release: opts.release,
        bless: opts.bless,
    }
}

fn run_one_vm(pkg: &Package, file: &Path, opts: &Options) -> Result<Vec<Outcome>, String> {
    let source = std::fs::read_to_string(file)
        .map_err(|e| format!("cannot read `{}`: {e}", file.display()))?;
    let display = display_path(pkg, file);
    let mut options = RunOptions::default();
    options.core_modules_dirs = &pkg.module_roots;
    let outcomes = if opts.list_only {
        interpreter::list_tests_from_source(&source, &display, &options)?
    } else {
        interpreter::run_tests_from_source(&source, &display, &options)?
    };
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
