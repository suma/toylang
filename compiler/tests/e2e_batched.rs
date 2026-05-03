//! Batched e2e tests — runs many small toylang sources through
//! the cranelift JIT (`compiler::compile_to_jit_main_with_options`)
//! inside a single `cargo test` worker. No fork, no exec, no
//! linker, no temp files: each sub-test is JIT-compiled into the
//! current process's address space and called via a Rust
//! function pointer.
//!
//! Why this lives separately from `e2e.rs`:
//!
//! - `e2e.rs` exercises the AOT pipeline end-to-end (object
//!   emission + system `cc` link + `Command::new(exe).status()`),
//!   which is what users actually run. Each sub-test in there
//!   pays the macOS first-execve cost of ~300 ms / fresh binary.
//! - This file gives the same codegen coverage but skips both
//!   the link and the spawn. It runs ~150 sub-tests in 2-3
//!   seconds wall-clock total — the equivalent through `e2e.rs`
//!   would be 45+ seconds.
//!
//! Earlier iterations of this file took a different approach
//! (one batched AOT binary per fixture, with a meta-`main` that
//! dispatched to `__t<i>_main` after a textual mangler renamed
//! collision-prone names). That prototype is gone — the
//! cranelift-jit loader from `compiler/README.md`'s future-work
//! list eliminates the spawn cost more cleanly than batching ever
//! could, and per-sub-test isolation means we don't need the
//! mangler at all.
//!
//! Coverage notes:
//!
//! - Exit-code sub-tests live in `EXIT_SUBTESTS` (auto-extracted
//!   from `e2e.rs` patterns; regenerate via the
//!   `dump_extracted` example).
//! - Stdout sub-tests live in `STDOUT_SUBTESTS`. Each runs with
//!   `JitProgram::run_capturing_stdout`, which routes the JIT
//!   runtime's `toy_print_*` helpers into a thread-local buffer.
//! - Patterns the extractor doesn't yet recognise stay in
//!   `e2e.rs`: panic / assert programs (process abort), exit-code
//!   expressions, multi-call tests, stderr assertions.

use std::path::PathBuf;
use std::time::Instant;

use compiler::{compile_to_jit_main_with_options, CompilerOptions, EmitKind};

/// Skip everything when the operator opts out via
/// `COMPILER_E2E=skip`. Originally there because the AOT
/// pipeline needed a working system `cc`; the JIT path doesn't
/// touch `cc` at all, but we keep the env var in case downstream
/// CI still toggles it.
fn skip_e2e() -> bool {
    std::env::var("COMPILER_E2E").map(|v| v == "skip").unwrap_or(false)
}

fn core_modules_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler crate has a workspace parent")
        .join("core")
}

/// One exit-code sub-test. `source` must define a
/// `fn main() -> u64` (or `-> i64`, returning a value `as u64`)
/// whose return value the runner compares against `expected`.
/// Strings are owned so this struct can hold both compile-time
/// literal fixtures and ones loaded from the auto-extracted
/// data table.
struct SubTest {
    name: String,
    source: String,
    expected: u64,
}

impl SubTest {
    fn new(name: impl Into<String>, source: impl Into<String>, expected: u64) -> Self {
        Self {
            name: name.into(),
            source: source.into(),
            expected,
        }
    }
}

/// `CompilerOptions` value with the workspace-relative core
/// modules dir wired in. The extracted EXIT_SUBTESTS reach into
/// `i64.abs` / `f64.sqrt` / etc. which live in the always-loaded
/// `prelude` (see `core/std/`) — without the dir, the
/// type-checker rejects every such test as "method not found".
fn jit_options_with_core() -> CompilerOptions {
    CompilerOptions {
        input: PathBuf::from("<jit>"),
        output: None,
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
        core_modules_dir: Some(core_modules_dir()),
    }
}

