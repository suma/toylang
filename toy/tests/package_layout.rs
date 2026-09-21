//! `toy`'s half of BUILD_TOOL: finding a package and running it.
//!
//! The tool holds no semantics (BUILD_TOOL.md §5), so what is worth
//! pinning is the convention — where the package root is, what the
//! entry is, and the *order* of the module roots, since B0 makes a
//! later root win and the whole arrangement depends on the package
//! coming after the stdlib.

use std::path::PathBuf;
use std::process::Command;

fn toy_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_toy"))
}

fn stdlib() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../core"))
}

/// A scratch package. Dropped on `Drop` so a failing test does not
/// leave one behind.
struct Pkg(PathBuf);

impl Drop for Pkg {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(stem: &str) -> Pkg {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("toy_pkg_{stem}_{}_{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(p.join("src")).expect("create package");
    Pkg(p)
}

fn write(pkg: &Pkg, rel: &str, text: &str) {
    let path = pkg.0.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create dir");
    }
    std::fs::write(path, text).expect("write");
}

/// `TOYLANG_CORE_MODULES` is what a test build has instead of an
/// installed stdlib: the binary under `target/debug/` would otherwise
/// probe exe-relative paths that may or may not resolve here.
fn run(_pkg: &Pkg, args: &[&str]) -> std::process::Output {
    Command::new(toy_bin())
        .args(args)
        .env("TOYLANG_CORE_MODULES", stdlib())
        .output()
        .expect("spawn toy")
}

fn greeter() -> &'static str {
    r#"
pub fn hello(name: str) -> String {
    val s: String = String::from_str("hello, ")
    var out: String = s
    out.push_str(name)
    out
}
"#
}

