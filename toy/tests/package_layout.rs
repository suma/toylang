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
    let exe = pkg.0.join("build").join(pkg.0.file_name().unwrap());
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
