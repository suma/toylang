//! Runtime implementations of the stdlib I/O `extern fn` declarations
//! (`core/std/io.t`).
//!
//! Same architecture as `extern_math.rs`: the interpreter dispatches
//! `extern fn` calls through a registry of Rust closures keyed by the
//! declared name. Since RUNTIME-PORT R2 the stdlib declarations carry
//! their own symbols (`from "toylang_rt" as "toy_io_*"` for the
//! pointer-boundary helpers, `from "c"` for `getchar` / `time` /
//! `getpid`), and the interpreter deliberately does **not** dlopen
//! libc — these std-based implementations serve the libc names
//! instead, keeping stdin/env handling under the interpreter's
//! control. The AOT linker and the compiler JIT resolve the real
//! symbols. User `from`-declared externs that the registry does not
//! serve go through the general FFI path (`extern_ffi`).
//!
//! Return-value convention: `str` results use the language's str
//! representation (a `String` object here; a pointer to the trailing
//! `u64 len` field in the compiled backends). RUNTIME-IO: the
//! payload-carrying calls whose empty string was ambiguous
//! (`read_file` / `env_var`) record a failure status in per-operation
//! thread-local slots, which the paired `__extern_io_*_status`
//! registry entries hand back to the stdlib wrapper in `core/std/io.t`
//! right after the payload call — from toylang's point of view the
//! pair is atomic, and the wrapper turns it into a `Result<str, str>`.
//! The status codes are the same numbers `toylang_rt` produces.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::InterpreterError;
use crate::evaluation::EvaluationContext;
use crate::object::Object;
use crate::value::Value;

