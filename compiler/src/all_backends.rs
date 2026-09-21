//! Run one program on every backend and report only what disagrees
//! (COMPILER_DEV_LOOP D6).
//!
//! toylang implements the same semantics several times over. Checking a
//! change by hand meant three commands and three outputs, and the
//! comparison — the only part that carries information — was done in
//! the reader's head. The `f` lexical-scoping bug is what that costs:
//! the type checker was fixed, the tree-walker was not, and the program
//! type checked while returning the wrong answer.
//!
//! This runs all three in one command and says nothing when they agree.
//! The program's own stdout still goes to stdout (once, not three
//! times), so the flag composes with a redirect the way a plain run
//! does; the verdict goes to stderr.
//!
//! With `--format=json` the verdict *is* the output: one JSON document
//! on stdout carrying the program's stdout, each backend's result and
//! every disagreement, so a caller reads one thing instead of splitting
//! the program's output from the verdict.
//!
//! **Which three.** The interpreter (tree-walker), this crate's
//! Cranelift JIT, and the AOT compiler. The interpreter crate's *own*
//! JIT is deliberately not one of them: this crate's binary depends on
//! `interpreter` with default features off, which drops that JIT, and
//! `RunOptions::jit` is silently ignored when it is absent — so
//! driving it from here would run the tree-walker a second time and
//! report a backend agreeing with itself. (The cross-backend test
//! suites do use it, and ask for the feature explicitly in
//! `[dev-dependencies]` plus assert `interpreter::jit_available()`.)

use std::path::PathBuf;
use std::process::Command;

use crate::{compile_file, CompilerOptions};

/// Whether to profile the run, and in which shape to report it
/// (MEMORY_PROFILING M1 / M4).
///
/// An enum rather than two booleans so "report as JSON, but do not
/// profile" cannot be spelled at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileMode {
    Off,
    Text,
    Json,
}

impl ProfileMode {
    fn enabled(self) -> bool {
        !matches!(self, ProfileMode::Off)
    }
}

/// What one backend produced.
struct Outcome {
    /// MEMORY_PROFILING M1. `None` when the backend was not asked for
    /// a profile, or could not produce one.
    memory: Option<interpreter::heap::MemoryStats>,
    /// Per-site totals (MEMORY_PROFILING M2), in source order.
    sites: Vec<(u64, interpreter::heap::SiteStats)>,
    /// Registered allocator layouts (MEMORY_PROFILING M3 residual), in
    /// registration order.
    layouts: Vec<interpreter::heap::AllocatorLayoutReport>,
    /// `None` when `main` returned something that is not a number, so
    /// there is no exit code to compare. Several examples return `bool`
    /// or `str`.
    exit: Option<i32>,
    stdout: String,
}

struct BackendResult {
    name: &'static str,
    /// `Err` carries why the backend could not run it at all — an AOT
    /// gap, a JIT-ineligible construct. That is a finding, not a
    /// disagreement, and is reported as such.
    outcome: Result<Outcome, String>,
}

/// Compare against the reference run, ignoring the exit code when
/// either side has none to give.
fn disagrees(reference: &Outcome, other: &Outcome) -> bool {
    if reference.stdout != other.stdout {
        return true;
    }
    match (reference.exit, other.exit) {
        // A process exit code is one byte; the interpreter's is not
        // truncated, so compare on the byte both can represent.
        (Some(a), Some(b)) => a & 0xff != b & 0xff,
        _ => false,
    }
}

