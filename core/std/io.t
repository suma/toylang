# Stdlib I/O (RUNTIME-IO). Auto-loaded from `<core>/std/io.t` so
# programs can call `io::read_line()` / `io::argc()` etc. without an
# `import` line, or `import std.io` for the bare names.
#
# Architecture (RUNTIME-PORT R2 / FFI_PLAN P1): every function
# delegates to an `extern fn` whose symbol and library come from the
# declaration itself:
#
# - `getchar` / `time` are declared `from "c"` — toylang code runs
#   them against the real libc. The interpreter does not dlopen libc
#   (its `extern_io` registry serves these names with std-based
#   implementations instead — RUNTIME_PORT R2 注意); the AOT linker
#   and the JIT's RTLD_DEFAULT fallback resolve the real symbols.
# - the pointer-boundary helpers (`argv`, `env`, `read_file`,
#   `file_exists`, `random`) are declared `from "toylang_rt"` — the
#   runtime crate's `toy_io_*` symbols marshal str handles to C
#   strings internally. They cannot be written in toylang yet: an
#   `extern fn` boundary cannot dereference C pointers (argv, FILE*)
#   on every backend.
#
# Failure convention: `read_file` / `env_var` return `""` when the
# file or variable does not exist. An `extern fn` boundary cannot
# carry a `Result` (compound returns do not cross it), so probe with
# `file_exists` when the empty string is a valid payload.
#
# `random()` is deliberately non-deterministic (seeded from the clock
# and process id) — programs that print it cannot be compared across
# runs or backends.

extern fn getchar() -> i32 from "c"
extern fn time(t: ptr) -> i64 from "c"

extern fn __extern_io_argc_u64() -> u64 from "toylang_rt" as "toy_io_argc"
extern fn __extern_io_arg_str(i: u64) -> str from "toylang_rt" as "toy_io_arg"
extern fn __extern_io_env_str(name: str) -> str from "toylang_rt" as "toy_io_env"
extern fn __extern_io_read_file_str(path: str) -> str from "toylang_rt" as "toy_io_read_file"
extern fn __extern_io_file_exists_bool(path: str) -> bool from "toylang_rt" as "toy_io_file_exists"
extern fn __extern_io_random_u64() -> u64 from "toylang_rt" as "toy_io_random"

# Read one line from stdin, without the trailing newline (`\n`, or
# `\r\n`). `""` at EOF.
pub fn read_line() -> str {
    var buf: Vec<u8> = Vec::new()
    loop {
        val c: i32 = getchar()
        if c == -1i32 { break }      # EOF
        if c == 10i32 { break }      # '\n'
        buf.push(c as u8)
    }
    if buf.size() > 0u64 {
        val last: u8 = buf.get(buf.size() - 1u64)
        if last == 13u8 { buf.pop() }   # strip '\r'
    }
    __builtin_str_from_bytes(buf.as_ptr(), buf.size())
}

# Number of program arguments (excluding the program name).
pub fn argc() -> u64 {
    __extern_io_argc_u64()
}

# The `i`-th program argument; `""` out of range.
pub fn arg(i: u64) -> str {
    __extern_io_arg_str(i)
}

# The value of the environment variable `name`; `""` when unset.
pub fn env_var(name: str) -> str {
    __extern_io_env_str(name)
}

# The contents of the file at `path`; `""` when it cannot be read.
pub fn read_file(path: str) -> str {
    __extern_io_read_file_str(path)
}

# Whether the file at `path` exists.
pub fn file_exists(path: str) -> bool {
    __extern_io_file_exists_bool(path)
}

# Seconds since the Unix epoch.
pub fn now() -> u64 {
    val t: i64 = time(__builtin_null_ptr())
    t as u64
}

# A pseudo-random `u64`. Not reproducible.
pub fn random() -> u64 {
    __extern_io_random_u64()
}