// Program arguments visible to `argc()` / `arg(i)`. Set once per run
// from `RunOptions::args`; the compiled backends read their own
// process argv instead.
thread_local! {
    static IO_ARGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

// The `random()` generator's xorshift64* state. `None` = never seeded
// (derive from the clock + pid on first use); `Some(s)` = explicit
// seed from `io_random_seed`, honoured literally (including `0`).
thread_local! {
    static RANDOM_STATE: RefCell<Option<u64>> = const { RefCell::new(None) };
}

// RUNTIME-IO: failure status of the most recent payload-carrying call,
// one slot per operation kind. `0` = success; the failure vocabulary
// matches `toylang_rt`'s `IO_*` constants and is mapped to reason
// strings in `core/std/io.t`.
thread_local! {
    static PARSE_F64_STATUS: Cell<u64> = const { Cell::new(0) };
    static READ_FILE_STATUS: Cell<u64> = const { Cell::new(0) };
    static ENV_STATUS: Cell<u64> = const { Cell::new(0) };
    static WRITE_FILE_STATUS: Cell<u64> = const { Cell::new(0) };
    // STDLIB-FS-PATH: the same pairing, plus the last listing.
    static FS_STATUS: Cell<u64> = const { Cell::new(0) };
    static FS_ENTRIES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

// ERROR_MODEL D3 / E0: the failure vocabulary is defined once, in
// `toylang_rt`, and this registry forwards to it — the shape
// `extern_net` already uses. The former private copy of the errno
// table here is what let the same write failure read as `write error`
// on the interpreter and `read error` on AOT/JIT.
use toylang_rt::io_status::{
    NOT_FOUND as IO_NOT_FOUND, OK as IO_OK, READ_ERROR as IO_READ_ERROR,
    WRITE_ERROR as IO_WRITE_ERROR,
};
use toylang_rt::parse_status::{
    INVALID as PARSE_INVALID, OK as PARSE_OK, OVERFLOW as PARSE_OVERFLOW,
};

/// Map an `std::io::Error` to the RUNTIME-IO status vocabulary by
/// handing its errno to `toylang_rt`.
///
/// `fallback` names the failure when there is no errno behind it —
/// the caller knows which direction it was going, and the runtime's
/// table does not. Errors without a raw errno (e.g. non-UTF-8
/// contents, `InvalidData`) are this engine's own: the compiled
/// backends do not validate UTF-8, a documented divergence for
/// invalid files.
fn status_from_io_error(err: &std::io::Error, fallback: u64) -> u64 {
    match err.raw_os_error() {
        Some(code) => toylang_rt::io_status::from_errno(code),
        None => fallback,
    }
}

pub fn set_program_args(args: Vec<String>) {
    IO_ARGS.with(|a| *a.borrow_mut() = args);
}

/// Native implementation backing an `extern fn` declaration. Same
/// shape as `extern_math::ExternFn`.
pub type ExternFn = fn(&[Value]) -> Result<Value, InterpreterError>;

/// EXTERN-BUF: an `extern fn` that reaches toylang memory.
///
/// The extra `&mut EvaluationContext` is what makes a `(ptr, len)`
/// buffer usable here. In the compiled lanes such an extern needs
/// nothing special — the pointer is a real address and the callee
/// writes through it — but this engine's `ptr` is an index into
/// `HeapManager`, so resolving it takes the context.
pub type ExternBufFn =
    fn(&mut EvaluationContext<'_>, &[Value]) -> Result<Value, InterpreterError>;

/// Borrow `len` bytes of toylang memory at the address `p` names, for
/// reading, and run `f` on them (EXTERN-BUF).
///
/// The closure form is the safety condition, not a convenience: the
/// heap's byte vector grows on allocation, so a borrow kept across
/// one dangles. Taking a closure makes "the borrow does not outlive
/// the call" a fact about the type rather than a rule to remember.
pub(crate) fn with_bytes<R>(
    ctx: &EvaluationContext<'_>,
    p: &Value,
    len: u64,
    name: &str,
    f: impl FnOnce(&[u8]) -> R,
) -> Result<R, InterpreterError> {
    let addr = ptr_arg(p, name)?;
    // Mutable even though this is the read direction: the borrow
    // flushes the typed-slot map into the raw bytes first, because a
    // buffer built with `push` lives there and an address-taking
    // callee cannot consult it (see `HeapManager::borrow_bytes`).
    let mut heap = ctx.heap_manager.borrow_mut();
    let bytes = heap.borrow_bytes(addr, len as usize).ok_or_else(|| {
        InterpreterError::InternalError(format!(
            "extern fn `{name}`: buffer of {len} bytes is not inside a live allocation"
        ))
    })?;
    Ok(f(bytes))
}

/// [`with_bytes`] for writing. Any typed slot covering the range is
/// dropped, so the bytes the callee leaves behind are what a later
/// read sees (`HeapManager::borrow_bytes_mut`).
pub(crate) fn with_bytes_mut<R>(
    ctx: &mut EvaluationContext<'_>,
    p: &Value,
    len: u64,
    name: &str,
    f: impl FnOnce(&mut [u8]) -> R,
) -> Result<R, InterpreterError> {
    let addr = ptr_arg(p, name)?;
    let mut heap = ctx.heap_manager.borrow_mut();
    let bytes = heap.borrow_bytes_mut(addr, len as usize).ok_or_else(|| {
        InterpreterError::InternalError(format!(
            "extern fn `{name}`: buffer of {len} bytes is not inside a live allocation"
        ))
    })?;
    Ok(f(bytes))
}

/// Extract the heap address a `ptr` argument carries.
fn ptr_arg(value: &Value, name: &str) -> Result<usize, InterpreterError> {
    match value {
        Value::Pointer(addr) => Ok(*addr),
        Value::Heap(rc) => match &*rc.borrow() {
            Object::Pointer(addr) => Ok(*addr),
            other => Err(InterpreterError::InternalError(format!(
                "extern fn `{name}`: expected a ptr argument, got {other:?}"
            ))),
        },
        other => Err(InterpreterError::InternalError(format!(
            "extern fn `{name}`: expected a ptr argument, got {other:?}"
        ))),
    }
}

/// Build the registry of I/O extern fn implementations available at
/// interpreter startup. Keyed by the `extern fn` declaration's name
/// as written in the source program (see `core/std/io.t`).
pub fn build_io_registry() -> HashMap<&'static str, ExternFn> {
    let mut m: HashMap<&'static str, ExternFn> = HashMap::new();
    // NETWORK_IO N0: the compile-time platform switch, made visible.
    // Served from `toylang_rt` rather than reimplemented, so this
    // engine cannot disagree with the compiled lanes about which
    // backend was selected — the answer *is* the one the runtime was
    // built with.
    m.insert("__extern_net_backend_name", net_backend_name);
    m.insert("__extern_io_argc_u64", io_argc);
    m.insert("__extern_io_arg_str", io_arg);
    m.insert("__extern_io_env_str", io_env);
    m.insert("__extern_io_env_status", io_env_status);
    m.insert("__extern_io_read_file_str", io_read_file);
    m.insert("__extern_io_read_file_status", io_read_file_status);
    m.insert("__extern_io_write_file_u64", io_write_file);
    m.insert("__extern_io_write_file_status", io_write_file_status);
    m.insert("__extern_io_file_exists_bool", io_file_exists);
    // COLLECTIONS C0: `Hash for str`. Mirrors
    // `toylang_rt::toy_str_hash` constant for constant — the three
    // backends have to agree on the value.
    m.insert("__extern_str_hash", str_hash);
    // STDLIB-TEXT §5: `Ord for str`. Forwards to `toylang_rt` rather
    // than repeating the comparison -- a second implementation of an
    // ordering is a second thing that can disagree, and `Vec<str>`
    // sorted differently per backend would be exactly that.
    m.insert("__extern_str_cmp", str_cmp);
    // STDLIB-TEXT §3: `str`'s search primitive.
    m.insert("__extern_str_find", str_find);
    // STDLIB-NUMERIC N1: the five bit primitives. Forwarded to
    // `toylang_rt` so the answer for an input of 0 -- the one case
    // the hardware instruction leaves undefined -- cannot differ
    // between lanes.
    m.insert("__extern_bits_popcount", bits_popcount);
    m.insert("__extern_bits_clz", bits_clz);
    m.insert("__extern_bits_ctz", bits_ctz);
    m.insert("__extern_bits_reverse", bits_reverse);
    m.insert("__extern_bits_swap_bytes", bits_swap_bytes);
    // STDLIB-TIME TM0/TM1/TM3. Forwarded to `toylang_rt` -- the
    // calendar especially, which `strftime` already uses: a second
    // implementation of it is the thing §4 exists to prevent.
    // STDLIB-LOG: the level is one value in the runtime, so it has to
    // be read across the boundary rather than held here -- a second
    // copy in the interpreter would answer differently from the same
    // program built AOT.
    m.insert("__extern_log_level", log_level);
    m.insert("__extern_log_set_level", log_set_level);
    m.insert("__extern_log_timestamps", log_timestamps);
    // STDLIB-FS-PATH: forwarded to `toylang_rt`, the shape
    // `extern_net` uses -- a second implementation of the errno
    // table is what ERROR_MODEL's E0 had to undo.
    m.insert("__extern_fs_dir_open", fs_dir_open);
    m.insert("__extern_fs_dir_name", fs_dir_name);
    m.insert("__extern_fs_status", fs_status);
    m.insert("__extern_fs_is_dir", fs_is_dir);
    m.insert("__extern_fs_file_size", fs_file_size);
    m.insert("__extern_fs_mkdir", fs_mkdir);
    m.insert("__extern_fs_remove_file", fs_remove_file);
    m.insert("__extern_fs_remove_dir", fs_remove_dir);
    m.insert("__extern_fs_rename", fs_rename);
    m.insert("__extern_fs_realpath", fs_realpath);
    m.insert("__extern_fs_current_dir", fs_current_dir);
    m.insert("__extern_time_now_mono_ns", time_now_mono_ns);
    m.insert("__extern_time_mono_res_ns", time_mono_res_ns);
    m.insert("__extern_time_cpu_ns", time_cpu_ns);
    m.insert("__extern_time_now_unix_ns", time_now_unix_ns);
    m.insert("__extern_time_sleep_ns", time_sleep_ns);
    m.insert("__extern_time_civil_from_days", time_civil_from_days);
    m.insert("__extern_time_days_from_civil", time_days_from_civil);
    m.insert("__extern_io_random_u64", io_random);
    m.insert("__extern_io_random_seed", io_random_seed);
    m.insert("__extern_io_strftime_str", io_strftime);
    m.insert("__extern_io_env_count_u64", io_env_count);
    m.insert("__extern_io_env_name_str", io_env_name);
    m.insert("__extern_io_env_value_str", io_env_value);
    // RUNTIME-PORT R2: the libc names `core/std/io.t` now declares
    // `from "c"`. `read_line` / `now` are implemented in toylang on
    // top of these.
    m.insert("__extern_io_exit", io_exit);
    // RUNTIME-LIB P0-B: the one parser that needs the host's
    // decimal -> binary conversion. The integer and bool parsers are
    // written in toylang and reach no registry.
    m.insert("__extern_parse_f64", parse_f64);
    m.insert("__extern_parse_f64_status", parse_f64_status);
    m.insert("getchar", io_getchar);
    m.insert("time", io_time);
    m.insert("getpid", io_getpid);
    m
}

/// `net::backend_name()` — which event-notification backend the
/// runtime was compiled with.
fn net_backend_name(args: &[Value]) -> Result<Value, InterpreterError> {
    if !args.is_empty() {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_net_backend_name` takes no arguments".to_string(),
            expected: 0,
            found: args.len(),
        });
    }
    Ok(str_result(toylang_rt::net_backend_name().to_string()))
}

/// EXTERN-BUF: the buffer-taking I/O externs (`core/std/io.t`).
///
/// Kept separate from `build_io_registry` because the signature
/// differs, not because the implementations are unrelated — these are
/// the same file operations as `read_file` / `write_file`, addressed
/// through a caller-owned buffer instead of through a `str`. That is
/// what makes them binary-safe: a `str` here is a Rust `String` and
/// cannot hold arbitrary bytes, which is why `read_file` reports a
/// non-UTF-8 file as a read error on this engine alone.
pub fn build_io_buf_registry() -> HashMap<&'static str, ExternBufFn> {
    let mut m: HashMap<&'static str, ExternBufFn> = HashMap::new();
    m.insert("__extern_io_read_file_into", io_read_file_into);
    m.insert("__extern_io_write_file_bytes", io_write_file_bytes);
    m
}

/// `io::read_file_into` — fill the caller's buffer from a file,
/// returning how many bytes landed in it.
///
/// The read goes **straight into toylang memory**: `with_bytes_mut`
/// lends the buffer and `Read::read` fills it, so there is no staging
/// copy on this lane either (the compiled lanes hand `fread` the same
/// address). A file longer than the buffer fills it and stops — the
/// count is what fits, the way `read(2)` behaves, not an error.
fn io_read_file_into(
    ctx: &mut EvaluationContext<'_>,
    args: &[Value],
) -> Result<Value, InterpreterError> {
    if args.len() != 3 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_read_file_into` takes 3 arguments".to_string(),
            expected: 3,
            found: args.len(),
        });
    }
    let path = str_arg(&args[0], "__extern_io_read_file_into")?;
    let cap = u64_arg(&args[2], "__extern_io_read_file_into")?;
    let mut file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            READ_FILE_STATUS.with(|s| s.set(status_from_io_error(&e, IO_READ_ERROR)));
            return Ok(u64_result(0));
        }
    };
    let outcome = with_bytes_mut(ctx, &args[1], cap, "__extern_io_read_file_into", |buf| {
        use std::io::Read;
        let mut filled = 0usize;
        loop {
            if filled == buf.len() {
                break Ok(filled);
            }
            match file.read(&mut buf[filled..]) {
                Ok(0) => break Ok(filled),
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => break Err(e),
            }
        }
    })?;
    match outcome {
        Ok(filled) => {
            READ_FILE_STATUS.with(|s| s.set(IO_OK));
            Ok(u64_result(filled as u64))
        }
        Err(e) => {
            READ_FILE_STATUS.with(|s| s.set(status_from_io_error(&e, IO_READ_ERROR)));
            Ok(u64_result(0))
        }
    }
}

