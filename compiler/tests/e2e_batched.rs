//! Batched e2e test prototype — combines several smaller e2e
//! sources into a single compiled-once-and-run-once binary, then
//! asserts every sub-test passed. The goal is to amortise the
//! ~300 ms per-fresh-binary first-execve cost macOS imposes (see
//! `compiler/examples/profile_e2e.rs` for the underlying
//! profiling) across N sub-tests so the wall-clock for the
//! batched fixture is one `spawn`-cost rather than N.
//!
//! ## What this proves (and what it doesn't)
//!
//! - **Speedup**: this single test runs ~10 sub-programs through
//!   the AOT compiler in well under 1 second (one cold spawn,
//!   ~300 ms). Doing the same 10 programs through the existing
//!   per-test `e2e.rs` runner spends ~3 seconds (10 cold spawns).
//! - **Coverage scope**: only sub-tests with no top-level
//!   `struct` / `enum` declarations and no `panic` / early-exit
//!   semantics work for now. Their `fn main() -> u64` bodies get
//!   renamed to per-subtest `fn __t<i>_main()` and concatenated;
//!   a generated meta-`main` calls each in turn, returns 0 on
//!   all-pass or the first-failed sub-test index on any miss.
//! - **Per-test reporting**: nextest sees this as one test;
//!   on failure the assertion message names the first sub-test
//!   that returned an unexpected value. The original per-test
//!   `e2e.rs` runner stays for granular debugging.
//!
//! ## Future work
//!
//! - Auto-mangle `struct` / `enum` declarations so tests with
//!   compound types can be batched too. (Need a real mini-rewriter
//!   rather than the substring substitution this prototype uses.)
//! - Move all 193 e2e sources into the batched harness so the
//!   per-test `e2e.rs` becomes opt-in for debugging (skip flag).
//! - cranelift-jit in-process loader (the real fix from
//!   `compiler/README.md`'s future-work list).

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Instant;

use compiler::{compile_file, CompilerOptions, EmitKind};

/// Borrowed from `e2e.rs`. Same skip semantics so the batched
/// fixture honours `COMPILER_E2E=skip` for environments that
/// can't run the AOT pipeline.
fn skip_e2e() -> bool {
    std::env::var("COMPILER_E2E").map(|v| v == "skip").unwrap_or(false)
}

fn core_modules_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate has a workspace parent")
        .join("core")
}

fn unique_path(stem: &str) -> PathBuf {
    static COUNTER: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    let n = COUNTER
        .get_or_init(|| std::sync::atomic::AtomicU64::new(0))
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(".toy_e2e_batched_{stem}_{n}"))
}

/// One sub-test in the batched fixture. `source` must define a
/// `fn main() -> u64` that returns a value the meta-main can
/// compare against `expected`. No top-level `struct` / `enum`
/// declarations (yet) — the prototype's `rename_main` pass only
/// handles function-name mangling.
struct SubTest {
    name: &'static str,
    source: &'static str,
    expected: u64,
}

/// Rewrite a sub-test's `fn main() -> u64` to a per-test entry
/// point. Bare-string substitution is enough for the prototype
/// since none of the chosen sub-tests reference `main` from
/// inside their bodies. Future work would parse + rewrite the
/// AST instead.
fn rename_main(source: &str, idx: usize) -> String {
    let new_name = format!("__t{idx}_main");
    source.replace("fn main", &format!("fn {new_name}"))
}

/// Concatenate every sub-test's renamed source, then append a
/// generated `fn main() -> u64` that calls each sub-test entry,
/// compares to the expected value, and returns the index of the
/// first failure (1-indexed) or 0 on all-pass. The 1-indexed
/// scheme leaves 0 free as "all green" since the language's
/// `main` exit-code is `u64`.
fn build_batched_source(tests: &[SubTest]) -> String {
    let mut out = String::with_capacity(tests.iter().map(|t| t.source.len()).sum::<usize>() + 1024);

    for (i, t) in tests.iter().enumerate() {
        out.push_str(&format!("# subtest {} = {}\n", i + 1, t.name));
        out.push_str(&rename_main(t.source, i));
        out.push('\n');
    }

    out.push_str("\nfn main() -> u64 {\n");
    for (i, t) in tests.iter().enumerate() {
        // Each subtest call: if the result doesn't match the
        // expected value, return the 1-indexed test number so
        // the assertion message can name it.
        out.push_str(&format!(
            "    if __t{i}_main() != {expected}u64 {{ return {one_indexed}u64 }}\n",
            i = i,
            expected = t.expected,
            one_indexed = i + 1,
        ));
    }
    out.push_str("    0u64\n");
    out.push_str("}\n");

    out
}

