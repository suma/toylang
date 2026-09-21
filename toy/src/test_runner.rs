//! `toy test` — running a package's `test` blocks (TEST_TOOL.md D3),
//! in parallel (TEST_PARALLEL.md).
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
//!
//! # Shape of a run
//!
//! **Plan, then run.** Every file is parsed and type-checked once up
//! front to learn what it declares; the filter and the duplicate fold
//! happen there, on names rather than on results. Then the planned
//! tests are cut into jobs with no dependencies between them and
//! handed to a pool of workers:
//!
//! | lane | job |
//! |---|---|
//! | AOT | one driver — compile a file's tests into a binary and run it |
//! | AOT | one `panics` test — its own binary, since a panic ends the process |
//! | IR VM | one test |
//!
//! Two rules keep this honest. **A job's front end and its execution
//! stay on one thread**, because the AST owns `Rc<Function>` and cannot
//! cross one; only an [`Outcome`] — strings and numbers — is handed
//! back. And **the report is assembled in plan order, not completion
//! order**, so `-j8` prints what `-j1` prints, byte for byte. That
//! equality is pinned by a test, and it is the only thing standing
//! between a parallel runner and a flaky one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    /// `--diagnostics=json`, for the parse / type errors met while
    /// planning (IR VM) or compiling a driver (AOT).
    pub diagnostics_json: bool,
    /// Run the blocks natively (TEST-TOOL T1) rather than on the IR
    /// VM. The default, because the lane that ships is the one worth
    /// testing — the bugs a real program hits are backend-specific.
    pub aot: bool,
    pub release: bool,
    /// TEST-TOOL T5: record the golden files instead of checking
    /// them. Reaches the program as `TOY_BLESS`, which
    /// `testing::assert_golden` reads.
    pub bless: bool,
    /// How many jobs to run at once (TEST-PARALLEL D2). `1` is the
    /// sequential runner: no threads are spawned at all.
    pub jobs: usize,
}

/// One `test` block's result, flattened for the report.
struct Outcome {
    name: String,
    file: String,
    line: u32,
    failure: Option<String>,
    /// What the test printed, kept so a failure can show it. Captured
    /// rather than let through: with workers running at once, letting
    /// twenty `println`s reach the terminal interleaves them into
    /// something no one can read (TEST-PARALLEL D4).
    output: String,
}

/// One planned test: what the plan pass learned about a `test` block
/// before anything ran.
struct Planned {
    name: String,
    file: String,
    line: u32,
    expect_panic: Option<Option<String>>,
    /// TEST-PARALLEL P5: `test "..." serial { }` — runs after every
    /// parallel job, one at a time.
    serial: bool,
    /// Position among the declaring file's blocks, which is how the
    /// IR VM lane names the one to run.
    index: usize,
}

/// One source file and the tests it contributes to this run.
struct FilePlan {
    path: PathBuf,
    display: String,
    tests: Vec<Planned>,
}

/// A unit of work with no dependency on any other.
enum Job {
    /// Compile one driver holding these tests and run it.
    AotDriver { file: usize, tests: Vec<usize> },
    /// One `panics` test in a binary of its own: a panic ends the
    /// process, so it cannot share a driver with anything that has to
    /// run after it (TEST-TOOL T4).
    AotPanics { file: usize, test: usize },
    /// One test on the IR VM.
    Vm { file: usize, test: usize },
}

impl Job {
    /// Which planned tests this job answers for, as `(file, test)`.
    fn covers(&self) -> Vec<(usize, usize)> {
        match self {
            Job::AotDriver { file, tests } => tests.iter().map(|t| (*file, *t)).collect(),
            Job::AotPanics { file, test } | Job::Vm { file, test } => vec![(*file, *test)],
        }
    }
}