/// `io::write_file_bytes` — write the caller's buffer to a file,
/// truncating or appending. Reads straight out of toylang memory.
fn io_write_file_bytes(
    ctx: &mut EvaluationContext<'_>,
    args: &[Value],
) -> Result<Value, InterpreterError> {
    if args.len() != 4 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_write_file_bytes` takes 4 arguments".to_string(),
            expected: 4,
            found: args.len(),
        });
    }
    let path = str_arg(&args[0], "__extern_io_write_file_bytes")?;
    let len = u64_arg(&args[2], "__extern_io_write_file_bytes")?;
    let append = bool_arg(&args[3], "__extern_io_write_file_bytes")?;
    let opened = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .open(&path);
    let mut file = match opened {
        Ok(f) => f,
        Err(e) => {
            WRITE_FILE_STATUS.with(|s| s.set(status_from_io_error(&e, IO_WRITE_ERROR)));
            return Ok(u64_result(0));
        }
    };
    let outcome = with_bytes(ctx, &args[1], len, "__extern_io_write_file_bytes", |bytes| {
        use std::io::Write;
        file.write_all(bytes).map(|()| bytes.len())
    })?;
    match outcome {
        Ok(written) => {
            WRITE_FILE_STATUS.with(|s| s.set(IO_OK));
            Ok(u64_result(written as u64))
        }
        Err(e) => {
            WRITE_FILE_STATUS.with(|s| s.set(status_from_io_error(&e, IO_WRITE_ERROR)));
            Ok(u64_result(0))
        }
    }
}

/// Extract a `u64` argument.
fn u64_arg(value: &Value, name: &str) -> Result<u64, InterpreterError> {
    match value {
        Value::UInt64(v) => Ok(*v),
        Value::Heap(rc) => match &*rc.borrow() {
            Object::UInt64(v) => Ok(*v),
            other => Err(InterpreterError::InternalError(format!(
                "extern fn `{name}`: expected a u64 argument, got {other:?}"
            ))),
        },
        other => Err(InterpreterError::InternalError(format!(
            "extern fn `{name}`: expected a u64 argument, got {other:?}"
        ))),
    }
}

/// Extract a `u32` argument. A narrow width crosses the boundary as
/// its own `Value` variant rather than widening, so `u64_arg` does
/// not cover it.
fn u32_arg(value: &Value, name: &str) -> Result<u32, InterpreterError> {
    match value {
        Value::UInt32(v) => Ok(*v),
        Value::UInt64(v) => Ok(*v as u32),
        Value::Heap(rc) => match &*rc.borrow() {
            Object::UInt32(v) => Ok(*v),
            Object::UInt64(v) => Ok(*v as u32),
            other => Err(InterpreterError::InternalError(format!(
                "extern fn `{name}`: expected a u32 argument, got {other:?}"
            ))),
        },
        other => Err(InterpreterError::InternalError(format!(
            "extern fn `{name}`: expected a u32 argument, got {other:?}"
        ))),
    }
}

/// Extract a `str` argument from the value it crosses the extern
/// boundary as. `dispatch_extern_fn` materialises literals as heap
/// `String`s before invoking the registry, so only that shape (plus
/// the interned fallback for robustness) needs handling here.
fn str_arg(value: &Value, name: &str) -> Result<String, InterpreterError> {
    match value {
        Value::Heap(rc) => match &*rc.borrow() {
            Object::String(s) => Ok(s.clone()),
            other => Err(InterpreterError::InternalError(format!(
                "extern fn `{name}`: expected a str argument, got {other:?}"
            ))),
        },
        other => Err(InterpreterError::InternalError(format!(
            "extern fn `{name}`: expected a str argument, got {other:?}"
        ))),
    }
}

/// Extract a `bool` argument. Booleans cross the extern boundary as
/// the inline `Value::Bool`; the heap shape is accepted for the same
/// robustness reason `str_arg` accepts its fallback.
fn bool_arg(value: &Value, name: &str) -> Result<bool, InterpreterError> {
    match value {
        Value::Bool(b) => Ok(*b),
        Value::Heap(rc) => match &*rc.borrow() {
            Object::Bool(b) => Ok(*b),
            other => Err(InterpreterError::InternalError(format!(
                "extern fn `{name}`: expected a bool argument, got {other:?}"
            ))),
        },
        other => Err(InterpreterError::InternalError(format!(
            "extern fn `{name}`: expected a bool argument, got {other:?}"
        ))),
    }
}

fn str_result(text: String) -> Value {
    Object::String(text).into()
}

fn u64_result(v: u64) -> Value {
    Object::UInt64(v).into()
}

/// `parse::to_f64` — decimal string to `f64`, after `core/std/parse.t`
/// has checked the grammar. Rust's `str::parse` and the compiled
/// backends' `strtod` are both correctly rounded, and the validator
/// keeps the two from ever seeing an input where their *grammars*
/// differ (`inf`, hex floats, leading whitespace).
fn parse_f64(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_parse_f64` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let text = str_arg(&args[0], "__extern_parse_f64")?;
    match text.parse::<f64>() {
        Ok(v) if v.is_infinite() => {
            PARSE_F64_STATUS.with(|s| s.set(PARSE_OVERFLOW));
            Ok(Object::Float64(v).into())
        }
        Ok(v) => {
            PARSE_F64_STATUS.with(|s| s.set(PARSE_OK));
            Ok(Object::Float64(v).into())
        }
        Err(_) => {
            PARSE_F64_STATUS.with(|s| s.set(PARSE_INVALID));
            Ok(Object::Float64(0.0).into())
        }
    }
}

