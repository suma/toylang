//! COMPILER_DEV_LOOP D6 — `--all-backends` and stdin input.
//!
//! Both features exist to remove round trips from the loop, so what is
//! pinned is the round-trip-shaped behaviour: one command runs every
//! backend and says nothing when they agree, a program can be handed
//! over a pipe instead of through a file, and a backend that cannot run
//! the program is reported rather than quietly counted as agreeing.
//!
//! Driven through the binary rather than the library because the
//! subject under test is the command: flag parsing, what lands on
//! stdout versus stderr, and the exit code.
//!
//! Set `COMPILER_E2E=skip` to opt out, same as the other e2e suites —
//! these link a native binary.

use std::io::Write;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_compiler");

fn skip_e2e() -> bool {
    std::env::var("COMPILER_E2E").map(|v| v == "skip").unwrap_or(false)
}

fn core_modules_dir() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../core").to_string()
}

struct Run {
    status: i32,
    stdout: String,
    stderr: String,
}

/// Feed `source` to the compiler on stdin with the given flags.
fn run_stdin(source: &str, extra: &[&str]) -> Run {
    let mut child = Command::new(BIN)
        .arg("-")
        .args(extra)
        .arg("--core-modules")
        .arg(core_modules_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn compiler");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(source.as_bytes())
        .expect("write source");
    let out = child.wait_with_output().expect("wait");
    Run {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn agreeing_backends_report_one_line() {
    if skip_e2e() {
        return;
    }
    let run = run_stdin(
        "fn main() -> u64 {\n    println(\"hello\")\n    7u64\n}\n",
        &["--all-backends"],
    );
    assert_eq!(run.status, 0, "stderr: {}", run.stderr);
    // The program's output goes to stdout exactly once, so the flag
    // composes with a redirect the way a plain run does.
    assert_eq!(run.stdout, "hello\n", "stderr: {}", run.stderr);
    // The verdict is one line, on stderr.
    assert_eq!(run.stderr.lines().count(), 1, "stderr: {}", run.stderr);
    assert!(run.stderr.contains("all 3 backends agree"), "stderr: {}", run.stderr);
    assert!(run.stderr.contains("exit=7"), "stderr: {}", run.stderr);
}

#[test]
fn a_backend_that_cannot_run_the_program_is_reported() {
    if skip_e2e() {
        return;
    }
    // `%` on f64 is interpreter-only (cranelift has no native fmod), so
    // both compiled backends refuse it. Silence here would look like
    // agreement while two thirds of the check had not run.
    let run = run_stdin(
        "fn main() -> u64 {\n    val d = 7.0f64 % 2.0f64\n    0u64\n}\n",
        &["--all-backends"],
    );
    assert_ne!(run.status, 0, "stderr: {}", run.stderr);
    assert!(
        run.stderr.contains("could not run the program"),
        "stderr: {}",
        run.stderr
    );
}

#[test]
fn a_program_that_does_not_type_check_stops_before_comparing() {
    if skip_e2e() {
        return;
    }
    let run = run_stdin(
        "fn main() -> u64 {\n    val x: bool = 1u64\n    0u64\n}\n",
        &["--all-backends"],
    );
    assert_ne!(run.status, 0);
    assert!(
        run.stderr.contains("nothing to compare against"),
        "stderr: {}",
        run.stderr
    );
}

#[test]
fn stdin_works_for_an_ordinary_compile() {
    if skip_e2e() {
        return;
    }
    // The dash is an input, not an unknown flag — the whole point is
    // that a throwaway program needs no throwaway file.
    let out = std::env::temp_dir().join(format!("toy_stdin_cli_{}", std::process::id()));
    let mut child = Command::new(BIN)
        .args(["-", "-o"])
        .arg(&out)
        .arg("--core-modules")
        .arg(core_modules_dir())
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn compiler");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"fn main() -> u64 { 5u64 }\n")
        .expect("write");
    let result = child.wait_with_output().expect("wait");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let status = Command::new(&out).status().expect("run compiled binary");
    let _ = std::fs::remove_file(&out);
    assert_eq!(status.code(), Some(5));
}

// --- `--format=json`: the result as one document ------------------------

fn parse_json(run: &Run) -> serde_json::Value {
    serde_json::from_str(&run.stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}):\n{}\nstderr: {}", run.stdout, run.stderr))
}

#[test]
fn all_backends_as_json_carries_the_output_inside_the_document() {
    if skip_e2e() {
        return;
    }
    let run = run_stdin(
        "fn main() -> u64 {\n    println(\"hello\")\n    7u64\n}\n",
        &["--all-backends", "--format=json"],
    );
    assert_eq!(run.status, 0, "stderr: {}", run.stderr);
    // The program's stdout moves into the document; a raw copy in
    // front of it would make stdout unparseable.
    let doc = parse_json(&run);
    assert_eq!(doc["agree"], true);
    assert_eq!(doc["exit"], 7);
    assert_eq!(doc["stdout"], "hello\n");
    let names: Vec<&str> =
        doc["backends"].as_array().unwrap().iter().map(|b| b["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["interpreter", "jit", "aot"]);
    assert!(doc["backends"].as_array().unwrap().iter().all(|b| b["status"] == "ok"));
    assert!(run.stderr.is_empty(), "stderr: {}", run.stderr);
}

#[test]
fn a_backend_that_cannot_run_the_program_is_a_failed_entry_in_json() {
    if skip_e2e() {
        return;
    }
    let run = run_stdin(
        "fn main() -> u64 {\n    val d = 7.0f64 % 2.0f64\n    0u64\n}\n",
        &["--all-backends", "--format", "json"],
    );
    assert_ne!(run.status, 0);
    let doc = parse_json(&run);
    assert_eq!(doc["agree"], false);
    assert!(
        doc["backends"].as_array().unwrap().iter().any(|b| b["status"] == "failed" && b["error"].is_string()),
        "{doc:#}"
    );
    assert!(!doc["problems"].as_array().unwrap().is_empty(), "{doc:#}");
}

#[test]
fn a_build_as_json_names_what_it_wrote() {
    if skip_e2e() {
        return;
    }
    let out = std::env::temp_dir().join(format!("toy_format_json_{}.ir", std::process::id()));
    let run = run_stdin(
        "fn main() -> u64 { 0u64 }\n",
        &["--emit", "ir", "-o", out.to_str().unwrap(), "--format=json"],
    );
    let written = out.exists();
    let _ = std::fs::remove_file(&out);
    assert_eq!(run.status, 0, "stderr: {}", run.stderr);
    let doc = parse_json(&run);
    assert_eq!(doc["emit"], "ir");
    assert_eq!(doc["output"], out.to_str().unwrap());
    assert!(written);
}

/// HEAP-CHECK H0: `--heap-check=report` counts double frees on every
/// backend and compares the reports like the memory profile. A block
/// freed twice is reported by (allocated, first freed, freed again), and
/// a block a resize moved says so; a program with no double free still
/// gets its "0" line.
#[test]
fn heap_check_report_names_each_double_free_on_every_backend() {
    if skip_e2e() {
        return;
    }
    let source = "\
fn main() -> u64 {
    val p: ptr = __builtin_heap_alloc(16u64)
    __builtin_heap_free(p)
    __builtin_heap_free(p)
    val q: ptr = __builtin_heap_alloc(8u64)
    val r: ptr = __builtin_heap_realloc(q, 64u64)
    __builtin_heap_free(q)
    __builtin_heap_free(r)
    0u64
}
";
    let run = run_stdin(source, &["--all-backends", "--heap-check=report"]);
    assert_eq!(run.status, 0, "stderr: {}", run.stderr);
    assert!(
        run.stderr.contains(
            "heap check: 2 double frees (2 distinct)\n\
             \x20 x1  in main: allocated at <stdin>:2:18, freed at <stdin>:3:5, freed again at <stdin>:4:5\n\
             \x20 x1  in main: allocated at <stdin>:5:18, moved by a resize, freed again at <stdin>:7:5\n"
        ),
        "stderr: {}",
        run.stderr
    );
    assert!(run.stderr.contains("all 3 backends agree"), "stderr: {}", run.stderr);

    let clean = run_stdin("fn main() -> u64 {\n    0u64\n}\n", &["--all-backends", "--heap-check=report"]);
    assert!(
        clean.stderr.contains("heap check: 0 double frees (0 distinct)\n"),
        "stderr: {}",
        clean.stderr
    );
}

/// HEAP-CHECK H2: poison stops the process, which the in-process JIT
/// lane cannot survive, so `--all-backends` points at the two ways
/// that work. A quarantine size means nothing without reuse mode.
#[test]
fn heap_check_modes_not_available_are_named() {
    let run = run_stdin("fn main() -> u64 {\n    0u64\n}\n", &["--all-backends", "--heap-check=poison"]);
    assert_ne!(run.status, 0);
    assert!(run.stderr.contains("interpreter --heap-check=poison"), "stderr: {}", run.stderr);
    let run = run_stdin(
        "fn main() -> u64 {\n    0u64\n}\n",
        &["--all-backends", "--heap-check=report", "--heap-quarantine=0"],
    );
    assert_ne!(run.status, 0);
    assert!(run.stderr.contains("--heap-check=reuse only"), "stderr: {}", run.stderr);
}

/// HEAP-CHECK H3: under `--all-backends`, reuse mode runs every lane
/// with the same recycling policy and compares how many blocks each
/// recycled.
#[test]
fn heap_check_reuse_runs_every_lane_and_compares_the_reuse() {
    if skip_e2e() {
        return;
    }
    let source = "\
fn main() -> u64 {
    val a: ptr = __builtin_heap_alloc(16u64)
    __builtin_heap_free(a)
    val b: ptr = __builtin_heap_alloc(16u64)
    if __builtin_ptr_eq(a, b) { 1u64 } else { 0u64 }
}
";
    let run = run_stdin(source, &["--all-backends", "--heap-check=reuse", "--heap-quarantine=0"]);
    assert_eq!(run.status, 0, "stderr: {}", run.stderr);
    assert!(run.stderr.contains("1 blocks reused"), "stderr: {}", run.stderr);
    assert!(run.stderr.contains("all 3 backends agree (exit=1)"), "stderr: {}", run.stderr);
}

/// HEAP-CHECK H0b: a double free is reported under the function that
/// set it off -- the innermost frame that is not a `Drop` impl or drop
/// glue -- because in a real program the free itself is usually in a
/// `drop` (`Box::drop`), which names the type but not the code.
#[test]
fn heap_check_names_the_function_behind_a_double_free() {
    if skip_e2e() {
        return;
    }
    let source = "\
fn release(p: ptr) {
    __builtin_heap_free(p)
}
fn main() -> u64 {
    val p: ptr = __builtin_heap_alloc(16u64)
    release(p)
    release(p)
    0u64
}
";
    let run = run_stdin(source, &["--all-backends", "--heap-check=report"]);
    assert!(
        run.stderr.contains(
            "heap check: 1 double frees (1 distinct)\n\
             \x20 x1  in release: allocated at <stdin>:5:18, freed at <stdin>:2:5, freed again at <stdin>:2:5\n"
        ),
        "stderr: {}",
        run.stderr
    );
    assert!(run.stderr.contains("all 3 backends agree"), "stderr: {}", run.stderr);
}

/// DOUBLE-DROP-LANE-DIVERGENCE: the Box list and tree examples free each
/// node once on the lanes that share the lowering too (they matched a
/// borrowed `&List` and freed its payloads).
#[test]
fn box_examples_free_each_node_once() {
    if skip_e2e() {
        return;
    }
    for example in ["box_linked_list.t", "box_binary_tree.t"] {
        let path = format!("{}/../interpreter/example/{example}", env!("CARGO_MANIFEST_DIR"));
        let out = Command::new(BIN)
            .arg(&path)
            .args(["--all-backends", "--heap-check=report", "--core-modules"])
            .arg(core_modules_dir())
            .output()
            .expect("spawn compiler");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("heap check: 0 double frees (0 distinct)\n"), "{example}: {stderr}");
        assert!(stderr.contains("all 3 backends agree"), "{example}: {stderr}");
    }
}