pub fn run(pkg: &Package, opts: &Options) -> Result<(), String> {
    // A golden path in a test (`tests/golden/one.bin`) is written
    // relative to the package, so that is where tests run from — on
    // both lanes, since the in-process one inherits `toy`'s directory
    // and the compiled one is spawned.
    //
    // The directory is process-wide, which is also why a test cannot
    // be given one of its own (TEST_PARALLEL.md §8): set once, before
    // any worker exists, and never touched again.
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
    let files = discover(pkg)?;
    if files.is_empty() {
        return Err(format!(
            "no `.t` files found in `{}`\n  looked in tests/, src/ and the package root",
            pkg.root.display()
        ));
    }

    let started = std::time::Instant::now();
    let plans = plan_all(pkg, &files, opts)?;

    if opts.list_only && opts.format == Format::Json {
        let tests: Vec<serde_json::Value> = plans
            .iter()
            .flat_map(|plan| plan.tests.iter())
            // TEST-PARALLEL P5: `serial` is a property of the test,
            // and it is the answer to "why did this suite not go any
            // faster" — so the inventory says it.
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "file": t.file,
                    "line": t.line,
                    "serial": t.serial,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&serde_json::Value::Array(tests)).unwrap_or_default());
        return Ok(());
    }
    if opts.list_only {
        let mut count = 0usize;
        for plan in &plans {
            for test in &plan.tests {
                let mark = if test.serial { "  serial" } else { "" };
                println!("{}  ({}:{}){mark}", test.name, test.file, test.line);
                count += 1;
            }
        }
        println!("{count} test(s)");
        return Ok(());
    }

    let outcomes = execute(pkg, &plans, opts)?;
    let elapsed = started.elapsed();

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

// ---------------------------------------------------------------
// Plan
// ---------------------------------------------------------------

/// The `test` blocks each file declares, in file order.
///
/// **Planning is a front end**: parse, type-check, read off the
/// blocks. It is the same work a run needs, so on one worker the IR VM
/// lane keeps what it checked (`prepared_for`) instead of paying for
/// it twice. With workers, it cannot — a checked program holds `Rc` —
/// so the listing is spread over them and each worker re-checks the
/// file it later takes a test from. Doing this sequentially while the
/// tests ran in parallel was worse than not parallelising at all: a
/// nine-file suite spent 0.19 s here and 0.02 s running.
type Listing = Vec<interpreter::TestCaseInfo>;

fn list_all(
    pkg: &Package,
    files: &[PathBuf],
    opts: &Options,
) -> Result<Vec<Listing>, String> {
    let workers = opts.jobs.max(1).min(files.len());
    if workers == 1 {
        return files
            .iter()
            .map(|file| list_one(pkg, file, opts))
            .collect();
    }
    let slots: Vec<OnceLock<Result<Listing, String>>> =
        files.iter().map(|_| OnceLock::new()).collect();
    let counter = AtomicUsize::new(0);
    let cursor = &counter;
    let slots_ref = &slots;
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(move || {
                loop {
                    let i = cursor.fetch_add(1, Ordering::Relaxed);
                    if i >= files.len() {
                        break;
                    }
                    let _ = slots_ref[i].set(list_one(pkg, &files[i], opts));
                }
            });
        }
    });
    let mut out = Vec::with_capacity(files.len());
    for slot in slots {
        match slot.into_inner() {
            Some(Ok(cases)) => out.push(cases),
            Some(Err(e)) => return Err(e),
            None => return Err("internal error: a file was never planned".to_string()),
        }
    }
    Ok(out)
}

/// One file's `test` blocks.
///
/// On the IR VM lane with a single worker the checked program is kept
/// (this thread is the one that will run the tests); otherwise it is
/// read and dropped, because the thread that plans a file is not
/// necessarily the thread that runs it and a checked program is the
/// whole stdlib plus the package.
fn list_one(
    pkg: &Package,
    file: &Path,
    opts: &Options,
) -> Result<Listing, String> {
    if opts.verbose {
        eprintln!("toy: planning {}", file.display());
    }
    let display = display_path(pkg, file);
    let keep = !opts.aot && !opts.list_only && opts.jobs.max(1) == 1;
    if keep {
        return Ok(prepared_for(pkg, file, &display, opts)?.cases().to_vec());
    }
    let source = std::fs::read_to_string(file)
        .map_err(|e| format!("cannot read `{}`: {e}", file.display()))?;
    let mut options = RunOptions::default();
    options.core_modules_dirs = &pkg.module_roots;
    options.diagnostics_json = opts.diagnostics_json;
    Ok(interpreter::list_tests_from_source(&source, &display, &options)?
        .into_iter()
        .map(|o| interpreter::TestCaseInfo {
            name: o.name,
            line: o.line,
            file: o.file,
            expect_panic: o.expect_panic,
            serial: o.serial,
        })
        .collect())
}

