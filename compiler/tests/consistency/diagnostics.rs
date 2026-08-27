//! DEBUG-OBS D0: what each engine says when a program *fails*.
//!
//! The rest of this suite compares answers — exit codes, stdout,
//! allocation totals. That is why four engines could drift into four
//! renderings of the same panic with every test still green
//! (`design-docs/DEBUG_OBSERVABILITY.md`, 実測 1).
//!
//! The tests below pin the divergence as it stands. They are green on
//! purpose: the gap is real and D1–D4 close it, and a suite that is
//! permanently red would cost more than the reminder is worth. Each
//! one becomes an `assert_diagnostic_consistent` call the moment the
//! engines agree — `assert_diagnostic_report` fails and says so.

use super::harness::{assert_diagnostic_report, run_diagnostic_lane_child};

/// Re-entry point for the two lanes that end in `process::exit`.
///
/// A plain `cargo nextest run` reaches this with no lane requested and
/// it does nothing. `lane_in_child` re-invokes this same binary with
/// `TOY_DIAG_LANE` set, and *that* process runs the program and dies
/// with its diagnostic on stderr.
#[test]
fn diagnostic_lane_child() {
    run_diagnostic_lane_child();
}

/// 実測 1 verbatim: a `panic` three calls deep.
///
/// Reading the pinned text top to bottom is the D1–D3 work list — the
/// location, the excerpt and the backtrace exist in exactly one lane,
/// and the frame that actually panicked (`c`) is the only one carrying
/// no call-site line.
#[test]
fn panic_three_calls_deep() {
    let source = r#"
fn c(n: u64) -> u64 { if n == 0u64 { panic("boom in c") } n - 1u64 }
fn b(n: u64) -> u64 { c(n) }
fn a(n: u64) -> u64 { b(n) }
fn main() -> u64 { a(0u64) }
"#;
    assert_diagnostic_report(
        source,
        "panic_three_calls_deep",
        r#"tree-walker (stderr):
  Runtime error occurred:
  Error at panic_three_calls_deep.t:2:38:
     |
   2 | fn c(n: u64) -> u64 { if n == 0u64 { panic("boom in c") } n - 1u64 }
     |                                      ^^^^^ panic: boom in c
     |
     = backtrace (innermost first):
         c (called at line 3)
         b (called at line 4)
         a (called at line 5)
         main
ir-vm (stderr):
  Runtime error occurred:
  boom in c
interpreter-jit (stderr):
  Runtime error occurred:
  panic: boom in c
compiler-jit (stdout):
  panic: boom in c
aot (stdout):
  panic: boom in c
"#,
    );
}

/// A `u64` underflow trap: the engines disagree about the *message*,
/// not just about how much context surrounds it. The tree-walker names
/// the two operands; the compiled runtimes have only a fixed string,
/// because the guard's helper takes no arguments.
#[test]
fn u64_underflow_trap() {
    let source = r#"
fn sub(a: u64, b: u64) -> u64 { a - b }
fn main() -> u64 { sub(1u64, 5u64) }
"#;
    assert_diagnostic_report(
        source,
        "u64_underflow_trap",
        r#"tree-walker (stderr):
  Runtime error occurred:
  Error at u64_underflow_trap.t:2:33:
     |
   2 | fn sub(a: u64, b: u64) -> u64 { a - b }
     |                                 ^ panic: u64 subtraction underflowed: 1 - 5
     |
     = backtrace (innermost first):
         sub (called at line 3)
         main
ir-vm (stderr):
  Runtime error occurred:
  u64 subtraction underflowed (left operand is smaller than the right)
interpreter-jit (stderr):
  Runtime error occurred:
  panic: u64 subtraction underflowed (left operand is smaller than the right)
compiler-jit (stdout):
  panic: u64 subtraction underflowed (left operand is smaller than the right)
aot (stdout):
  panic: u64 subtraction underflowed (left operand is smaller than the right)
"#,
    );
}

/// An out-of-bounds array index: three different sentences for one
/// failure, and the tree-walker's is the only one that is not even a
/// `panic:` line (it comes from a different error path, so it carries
/// no location either).
#[test]
fn array_index_out_of_bounds() {
    let source = r#"
fn main() -> u64 {
    val arr: [i64; 3] = [1i64, 2i64, 3i64]
    var i: u64 = 5u64
    val v: i64 = arr[i]
    0u64
}
"#;
    assert_diagnostic_report(
        source,
        "array_index_out_of_bounds",
        r#"tree-walker (stderr):
  Runtime error occurred:
  Array index 5 out of bounds for array of size 3
ir-vm (stderr):
  Runtime error occurred:
  array index out of bounds (index is at or past the array's length)
interpreter-jit (stderr):
  Runtime error occurred:
  Array index 5 out of bounds for array of size 3
compiler-jit (stdout):
  panic: array index out of bounds (index is at or past the array's length)
aot (stdout):
  panic: array index out of bounds (index is at or past the array's length)
"#,
    );
}

/// A `requires` violation. Contracts are the feature this language
/// asks people to lean on, and the tree-walker's message is the reason
/// why — it names the argument that broke the predicate. The compiled
/// backends reduce all of that to three words, so the same failing run
/// is diagnosable under one engine and a guessing game under another.
#[test]
fn requires_violation() {
    let source = r#"
fn f(n: u64) -> u64
    requires n > 0u64
{
    n
}
fn main() -> u64 { f(0u64) }
"#;
    assert_diagnostic_report(
        source,
        "requires_violation",
        r#"tree-walker (stderr):
  Runtime error occurred:
  Contract violation: `requires` clause #1 of function `f` evaluated to false (with n = 0)
ir-vm (stderr):
  Runtime error occurred:
  requires violation
interpreter-jit (stderr):
  Runtime error occurred:
  Contract violation: `requires` clause #1 of function `f` evaluated to false (with n = 0)
compiler-jit (stdout):
  panic: requires violation
aot (stdout):
  panic: requires violation
"#,
    );
}

/// A panic raised inside the stdlib (DEBUG-OBS D2).
///
/// The tree-walker names `core/std/option.t` and quotes that file's
/// line — before D2 it had no position for an imported node at all.
/// The other four engines say the message and nothing else, which is
/// what D3 changes: they carry no site, so they cannot name a file
/// they were never told about.
#[test]
fn panic_inside_the_stdlib() {
    let source = r#"
fn main() -> u64 {
    val o: Option<u64> = Option::None
    val v: u64 = o.unwrap()
    v
}
"#;
    assert_diagnostic_report(
        source,
        "panic_inside_the_stdlib",
        r#"tree-walker (stderr):
  Runtime error occurred:
  Error at core/std/option.t:57:29:
     |
  57 |             Option::None => panic("Option::unwrap on None"),
     |                             ^^^^^ panic: Option::unwrap on None
     |
     = backtrace (innermost first):
         Option::unwrap (called at line 4)
         main
ir-vm (stderr):
  Runtime error occurred:
  Option::unwrap on None
interpreter-jit (stderr):
  Runtime error occurred:
  Error at core/std/option.t:57:29:
     |
  57 |             Option::None => panic("Option::unwrap on None"),
     |                             ^^^^^ panic: Option::unwrap on None
     |
     = backtrace (innermost first):
         Option::unwrap (called at line 4)
         main
compiler-jit (stdout):
  panic: Option::unwrap on None
aot (stdout):
  panic: Option::unwrap on None
"#,
    );
}