/// Run `source` on every backend. Returns the process exit code: 0 when
/// they all ran and agreed. `json` selects the `--format=json` report.
pub fn run(
    options: &CompilerOptions,
    source: &str,
    display_name: &str,
    profile: ProfileMode,
    json: bool,
) -> i32 {
    let interpreter = BackendResult {
        name: "interpreter",
        outcome: run_interpreter(options, source, display_name, profile),
    };
    let jit = BackendResult { name: "jit", outcome: run_jit(options, source, profile) };
    let aot = BackendResult { name: "aot", outcome: run_aot(options, profile) };

    // The interpreter is the reference: it is the most complete
    // implementation and the one whose diagnostics are worth reading
    // when something is wrong.
    let reference = match &interpreter.outcome {
        Ok(o) => o,
        Err(e) => {
            if json {
                let report = serde_json::json!({
                    "agree": false,
                    "exit": null,
                    "stdout": null,
                    "backends": [backend_json(&interpreter, None), backend_json(&jit, None), backend_json(&aot, None)],
                    "problems": ["interpreter could not run the program, so there is nothing to compare against"],
                });
                println!("{}", pretty(&report));
                return 1;
            }
            eprintln!("interpreter could not run the program, so there is nothing to compare against:");
            for line in e.lines() {
                eprintln!("  {line}");
            }
            return 1;
        }
    };

    // The program's output, once. Emitted before the verdict so a
    // redirect captures exactly what a plain run would have. The JSON
    // report carries it inside the document instead.
    if !json {
        print!("{}", reference.stdout);
    }

    let mut problems: Vec<String> = Vec::new();
    for backend in [&jit, &aot] {
        match &backend.outcome {
            Err(e) => problems.push(format!(
                "{} could not run the program:\n{}",
                backend.name,
                indent(e)
            )),
            Ok(o) if disagrees(reference, o) => problems.push(format!(
                "{} disagrees with the interpreter:\n  interpreter: exit={:?} stdout={:?}\n  {:<11}  exit={:?} stdout={:?}",
                backend.name,
                reference.exit,
                reference.stdout,
                backend.name,
                o.exit,
                o.stdout
            )),
            Ok(_) => {}
        }
    }

    // MEMORY_PROFILING M1: the acceptance criterion for the phase is
    // that the strict metrics are identical everywhere, so a mismatch
    // is a failure of the same weight as a wrong answer.
    if profile.enabled() {
        for backend in [&jit, &aot] {
            let Ok(other) = &backend.outcome else { continue };
            let Some(theirs) = other.memory else { continue };
            let leaks_ours = interpreter::heap::MemoryStats::leak_report(&reference.sites);
            let leaks_theirs = interpreter::heap::MemoryStats::leak_report(&other.sites);
            if leaks_ours != leaks_theirs {
                problems.push(format!(
                    "{} attributes leaks differently:\n  interpreter:\n{}  {}:\n{}",
                    backend.name,
                    indent(&leaks_ours),
                    backend.name,
                    indent(&leaks_theirs),
                ));
            }
            let layouts_ours = interpreter::heap::allocator_layout_report_text(&reference.layouts);
            let layouts_theirs = interpreter::heap::allocator_layout_report_text(&other.layouts);
            if layouts_ours != layouts_theirs {
                problems.push(format!(
                    "{} reports different allocator layouts:\n  interpreter:\n{}  {}:\n{}",
                    backend.name,
                    indent(&layouts_ours),
                    backend.name,
                    indent(&layouts_theirs),
                ));
            }
            if let Some(ours) = reference.memory
                && ours != theirs
            {
                problems.push(format!(
                    "{} reports different allocation totals:\n  interpreter:\n{}  {}:\n{}",
                    backend.name,
                    indent(&ours.report()),
                    backend.name,
                    indent(&theirs.report()),
                ));
            }
        }
    }

    if json {
        let report = serde_json::json!({
            "agree": problems.is_empty(),
            "exit": reference.exit,
            "stdout": reference.stdout,
            "backends": [
                backend_json(&interpreter, None),
                backend_json(&jit, Some(reference)),
                backend_json(&aot, Some(reference)),
            ],
            "problems": problems,
        });
        println!("{}", pretty(&report));
        // The allocation report keeps its own flag and its own stream.
        if problems.is_empty() {
            report_memory(reference, profile);
        }
        return if problems.is_empty() { 0 } else { 1 };
    }

    if problems.is_empty() {
        report_memory(reference, profile);
        eprintln!(
            "all 3 backends agree (exit={})",
            reference.exit.map(|c| c.to_string()).unwrap_or_else(|| "n/a".to_string())
        );
        return 0;
    }
    for p in &problems {
        eprintln!("{p}");
    }
    1
}

/// The allocation totals of the agreed run, on stderr in the shape
/// `--format` asked for.
fn report_memory(reference: &Outcome, profile: ProfileMode) {
    let Some(stats) = reference.memory else { return };
    match profile {
        ProfileMode::Json => eprint!("{}", stats.report_json(&reference.sites, &reference.layouts)),
        _ => {
            eprint!("{}", stats.report());
            eprint!("{}", interpreter::heap::MemoryStats::leak_report(&reference.sites));
            eprint!("{}", interpreter::heap::allocator_layout_report_text(&reference.layouts));
        }
    }
}

