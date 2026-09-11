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
use crate::test_times::{History, Lane};

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
    /// How long it took, for the next run's schedule (P4). Not
    /// reported: a per-test time in the output would make `-j1` and
    /// `-jN` disagree byte for byte, which is the one property the
    /// whole design rests on.
    took: std::time::Duration,
}

/// One planned test: what the plan pass learned about a `test` block
/// before anything ran.
struct Planned {
    name: String,
    file: String,
    line: u32,
    expect_panic: Option<Option<String>>,
    /// Position among the declaring file's blocks, which is how the
    /// IR VM lane names the one to run.
    index: usize,
}

/// One source file and the tests it contributes to this run.
struct FilePlan {
    path: PathBuf,
    display: String,
    tests: Vec<Planned>,
    /// What parsing and type-checking this file cost in the plan pass.
    /// It is the price of *splitting* the file across workers, since
    /// each one that takes a test from it has to check it again (P4).
    front_end: std::time::Duration,
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
    /// Every test in one file on the IR VM, checked once. Chosen when
    /// the file's tests are cheaper than checking it (see
    /// [`split_pays`]).
    VmFile { file: usize, tests: Vec<usize> },
}

impl Job {
    /// Which planned tests this job answers for, as `(file, test)`.
    fn covers(&self) -> Vec<(usize, usize)> {
        match self {
            Job::AotDriver { file, tests } | Job::VmFile { file, tests } => {
                tests.iter().map(|t| (*file, *t)).collect()
            }
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

    if opts.list_only {
        let mut count = 0usize;
        for plan in &plans {
            for test in &plan.tests {
                println!("{}  ({}:{})", test.name, test.file, test.line);
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
type Listing = (Vec<interpreter::TestCaseInfo>, std::time::Duration);

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
) -> Result<(Vec<interpreter::TestCaseInfo>, std::time::Duration), String> {
    if opts.verbose {
        eprintln!("toy: planning {}", file.display());
    }
    let started = std::time::Instant::now();
    let display = display_path(pkg, file);
    let keep = !opts.aot && !opts.list_only && opts.jobs.max(1) == 1;
    if keep {
        let cases = prepared_for(pkg, file, &display)?.cases().to_vec();
        return Ok((cases, started.elapsed()));
    }
    let source = std::fs::read_to_string(file)
        .map_err(|e| format!("cannot read `{}`: {e}", file.display()))?;
    let mut options = RunOptions::default();
    options.core_modules_dirs = &pkg.module_roots;
    let cases = interpreter::list_tests_from_source(&source, &display, &options)?
        .into_iter()
        .map(|o| interpreter::TestCaseInfo {
            name: o.name,
            line: o.line,
            file: o.file,
            expect_panic: o.expect_panic,
        })
        .collect();
    Ok((cases, started.elapsed()))
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
    for (file, (cases, front_end)) in files.iter().zip(listed) {
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
                index,
            });
        }
        plans.push(FilePlan {
            path: file.clone(),
            display,
            tests,
            front_end,
        });
    }
    Ok(plans)
}

// ---------------------------------------------------------------
// Schedule
// ---------------------------------------------------------------

/// Cut the plan into jobs and run them, `opts.jobs` at a time.
fn execute(pkg: &Package, plans: &[FilePlan], opts: &Options) -> Result<Vec<Outcome>, String> {
    // What the last run cost. A missing or stale file costs a worse
    // schedule and nothing else (P4).
    let times_path =
        crate::test_times::path_for(&pkg.profile_dir(crate::package::Profile::of(opts.release)));
    let history = crate::test_times::load(&times_path);
    let jobs = build_jobs(plans, opts, &history);
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

    let workers = opts.jobs.max(1).min(jobs.len());
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
        let jobs_ref = &jobs;
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
    }

    // A job that could not even be compiled is reported the way it was
    // before there were jobs: as the run's error. The first one in job
    // order, so the message does not depend on who lost the race.
    if let Some(error) = errors.iter().find_map(|e| e.get()) {
        return Err(error.clone());
    }

    let mut measured = History::default();
    for (file, plan) in plans.iter().enumerate() {
        if opts.aot && plan.tests.iter().any(|t| t.expect_panic.is_none()) {
            // A driver's cost is the file's: one compile, one link,
            // one process for every test in it.
            if let Some(outcome) = plan
                .tests
                .iter()
                .enumerate()
                .find(|(_, t)| t.expect_panic.is_none())
                .and_then(|(i, _)| slots[file][i].get())
            {
                measured.record_driver(plan.display.clone(), outcome.took);
            }
        }
    }

    let mut out = Vec::new();
    for (file, plan) in plans.iter().enumerate() {
        for (test, planned) in plan.tests.iter().enumerate() {
            match slots[file][test].get() {
                Some(outcome) => {
                    // A driver's time belongs to the file, not to each
                    // of its tests: recording it per test would make
                    // every one of them look like the whole binary.
                    let per_test = !opts.aot || planned.expect_panic.is_some();
                    if per_test {
                        let lane = if opts.aot { Lane::Aot } else { Lane::Vm };
                        measured.record_test(lane, key_of(planned), outcome.took);
                    }
                    out.push(clone_outcome(outcome))
                }
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
                    took: std::time::Duration::ZERO,
                }),
            }
        }
    }

    // Merge rather than replace: a filtered run measures a handful of
    // tests, and forgetting the rest would leave the next full run
    // scheduling blind.
    let mut history = history;
    history.absorb(measured);
    if !history.is_empty() {
        crate::test_times::save(&times_path, &history);
    }
    Ok(out)
}