/// Learn what every file declares, apply the filter, and drop the
/// duplicates — all before anything runs.
///
/// **The fold used to happen after the fact.** A module's tests come
/// along with whichever program integrates it, and `tests/a.t` and the
/// entry both integrate `src/`, so the same block was *run* once per
/// program that pulled it in and then reported once. Identifying a
/// block by where it is written (file, line, name) is the same rule as
/// before; doing it here means the extra runs do not happen either.
fn plan_all(pkg: &Package, files: &[PathBuf], opts: &Options) -> Result<Vec<FilePlan>, String> {
    let listed = list_all(pkg, files, opts)?;
    let mut seen: std::collections::HashSet<(String, u32, String)> =
        std::collections::HashSet::new();
    let mut plans = Vec::with_capacity(files.len());
    for (file, cases) in files.iter().zip(listed) {
        let display = display_path(pkg, file);
        let mut tests = Vec::new();
        for (index, case) in cases.into_iter().enumerate() {
            if opts.filter.as_deref().is_some_and(|f| !case.name.contains(f)) {
                continue;
            }
            // A test carried in from a module names its own file; one
            // written in this file does not, and the file is this one.
            let file_name = case.file.unwrap_or_else(|| display.clone());
            let key = (file_name.clone(), case.line, case.name.clone());
            if !seen.insert(key) {
                continue;
            }
            tests.push(Planned {
                name: case.name,
                file: file_name,
                line: case.line,
                expect_panic: case.expect_panic,
                serial: case.serial,
                index,
            });
        }
        plans.push(FilePlan {
            path: file.clone(),
            display,
            tests,
        });
    }
    Ok(plans)
}

// ---------------------------------------------------------------
// Schedule
// ---------------------------------------------------------------

/// Cut the plan into jobs and run them, `opts.jobs` at a time.
fn execute(pkg: &Package, plans: &[FilePlan], opts: &Options) -> Result<Vec<Outcome>, String> {
    let Schedule { jobs, serial_from } = build_jobs(plans, opts);
    if jobs.is_empty() {
        return Ok(Vec::new());
    }

    // One slot per planned test, filled by whichever worker owns the
    // job that covers it. `OnceLock` because a slot is written exactly
    // once and read after every worker has finished.
    let slots: Vec<Vec<OnceLock<Outcome>>> = plans
        .iter()
        .map(|p| p.tests.iter().map(|_| OnceLock::new()).collect())
        .collect();
    let errors: Vec<OnceLock<String>> = jobs.iter().map(|_| OnceLock::new()).collect();

    let workers = opts.jobs.max(1).min(serial_from.max(1));
    if workers == 1 {
        // No threads at all in the sequential case: it is the fallback
        // for anything the parallel path would disturb (`--bless`), so
        // it must not be the parallel path with one worker.
        for (id, job) in jobs.iter().enumerate() {
            run_job(pkg, plans, opts, job, id, &slots, &errors, 0);
        }
    } else {
        let counter = AtomicUsize::new(0);
        let cursor = &counter;
        let jobs_ref = &jobs[..serial_from];
        let slots_ref = &slots;
        let errors_ref = &errors;
        std::thread::scope(|scope| {
            for worker in 0..workers {
                scope.spawn(move || {
                    // One cursor, many workers. Jobs are wildly uneven
                    // — one test runs for seconds while its neighbours
                    // take a millisecond — so a static split would
                    // leave workers idle behind the long one.
                    loop {
                        let id = cursor.fetch_add(1, Ordering::Relaxed);
                        if id >= jobs_ref.len() {
                            break;
                        }
                        run_job(
                            pkg,
                            plans,
                            opts,
                            &jobs_ref[id],
                            id,
                            slots_ref,
                            errors_ref,
                            worker,
                        );
                    }
                });
            }
        });
        // TEST-PARALLEL P5: the `serial` ones, after every worker has
        // finished, one at a time. Last rather than first because
        // "after everything else" is a rule a reader can hold, and a
        // test that needs the machine to itself gets exactly that.
        for (offset, job) in jobs[serial_from..].iter().enumerate() {
            let id = serial_from + offset;
            run_job(pkg, plans, opts, job, id, &slots, &errors, 0);
        }
    }

    // A job that could not even be compiled is reported the way it was
    // before there were jobs: as the run's error. The first one in job
    // order, so the message does not depend on who lost the race.
    if let Some(error) = errors.iter().find_map(|e| e.get()) {
        return Err(error.clone());
    }

    let mut out = Vec::new();
    for (file, plan) in plans.iter().enumerate() {
        for (test, planned) in plan.tests.iter().enumerate() {
            match slots[file][test].get() {
                Some(outcome) => out.push(clone_outcome(outcome)),
                // Not reachable: every planned test belongs to exactly
                // one job. Reported rather than panicked on, because a
                // silently missing test is the one failure mode a test
                // runner must never have.
                None => out.push(Outcome {
                    name: planned.name.clone(),
                    file: planned.file.clone(),
                    line: planned.line,
                    failure: Some("internal error: no job claimed this test".to_string()),
                    output: String::new(),
                }),
            }
        }
    }
    Ok(out)
}

