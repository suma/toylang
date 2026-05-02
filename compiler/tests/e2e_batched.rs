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
    // nextest spawns each `#[test]` in its own process. Without
    // the PID, two parallel batched fixtures would race on the
    // same `.toy_e2e_batched_<stem>_0` temp file because each
    // process restarts the counter at 0. Adding `std::process::id()`
    // gives us unique paths across the parallel test runners.
    let pid = std::process::id();
    std::env::temp_dir().join(format!(".toy_e2e_batched_{stem}_{pid}_{n}"))
}

/// One sub-test in the batched fixture. `source` must define a
/// `fn main() -> u64` (or `-> i64`, returning a value `as u64`)
/// that the meta-main can compare against `expected`. The
/// `mangle_sub_test` pass below handles `struct` / `enum` /
/// `trait` / `impl` collisions and helper-fn names. Strings are
/// owned so this struct can hold both compile-time-literal
/// fixtures and ones extracted at test runtime from `e2e.rs`.
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

/// Rewrite a sub-test's `fn main() -> u64` to a per-test entry
/// point. Bare-string substitution is enough for the prototype
/// since none of the chosen sub-tests reference `main` from
/// inside their bodies. Future work would parse + rewrite the
/// AST instead.
fn rename_main(source: &str, idx: usize) -> String {
    let new_name = format!("__t{idx}_main");
    source.replace("fn main", &format!("fn {new_name}"))
}