/// JIT-based driver for an exit-code sub-test batch. Each test is
/// independently compiled through `compile_to_jit_main_with_options`
/// (so prelude auto-loading sees the workspace's `core/`) and
/// called in-process; the macOS first-execve cost (~300 ms /
/// fresh binary) the AOT spawn path used to amortise across the
/// batch is gone entirely — there's no spawn at all.
///
/// As a side-effect the mangler is unused by the exit-code path
/// now; it stays for the stdout-batched path below because that
/// one prints into a single shared process and has to concatenate
/// sub-tests.
///
/// Returns (failure-1-indexed-or-0, total compile dur, total run dur).
fn compile_and_run_batched_jit(
    tests: &[SubTest],
) -> (i32, std::time::Duration, std::time::Duration) {
    let opts = jit_options_with_core();
    let mut total_compile = std::time::Duration::ZERO;
    let mut total_run = std::time::Duration::ZERO;
    for (i, t) in tests.iter().enumerate() {
        let t_compile = Instant::now();
        let prog = match compile_to_jit_main_with_options(&t.source, &opts) {
            Ok(p) => p,
            Err(err) => panic!(
                "sub-test #{} ({}): JIT compile failed: {err}",
                i + 1,
                t.name
            ),
        };
        total_compile += t_compile.elapsed();

        let t_run = Instant::now();
        let got = prog.run();
        total_run += t_run.elapsed();
        if got != t.expected {
            // Returning the 1-indexed failure number keeps the
            // call site's existing reporting shape intact.
            return ((i + 1) as i32, total_compile, total_run);
        }
    }
    (0, total_compile, total_run)
}

#[test]
fn batched_smoke_runs_ten_subtests_in_one_spawn() {
    if skip_e2e() {
        return;
    }
    // Hand-picked sub-tests that don't introduce top-level
    // declarations (no struct / enum / trait), so the prototype's
    // string-replace renamer suffices.
    let tests: Vec<SubTest> = vec![
        SubTest::new("literal_42", "fn main() -> u64 { 42u64 }\n", 42),
        SubTest::new(
            "fib_8",
            "fn fib(n: u64) -> u64 { if n <= 1u64 { n } else { fib(n - 1u64) + fib(n - 2u64) } }\nfn main() -> u64 { fib(8u64) }\n",
            21,
        ),
        SubTest::new(
            "for_loop_sum_0_to_9",
            "fn main() -> u64 {\n    var sum = 0u64\n    for i in 0u64..10u64 {\n        sum = sum + i\n    }\n    sum\n}\n",
            45,
        ),
        SubTest::new(
            "elif_chain",
            "fn classify(x: u64) -> u64 {\n    if x < 10u64 { 1u64 } elif x < 20u64 { 2u64 } else { 3u64 }\n}\nfn main() -> u64 { classify(5u64) + classify(15u64) * 10u64 + classify(25u64) * 100u64 }\n",
            1 + 2 * 10 + 3 * 100,
        ),
        SubTest::new(
            "short_circuit_and",
            "fn main() -> u64 {\n    val a: bool = true\n    val b: bool = false\n    if a && b { 1u64 } else { 0u64 }\n}\n",
            0,
        ),
        SubTest::new(
            "short_circuit_or",
            "fn main() -> u64 {\n    val a: bool = false\n    val b: bool = true\n    if a || b { 7u64 } else { 0u64 }\n}\n",
            7,
        ),
        SubTest::new(
            "match_literal_u64",
            "fn main() -> u64 {\n    val n: u64 = 2u64\n    match n {\n        0u64 => 10u64,\n        1u64 => 20u64,\n        2u64 => 30u64,\n        _ => 99u64,\n    }\n}\n",
            30,
        ),
        SubTest::new(
            "while_break",
            "fn main() -> u64 {\n    var i = 0u64\n    while i < 100u64 {\n        if i >= 7u64 { break }\n        i = i + 1u64\n    }\n    i\n}\n",
            7,
        ),
        SubTest::new(
            "f64_arith_and_cast",
            "fn main() -> u64 {\n    val x: f64 = 3.5f64\n    val y: f64 = 2.0f64\n    val z: f64 = x * y + 0.5f64\n    z as u64\n}\n",
            7,
        ),
        SubTest::new(
            "i64_to_u64_negate",
            "fn main() -> u64 {\n    val n: i64 = -5i64\n    val m: i64 = 0i64 - n\n    m as u64\n}\n",
            5,
        ),
    ];

    run_batched("batched e2e", &tests);
}

