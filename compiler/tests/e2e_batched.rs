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

/// `include_str!`-loaded copy of the existing per-test `e2e.rs`
/// runner. Parsed at test runtime by `extract_simple_e2e_tests`
/// to harvest sub-tests automatically.
const E2E_RS: &str = include_str!("e2e.rs");

/// Walk `E2E_RS` looking for the simple `compile_and_run` test
/// pattern and extract `(name, source, expected)` tuples ready
/// to feed into the batched fixture.
///
/// The pattern this targets:
/// ```text
/// #[test]
/// fn TEST_NAME() {
///     if skip_e2e() {
///         return;
///     }
///     [optional comments]
///     let src = r#"...source..."#;
///     let code = compile_and_run(src, "stem");
///     assert_eq!(code, EXPECTED);
/// }
/// ```
/// or its inline-source variant where the `r#"..."#` literal is
/// passed directly as the first arg of `compile_and_run`.
///
/// Tests that use any of the following are **skipped** (they
/// don't fit the batched harness today):
/// - `compile_and_capture` (need stdout assertions)
/// - `panic` / `assert` programs (the meta-main can't continue
///   past a panic)
/// - more than one `compile_and_run` call (each result needs its
///   own sub-test slot)
/// - non-`u64` expected (parsed as integer, not signed)
/// - `assert!` macro instead of `assert_eq!(code, ...)` (custom
///   shape we'd need a richer extractor for)
fn extract_simple_e2e_tests() -> Vec<SubTest> {
    let mut out = Vec::new();
    // Find every `#[test]` annotation, capture the function name
    // from the next `fn NAME()` line, then scan ahead within the
    // *next test* boundary for the markers we need: `let src =
    // r#"..."#;` and `assert_eq!(code, NUMBER)`. We skip
    // brace-counting entirely — the raw-string source contains
    // `{` / `}` that would corrupt a naive counter — and instead
    // bound each test by "everything up to the next `#[test]` or
    // EOF", which is correct for the Rust formatting used in
    // e2e.rs (`#[test]` always appears at column 0 between
    // tests).
    let test_starts: Vec<usize> = E2E_RS
        .match_indices("\n#[test]\n")
        .map(|(idx, _)| idx + 1)
        .collect();
    for (i, &start) in test_starts.iter().enumerate() {
        let end = test_starts.get(i + 1).copied().unwrap_or(E2E_RS.len());
        let block = &E2E_RS[start..end];
        // Pull the function name out of `fn NAME(`.
        let fn_idx = match block.find("fn ") {
            Some(p) => p + "fn ".len(),
            None => continue,
        };
        let paren = match block[fn_idx..].find('(') {
            Some(p) => p,
            None => continue,
        };
        let name = block[fn_idx..fn_idx + paren].trim().to_string();
        if name.is_empty() {
            continue;
        }

        // Skip tests whose body uses unsupported helpers / shapes.
        // Two `compile_and_run` invocation patterns are supported:
        //   1. `let code = compile_and_run(src, "stem"); assert_eq!(code, N);`
        //   2. `assert_eq!(compile_and_run(src, "stem"), N);`
        // Pattern 2 doesn't have a separate `let code = ...` line.
        let cap = block.contains("compile_and_capture");
        let cr_count = block.matches("compile_and_run").count();
        let has_assert_eq_code = block.contains("assert_eq!(code,");
        let has_assert_eq_inline = block.contains("assert_eq!(compile_and_run");
        let has_panic = block.contains("panic(");
        let has_assert = block.contains("assert(");
        if cap || cr_count != 1 || (!has_assert_eq_code && !has_assert_eq_inline)
            || has_panic || has_assert
        {
            // BATCHED_VERBOSE=1 dumps each skip with the matched
            // flags, useful when the extractor pattern needs
            // expansion to cover a new test shape.
            if std::env::var("BATCHED_VERBOSE").is_ok() {
                eprintln!(
                    "skip {name}: cap={cap} cr_count={cr_count} eq_code={has_assert_eq_code} eq_inline={has_assert_eq_inline} panic={has_panic} assert={has_assert}"
                );
            }
            continue;
        }

        // Pull the raw-string literal source out of the body.
        // Tests use one of these shapes:
        //   let src = r#"..."#;     (most common)
        //   compile_and_run(r#"..."#, "stem")
        //   compile_and_run("inline source\n", "stem")
        let source = if let Some(src) = extract_raw_string(block, "let src = r#\"", "\"#") {
            src
        } else if let Some(src) = extract_raw_string(block, "compile_and_run(r#\"", "\"#,") {
            src
        } else if let Some(src) = extract_raw_string(block, "compile_and_run(\"", "\",") {
            unescape_rust_string(&src)
        } else {
            continue;
        };

        // Parse the expected exit code from `assert_eq!(code, NUMBER)`.
        let expected = match extract_assert_expected(block) {
            Some(n) => n,
            None => continue,
        };

        out.push(SubTest::new(name, source, expected));
    }
    out
}

