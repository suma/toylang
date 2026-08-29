# Stdlib I/O (RUNTIME-IO): the `io::` module backed by `extern fn`
# declarations in `core/std/io.t`. Same pattern as `core/std/math.t` —
# each backend resolves the externs differently (interpreter registry,
# AOT C runtime symbols, JIT mirrors).
#
#   - `io::read_line()`   — one line from stdin, no trailing newline;
#                           Err(IoError::EndOfInput) at EOF
#   - `io::argc()`        — number of program arguments
#   - `io::arg(i)`        — the i-th program argument ("" out of range)
#   - `io::env_var(name)` — the environment variable;
#                           Err(IoError::NotFound) when unset
#   - `io::read_file(p)`  — file contents; Err(IoError) when unreadable
#   - `io::file_exists(p)`— probe before reading
#   - `io::now()`         — seconds since the Unix epoch
#   - `io::random()`      — pseudo-random u64 (not reproducible)
#   - `io::random_seed(s)`— make `random()` reproducible
#   - `io::strftime(f,s)` — format epoch seconds (UTC, strftime subset)
#   - `io::env_count()`   — number of environment variables
#   - `io::env_name(i)`   — the i-th environment variable's name
#   - `io::env_value(i)`  — the i-th environment variable's value
#
# Run: printf 'hello\n' | cargo run -q -p interpreter -- example/io_demo.t alpha beta
# Expected exit code: 42
#
# `read_file` / `env_var` / `read_line` return `Result<_, IoError>`,
# whose Err variant is exhaustively matchable (and renders through
# `Display`: `println(err)` prints `not found` etc.).

fn main() -> u64 {
    val line = io::read_line()
    val line_ok = match line {
        Result::Ok(text) => text == "hello",
        Result::Err(_) => false,
    }
    val count = io::argc()
    val first = io::arg(0u64)
    val home = io::env_var("HOME")
    val data = io::read_file("interpreter/example/io_demo.t")
    val data_ok = match data {
        Result::Ok(text) => text != "",
        Result::Err(_) => false,
    }
    # A missing file names its reason as an IoError variant, on every
    # backend. `println(err)` would render it through `Display`.
    val missing = io::read_file("interpreter/example/no_such_file.t")
    val missing_ok = match missing {
        Result::Err(IoError::NotFound) => true,
        _ => false,
    }
    val present = io::file_exists("interpreter/example/io_demo.t")
    # Seeded `random` is reproducible across runs and backends.
    io::random_seed(99u64)
    val r1 = io::random()
    val r2 = io::random()
    # UTC formatting of a fixed timestamp is deterministic.
    val ts = io::strftime("%Y-%m-%d %H:%M", 1700000000u64)
    # The environment is enumerable, and HOME is among the names.
    var has_home = false
    var e: u64 = 0u64
    while e < io::env_count() {
        if io::env_name(e) == "HOME" { has_home = true }
        e = e + 1u64
    }
    if line_ok
        && count == 2u64
        && first == "alpha"
        && home.is_ok()
        && data_ok
        && missing_ok
        && present
        && io::now() > 1700000000u64
        && r1 != 0u64 && r2 != 0u64
        && ts == "2023-11-14 22:13"
        && has_home
        && io::env_count() > 0u64 {
        42u64
    } else {
        0u64
    }
}
