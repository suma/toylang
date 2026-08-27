// LLM-LOOP P6 — a runtime failure says where it happened and why.
//
// Before this, `panic("boom")` reported exactly `panic: boom`: no line,
// no call path. A contract violation named the clause by index but not
// the values that made it false. Both left one option — add prints and
// run again — which is the round trip the whole LLM-LOOP effort exists
// to remove.

use crate::common;

use crate::common::test_program;

/// Run a program and return what it printed.
fn stdout_of(source: &str) -> String {
    let (result, out) = interpreter::output::with_capture(|| test_program(source));
    result.unwrap_or_else(|e| panic!("expected the program to run:\n{e}"));
    out
}

/// Run a program expected to fail at runtime and return the diagnostic.
fn runtime_failure(source: &str) -> String {
    match test_program(source) {
        Ok(v) => panic!("expected a runtime failure, got {v:?}"),
        Err(e) => e,
    }
}

// --- panic ------------------------------------------------------------

#[test]
fn panic_reports_the_line_it_fired_on() {
    let diags = runtime_failure(
        "fn boom() -> u64 {
            panic(\"kaboom\")
        }
        fn main() -> u64 { boom() }",
    );
    assert!(diags.contains("kaboom"), "{diags}");
    assert!(diags.contains("test.t:2:"), "expected the panic's own line:\n{diags}");
}

#[test]
fn panic_reports_the_call_path_that_reached_it() {
    // The question a bare message cannot answer: *which* path got here.
    let diags = runtime_failure(
        "fn inner(n: u64) -> u64 { panic(\"deep\") }
        fn middle(n: u64) -> u64 { inner(n) }
        fn main() -> u64 { middle(3u64) }",
    );
    assert!(diags.contains("backtrace"), "{diags}");
    let inner_at = diags
        .find("inner")
        .unwrap_or_else(|| panic!("no `inner` frame:\n{diags}"));
    let middle_at = diags
        .find("middle")
        .unwrap_or_else(|| panic!("no `middle` frame:\n{diags}"));
    assert!(
        inner_at < middle_at,
        "backtrace should be innermost first:\n{diags}"
    );
}

#[test]
fn a_failed_assert_reports_like_a_panic() {
    let diags = runtime_failure(
        "fn check(n: u64) -> u64 {
            assert(n > 10u64, \"n too small\")
            n
        }
        fn main() -> u64 { check(3u64) }",
    );
    assert!(diags.contains("n too small"), "{diags}");
    assert!(diags.contains("test.t:2:"), "{diags}");
    assert!(diags.contains("check"), "{diags}");
}

#[test]
fn a_passing_assert_costs_nothing_observable() {
    common::assert_program_result_u64(
        "fn check(n: u64) -> u64 {
            assert(n > 1u64, \"n too small\")
            n
        }
        fn main() -> u64 { check(42u64) }",
        42,
    );
}

// --- contracts --------------------------------------------------------

#[test]
fn a_requires_violation_reports_the_argument_values() {
    // `clause #1 evaluated to false` says which predicate failed; the
    // values say what to fix.
    let diags = runtime_failure(
        "fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
        {
            a / b
        }
        fn main() -> i64 { divide(20i64, 0i64) }",
    );
    assert!(diags.contains("requires"), "{diags}");
    assert!(diags.contains("a = 20"), "argument values missing:\n{diags}");
    assert!(diags.contains("b = 0"), "argument values missing:\n{diags}");
}

#[test]
fn an_ensures_violation_reports_the_result_too() {
    let diags = runtime_failure(
        "fn buggy_abs(x: i64) -> i64
            ensures result >= 0i64
        {
            -x
        }
        fn main() -> i64 { buggy_abs(5i64) }",
    );
    assert!(diags.contains("ensures"), "{diags}");
    assert!(diags.contains("x = 5"), "{diags}");
    assert!(diags.contains("result = -5"), "the returned value is the point:\n{diags}");
}

#[test]
fn a_satisfied_contract_stays_quiet() {
    common::assert_program_result_i64(
        "fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
            ensures result * b == a
        {
            a / b
        }
        fn main() -> i64 { divide(20i64, 4i64) }",
        5,
    );
}