/// One job per driver on the compiled lane, one per test on the VM.
///
/// **A file split across workers is checked once per worker**, since a
/// checked program holds `Rc` and cannot be shared. That is the price
/// of scheduling a test at a time, and it is what makes a slow suite
/// fast: `poc/logsearch` has fourteen tests in two files and spends
/// seconds in them, so paying a front end per worker to spread them is
/// the whole win (4.83 s to 0.80 s). A suite of millisecond tests pays
/// the same price for nothing — measurably so at low `-j`, where there
/// are too few workers to hide it. Telling the two apart would need
/// the tests' durations, and nothing here knows them: they are only
/// learnable by running, and the runner keeps nothing between runs.
/// So it splits unconditionally. `TEST_PARALLEL.md` §6 has the
/// measurements, from when it did keep them.
/// Every job, with the ones that may not run beside anything else
/// gathered at the end.
///
/// TEST-PARALLEL P5: one list rather than two, so a job keeps a single
/// id (which is what the error slots are indexed by) and the workers'
/// cursor needs no second bound. `serial_from` is where the parallel
/// part stops.
struct Schedule {
    jobs: Vec<Job>,
    serial_from: usize,
}

fn build_jobs(plans: &[FilePlan], opts: &Options) -> Schedule {
    let mut jobs = Vec::new();
    let mut serial = Vec::new();
    for (file, plan) in plans.iter().enumerate() {
        if plan.tests.is_empty() {
            continue;
        }
        if !opts.aot {
            for (test, planned) in plan.tests.iter().enumerate() {
                let job = Job::Vm { file, test };
                if planned.serial {
                    serial.push(job);
                } else {
                    jobs.push(job);
                }
            }
            continue;
        }
        let mut plain = Vec::new();
        for (test, planned) in plan.tests.iter().enumerate() {
            if planned.expect_panic.is_some() {
                let job = Job::AotPanics { file, test };
                if planned.serial {
                    serial.push(job);
                } else {
                    jobs.push(job);
                }
            } else if planned.serial {
                // A driver of its own: sharing one with the file's
                // other tests would make them serial too, and a
                // `serial` test says nothing about its neighbours.
                serial.push(Job::AotDriver { file, tests: vec![test] });
            } else {
                plain.push(test);
            }
        }
        if !plain.is_empty() {
            jobs.push(Job::AotDriver { file, tests: plain });
        }
    }
    let serial_from = jobs.len();
    jobs.extend(serial);
    Schedule { jobs, serial_from }
}

/// Run one job and file its outcomes. Never returns a failure: a job
/// that could not run records its error and leaves the slots it owns
/// to the caller's reconciliation.
#[allow(clippy::too_many_arguments)]
fn run_job(
    pkg: &Package,
    plans: &[FilePlan],
    opts: &Options,
    job: &Job,
    id: usize,
    slots: &[Vec<OnceLock<Outcome>>],
    errors: &[OnceLock<String>],
    worker: usize,
) {
    if opts.verbose {
        eprintln!("toy: [worker {worker}] job {id}: {}", describe(plans, job));
    }
    let produced = match job {
        Job::AotDriver { file, tests } => run_aot_driver(pkg, &plans[*file], opts, tests),
        Job::AotPanics { file, test } => {
            run_aot_panics(pkg, &plans[*file], opts, *test).map(|o| vec![o])
        }
        Job::Vm { file, test } => run_vm_one(pkg, &plans[*file], opts, *test).map(|o| vec![o]),
    };
    match produced {
        Ok(outcomes) => {
            for ((file, test), outcome) in job.covers().into_iter().zip(outcomes) {
                let _ = slots[file][test].set(outcome);
            }
        }
        Err(e) => {
            let _ = errors[id].set(e);
        }
    }
}