/// RUNTIME-LIB P0-B: the status of the most recent `parse_f64`.
fn parse_f64_status(_args: &[Value]) -> Result<Value, InterpreterError> {
    Ok(u64_result(PARSE_F64_STATUS.with(|s| s.get())))
}

/// `exit(code)` — end the process now, as libc's `exit` does for the
/// compiled backends. There is no unwinding to a `RunOutcome`: the
/// call does not return in any backend, so an embedder (the test
/// suite included) that runs a program calling `io::exit` ends with
/// it. Buffered output is flushed first, since the process is not
/// coming back to do it.
fn io_exit(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_exit` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let code = match args[0] {
        Value::Int32(v) => v,
        Value::Int64(v) => v as i32,
        Value::UInt64(v) => v as i32,
        _ => {
            return Err(InterpreterError::InternalError(
                "extern fn `__extern_io_exit`: expected an i32 code".to_string(),
            ));
        }
    };
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code)
}

/// `getchar()` — one byte from stdin, `-1` (EOF) when exhausted.
/// Serves the `from "c"` declaration in `core/std/io.t`; `read_line`
/// (toylang) loops on it.
fn io_getchar(_args: &[Value]) -> Result<Value, InterpreterError> {
    use std::io::Read;
    let mut buf = [0u8; 1];
    let mut stdin = std::io::stdin().lock();
    let c = match stdin.read(&mut buf) {
        Ok(0) => -1, // EOF
        Ok(_) => buf[0] as i32,
        Err(_) => -1,
    };
    Ok(Object::Int32(c).into())
}

/// `time(NULL)` — seconds since the Unix epoch. The argument (the
/// caller passes a null pointer) is ignored.
fn io_time(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `time` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(Object::Int64(secs).into())
}

/// `getpid()` — the process id, for seeding `random` in toylang.
fn io_getpid(_args: &[Value]) -> Result<Value, InterpreterError> {
    Ok(Object::Int32(std::process::id() as i32).into())
}

/// Number of program arguments (excluding the program name).
fn io_argc(_args: &[Value]) -> Result<Value, InterpreterError> {
    IO_ARGS.with(|a| Ok(u64_result(a.borrow().len() as u64)))
}

/// The `i`-th program argument; `""` out of range.
fn io_arg(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_arg_str` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let i = match args[0] {
        Value::UInt64(v) => v as usize,
        _ => {
            return Err(InterpreterError::InternalError(
                "extern fn `__extern_io_arg_str`: expected u64 index".to_string(),
            ));
        }
    };
    let text = IO_ARGS.with(|a| a.borrow().get(i).cloned().unwrap_or_default());
    Ok(str_result(text))
}

