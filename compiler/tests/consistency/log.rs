//! `core/std/log.t` — the level, the filter and the line (STDLIB-LOG).
//!
//! Every test here compares stderr, not just stdout: a logging
//! library whose lanes write the same words to different descriptors
//! has not agreed, and `assert_consistent` alone cannot see the
//! difference.

use super::harness::{
    compiled_run_streams, compiled_run_streams_env, interpreter_streams, skip_e2e,
};

/// Run on the tree-walker and AOT, require both streams to match, and
/// hand back `(stdout, stderr)`.
fn both_lanes(source: &str, stem: &str) -> Option<(String, String)> {
    let (code, stdout, stderr) = compiled_run_streams(source, stem)?;
    assert_eq!(code, 0, "compiled binary exited {code}:\n{stderr}");
    let (interp_out, interp_err) = interpreter_streams(source);
    assert_eq!(interp_out, stdout, "tree-walker vs AOT stdout");
    assert_eq!(interp_err, stderr, "tree-walker vs AOT stderr");
    Some((stdout, stderr))
}

#[test]
fn the_level_decides_which_records_are_written() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            log::set_level(Level::Warn)
            log::error("e1")
            log::warn("w1")
            log::info("i1")
            log::debug("d1")
            log::trace("t1")
            log::set_level(Level::Trace)
            log::trace("t2")
            0u64
        }
    "#;
    let Some((stdout, stderr)) = both_lanes(src, "log_levels") else {
        return;
    };
    // `warn` admits the two levels at or above it and nothing below,
    // and the format is `LEVEL message` with a single space.
    assert_eq!(stderr, "ERROR e1\nWARN w1\nTRACE t2\n");
    // The point of §1: a program's own output is untouched by any of
    // this, so `prog > out.txt` still holds only what the program
    // printed.
    assert_eq!(stdout, "");
}

#[test]
fn enabled_answers_what_the_call_would_do() {
    if skip_e2e() {
        return;
    }
    // The predicate is the whole reason `enabled` exists: hoisting it
    // out of a loop is only correct if it agrees with the filter
    // inside `at`.
    let src = r#"
        fn main() -> u64 {
            log::set_level(Level::Info)
            val e: bool = log::enabled(Level::Error)
            val i: bool = log::enabled(Level::Info)
            val d: bool = log::enabled(Level::Debug)
            println("{e} {i} {d}")
            if log::enabled(Level::Debug) { log::debug("unreachable") }
            log::info("reached")
            0u64
        }
    "#;
    let Some((stdout, stderr)) = both_lanes(src, "log_enabled") else {
        return;
    };
    assert_eq!(stdout, "true true false\n");
    assert_eq!(stderr, "INFO reached\n");
}

#[test]
fn a_level_prints_as_its_name_and_survives_a_round_trip() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            println(Level::Error)
            println(log::level_name(Level::Trace))
            log::set_level(Level::Debug)
            println(log::current_level())
            println(log::level_rank(Level::Warn))
            0u64
        }
    "#;
    let Some((stdout, stderr)) = both_lanes(src, "log_level_names") else {
        return;
    };
    assert_eq!(stdout, "ERROR\nTRACE\nDEBUG\n1\n");
    assert_eq!(stderr, "");
}

#[test]
fn toy_log_sets_the_level_the_program_starts_at() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            log::warn("w")
            log::info("i")
            0u64
        }
    "#;
    // Set on the subprocess rather than on this process: see
    // `compiled_run_streams_env`.
    let Some((_, _, warn_err)) =
        compiled_run_streams_env(src, "log_env_warn", &[("TOY_LOG", "warn")])
    else {
        return;
    };
    assert_eq!(warn_err, "WARN w\n");

    let Some((_, _, debug_err)) =
        compiled_run_streams_env(src, "log_env_debug", &[("TOY_LOG", "debug")])
    else {
        return;
    };
    assert_eq!(debug_err, "WARN w\nINFO i\n");
}

#[test]
fn a_misspelt_toy_log_says_so_instead_of_going_quiet() {
    if skip_e2e() {
        return;
    }
    // The failure this prevents is silent: someone writes
    // `TOY_LOG=verbose`, sees no `debug` output, and concludes the
    // logging is broken rather than the spelling.
    let src = r#"
        fn main() -> u64 {
            log::info("i")
            0u64
        }
    "#;
    let Some((_, _, stderr)) =
        compiled_run_streams_env(src, "log_env_bad", &[("TOY_LOG", "verbose")])
    else {
        return;
    };
    assert!(
        stderr.contains("TOY_LOG must be one of error/warn/info/debug/trace"),
        "no warning for an unrecognised TOY_LOG:\n{stderr}"
    );
    // And it falls back to `info` rather than to silence.
    assert!(stderr.ends_with("INFO i\n"), "stderr was:\n{stderr}");
    // Once, not once per call.
    assert_eq!(stderr.matches("TOY_LOG must be").count(), 1);
}

#[test]
fn toy_log_time_prefixes_the_one_timestamp_format_this_stdlib_has() {
    if skip_e2e() {
        return;
    }
    // L2. Off by default, which is what lets every other test here
    // compare an exact string; on, the prefix is `DateTime::to_str`
    // rather than a second spelling of ISO 8601.
    let src = r#"
        fn main() -> u64 {
            log::error("boom")
            0u64
        }
    "#;
    let Some((_, _, stderr)) =
        compiled_run_streams_env(src, "log_env_time", &[("TOY_LOG_TIME", "1")])
    else {
        return;
    };
    let line = stderr.trim_end();
    let (stamp, rest) = line.split_once(' ').expect("a timestamp then the record");
    assert_eq!(rest, "ERROR boom", "record after the stamp:\n{stderr}");
    assert_eq!(stamp.len(), 20, "not an ISO 8601 instant: {stamp}");
    assert!(
        stamp.ends_with('Z') && stamp.as_bytes()[10] == b'T',
        "not an ISO 8601 instant: {stamp}"
    );
    assert!(
        stamp.starts_with("20"),
        "the clock is not reading a real date: {stamp}"
    );
}

#[test]
fn a_program_may_define_names_this_module_also_uses() {
    if skip_e2e() {
        return;
    }
    // An unqualified call in a stdlib body resolves to a user
    // function of the same name, so `log.t` calls its own functions
    // through `log::`. Without that, this program failed to type
    // check -- with the error pointing at a line of `log.t` rather
    // than at anything the program contains.
    let src = r#"
        fn at(n: u64) -> u64 { n + 1u64 }
        fn enabled(n: u64) -> u64 { n * 2u64 }
        fn info(n: u64) -> u64 { n * 10u64 }

        fn main() -> u64 {
            log::set_level(Level::Info)
            log::info("still logging")
            println(at(1u64) + enabled(2u64) + info(3u64))
            0u64
        }
    "#;
    let Some((stdout, stderr)) = both_lanes(src, "log_user_shadow") else {
        return;
    };
    assert_eq!(stdout, "36\n");
    assert_eq!(stderr, "INFO still logging\n");
}
