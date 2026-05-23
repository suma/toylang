//! Phase 4c integration tests for the Full AST cache.
//!
//! Each test uses an isolated `TOY_CACHE_DIR` via `tempfile::tempdir`
//! and is marked `#[serial]` because the cache directory is selected
//! by an env var (process-global mutable state).

use interpreter::{RunOptions, RunOutcome, run_source};
use serial_test::serial;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const SAMPLE: &str = r#"
fn fib(n: u64) -> u64 {
    if n <= 1u64 {
        n
    } else {
        fib(n - 1u64) + fib(n - 2u64)
    }
}

fn main() -> u64 {
    fib(7u64)
}
"#;

/// Set `TOY_CACHE_DIR` for the duration of one test. Restores the
/// previous value (or unsets) on drop.
struct CacheEnv {
    _dir: TempDir,
    prev_cache_dir: Option<String>,
    prev_disable: Option<String>,
}

impl CacheEnv {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let prev_cache_dir = std::env::var("TOY_CACHE_DIR").ok();
        let prev_disable = std::env::var("TOY_CACHE_DISABLE").ok();
        unsafe {
            std::env::set_var("TOY_CACHE_DIR", dir.path());
            std::env::remove_var("TOY_CACHE_DISABLE");
        }
        Self {
            _dir: dir,
            prev_cache_dir,
            prev_disable,
        }
    }

    fn cache_path(&self) -> &Path {
        self._dir.path()
    }

    fn disable_cache(&self) {
        unsafe {
            std::env::set_var("TOY_CACHE_DISABLE", "1");
        }
    }

    fn enable_cache(&self) {
        unsafe {
            std::env::remove_var("TOY_CACHE_DISABLE");
        }
    }
}

impl Drop for CacheEnv {
    fn drop(&mut self) {
        unsafe {
            match &self.prev_cache_dir {
                Some(v) => std::env::set_var("TOY_CACHE_DIR", v),
                None => std::env::remove_var("TOY_CACHE_DIR"),
            }
            match &self.prev_disable {
                Some(v) => std::env::set_var("TOY_CACHE_DISABLE", v),
                None => std::env::remove_var("TOY_CACHE_DISABLE"),
            }
        }
    }
}

fn run(source: &str) -> RunOutcome {
    run_source(source, "test.t", &RunOptions::default()).expect("run_source")
}

fn count_full_files(cache_dir: &Path) -> usize {
    let mut n = 0;
    if !cache_dir.exists() {
        return 0;
    }
    for prefix_entry in fs::read_dir(cache_dir).into_iter().flatten().flatten() {
        if !prefix_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        for entry in fs::read_dir(prefix_entry.path()).into_iter().flatten().flatten() {
            if entry.path().extension().and_then(|s| s.to_str()) == Some("full") {
                n += 1;
            }
        }
    }
    n
}

#[test]
#[serial]
fn warm_run_matches_cold_run() {
    let env = CacheEnv::new();

    // Cold: cache empty, populate it during this run.
    let cold = run(SAMPLE);
    assert_eq!(cold.exit_code, Some(13)); // fib(7) = 13

    // Cache must contain at least the core stdlib modules after a
    // cold run.
    let after_cold = count_full_files(env.cache_path());
    assert!(
        after_cold > 0,
        ".full files should be populated by the cold run (found {})",
        after_cold
    );

    // Warm: same source, second invocation, must produce the same
    // exit code as the cold run.
    let warm = run(SAMPLE);
    assert_eq!(warm.exit_code, cold.exit_code);
}

#[test]
#[serial]
fn disable_env_var_skips_cache() {
    let env = CacheEnv::new();
    env.disable_cache();

    // First run with cache disabled: no .full files should be saved.
    let first = run(SAMPLE);
    assert_eq!(first.exit_code, Some(13));
    assert_eq!(
        count_full_files(env.cache_path()),
        0,
        "TOY_CACHE_DISABLE=1 must suppress save"
    );

    // Re-enable cache and confirm the next run populates entries.
    env.enable_cache();
    let _ = run(SAMPLE);
    assert!(
        count_full_files(env.cache_path()) > 0,
        "removing TOY_CACHE_DISABLE must re-enable save"
    );
}

#[test]
#[serial]
fn cache_is_reused_across_runs_with_same_stdlib() {
    // Only integrated modules (auto-loaded stdlib) are cached — the
    // user's own source is parsed by `CompilerSession` directly and
    // does not flow through `integrate_module_into_program_with_options_full`.
    // So running two distinct user programs that share the same
    // stdlib must keep the cache entry count stable across the
    // second run while still producing each program's correct
    // result.
    let env = CacheEnv::new();

    let cold = run(SAMPLE);
    assert_eq!(cold.exit_code, Some(13));
    let n_first = count_full_files(env.cache_path());
    assert!(n_first > 0, "stdlib must be cached after the cold run");

    let modified = SAMPLE.replace("fib(7u64)", "fib(8u64)");
    let warm = run(&modified);
    assert_eq!(warm.exit_code, Some(21)); // fib(8) = 21

    // Stdlib hashes are unchanged, so the cache directory shouldn't
    // grow new entries on the second run.
    let n_second = count_full_files(env.cache_path());
    assert_eq!(
        n_first, n_second,
        "cache entry count must not grow when only the user source changes (stdlib unchanged)"
    );
}

#[test]
#[serial]
fn corrupt_cache_falls_back_to_parse() {
    use std::io::Write;
    let env = CacheEnv::new();

    let cold = run(SAMPLE);
    let cache_root = env.cache_path();
    assert!(count_full_files(cache_root) > 0);

    // Truncate every .full file in the cache. A subsequent run must
    // ignore the corrupt entries and rebuild via the slow path while
    // still producing the same exit code.
    for prefix_entry in fs::read_dir(cache_root).unwrap().flatten() {
        for entry in fs::read_dir(prefix_entry.path()).unwrap().flatten() {
            if entry.path().extension().and_then(|s| s.to_str()) == Some("full") {
                let path = entry.path();
                let bytes = fs::read(&path).unwrap();
                let mut handle = fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&path)
                    .unwrap();
                handle.write_all(&bytes[..bytes.len() / 4]).unwrap();
            }
        }
    }

    let after_corrupt = run(SAMPLE);
    assert_eq!(after_corrupt.exit_code, cold.exit_code);
}