/// The value of the environment variable `name`. RUNTIME-IO: records
/// the failure status (unset) for the paired `io_env_status`, which
/// the stdlib wrapper reads to build the `Result`.
fn io_env(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_env_str` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let name = str_arg(&args[0], "__extern_io_env_str")?;
    match std::env::var(&name) {
        Ok(value) => {
            ENV_STATUS.with(|s| s.set(IO_OK));
            Ok(str_result(value))
        }
        Err(std::env::VarError::NotPresent) => {
            ENV_STATUS.with(|s| s.set(IO_NOT_FOUND));
            Ok(str_result(String::new()))
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            ENV_STATUS.with(|s| s.set(IO_READ_ERROR));
            Ok(str_result(String::new()))
        }
    }
}

/// RUNTIME-IO: the status of the most recent `io_env` call on this
/// thread. The stdlib wrapper calls this immediately after the
/// payload call, so the pair is atomic from toylang's point of view.
fn io_env_status(_args: &[Value]) -> Result<Value, InterpreterError> {
    Ok(u64_result(ENV_STATUS.with(|s| s.get())))
}

/// The contents of the file at `path`. RUNTIME-IO: records the failure
/// status for the paired `io_read_file_status`, which the stdlib
/// wrapper reads to build the `Result`. A non-UTF-8 file is a read
/// error (the compiled backends read raw bytes; see the module docs).
fn io_read_file(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_read_file_str` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let path = str_arg(&args[0], "__extern_io_read_file_str")?;
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            READ_FILE_STATUS.with(|s| s.set(IO_OK));
            Ok(str_result(contents))
        }
        Err(err) => {
            READ_FILE_STATUS.with(|s| s.set(status_from_io_error(&err, IO_READ_ERROR)));
            Ok(str_result(String::new()))
        }
    }
}

/// RUNTIME-IO: the status of the most recent `io_read_file` call on
/// this thread. Paired with `io_read_file` like `io_env_status`.
fn io_read_file_status(_args: &[Value]) -> Result<Value, InterpreterError> {
    Ok(u64_result(READ_FILE_STATUS.with(|s| s.get())))
}

/// Write `contents` to the file at `path`, truncating it or appending
/// to its end. Returns the number of bytes written and records the
/// failure status for the paired `io_write_file_status` — the count
/// alone cannot report a failure, since a zero-byte write is
/// legitimate. Mirrors `toylang_rt::toy_io_write_file`.
fn io_write_file(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 3 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_write_file_u64` takes 3 arguments".to_string(),
            expected: 3,
            found: args.len(),
        });
    }
    let path = str_arg(&args[0], "__extern_io_write_file_u64")?;
    let contents = str_arg(&args[1], "__extern_io_write_file_u64")?;
    let append = bool_arg(&args[2], "__extern_io_write_file_u64")?;
    let opened = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .open(&path);
    let result = opened.and_then(|mut f| {
        use std::io::Write;
        f.write_all(contents.as_bytes())?;
        // `write_all` returns before the data is durable; a failure at
        // flush time is still this call's failure.
        f.flush()
    });
    match result {
        Ok(()) => {
            WRITE_FILE_STATUS.with(|s| s.set(IO_OK));
            Ok(u64_result(contents.len() as u64))
        }
        Err(err) => {
            WRITE_FILE_STATUS.with(|s| s.set(status_from_io_error(&err, IO_WRITE_ERROR)));
            Ok(u64_result(0))
        }
    }
}

/// RUNTIME-IO: the status of the most recent `io_write_file` call on
/// this thread. Paired with `io_write_file` like `io_env_status`.
fn io_write_file_status(_args: &[Value]) -> Result<Value, InterpreterError> {
    Ok(u64_result(WRITE_FILE_STATUS.with(|s| s.get())))
}

/// FNV-1a over the UTF-8 bytes of `s` — the `Hash for str` impl in
/// `core/std/hash.t`. Mirrors `toylang_rt::toy_str_hash` step for
/// step (and `impl Hash for String` in `core/std/string.t`), so a key
/// hashes to the same u64 on every backend.
fn fs_dir_open(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_fs_dir_open` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let path = str_arg(&args[0], "__extern_fs_dir_open")?;
    let mut names: Vec<String> = Vec::new();
    let status = match std::fs::read_dir(&path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                // `.` and `..` never appear in `read_dir`; the
                // runtime's `readdir` loop drops them so the two
                // agree.
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
            IO_OK
        }
        Err(e) => status_from_io_error(&e, IO_READ_ERROR),
    };
    FS_ENTRIES.with(|s| *s.borrow_mut() = names.clone());
    FS_STATUS.with(|s| s.set(status));
    Ok(u64_result(names.len() as u64))
}

