//! Platform layer for the BSD family: kqueue, and the socket calls
//! whose shape differs from Linux's (NETWORK_IO.md §2).
//!
//! This file *is* the porting contract. Everything `net.rs` /
//! `poll.rs` reach for has to exist here and in `sys_epoll.rs` under
//! the same name, or that target fails to build — which is the whole
//! reason the switch is one `#[cfg_attr(path)] mod sys;` rather than
//! forty `#[cfg]`s scattered through the callers. Keep `#[cfg]` out of
//! those files: OS differences live here.
//!
//! **The numbers are checked, not trusted.** They are transcribed from
//! the system headers by hand (the crate is dependency-free), so
//! `compiler/tests/net_abi_tests.rs` compiles a C probe that prints
//! the real values and compares them against these. A wrong constant
//! would otherwise compile cleanly and misbehave at run time.
//!
//! Scope: macOS is what this is verified on. FreeBSD 12+ widened
//! `struct kevent` with an `ext[4]` tail, so adding it means a `cfg`
//! on the struct below plus probe rows for that host — not a new file.

/// Which event-notification backend this build uses. Reaches toylang
/// through `net::backend_name()`, so a program (and the consistency
/// tests) can see which half of the switch is live.
pub const BACKEND_NAME: &str = "kqueue";

// ---------------------------------------------------------------------------
// Socket-level constants whose *values* differ from Linux's.
// ---------------------------------------------------------------------------

pub const AF_INET: i32 = 2;
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
/// Linux spells this 1; the BSDs use 0xffff.
pub const SOL_SOCKET: i32 = 0xffff;
/// Linux 2.
pub const SO_REUSEADDR: i32 = 0x0004;
/// Linux 20 / 21.
pub const SO_RCVTIMEO: i32 = 0x1006;
pub const SO_SNDTIMEO: i32 = 0x1005;
pub const SO_ERROR: i32 = 0x1007;
/// No Linux counterpart: there, `send(MSG_NOSIGNAL)` suppresses
/// SIGPIPE per call instead of the socket carrying the option.
pub const SO_NOSIGPIPE: i32 = 0x1022;
/// Linux 0o4000 (2048).
pub const O_NONBLOCK: i32 = 0x0004;
pub const F_GETFL: i32 = 3;
pub const F_SETFL: i32 = 4;

/// `struct sockaddr_in` is 16 bytes on both platforms, but its first
/// two bytes mean different things (the BSDs split them into `sin_len`
/// and a one-byte `sin_family`). The runtime builds one internally so
/// toylang never sees a sockaddr; the size is what a stack buffer for
/// it has to be.
pub const SOCKADDR_IN_BYTES: usize = 16;

/// `struct timeval`'s microseconds field is 32-bit here and 64-bit on
/// Linux, so the option payload differs in layout as well as in name.
pub const TIMEVAL_USEC_BYTES: usize = 4;

// ---------------------------------------------------------------------------
// kqueue.
// ---------------------------------------------------------------------------

pub const EVFILT_READ: i16 = -1;
pub const EVFILT_WRITE: i16 = -2;

pub const EV_ADD: u16 = 0x0001;
pub const EV_DELETE: u16 = 0x0002;
pub const EV_ONESHOT: u16 = 0x0010;
pub const EV_CLEAR: u16 = 0x0020;
pub const EV_EOF: u16 = 0x8000;
pub const EV_ERROR: u16 = 0x4000;

/// 32 bytes on LP64. FreeBSD 12+ appends `ext[4]`; see the module
/// docs.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KEvent {
    pub ident: usize,
    pub filter: i16,
    pub flags: u16,
    pub fflags: u32,
    pub data: isize,
    pub udata: *mut u8,
}

/// The size the ABI probe checks this file's `KEvent` against.
pub const EVENT_STRUCT_BYTES: usize = 32;

// The transcription and the compiler have to agree before the probe
// even runs: a `#[repr(C)]` mistake here would otherwise only show up
// as a mismatch against the C header.
const _: () = assert!(core::mem::size_of::<KEvent>() == EVENT_STRUCT_BYTES);

// ---------------------------------------------------------------------------
// errno. The network values disagree with Linux's, unlike the four
// (ENOENT / EPERM / EACCES / EISDIR) the existing file I/O relies on.
// ---------------------------------------------------------------------------