#[test]
fn batched_with_struct_and_enum_decls() {
    if skip_e2e() {
        return;
    }
    // Sub-tests with their own top-level `struct` / `enum`
    // declarations. The mangler renames each declared name with
    // the per-sub-test prefix so two tests can both declare
    // `Point` / `Color` etc. without colliding after concatenation.
    let tests: Vec<SubTest> = vec![
        SubTest::new(
            "struct_point_sum",
            "struct Point { x: u64, y: u64 }\nfn make() -> Point { Point { x: 3u64, y: 4u64 } }\nfn main() -> u64 { val p = make()\n p.x + p.y }\n",
            7,
        ),
        SubTest::new(
            "struct_point_product",
            // Same `Point` name as above — would collide without the mangler.
            "struct Point { x: u64, y: u64 }\nfn main() -> u64 {\n    val p = Point { x: 5u64, y: 6u64 }\n    p.x * p.y\n}\n",
            30,
        ),
        SubTest::new(
            "enum_color_red",
            "enum Color { Red, Green, Blue }\nfn main() -> u64 {\n    val c: Color = Color::Red\n    match c {\n        Color::Red => 1u64,\n        Color::Green => 2u64,\n        Color::Blue => 3u64,\n    }\n}\n",
            1,
        ),
        SubTest::new(
            "enum_color_blue",
            "enum Color { Red, Green, Blue }\nfn main() -> u64 {\n    val c: Color = Color::Blue\n    match c {\n        Color::Red => 10u64,\n        Color::Green => 20u64,\n        Color::Blue => 30u64,\n    }\n}\n",
            30,
        ),
        SubTest::new(
            "helper_fn_named_add",
            "fn add(a: u64, b: u64) -> u64 { a + b }\nfn main() -> u64 { add(7u64, 8u64) }\n",
            15,
        ),
        SubTest::new(
            "helper_fn_named_add_doubled",
            "fn add(a: u64, b: u64) -> u64 { (a + b) * 2u64 }\nfn main() -> u64 { add(3u64, 4u64) }\n",
            14,
        ),
    ];

    run_batched("batched e2e (decls)", &tests);
}

/// Shared helper: compile + run a batched fixture, log timings,
/// panic naming the first failing sub-test (if any). Threading
/// the label keeps the per-test stderr line distinguishable.
fn run_batched(label: &str, tests: &[SubTest]) {
    let (code, compile_dur, run_dur) = compile_and_run_batched_jit(tests);
    eprintln!(
        "{label}: {} sub-tests via JIT, compile {:?}, run {:?}",
        tests.len(),
        compile_dur,
        run_dur
    );
    if code != 0 {
        let failed = tests
            .get((code - 1) as usize)
            .map(|t| t.name.as_str())
            .unwrap_or("<unknown>");
        panic!(
            "{label}: sub-test #{code} ({failed}) returned an unexpected value",
        );
    }
}

// Sub-test sources that previously lived as individual
// `#[test]` functions in `compiler/tests/e2e.rs`. Migrated here
// as static data tables; the per-test definitions in `e2e.rs`
// were removed in the same commit.
//
// To regenerate after adding new tests to the e2e suite:
//   cargo run --release -p compiler --example dump_extracted \
//     > compiler/tests/batched_data/extracted.rs
// (the dumper still reads `e2e.rs`, so any new tests written
// in the recognised patterns get captured automatically.)
include!("batched_data/extracted.rs");

/// Wrap the static `EXIT_SUBTESTS` table (regenerated via the
/// `dump_extracted` example) into the runtime-friendly
/// `Vec<SubTest>` shape `run_batched` consumes. The table lives
/// in `batched_data/extracted.rs`; entries previously came from
/// the per-test functions in `e2e.rs` that have since been
/// removed.
fn extract_simple_e2e_tests() -> Vec<SubTest> {
    EXIT_SUBTESTS
        .iter()
        .map(|(name, source, expected)| SubTest::new(*name, *source, *expected))
        .collect()
}


