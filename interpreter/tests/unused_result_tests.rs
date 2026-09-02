//! MUST-USE: a `Result` produced and dropped on the floor.
//!
//! The language has no exceptions by design, so a failure travels in
//! the return value or not at all. `?` gave it a way to travel;
//! nothing made forgetting visible, and a program that ignored a
//! failed write reported success.
//!
//! These pin both halves: what is reported, and — more importantly —
//! what is not. A warning nobody can silence, or one that fires where
//! the value was obviously wanted, is worse than none.


fn warnings_for(source: &str) -> Vec<String> {
    let mut session = compiler_core::CompilerSession::new();
    let mut program = session
        .parse_program_all_errors(source, "test.t")
        .expect("parse");
    let core = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../core"));
    interpreter::check_typing_diagnostics(
        &mut program,
        session.string_interner_mut(),
        Some(source),
        Some("test.t"),
        Some(core.as_path()),
    )
    .expect("the program type-checks; the findings are warnings")
    .iter()
    .map(|d| format!("[{}] {}", d.code, d.message))
    .collect()
}

fn unused_result_warnings(source: &str) -> Vec<String> {
    warnings_for(source)
        .into_iter()
        .filter(|w| w.contains("E0025"))
        .collect()
}

/// A discarded call, and the message naming what produced the value.
#[test]
fn a_discarded_result_is_warned_about() {
    let warnings = unused_result_warnings(
        r#"
        fn risky(n: u64) -> Result<u64, str> {
            if n == 0u64 { Result::Err("zero") } else { Result::Ok(n) }
        }
        fn main() -> u64 {
            risky(1u64)
            0u64
        }
        "#,
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("`risky(...)`"), "{warnings:?}");
    assert!(warnings[0].contains("Result<u64, str>"), "{warnings:?}");
}

/// The shape the check exists for: an IO call whose failure would
/// otherwise be invisible.
#[test]
fn a_discarded_io_result_is_warned_about() {
    let warnings = unused_result_warnings(
        r#"
        fn main() -> u64 {
            io::write_file("out.txt", "body")
            0u64
        }
        "#,
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("write_file"), "{warnings:?}");
}

/// The three ways out, none of which may warn. The third is the
/// escape hatch, and it needs no syntax of its own — binding the
/// value is what says the result was considered.
#[test]
fn handling_propagating_or_binding_the_result_is_quiet() {
    let warnings = unused_result_warnings(
        r#"
        fn risky(n: u64) -> Result<u64, str> {
            if n == 0u64 { Result::Err("zero") } else { Result::Ok(n) }
        }

        fn handled() -> u64 {
            match risky(1u64) {
                Result::Ok(v) => v,
                Result::Err(_) => 0u64,
            }
        }

        fn propagated() -> Result<u64, str> {
            val v = risky(2u64)?
            Result::Ok(v)
        }

        fn ignored() -> u64 {
            val _ignored = risky(3u64)
            7u64
        }

        fn main() -> u64 { handled() + ignored() }
        "#,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// A block's last statement is its value, so it is not discarded
/// here — whether the enclosing position wants it is a question this
/// pass cannot answer from the block alone. Reporting it would fire
/// on every `Result`-returning function's tail.
#[test]
fn a_tail_result_is_not_discarded() {
    let warnings = unused_result_warnings(
        r#"
        fn risky(n: u64) -> Result<u64, str> {
            if n == 0u64 { Result::Err("zero") } else { Result::Ok(n) }
        }
        fn forwarded() -> Result<u64, str> {
            risky(4u64)
        }
        fn main() -> u64 {
            match forwarded() {
                Result::Ok(v) => v,
                Result::Err(_) => 0u64,
            }
        }
        "#,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// `Option` is out of scope: an ignored one is usually a lookup whose
/// absence is the answer. Pinned so widening the rule is a decision
/// rather than a slip.
#[test]
fn a_discarded_option_is_not_warned_about() {
    let warnings = unused_result_warnings(
        r#"
        fn maybe(n: u64) -> Option<u64> {
            if n == 0u64 { Option::None } else { Option::Some(n) }
        }
        fn main() -> u64 {
            maybe(1u64)
            0u64
        }
        "#,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// A discarded value that is not a `Result` at all — the check must
/// not fire on ordinary statement expressions.
#[test]
fn a_discarded_scalar_is_not_warned_about() {
    let warnings = unused_result_warnings(
        r#"
        fn count() -> u64 { 3u64 }
        fn main() -> u64 {
            count()
            println("done")
            0u64
        }
        "#,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// The stdlib is checked alongside the user's program, so a warning
/// there would land on every single compilation. This is the canary
/// for that.
#[test]
fn the_stdlib_discards_no_results() {
    let warnings = unused_result_warnings(r#"fn main() -> u64 { 0u64 }"#);
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// Every discarded statement in a block is reported, not just the
/// first — a loop body that ignores two writes has two bugs.
#[test]
fn each_discarded_result_is_reported() {
    let warnings = unused_result_warnings(
        r#"
        fn risky(n: u64) -> Result<u64, str> { Result::Ok(n) }
        fn main() -> u64 {
            risky(1u64)
            risky(2u64)
            0u64
        }
        "#,
    );
    assert_eq!(warnings.len(), 2, "{warnings:?}");
}