// --- arithmetic guards (P6-3) ----------------------------------------

#[test]
fn u64_subtraction_underflow_traps_instead_of_wrapping() {
    // `0u64 - 1u64` wrapping to 18446744073709551615 is a favourite way
    // to lose an afternoon: the value looks like a plausible large
    // number, so the symptom shows up far from the cause.
    let diags = runtime_failure(
        "fn main() -> u64 {
            val a: u64 = 0u64
            val b: u64 = 1u64
            a - b
        }",
    );
    assert!(diags.contains("underflow"), "{diags}");
    // Values and position come from the panic path (P6-1 / P6-2).
    assert!(diags.contains("0 - 1"), "operands should be named:\n{diags}");
    assert!(diags.contains("test.t:4:"), "{diags}");
}

#[test]
fn a_subtraction_that_fits_is_unaffected() {
    common::assert_program_result_u64(
        "fn main() -> u64 {
            val a: u64 = 5u64
            val b: u64 = 3u64
            a - b
        }",
        2,
    );
}

#[test]
fn signed_subtraction_going_negative_is_not_an_error() {
    // Only *unsigned* subtraction is guarded: `i64` has somewhere to go.
    common::assert_program_result_i64(
        "fn main() -> i64 {
            val a: i64 = 3i64
            val b: i64 = 10i64
            a - b
        }",
        -7,
    );
}

#[test]
fn u64_addition_still_wraps() {
    // Deliberately unguarded so far — this pins the current boundary of
    // P6-3 rather than endorsing it. `u64::MAX + 5` wraps to 4.
    common::assert_program_result_u64(
        "fn main() -> u64 {
            val a: u64 = 18446744073709551615u64
            a + 5u64
        }",
        4,
    );
}

// --- DEBUG-OBS D1: the backtrace's holes ---------------------------
//
// Every test below failed before D1: the frame was missing, the line
// was missing, or the same frame was printed seven times. The gaps
// were invisible because the assertions above only ask that *some*
// frames appear in the right order.

