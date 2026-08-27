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

use super::harness::{assert_diagnostic_consistent, run_diagnostic_lane_child};

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
    assert_diagnostic_consistent(source, "panic_three_calls_deep");
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
    assert_diagnostic_consistent(source, "u64_underflow_trap");
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
    assert_diagnostic_consistent(source, "array_index_out_of_bounds");
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
    assert_diagnostic_consistent(source, "requires_violation");
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
    assert_diagnostic_consistent(source, "panic_inside_the_stdlib");
}

/// A recursive panic (DEBUG-OBS D4).
///
/// The folding is where the two backtrace renderers could most easily
/// drift: one is `compiler_ir::render_backtrace`, the other is its
/// hand-copied no_std twin in `toylang_rt` (this crate is
/// dependency-free by design, the same pairing
/// `format_alloc_budget_violation` already has). Nothing but a test
/// like this would notice them disagreeing.
#[test]
fn a_recursive_panic_folds_the_same_everywhere() {
    let source = r#"
fn f(n: u64) -> u64 {
    if n == 0u64 { panic("bottom") }
    f(n - 1u64)
}
fn main() -> u64 { f(7u64) }
"#;
    assert_diagnostic_consistent(source, "a_recursive_panic_folds_the_same_everywhere");
}

/// A method's frame is named by its type in every engine.
///
/// The compiled backends get that name from the IR's `display_name`,
/// set where the method is declared — the mangled `toy_S__boom` has
/// the monomorph's type arguments in it and would read as neither the
/// tree-walker's `S::boom` nor anything a user wrote.
#[test]
fn a_method_frame_is_named_the_same_everywhere() {
    let source = r#"
struct S { v: i64 }
impl S {
    fn boom(&self) -> i64 { panic("method boom") }
}
fn go(s: S) -> i64 { s.boom() }
fn main() -> u64 {
    val s = S { v: 1i64 }
    val r: i64 = go(s)
    0u64
}
"#;
    assert_diagnostic_consistent(source, "a_method_frame_is_named_the_same_everywhere");
}

/// `__builtin_backtrace()` reads the same stack in every engine
/// (DEBUG-OBS D5).
///
/// Pinned through the *stdout* lane rather than the diagnostic one:
/// the program prints the answer and lives, so this is an ordinary
/// consistency check — and it catches the two backtrace renderers
/// disagreeing from the other direction than a panic does.
#[test]
fn the_backtrace_builtin_agrees_across_backends() {
    let source = r#"
fn inner() -> str { __builtin_backtrace() }
fn outer() -> str { inner() }
fn main() -> u64 {
    println(outer())
    0u64
}
"#;
    super::harness::assert_stdout_consistent(source, "the_backtrace_builtin_agrees_across_backends");
}

/// Reading past the end of a `Vec` (DEBUG-OBS D6).
///
/// A built-in array traps on an out-of-range index; this was the one
/// indexed read that did not, and it failed *through the host* —
/// `value not defined` from inside the IR VM, with nothing left of the
/// toylang program. Now it is an ordinary panic, and every engine
/// reports it the same way.
#[test]
fn a_vec_read_past_the_end() {
    let source = r#"
fn main() -> u64 {
    var v: Vec<i64> = Vec::new()
    v.push(1i64)
    val x: i64 = v.get(5u64)
    0u64
}
"#;
    assert_diagnostic_consistent(source, "a_vec_read_past_the_end");
}

/// A violated allocation budget (ALLOC-CONTRACT-SUGAR).
///
/// The only diagnostic whose numbers are not known until it fires, so
/// its sentence is assembled from a static head laid in `.rodata` and
/// readings the runtime formats between it and the frame's tail. That
/// seam is exactly where the engines could drift apart, and nothing
/// but a comparison would notice.
#[test]
fn a_violated_allocation_budget() {
    let source = r#"
fn grow() -> u64
    ensures allocates(0u64)
{
    val p: ptr = __builtin_heap_alloc(64u64)
    1u64
}
fn main() -> u64 { grow() }
"#;
    assert_diagnostic_consistent(source, "a_violated_allocation_budget");
}