fn compile_and_run_batched(tests: &[SubTest]) -> (i32, std::time::Duration, std::time::Duration) {
    let combined = build_batched_source(tests);
    let src_path = unique_path("batched.t");
    std::fs::write(&src_path, &combined).expect("write batched source");
    let exe_path = unique_path("batched");

    let opts = CompilerOptions {
        input: src_path.clone(),
        output: Some(exe_path.clone()),
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
        core_modules_dir: Some(core_modules_dir()),
    };

    let t_compile = Instant::now();
    compile_file(&opts).expect("batched compile_file failed");
    let compile_dur = t_compile.elapsed();

    let t_run = Instant::now();
    let status = Command::new(&exe_path)
        .status()
        .expect("spawn batched executable");
    let run_dur = t_run.elapsed();

    let code = status.code().expect("batched: no exit code");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    (code, compile_dur, run_dur)
}

#[test]
fn batched_smoke_runs_ten_subtests_in_one_spawn() {
    if skip_e2e() {
        return;
    }
    // Hand-picked sub-tests that don't introduce top-level
    // declarations (no struct / enum / trait), so the prototype's
    // string-replace renamer suffices.
    let tests: &[SubTest] = &[
        SubTest {
            name: "literal_42",
            source: "fn main() -> u64 { 42u64 }\n",
            expected: 42,
        },
        SubTest {
            name: "fib_8",
            source: "fn fib(n: u64) -> u64 { if n <= 1u64 { n } else { fib(n - 1u64) + fib(n - 2u64) } }\n\
                     fn main() -> u64 { fib(8u64) }\n",
            expected: 21,
        },
        SubTest {
            name: "for_loop_sum_0_to_9",
            source: "fn main() -> u64 {\n    var sum = 0u64\n    for i in 0u64..10u64 {\n        sum = sum + i\n    }\n    sum\n}\n",
            expected: 45,
        },
        SubTest {
            name: "elif_chain",
            source: "fn classify(x: u64) -> u64 {\n    if x < 10u64 { 1u64 } elif x < 20u64 { 2u64 } else { 3u64 }\n}\n\
                     fn main() -> u64 { classify(5u64) + classify(15u64) * 10u64 + classify(25u64) * 100u64 }\n",
            expected: 1 + 2 * 10 + 3 * 100,
        },
        SubTest {
            name: "short_circuit_and",
            source: "fn main() -> u64 {\n    val a: bool = true\n    val b: bool = false\n    if a && b { 1u64 } else { 0u64 }\n}\n",
            expected: 0,
        },
        SubTest {
            name: "short_circuit_or",
            source: "fn main() -> u64 {\n    val a: bool = false\n    val b: bool = true\n    if a || b { 7u64 } else { 0u64 }\n}\n",
            expected: 7,
        },
        SubTest {
            name: "match_literal_u64",
            source: "fn main() -> u64 {\n    val n: u64 = 2u64\n    match n {\n        0u64 => 10u64,\n        1u64 => 20u64,\n        2u64 => 30u64,\n        _ => 99u64,\n    }\n}\n",
            expected: 30,
        },
        SubTest {
            name: "while_break",
            source: "fn main() -> u64 {\n    var i = 0u64\n    while i < 100u64 {\n        if i >= 7u64 { break }\n        i = i + 1u64\n    }\n    i\n}\n",
            expected: 7,
        },
        SubTest {
            name: "f64_arith_and_cast",
            source: "fn main() -> u64 {\n    val x: f64 = 3.5f64\n    val y: f64 = 2.0f64\n    val z: f64 = x * y + 0.5f64\n    z as u64\n}\n",
            expected: 7,
        },
        SubTest {
            name: "i64_to_u64_negate",
            source: "fn main() -> u64 {\n    val n: i64 = -5i64\n    val m: i64 = 0i64 - n\n    m as u64\n}\n",
            expected: 5,
        },
    ];

    let (code, compile_dur, run_dur) = compile_and_run_batched(tests);
    eprintln!(
        "batched e2e: {} sub-tests, compile {:?}, spawn+run {:?}",
        tests.len(),
        compile_dur,
        run_dur
    );
    if code != 0 {
        let failed = tests
            .get((code - 1) as usize)
            .map(|t| t.name)
            .unwrap_or("<unknown>");
        panic!(
            "batched e2e: sub-test #{code} ({failed}) returned an unexpected value",
        );
    }
}