#[test]
fn a_method_frame_is_named_by_its_receiver_type() {
    // 実測 3: the innermost frame — the one that actually panicked —
    // was absent entirely, because frames were pushed on the
    // `Expr::Call` branch alone.
    let diags = runtime_failure(
        "struct S { v: i64 }
        impl S { fn boom(&self) -> i64 { panic(\"method boom\") } }
        fn go(s: S) -> i64 { s.boom() }
        fn main() -> u64 {
            val s = S { v: 1i64 }
            val r: i64 = go(s)
            0u64
        }",
    );
    assert!(
        diags.contains("S::boom"),
        "the panicking method should be the innermost frame, qualified by its type:\n{diags}"
    );
    let boom_at = diags.find("S::boom").unwrap();
    let go_at = diags.find("\n       go").unwrap_or_else(|| panic!("no `go` frame:\n{diags}"));
    assert!(boom_at < go_at, "innermost first:\n{diags}");
}

#[test]
fn the_entry_function_is_a_frame() {
    let diags = runtime_failure(
        "fn boom() -> u64 { panic(\"x\") }
        fn main() -> u64 { boom() }",
    );
    let boom_at = diags.find("\n       boom").unwrap_or_else(|| panic!("no `boom`:\n{diags}"));
    let main_at = diags
        .find("\n       main")
        .unwrap_or_else(|| panic!("`main` should close the backtrace:\n{diags}"));
    assert!(boom_at < main_at, "`main` is outermost:\n{diags}");
}

#[test]
fn every_frame_says_where_it_was_called_from() {
    // 実測 4: `(called at line N)` was unreachable code — the builder
    // pushed `None` for the argument list the call site is read from.
    let diags = runtime_failure(
        "fn inner(n: u64) -> u64 { panic(\"deep\") }
        fn middle(n: u64) -> u64 { inner(n) }
        fn main() -> u64 { middle(3u64) }",
    );
    assert!(
        diags.contains("inner (called at line 2)"),
        "`inner` is called on line 2:\n{diags}"
    );
    assert!(
        diags.contains("middle (called at line 3)"),
        "`middle` is called on line 3:\n{diags}"
    );
}

#[test]
fn a_closure_call_gets_a_frame() {
    let diags = runtime_failure(
        "fn main() -> u64 {
            val f = fn(n: u64) -> u64 { panic(\"in closure\") }
            f(1u64)
        }",
    );
    assert!(diags.contains("in closure"), "{diags}");
    assert!(
        diags.contains("\n       f (called at line 3)"),
        "the closure's binding names its frame:\n{diags}"
    );
}

#[test]
fn an_associated_function_frame_is_qualified() {
    let diags = runtime_failure(
        "struct S { v: i64 }
        impl S { fn make(n: i64) -> S { panic(\"no S for you\") } }
        fn main() -> u64 {
            val s: S = S::make(1i64)
            0u64
        }",
    );
    assert!(
        diags.contains("S::make"),
        "an associated function is named `Type::fn`:\n{diags}"
    );
}

#[test]
fn repeated_frames_are_folded_with_a_count() {
    // 実測 6: seven identical lines, none of which said anything the
    // first did not.
    let diags = runtime_failure(
        "fn f(n: u64) -> u64 {
            if n == 0u64 { panic(\"bottom\") }
            f(n - 1u64)
        }
        fn main() -> u64 { f(7u64) }",
    );
    assert!(
        diags.contains("f (x7, called at line 3)"),
        "the recursive run should fold to one line with its count:\n{diags}"
    );
    assert_eq!(
        diags.matches("\n       f ").count(),
        2,
        "one folded line for the recursion, one for the call from `main`:\n{diags}"
    );
}

#[test]
fn a_deep_backtrace_says_how_much_it_left_out() {
    // Mutual recursion: no two adjacent frames are the same call, so
    // folding cannot shorten it and the depth cap is what keeps the
    // message from scrolling away.
    let diags = runtime_failure(
        "fn f(n: u64) -> u64 {
            if n == 0u64 { panic(\"bottom\") }
            g(n)
        }
        fn g(n: u64) -> u64 { f(n - 1u64) }
        fn main() -> u64 { f(13u64) }",
    );
    assert!(
        diags.contains("frames elided"),
        "a stack past the cap says so rather than being silently cut:\n{diags}"
    );
    assert!(
        diags.contains("\n       main"),
        "the outermost frames survive the elision:\n{diags}"
    );
}

// --- DEBUG-OBS D5: what a program can ask, and what a tool can read --

#[test]
fn a_function_can_name_itself() {
    // Parser-level, so it costs nothing at run time — and it agrees
    // with the name a backtrace frame uses, which is the point of
    // having it at all.
    let source = "struct S { v: i64 }
        impl S { fn who(&self) -> str { __builtin_function_name() } }
        fn free_fn() -> str { __builtin_function_name() }
        fn main() -> u64 {
            println(free_fn())
            val s = S { v: 1i64 }
            println(s.who())
            println(__builtin_function_name())
            0u64
        }";
    let out = stdout_of(source);
    assert_eq!(out, "free_fn\nS::who\nmain\n", "{out}");
}

#[test]
fn a_program_can_ask_how_it_got_here() {
    let source = "fn inner() -> str { __builtin_backtrace() }
        fn outer() -> str { inner() }
        fn main() -> u64 {
            println(outer())
            0u64
        }";
    let out = stdout_of(source);
    assert!(out.contains("inner (called at line 2)"), "{out}");
    assert!(out.contains("outer (called at line 4)"), "{out}");
    assert!(out.trim_end().ends_with("main"), "the entry frame closes it:\n{out}");
}

#[test]
fn a_contract_violation_says_which_call_broke_it() {
    // The question a contract report leaves open: not "what was
    // false", which it always said, but "who passed that argument".
    let diags = runtime_failure(
        "fn half(n: u64) -> u64
            requires n % 2u64 == 0u64
        {
            n / 2u64
        }
        fn caller() -> u64 { half(3u64) }
        fn main() -> u64 { caller() }",
    );
    assert!(diags.contains("with n = 3"), "{diags}");
    assert!(
        diags.contains("test.t:2:"),
        "the failing clause has a position:\n{diags}"
    );
    assert!(
        diags.contains("half (called at line 6)") && diags.contains("caller (called at line 7)"),
        "a contract violation carries a backtrace like any other failure:\n{diags}"
    );
}

// --- DEBUG-OBS D6: runaway recursion and stdlib bounds --------------

#[test]
fn a_runaway_recursion_says_so_instead_of_hanging() {
    // 実測 6: this used to run for 60 seconds and produce nothing —
    // the IR VM's frame vector grew until the process was killed.
    let diags = runtime_failure(
        "fn f(n: u64) -> u64 { f(n + 1u64) }
        fn main() -> u64 { f(0u64) }",
    );
    assert!(diags.contains("recursion limit exceeded"), "{diags}");
    // The folding earns its keep here: a thousand identical frames
    // would bury the message that explains them.
    assert!(
        diags.contains("f (x"),
        "the repeated frame should fold to one line:\n{diags}"
    );
    assert!(diags.lines().count() < 12, "one screen, not a thousand:\n{diags}");
}

#[test]
fn a_vec_read_past_the_end_is_a_toylang_failure() {
    // 実測 7: `value not defined`, a Rust panic from inside the IR VM,
    // with no toylang position or backtrace left.
    let diags = runtime_failure(
        "fn main() -> u64 {
            var v: Vec<i64> = Vec::new()
            v.push(1i64)
            val x: i64 = v.get(5u64)
            0u64
        }",
    );
    assert!(diags.contains("Vec::get index out of bounds"), "{diags}");
    assert!(diags.contains("core/std/collections/vec.t:"), "{diags}");
    assert!(diags.contains("Vec::get (called at line 4)"), "{diags}");
}

#[test]
fn popping_an_empty_vec_says_what_happened() {
    let diags = runtime_failure(
        "fn main() -> u64 {
            var v: Vec<i64> = Vec::new()
            val x: i64 = v.pop()
            0u64
        }",
    );
    assert!(diags.contains("Vec::pop on an empty Vec"), "{diags}");
}

#[test]
fn a_string_read_past_the_end_is_a_toylang_failure() {
    let diags = runtime_failure(
        "fn main() -> u64 {
            val s: String = String::from_str(\"hi\")
            val c: u8 = s.get(99u64)
            0u64
        }",
    );
    assert!(diags.contains("String::get index out of bounds"), "{diags}");
}

#[test]
fn a_failing_program_runs_exactly_once() {
    // 実測 2: a diverging IR VM run used to be answered by replaying
    // the whole program on the tree-walker, which is a *second run*.
    // `io::random` advancing twice is the cheapest way to see it: the
    // seed is fixed, so a single run must print the first value of the
    // sequence.
    let source = "fn main() -> u64 {
            io::random_seed(7u64)
            val a: u64 = io::random()
            println(a)
            panic(\"stop\")
        }";
    let (result, out) = interpreter::output::with_capture(|| test_program(source));
    assert!(result.is_err(), "the program is supposed to fail");
    assert_eq!(out.lines().count(), 1, "printed once, not twice:\n{out}");

    // The same seed, run to completion, must give the same first value.
    let reference = "fn main() -> u64 {
            io::random_seed(7u64)
            val a: u64 = io::random()
            println(a)
            0u64
        }";
    let expected = stdout_of(reference);
    assert_eq!(out, expected, "the failing run consumed extra randomness");
}

#[test]
fn a_dyn_call_does_not_show_its_dispatch_thunk() {
    // The vtable thunk is a real function in the IR, and it would
    // otherwise appear between the method and its caller — plumbing
    // the reader did not write.
    let diags = runtime_failure(
        "trait Speak { fn speak(&self) -> u64 }
        struct Dog { v: u64 }
        impl Speak for Dog { fn speak(&self) -> u64 { panic(\"woof\") } }
        fn go(s: &dyn Speak) -> u64 { s.speak() }
        fn main() -> u64 { val d = Dog { v: 1u64 } go(&d) }",
    );
    assert!(diags.contains("Dog::speak"), "{diags}");
    assert!(!diags.contains("thunk"), "dispatch plumbing leaked:\n{diags}");
}
