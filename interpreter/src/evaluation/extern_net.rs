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
    m.insert("__extern_net_bind", net_bind);
    m.insert("__extern_net_local_port", net_local_port);
    m.insert("__extern_net_accept", net_accept);
    // N4: addresses, socket options, UDP.
    m.insert("__extern_net_local_addr", net_local_addr);
    m.insert("__extern_net_peer_addr", net_peer_addr);
    m.insert("__extern_net_peer_port", net_peer_port);
    m.insert("__extern_net_set_nodelay", net_set_nodelay);
    m.insert("__extern_net_set_timeout", net_set_timeout);
    m.insert("__extern_net_udp_bind", net_udp_bind);
    m.insert("__extern_net_set_dest", net_set_dest);
    m.insert("__extern_net_last_peer_addr", net_last_peer_addr);
    m.insert("__extern_net_last_peer_port", net_last_peer_port);
    m.insert("__extern_net_resolve", net_resolve);
    // EVENT_POLLING N3.
    m.insert("__extern_poll_create", poll_create);
    m.insert("__extern_poll_ctl", poll_ctl);
    m.insert("__extern_poll_wait", poll_wait);
    m.insert("__extern_poll_event_token", poll_event_token);
    m.insert("__extern_poll_event_flags", poll_event_flags);
    m.insert("__extern_poll_event_error", poll_event_error);
    m.insert("__extern_poll_error_status", poll_error_status);
    m
}

/// The two that carry bytes across the boundary.
pub fn build_net_buf_registry() -> HashMap<&'static str, ExternBufFn> {
    let mut m: HashMap<&'static str, ExternBufFn> = HashMap::new();
    m.insert("__extern_net_send", net_send);
    m.insert("__extern_net_recv", net_recv);
    m.insert("__extern_net_send_to", net_send_to);
    m.insert("__extern_net_recv_from", net_recv_from);
    m
}

