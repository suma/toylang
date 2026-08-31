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
# Failure convention (RUNTIME-IO): `read_file` / `env_var` /
# `read_line` return a `Result<_, IoError>` whose `Err` names the
# failure as an `IoError` variant (exhaustively matchable; rendered
# through `Display` as `not found` / `permission denied` / ...). The
# payload-carrying extern records a failure status in the runtime;
# the paired `__extern_io_*_status` extern hands it back, so the two
# calls together are atomic from toylang's point of view — the
# boundary itself still carries only scalars. The status codes are
# produced identically by the interpreter registry (`extern_io.rs`)
# and `toylang_rt`; `file_exists` remains an independent query, not a
# failure probe.
#
# `random()` is deliberately non-deterministic (seeded from the clock
# and process id) — programs that print it cannot be compared across
# runs or backends.

extern fn getchar() -> i32 from "c"
extern fn time(t: ptr) -> i64 from "c"
extern fn __extern_io_exit(code: i32) from "c" as "exit"

extern fn __extern_io_argc_u64() -> u64 from "toylang_rt" as "toy_io_argc"
extern fn __extern_io_arg_str(i: u64) -> str from "toylang_rt" as "toy_io_arg"
extern fn __extern_io_env_str(name: str) -> str from "toylang_rt" as "toy_io_env"
extern fn __extern_io_env_status() -> u64 from "toylang_rt" as "toy_io_env_status"
extern fn __extern_io_read_file_str(path: str) -> str from "toylang_rt" as "toy_io_read_file"
extern fn __extern_io_read_file_status() -> u64 from "toylang_rt" as "toy_io_read_file_status"
extern fn __extern_io_read_file_into(path: str, buf: ptr, cap: u64) -> u64 from "toylang_rt" as "toy_io_read_file_into"
extern fn __extern_io_write_file_bytes(path: str, buf: ptr, len: u64, append: bool) -> u64 from "toylang_rt" as "toy_io_write_file_bytes"
extern fn __extern_io_write_file_u64(path: str, contents: str, append: bool) -> u64 from "toylang_rt" as "toy_io_write_file"
extern fn __extern_io_write_file_status() -> u64 from "toylang_rt" as "toy_io_write_file_status"
extern fn __extern_io_file_exists_bool(path: str) -> bool from "toylang_rt" as "toy_io_file_exists"
extern fn __extern_io_random_u64() -> u64 from "toylang_rt" as "toy_io_random"
extern fn __extern_io_random_seed(seed: u64) from "toylang_rt" as "toy_io_random_seed"
extern fn __extern_io_strftime_str(fmt: str, secs: u64) -> str from "toylang_rt" as "toy_io_strftime"
extern fn __extern_io_env_count_u64() -> u64 from "toylang_rt" as "toy_io_env_count"
extern fn __extern_io_env_name_str(i: u64) -> str from "toylang_rt" as "toy_io_env_name"
extern fn __extern_io_env_value_str(i: u64) -> str from "toylang_rt" as "toy_io_env_value"

# The reason an I/O operation failed. `?` propagates it unchanged, and
# a `match` over it is exhaustive — handle every variant or fall back
# to `_`. Rendering goes through `Display`: `println(err)` prints the
# reason text (`not found`, `permission denied`, ...).
pub enum IoError {
    NotFound,          # the path / variable does not exist
    PermissionDenied,  # the OS denied the access
    IsADirectory,      # the path names a directory
    ReadError,         # any other read failure
    WriteError,        # any other write failure
    EndOfInput,        # `read_line`: EOF before any byte was read
    Unknown,           # a failure with no errno behind it
}

impl Display for IoError {
    fn to_str(&self) -> str {
        match self {
            IoError::NotFound => "not found",
            IoError::PermissionDenied => "permission denied",
            IoError::IsADirectory => "is a directory",
            IoError::ReadError => "read error",
            IoError::WriteError => "write error",
            IoError::EndOfInput => "end of input",
            IoError::Unknown => "unknown error",
        }
    }
}

# Map a failure status code (recorded by the runtime alongside the
# payload-carrying call, see the extern declarations above) to its
# `IoError` variant. The codes are produced identically by the
# interpreter registry (`extern_io.rs`) and `toylang_rt`, so the same
# failure reads the same on every backend.
fn io_error_from_status(status: u64) -> IoError {
    if status == 1u64 { IoError::NotFound }
    elif status == 2u64 { IoError::PermissionDenied }
    elif status == 3u64 { IoError::IsADirectory }
    elif status == 4u64 { IoError::ReadError }
    elif status == 5u64 { IoError::WriteError }
    else { IoError::Unknown }
}