/// One job per driver on the compiled lane, one per test on the VM —
/// unless the last run says splitting a file cannot pay.
///
/// **Splitting a file is not free.** A checked program holds `Rc`, so
/// every worker that takes a test from a file checks that file again;
/// cutting a file into `n` jobs can cost up to `n - 1` extra front
/// ends. It pays when the tests are slower than the front end and
/// loses when they are not — a nine-file suite of millisecond tests
/// spent all of its time re-checking. The last run measured both
/// numbers, so the question is answered rather than guessed. With no
/// history, split: that is the shape that makes a slow suite fast,
/// and one run later the answer is known.
fn build_jobs(plans: &[FilePlan], opts: &Options, history: &History) -> Vec<Job> {
    let mut jobs = Vec::new();
    for (file, plan) in plans.iter().enumerate() {
        if plan.tests.is_empty() {
            continue;
        }
        if !opts.aot {
            if split_pays(plan, history) {
                for test in 0..plan.tests.len() {
                    jobs.push(Job::Vm { file, test });
                }
            } else {
                jobs.push(Job::VmFile {
                    file,
                    tests: (0..plan.tests.len()).collect(),
                });
            }
            continue;
        }
        let mut plain = Vec::new();
        for (test, planned) in plan.tests.iter().enumerate() {
            if planned.expect_panic.is_some() {
                jobs.push(Job::AotPanics { file, test });
            } else {
                plain.push(test);
            }
        }
        if !plain.is_empty() {
            jobs.push(Job::AotDriver { file, tests: plain });
        }
    }
    order_jobs(&mut jobs, plans, history);
    jobs
}

/// Is this file's work worth more than checking it again?
fn split_pays(plan: &FilePlan, history: &History) -> bool {
    if plan.tests.len() < 2 {
        return false;
    }
    let mut total = std::time::Duration::ZERO;
    for planned in &plan.tests {
        match history.test(Lane::Vm, &key_of(planned)) {
            Some(took) => total += took,
            // Never measured: assume it is worth splitting rather than
            // decide from a number nobody has.
            None => return true,
        }
    }
    // The front end this run just paid, which is the same work a
    // second worker would have to repeat.
    total > plan.front_end
}

