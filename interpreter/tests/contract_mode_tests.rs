//! Integration tests for the `INTERPRETER_CONTRACTS` env-var gate that
//! controls which Design-by-Contract clauses get evaluated at runtime.
//!
//! Each test spawns the interpreter binary so we exercise the same env-var
//! reading path users do. Programs are written so the contract status is
//! observable: a violation produces a non-zero exit and a "Contract
//! violation" message; a skipped clause lets the body's natural behaviour
//! through (a divide-by-zero panic, or a buggy return value reaching main).

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_interpreter");

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_with(env_value: Option<&str>, source_path: &str) -> Run {
    let mut cmd = Command::new(BIN);
    cmd.arg(source_path);
    // Detach the JIT to keep the test focused on the interpreter's contract
    // path; otherwise we'd be testing the JIT eligibility fallback too.
    cmd.env_remove("INTERPRETER_JIT");
    match env_value {
        Some(v) => {
            cmd.env("INTERPRETER_CONTRACTS", v);
        }
        None => {
            cmd.env_remove("INTERPRETER_CONTRACTS");
        }
    }
    let out = cmd.output().expect("failed to spawn interpreter binary");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Fixture programs, in a directory owned by the calling test.
///
/// Every test in this file used to write the *same two paths* under
/// `tests/fixtures/`. nextest runs each test in its own process, so a
/// test could be reading a file while another was mid-`fs::write` —
/// producing a truncated program, a parse error, and a failure that
/// vanished on re-run. A flaky test is worth fixing out of proportion
/// to how often it fires: the cost is not the one red run, it is the
/// investigation into whether the change under test broke something.
struct Fixtures {
    /// Deleted on drop; every path below points inside it, so the temp
    /// dir has to outlive the runs that read them.
    _dir: tempfile::TempDir,
    /// `requires b != 0i64` is violated by main.
    /// With pre-checks on, the call aborts with a contract-violation
    /// message. With pre-checks off, the body actually divides by zero.
    pre_violation: String,
    /// `ensures result >= 0i64` is violated by an implementation that
    /// lies about absolute value. With post-checks on, ensures catches
    /// the bug; with post-checks off, the lie reaches main.
    post_violation: String,
}

fn fixtures() -> Fixtures {
    use std::fs;

    let dir = tempfile::tempdir().expect("create fixture dir");

    let pre_violation = dir.path().join("contract_pre_violation.t");
    fs::write(
        &pre_violation,
        // Calling divide(20, 0) violates `requires b != 0i64`. Exit code is
        // unused — we assert against the message + status pattern instead.
        "fn divide(a: i64, b: i64) -> i64\n    \
         requires b != 0i64\n    \
         ensures result * b == a\n{\n    \
         a / b\n}\n\
         fn main() -> i64 { divide(20i64, 0i64) }\n",
    )
    .expect("write pre-violation fixture");

    let post_violation = dir.path().join("contract_post_violation.t");
    fs::write(
        &post_violation,
        // buggy_abs returns -x for any x, violating `ensures result >= 0i64`.
        "fn buggy_abs(x: i64) -> i64\n    \
         ensures result >= 0i64\n{\n    \
         -x\n}\n\
         fn main() -> i64 { buggy_abs(5i64) }\n",
    )
    .expect("write post-violation fixture");

    Fixtures {
        pre_violation: pre_violation.to_string_lossy().into_owned(),
        post_violation: post_violation.to_string_lossy().into_owned(),
        _dir: dir,
    }
}

#[test]
fn default_evaluates_both_pre_and_post() {
    let fx = fixtures();
    // Unset env var → default mode → requires fires first.
    let r = run_with(None, &fx.pre_violation);
    assert!(
        r.stderr.contains("Contract violation") && r.stderr.contains("requires"),
        "stdout: {}\nstderr: {}",
        r.stdout,
        r.stderr
    );

    // Default mode also runs ensures.
    let r = run_with(None, &fx.post_violation);
    assert!(
        r.stderr.contains("Contract violation") && r.stderr.contains("ensures"),
        "stdout: {}\nstderr: {}",
        r.stdout,
        r.stderr
    );
}

#[test]
fn all_value_matches_default() {
    let fx = fixtures();
    let r = run_with(Some("all"), &fx.pre_violation);
    assert!(r.stderr.contains("Contract violation"));
    assert!(r.stderr.contains("requires"));
}

#[test]
fn pre_value_runs_only_requires() {
    let fx = fixtures();
    // requires still catches the bad input...
    let r = run_with(Some("pre"), &fx.pre_violation);
    assert!(
        r.stderr.contains("Contract violation") && r.stderr.contains("requires"),
        "expected requires violation, got stdout={}\nstderr={}",
        r.stdout,
        r.stderr
    );

    // ...but ensures is skipped, so the buggy return value reaches main and
    // the program exits "successfully" without a contract message. The
    // process exit code carries the (negative) i64 result.
    let r = run_with(Some("pre"), &fx.post_violation);
    assert!(
        !r.stderr.contains("Contract violation"),
        "ensures should be skipped, got stdout={}",
        r.stdout
    );
}

#[test]
fn post_value_runs_only_ensures() {
    let fx = fixtures();
    // requires is skipped, so divide(20, 0) reaches the body and panics.
    // The Rust-level panic surfaces on stderr, *not* as a ContractViolation.
    let r = run_with(Some("post"), &fx.pre_violation);
    assert!(
        !r.stderr.contains("Contract violation"),
        "requires should be skipped, got stdout={}",
        r.stdout
    );
    assert_ne!(r.code, 0, "divide-by-zero should crash the process");

    // ensures still catches the buggy return.
    let r = run_with(Some("post"), &fx.post_violation);
    assert!(
        r.stderr.contains("Contract violation") && r.stderr.contains("ensures"),
        "expected ensures violation, got stdout={}\nstderr={}",
        r.stdout,
        r.stderr
    );
}

#[test]
fn off_value_disables_both() {
    let fx = fixtures();
    // Both clauses skipped; the divide body panics on the bad arg.
    let r = run_with(Some("off"), &fx.pre_violation);
    assert!(!r.stderr.contains("Contract violation"));
    assert_ne!(r.code, 0);

    // ensures skipped too — the buggy_abs program completes without
    // a contract diagnostic.
    let r = run_with(Some("off"), &fx.post_violation);
    assert!(!r.stderr.contains("Contract violation"));
}

#[test]
fn unknown_value_warns_and_falls_back_to_all() {
    let fx = fixtures();
    let r = run_with(Some("bogus"), &fx.pre_violation);
    // Warning goes to stderr; behaviour matches `all` so requires fires.
    assert!(
        r.stderr.contains("INTERPRETER_CONTRACTS") && r.stderr.contains("not recognised"),
        "expected warning on stderr, got: {}",
        r.stderr
    );
    assert!(r.stderr.contains("Contract violation") && r.stderr.contains("requires"));
}

#[test]
fn case_insensitive_value() {
    let fx = fixtures();
    // `OFF` should work the same as `off`.
    let r = run_with(Some("OFF"), &fx.pre_violation);
    assert!(!r.stderr.contains("Contract violation"));
}
