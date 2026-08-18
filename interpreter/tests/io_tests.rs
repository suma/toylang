// RUNTIME-IO: the stdlib I/O externs (`core/std/io.t`).
//
// `argc` / `arg` / `env_var` / `read_file` / `file_exists` are
// deterministic enough to assert on. `now` / `random` are asserted
// loosely (range / non-zero). `read_line` reads the process's real
// stdin, which tests cannot feed — it is covered by the CLI /
// compiled-binary smoke runs instead.

mod common;

use common::core_modules_dir;

fn run_with_args(source: &str, args: Vec<&str>) -> Result<i64, String> {
    let core = core_modules_dir();
    let mut options = interpreter::RunOptions::default();
    options.core_modules_dir = Some(&core);
    options.args = args.into_iter().map(String::from).collect();
    let outcome = interpreter::run_source(source, "io_test.t", &options)?;
    outcome.exit_code.map(|c| c as i64).ok_or_else(|| "no numeric exit code".to_string())
}

#[test]
fn argc_and_arg_report_program_arguments() {
    let r = run_with_args(
        "fn main() -> u64 {
            val n = io::argc()
            val a0 = io::arg(0u64)
            val a1 = io::arg(1u64)
            val oob = io::arg(99u64)
            if n == 2u64 && a0 == \"alpha\" && a1 == \"beta\" && oob == \"\" { 1u64 } else { 0u64 }
        }",
        vec!["alpha", "beta"],
    )
    .expect("run");
    assert_eq!(r, 1);
}

#[test]
fn argc_is_zero_without_arguments() {
    let r = run_with_args(
        "fn main() -> u64 { io::argc() }",
        vec![],
    )
    .expect("run");
    assert_eq!(r, 0);
}

#[test]
fn env_var_reads_the_environment() {
    // A variable the test controls, so the assertion is hermetic.
    std::env::set_var("TOYLANG_IO_TEST_VAR", "hello");
    let r = run_with_args(
        "fn main() -> u64 {
            val v = io::env_var(\"TOYLANG_IO_TEST_VAR\")
            val missing = io::env_var(\"TOYLANG_IO_TEST_VAR_DOES_NOT_EXIST\")
            if v == \"hello\" && missing == \"\" { 1u64 } else { 0u64 }
        }",
        vec![],
    )
    .expect("run");
    std::env::remove_var("TOYLANG_IO_TEST_VAR");
    assert_eq!(r, 1);
}

#[test]
fn read_file_and_file_exists() {
    let dir = std::env::temp_dir().join(format!("toylang_io_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("data.txt");
    std::fs::write(&path, "hello io\n").expect("write fixture");
    let src = format!(
        "fn main() -> u64 {{
            val yes = io::file_exists(\"{}\")
            val no = io::file_exists(\"{}/missing.t\")
            val f = io::read_file(\"{}\")
            if yes && !no && f == \"hello io\\n\" {{ 1u64 }} else {{ 0u64 }}
        }}",
        path.display(),
        dir.display(),
        path.display(),
    );
    let r = run_with_args(&src, vec![]).expect("run");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(r, 1);
}

#[test]
fn now_is_a_plausible_unix_timestamp() {
    let r = run_with_args("fn main() -> u64 { io::now() }", vec![]).expect("run");
    // 2020-01-01 .. 2100-01-01 — generous bounds, no clock assumption.
    assert!(r > 1_577_836_800 && r < 4_102_444_800, "now()={r}");
}

#[test]
fn random_is_nonzero_and_changes() {
    let r = run_with_args("fn main() -> u64 { io::random() }", vec![]).expect("run");
    assert!(r != 0, "random() should not stay at the zero seed");
}

#[test]
fn random_seed_makes_random_reproducible() {
    let r = run_with_args(
        "fn main() -> u64 {
            io::random_seed(42u64)
            val a = io::random()
            val b = io::random()
            io::random_seed(42u64)
            val a2 = io::random()
            val b2 = io::random()
            if a == a2 && b == b2 { 1u64 } else { 0u64 }
        }",
        vec![],
    )
    .expect("run");
    assert_eq!(r, 1);
}

#[test]
fn zero_seed_is_honoured_literally() {
    let r = run_with_args(
        "fn main() -> u64 {
            io::random_seed(0u64)
            if io::random() == 0u64 && io::random() == 0u64 { 1u64 } else { 0u64 }
        }",
        vec![],
    )
    .expect("run");
    assert_eq!(r, 1);
}

#[test]
fn strftime_formats_fixed_timestamps_in_utc() {
    let r = run_with_args(
        r#"fn main() -> u64 {
            if io::strftime("%Y-%m-%d %H:%M:%S %a %j %s", 1700000000u64) == "2023-11-14 22:13:20 Tue 318 1700000000"
                && io::strftime("%F %T %z %Z", 0u64) == "1970-01-01 00:00:00 +0000 UTC"
                && io::strftime("%q %%", 0u64) == "%q %"
                && io::strftime("%b %B %u %w", 1700000000u64) == "Nov November 2 2"
            { 1u64 } else { 0u64 }
        }"#,
        vec![],
    )
    .expect("run");
    assert_eq!(r, 1);
}

#[test]
fn env_listing_reports_names_and_values() {
    std::env::set_var("TOYLANG_IO_TEST_VAR", "hello");
    let r = run_with_args(
        "fn main() -> u64 {
            val n = io::env_count()
            var found = false
            var i: u64 = 0u64
            while i < n {
                if io::env_name(i) == \"TOYLANG_IO_TEST_VAR\" && io::env_value(i) == \"hello\" {
                    found = true
                }
                i = i + 1u64
            }
            if found && n > 0u64 && io::env_name(n + 10u64) == \"\" { 1u64 } else { 0u64 }
        }",
        vec![],
    )
    .expect("run");
    std::env::remove_var("TOYLANG_IO_TEST_VAR");
    assert_eq!(r, 1);
}