/// Longest job first.
///
/// With a shared cursor the order jobs are handed out in is the whole
/// schedule: start the long one last and every worker waits for it
/// alone. `poc/logsearch` has one test that runs for seconds among
/// thirteen that take milliseconds, which is exactly the shape that
/// punishes plan order.
///
/// A job nobody has timed sorts **first**, not last. An unknown may be
/// the long one, and the two mistakes are not the same size: starting
/// a short job early costs nothing, while starting a long one late
/// costs its whole length. Ties keep plan order, so a first run — when
/// everything is unknown — behaves exactly as it did before.
fn order_jobs(jobs: &mut [Job], plans: &[FilePlan], history: &History) {
    let unknown = std::time::Duration::MAX;
    let estimate = |job: &Job| -> std::time::Duration {
        match job {
            Job::Vm { file, test } => history
                .test(Lane::Vm, &key_of(&plans[*file].tests[*test]))
                .unwrap_or(unknown),
            Job::VmFile { file, tests } => {
                let mut total = plans[*file].front_end;
                for test in tests {
                    match history.test(Lane::Vm, &key_of(&plans[*file].tests[*test])) {
                        Some(took) => total += took,
                        None => return unknown,
                    }
                }
                total
            }
            Job::AotPanics { file, test } => history
                .test(Lane::Aot, &key_of(&plans[*file].tests[*test]))
                .unwrap_or(unknown),
            Job::AotDriver { file, .. } => {
                history.driver(&plans[*file].display).unwrap_or(unknown)
            }
        }
    };
    // Stable, so equal estimates — and the all-unknown first run —
    // keep the order the plan produced. Descending, hence `Reverse`.
    jobs.sort_by_key(|job| std::cmp::Reverse(estimate(job)));
}

fn key_of(planned: &Planned) -> (String, u32, String) {
    (planned.file.clone(), planned.line, planned.name.clone())
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
        Job::VmFile { file, tests } => tests
            .iter()
            .map(|test| run_vm_one(pkg, &plans[*file], opts, *test))
            .collect(),
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
        Job::VmFile { file, tests } => {
            format!("{} ({} test(s), vm)", plans[*file].display, tests.len())
        }
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
) -> Result<std::rc::Rc<interpreter::PreparedTests>, String> {
    if let Some(hit) = PREPARED.with(|c| c.borrow().get(path).cloned()) {
        return Ok(hit);
    }
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
    let mut options = RunOptions::default();
    options.core_modules_dirs = &pkg.module_roots;
    let prepared =
        std::rc::Rc::new(interpreter::prepare_tests(&source, display, &options)?);
    PREPARED.with(|c| c.borrow_mut().insert(path.to_path_buf(), prepared.clone()));
    Ok(prepared)
}

/// One test on the IR VM, in this process.
fn run_vm_one(
    pkg: &Package,
    plan: &FilePlan,
    _opts: &Options,
    test: usize,
) -> Result<Outcome, String> {
    let planned = &plan.tests[test];
    let prepared = prepared_for(pkg, &plan.path, &plan.display)?;
    // The sink is thread-local, so two workers printing at once keep
    // their output apart (`interpreter::output` was built that way
    // precisely because an OS-level redirect would not).
    let started = std::time::Instant::now();
    let (outcome, printed) =
        interpreter::output::with_capture(|| prepared.run_one(planned.index));
    // The block's own time, not the job's: the front end that had to
    // run first is a separate measurement, and conflating them would
    // make every test in a big package look expensive.
    let took = started.elapsed();
    Ok(Outcome {
        name: planned.name.clone(),
        file: planned.file.clone(),
        line: planned.line,
        failure: outcome.failure,
        output: printed,
        took,
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
    let started = std::time::Instant::now();
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
        // Compiling included: on this lane that is most of it, and it
        // is what the next run has to schedule around.
        took: started.elapsed(),
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
    let began = std::time::Instant::now();
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

    // One driver is one compile, one link and one process for all of
    // its tests; there is no marker to cut the time by, so the job's
    // cost belongs to the file rather than to any block in it.
    let took = began.elapsed();
    let mut result = Vec::with_capacity(tests.len());
    for t in tests {
        let planned = &plan.tests[*t];
        let position = started.iter().position(|s| *s == planned.name);
        let failure = match position {
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
            took,
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
        took: o.took,
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