fn describe(plans: &[FilePlan], job: &Job) -> String {
    match job {
        Job::AotDriver { file, tests } => {
            format!("{} ({} test(s), aot)", plans[*file].display, tests.len())
        }
        Job::AotPanics { file, test } => {
            format!("{} (aot, panics)", plans[*file].tests[*test].name)
        }
        Job::Vm { file, test } => format!("{} (vm)", plans[*file].tests[*test].name),
    }
}

// ---------------------------------------------------------------
// The IR VM lane
// ---------------------------------------------------------------

thread_local! {
    /// Files this worker has already parsed and type-checked.
    ///
    /// A checked program holds `Rc`, so it cannot be shared between
    /// workers; each one prepares a file the first time it takes a
    /// test from it. With `T` workers and `F` files the front end runs
    /// at most `T * F` times instead of `F` — ~40 ms a file for
    /// `poc/logsearch`, against seconds of tests. That is the price of
    /// an AST that is not `Send`, and it is worth paying to schedule
    /// a test at a time.
    static PREPARED: std::cell::RefCell<HashMap<PathBuf, std::rc::Rc<interpreter::PreparedTests>>> =
        std::cell::RefCell::new(HashMap::new());
}

fn prepared_for(
    pkg: &Package,
    path: &Path,
    display: &str,
    opts: &Options,
) -> Result<std::rc::Rc<interpreter::PreparedTests>, String> {
    if let Some(hit) = PREPARED.with(|c| c.borrow().get(path).cloned()) {
        return Ok(hit);
    }
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
    let mut options = RunOptions::default();
    options.core_modules_dirs = &pkg.module_roots;
    options.diagnostics_json = opts.diagnostics_json;
    let prepared =
        std::rc::Rc::new(interpreter::prepare_tests(&source, display, &options)?);
    PREPARED.with(|c| c.borrow_mut().insert(path.to_path_buf(), prepared.clone()));
    Ok(prepared)
}

/// One test on the IR VM, in this process.
fn run_vm_one(
    pkg: &Package,
    plan: &FilePlan,
    opts: &Options,
    test: usize,
) -> Result<Outcome, String> {
    let planned = &plan.tests[test];
    let prepared = prepared_for(pkg, &plan.path, &plan.display, opts)?;
    // The sink is thread-local, so two workers printing at once keep
    // their output apart (`interpreter::output` was built that way
    // precisely because an OS-level redirect would not).
    let (outcome, printed) =
        interpreter::output::with_capture(|| prepared.run_one(planned.index));
    Ok(Outcome {
        name: planned.name.clone(),
        file: planned.file.clone(),
        line: planned.line,
        failure: outcome.failure,
        output: printed,
    })
}

// ---------------------------------------------------------------
// The compiled lane
// ---------------------------------------------------------------

/// Compile `file` with a test entry, optionally holding one test.
fn compile_driver(
    pkg: &Package,
    plan: &FilePlan,
    opts: &Options,
    only: Option<&[String]>,
    single: Option<&str>,
) -> Result<PathBuf, String> {
    let profile = crate::package::Profile::of(opts.release);
    let exe = pkg.test_exe_path(profile, &plan.path, single);
    if let Some(parent) = exe.parent() {
        pkg.ensure_dir(parent)?;
    }
    let mut options = compiler::options::CompilerOptions::new(plan.path.clone());
    options.output = Some(exe.clone());
    options.release = opts.release;
    options.core_modules_dirs = pkg.module_roots.clone();
    options.link_cache_dir = Some(pkg.link_cache_dir());
    options.test_mode = true;
    options.test_only = only.map(|names| names.to_vec());
    options.diagnostics_json = opts.diagnostics_json;
    compiler::compile_file(&options)?;
    Ok(exe)
}

/// `TOY_BLESS` for the compiled lane, which is a child process.
fn bless_env(opts: &Options) -> Vec<(String, String)> {
    if opts.bless {
        vec![("TOY_BLESS".to_string(), "1".to_string())]
    } else {
        Vec::new()
    }
}

