# RUNTIME-LIB P0-A: writing files, the error stream, and `io::exit`.
#
# `write_file` / `append_file` are the pair of `read_file` and follow
# the same convention: `Ok(n)` carries the number of bytes written,
# `Err(e)` names the reason as an `IoError` a `match` can be
# exhaustive over. `eprint` / `eprintln` render exactly like `print` /
# `println` — `Display` dispatch included — but on stderr, so a
# program's diagnostics do not end up inside its output when a caller
# pipes it. `io::exit(code)` ends the run with a chosen status; this
# example does not call it, since the value `main` returns says the
# same thing here.

# The directory to scribble in. `TMPDIR` is set on macOS and usually
# not on Linux, which is what the `Err` arm is for.
fn temp_dir() -> str {
    val v = io::env_var("TMPDIR")
    match v {
        Result::Ok(dir) => dir,
        Result::Err(_) => "/tmp",
    }
}

fn main() -> u64 {
    val path: str = "{temp_dir()}/toylang_example_io.txt"

    val first = io::write_file(path, "one\n")
    match first {
        Result::Ok(n) => { println("wrote {n} bytes") }
        Result::Err(e) => { eprintln("write failed: {e}") }
    }

    # `append_file` keeps what is already there; a second `write_file`
    # would have replaced it.
    val second = io::append_file(path, "two\n")
    match second {
        Result::Ok(n) => { println("appended {n} bytes") }
        Result::Err(e) => { eprintln("append failed: {e}") }
    }

    val back = io::read_file(path)
    match back {
        Result::Ok(text) => { println("file holds {text.len()} bytes") }
        Result::Err(e) => { eprintln("read failed: {e}") }
    }

    # A failure the program can see: the directory does not exist, so
    # the reason is `NotFound` rather than a write error.
    val bad = io::write_file("/no/such/directory/toylang.txt", "x")
    match bad {
        Result::Ok(_) => { println("unexpectedly wrote") }
        Result::Err(e) => { eprintln("expected failure: {e}") }
    }

    0u64
}