/// One backend's entry in the JSON report. `reference` is the run it is
/// compared against; `None` for the reference itself.
fn backend_json(backend: &BackendResult, reference: Option<&Outcome>) -> serde_json::Value {
    match &backend.outcome {
        Ok(o) => {
            let status = match reference {
                Some(r) if disagrees(r, o) => "disagrees",
                _ => "ok",
            };
            serde_json::json!({
                "name": backend.name,
                "status": status,
                "exit": o.exit,
                "stdout": o.stdout,
            })
        }
        Err(e) => serde_json::json!({
            "name": backend.name,
            "status": "failed",
            "error": e,
        }),
    }
}

fn pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn indent(text: &str) -> String {
    text.lines().map(|l| format!("  {l}\n")).collect()
}

fn run_interpreter(
    options: &CompilerOptions,
    source: &str,
    display_name: &str,
    profile: ProfileMode,
) -> Result<Outcome, String> {
    let core = crate::resolve_core_modules_dirs(options.core_modules_dirs.clone());
    let mut run_options = interpreter::RunOptions::default();
    run_options.core_modules_dirs = &core;
    if profile.enabled() {
        interpreter::heap::reset_profile();
    }
    let (result, stdout) = interpreter::output::with_capture(|| {
        interpreter::run_source(source, display_name, &run_options)
    });
    let memory = profile.enabled().then(interpreter::heap::profile);
    let sites = if profile.enabled() { interpreter::heap::profile_sites() } else { Vec::new() };
    let layouts = if profile.enabled() { interpreter::heap::allocator_layouts() } else { Vec::new() };
    result
        .map(|outcome| Outcome { exit: outcome.exit_code, stdout, memory, sites, layouts })
        // `run_source` has already rendered the diagnostic to stderr;
        // repeating it here would print the same text twice.
        .map_err(|_| "see the diagnostics above".to_string())
}

fn run_jit(
    options: &CompilerOptions,
    source: &str,
    profile: ProfileMode,
) -> Result<Outcome, String> {
    let program = crate::compile_to_jit_main_with_options(source, options)?;
    if profile.enabled() {
        crate::jit::reset_memory_profile();
    }
    let (exit, stdout) = program.run_capturing_stdout();
    let memory = profile.enabled().then(crate::jit::memory_profile);
    let sites = if profile.enabled() { crate::jit::memory_profile_sites() } else { Vec::new() };
    let layouts = if profile.enabled() { crate::jit::memory_profile_layouts() } else { Vec::new() };
    Ok(Outcome { exit: Some(exit as i32), stdout, memory, sites, layouts })
}

fn run_aot(options: &CompilerOptions, profile: ProfileMode) -> Result<Outcome, String> {
    let exe = temp_path("toy_all_backends");
    let mut aot_options = options.clone();
    aot_options.output = Some(exe.clone());
    aot_options.emit = crate::EmitKind::Executable;
    compile_file(&aot_options)?;
    let mut cmd = Command::new(&exe);
    if profile.enabled() {
        // The compiled runtime writes its report to stderr at exit
        // when this is set; the parent parses it back.
        //
        // Always the text shape, even under `--format=json`:
        // what crosses this boundary is the numbers, and the shape is
        // the driver's choice about its own stderr. Asking the child
        // for JSON would buy a second parser and nothing else. The C
        // runtime's JSON is instead pinned directly, byte for byte,
        // by `aot_json_report_matches_the_shared_one`.
        cmd.env("TOY_PROFILE_MEM", "1");
    }
    let output = cmd
        .output()
        .map_err(|e| format!("could not spawn the compiled binary: {e}"));
    let _ = std::fs::remove_file(&exe);
    let output = output?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let memory = if profile.enabled() { parse_memory_report(&stderr) } else { None };
    let sites = if profile.enabled() { parse_leak_report(&stderr) } else { Vec::new() };
    let layouts = if profile.enabled() { parse_layout_report(&stderr) } else { Vec::new() };
    Ok(Outcome {
        exit: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        memory,
        sites,
        layouts,
    })
}