pub const EINTR: i32 = 4;
pub const EAGAIN: i32 = 35;
pub const EINVAL: i32 = 22;
pub const EMFILE: i32 = 24;
pub const EPIPE: i32 = 32;
pub const EADDRINUSE: i32 = 48;
pub const EADDRNOTAVAIL: i32 = 49;
pub const ENETUNREACH: i32 = 51;
pub const ECONNABORTED: i32 = 53;
pub const ECONNRESET: i32 = 54;
pub const ENOTCONN: i32 = 57;
pub const ETIMEDOUT: i32 = 60;
pub const ECONNREFUSED: i32 = 61;
pub const EHOSTUNREACH: i32 = 65;
pub const EINPROGRESS: i32 = 36;

// ---------------------------------------------------------------------------
// Socket operations whose *shape* differs from Linux's (NETWORK_IO.md
// §2). Everything above this layer calls these names and never a
// syscall directly, so the two `sys_*.rs` files are the only place a
// platform difference is spelled.
// ---------------------------------------------------------------------------

use crate::{
    accept, close, fcntl, inet_pton, send, setsockopt, NET_ADDR_IN_USE,
    NET_ADDR_NOT_AVAILABLE, NET_BROKEN_PIPE, NET_CONNECTION_ABORTED, NET_CONNECTION_REFUSED,
    NET_CONNECTION_RESET, NET_HOST_UNREACHABLE, NET_IN_PROGRESS, NET_INTERRUPTED,
    NET_INVALID_INPUT, NET_NETWORK_UNREACHABLE, NET_NOT_CONNECTED, NET_TIMED_OUT,
    NET_TOO_MANY_OPEN_FILES, NET_UNKNOWN, NET_WOULD_BLOCK,
};

/// A new stream socket, already non-blocking. Returns `-1` with errno
/// set, like the syscall it wraps.
///
/// Linux gets both properties out of `socket()` itself
/// (`SOCK_NONBLOCK | SOCK_CLOEXEC`); here it takes three calls. The
/// third is `SO_NOSIGPIPE`, which has no Linux counterpart at all —
/// there, `send(MSG_NOSIGNAL)` suppresses the signal per call. Either
/// way the socket cannot kill the process by writing to a closed
/// peer, which is the property `net.t` relies on: a `write` to a
/// hung-up connection has to come back as `BrokenPipe`, not as a
/// signal.
pub fn socket_stream(family: i32) -> i32 {
    let fd = unsafe { crate::socket(family, SOCK_STREAM, 0) };
    if fd < 0 {
        return -1;
    }
    if set_blocking(fd, false) != 0 {
        close_preserving_errno(fd);
        return -1;
    }
    set_nosigpipe(fd);
    fd
}


/// Accept a pending connection, handing back a non-blocking fd.
///
/// Linux does this in one `accept4()`; the BSDs inherit neither the
/// non-blocking flag nor `SO_NOSIGPIPE` from the listener, so both
/// are set again on the accepted socket. Forgetting either is the
/// classic BSD bug: the listener is non-blocking, the connection is
/// not, and the first `read` on an idle connection blocks the whole
/// single-threaded program.
pub fn accept_nonblocking(listen_fd: i32) -> i32 {
    let fd = unsafe { accept(listen_fd, core::ptr::null_mut(), core::ptr::null_mut()) };
    if fd < 0 {
        return -1;
    }
    if set_blocking(fd, false) != 0 {
        close_preserving_errno(fd);
        return -1;
    }
    set_nosigpipe(fd);
    fd
}

/// Read the port back out of a `sockaddr_in`. Same offset and byte
/// order on both platforms, but it lives here because it reads the
/// struct — the point of the layer is that nothing above it does.
pub fn sockaddr_port(sa: *const u8) -> u16 {
    unsafe { u16::from_be_bytes([*sa.add(2), *sa.add(3)]) }
}

/// `send(2)` that cannot raise SIGPIPE. The socket already carries
/// `SO_NOSIGPIPE`, so no per-call flag is needed; Linux passes
/// `MSG_NOSIGNAL` here instead.
pub fn send_nosignal(fd: i32, buf: *const u8, len: usize) -> isize {
    unsafe { send(fd, buf, len, 0) }
}