# Read one line from stdin, without the trailing newline (`\n`, or
# `\r\n`). `Err(IoError::EndOfInput)` at EOF — raised only before any
# byte was read, so a final line without a newline is still an `Ok`.
# An empty line is `Ok("")`.
pub unsafe fn read_line() -> Result<str, IoError> {
    val first: i32 = getchar()
    if first == -1i32 {
        return Result::Err(IoError::EndOfInput)
    }
    var buf: Vec<u8> = Vec::new()
    var c: i32 = first
    loop {
        if c == '\n' { break }
        buf.push(c as u8)
        c = getchar()
        if c == -1i32 { break }      # EOF mid-line: the line is what we have
    }
    if buf.size() > 0u64 {
        val last: u8 = buf.get(buf.size() - 1u64)
        if last == '\r' { buf.pop() }   # strip the CR of a CRLF
    }
    Result::Ok(__builtin_str_from_bytes(buf.as_ptr(), buf.size()))
}

# Read the file at `path` into `buf`, returning how many bytes landed
# in it (EXTERN-BUF).
#
# The bytes go **straight into the caller's buffer** — nothing is
# allocated here and nothing is copied through an intermediate, on any
# backend. That is the difference from `read_file`, which hands back a
# fresh `str`:
#
#     var v: Vec<u8> = Vec::with_capacity(4096u64)
#     match v.capacity_span() {
#         Option::Some(room) => {
#             val n = io::read_file_into(path, room)?
#             v.set_size(n)
#         }
#         Option::None => { }
#     }
#
# It is also the binary-safe way to read a file: a `str` cannot hold
# arbitrary bytes on the tree-walker, so `read_file` reports a
# non-UTF-8 file as a read error there while the compiled lanes accept
# it. Bytes have no such split.
#
# A file longer than the span fills it and stops — the count is what
# fit, not a failure. Compare it with `buf.len()` to notice.
pub fn read_file_into(path: str, buf: Span<u8>) -> Result<u64, IoError> {
    val n: u64 = __extern_io_read_file_into(path, buf.as_raw(), buf.len())
    val status: u64 = __extern_io_read_file_status()
    if status == 0u64 {
        Result::Ok(n)
    } else {
        # Bound first: an enum-producing call cannot be a payload
        # expression in the compiled lanes (`env_var` above does the
        # same).
        val err: IoError = io_error_from_status(status)
        Result::Err(err)
    }
}

# Write `buf`'s bytes to `path`, replacing what was there. Reads
# straight out of the caller's buffer (EXTERN-BUF), and carries
# arbitrary bytes — embedded NULs included — unlike the `str`-taking
# `write_file`.
pub fn write_file_bytes(path: str, buf: Span<u8>) -> Result<u64, IoError> {
    # Bound rather than returned directly, like `write_file` above: an
    # enum-producing call is not lowerable in tail position.
    val r: Result<u64, IoError> = write_bytes_at(path, buf, false)
    r
}

# `write_file_bytes` that adds to the end of the file instead.
pub fn append_file_bytes(path: str, buf: Span<u8>) -> Result<u64, IoError> {
    val r: Result<u64, IoError> = write_bytes_at(path, buf, true)
    r
}

fn write_bytes_at(path: str, buf: Span<u8>, append: bool) -> Result<u64, IoError> {
    val n: u64 = __extern_io_write_file_bytes(path, buf.as_raw(), buf.len(), append)
    val status: u64 = __extern_io_write_file_status()
    if status == 0u64 {
        Result::Ok(n)
    } else {
        val err: IoError = io_error_from_status(status)
        Result::Err(err)
    }
}

# Number of program arguments (excluding the program name).
pub fn argc() -> u64 {
    __extern_io_argc_u64()
}

# The `i`-th program argument; `""` out of range.
pub fn arg(i: u64) -> str {
    __extern_io_arg_str(i)
}

# The value of the environment variable `name`; `Err(IoError::NotFound)`
# when unset. An empty value is a valid `Ok("")`.
pub fn env_var(name: str) -> Result<str, IoError> {
    val value: str = __extern_io_env_str(name)
    val status: u64 = __extern_io_env_status()
    if status == 0u64 {
        Result::Ok(value)
    } else {
        val err: IoError = io_error_from_status(status)
        Result::Err(err)
    }
}

