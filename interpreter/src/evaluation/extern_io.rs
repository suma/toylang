//! Runtime implementations of the stdlib I/O `extern fn` declarations
//! (`core/std/io.t`).
//!
//! Same architecture as `extern_math.rs`: the interpreter dispatches
//! `extern fn` calls through a registry of Rust closures keyed by the
//! declared name. Each backend resolves the same names differently —
//! the AOT re-declares them as `Linkage::Import` calls against the
//! `toy_io_*` symbols in the `toylang_rt` crate (via
//! `compiler_lower::program::libm_import_name_for`), and the
//! compiler-side JIT registers the same crate's symbols in
//! `compiler/src/jit.rs::register_runtime_symbols`.
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
    m.insert("__extern_io_read_line_str", io_read_line);
    m.insert("__extern_io_argc_u64", io_argc);
    m.insert("__extern_io_arg_str", io_arg);
    m.insert("__extern_io_env_str", io_env);
    m.insert("__extern_io_read_file_str", io_read_file);
    m.insert("__extern_io_file_exists_bool", io_file_exists);
    m.insert("__extern_io_now_u64", io_now);
    m.insert("__extern_io_random_u64", io_random);
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

/// Read one line from stdin, without the trailing newline (`\n`, or
/// `\r\n`). `""` at EOF.
fn io_read_line(_args: &[Value]) -> Result<Value, InterpreterError> {
    let mut line = String::new();
    use std::io::Read;
    let mut buf = [0u8; 1];
    let mut stdin = std::io::stdin().lock();
    loop {
        match stdin.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {
                if buf[0] == b'\n' {
                    break;
                }
                line.push(buf[0] as char);
            }
            Err(_) => break,
        }
    }
    if line.ends_with('\r') {
        line.pop();
    }
    Ok(str_result(line))
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

/// Seconds since the Unix epoch.
fn io_now(_args: &[Value]) -> Result<Value, InterpreterError> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(u64_result(secs))
}

/// A pseudo-random `u64`. xorshift64* seeded from the clock and the
/// process id — deliberately not reproducible, so programs that print
/// it cannot be compared across runs.
fn io_random(_args: &[Value]) -> Result<Value, InterpreterError> {
    thread_local! {
        static STATE: RefCell<u64> = const { RefCell::new(0) };
    }
    let seed = STATE.with(|s| {
        let mut st = *s.borrow();
        if st == 0 {
            st = (SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0))
                ^ ((std::process::id() as u64) << 32);
            if st == 0 {
                st = 0x9E3779B97F4A7C15;
            }
        }
        *s.borrow_mut() = st;
        st
    });
    let mut x = seed;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    Ok(u64_result(x.wrapping_mul(0x2545F4914F6CDD1D)))
}