/// Find the first occurrence of `start_marker` in `body`, then
/// return the substring up to (but not including) the next
/// occurrence of `end_marker`. Returns `None` if either marker
/// is missing.
fn extract_raw_string(body: &str, start_marker: &str, end_marker: &str) -> Option<String> {
    let s = body.find(start_marker)? + start_marker.len();
    let rest = &body[s..];
    let e = rest.find(end_marker)?;
    Some(rest[..e].to_string())
}

/// Cheap Rust-string-literal unescape for the inline form of
/// `compile_and_run("...", "stem")`. Only handles the escape
/// sequences the existing e2e tests use (`\n`, `\t`, `\"`,
/// `\\`).
fn unescape_rust_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Pull the integer expected value out of one of:
///   - `assert_eq!(code, N)` — paired with a separate
///     `let code = compile_and_run(...)`.
///   - `assert_eq!(compile_and_run(SRC, "STEM"), N)` — inline
///     form. Need to find the comma that closes the
///     `compile_and_run(...)` call (depth-aware) and read N
///     between that comma and the matching outer `)`.
/// Tolerates a trailing `as u64` / `as i32` cast on `N`.
/// Returns `None` if the integer can't be parsed (e.g., the
/// expected value is a Rust expression like `1 + 2 * 10`); the
/// extractor then skips the test.
fn extract_assert_expected(body: &str) -> Option<u64> {
    if let Some(s) = body.find("assert_eq!(code,") {
        let s = s + "assert_eq!(code,".len();
        let rest = &body[s..];
        let e = rest.find(')')?;
        return parse_expected_int(&rest[..e]);
    }
    // Inline form: `assert_eq!(compile_and_run(...), N)`. Walk
    // depth-aware from the start of the call to find the comma
    // separating the two macro arguments (depth==0 at top of
    // assert_eq!), then read until the closing paren.
    let macro_start = body.find("assert_eq!(compile_and_run")?;
    let body_after = &body[macro_start + "assert_eq!(".len()..];
    let mut depth = 0;
    let mut comma_pos: Option<usize> = None;
    for (i, ch) in body_after.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                comma_pos = Some(i);
                break;
            }
            _ => {}
        }
    }
    let comma_pos = comma_pos?;
    let after_comma = &body_after[comma_pos + 1..];
    let mut depth = 0;
    let mut close_pos: Option<usize> = None;
    for (i, ch) in after_comma.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => {
                close_pos = Some(i);
                break;
            }
            ')' => depth -= 1,
            _ => {}
        }
    }
    let close_pos = close_pos?;
    parse_expected_int(&after_comma[..close_pos])
}

fn parse_expected_int(raw: &str) -> Option<u64> {
    let raw = raw.trim().trim_end_matches(';').trim();
    let raw = raw.split(" as ").next().unwrap_or(raw).trim();
    raw.parse::<u64>().ok()
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
