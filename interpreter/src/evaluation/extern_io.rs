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
//! `u64 len` field in the compiled backends). Failure convention:
//! read / env lookups return `""` for "not found / unreadable" — an
//! `extern fn` boundary cannot carry a `Result` (compound returns do
//! not cross it), so callers probe with `file_exists` / `env` instead.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::InterpreterError;
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

pub fn set_program_args(args: Vec<String>) {
    IO_ARGS.with(|a| *a.borrow_mut() = args);
}

/// Native implementation backing an `extern fn` declaration. Same
/// shape as `extern_math::ExternFn`.
pub type ExternFn = fn(&[Value]) -> Result<Value, InterpreterError>;

/// Build the registry of I/O extern fn implementations available at
/// interpreter startup. Keyed by the `extern fn` declaration's name
/// as written in the source program (see `core/std/io.t`).
pub fn build_io_registry() -> HashMap<&'static str, ExternFn> {
    let mut m: HashMap<&'static str, ExternFn> = HashMap::new();
    m.insert("__extern_io_argc_u64", io_argc);
    m.insert("__extern_io_arg_str", io_arg);
    m.insert("__extern_io_env_str", io_env);
    m.insert("__extern_io_read_file_str", io_read_file);
    m.insert("__extern_io_file_exists_bool", io_file_exists);
    m.insert("__extern_io_random_u64", io_random);
    m.insert("__extern_io_random_seed", io_random_seed);
    m.insert("__extern_io_strftime_str", io_strftime);
    m.insert("__extern_io_env_count_u64", io_env_count);
    m.insert("__extern_io_env_name_str", io_env_name);
    m.insert("__extern_io_env_value_str", io_env_value);
    // RUNTIME-PORT R2: the libc names `core/std/io.t` now declares
    // `from "c"`. `read_line` / `now` are implemented in toylang on
    // top of these.
    m.insert("getchar", io_getchar);
    m.insert("time", io_time);
    m.insert("getpid", io_getpid);
    m
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

fn str_result(text: String) -> Value {
    Object::String(text).into()
}

fn u64_result(v: u64) -> Value {
    Object::UInt64(v).into()
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

/// The value of the environment variable `name`; `""` when unset.
fn io_env(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_env_str` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let name = str_arg(&args[0], "__extern_io_env_str")?;
    Ok(str_result(std::env::var(&name).unwrap_or_default()))
}

/// The contents of the file at `path`; `""` when it cannot be read.
fn io_read_file(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_io_read_file_str` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    let path = str_arg(&args[0], "__extern_io_read_file_str")?;
    Ok(str_result(std::fs::read_to_string(&path).unwrap_or_default()))
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