/// `net::backend_name()` — which event-notification backend the
/// runtime was compiled with.
fn net_backend_name(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_backend_name", args, 0)?;
    Ok(str_value(toylang_rt::net_backend_name().to_string()))
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

fn net_bind(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_bind", args, 3)?;
    let addr = str_arg(&args[0], "__extern_net_bind")?;
    let port = u64_arg(&args[1], "__extern_net_bind")?;
    let backlog = fd_arg(&args[2], "__extern_net_bind")?;
    Ok(Value::Int32(toylang_rt::net_bind(addr.as_bytes(), port, backlog)))
}

fn net_local_port(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_local_port", args, 1)?;
    let fd = fd_arg(&args[0], "__extern_net_local_port")?;
    Ok(Value::UInt64(toylang_rt::net_local_port(fd)))
}

fn net_accept(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_accept", args, 1)?;
    let fd = fd_arg(&args[0], "__extern_net_accept")?;
    Ok(Value::Int32(toylang_rt::net_accept(fd)))
}

fn poll_create(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_poll_create", args, 0)?;
    Ok(Value::Int32(toylang_rt::poll_create()))
}

fn poll_ctl(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_poll_ctl", args, 4)?;
    let pfd = fd_arg(&args[0], "__extern_poll_ctl")?;
    let fd = fd_arg(&args[1], "__extern_poll_ctl")?;
    let token = u64_arg(&args[2], "__extern_poll_ctl")?;
    let interest = u64_arg(&args[3], "__extern_poll_ctl")? as u32;
    Ok(Value::UInt64(toylang_rt::poll_ctl(pfd, fd, token, interest)))
}

fn poll_wait(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_poll_wait", args, 2)?;
    let pfd = fd_arg(&args[0], "__extern_poll_wait")?;
    let timeout = i64_arg(&args[1], "__extern_poll_wait")?;
    Ok(Value::UInt64(toylang_rt::poll_wait(pfd, timeout)))
}

fn poll_event_token(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_poll_event_token", args, 1)?;
    let i = u64_arg(&args[0], "__extern_poll_event_token")?;
    Ok(Value::UInt64(toylang_rt::poll_event_token(i)))
}

fn poll_event_flags(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_poll_event_flags", args, 1)?;
    let i = u64_arg(&args[0], "__extern_poll_event_flags")?;
    Ok(Value::UInt32(toylang_rt::poll_event_flags(i)))
}

fn poll_event_error(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_poll_event_error", args, 1)?;
    let i = u64_arg(&args[0], "__extern_poll_event_error")?;
    Ok(Value::UInt64(toylang_rt::poll_event_error(i)))
}

fn poll_error_status(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_poll_error_status", args, 1)?;
    let errno = u64_arg(&args[0], "__extern_poll_error_status")?;
    Ok(Value::UInt64(toylang_rt::poll_error_status(errno)))
}

/// A signed argument. `timeout_ms` is the only one, and its negative
/// values are load-bearing (-1 means "wait forever").
fn i64_arg(value: &Value, name: &str) -> Result<i64, InterpreterError> {
    match value {
        Value::Int64(v) => Ok(*v),
        Value::UInt64(v) => Ok(*v as i64),
        Value::Int32(v) => Ok(*v as i64),
        Value::Int16(v) => Ok(*v as i64),
        Value::Int8(v) => Ok(*v as i64),
        other => Err(InterpreterError::InternalError(format!(
            "extern fn `{name}`: expected a signed integer, got {other:?}"
        ))),
    }
}

/// The dotted quad of one end of `fd`.
fn addr_text(fd: i32, peer: bool) -> String {
    let mut out = [0u8; 16];
    let n = toylang_rt::net_addr_text(fd, peer, &mut out);
    String::from_utf8_lossy(&out[..n]).into_owned()
}

fn net_local_addr(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_local_addr", args, 1)?;
    let fd = fd_arg(&args[0], "__extern_net_local_addr")?;
    Ok(str_value(addr_text(fd, false)))
}

fn net_peer_addr(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_peer_addr", args, 1)?;
    let fd = fd_arg(&args[0], "__extern_net_peer_addr")?;
    Ok(str_value(addr_text(fd, true)))
}

fn net_peer_port(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_peer_port", args, 1)?;
    let fd = fd_arg(&args[0], "__extern_net_peer_port")?;
    Ok(Value::UInt64(toylang_rt::net_addr_port(fd, true)))
}

fn net_set_nodelay(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_set_nodelay", args, 2)?;
    let fd = fd_arg(&args[0], "__extern_net_set_nodelay")?;
    let on = bool_arg(&args[1], "__extern_net_set_nodelay")?;
    Ok(Value::UInt64(toylang_rt::net_set_nodelay(fd, on)))
}

fn net_set_timeout(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_set_timeout", args, 3)?;
    let fd = fd_arg(&args[0], "__extern_net_set_timeout")?;
    let ms = i64_arg(&args[1], "__extern_net_set_timeout")?;
    let write_side = bool_arg(&args[2], "__extern_net_set_timeout")?;
    Ok(Value::UInt64(toylang_rt::net_set_timeout(fd, ms, write_side)))
}

fn net_udp_bind(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_udp_bind", args, 2)?;
    let addr = str_arg(&args[0], "__extern_net_udp_bind")?;
    let port = u64_arg(&args[1], "__extern_net_udp_bind")?;
    Ok(Value::Int32(toylang_rt::net_udp_bind(addr.as_bytes(), port)))
}

fn net_set_dest(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_set_dest", args, 2)?;
    let addr = str_arg(&args[0], "__extern_net_set_dest")?;
    let port = u64_arg(&args[1], "__extern_net_set_dest")?;
    let ok = toylang_rt::net_set_dest(addr.as_bytes(), port);
    Ok(Value::UInt64(if ok { 0 } else { toylang_rt::net_status() }))
}

fn net_last_peer_addr(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_last_peer_addr", args, 0)?;
    let mut out = [0u8; 16];
    let n = toylang_rt::net_last_peer_addr(&mut out);
    Ok(str_value(String::from_utf8_lossy(&out[..n]).into_owned()))
}

fn net_last_peer_port(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_last_peer_port", args, 0)?;
    Ok(Value::UInt64(toylang_rt::net_last_peer_port()))
}

/// `UdpSocket::send_to` — the bytes go straight out of toylang memory.
fn net_send_to(
    ctx: &mut EvaluationContext<'_>,
    args: &[Value],
) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_send_to", args, 3)?;
    let fd = fd_arg(&args[0], "__extern_net_send_to")?;
    let len = u64_arg(&args[2], "__extern_net_send_to")?;
    let mut dest = [0u8; 16];
    let (dest_len, port) = toylang_rt::net_dest(&mut dest);
    if len == 0 {
        return Ok(Value::UInt64(toylang_rt::net_send_to(
            fd,
            &[],
            &dest[..dest_len],
            port,
        )));
    }
    let sent = with_bytes(ctx, &args[1], len, "__extern_net_send_to", |bytes| {
        toylang_rt::net_send_to(fd, bytes, &dest[..dest_len], port)
    })?;
    Ok(Value::UInt64(sent))
}

/// `UdpSocket::recv_from` — `recvfrom(2)` fills the caller's buffer.
fn net_recv_from(
    ctx: &mut EvaluationContext<'_>,
    args: &[Value],
) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_recv_from", args, 3)?;
    let fd = fd_arg(&args[0], "__extern_net_recv_from")?;
    let len = u64_arg(&args[2], "__extern_net_recv_from")?;
    if len == 0 {
        return Ok(Value::UInt64(toylang_rt::net_recv_from(fd, &mut [])));
    }
    let got = with_bytes_mut(ctx, &args[1], len, "__extern_net_recv_from", |bytes| {
        toylang_rt::net_recv_from(fd, bytes)
    })?;
    Ok(Value::UInt64(got))
}

fn net_resolve(args: &[Value]) -> Result<Value, InterpreterError> {
    expect_args("__extern_net_resolve", args, 1)?;
    let host = str_arg(&args[0], "__extern_net_resolve")?;
    let mut out = [0u8; 16];
    let n = toylang_rt::net_resolve(host.as_bytes(), &mut out);
    Ok(str_value(String::from_utf8_lossy(&out[..n]).into_owned()))
}

/// A toylang `str` result — a `String` object on this engine.
fn str_value(s: String) -> Value {
    Value::Heap(std::rc::Rc::new(std::cell::RefCell::new(Object::String(s))))
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
            other => Err(InterpreterError::InternalError(format!(
                "extern fn `{name}`: expected a bool argument, got {other:?}"
            ))),
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
