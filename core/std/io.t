# Stdlib I/O (RUNTIME-IO). Auto-loaded from `<core>/std/io.t` so
# programs can call `io::read_line()` / `io::argc()` etc. without an
# `import` line, or `import std.io` for the bare names.
#
# Architecture: every function delegates to an `extern fn` whose name
# each backend resolves differently — the same pattern as
# `core/std/math.t`:
#
# - interpreter: dispatched by the registry in
#   `interpreter::evaluation::extern_io::build_io_registry`.
# - AOT compiler: re-declared as a `Linkage::Import` call against the
#   `toy_io_*` symbols in `compiler/runtime/toylang_rt.c` via
#   `compiler_lower::program::libm_import_name_for`.
# - JIT (cranelift): the Rust mirrors registered in
#   `compiler/src/jit.rs::register_runtime_symbols`.
#
# Failure convention: `read_file` / `env_var` return `""` when the
# file or variable does not exist. An `extern fn` boundary cannot
# carry a `Result` (compound returns do not cross it), so probe with
# `file_exists` when the empty string is a valid payload.
#
# `random()` is deliberately non-deterministic (seeded from the clock
# and process id) — programs that print it cannot be compared across
# runs or backends.

extern fn __extern_io_read_line_str() -> str
extern fn __extern_io_argc_u64() -> u64
extern fn __extern_io_arg_str(i: u64) -> str
extern fn __extern_io_env_str(name: str) -> str
extern fn __extern_io_read_file_str(path: str) -> str
extern fn __extern_io_file_exists_bool(path: str) -> bool
extern fn __extern_io_now_u64() -> u64
extern fn __extern_io_random_u64() -> u64

# Read one line from stdin, without the trailing newline. `""` at EOF.
pub fn read_line() -> str {
    __extern_io_read_line_str()
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
    __extern_io_now_u64()
}

# A pseudo-random `u64`. Not reproducible.
pub fn random() -> u64 {
    __extern_io_random_u64()
}
