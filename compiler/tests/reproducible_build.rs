//! The same source must compile to the same bytes.
//!
//! This is not an aesthetic property. `driver.rs` caches linked
//! binaries under a hash of the object bytes, and `cc` is ~70% of an
//! AOT build's wall clock — so a compiler whose output drifts run to
//! run turns that cache into a directory that only ever grows. It did
//! exactly that: eleven invocations produced eleven cache entries and
//! zero hits, while the doc comment claimed a hit rate approaching
//! 100%.
//!
//! Two `HashMap`/`HashSet` iterations were behind it, in the passes
//! that assign `FuncId`s and lay out `.rodata`. Rust seeds each
//! `RandomState` differently, so the order changed on every run and
//! carried all the way into the emitted object.
//!
//! **These tests spawn the binary rather than calling the library.**
//! An in-process comparison is a weaker check: it shares one process's
//! hashing state, which is the thing that varies.
//!
//! Set `COMPILER_E2E=skip` to opt out, same as the other e2e suites.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_compiler");

fn skip_e2e() -> bool {
    std::env::var("COMPILER_E2E").map(|v| v == "skip").unwrap_or(false)
}

fn core_modules_dir() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../core").to_string()
}

fn unique_dir(stem: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let p = std::env::temp_dir().join(format!("toy_repro_{stem}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&p).expect("create temp dir");
    p
}

/// Compile `source` to an object file in a fresh process.
fn emit_object(dir: &Path, source: &str, name: &str, link_cache: Option<&Path>) -> Vec<u8> {
    // One source path for every call. DEBUG-OBS D3 embeds the file
    // name in each panic site's `.rodata` diagnostic, so compiling
    // `a.t` and `b.t` would differ in the bytes that name the file —
    // which is correct, and not what this test is asking about.
    let src_path = dir.join("prog.t");
    std::fs::write(&src_path, source).expect("write source");
    let out = dir.join(format!("{name}.o"));
    let mut cmd = Command::new(BIN);
    cmd.arg(&src_path)
        .arg("--emit=obj")
        .arg("-o")
        .arg(&out)
        .arg("--core-modules")
        .arg(core_modules_dir());
    if let Some(cache) = link_cache {
        cmd.env("TOY_LINK_CACHE_DIR", cache);
    }
    let status = cmd.output().expect("spawn compiler");
    assert!(
        status.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    std::fs::read(&out).expect("read object")
}

/// Compile `source` to Cranelift IR text in a fresh process.
fn emit_clif(dir: &Path, source: &str, name: &str) -> String {
    let src_path = dir.join("prog.t");
    std::fs::write(&src_path, source).expect("write source");
    let out = dir.join(format!("{name}.clif"));
    let status = Command::new(BIN)
        .arg(&src_path)
        .arg("--emit=clif")
        .arg("-o")
        .arg(&out)
        .arg("--core-modules")
        .arg(core_modules_dir())
        .output()
        .expect("spawn compiler");
    assert!(
        status.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    std::fs::read_to_string(&out).expect("read clif")
}

/// A program broad enough to exercise the passes that were unordered:
/// generic methods (which get monomorphised lazily, so their `FuncId`s
/// follow the order bodies are lowered in), several distinct panic and
/// print strings (which land in `.rodata` in definition order), and a
/// trait impl.
const BROAD_PROGRAM: &str = r#"
struct Counter { n: i64 }

trait Describe {
    fn describe(&self) -> str
}

impl Describe for Counter {
    fn describe(&self) -> str { "counter" }
}

impl Counter {
    fn bump(&mut self) { self.n = self.n + 1i64 }
    fn get(&self) -> i64 { self.n }
}

fn checked(n: i64) -> i64 {
    if n < 0i64 { panic("negative input") }
    if n > 100i64 { panic("input too large") }
    n
}

fn main() -> u64 {
    var c = Counter { n: 0i64 }
    c.bump()
    c.bump()
    val v: Vec<u64> = Vec::new()
    val s = String::from_str("hi")
    println("start")
    println(c.describe())
    println("done")
    checked(c.get()) as u64 + s.len() + v.size()
}
"#;

#[test]
fn the_same_source_compiles_to_the_same_object_bytes() {
    if skip_e2e() {
        return;
    }
    let dir = unique_dir("bytes");
    let first = emit_object(&dir, BROAD_PROGRAM, "a", None);
    let second = emit_object(&dir, BROAD_PROGRAM, "b", None);
    let third = emit_object(&dir, BROAD_PROGRAM, "c", None);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(first.len(), second.len(), "object size is not reproducible");
    assert!(
        first == second && second == third,
        "the same program produced different object bytes across processes; \
         something in lowering or codegen is iterating a HashMap/HashSet"
    );
}

/// The object bytes were already reproducible when this was written; the
/// *text* was not. `declare_imports` walked a `HashMap<FuncId, _>` and
/// cranelift numbers imports in declaration order, so the same call came
/// out as `fn3` in one run and `fn24` in the next, and every `call`
/// referring to it moved with it.
///
/// That only ever hurt a reader -- and anyone diffing two `--emit clif`
/// dumps to check a codegen change did not alter what is emitted, which
/// is the one thing the dump is for.
#[test]
fn the_same_source_emits_the_same_clif_text() {
    if skip_e2e() {
        return;
    }
    let dir = unique_dir("clif");
    let first = emit_clif(&dir, BROAD_PROGRAM, "a");
    let second = emit_clif(&dir, BROAD_PROGRAM, "b");
    let third = emit_clif(&dir, BROAD_PROGRAM, "c");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        first == second && second == third,
        "the same program produced different CLIF text across processes; \
         something feeding cranelift's declaration order is iterating a \
         HashMap/HashSet"
    );
}

#[test]
fn an_unchanged_program_hits_the_link_cache() {
    if skip_e2e() {
        return;
    }
    // The property users actually get: linking the same program twice
    // costs one `cc` invocation, not two. Asserted through the cache
    // directory because that is where the failure was visible — one
    // entry per invocation, forever.
    let dir = unique_dir("linkcache");
    let cache = dir.join("cache");
    std::fs::create_dir_all(&cache).expect("create cache dir");
    let src = dir.join("p.t");
    std::fs::write(&src, BROAD_PROGRAM).expect("write source");

    let mut built = 0;
    for i in 0..3 {
        let out = dir.join(format!("exe{i}"));
        let result = Command::new(BIN)
            .arg(&src)
            .arg("-o")
            .arg(&out)
            .arg("--core-modules")
            .arg(core_modules_dir())
            .env("TOY_LINK_CACHE_DIR", &cache)
            .output()
            .expect("spawn compiler");
        assert!(
            result.status.success(),
            "compile failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        built += 1;
    }

    let entries = std::fs::read_dir(&cache)
        .expect("read cache dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "bin"))
        .count();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        entries, 1,
        "{built} builds of one unchanged program left {entries} link-cache entries; \
         one means every build after the first was a hit"
    );
}

#[test]
fn a_cached_link_produces_a_working_binary() {
    if skip_e2e() {
        return;
    }
    // The cache hands back a copy rather than a fresh link, so the
    // copy has to keep the executable bit and still run.
    let dir = unique_dir("linkrun");
    let cache = dir.join("cache");
    let src = dir.join("p.t");
    std::fs::write(&src, "fn main() -> u64 { 7u64 }\n").expect("write source");

    let mut codes = Vec::new();
    for i in 0..2 {
        let out = dir.join(format!("exe{i}"));
        let built = Command::new(BIN)
            .arg(&src)
            .arg("-o")
            .arg(&out)
            .arg("--core-modules")
            .arg(core_modules_dir())
            .env("TOY_LINK_CACHE_DIR", &cache)
            .output()
            .expect("spawn compiler");
        assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
        codes.push(Command::new(&out).status().expect("run binary").code());
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(codes, vec![Some(7), Some(7)], "the cached binary did not run");
}