/// Parse a sub-test source through the frontend, walk every
/// top-level `Stmt::StructDecl` / `Stmt::EnumDecl` /
/// `Stmt::TraitDecl` / `Stmt::ImplBlock` / `Stmt::Function`
/// (other than the special `main`), collect their declared
/// names, and return a textually-mangled copy of the source
/// where each collected name has the `__t<idx>__` prefix
/// prepended at every occurrence. Sub-tests can therefore share
/// declaration names (`Point`, `Color`, etc.) without colliding
/// after concatenation.
///
/// Why textual substitution at the symbol-name level works for
/// the toy language: identifiers are atomic tokens with no
/// punctuation collisions, and the parser's interner means
/// every reference site uses the same source spelling. Locating
/// occurrences with `\b<name>\b`-style word-boundary matching
/// (here: ASCII identifier-character boundary) is sound for the
/// language's grammar — there are no string-literal-embedded
/// type names the JIT / AOT pipeline cares about.
fn mangle_sub_test(source: &str, idx: usize) -> Result<String, String> {
    use frontend::ast::Stmt;
    let mut parser = frontend::ParserWithInterner::new(source);
    let program = parser
        .parse_program()
        .map_err(|e| format!("mangler parse: {e:?}"))?;
    let interner = parser.get_string_interner();

    let mut decl_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for i in 0..program.statement.len() {
        let sref = frontend::ast::StmtRef(i as u32);
        let stmt = match program.statement.get(&sref) {
            Some(s) => s,
            None => continue,
        };
        match &stmt {
            Stmt::StructDecl { name, .. } | Stmt::EnumDecl { name, .. } | Stmt::TraitDecl { name, .. } => {
                if let Some(n) = interner.resolve(*name) {
                    decl_names.insert(n.to_string());
                }
            }
            Stmt::ImplBlock { target_type, trait_name, .. } => {
                // `impl <Trait> for <Type>` — `target_type` may be
                // a primitive (`i64`, `f64`, ...) for extension
                // traits; we must not rename those because the
                // language relies on their canonical interner name
                // for primitive dispatch (`i64.abs()` resolves
                // through the symbol `"i64"`, not a user-defined
                // type). Same protection for `Self`. The trait
                // name itself stays mangled because two batched
                // sub-tests can each declare `trait Negate { ... }`.
                if let Some(n) = interner.resolve(*target_type) {
                    if !is_primitive_type_name(n) {
                        decl_names.insert(n.to_string());
                    }
                }
                if let Some(t) = trait_name {
                    if let Some(n) = interner.resolve(*t) {
                        decl_names.insert(n.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    // Top-level non-main fn names: collide if two sub-tests both
    // define a helper `add(...)`. Walk `program.function` and
    // mangle every entry except (a) `main` (which `rename_main`
    // already handles via `fn main → fn __t<i>_main`) and (b)
    // `extern fn` declarations — those need to keep their literal
    // name so each backend's extern dispatch table can resolve
    // them (`__extern_sqrt_f64`, `__extern_abs_i64`, etc.).
    // Renaming `__extern_sqrt_f64` to `__t<i>____extern_sqrt_f64`
    // makes the linker / runtime miss the symbol entirely.
    for f in &program.function {
        if f.is_extern {
            continue;
        }
        if let Some(n) = interner.resolve(f.name) {
            if n != "main" {
                decl_names.insert(n.to_string());
            }
        }
    }

    // Drop any `extern fn …` declarations the test source carries.
    // The auto-loaded `core/std/math.t` already declares every
    // math intrinsic the per-test sources reference; duplicating
    // the declaration here would trigger the IR's
    // `function_index` collision panic the moment two batched
    // sub-tests both write the same extern fn line. Single-line
    // extern decls are the only shape used in practice in
    // `e2e.rs` (no body), so a `^\s*extern fn` drop is enough.
    let cleaned: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("extern fn"))
        .map(|line| {
            let mut s = line.to_string();
            s.push('\n');
            s
        })
        .collect();
    let mut out = cleaned;
    let prefix = format!("__t{idx}__");
    // Replace longest names first so a shorter prefix can't eat
    // into a longer one (e.g., `Foo` before `FooBar` would
    // mis-replace `FooBar` as `__t0__FooBar` instead of leaving
    // it whole and prefixing only the standalone `Foo`).
    let mut sorted: Vec<&String> = decl_names.iter().collect();
    sorted.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for name in sorted {
        out = replace_word(&out, name, &format!("{prefix}{name}"));
    }
    Ok(out)
}

/// Remove every `__t<digits>__` substring from `s`. Used by the
/// stdout-batched runner: the mangler rewrites declared type
/// names to `__t<i>__Foo`, and any `println(value)` of a
/// struct / enum value emits that mangled name through the
/// runtime's display formatter. Stripping the prefix restores
/// the user's pre-mangling expectation for comparison.
fn strip_mangle_prefix(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        // Try to match `__t<digits>__` starting at `i`.
        if i + 3 <= bytes.len() && &bytes[i..i + 3] == b"__t" {
            let mut j = i + 3;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 3 && j + 2 <= bytes.len() && &bytes[j..j + 2] == b"__" {
                i = j + 2; // skip the entire `__t<digits>__` token
                continue;
            }
        }
        let ch_start = i;
        let ch_end = (1..=4)
            .map(|n| ch_start + n)
            .find(|&end| s.is_char_boundary(end))
            .unwrap_or(ch_start + 1);
        out.push_str(&s[ch_start..ch_end]);
        i = ch_end;
    }
    out
}

/// Returns true when `name` is one of the language's primitive /
/// keyword type names that the JIT / type checker dispatches on
/// by canonical symbol. Renaming any of these would break
/// extension-trait dispatch (`impl Foo for i64 { ... }`),
/// `Self` resolution inside impl bodies, and a few other
/// hard-coded type-name lookups in the frontend.
fn is_primitive_type_name(name: &str) -> bool {
    matches!(
        name,
        "i64" | "u64" | "f64" | "bool" | "str" | "ptr" | "usize" | "Self" | "main"
    )
}

/// Replace every whole-word occurrence of `needle` in `haystack`
/// with `replacement`. "Whole-word" means the surrounding chars
/// (or buffer boundary) are not ASCII letter / digit / underscore
/// — toy lang's identifier alphabet. Lets the mangler rename
/// `Point` without touching `MyPoint` or `Pointer`.
fn replace_word(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let bytes = haystack.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut out = String::with_capacity(haystack.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + needle_bytes.len() <= bytes.len()
            && &bytes[i..i + needle_bytes.len()] == needle_bytes
        {
            let before = if i == 0 { None } else { Some(bytes[i - 1]) };
            let after = bytes.get(i + needle_bytes.len()).copied();
            let is_id_char = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
            let bound_before = before.map_or(true, |c| !is_id_char(c));
            let bound_after = after.map_or(true, |c| !is_id_char(c));
            if bound_before && bound_after {
                out.push_str(replacement);
                i += needle_bytes.len();
                continue;
            }
        }
        // Not a match: append the next UTF-8 codepoint and move on.
        // The toy lang source is ASCII in practice but be defensive.
        let ch_start = i;
        let ch_end = (1..=4)
            .map(|n| ch_start + n)
            .find(|&end| haystack.is_char_boundary(end))
            .unwrap_or(ch_start + 1);
        out.push_str(&haystack[ch_start..ch_end]);
        i = ch_end;
    }
    out
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
        // First mangle every top-level decl name with `__t<i>__`
        // so two sub-tests can each define their own `Point` /
        // `Color` / helper fn without colliding. `rename_main`
        // then turns `fn main` into the per-sub-test entry point
        // the meta-main below dispatches into.
        let mangled = mangle_sub_test(&t.source, i)
            .unwrap_or_else(|err| panic!("subtest {} ({}) mangle failed: {err}", i, t.name));
        out.push_str(&rename_main(&mangled, i));
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
    let (code, compile_dur, run_dur) = compile_and_run_batched(tests);
    eprintln!(
        "{label}: {} sub-tests, compile {:?}, spawn+run {:?}",
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
    let mut tests = tests;
    if tests.is_empty() {
        // The extractor found nothing. Most likely the e2e.rs
        // pattern changed. Surface a clear error rather than
        // silently passing.
        panic!("extract_simple_e2e_tests returned 0 sub-tests — extractor pattern likely stale");
    }

    // Try the whole batch first; if compile fails (e.g. the
    // mangler tripped on some construct), bisect to find the
    // offender. Today everything compiles together, so the
    // bisect arm is dead code under normal circumstances.
    let combined = build_batched_source(&tests);
    let src_path = unique_path("batched_extracted.t");
    std::fs::write(&src_path, &combined).expect("write combined");
    let exe_path = unique_path("batched_extracted");
    let opts = CompilerOptions {
        input: src_path.clone(),
        output: Some(exe_path.clone()),
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
        core_modules_dir: Some(core_modules_dir()),
    };
    let t_compile = Instant::now();
    let compile_result = compile_file(&opts);
    let compile_dur = t_compile.elapsed();
    let _ = std::fs::remove_file(&src_path);

    if let Err(err) = compile_result {
        // Bisect: drop sub-tests one at a time from the end and
        // retry until we find the smallest set that still fails.
        // This narrows the offending sub-test for diagnostic
        // output without aborting the whole run.
        eprintln!("batched compile failed; bisecting {} sub-tests...", tests.len());
        let original_names: Vec<String> = tests.iter().map(|t| t.name.clone()).collect();
        let mut last_err = err;
        for trim_count in 1..tests.len() {
            tests.pop();
            let combined = build_batched_source(&tests);
            std::fs::write(&src_path, &combined).expect("write trimmed");
            match compile_file(&opts) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&src_path);
                    let dropped: Vec<&str> = original_names
                        [original_names.len() - trim_count..]
                        .iter()
                        .map(|s| s.as_str())
                        .collect();
                    panic!(
                        "auto-batched e2e: dropping the last {trim_count} sub-test(s) made the build pass. \
                         Dropped: {:?}\nLast error before bisect: {last_err}",
                        dropped,
                    );
                }
                Err(e) => last_err = e,
            }
        }
        let _ = std::fs::remove_file(&src_path);
        panic!("auto-batched e2e: every trimmed prefix still failed. Last error: {last_err}");
    }

    let t_run = Instant::now();
    let status = Command::new(&exe_path)
        .status()
        .expect("spawn extracted batched executable");
    let run_dur = t_run.elapsed();
    let _ = std::fs::remove_file(&exe_path);

    let code = status.code().expect("no exit code");
    eprintln!(
        "batched e2e (extracted): {} sub-tests, compile {:?}, spawn+run {:?}",
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
            "batched e2e (extracted): sub-test #{code} ({failed}) returned an unexpected value",
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

/// Marker `println` lines the meta-main emits around each sub-
/// test's stdout. Picked to be unlikely to appear inside any
/// reasonable test output: 7 `<` / `>` chars plus a numeric
/// index. The runner splits the captured stdout on these
/// markers to recover per-sub-test sections.
fn batch_begin(idx: usize) -> String {
    format!("<<<<<<<__BATCH_BEGIN_{idx}__>>>>>>>")
}
fn batch_end(idx: usize) -> String {
    format!("<<<<<<<__BATCH_END_{idx}__>>>>>>>")
}

fn build_stdout_batched_source(tests: &[StdoutSubTest]) -> String {
    let mut out =
        String::with_capacity(tests.iter().map(|t| t.source.len()).sum::<usize>() + 1024);

    for (i, t) in tests.iter().enumerate() {
        out.push_str(&format!("# stdout subtest {} = {}\n", i + 1, t.name));
        let mangled = mangle_sub_test(&t.source, i)
            .unwrap_or_else(|err| panic!("stdout subtest {} ({}) mangle failed: {err}", i, t.name));
        out.push_str(&rename_main(&mangled, i));
        out.push('\n');
    }

    out.push_str("\nfn main() -> u64 {\n");
    for (i, _t) in tests.iter().enumerate() {
        // Wrap each sub-test call between `println` markers. The
        // sub-test's own printlns appear between them; the
        // meta-main ignores the sub-test's return value.
        out.push_str(&format!(
            "    println(\"{begin}\")\n    val _r{i}: u64 = __t{i}_main()\n    println(\"{end}\")\n",
            begin = batch_begin(i),
            end = batch_end(i),
            i = i,
        ));
    }
    out.push_str("    0u64\n");
    out.push_str("}\n");
    out
}

/// Run a stdout-batched fixture: compile, spawn, capture stdout,
/// split on the batch markers, compare each section against the
/// recorded `expected_stdout`. Reports per-sub-test diffs on
/// mismatch.
fn run_stdout_batched(label: &str, tests: &[StdoutSubTest]) {
    let combined = build_stdout_batched_source(tests);
    let src_path = unique_path("batched_stdout.t");
    std::fs::write(&src_path, &combined).expect("write stdout batched source");
    let exe_path = unique_path("batched_stdout");
    let opts = CompilerOptions {
        input: src_path.clone(),
        output: Some(exe_path.clone()),
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
        core_modules_dir: Some(core_modules_dir()),
    };
    let t_compile = Instant::now();
    compile_file(&opts).expect("stdout batched compile_file failed");
    let compile_dur = t_compile.elapsed();

    let t_run = Instant::now();
    let output = Command::new(&exe_path)
        .output()
        .expect("spawn stdout batched executable");
    let run_dur = t_run.elapsed();
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    eprintln!(
        "{label}: {} sub-tests, compile {:?}, spawn+run {:?}",
        tests.len(),
        compile_dur,
        run_dur
    );

    // Walk each sub-test, find its [begin..end] section in
    // stdout, compare to expected.
    let mut errors: Vec<String> = Vec::new();
    for (i, t) in tests.iter().enumerate() {
        let begin = batch_begin(i);
        let end = batch_end(i);
        let section = match stdout.find(&begin).and_then(|s| {
            let after_begin = s + begin.len();
            // Skip the newline that terminates the begin marker.
            let after_begin = if stdout.as_bytes().get(after_begin) == Some(&b'\n') {
                after_begin + 1
            } else {
                after_begin
            };
            let end_pos = stdout[after_begin..].find(&end)?;
            Some(stdout[after_begin..after_begin + end_pos].to_string())
        }) {
            Some(s) => s,
            None => {
                errors.push(format!(
                    "{label}: sub-test #{} ({}): no output section found between markers",
                    i + 1,
                    t.name
                ));
                continue;
            }
        };
        // Strip every `__t<digits>__` prefix from the captured
        // section so `println(Color::Green)` (mangled to print
        // `__t12__Color::Green`) compares equal to the user's
        // pre-mangling expectation. Single-pass scan; no regex
        // dep needed.
        let normalised = strip_mangle_prefix(&section);
        if normalised != t.expected_stdout {
            errors.push(format!(
                "{label}: sub-test #{} ({}): stdout mismatch\nexpected: {:?}\nactual:   {:?}",
                i + 1,
                t.name,
                t.expected_stdout,
                normalised,
            ));
        }
    }

    if !errors.is_empty() {
        panic!(
            "{label}: {} sub-test(s) failed:\n{}",
            errors.len(),
            errors.join("\n---\n")
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