/// A single `panics` test, in a binary of its own.
fn run_aot_panics(
    pkg: &Package,
    plan: &FilePlan,
    opts: &Options,
    test: usize,
) -> Result<Outcome, String> {
    let planned = &plan.tests[test];
    let names = [planned.name.clone()];
    let exe = compile_driver(pkg, plan, opts, Some(&names), Some(&planned.name))?;
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
        Some(format!("expected `{}` to panic, but it returned", planned.name))
    } else {
        match &planned.expect_panic {
            Some(Some(wanted)) if !text.contains(wanted.as_str()) => Some(format!(
                "expected a panic containing `{wanted}`, but it said:\n{text}"
            )),
            _ => None,
        }
    };
    Ok(Outcome {
        name: planned.name.clone(),
        file: planned.file.clone(),
        line: planned.line,
        failure,
        output: String::from_utf8_lossy(&run.stdout).into_owned(),
    })
}

/// The tests that do not expect a panic, in one driver.
///
/// **The lane stops at the first failure.** An assertion failure is a
/// panic and a panic ends the process, so the marker stream tells us
/// which test was running when it died and everything after it never
/// ran. Reporting every failure in one pass needs a process per test
/// (`TEST_PARALLEL.md` D1's shape); this is the cheap form, and the
/// report says which tests did not get a turn.
fn run_aot_driver(
    pkg: &Package,
    plan: &FilePlan,
    opts: &Options,
    tests: &[usize],
) -> Result<Vec<Outcome>, String> {
    // Name the ones to include rather than the ones to skip: the
    // driver runs a set, and "everything except the panicking tests"
    // is that set.
    let names: Vec<String> = tests.iter().map(|t| plan.tests[*t].name.clone()).collect();
    let exe = compile_driver(pkg, plan, opts, Some(&names), None)?;
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
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();

    // The driver never reached its first test. Whatever it said on
    // the way out is the only evidence there is, and it used to be
    // dropped: every test read "not run: an earlier test ended the
    // process", naming an earlier test that does not exist. A flake
    // of this shape cost an afternoon because the report withheld
    // the reason.
    let died_at_startup = started.is_empty() && !ok;
    let startup_failure = || {
        let code = out
            .status
            .code()
            .map(|c| format!("exit code {c}"))
            .unwrap_or_else(|| "killed by a signal".to_string());
        if failure_text.trim().is_empty() {
            format!("the test driver ended before its first test ({code}), saying nothing")
        } else {
            format!("the test driver ended before its first test ({code}):\n{failure_text}")
        }
    };

    let mut result = Vec::with_capacity(tests.len());
    for t in tests {
        let planned = &plan.tests[*t];
        let position = started.iter().position(|s| *s == planned.name);
        let failure = match position {
            None if died_at_startup => Some(startup_failure()),
            // Never started: an earlier test ended the process.
            None => Some("not run: an earlier test ended the process".to_string()),
            // Started, and it is the last one, and we died: this is it.
            Some(i) if !ok && i + 1 == started.len() => Some(failure_text.clone()),
            Some(_) => None,
        };
        result.push(Outcome {
            name: planned.name.clone(),
            file: planned.file.clone(),
            line: planned.line,
            failure,
            // One driver's output belongs to all of its tests; there
            // is no marker on stdout to cut it by. Shown only when
            // something in it failed.
            output: printed.clone(),
        });
    }
    Ok(result)
}

/// Paths relative to the package root, so a report is the same
/// wherever the package is checked out.
fn display_path(pkg: &Package, file: &Path) -> String {
    file.strip_prefix(&pkg.root)
        .unwrap_or(file)
        .to_string_lossy()
        .into_owned()
}

fn clone_outcome(o: &Outcome) -> Outcome {
    Outcome {
        name: o.name.clone(),
        file: o.file.clone(),
        line: o.line,
        failure: o.failure.clone(),
        output: o.output.clone(),
    }
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
        // What the test printed before it died, held back until now so
        // that workers running at once cannot interleave their output.
        if !o.output.trim().is_empty() {
            eprintln!("    --- output ---");
            for line in o.output.lines() {
                eprintln!("    {line}");
            }
        }
    }
    let passed = outcomes.len() - failed.len();
    println!(
        "{passed} passed, {} failed   {:.2} s",
        failed.len(),
        elapsed.as_secs_f64()
    );
}

/// The machine form. Hand-written rather than pretty-printed through
/// serde like the other `--format=json` documents: one record per line
/// is a shape readers depend on (a failing test is one `grep` away).
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
