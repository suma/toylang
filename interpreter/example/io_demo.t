# Stdlib I/O (RUNTIME-IO): the `io::` module backed by `extern fn`
# declarations in `core/std/io.t`. Same pattern as `core/std/math.t` —
# each backend resolves the externs differently (interpreter registry,
# AOT C runtime symbols, JIT mirrors).
#
#   - `io::read_line()`   — one line from stdin, no trailing newline
#   - `io::argc()`        — number of program arguments
#   - `io::arg(i)`        — the i-th program argument ("" out of range)
#   - `io::env_var(name)` — the environment variable ("" when unset)
#   - `io::read_file(p)`  — file contents ("" when unreadable)
#   - `io::file_exists(p)`— probe before reading
#   - `io::now()`         — seconds since the Unix epoch
#   - `io::random()`      — pseudo-random u64 (not reproducible)
#
# Run: printf 'hello\n' | cargo run -q -p interpreter -- example/io_demo.t alpha beta
# Expected exit code: 42
#
# `read_file` returns "" for a missing file, so probe with
# `file_exists` when "" is a valid payload.

fn main() -> u64 {
    val line = io::read_line()
    val count = io::argc()
    val first = io::arg(0u64)
    val home = io::env_var("HOME")
    val data = io::read_file("interpreter/example/io_demo.t")
    val present = io::file_exists("interpreter/example/io_demo.t")
    if line == "hello"
        && count == 2u64
        && first == "alpha"
        && home != ""
        && data != ""
        && present
        && io::now() > 1700000000u64 {
        42u64
    } else {
        0u64
    }
}