/// Turn blocking mode on or off. `0` on success, `-1` with errno set.
///
/// Blocking is a property of the fd rather than of the call
/// (NETWORK_IO.md 論点 1), so this is what decides whether `accept` /
/// `read` / `write` return `WouldBlock` or wait.
pub fn set_blocking(fd: i32, on: bool) -> i32 {
    let flags = unsafe { fcntl(fd, F_GETFL, 0) };
    if flags < 0 {
        return -1;
    }
    let next = if on { flags & !O_NONBLOCK } else { flags | O_NONBLOCK };
    if unsafe { fcntl(fd, F_SETFL, next) } < 0 {
        -1
    } else {
        0
    }
}

/// Fill `out` (at least [`SOCKADDR_IN_BYTES`] writable bytes) with a
/// `sockaddr_in` for `text:port`. `0` on success, `-1` when the text
/// is not a numeric address of this family.
///
/// **This is why toylang never sees a sockaddr.** The BSDs put a
/// one-byte `sin_len` in front of a one-byte `sin_family`; Linux has
/// a two-byte `sin_family` and no length at all. Both structs are 16
/// bytes, so the difference is invisible to a caller passing a buffer
/// — and it stays invisible only because the writing happens here.
pub fn sockaddr_from_str(text: &[u8], port: u16, family: i32, out: *mut u8) -> i32 {
    if family != AF_INET {
        return -1;
    }
    // `inet_pton` wants a C string; addresses are short, and a text
    // longer than this buffer cannot be a dotted quad anyway.
    let mut cstr = [0u8; 64];
    if text.len() >= cstr.len() {
        return -1;
    }
    cstr[..text.len()].copy_from_slice(text);
    let mut octets = [0u8; 4];
    if unsafe { inet_pton(AF_INET, cstr.as_ptr(), octets.as_mut_ptr()) } != 1 {
        return -1;
    }
    unsafe {
        core::ptr::write_bytes(out, 0, SOCKADDR_IN_BYTES);
        *out = SOCKADDR_IN_BYTES as u8;
        *out.add(1) = AF_INET as u8;
        let be = port.to_be_bytes();
        *out.add(2) = be[0];
        *out.add(3) = be[1];
        core::ptr::copy_nonoverlapping(octets.as_ptr(), out.add(4), 4);
    }
    0
}


/// Map an errno to the OS-independent status vocabulary toylang sees.
///
/// This has to live per-platform because the *values* disagree:
/// EAGAIN is 11 on Linux and 35 here, EADDRINUSE 98 against 48,
/// ECONNREFUSED 111 against 61. The four the existing file I/O relies
/// on (ENOENT / EPERM / EACCES / EISDIR) happen to match, which is
/// exactly why `io_status_from_errno` could be written once and this
/// cannot.
pub fn status_from_errno(err: i32) -> u64 {
    match err {
        EAGAIN => NET_WOULD_BLOCK,
        EINPROGRESS => NET_IN_PROGRESS,
        EINTR => NET_INTERRUPTED,
        ECONNREFUSED => NET_CONNECTION_REFUSED,
        ECONNRESET => NET_CONNECTION_RESET,
        ECONNABORTED => NET_CONNECTION_ABORTED,
        EPIPE => NET_BROKEN_PIPE,
        ENOTCONN => NET_NOT_CONNECTED,
        EADDRINUSE => NET_ADDR_IN_USE,
        EADDRNOTAVAIL => NET_ADDR_NOT_AVAILABLE,
        ENETUNREACH => NET_NETWORK_UNREACHABLE,
        EHOSTUNREACH => NET_HOST_UNREACHABLE,
        ETIMEDOUT => NET_TIMED_OUT,
        EMFILE => NET_TOO_MANY_OPEN_FILES,
        EINVAL => NET_INVALID_INPUT,
        _ => NET_UNKNOWN,
    }
}

/// Ask the socket to report a dead peer as `EPIPE` rather than
/// raising SIGPIPE. A failure is ignored on purpose: the socket still
/// works, and the fallback is the process's own SIGPIPE disposition.
fn set_nosigpipe(fd: i32) {
    let on: i32 = 1;
    unsafe {
        setsockopt(
            fd,
            SOL_SOCKET,
            SO_NOSIGPIPE,
            (&on as *const i32).cast(),
            core::mem::size_of::<i32>() as u32,
        );
    }
}

/// Close `fd` without disturbing the errno the caller is about to
/// report. `close` can fail on its own, and its errno would otherwise
/// replace the one that explains why we are unwinding.
fn close_preserving_errno(fd: i32) {
    let saved = crate::current_errno();
    unsafe { close(fd) };
    crate::set_errno(saved);
}
