//! Runtime implementations of the stdlib networking `extern fn`
//! declarations (`core/std/net.t`).
//!
//! **These forward to `toylang_rt` rather than reimplementing the
//! syscalls on `std::net`** (NETWORK_IO.md §7). The interpreter
//! already links the runtime crate, and the alternative — a second
//! implementation over `std::net` — would mean two mappings from
//! errno to `NetError` kept in step by comment. The existing
//! RUNTIME-IO externs do carry two implementations, with a note
//! promising they "produce the same numbers"; there is no reason to
//! take that on again here, and a socket has more failure modes to
//! disagree about than a file.
//!
//! The buffer-taking pair (`send` / `recv`) lives in the
//! `ExternBufFn` registry instead, because a `ptr` on this engine is
//! an index into `HeapManager` rather than an address. `with_bytes` /
//! `with_bytes_mut` lend the real bytes for the duration of the call
//! (EXTERN-BUF), so the syscall reads and writes **toylang memory
//! directly** — the same zero-copy shape the compiled lanes get, not
//! a staging buffer.

use std::collections::HashMap;

use crate::error::InterpreterError;
use crate::evaluation::extern_io::{with_bytes, with_bytes_mut, ExternBufFn, ExternFn};
use crate::evaluation::EvaluationContext;
use crate::object::Object;
use crate::value::Value;

/// The scalar-only net externs (`core/std/net.t`).
pub fn build_net_registry() -> HashMap<&'static str, ExternFn> {
    let mut m: HashMap<&'static str, ExternFn> = HashMap::new();
    m.insert("__extern_net_backend_name", net_backend_name);
    m.insert("__extern_net_socket", net_socket);
    m.insert("__extern_net_connect", net_connect);
    m.insert("__extern_net_close", net_close);
    m.insert("__extern_net_set_blocking", net_set_blocking);
    m.insert("__extern_net_take_error", net_take_error);
    m.insert("__extern_net_shutdown_write", net_shutdown_write);
    m.insert("__extern_net_status", net_status);
    m
}

/// The two that carry bytes across the boundary.
pub fn build_net_buf_registry() -> HashMap<&'static str, ExternBufFn> {
    let mut m: HashMap<&'static str, ExternBufFn> = HashMap::new();
    m.insert("__extern_net_send", net_send);
    m.insert("__extern_net_recv", net_recv);
    m
}

/// `net::backend_name()` — which event-notification backend the
/// runtime was compiled with.
fn net_backend_name(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_backend_name", args, 0)?;
    Ok(Value::Heap(std::rc::Rc::new(std::cell::RefCell::new(
        Object::String(toylang_rt::net_backend_name().to_string()),
    ))))
}

fn net_socket(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_socket", args, 0)?;
    Ok(Value::Int32(toylang_rt::net_socket()))
}

fn net_connect(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_connect", args, 3)?;
    let fd = fd_arg(&args[0], "__extern_net_connect")?;
    let addr = str_arg(&args[1], "__extern_net_connect")?;
    let port = u64_arg(&args[2], "__extern_net_connect")?;
    Ok(Value::UInt64(toylang_rt::net_connect(fd, addr.as_bytes(), port)))
}

fn net_close(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_close", args, 1)?;
    let fd = fd_arg(&args[0], "__extern_net_close")?;
    Ok(Value::UInt64(toylang_rt::net_close(fd)))
}

fn net_set_blocking(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_set_blocking", args, 2)?;
    let fd = fd_arg(&args[0], "__extern_net_set_blocking")?;
    let on = bool_arg(&args[1], "__extern_net_set_blocking")?;
    Ok(Value::UInt64(toylang_rt::net_set_blocking(fd, on)))
}

fn net_take_error(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_take_error", args, 1)?;
    let fd = fd_arg(&args[0], "__extern_net_take_error")?;
    Ok(Value::UInt64(toylang_rt::net_take_error(fd)))
}