# The contents of the file at `path`; `Err(err)` when it cannot be
# read (variants from `io_error_from_status`).
pub fn read_file(path: str) -> Result<str, IoError> {
    val contents: str = __extern_io_read_file_str(path)
    val status: u64 = __extern_io_read_file_status()
    if status == 0u64 {
        Result::Ok(contents)
    } else {
        val err: IoError = io_error_from_status(status)
        Result::Err(err)
    }
}

# Write `contents` to the file at `path`, replacing what was there
# (the file is created when it does not exist). `Ok(n)` is the number
# of bytes written — always `contents`' length on success — and
# `Err(err)` names why the write failed.
pub fn write_file(path: str, contents: str) -> Result<u64, IoError> {
    val r: Result<u64, IoError> = write_file_with_mode(path, contents, false)
    r
}

# Add `contents` to the end of the file at `path`, keeping what was
# there (the file is created when it does not exist). Same result
# convention as `write_file`.
pub fn append_file(path: str, contents: str) -> Result<u64, IoError> {
    val r: Result<u64, IoError> = write_file_with_mode(path, contents, true)
    r
}

# The shared body: the extern carries the mode as a flag so the two
# entry points are one symbol, and the byte count and the status come
# back as the usual RUNTIME-IO pair (a zero-byte write is legitimate,
# so the count alone cannot report a failure).
fn write_file_with_mode(path: str, contents: str, append: bool) -> Result<u64, IoError> {
    val written: u64 = __extern_io_write_file_u64(path, contents, append)
    val status: u64 = __extern_io_write_file_status()
    if status == 0u64 {
        Result::Ok(written)
    } else {
        val err: IoError = io_error_from_status(status)
        Result::Err(err)
    }
}

# End the process now with `code` as its exit status, without
# returning to the caller. This is the normal-path counterpart to
# `panic`: nothing is printed, no `Drop` runs, and buffered output is
# flushed first. The low 8 bits are what a shell sees (`exit(256u64)`
# reports 0), the same truncation `main`'s return value goes through.
pub fn exit(code: u64) {
    __extern_io_exit(code as i32)
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

# Re-seed the `random()` generator. After `random_seed(s)` the
# sequence is reproducible (identical across runs and backends for
# the same `s`), which is how programs print deterministic results
# or tests assert on them. `random_seed(0u64)` is honoured literally
# (the sequence stays at 0).
pub fn random_seed(seed: u64) {
    __extern_io_random_seed(seed)
}

# Format Unix epoch seconds as a UTC date/time string using a
# documented subset of C `strftime` specifiers:
#
#   %%   literal `%`
#   %a %A  abbreviated / full weekday name (Sun..Sat / Sunday..)
#   %b %B  abbreviated / full month name (Jan..Dec / January..)
#   %C   century (year / 100, zero-padded)
#   %d   day of month 01-31
#   %D   %m/%d/%y
#   %e   day of month 1-31 (space-padded)
#   %F   %Y-%m-%d
#   %H   hour 00-23
#   %I   hour 01-12
#   %j   day of year 001-366
#   %m   month 01-12
#   %M   minute 00-59
#   %n   newline
#   %p   AM / PM
#   %R   %H:%M
#   %S   second 00-59
#   %s   epoch seconds
#   %t   tab
#   %T   %H:%M:%S
#   %u   weekday 1-7 (Monday=1)
#   %w   weekday 0-6 (Sunday=0)
#   %y   year without century 00-99
#   %Y   full year, zero-padded to 4 digits
#   %z   UTC offset (`+0000`)
#   %Z   timezone name (`UTC`)
#
# The conversion is **UTC**, never local time, so a fixed timestamp
# formats identically regardless of the host timezone. Unknown
# specifiers pass through literally (`%q` → `%q`), matching libc.
pub fn strftime(fmt: str, secs: u64) -> str {
    __extern_io_strftime_str(fmt, secs)
}

# Number of environment variables.
pub fn env_count() -> u64 {
    __extern_io_env_count_u64()
}

# The name (before `=`) of the `i`-th environment variable; `""` out
# of range. The order is the process's `environ` order.
pub fn env_name(i: u64) -> str {
    __extern_io_env_name_str(i)
}

# The value (after `=`) of the `i`-th environment variable; `""` out
# of range.
pub fn env_value(i: u64) -> str {
    __extern_io_env_value_str(i)
}