#[test]
fn batched_e2e_extracted_from_file() {
    if skip_e2e() {
        return;
    }
    let tests = extract_simple_e2e_tests();
    eprintln!("auto-extracted {} sub-tests from e2e.rs", tests.len());
    if tests.is_empty() {
        panic!("extract_simple_e2e_tests returned 0 sub-tests — extractor pattern likely stale");
    }

    // Each sub-test is independently JIT-compiled. Iteration
    // order is preserved so the failure-index matches the source
    // order in `EXIT_SUBTESTS` for easy diagnosis. We collect all
    // failures rather than stopping at the first one — running
    // through the rest costs ~3 ms each at this point and gives a
    // complete picture when many tests break together (e.g. after
    // a codegen refactor).
    let opts = jit_options_with_core();
    let mut total_compile = std::time::Duration::ZERO;
    let mut total_run = std::time::Duration::ZERO;
    let mut failures: Vec<String> = Vec::new();
    for (i, t) in tests.iter().enumerate() {
        let t_compile = Instant::now();
        let prog = match compile_to_jit_main_with_options(&t.source, &opts) {
            Ok(p) => p,
            Err(err) => {
                failures.push(format!(
                    "#{} ({}): JIT compile failed: {err}",
                    i + 1,
                    t.name
                ));
                continue;
            }
        };
        total_compile += t_compile.elapsed();

        let t_run = Instant::now();
        let got = prog.run();
        total_run += t_run.elapsed();
        if got != t.expected {
            failures.push(format!(
                "#{} ({}): expected {}, got {}",
                i + 1,
                t.name,
                t.expected,
                got
            ));
        }
    }

    eprintln!(
        "batched e2e (extracted): {} sub-tests via JIT, compile {:?}, run {:?}",
        tests.len(),
        total_compile,
        total_run
    );
    if !failures.is_empty() {
        panic!(
            "batched e2e (extracted): {} sub-test(s) failed:\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
    }
}

// ============================================================
// Stdout-asserting sub-tests (Phase 2 of the batched runner).
// ============================================================
//
// Tests that use `compile_and_capture` and assert on `stdout`
// content (typically `println` programs) get a separate batched
// fixture. The meta-main wraps each sub-test call between two
// distinctive delimiter `println` calls; the runner captures
// stdout, splits on the delimiters, and compares each section
// to the recorded expected output.
//
// Why a separate fixture: the existing exit-code batched runner
// uses sub-test return values as the failure signal. Stdout
// tests don't return interesting exit codes (usually 0), so we
// need a different reporting channel — the printed delimiter +
// content lets the runner reconstruct per-sub-test output even
// though they all share one process.

/// One stdout sub-test. `source` defines a `fn main() -> u64`
/// that prints to stdout and returns 0 (or any value — we
/// ignore it). `expected_stdout` is compared verbatim against
/// the captured section.
struct StdoutSubTest {
    name: String,
    source: String,
    expected_stdout: String,
}

impl StdoutSubTest {
    fn new(
        name: impl Into<String>,
        source: impl Into<String>,
        expected_stdout: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            source: source.into(),
            expected_stdout: expected_stdout.into(),
        }
    }
}

/// JIT-based driver for stdout sub-tests. Each test is
/// independently compiled with `compile_to_jit_main_with_options`
/// and run via `JitProgram::run_capturing_stdout`, which routes
/// the runtime `toy_print_*` helpers into a thread-local
/// Vec<u8> for the duration of one program. No spawn, no shared
/// process, no marker-based stdout splitting.
///
/// Bonus: with one program per sub-test, we no longer need to
/// mangle struct / enum / fn names to keep them isolated, and
/// we no longer have to filter `__t<n>__` prefixes back out of
/// the captured output. The mangler stays available in this
/// file because new test additions can re-batch programs that
/// share top-level names if a future use case wants it; but the
/// code path here doesn't go through it.
fn run_stdout_batched(label: &str, tests: &[StdoutSubTest]) {
    let opts = jit_options_with_core();
    let mut total_compile = std::time::Duration::ZERO;
    let mut total_run = std::time::Duration::ZERO;
    let mut errors: Vec<String> = Vec::new();
    for (i, t) in tests.iter().enumerate() {
        let t_compile = Instant::now();
        let prog = match compile_to_jit_main_with_options(&t.source, &opts) {
            Ok(p) => p,
            Err(err) => {
                errors.push(format!(
                    "#{} ({}): JIT compile failed: {err}",
                    i + 1,
                    t.name
                ));
                continue;
            }
        };
        total_compile += t_compile.elapsed();

        let t_run = Instant::now();
        let (_exit, captured) = prog.run_capturing_stdout();
        total_run += t_run.elapsed();
        if captured != t.expected_stdout {
            errors.push(format!(
                "#{} ({}): stdout mismatch\nexpected: {:?}\nactual:   {:?}",
                i + 1,
                t.name,
                t.expected_stdout,
                captured,
            ));
        }
    }

    eprintln!(
        "{label}: {} sub-tests via JIT, compile {:?}, run {:?}",
        tests.len(),
        total_compile,
        total_run
    );
    if !errors.is_empty() {
        panic!(
            "{label}: {} sub-test(s) failed:\n  {}",
            errors.len(),
            errors.join("\n---\n  ")
        );
    }
}