fn net_shutdown_write(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_shutdown_write", args, 1)?;
    let fd = fd_arg(&args[0], "__extern_net_shutdown_write")?;
    Ok(Value::UInt64(toylang_rt::net_shutdown_write(fd)))
}

fn net_status(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_status", args, 0)?;
    Ok(Value::UInt64(toylang_rt::net_status()))
}

/// `TcpStream::write` — the bytes go straight out of toylang memory.
fn net_send(
    ctx: &mut EvaluationContext<'_>,
    args: &[Value],
) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_send", args, 3)?;
    let fd = fd_arg(&args[0], "__extern_net_send")?;
    let len = u64_arg(&args[2], "__extern_net_send")?;
    if len == 0 {
        return Ok(Value::UInt64(toylang_rt::net_send(fd, &[])));
    }
    let sent = with_bytes(ctx, &args[1], len, "__extern_net_send", |bytes| {
        toylang_rt::net_send(fd, bytes)
    })?;
    Ok(Value::UInt64(sent))
}

/// `TcpStream::read` — `recv(2)` fills the caller's buffer in place.
fn net_recv(
    ctx: &mut EvaluationContext<'_>,
    args: &[Value],
) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_recv", args, 3)?;
    let fd = fd_arg(&args[0], "__extern_net_recv")?;
    let len = u64_arg(&args[2], "__extern_net_recv")?;
    if len == 0 {
        return Ok(Value::UInt64(toylang_rt::net_recv(fd, &mut [])));
    }
    let got = with_bytes_mut(ctx, &args[1], len, "__extern_net_recv", |bytes| {
        toylang_rt::net_recv(fd, bytes)
    })?;
    Ok(Value::UInt64(got))
}

fn expect_args(name: &'static str, args: &[Value], want: usize) -> Result<(), InterpreterError> {
    if args.len() == want {
        return Ok(());
    }
    Err(InterpreterError::FunctionParameterMismatch {
        message: format!("extern fn `{name}` takes {want} arguments"),
        expected: want,
        found: args.len(),
    })
}

/// A file descriptor argument. Declared `i32` in `net.t`, but a
/// literal that never went through a narrowing position can still
/// arrive as a wider integer, so every integer width is accepted and
/// truncated the way the C boundary would.
fn fd_arg(value: &Value, name: &str) -> Result<i32, InterpreterError> {
    match value {
        Value::Int32(v) => Ok(*v),
        Value::Int64(v) => Ok(*v as i32),
        Value::UInt64(v) => Ok(*v as i32),
        Value::Int16(v) => Ok(*v as i32),
        Value::Int8(v) => Ok(*v as i32),
        Value::UInt32(v) => Ok(*v as i32),
        Value::UInt16(v) => Ok(*v as i32),
        Value::UInt8(v) => Ok(*v as i32),
        other => Err(InterpreterError::InternalError(format!(
            "extern fn `{name}`: expected a file descriptor, got {other:?}"
        ))),
    }
}

fn u64_arg(value: &Value, name: &str) -> Result<u64, InterpreterError> {
    match value {
        Value::UInt64(v) => Ok(*v),
        Value::Int64(v) => Ok(*v as u64),
        Value::UInt32(v) => Ok(*v as u64),
        Value::UInt16(v) => Ok(*v as u64),
        Value::UInt8(v) => Ok(*v as u64),
        other => Err(InterpreterError::InternalError(format!(
            "extern fn `{name}`: expected an integer, got {other:?}"
        ))),
    }
}

fn bool_arg(value: &Value, name: &str) -> Result<bool, InterpreterError> {
    match value {
        Value::Bool(b) => Ok(*b),
        Value::Heap(rc) => match &*rc.borrow() {
            Object::Bool(b) => Ok(*b),
            other => {
                return Err(InterpreterError::InternalError(format!(
                    "extern fn `{name}`: expected a bool argument, got {other:?}"
                )))
            }
        },
        other => Err(InterpreterError::InternalError(format!(
            "extern fn `{name}`: expected a bool, got {other:?}"
        ))),
    }
}

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
