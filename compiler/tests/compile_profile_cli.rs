//! COMPILE-PROFILE — `compiler --profile=compile`.
//!
//! Driven through the binary: what is pinned is the report a person
//! (or a script) reads — which phases appear, in what order, that the
//! source counts describe the files actually read, and that the report
//! stays on stderr so `--format=json`'s build document on stdout is
//! untouched. Timings themselves are not pinned; they only have to be
//! consistent with each other.
//!
//! Set `COMPILER_E2E=skip` to opt out, same as the other e2e suites —
//! these link a native binary.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_compiler");

const PROGRAM: &str = "fn twice(n: u64) -> u64 {\n    n * 2u64\n}\n\nfn main() -> u64 {\n    twice(21u64)\n}\n";

fn skip_e2e() -> bool {
    std::env::var("COMPILER_E2E").map(|v| v == "skip").unwrap_or(false)
}

fn core_modules_dir() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../core").to_string()
}

/// A directory of this test's own, so parallel tests do not share
/// inputs or outputs.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("toy_compile_profile_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn compile(dir: &std::path::Path, source: &str, extra: &[&str]) -> std::process::Output {
    let input = dir.join("main.t");
    std::fs::write(&input, source).expect("write source");
    Command::new(BIN)
        .arg(&input)
        .arg("-o")
        .arg(dir.join("main"))
        .arg("--core-modules")
        .arg(core_modules_dir())
        .args(extra)
        .output()
        .expect("run compiler")
}

fn names(phases: &serde_json::Value) -> Vec<&str> {
    phases.as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect()
}

#[test]
fn the_json_report_names_every_phase_and_the_files_read() {
    if skip_e2e() {
        return;
    }
    let dir = scratch("json");
    let out = compile(&dir, PROGRAM, &["--profile=compile", "--format=json"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr:\n{stderr}");

    // stdout is still the build's own document.
    let built: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert_eq!(built["emit"], "exe");

    let report: serde_json::Value = serde_json::from_str(&stderr).expect("stderr is one JSON document");
    assert_eq!(
        names(&report["phases"]),
        [
            "read_source", "parse", "modules", "resolve_aliases", "recursive_types",
            "typecheck", "lower", "codegen", "link",
        ],
        "{report:#}"
    );
    let phase = |name: &str| {
        report["phases"].as_array().unwrap().iter().find(|p| p["name"] == name).unwrap().clone()
    };
    assert_eq!(names(&phase("lower")["children"]), ["declare", "bodies", "finish"]);
    assert_eq!(
        names(&phase("codegen")["children"]),
        ["declare", "compile_functions", "define", "emit_object"]
    );

    // The phases are sequential and inside the total.
    let total = report["total_ms"].as_f64().unwrap();
    let walls: f64 = report["phases"].as_array().unwrap().iter().map(|p| p["wall_ms"].as_f64().unwrap()).sum();
    assert!(walls <= total + 0.01, "phases {walls} ms exceed the total {total} ms");

    // The entry is the file written above; the stdlib came along.
    let sources = &report["sources"];
    assert_eq!(sources["entry"]["bytes"], PROGRAM.len());
    assert_eq!(sources["entry"]["lines"], PROGRAM.lines().count());
    let files = sources["files"].as_array().unwrap();
    assert!(files.iter().any(|f| f["origin"] == "stdlib"), "{sources:#}");
    assert!(files.iter().any(|f| f["origin"] == "prelude"), "{sources:#}");
    let module_bytes: u64 = files.iter().filter(|f| f["origin"] != "entry").map(|f| f["bytes"].as_u64().unwrap()).sum();
    assert_eq!(sources["modules"]["bytes"], module_bytes);

    let counters = &report["counters"];
    assert_eq!(counters["typecheck.functions_entry"], 2);
    assert!(counters["lower.functions_lowered"].as_u64().unwrap() >= 2);
    assert!(counters["link.exe_bytes"].as_u64().unwrap() > 0);

    let lowered: Vec<_> = report["hot"]["lower"].as_array().unwrap().iter().map(|h| h["name"].as_str().unwrap()).collect();
    assert!(lowered.contains(&"main"), "{lowered:?}");
    let compiled = report["hot"]["codegen"].as_array().unwrap();
    assert!(compiled.iter().all(|h| h["code_bytes"].as_u64().unwrap() > 0), "{compiled:?}");
}

#[test]
fn a_failed_compile_still_reports_how_far_it_got() {
    if skip_e2e() {
        return;
    }
    let dir = scratch("failed");
    let out = compile(&dir, "fn main() -> u64 {\n    val x: u64 = true\n    x\n}\n", &["--profile=compile"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.starts_with("compile profile:"), "stderr:\n{stderr}");
    assert!(stderr.lines().any(|l| l.starts_with("typecheck ")), "stderr:\n{stderr}");
    assert!(!stderr.lines().any(|l| l.starts_with("lower ")), "stderr:\n{stderr}");
    assert!(stderr.contains("[E0001]"), "stderr:\n{stderr}");
}

#[test]
fn the_profile_is_refused_where_it_would_mean_nothing() {
    let dir = scratch("refused");
    let out = compile(&dir, PROGRAM, &["--profile=compile", "--all-backends"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("drop --all-backends"));

    let out = compile(&dir, PROGRAM, &["--profile=speed"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("`mem` or `compile`"));
}
