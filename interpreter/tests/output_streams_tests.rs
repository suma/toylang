// RUNTIME-LIB P0-A: `eprint` / `eprintln` — the same rendering as
// `print` / `println`, on the error stream.
//
// The interesting property is not the text but the descriptor, so
// every test here captures both streams and asserts on the pair. The
// cross-backend half (does the AOT binary put the same bytes on the
// same descriptor) lives in `compiler/tests/consistency/runtime_io.rs`;
// this file covers the two interpreting engines, which is where the
// `Display` dispatch and the compound rendering happen.

use crate::common::{core_modules_dir, test_program};

/// Run a program and return `(stdout, stderr)`.
fn streams_of(source: &str) -> (String, String) {
    let (result, out, err) =
        interpreter::output::with_stdout_stderr_capture(|| test_program(source));
    result.unwrap_or_else(|e| panic!("expected the program to run:\n{e}"));
    (out, err)
}

/// The same through the default engine (the IR VM), which renders
/// print instructions itself rather than walking the tree.
fn streams_of_ir_vm(source: &str) -> (String, String) {
    let core = core_modules_dir();
    let mut options = interpreter::RunOptions::default();
    options.core_modules_dir = Some(&core);
    let (result, out, err) = interpreter::output::with_stdout_stderr_capture(|| {
        interpreter::run_source(source, "streams_test.t", &options)
    });
    result.unwrap_or_else(|e| panic!("expected the program to run:\n{e}"));
    (out, err)
}

#[test]
fn eprintln_writes_to_stderr_and_leaves_stdout_alone() {
    let (out, err) = streams_of(
        r#"fn main() -> u64 {
            println("out")
            eprintln("err")
            0u64
        }"#,
    );
    assert_eq!(out, "out\n");
    assert_eq!(err, "err\n");
}

#[test]
fn eprint_does_not_append_a_newline() {
    let (out, err) = streams_of(
        r#"fn main() -> u64 {
            eprint("a")
            eprint("b")
            eprintln("c")
            0u64
        }"#,
    );
    assert_eq!(out, "");
    assert_eq!(err, "abc\n");
}

#[test]
fn the_error_stream_renders_any_type_the_way_print_does() {
    // Same formatting path: a struct comes out as
    // `Name { field: value }` with fields in alphabetical order.
    let (out, err) = streams_of(
        r#"struct P { y: i64, x: i64 }

        fn main() -> u64 {
            eprintln(42u64)
            eprintln(true)
            eprintln(P { y: 2i64, x: 1i64 })
            0u64
        }"#,
    );
    assert_eq!(out, "");
    assert_eq!(err, "42\ntrue\nP { x: 1, y: 2 }\n");
}

#[test]
fn a_display_impl_decides_how_the_error_stream_renders_it() {
    // `eprintln(v)` is rewritten to `eprintln(v.to_str())` by the same
    // dispatch `println` uses — on the method's presence, not on a
    // recorded `impl Display for`.
    let (out, err) = streams_of(
        r#"struct Point { x: i64, y: i64 }

        impl Display for Point {
            fn to_str(&self) -> str {
                "({self.x}, {self.y})"
            }
        }

        fn main() -> u64 {
            eprintln(Point { x: 3i64, y: 4i64 })
            0u64
        }"#,
    );
    assert_eq!(out, "");
    assert_eq!(err, "(3, 4)\n");
}

#[test]
fn the_ir_vm_splits_the_streams_the_same_way() {
    let (out, err) = streams_of_ir_vm(
        r#"struct P { x: i64, y: i64 }

        fn main() -> u64 {
            println("out 1")
            eprintln("err 1")
            eprintln(P { x: 1i64, y: 2i64 })
            println("out 2")
            0u64
        }"#,
    );
    assert_eq!(out, "out 1\nout 2\n");
    assert_eq!(err, "err 1\nP { x: 1, y: 2 }\n");
}

#[test]
fn eprint_is_not_a_reserved_word() {
    // The builtins are resolved by symbol like `print` / `println`, so
    // a program that binds the name keeps it.
    let (out, err) = streams_of(
        r#"fn main() -> u64 {
            val eprint: u64 = 3u64
            println(eprint)
            0u64
        }"#,
    );
    assert_eq!(out, "3\n");
    assert_eq!(err, "");
}