/// Read back the report the compiled runtime wrote to stderr.
///
/// Deliberately strict: an unrecognised line count or field name
/// yields `None` rather than a partially-filled struct, because a
/// silently-zero metric would compare equal to a backend that
/// legitimately allocated nothing.
fn parse_memory_report(stderr: &str) -> Option<interpreter::heap::MemoryStats> {
    let mut stats = interpreter::heap::MemoryStats::ZERO;
    let mut seen = 0;
    for line in stderr.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(value) = value.parse::<u64>() else {
            continue;
        };
        match name {
            "alloc_count" => stats.alloc_count = value,
            "free_count" => stats.free_count = value,
            "realloc_count" => stats.realloc_count = value,
            "cumulative_bytes" => stats.cumulative_bytes = value,
            "live_bytes" => stats.live_bytes = value,
            "peak_live_bytes" => stats.peak_live_bytes = value,
            "peak_at_request" => stats.peak_at_request = value,
            _ => continue,
        }
        seen += 1;
    }
    (seen == 7).then_some(stats)
}

/// Read the leak section back out of a compiled run's stderr.
///
/// Only the leaking sites appear there, which is exactly what the
/// comparison needs: a site that allocated and freed everything is not
/// a leak on any backend.
fn parse_leak_report(stderr: &str) -> Vec<(u64, interpreter::heap::SiteStats)> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        // `  [<file>:]<line>:<col>  <n> allocations  <b> bytes`
        // The file is optional and may itself contain colons (a path),
        // so the position is split from the right.
        let t = line.trim();
        let Some((pos, rest)) = t.split_once("  ") else {
            continue;
        };
        let Some((pos2, c)) = pos.rsplit_once(':') else {
            continue;
        };
        let (file, l) = match pos2.rsplit_once(':') {
            Some((file, l)) => (Some(file), l),
            None => (None, pos2),
        };
        let (Ok(l), Ok(c)) = (l.parse::<u64>(), c.parse::<u64>()) else {
            continue;
        };
        let mut f = rest.split_whitespace();
        let (Some(count), Some(_), Some(bytes)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let (Ok(count), Ok(bytes)) = (count.parse::<u64>(), bytes.parse::<u64>()) else {
            continue;
        };
        out.push((
            (l << 32) | c,
            interpreter::heap::SiteStats {
                file: file.unwrap_or("").to_string(),
                live_count: count,
                live_bytes: bytes,
                ..Default::default()
            },
        ));
    }
    out
}

/// Read the allocator-layout section back out of a compiled run's
/// stderr (MEMORY_PROFILING M3 residual).
///
/// `  <name>  managed <m>  live <l>  free_blocks <fb>  largest_free <lf>  external_fragmentation <p> permille`
///
/// The external-fragmentation field is ignored on read: it is derived
/// from the four numbers and re-derived on the interpreter side, so it
/// need not be parsed back.
fn parse_layout_report(stderr: &str) -> Vec<interpreter::heap::AllocatorLayoutReport> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        let t = line.trim();
        if !t.contains("  managed ") {
            continue;
        }
        // `name  managed m  live l  free_blocks fb  largest_free lf  external_fragmentation p permille`
        let mut f = t.splitn(2, "  ");
        let name = f.next().unwrap_or("");
        let g = f
            .next()
            .unwrap_or("")
            .split_whitespace();
        // "managed" <m> "live" <l> "free_blocks" <fb> "largest_free" <lf> "external_fragmentation" <p> "permille"
        let values: Vec<u64> = g
            .filter_map(|tok| tok.parse::<u64>().ok())
            .collect();
        if values.len() < 4 {
            continue;
        }
        out.push(interpreter::heap::AllocatorLayoutReport {
            name: name.to_string(),
            managed_bytes: values[0],
            live_bytes: values[1],
            free_blocks: values[2],
            largest_free: values[3],
        });
    }
    out
}

/// A collision-free path in the system temp directory. Process id plus
/// nanoseconds, so parallel invocations do not overwrite each other's
/// binary between the link and the spawn.
pub(crate) fn temp_path(stem: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut p = std::env::temp_dir();
    p.push(format!("{stem}_{}_{nanos}", std::process::id()));
    p
}