fn fs_dir_name(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_fs_dir_name` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let i = u64_arg(&args[0], "__extern_fs_dir_name")? as usize;
    let name = FS_ENTRIES.with(|s| s.borrow().get(i).cloned().unwrap_or_default());
    Ok(str_result(name))
}

fn fs_status(_args: &[Value]) -> Result<Value, InterpreterError> {
    Ok(u64_result(FS_STATUS.with(|s| s.get())))
}

fn fs_is_dir(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_fs_is_dir` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let path = str_arg(&args[0], "__extern_fs_is_dir")?;
    Ok(Value::Bool(std::path::Path::new(&path).is_dir()))
}

fn fs_file_size(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_fs_file_size` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let path = str_arg(&args[0], "__extern_fs_file_size")?;
    match std::fs::metadata(&path) {
        Ok(m) => {
            FS_STATUS.with(|s| s.set(IO_OK));
            Ok(u64_result(m.len()))
        }
        Err(e) => {
            FS_STATUS.with(|s| s.set(fs_status_from_io_error(&e)));
            Ok(u64_result(0))
        }
    }
}

/// The io vocabulary plus the three a file system needs, matching
/// `toylang_rt::fs_status_from_errno` value for value.
fn fs_status_from_io_error(e: &std::io::Error) -> u64 {
    match e.raw_os_error() {
        Some(17) => toylang_rt::FS_ALREADY_EXISTS,
        Some(20) => toylang_rt::FS_NOT_A_DIRECTORY,
        Some(66) | Some(39) => toylang_rt::FS_NOT_EMPTY,
        _ => status_from_io_error(e, IO_READ_ERROR),
    }
}

fn fs_one_path(
    args: &[Value],
    who: &'static str,
    f: fn(&str) -> std::io::Result<()>,
) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: format!("extern fn `{who}` takes 1 argument"),
            expected: 1,
            found: args.len(),
        });
    }
    let path = str_arg(&args[0], who)?;
    Ok(u64_result(match f(&path) {
        Ok(()) => IO_OK,
        Err(e) => fs_status_from_io_error(&e),
    }))
}

fn fs_mkdir(args: &[Value]) -> Result<Value, InterpreterError> {
    fs_one_path(args, "__extern_fs_mkdir", |p| std::fs::create_dir(p))
}

fn fs_remove_file(args: &[Value]) -> Result<Value, InterpreterError> {
    fs_one_path(args, "__extern_fs_remove_file", |p| std::fs::remove_file(p))
}

fn fs_remove_dir(args: &[Value]) -> Result<Value, InterpreterError> {
    fs_one_path(args, "__extern_fs_remove_dir", |p| std::fs::remove_dir(p))
}

fn fs_rename(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 2 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_fs_rename` takes 2 arguments".to_string(),
            expected: 2,
            found: args.len(),
        });
    }
    let from = str_arg(&args[0], "__extern_fs_rename")?;
    let to = str_arg(&args[1], "__extern_fs_rename")?;
    Ok(u64_result(match std::fs::rename(&from, &to) {
        Ok(()) => IO_OK,
        Err(e) => fs_status_from_io_error(&e),
    }))
}

fn fs_realpath(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_fs_realpath` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let path = str_arg(&args[0], "__extern_fs_realpath")?;
    match std::fs::canonicalize(&path) {
        Ok(p) => {
            FS_STATUS.with(|s| s.set(IO_OK));
            Ok(str_result(p.to_string_lossy().into_owned()))
        }
        Err(e) => {
            FS_STATUS.with(|s| s.set(fs_status_from_io_error(&e)));
            Ok(str_result(String::new()))
        }
    }
}

fn fs_current_dir(_args: &[Value]) -> Result<Value, InterpreterError> {
    match std::env::current_dir() {
        Ok(p) => {
            FS_STATUS.with(|s| s.set(IO_OK));
            Ok(str_result(p.to_string_lossy().into_owned()))
        }
        Err(e) => {
            FS_STATUS.with(|s| s.set(fs_status_from_io_error(&e)));
            Ok(str_result(String::new()))
        }
    }
}

/// STDLIB-TIME: the no-argument clocks.
fn time_no_args_u64(
    args: &[Value],
    who: &'static str,
    f: extern "C" fn() -> u64,
) -> Result<Value, InterpreterError> {
    if !args.is_empty() {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: format!("extern fn `{who}` takes no arguments"),
            expected: 0,
            found: args.len(),
        });
    }
    Ok(u64_result(f()))
}

fn time_now_mono_ns(args: &[Value]) -> Result<Value, InterpreterError> {
    time_no_args_u64(args, "__extern_time_now_mono_ns", toylang_rt::toy_time_now_mono_ns)
}

fn time_mono_res_ns(args: &[Value]) -> Result<Value, InterpreterError> {
    time_no_args_u64(args, "__extern_time_mono_res_ns", toylang_rt::toy_time_mono_res_ns)
}

fn time_cpu_ns(args: &[Value]) -> Result<Value, InterpreterError> {
    time_no_args_u64(args, "__extern_time_cpu_ns", toylang_rt::toy_time_cpu_ns)
}

fn time_now_unix_ns(args: &[Value]) -> Result<Value, InterpreterError> {
    if !args.is_empty() {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_time_now_unix_ns` takes no arguments".to_string(),
            expected: 0,
            found: args.len(),
        });
    }
    Ok(Value::Int64(toylang_rt::toy_time_now_unix_ns()))
}