#[test]
fn batched_stdout_smoke() {
    if skip_e2e() {
        return;
    }
    let tests: Vec<StdoutSubTest> = vec![
        StdoutSubTest::new(
            "println_string_literal",
            "fn main() -> u64 {\n    println(\"hello, world\")\n    0u64\n}\n",
            "hello, world\n",
        ),
        StdoutSubTest::new(
            "print_without_newline",
            "fn main() -> u64 {\n    print(\"foo\")\n    print(\"bar\")\n    println(\"!\")\n    0u64\n}\n",
            "foobar!\n",
        ),
        StdoutSubTest::new(
            "println_numeric",
            "fn main() -> u64 {\n    println(42u64)\n    println(-7i64)\n    0u64\n}\n",
            "42\n-7\n",
        ),
        StdoutSubTest::new(
            "println_bool",
            "fn main() -> u64 {\n    println(true)\n    println(false)\n    0u64\n}\n",
            "true\nfalse\n",
        ),
    ];
    run_stdout_batched("batched stdout (smoke)", &tests);
}

/// Wrap the static `STDOUT_SUBTESTS` table into the runtime
/// `Vec<StdoutSubTest>` shape `run_stdout_batched` consumes. The
/// table lives in `batched_data/extracted.rs`; entries
/// previously came from per-test functions in `e2e.rs` that
/// have since been removed.
fn extract_simple_stdout_tests() -> Vec<StdoutSubTest> {
    STDOUT_SUBTESTS
        .iter()
        .map(|(name, source, expected)| StdoutSubTest::new(*name, *source, *expected))
        .collect()
}

#[test]
fn batched_stdout_extracted_from_file() {
    if skip_e2e() {
        return;
    }
    let tests = extract_simple_stdout_tests();
    eprintln!("auto-extracted {} stdout sub-tests from e2e.rs", tests.len());
    if tests.is_empty() {
        panic!(
            "extract_simple_stdout_tests returned 0 sub-tests — extractor pattern likely stale"
        );
    }
    run_stdout_batched("batched stdout (extracted)", &tests);
}

// ============================================================
// Panic / assert sub-tests — investigated, not implemented.
// ============================================================
//
// Initial design: keep one `#[test]` that compiles + spawns each
// panic/assert source individually (since `panic` aborts the
// process and prevents shared `main` execution) using
// `std::thread::spawn` for parallelism. Empirical measurement on
// Apple Silicon (release build):
//
//   nextest runs 6 individual panic #[test]s in:    ~1.97 s
//   batched runner with parallel threads:           ~2.08 s
//   batched runner with sequential serial:          ~2.58 s
//
// The macOS kernel serialises Gatekeeper / xprotect first-execve
// scans, so the parallel batched approach offers no measurable
// speedup over nextest's existing per-test parallelism — each
// fresh binary still pays its full ~300 ms first-run cost in
// kernel time regardless of who issued the spawn. Implementing
// the batched fixture would have meant adding ~150 LoC, removing
// 6 tests from e2e.rs, and netting roughly zero wall-clock change.
//
// Conclusion: leave panic / assert tests on the per-test
// `e2e.rs` runner. The actual remaining win on the test suite
// would come from the cranelift-jit in-process loader noted in
// `compiler/README.md`'s future-work list (the only way to skip
// the kernel-side first-execve cost entirely).
