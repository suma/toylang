//! RUNTIME-LIB P0-A: writing files, and the error stream.
//!
//! `write_file` / `append_file` join `read_file` on the RUNTIME-IO
//! `Result` convention (a payload-carrying extern plus a paired
//! status extern), so the backends have to agree on the byte count,
//! on the reason a failed write gives, and on the file that is left
//! behind.
//!
//! `eprint` / `eprintln` are compared differently from everything
//! else in this directory: the question is not what was printed but
//! **which descriptor it came out of**, so the lanes are compared as
//! (stdout, stderr) pairs rather than by an exit code. `io::exit` has
//! no lane at all — it ends the process, which in-process lanes share
//! with the test runner.

use super::harness::{
    assert_consistent, assert_stdout_consistent, compiled_run_streams, interpreter_streams,
    skip_e2e, unique_path,
};

/// A temp path the program can write to, spelled into the source.
fn scratch_path(stem: &str) -> std::path::PathBuf {
    let p = unique_path(&format!("{stem}.txt"));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn a_written_file_reads_back_with_the_bytes_that_were_written() {
    let path = scratch_path("write_file_roundtrip");
    let src = format!(
        r#"
        fn main() -> u64 {{
            val w = io::write_file("{path}", "hello io")
            val n = match w {{
                Result::Ok(n) => n,
                Result::Err(_) => 0u64,
            }}
            val back = io::read_file("{path}")
            val same = match back {{
                Result::Ok(text) => text == "hello io",
                Result::Err(_) => false,
            }}
            if same {{ n }} else {{ 0u64 }}
        }}
        "#,
        path = path.display(),
    );
    // 8 bytes written, and the file holds exactly them.
    assert_consistent(&src, "write_file_roundtrip");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_write_replaces_and_an_append_adds() {
    let path = scratch_path("append_file");
    let src = format!(
        r#"
        fn main() -> u64 {{
            val a = io::write_file("{path}", "one")
            val b = io::write_file("{path}", "two")
            val c = io::append_file("{path}", "three")
            val back = io::read_file("{path}")
            match back {{
                Result::Ok(text) => if text == "twothree" {{ 1u64 }} else {{ 2u64 }},
                Result::Err(_) => 3u64,
            }}
        }}
        "#,
        path = path.display(),
    );
    assert_consistent(&src, "append_file");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_write_into_a_missing_directory_reports_the_same_reason_everywhere() {
    // The reason travels as an `IoError` variant and is rendered by
    // `Display`, so this pins both the classification (a missing
    // directory is `NotFound`, not a write error) and the wording.
    let src = r#"
        fn main() -> u64 {
            val w = io::write_file("/no/such/directory/toylang.txt", "x")
            match w {
                Result::Ok(_) => { println("wrote") }
                Result::Err(e) => { println(e) }
            }
            0u64
        }
    "#;
    assert_stdout_consistent(src, "write_file_missing_dir");
}

#[test]
fn eprint_writes_to_stderr_and_print_to_stdout() {
    if skip_e2e() {
        return;
    }
    // Interleaved deliberately: a backend that buffered one stream
    // and flushed it at exit would still pass a "did the bytes
    // appear" test, and fail this one on ordering within a stream.
    let src = r#"
        struct P { x: i64, y: i64 }

        fn main() -> u64 {
            println("out 1")
            eprintln("err 1")
            eprint("err ")
            eprintln(2u64)
            val p = P { x: 1i64, y: 2i64 }
            eprintln(p)
            println("out 2")
            0u64
        }
    "#;
    let (_code, stdout, stderr) =
        compiled_run_streams(src, "eprint_streams").expect("compile and run");
    assert_eq!(stdout, "out 1\nout 2\n", "stderr text leaked into stdout");
    assert_eq!(
        stderr, "err 1\nerr 2\nP { x: 1, y: 2 }\n",
        "the error stream is not what the program wrote"
    );

    // The interpreting engines render the same two streams. Captured
    // in-process, which is why this lane is not `compiled_run_streams`.
    let (interp_out, interp_err) = interpreter_streams(src);
    assert_eq!(interp_out, stdout, "tree-walker vs AOT stdout");
    assert_eq!(interp_err, stderr, "tree-walker vs AOT stderr");
}