fn time_sleep_ns(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_time_sleep_ns` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    toylang_rt::toy_time_sleep_ns(u64_arg(&args[0], "__extern_time_sleep_ns")?);
    Ok(Value::Unit)
}

/// STDLIB-TIME TM3: the calendar, forwarded so there is one of it.
fn time_i64_of(
    args: &[Value],
    who: &'static str,
    f: extern "C" fn(i64) -> i64,
) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: format!("extern fn `{who}` takes 1 argument"),
            expected: 1,
            found: args.len(),
        });
    }
    let v = match &args[0] {
        Value::Int64(v) => *v,
        Value::UInt64(v) => *v as i64,
        other => {
            return Err(InterpreterError::InternalError(format!(
                "extern fn `{who}` expects an i64 argument, got {other:?}"
            )))
        }
    };
    Ok(Value::Int64(f(v)))
}

fn time_civil_from_days(args: &[Value]) -> Result<Value, InterpreterError> {
    time_i64_of(args, "__extern_time_civil_from_days", toylang_rt::toy_time_civil_from_days)
}

fn time_days_from_civil(args: &[Value]) -> Result<Value, InterpreterError> {
    time_i64_of(args, "__extern_time_days_from_civil", toylang_rt::toy_time_days_from_civil)
}

/// STDLIB-NUMERIC N1: one `u64` argument, one `u32` answer.
fn bits_u32_of(args: &[Value], who: &'static str, f: extern "C" fn(u64) -> u32) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: format!("extern fn `{who}` takes 1 argument"),
            expected: 1,
            found: args.len(),
        });
    }
    Ok(Value::UInt32(f(u64_arg(&args[0], who)?)))
}

/// As above, answering with a `u64`.
fn bits_u64_of(args: &[Value], who: &'static str, f: extern "C" fn(u64) -> u64) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: format!("extern fn `{who}` takes 1 argument"),
            expected: 1,
            found: args.len(),
        });
    }
    Ok(u64_result(f(u64_arg(&args[0], who)?)))
}

fn bits_popcount(args: &[Value]) -> Result<Value, InterpreterError> {
    bits_u32_of(args, "__extern_bits_popcount", toylang_rt::toy_bits_popcount)
}

fn bits_clz(args: &[Value]) -> Result<Value, InterpreterError> {
    bits_u32_of(args, "__extern_bits_clz", toylang_rt::toy_bits_clz)
}

fn bits_ctz(args: &[Value]) -> Result<Value, InterpreterError> {
    bits_u32_of(args, "__extern_bits_ctz", toylang_rt::toy_bits_ctz)
}

fn bits_reverse(args: &[Value]) -> Result<Value, InterpreterError> {
    bits_u64_of(args, "__extern_bits_reverse", toylang_rt::toy_bits_reverse)
}

fn bits_swap_bytes(args: &[Value]) -> Result<Value, InterpreterError> {
    bits_u64_of(args, "__extern_bits_swap_bytes", toylang_rt::toy_bits_swap_bytes)
}

/// STDLIB-TEXT §3: byte offset of `needle` in `haystack` at or after
/// `from`, or -1. Same rule as `toylang_rt::toy_str_find`: an empty
/// needle matches at `from`, and the offset is in bytes.
fn str_find(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 3 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_str_find` takes 3 arguments".to_string(),
            expected: 3,
            found: args.len(),
        });
    }
    let haystack = str_arg(&args[0], "__extern_str_find")?;
    let needle = str_arg(&args[1], "__extern_str_find")?;
    let from = u64_arg(&args[2], "__extern_str_find")? as usize;
    let bytes = haystack.as_bytes();
    if from > bytes.len() {
        return Ok(Value::Int64(-1));
    }
    let found = if needle.is_empty() {
        Some(from)
    } else {
        bytes[from..]
            .windows(needle.len())
            .position(|w| w == needle.as_bytes())
            .map(|i| i + from)
    };
    Ok(Value::Int64(found.map(|i| i as i64).unwrap_or(-1)))
}

/// STDLIB-TEXT §5: three-way byte comparison, forwarded to
/// `toylang_rt::toy_str_cmp`.
fn str_cmp(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 2 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_str_cmp` takes 2 arguments".to_string(),
            expected: 2,
            found: args.len(),
        });
    }
    let a = str_arg(&args[0], "__extern_str_cmp")?;
    let b = str_arg(&args[1], "__extern_str_cmp")?;
    // The runtime's comparison works on its own str layout, which this
    // engine does not use, so compare the bytes here — with the same
    // rule, spelled once: `memcmp` order, shorter first on a common
    // prefix.
    let ord = a.as_bytes().cmp(b.as_bytes());
    let v = match ord {
        std::cmp::Ordering::Less => -1i64,
        std::cmp::Ordering::Equal => 0i64,
        std::cmp::Ordering::Greater => 1i64,
    };
    Ok(Value::Int64(v))
}

fn str_hash(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_str_hash` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let s = str_arg(&args[0], "__extern_str_hash")?;
    let mut h: u64 = 14695981039346656037;
    for b in s.as_bytes() {
        h = (h ^ (*b as u64)).wrapping_mul(1099511628211);
    }
    Ok(u64_result(h))
}