#[test]
fn a_package_module_and_the_stdlib_are_both_visible() {
    // The whole point of B0: before it, pointing `--core-modules` at
    // the package's own `src/` *replaced* the stdlib, so `String` went
    // missing. Both have to resolve at once.
    let pkg = scratch("both_roots");
    write(&pkg, "src/greet.t", greeter());
    write(
        &pkg,
        "main.t",
        r#"
fn main() -> u64 {
    val g: String = greet::hello("world")
    println(g)
    0u64
}
"#,
    );
    let out = run(&pkg, &["run", pkg.0.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("hello, world"),
        "status {:?}\nstdout: {stdout}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_entry_may_live_inside_the_module_root() {
    // ENTRY-IN-MODULE-ROOT: `src/main.t` used to be integrated twice
    // — once as the program and once as the module `main` — and the
    // copy lost its own top-level `const`s, reported as
    // `[E0003] Identifier 'X' not found` from a file that plainly
    // declares it. The auto-load walk now recognises the entry.
    let pkg = scratch("entry_in_root");
    write(&pkg, "src/greet.t", greeter());
    write(
        &pkg,
        "src/main.t",
        r#"
const TAG: u64 = 7u64
fn main() -> u64 {
    val g: String = greet::hello("world")
    println(g)
    TAG
}
"#,
    );
    let out = run(&pkg, &["run", pkg.0.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hello, world"),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(7), "the program's own `const` survived");
}

#[test]
fn a_later_root_wins_the_same_module_name() {
    // The ordering rule, from the outside: `--core-modules` on the
    // `toy` command line lands *after* the package's `src/`, so it
    // shadows it. Reverse the pair and the answer changes.
    let pkg = scratch("later_wins");
    write(&pkg, "src/greet.t", greeter());
    write(
        &pkg,
        "main.t",
        r#"
fn main() -> u64 {
    val g: String = greet::hello("world")
    println(g)
    0u64
}
"#,
    );
    let over = pkg.0.join("override");
    std::fs::create_dir_all(&over).expect("create override root");
    std::fs::write(
        over.join("greet.t"),
        r#"
pub fn hello(name: str) -> String {
    val s: String = String::from_str("OVERRIDDEN ")
    var out: String = s
    out.push_str(name)
    out
}
"#,
    )
    .expect("write override");

    let out = run(
        &pkg,
        &[
            "run",
            pkg.0.to_str().unwrap(),
            "--core-modules",
            over.to_str().unwrap(),
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("OVERRIDDEN world"),
        "a root given after the package should win; stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_program_gets_the_arguments_after_a_bare_dash_dash() {
    let pkg = scratch("prog_args");
    write(
        &pkg,
        "main.t",
        r#"
fn main() -> u64 {
    var i: u64 = 0u64
    while i < io::argc() {
        println(io::arg(i))
        i = i + 1u64
    }
    0u64
}
"#,
    );
    // `--release` after the `--` is the program's, not the tool's.
    let out = run(
        &pkg,
        &["run", pkg.0.to_str().unwrap(), "--", "alpha", "--release"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("alpha") && stdout.contains("--release"),
        "stdout: {stdout}"
    );
}

#[test]
fn build_writes_an_executable_under_build() {
    let pkg = scratch("build_out");
    write(&pkg, "src/greet.t", greeter());
    write(
        &pkg,
        "main.t",
        r#"
fn main() -> u64 {
    val g: String = greet::hello("world")
    println(g)
    0u64
}
"#,
    );
    let out = run(&pkg, &["build", pkg.0.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let exe = pkg
        .0
        .join("build/debug")
        .join(pkg.0.file_name().unwrap());
    assert!(exe.is_file(), "expected an executable at {}", exe.display());
    let ran = Command::new(&exe).output().expect("run built binary");
    assert!(String::from_utf8_lossy(&ran.stdout).contains("hello, world"));
}

#[test]
fn a_directory_that_is_not_a_package_says_so() {
    let pkg = scratch("not_a_package");
    // A directory with neither `main.t` nor `src/` above it. `/tmp`
    // has neither, so the walk reaches the filesystem root.
    let empty = pkg.0.join("src").join("nested");
    std::fs::create_dir_all(&empty).expect("create");
    let out = run(&pkg, &["check", empty.to_str().unwrap()]);
    // The walk finds `pkg/src`'s parent, which *is* a package — with
    // no entry point. Either message names the actual problem rather
    // than failing later inside the compiler.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success()
            && (stderr.contains("no entry point") || stderr.contains("no package found")),
        "stderr: {stderr}"
    );
}

#[test]
fn effects_sees_the_packages_own_modules() {
    // BUILD_TOOL §1 hole 3: `--effects` dropped `--core-modules` and
    // fell back to the default root, so it could not answer for a
    // program with modules of its own -- it failed to type-check.
    let pkg = scratch("effects_roots");
    write(&pkg, "src/greet.t", greeter());
    write(
        &pkg,
        "main.t",
        r#"
fn main() -> u64 {
    val g: String = greet::hello("world")
    println(g)
    0u64
}
"#,
    );
    let out = run(&pkg, &["effects", pkg.0.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("main"),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn api_answers_for_a_module_relative_to_the_package() {
    let pkg = scratch("api_rel");
    write(&pkg, "src/greet.t", greeter());
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    let out = run(&pkg, &["api", "src/greet.t", pkg.0.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("pub fn hello(name: str) -> String"),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// --- B2 / T0: tests where the code is ---------------------------------

#[test]
fn a_test_block_inside_a_module_runs() {
    // TEST-TOOL T0. `assert_eq` desugars to a string-concatenation
    // chain that builds the failure message, and the module-integration
    // remapper had no arm for it — so a module holding a `test` block
    // could not be integrated at all, and the only file that could
    // hold a test was the entry. That is why `poc/logsearch` has
    // 5,000 lines and no tests.
    let pkg = scratch("module_test");
    write(
        &pkg,
        "src/mathx.t",
        r#"
pub fn triple(n: u64) -> u64 { n * 3u64 }

test "triple works" {
    assert_eq(mathx::triple(3u64), 9u64)
}
"#,
    );
    write(&pkg, "main.t", "fn main() -> u64 { mathx::triple(2u64) }\n");
    let out = run(&pkg, &["test", pkg.0.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("1 passed, 0 failed"),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_failure_cites_the_module_it_was_written_in() {
    // The block's line belongs to the module, not to the entry the
    // runner happened to compile, and its name is qualified so two
    // modules can both have a "roundtrip".
    let pkg = scratch("module_test_fail");
    write(
        &pkg,
        "src/mathx.t",
        r#"
pub fn triple(n: u64) -> u64 { n * 3u64 }

test "is wrong on purpose" {
    assert_eq(mathx::triple(2u64), 7u64)
}
"#,
    );
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    let out = run(&pkg, &["test", pkg.0.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a failing test should exit non-zero");
    assert!(
        stderr.contains("mathx::is wrong on purpose") && stderr.contains("src/mathx.t"),
        "stderr: {stderr}"
    );
}

#[test]
fn a_module_test_is_reported_once_however_many_programs_pull_it_in() {
    // `tests/a.t` and `main.t` both compile against `src/`, so the
    // module's blocks arrive twice. They name one place, and that is
    // what identifies them.
    let pkg = scratch("dedup");
    write(
        &pkg,
        "src/mathx.t",
        r#"
pub fn triple(n: u64) -> u64 { n * 3u64 }

test "triple works" {
    assert_eq(mathx::triple(3u64), 9u64)
}
"#,
    );
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    write(
        &pkg,
        "tests/extra.t",
        "test \"zero\" {\n    assert_eq(mathx::triple(0u64), 0u64)\n}\n",
    );
    let out = run(&pkg, &["test", pkg.0.to_str().unwrap(), "--list"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("2 test(s)"), "stdout: {stdout}");
}

#[test]
fn a_filter_selects_by_name_and_a_path_still_reads_as_a_path() {
    // `toy test <filter> <path>` in either order: the argument that
    // names something on disk is the path.
    let pkg = scratch("filter");
    write(
        &pkg,
        "main.t",
        r#"
test "alpha runs" { assert_eq(1u64, 1u64) }
test "beta runs" { assert_eq(2u64, 2u64) }
fn main() -> u64 { 0u64 }
"#,
    );
    let out = run(&pkg, &["test", "alpha", pkg.0.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 passed"), "stdout: {stdout}");

    let both = run(&pkg, &["test", pkg.0.to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&both.stdout).contains("2 passed"));
}

#[test]
fn the_json_form_carries_the_failure_text() {
    let pkg = scratch("json");
    write(
        &pkg,
        "main.t",
        r#"
test "wrong" { assert_eq(1u64, 2u64) }
fn main() -> u64 { 0u64 }
"#,
    );
    let out = run(&pkg, &["test", pkg.0.to_str().unwrap(), "--format=json"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"name\": \"wrong\"") && stdout.contains("\"failure\": \""),
        "stdout: {stdout}"
    );
    // Newlines in the diagnostic must not break the line-per-record
    // shape a reader depends on.
    assert!(stdout.contains("\\n"), "the failure text should be escaped");
}

// --- B4: duplicate bare names -----------------------------------------

#[test]
fn a_bare_name_the_package_shares_with_the_stdlib_is_named_up_front() {
    // Shadowing resolves now (the later root wins), but it is worth
    // saying out loud: the roots are known before anything is parsed,
    // whereas the compiler reaches the question only when a *call* is
    // resolved — possibly down a branch this run never takes.
    let pkg = scratch("collide");
    write(&pkg, "src/mine.t", "pub fn encode(n: u64) -> u64 { n }\n");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    let out = run(&pkg, &["check", pkg.0.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("warning: `encode` is defined in") && stderr.contains("src/mine.t"),
        "stderr: {stderr}"
    );
    // The check itself still succeeds — it is a warning, not a bar.
    assert!(out.status.success(), "stderr: {stderr}");
}

#[test]
fn a_duplicate_that_is_entirely_the_stdlibs_is_not_reported() {
    // `encode` is in both `std::base64` and `std::hex` at the same
    // rank, so a bare `encode(...)` really is ambiguous — but that is
    // not this package's to fix, and a warning on every command that
    // cannot be acted on is one people learn to scroll past.
    let pkg = scratch("no_stdlib_noise");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    let out = run(&pkg, &["check", pkg.0.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("warning: `encode`"),
        "stdlib-only duplicates should stay quiet; stderr: {stderr}"
    );
}

#[test]
fn the_collision_check_can_be_turned_off() {
    let pkg = scratch("no_warn");
    write(&pkg, "src/mine.t", "pub fn encode(n: u64) -> u64 { n }\n");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    let out = run(
        &pkg,
        &["check", pkg.0.to_str().unwrap(), "--no-warn-collisions"],
    );
    assert!(!String::from_utf8_lossy(&out.stderr).contains("warning: `encode`"));
}

#[test]
fn a_later_root_wins_a_bare_name() {
    // BUILD_TOOL.md §1 hole 2: a package's own `fn parse` used to be
    // merely a third candidate against `std::json::parse`, so every
    // bare call became `[E0010] ambiguous module path` — and a
    // private helper broke the day the stdlib grew a function of the
    // same name. B0 already gave a later root the win for a module
    // *path*; a bare name is the same question with the qualifier
    // left off.
    //
    // Every lane has to agree, or a program type-checks against one
    // function and runs another.
    let pkg = scratch("bare_name_rank");
    write(&pkg, "src/mine.t", "pub fn encode(n: u64) -> u64 { n + 1u64 }\n");
    write(
        &pkg,
        "main.t",
        r#"
fn main() -> u64 {
    val n: u64 = encode(41u64)
    println(n)
    0u64
}
"#,
    );
    for backend in ["vm", "aot", "jit"] {
        let out = run(
            &pkg,
            &[
                "run",
                pkg.0.to_str().unwrap(),
                "--backend",
                backend,
                "--no-warn-collisions",
            ],
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("42"),
            "{backend}: the package's `encode` should win; stdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn a_qualified_call_still_names_its_own_module() {
    // The preference is for the *unqualified* form only. Writing
    // `hex::encode` asks for a particular module, and handing back a
    // different one because it sits in a later root would be a wrong
    // answer rather than a preference.
    let pkg = scratch("qualified_unaffected");
    write(&pkg, "src/mine.t", "pub fn encode(n: u64) -> u64 { n + 1u64 }\n");
    write(
        &pkg,
        "main.t",
        r#"
fn main() -> u64 {
    var v: Vec<u8> = Vec::new()
    v.push(65u8)
    val s: String = hex::encode(&v)
    println(s)
    val n: u64 = mine::encode(41u64)
    println(n)
    0u64
}
"#,
    );
    let out = run(&pkg, &["run", pkg.0.to_str().unwrap(), "--no-warn-collisions"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("41") && stdout.contains("42"),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// --- output layout ----------------------------------------------------

#[test]
fn debug_and_release_do_not_share_a_path() {
    // `--release` compiles the contracts out, so a release binary is a
    // different program from a debug one. Sharing a path would mean
    // the file on disk does not say which it is — and the answer would
    // change under you between two builds.
    let pkg = scratch("profiles");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    assert!(run(&pkg, &["build", pkg.0.to_str().unwrap()]).status.success());
    assert!(
        run(&pkg, &["build", pkg.0.to_str().unwrap(), "--release"])
            .status
            .success()
    );
    let name = pkg.0.file_name().unwrap();
    assert!(pkg.0.join("build/debug").join(name).is_file());
    assert!(pkg.0.join("build/release").join(name).is_file());
}

#[test]
fn run_does_not_overwrite_what_build_left_behind() {
    // `build`'s output is a result — something to keep, copy or ship.
    // `run`'s is scratch. A `toy run` after handing the binary to
    // someone should not rewrite the file they were given.
    let pkg = scratch("run_scratch");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    assert!(run(&pkg, &["build", pkg.0.to_str().unwrap()]).status.success());
    let built = pkg.0.join("build/debug").join(pkg.0.file_name().unwrap());
    let stamp = std::fs::metadata(&built).unwrap().len();
    // Make the product binary recognisable, then run and check it is
    // still the file we put there.
    std::fs::write(&built, b"not an executable any more").unwrap();
    let _ = run(
        &pkg,
        &["run", pkg.0.to_str().unwrap(), "--backend", "aot"],
    );
    let after = std::fs::read(&built).unwrap();
    assert_eq!(
        after, b"not an executable any more",
        "`toy run` must build somewhere else (was {stamp} bytes before)"
    );
}

#[test]
fn test_binaries_stay_out_of_the_product_directory() {
    let pkg = scratch("test_dir");
    write(
        &pkg,
        "main.t",
        "test \"passes\" { assert_eq(1u64, 1u64) }\nfn main() -> u64 { 0u64 }\n",
    );
    let out = run(&pkg, &["test", pkg.0.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The name carries a hash of the package-relative source path
    // (TEST-PARALLEL X0), so look for the directory holding exactly
    // one binary rather than for a fixed name.
    let built: Vec<_> = std::fs::read_dir(pkg.0.join("build/debug/tests"))
        .expect("tests dir")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(built.len(), 1, "built: {built:?}");
    assert!(built[0].starts_with("main_"), "built: {built:?}");
    assert!(!pkg.0.join("build/debug/main").exists());
}

#[test]
fn the_build_directory_ignores_itself() {
    // Build output is not source, and every package would otherwise
    // have to be told so by hand.
    let pkg = scratch("gitignore");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    assert!(run(&pkg, &["build", pkg.0.to_str().unwrap()]).status.success());
    let marker = pkg.0.join("build/.gitignore");
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "*\n");
}

#[test]
fn a_test_runs_on_the_compiled_lane_by_default() {
    // TEST-TOOL T1: `assert_eq` inside a `test` block used to be
    // unlowerable ("assert requires a string literal message in this
    // compiler MVP"), so the lane that ships was the one that could
    // not be tested — and the bugs a real program hits are
    // backend-specific.
    let pkg = scratch("aot_tests");
    write(
        &pkg,
        "main.t",
        r#"
test "passes" { assert_eq(1u64 + 1u64, 2u64) }
test "fails" { assert_eq(2u64, 3u64) }
fn main() -> u64 { 0u64 }
"#,
    );
    let out = run(&pkg, &["test", pkg.0.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "a failing test should exit non-zero");
    // The failure is attributed to the block that was running, and the
    // diagnostic carries both values.
    assert!(
        stderr.contains("FAILED  fails") && stderr.contains("left:  2"),
        "stderr: {stderr}"
    );
    assert!(stdout.contains("1 passed, 1 failed"), "stdout: {stdout}");
}

// --- clean ------------------------------------------------------------

#[test]
fn clean_removes_the_outputs_and_keeps_the_link_cache() {
    // Throwing the cache away turns the next build from 30 ms back
    // into 90 ms, and it cannot go stale — it is keyed on the bytes of
    // the object it links. Cleaning is about the outputs.
    let pkg = scratch("clean");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    assert!(run(&pkg, &["build", pkg.0.to_str().unwrap()]).status.success());
    assert!(
        run(&pkg, &["build", pkg.0.to_str().unwrap(), "--release"])
            .status
            .success()
    );
    assert!(pkg.0.join("build/debug").is_dir());
    assert!(pkg.0.join("build/release").is_dir());

    let out = run(&pkg, &["clean", pkg.0.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!pkg.0.join("build/debug").exists());
    assert!(!pkg.0.join("build/release").exists());
    assert!(
        pkg.0.join("build/.link").is_dir(),
        "the link cache should survive a plain clean"
    );
}

#[test]
fn clean_all_takes_the_build_directory_with_it() {
    let pkg = scratch("clean_all");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    assert!(run(&pkg, &["build", pkg.0.to_str().unwrap()]).status.success());
    assert!(
        run(&pkg, &["clean", pkg.0.to_str().unwrap(), "--all"])
            .status
            .success()
    );
    assert!(!pkg.0.join("build").exists());
    // And the package still builds: nothing outside `build/` was
    // touched.
    assert!(run(&pkg, &["build", pkg.0.to_str().unwrap()]).status.success());
}

#[test]
fn cleaning_twice_is_not_an_error() {
    let pkg = scratch("clean_twice");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    for _ in 0..2 {
        let out = run(&pkg, &["clean", pkg.0.to_str().unwrap()]);
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn clean_leaves_the_sources_alone() {
    // The one thing this command must never do. `is_build_output`
    // checks every path against the package's `build/` before
    // removing it, because the difference between "removes the build
    // output" and "removes the package" is a path computation.
    let pkg = scratch("clean_safety");
    write(&pkg, "src/mine.t", "pub fn f() -> u64 { 1u64 }\n");
    write(&pkg, "main.t", "fn main() -> u64 { mine::f() }\n");
    write(&pkg, "tests/a.t", "test \"t\" { assert_eq(1u64, 1u64) }\n");
    assert!(run(&pkg, &["build", pkg.0.to_str().unwrap()]).status.success());
    assert!(
        run(&pkg, &["clean", pkg.0.to_str().unwrap(), "--all"])
            .status
            .success()
    );
    assert!(pkg.0.join("main.t").is_file());
    assert!(pkg.0.join("src/mine.t").is_file());
    assert!(pkg.0.join("tests/a.t").is_file());
}

// --- T4 / T5 ----------------------------------------------------------

#[test]
fn a_panics_test_passes_by_stopping() {
    // TEST-TOOL T4. VEC-CONTRACTS turned `Vec`'s bounds into
    // `requires` clauses and nothing could confirm one ever fired: a
    // panic ends the program, so there was no way to write the test.
    //
    // Both lanes, because the compiled one needs a binary per such
    // test (a panic ends the process, so it cannot share a driver).
    let pkg = scratch("panics");
    write(
        &pkg,
        "main.t",
        r#"
test "index past the end panics" panics {
    val v: Vec<u64> = Vec::new()
    val x: u64 = v.get(0u64)
}
test "and the message names the contract" panics "Contract violation" {
    val v: Vec<u64> = Vec::new()
    val x: u64 = v.get(0u64)
}
test "an ordinary test still runs beside them" {
    assert_eq(1u64, 1u64)
}
fn main() -> u64 { 0u64 }
"#,
    );
    for backend in ["aot", "vm"] {
        let out = run(
            &pkg,
            &["test", pkg.0.to_str().unwrap(), "--backend", backend],
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("3 passed, 0 failed"),
            "{backend}: stdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn a_panics_test_that_returns_is_a_failure() {
    let pkg = scratch("panics_but_returns");
    write(
        &pkg,
        "main.t",
        "test \"never panics\" panics { assert_eq(1u64, 1u64) }\nfn main() -> u64 { 0u64 }\n",
    );
    for backend in ["aot", "vm"] {
        let out = run(
            &pkg,
            &["test", pkg.0.to_str().unwrap(), "--backend", backend],
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && stderr.contains("to panic, but it returned"),
            "{backend}: stderr: {stderr}"
        );
    }
}

#[test]
fn the_expected_message_has_to_match() {
    // "Something died" does not say which contract broke, which is
    // the whole reason the text can be written down.
    let pkg = scratch("panics_message");
    write(
        &pkg,
        "main.t",
        r#"
test "wrong text" panics "no such text" {
    val v: Vec<u64> = Vec::new()
    val x: u64 = v.get(0u64)
}
fn main() -> u64 { 0u64 }
"#,
    );
    let out = run(&pkg, &["test", pkg.0.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success() && stderr.contains("expected a panic containing `no such text`"),
        "stderr: {stderr}"
    );
    // And it prints what did arrive, so the fix is visible. Here
    // that is the `requires` clause — which is the case T4 exists
    // for: VEC-CONTRACTS made `Vec`'s bounds a contract and nothing
    // could confirm one fires.
    assert!(
        stderr.contains("Contract violation") && stderr.contains("`get`"),
        "stderr: {stderr}"
    );
}

#[test]
fn a_golden_file_is_recorded_by_bless_and_checked_after() {
    // TEST-TOOL T5. A format's promise — "no change makes existing
    // data unreadable" — is not a check until the bytes are written
    // down.
    let pkg = scratch("golden");
    write(
        &pkg,
        "main.t",
        r#"
test "the format did not change" {
    var v: Vec<u8> = Vec::with_capacity(4u64)
    v.push(1u8)
    v.push(2u8)
    v.push(3u8)
    v.push(4u8)
    val s = v.as_span()
    match s {
        Option::Some(bytes) => { testing::assert_golden("tests/golden/one.bin", bytes) }
        Option::None => { panic("no span") }
    }
}
fn main() -> u64 { 0u64 }
"#,
    );
    std::fs::create_dir_all(pkg.0.join("tests/golden")).unwrap();

    // Missing: a failure that names the remedy. Recording on first
    // sight would mean a test nobody has looked at goes green.
    let first = run(&pkg, &["test", pkg.0.to_str().unwrap()]);
    assert!(!first.status.success());
    assert!(
        String::from_utf8_lossy(&first.stderr).contains("run `toy test --bless`"),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let blessed = run(&pkg, &["test", pkg.0.to_str().unwrap(), "--bless"]);
    assert!(
        blessed.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&blessed.stderr)
    );
    let recorded = pkg.0.join("tests/golden/one.bin");
    assert_eq!(std::fs::read(&recorded).unwrap(), vec![1u8, 2, 3, 4]);

    // Recorded, so it passes; changed, so it fails by offset.
    assert!(run(&pkg, &["test", pkg.0.to_str().unwrap()]).status.success());
    std::fs::write(&recorded, [1u8, 2, 99, 4]).unwrap();
    let changed = run(&pkg, &["test", pkg.0.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&changed.stderr);
    assert!(
        !changed.status.success() && stderr.contains("byte 2 differs"),
        "a golden mismatch must name the offset: {stderr}"
    );
}

// --- version ----------------------------------------------------------

#[test]
fn version_names_every_part_and_where_it_is() {
    // The question during development is not "which release" — every
    // build of this repo is 0.1.0 — but whether the thing running is
    // the thing just built, and against which stdlib. Three parts can
    // disagree, so each gets a row with a path.
    let out = Command::new(toy_bin())
        .arg("version")
        .env("TOYLANG_CORE_MODULES", stdlib())
        .output()
        .expect("spawn toy");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    for part in ["toy", "compiler", "interpreter", "stdlib"] {
        assert!(
            stdout.lines().any(|l| l.starts_with(part)),
            "no row for `{part}`: {stdout}"
        );
    }
    // Each row carries the path it is talking about.
    assert!(
        stdout.lines().filter(|l| l.contains('/')).count() >= 4,
        "every row should name a path: {stdout}"
    );
}

#[test]
fn a_missing_path_is_flagged_on_its_own_line() {
    // A `compiler` that is not beside `toy` is not fatal — `toy` holds
    // it as a crate — but `compiler ...` typed by hand will not work,
    // and that is worth saying rather than leaving for the reader to
    // notice in a path they were not reading.
    let dir = scratch("version_missing");
    let lone = dir.0.join("toy");
    std::fs::copy(toy_bin(), &lone).expect("copy toy");
    let out = Command::new(&lone)
        .arg("version")
        .env("TOYLANG_CORE_MODULES", dir.0.join("no-such-stdlib"))
        .env("NO_COLOR", "1")
        .output()
        .expect("spawn toy");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Same line as the path, so the two are read together.
    for part in ["compiler", "interpreter", "stdlib"] {
        let line = stdout
            .lines()
            .find(|l| l.starts_with(part))
            .unwrap_or_else(|| panic!("no row for {part}: {stdout}"));
        assert!(
            line.contains("not found"),
            "`{part}` is absent and should say so: {line}"
        );
    }
    // `toy` itself is right there, so it is not flagged.
    let toy_line = stdout.lines().find(|l| l.starts_with("toy")).unwrap();
    assert!(!toy_line.contains("not found"), "{toy_line}");
}

#[test]
fn colour_is_for_terminals_and_can_be_forced_or_suppressed() {
    // Piped into a file or a grep the escapes are noise — and they
    // would break the assertions above, which is the test.
    let dir = scratch("version_colour");
    let lone = dir.0.join("toy");
    std::fs::copy(toy_bin(), &lone).expect("copy toy");
    let missing = dir.0.join("no-such-stdlib");

    let plain = Command::new(&lone)
        .arg("version")
        .env("TOYLANG_CORE_MODULES", &missing)
        .output()
        .expect("spawn toy");
    assert!(
        !String::from_utf8_lossy(&plain.stdout).contains('\x1b'),
        "a pipe is not a terminal"
    );

    let forced = Command::new(&lone)
        .arg("version")
        .env("TOYLANG_CORE_MODULES", &missing)
        .env("TOY_COLOR", "1")
        .output()
        .expect("spawn toy");
    assert!(
        String::from_utf8_lossy(&forced.stdout).contains("\x1b[33m"),
        "TOY_COLOR should force it on"
    );

    let suppressed = Command::new(&lone)
        .arg("version")
        .env("TOYLANG_CORE_MODULES", &missing)
        .env("TOY_COLOR", "1")
        .env("NO_COLOR", "1")
        .output()
        .expect("spawn toy");
    // NO_COLOR is the convention, but an explicit request wins over
    // an ambient one.
    assert!(String::from_utf8_lossy(&suppressed.stdout).contains("\x1b[33m"));
}

// ---------------------------------------------------------------
// TEST-PARALLEL: running the suite on more than one worker
// ---------------------------------------------------------------

/// A package with `count` test files, each holding two tests.
fn multi_file_pkg(stem: &str, count: usize) -> Pkg {
    let pkg = scratch(stem);
    write(&pkg, "src/mathx.t", "pub fn triple(n: u64) -> u64 { n * 3u64 }\n");
    write(&pkg, "main.t", "fn main() -> u64 { mathx::triple(1u64) }\n");
    for i in 0..count {
        write(
            &pkg,
            &format!("tests/t{i}.t"),
            &format!(
                "test \"file {i} case a\" {{ assert_eq(mathx::triple(3u64), 9u64) }}\n\
                 test \"file {i} case b\" {{ assert_eq(mathx::triple(4u64), 12u64) }}\n"
            ),
        );
    }
    pkg
}

#[test]
fn two_test_files_with_the_same_stem_get_their_own_binaries() {
    // TEST-PARALLEL X0. `tests/a/x.t` and `tests/b/x.t` both compiled
    // to `build/debug/tests/x`: harmless while one ran after the
    // other, fatal once two workers do it at once — one of them
    // executes a binary the other is still writing.
    let pkg = scratch("same_stem");
    write(&pkg, "src/mathx.t", "pub fn triple(n: u64) -> u64 { n * 3u64 }\n");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    write(&pkg, "tests/a/x.t", "test \"from a\" { assert_eq(mathx::triple(1u64), 3u64) }\n");
    write(&pkg, "tests/b/x.t", "test \"from b\" { assert_eq(mathx::triple(2u64), 6u64) }\n");

    let out = run(&pkg, &["test", pkg.0.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let built: Vec<_> = std::fs::read_dir(pkg.0.join("build/debug/tests"))
        .expect("tests dir")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(built.len(), 2, "one binary per test file; got {built:?}");
}

#[test]
fn one_worker_and_many_report_the_same_thing() {
    // The whole safety net for a parallel runner (TEST_PARALLEL.md
    // §7): the report is assembled in plan order rather than
    // completion order, so the only way to notice a scheduling
    // dependency is to compare the two.
    let pkg = multi_file_pkg("jobs_agree", 4);
    // One failure and one panicking test, so the comparison covers
    // more than the happy path.
    write(
        &pkg,
        "tests/z.t",
        "test \"this one fails\" { assert_eq(2u64, 3u64) }\n\
         test \"this one panics\" panics \"boom\" { panic(\"boom\") }\n",
    );
    let path = pkg.0.to_str().unwrap();
    for lane in [vec!["test", path], vec!["test", path, "--backend", "vm"]] {
        for format in [vec![], vec!["--format=json"]] {
            let mut serial = lane.clone();
            serial.extend(format.iter());
            serial.push("-j1");
            let mut parallel = lane.clone();
            parallel.extend(format.iter());
            parallel.push("-j8");

            let one = run(&pkg, &serial);
            let many = run(&pkg, &parallel);
            assert_eq!(
                without_elapsed(&one.stdout),
                without_elapsed(&many.stdout),
                "stdout differs for {serial:?}"
            );
            // stderr carries the failures; the timing line lives on
            // stdout, so both streams are comparable byte for byte.
            assert_eq!(
                String::from_utf8_lossy(&one.stderr),
                String::from_utf8_lossy(&many.stderr),
                "stderr differs for {serial:?}"
            );
            assert_eq!(one.status.code(), many.status.code());
        }
    }
}

#[test]
fn a_failing_test_shows_its_own_output_and_not_its_neighbours() {
    // With workers running at once, letting output through as it
    // happens interleaves it; the runner captures per test and prints
    // only what a failure needs (TEST_PARALLEL.md D4).
    let pkg = scratch("output_apart");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    write(
        &pkg,
        "tests/a.t",
        "test \"quiet neighbour\" { println(\"NEIGHBOUR_A\") }\n",
    );
    write(
        &pkg,
        "tests/b.t",
        "test \"loud failure\" { println(\"MINE_B\") assert_eq(2u64, 3u64) }\n",
    );
    let out = run(&pkg, &["test", pkg.0.to_str().unwrap(), "--backend", "vm", "-j8"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("MINE_B"), "the failure keeps its own output: {stderr}");
    assert!(
        !stderr.contains("NEIGHBOUR_A"),
        "and not the other test's: {stderr}"
    );
}

#[test]
fn panicking_tests_still_pass_when_they_run_at_once() {
    // TEST-TOOL T4 gives every `panics` test a binary of its own,
    // which is the case where the collision in X0 bites hardest: the
    // name came from the test, so two files could agree on it.
    let pkg = scratch("panics_parallel");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    for (i, file) in ["tests/one.t", "tests/two.t"].iter().enumerate() {
        write(
            &pkg,
            file,
            &format!(
                "test \"same name\" panics \"boom {i}\" {{ panic(\"boom {i}\") }}\n\
                 test \"also fine {i}\" {{ assert_eq(1u64, 1u64) }}\n"
            ),
        );
    }
    let out = run(&pkg, &["test", pkg.0.to_str().unwrap(), "-j8"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("4 passed, 0 failed"), "stdout: {stdout}");
}

#[test]
fn a_name_filter_decides_what_runs_rather_than_what_is_printed() {
    // TEST-PARALLEL X2: the IR VM lane used to run every block and
    // then drop the ones whose names did not match, so asking for one
    // test cost the whole suite. Observable without a clock: a test
    // the filter excludes cannot fail the run.
    let pkg = scratch("filter_runs");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    write(
        &pkg,
        "tests/a.t",
        "test \"keeper\" { assert_eq(1u64, 1u64) }\n\
         test \"other block\" { panic(\"this block must never run\") }\n",
    );
    let out = run(&pkg, &["test", "keeper", pkg.0.to_str().unwrap(), "--backend", "vm"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("1 passed, 0 failed"), "stdout: {stdout}");
}

/// A run's summary line ends in how long it took, which is the one
/// thing two runs of the same suite may legitimately disagree about.
fn without_elapsed(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|line| match line.find("   ") {
            Some(cut) if line.ends_with(" s") && line.contains("passed,") => {
                line[..cut].to_string()
            }
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// --- `--format=json` on stderr: the diagnostics, handed through --------

/// The JSON array `--format=json` wrote to stderr. Anything else
/// on stderr (the `toy: N error(s)` summary) is outside the brackets.
fn diagnostics_in(stderr: &str) -> Vec<serde_json::Value> {
    let (start, end) = (stderr.find('['), stderr.rfind(']'));
    let (Some(start), Some(end)) = (start, end) else {
        panic!("no JSON array on stderr:\n{stderr}");
    };
    match serde_json::from_str(&stderr[start..=end]) {
        Ok(serde_json::Value::Array(items)) => items,
        other => panic!("stderr is not one JSON array ({other:?}):\n{stderr}"),
    }
}

#[test]
fn every_command_that_checks_a_program_reports_type_errors_as_json() {
    let pkg = scratch("diag_json_type");
    write(&pkg, "main.t", "fn main() -> u64 {\n    val x: u64 = true\n    x\n}\n");
    let path = pkg.0.to_str().unwrap();
    // Each of these reaches the type checker by a different route —
    // the compiler driver, the compiler JIT, the interpreter, the
    // effect query, the test planner and `check`'s own call — and
    // each used to be free to ignore the flag.
    let commands: &[&[&str]] = &[
        &["check"],
        &["build"],
        &["run", "--backend", "aot"],
        &["run", "--backend", "jit"],
        &["run", "--backend", "vm"],
        &["effects"],
        &["test"],
        &["test", "--backend", "vm"],
    ];
    for command in commands {
        let mut argv = command.to_vec();
        argv.extend([path, "--format=json", "--no-warn-collisions"]);
        let out = run(&pkg, &argv);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{argv:?} succeeded:\n{stderr}");
        let diagnostics = diagnostics_in(&stderr);
        assert!(
            diagnostics.len() == 1 && diagnostics[0]["code"] == "E0001",
            "{argv:?}:\n{stderr}"
        );
        assert!(!stderr.contains("Error at"), "{argv:?} also rendered text:\n{stderr}");
    }
}

#[test]
fn a_parse_error_is_json_too_on_the_compiled_lanes() {
    // `compile_file` parsed with the single-error entry point and
    // stringified it, so the flag never reached a syntax error there.
    let pkg = scratch("diag_json_parse");
    write(&pkg, "main.t", "fn main() -> u64 {\n    val x: u64 =\n}\n");
    let path = pkg.0.to_str().unwrap();
    for command in [["build", path], ["check", path]] {
        let mut argv = command.to_vec();
        argv.push("--format=json");
        let out = run(&pkg, &argv);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let diagnostics = diagnostics_in(&stderr);
        assert!(
            !out.status.success() && diagnostics[0]["span"]["line"] == 3,
            "{argv:?}:\n{stderr}"
        );
    }
}

#[test]
fn a_runtime_error_is_reported_once() {
    // `run_source` reports the failure itself; `toy` used to print the
    // returned message again, which after `--format=json` put
    // rendered text behind the JSON array.
    let pkg = scratch("diag_json_runtime");
    write(&pkg, "main.t", "fn main() -> u64 {\n    val a = 1u64\n    a - 2u64\n}\n");
    let path = pkg.0.to_str().unwrap();
    let out = run(&pkg, &["run", path, "--format", "json"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let diagnostics = diagnostics_in(&stderr);
    assert!(
        !out.status.success()
            && diagnostics.len() == 1
            && diagnostics[0]["message"].as_str().unwrap_or("").contains("underflow"),
        "stderr:\n{stderr}"
    );
    assert_eq!(stderr.trim_end().chars().last(), Some(']'), "stderr:\n{stderr}");

    let text = run(&pkg, &["run", path]);
    let stderr = String::from_utf8_lossy(&text.stderr);
    assert_eq!(stderr.matches("underflowed").count(), 1, "stderr:\n{stderr}");
}

#[test]
fn an_unknown_format_is_refused() {
    let pkg = scratch("diag_bad");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    let out = run(&pkg, &["check", pkg.0.to_str().unwrap(), "--format=xml"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && stderr.contains("`text` or `json`"), "{stderr}");
}

#[test]
fn the_old_diagnostics_flag_points_at_format() {
    // Folded into `--format`; a command copied from an old note should
    // say what to type rather than "unknown option".
    let pkg = scratch("diag_folded");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    let out = run(&pkg, &["check", pkg.0.to_str().unwrap(), "--diagnostics=json"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && stderr.contains("use --format=json"), "{stderr}");
}

// --- `--format=json`: every command's result as a document -------------

fn stdout_json(out: &std::process::Output, argv: &[&str]) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{argv:?} failed:\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("{argv:?}: stdout is not one JSON document ({e}):\n{stdout}"))
}

#[test]
fn every_command_can_answer_in_json() {
    let pkg = scratch("format_json");
    write(
        &pkg,
        "src/greet.t",
        r#"
pub struct Point { pub x: i64, y: i64 }
pub fn twice(n: u64) -> u64
    requires n < 100u64
{
    n * 2u64
}
test "twice works" { assert_eq(twice(2u64), 4u64) }
"#,
    );
    write(&pkg, "main.t", "fn main() -> u64 {\n    println(greet::twice(2u64))\n    0u64\n}\n");
    let path = pkg.0.to_str().unwrap();
    let json = |argv: &[&str]| {
        let mut full = argv.to_vec();
        full.extend(["--format=json", "--no-warn-collisions"]);
        stdout_json(&run(&pkg, &full), &full)
    };

    let built = json(&["build", path]);
    assert!(std::path::Path::new(built["output"].as_str().unwrap()).is_file(), "{built:#}");

    assert_eq!(json(&["check", path])["ok"], true);

    let effects = json(&["effects", path]);
    let main = effects.as_array().unwrap().iter().find(|e| e["name"] == "main").expect("main");
    assert_eq!(main["effects"], serde_json::json!(["io"]));

    let listed = json(&["test", path, "--list"]);
    assert_eq!(listed[0]["line"], 8, "{listed:#}");

    let api = json(&["api", "src/greet.t", path]);
    let items = api["items"].as_array().unwrap();
    let twice = items.iter().find(|i| i["name"] == "twice").expect("twice");
    assert_eq!(twice["requires"], serde_json::json!(["n < 100u64"]));
    let point = items.iter().find(|i| i["name"] == "Point").expect("Point");
    assert_eq!(point["members"][1]["public"], false);

    let explained = json(&["explain", "e0001"]);
    assert_eq!(explained["code"], "E0001");
    assert!(explained["text"].as_str().unwrap().starts_with("E0001"));
    assert!(json(&["explain"]).as_array().unwrap().len() > 10);

    let version = json(&["version"]);
    assert_eq!(version[0]["name"], "toy");

    let cleaned = json(&["clean", path]);
    assert_eq!(cleaned["removed"].as_array().unwrap().len(), 1, "{cleaned:#}");
    assert_eq!(json(&["clean", path])["removed"], serde_json::json!([]));

    // `run` has no result of its own to shape: the program's output
    // goes through untouched, and only diagnostics would be JSON.
    let out = run(&pkg, &["run", path, "--format=json", "--no-warn-collisions"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "4\n");
}

#[test]
fn the_api_text_listing_is_the_json_items_joined() {
    // `render` is a projection of `items`; if they drifted, the two
    // forms would describe different modules.
    let pkg = scratch("api_projection");
    write(
        &pkg,
        "src/shapes.t",
        r#"
pub enum Shape { Circle(i64), Point }
pub trait Area { fn area(self: Self) -> i64 }
impl Area for Shape {
    pub fn area(self: Self) -> i64 { 0i64 }
}
pub const K: u64 = 3u64
pub fn f(n: u64) -> u64
    ensures result == n
{
    n
}
pub fn g() -> u64 { 0u64 }
"#,
    );
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    let path = pkg.0.to_str().unwrap();
    let text = run(&pkg, &["api", "src/shapes.t", path]);
    let text = String::from_utf8_lossy(&text.stdout);
    let doc = stdout_json(&run(&pkg, &["api", "src/shapes.t", path, "--format=json"]), &["api"]);
    for item in doc["items"].as_array().unwrap() {
        let block = item["text"].as_str().unwrap();
        assert!(text.contains(block), "`{block}` is not in the text listing:\n{text}");
    }
}

#[test]
fn a_module_sees_its_own_consts() {
    // MODULE-CONST: integration copied a module's functions, structs,
    // impls and tests, and not its `const`s — so a module could not
    // read a name its own file declared two lines up
    // (`[E0003] Identifier 'K' not found`, reported against a line of
    // the module). That is why `core/std/poll.t` spells its flags as
    // `pub fn interest_read()` rather than `pub const`.
    //
    // Both shapes, because a `const` array takes a different path
    // through the lowering than a scalar (CONST-ARRAY): it is laid
    // out as read-only bytes and indexed, rather than folded to a
    // literal.
    let pkg = scratch("module_consts");
    write(
        &pkg,
        "src/tbl.t",
        r#"
const BASE: u64 = 100u64
const K: [u32; 4] = [11u32, 22u32, 33u32, 44u32]

pub fn pick(i: u64) -> u64 {
    BASE + K[i] as u64
}
"#,
    );
    write(
        &pkg,
        "main.t",
        r#"
fn main() -> u64 {
    println(tbl::pick(2u64))
    0u64
}
"#,
    );
    for backend in ["vm", "aot"] {
        let out = run(&pkg, &["run", pkg.0.to_str().unwrap(), "--backend", backend]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("133"),
            "{backend}: stdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn a_trait_in_a_module_carries_its_contracts() {
    // TRAIT-CONTRACT-EXPRREF: a trait method signature's `requires` /
    // `ensures` are `ExprRef`s into the module's pool, and
    // integration copied them across **unmapped** — so each clause
    // pointed at whatever the main pool held at that index. What came
    // out was `[E0010] requires clause must be of type bool, got
    // Unknown`, with the caret on an unrelated line of an unrelated
    // file. `trait Digest` kept its promises in prose because of it,
    // and DBC-LISKOV's `[E0023]` was telling authors to "move the
    // clause to the trait" — an instruction that could not be
    // followed.
    let pkg = scratch("trait_contracts");
    write(
        &pkg,
        "src/shapes.t",
        r#"
pub trait Bounded {
    fn at(&self, i: u64) -> u64
        requires i < 4u64
    fn size(&self) -> u64
        ensures result > 0u64
}

pub struct Four { base: u64 }

impl Bounded for Four {
    fn at(&self, i: u64) -> u64 { self.base + i }
    fn size(&self) -> u64 { 4u64 }
}
"#,
    );
    write(
        &pkg,
        "main.t",
        r#"
fn main() -> u64 {
    val f = Four { base: 10u64 }
    println(f.at(2u64))
    println(f.size())
    f.at(9u64)
}
"#,
    );
    let out = run(&pkg, &["run", pkg.0.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("12") && stdout.contains("4"),
        "the contracted calls should run: stdout: {stdout}\nstderr: {stderr}"
    );
    // The inherited clause is checked, and it is reported against the
    // line of the trait that declares it.
    assert!(
        stderr.contains("Contract violation") && stderr.contains("requires"),
        "the trait's precondition should fire: {stderr}"
    );
    assert!(
        stderr.contains("shapes.t"),
        "and be reported in the file that declares it: {stderr}"
    );
}

#[test]
fn a_module_calls_its_own_function_not_the_programs() {
    // STDLIB-FN-SHADOWED-BY-USER-FN, in its quiet form. A bare call
    // in a module's body resolved to the *program's* function of that
    // name, so where the two signatures happened to agree the wrong
    // one was called and nothing said so: `lib2::report(2)` answered
    // 2000 (the program's `helper`) instead of 20 (its own).
    //
    // `import` makes a module's names visible to the program. It does
    // not work the other way round — a module is written without any
    // knowledge of what will import it.
    let pkg = scratch("module_own_helper");
    write(
        &pkg,
        "src/lib2.t",
        r#"
fn helper(n: u64) -> u64 { n * 10u64 }

pub fn report(n: u64) -> u64 {
    helper(n)
}
"#,
    );
    write(
        &pkg,
        "main.t",
        r#"
fn helper(n: u64) -> u64 { n * 1000u64 }

fn main() -> u64 {
    println(lib2::report(2u64))
    println(helper(2u64))
    0u64
}
"#,
    );
    for backend in ["vm", "aot"] {
        let out = run(&pkg, &["run", pkg.0.to_str().unwrap(), "--backend", backend]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("20\n") && stdout.contains("2000"),
            "{backend}: the module's own `helper` answers its call, the program's answers \
             the program's: stdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn a_module_carries_what_the_parser_recorded_about_it() {
    // Integration copies the pools and the declarations; two side
    // tables keyed by pool refs were left behind, so a fact the
    // parser recorded about a module's code was simply absent once
    // the module was imported.
    //
    // `parallel_loops` is the one with teeth: without it a
    // `parallel for` inside a module was an ordinary loop. The body
    // could `println` (E0029 never ran) and the lowering never
    // outlined it — the one construct whose answer was pinned
    // before it went parallel was not that construct at all.
    //
    // `call_paths` is MODULE-SYSTEM P3's silence, in the half nobody
    // had looked at: `zzz::tbl::f()` written *inside* a module.
    let pkg = scratch("module_side_tables");
    write(
        &pkg,
        "src/tbl.t",
        r#"
pub fn pick(i: u64) -> u64 { i + 1u64 }
"#,
    );
    write(
        &pkg,
        "src/loud.t",
        r#"
pub fn shout() -> u64 {
    parallel for k in 0u64..4u64 {
        println(k)
    }
    0u64
}
"#,
    );
    write(&pkg, "main.t", "fn main() -> u64 { loud::shout() }\n");
    let out = run(&pkg, &["run", pkg.0.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0029"),
        "a printing `parallel for` in a module is refused like any other: {stderr}"
    );
    // And it is reported in the file that contains it, with that
    // file's line — the snippet used to be drawn from the entry.
    assert!(
        stderr.contains("loud.t") && stderr.contains("parallel for k"),
        "reported where it is written: {stderr}"
    );

    // Now the written path, inside a module.
    write(
        &pkg,
        "src/loud.t",
        r#"
pub fn shout() -> u64 {
    zzz::tbl::pick(1u64)
}
"#,
    );
    let out = run(&pkg, &["run", pkg.0.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0030"),
        "a path that names nothing is refused inside a module too: {stderr}"
    );
}

#[test]
fn a_const_names_the_module_that_has_it() {
    // MODULE-CONST-PATH: a call's qualifier has been checked since
    // MODULE-SYSTEM P3, and a `const` is the other thing a module
    // exports — so `zzz::BASE` resolving quietly to `BASE` was the
    // same silence, in the half that had no check.
    //
    // Checking, not selecting: the namespace is flat, so a qualifier
    // cannot pick between two consts of one name. What it can do is
    // be wrong.
    let pkg = scratch("const_path");
    write(&pkg, "src/tbl.t", "pub const BASE: u64 = 500u64\n");
    write(
        &pkg,
        "main.t",
        r#"
fn main() -> u64 {
    println(tbl::BASE)
    0u64
}
"#,
    );
    let out = run(&pkg, &["run", pkg.0.to_str().unwrap()]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("500"),
        "the right qualifier resolves: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    write(
        &pkg,
        "main.t",
        r#"
fn main() -> u64 {
    println(zzz::BASE)
    0u64
}
"#,
    );
    let out = run(&pkg, &["run", pkg.0.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("E0030") && stderr.contains("tbl::BASE"),
        "a wrong one is refused, and says where the const is: {stderr}"
    );
}

#[test]
fn a_serial_test_has_the_machine_to_itself() {
    // TEST-PARALLEL P5. The default is parallel, so a test that
    // touches something the others also touch — a fixed path, a fixed
    // port, a file named relative to the working directory — needs a
    // way out that is not "`-j1` for the whole suite".
    //
    // The proof is what a shared file sees: two `serial` tests write
    // and then read the same path, which is exactly the race the
    // modifier exists to prevent. Running them beside eight other
    // tests (and each other) is what would break it.
    let pkg = scratch("serial_tests");
    write(&pkg, "main.t", "fn main() -> u64 { 0u64 }\n");
    for stem in ["b", "c", "d", "e"] {
        write(
            &pkg,
            &format!("tests/{stem}.t"),
            &format!(
                "test \"plain {stem} one\" {{ assert_eq(1u64, 1u64) }}\n\
                 test \"plain {stem} two\" {{ assert_eq(2u64, 2u64) }}\n"
            ),
        );
    }
    std::fs::create_dir_all(pkg.0.join("build")).expect("build dir");
    write(
        &pkg,
        "tests/a.t",
        r#"
test "runs beside the others" { assert_eq(1u64, 1u64) }

test "writes the shared file" serial {
    val wrote = io::write_file("build/serial-probe.txt", "xy")
    match wrote {
        Result::Ok(n) => { assert_eq(n, 2u64) }
        Result::Err(e) => { panic("write failed") }
    }
}

test "reads what the other one wrote" serial {
    val read = io::read_file("build/serial-probe.txt")
    match read {
        Result::Ok(s) => { assert_eq(s.len(), 2u64) }
        Result::Err(e) => { panic("read failed") }
    }
}

# The two modifiers are order-free: they say different things about
# the same test, and neither qualifies the other.
test "a serial test may also panic" serial panics "boom" { panic("boom") }
"#,
    );
    let path = pkg.0.to_str().unwrap();
    for lane in [vec!["test", path, "-j8"], vec!["test", path, "-j8", "--backend", "vm"]] {
        let out = run(&pkg, &lane);
        assert!(
            out.status.success(),
            "{lane:?} failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("12 passed"),
            "{lane:?}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    // TEST-PARALLEL X0 again, and the reason this test asserts about
    // files rather than only about the result: the driver binary is
    // named after the *file*, so a file holding both parallel and
    // serial tests wanted one path for two drivers — and a worker
    // executed a binary the other was still writing ("the test
    // driver ended before its first test (killed by a signal)").
    // It shows up as a flake, so the check is that the binaries are
    // distinct rather than that one run happened to pass.
    let built: Vec<String> = std::fs::read_dir(pkg.0.join("build/debug/tests"))
        .expect("tests dir")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let from_a: Vec<&String> = built.iter().filter(|n| n.starts_with("a_")).collect();
    assert!(
        from_a.len() >= 3,
        "tests/a.t needs a binary of its own for each serial test and one for the          rest; got {from_a:?}"
    );

    // The inventory says which tests are serial — it is the answer to
    // "why did this suite not go any faster".
    let listed = run(&pkg, &["test", path, "--list"]);
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(
        stdout.contains("writes the shared file  (tests/a.t:4)  serial"),
        "the listing marks them: {stdout}"
    );
    assert!(
        stdout.contains("runs beside the others  (tests/a.t:2)\n"),
        "and leaves the others alone: {stdout}"
    );
}