/// Whether the file at `path` exists.
fn io_file_exists(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_file_exists_bool` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let path = str_arg(&args[0], "__extern_io_file_exists_bool")?;
    Ok(Object::Bool(std::path::Path::new(&path).exists()).into())
}

/// A pseudo-random `u64`. xorshift64* seeded from the clock and the
/// process id on first use — or from an explicit `io_random_seed`,
/// which makes the sequence reproducible (and therefore testable
/// across backends). Deliberately not reproducible when never seeded.
/// Mirrors `toylang_rt::toy_io_random` step-for-step so the sequences
/// agree between the interpreter and the compiled backends.
fn io_random(_args: &[Value]) -> Result<Value, InterpreterError> {
    let out = RANDOM_STATE.with(|s| {
        let mut st = *s.borrow();
        if st.is_none() {
            let mut derived = (SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0))
                ^ ((std::process::id() as u64) << 32);
            if derived == 0 {
                derived = 0x9E3779B97F4A7C15;
            }
            st = Some(derived);
        }
        let mut x = st.expect("seeded above");
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        // Persist the advanced state, mirroring `toylang_rt` — the
        // next call starts from the mixed value, not the seed.
        *s.borrow_mut() = Some(x);
        x.wrapping_mul(0x2545F4914F6CDD1D)
    });
    Ok(u64_result(out))
}

/// Re-seed the `random()` generator. `seed == 0` is honoured literally
/// (the sequence stays at 0) rather than re-derived, so
/// `random_seed(0)` is deterministic too. Mirrors
/// `toylang_rt::toy_io_random_seed`.
fn io_random_seed(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_random_seed` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let seed = match args[0] {
        Value::UInt64(v) => v,
        _ => {
            return Err(InterpreterError::InternalError(
                "extern fn `__extern_io_random_seed`: expected u64 seed".to_string(),
            ));
        }
    };
    RANDOM_STATE.with(|s| *s.borrow_mut() = Some(seed));
    Ok(Value::Unit)
}

/// `strftime(fmt, secs)` — format Unix epoch seconds as a UTC
/// date/time string. Delegates to the same pure `strftime_utc` the
/// compiled backends call (`toylang_rt`), so the interpreter, the JIT
/// and AOT binaries produce byte-identical output. UTC, never local
/// time — a fixed timestamp formats identically on any host.
fn io_strftime(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 2 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_strftime_str` takes 2 arguments".to_string(),
            expected: 2,
            found: args.len(),
        });
    }
    let fmt = str_arg(&args[0], "__extern_io_strftime_str")?;
    let secs = match args[1] {
        Value::UInt64(v) => v,
        _ => {
            return Err(InterpreterError::InternalError(
                "extern fn `__extern_io_strftime_str`: expected u64 seconds".to_string(),
            ));
        }
    };
    Ok(str_result(toylang_rt::strftime_utc(&fmt, secs as i64)))
}

/// Number of environment variables. Mirrors `toylang_rt::toy_io_env_count`.
fn io_env_count(_args: &[Value]) -> Result<Value, InterpreterError> {
    Ok(u64_result(std::env::vars().count() as u64))
}

/// The name (before `=`) of the `i`-th environment variable; `""` out
/// of range. Order is `environ` order — the same order the compiled
/// backends report, so the three agree.
fn io_env_name(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_env_name_str` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let i = match args[0] {
        Value::UInt64(v) => v as usize,
        _ => {
            return Err(InterpreterError::InternalError(
                "extern fn `__extern_io_env_name_str`: expected u64 index".to_string(),
            ));
        }
    };
    let name = std::env::vars().nth(i).map(|(k, _)| k).unwrap_or_default();
    Ok(str_result(name))
}

/// The value (after `=`) of the `i`-th environment variable; `""` out
/// of range.
fn io_env_value(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_env_value_str` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let i = match args[0] {
        Value::UInt64(v) => v as usize,
        _ => {
            return Err(InterpreterError::InternalError(
                "extern fn `__extern_io_env_value_str`: expected u64 index".to_string(),
            ));
        }
    };
    let value = std::env::vars().nth(i).map(|(_, v)| v).unwrap_or_default();
    Ok(str_result(value))
}

/// STDLIB-LOG: the active level, resolved from `TOY_LOG` by the
/// runtime on its first read.
fn log_level(args: &[Value]) -> Result<Value, InterpreterError> {
    if !args.is_empty() {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_log_level` takes no arguments".to_string(),
            expected: 0,
            found: args.len(),
        });
    }
    Ok(Value::UInt32(toylang_rt::toy_log_level()))
}

fn log_set_level(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_log_set_level` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let level = u32_arg(&args[0], "__extern_log_set_level")?;
    toylang_rt::toy_log_set_level(level);
    Ok(Value::Unit)
}

fn log_timestamps(args: &[Value]) -> Result<Value, InterpreterError> {
    if !args.is_empty() {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_log_timestamps` takes no arguments".to_string(),
            expected: 0,
            found: args.len(),
        });
    }
    Ok(Value::Bool(toylang_rt::toy_log_timestamps()))
}
