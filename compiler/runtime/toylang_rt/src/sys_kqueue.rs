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

/// A new non-blocking datagram socket, or `-1`.
///
/// Same three calls as [`socket_stream`]; only the type differs. UDP
/// cannot raise SIGPIPE the way a stream can, but `SO_NOSIGPIPE` is
/// set anyway so the two socket kinds behave identically when
/// something unexpected happens.
pub fn socket_dgram(family: i32) -> i32 {
    let fd = unsafe { crate::socket(family, SOCK_DGRAM, 0) };
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

/// Write `SO_RCVTIMEO` / `SO_SNDTIMEO`, which take a `struct timeval`.
///
/// The whole set goes through here because the payload's *layout*
/// differs, not just the option number: `tv_usec` is 32-bit on the
/// BSDs and 64-bit on Linux, so a shared struct would be wrong on one
/// of them.
pub fn set_timeout(fd: i32, which: i32, ms: i64) -> i32 {
    #[repr(C)]
    struct TimeVal {
        tv_sec: i64,
        tv_usec: i32,
        _pad: i32,
    }
    const _: () = assert!(core::mem::size_of::<TimeVal>() == 16);
    let tv = TimeVal {
        tv_sec: ms / 1000,
        tv_usec: ((ms % 1000) * 1000) as i32,
        _pad: 0,
    };
    unsafe {
        crate::setsockopt(
            fd,
            SOL_SOCKET,
            which,
            (&tv as *const TimeVal).cast(),
            core::mem::size_of::<TimeVal>() as u32,
        )
    }
}

/// Render a `sockaddr_in`'s address into `out` as a dotted quad,
/// returning how many bytes it wrote (0 on failure).
///
/// Reads the struct, so it lives here for the same reason
/// [`sockaddr_port`] does — nothing above this layer touches a
/// sockaddr.
pub fn sockaddr_to_str(sa: *const u8, out: &mut [u8]) -> usize {
    if out.len() < 16 {
        return 0;
    }
    let written = unsafe {
        crate::inet_ntop(
            AF_INET,
            sa.add(4),
            out.as_mut_ptr(),
            out.len() as u32,
        )
    };
    if written.is_null() {
        return 0;
    }
    // `inet_ntop` NUL-terminates; the length is up to that byte.
    out.iter().position(|b| *b == 0).unwrap_or(out.len())
}

/// `TCP_NODELAY`, for turning Nagle's algorithm off.
///
/// The numbers happen to agree on both platforms, but they live here
/// rather than in the shared layer because nothing guarantees the
/// next platform will agree — the porting contract is that a socket
/// constant is `sys`'s to state.
pub const IPPROTO_TCP: i32 = 6;
pub const TCP_NODELAY: i32 = 1;

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

// ---------------------------------------------------------------------------
// Event notification (EVENT_POLLING.md §3). `poll.rs` above calls only
// the four names below and never sees an `EVFILT_*` or an `EV_*`.
// ---------------------------------------------------------------------------

use crate::RawEvent;

unsafe extern "C" {
    fn kqueue() -> i32;
    fn kevent(
        kq: i32,
        changelist: *const KEvent,
        nchanges: i32,
        eventlist: *mut KEvent,
        nevents: i32,
        timeout: *const TimeSpec,
    ) -> i32;
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TimeSpec {
    tv_sec: i64,
    tv_nsec: i64,
}

const F_SETFD: i32 = 2;
const FD_CLOEXEC: i32 = 1;
const ENOENT: i32 = 2;

/// Platform-neutral interest bits. The numbers are defined *here*
/// rather than being the raw `EVFILT_*` / `EPOLL*` ones, because the
/// two backends disagree about those and the layer above must not
/// have to know which it is talking to.
pub const INTEREST_READ: u32 = 1 << 0;
pub const INTEREST_WRITE: u32 = 1 << 1;
pub const INTEREST_EDGE: u32 = 1 << 2;
pub const INTEREST_ONESHOT: u32 = 1 << 3;

pub const EVENT_READ: u32 = 1 << 0;
pub const EVENT_WRITE: u32 = 1 << 1;
/// The peer closed its end. Still readable — whatever is buffered is
/// still there to drain.
pub const EVENT_HUP: u32 = 1 << 2;
pub const EVENT_ERROR: u32 = 1 << 3;

/// The poller fd, close-on-exec. Negative on failure.
///
/// `kqueue()` has no CLOEXEC variant the way `epoll_create1` does, so
/// the flag is a second call. A poller that survived an `exec` would
/// hold every registered descriptor open in the child.
pub fn poll_create() -> i32 {
    let fd = unsafe { kqueue() };
    if fd < 0 {
        return -1;
    }
    unsafe { crate::fcntl(fd, F_SETFD, FD_CLOEXEC) };
    fd
}

/// Register, modify, or (with `interest == 0`) unregister `fd`.
/// `0` on success, `-1` with errno set.
///
/// **The failure is synchronous** (決定 6). kqueue reports a bad
/// changelist entry as an `EV_ERROR` *event*, which would give
/// toylang a world where `register` succeeded and the error arrived
/// later — unlike epoll, where `epoll_ctl` fails on the spot. So the
/// changes go in with an eventlist attached and a zero timeout, and
/// any `EV_ERROR` is turned back into an errno here.
///
/// `ENOENT` from deleting a filter that was never added is swallowed:
/// "make sure this is not registered" is the caller's intent, and it
/// is satisfied.
pub fn poll_ctl(pfd: i32, fd: i32, token: u64, interest: u32) -> i32 {
    let mut flags: u16 = 0;
    if interest & INTEREST_EDGE != 0 {
        flags |= EV_CLEAR;
    }
    if interest & INTEREST_ONESHOT != 0 {
        flags |= EV_ONESHOT;
    }
    let mut changes: [KEvent; 2] = [
        change(fd, EVFILT_READ, interest & INTEREST_READ != 0, flags, token),
        change(fd, EVFILT_WRITE, interest & INTEREST_WRITE != 0, flags, token),
    ];
    let mut out: [KEvent; 2] = changes;
    let zero = TimeSpec::default();
    let n = unsafe {
        kevent(
            pfd,
            changes.as_mut_ptr(),
            changes.len() as i32,
            out.as_mut_ptr(),
            out.len() as i32,
            &zero,
        )
    };
    if n < 0 {
        return -1;
    }
    for ev in out.iter().take(n as usize) {
        if ev.flags & EV_ERROR != 0 && ev.data != 0 && ev.data as i32 != ENOENT {
            crate::set_errno(ev.data as i32);
            return -1;
        }
    }
    0
}

/// Wait for readiness, writing at most `cap` **merged** events into
/// `out`. Returns the count, or `-1` with errno set.
///
/// kqueue reports the read and write sides of one fd as two events,
/// and the merge (決定 1) happens here so nothing above sees the
/// per-filter shape. Two reasons it is worth the linear scan: a loop
/// written on macOS then iterates the same number of times on Linux,
/// and a caller cannot be handed a second event for a descriptor it
/// closed while handling the first.
///
/// The raw buffer holds `2 * cap` because kqueue's own limit applies
/// *before* merging — asking for `cap` raw events could yield fewer
/// than `cap` merged ones and leave the rest queued.
pub fn poll_wait(pfd: i32, out: &mut [RawEvent], cap: usize, timeout_ms: i64) -> isize {
    const RAW_CAP: usize = 2 * crate::POLL_EVENTS_CAP;
    let mut raw: [KEvent; RAW_CAP] = [KEvent {
        ident: 0,
        filter: 0,
        flags: 0,
        fflags: 0,
        data: 0,
        udata: core::ptr::null_mut(),
    }; RAW_CAP];
    let want = core::cmp::min(cap, crate::POLL_EVENTS_CAP) * 2;
    let ts = TimeSpec {
        tv_sec: timeout_ms / 1000,
        tv_nsec: (timeout_ms % 1000) * 1_000_000,
    };
    // A negative timeout means "forever", which kqueue spells as a
    // null pointer rather than a negative number.
    let ts_ptr: *const TimeSpec = if timeout_ms < 0 { core::ptr::null() } else { &ts };
    let n = unsafe {
        kevent(
            pfd,
            core::ptr::null(),
            0,
            raw.as_mut_ptr(),
            want as i32,
            ts_ptr,
        )
    };
    if n < 0 {
        return -1;
    }
    let mut len = 0usize;
    for ev in raw.iter().take(n as usize) {
        let token = ev.udata as u64;
        let mut flags = match ev.filter {
            EVFILT_READ => EVENT_READ,
            EVFILT_WRITE => EVENT_WRITE,
            _ => 0,
        };
        if ev.flags & EV_EOF != 0 {
            flags |= EVENT_HUP;
        }
        let mut error = 0u32;
        if ev.flags & EV_ERROR != 0 {
            flags |= EVENT_ERROR;
            // Unlike epoll, the errno is right here in `data` — no
            // `SO_ERROR` round trip.
            error = ev.data as u32;
        }
        // Merge into an entry for the same token if one is already
        // staged. The scan is linear because a wait returns a handful
        // of events, not thousands; a map would cost more to build
        // than it saves.
        if let Some(slot) = out[..len].iter_mut().find(|e| e.token == token) {
            slot.flags |= flags;
            if slot.error == 0 {
                slot.error = error;
            }
            continue;
        }
        if len == cap {
            break;
        }
        out[len] = RawEvent { token, flags, error };
        len += 1;
    }
    len as isize
}

/// One changelist entry: add the filter, or delete it when the
/// interest does not name it.
fn change(fd: i32, filter: i16, wanted: bool, extra: u16, token: u64) -> KEvent {
    KEvent {
        ident: fd as usize,
        filter,
        flags: if wanted { EV_ADD | extra } else { EV_DELETE },
        fflags: 0,
        data: 0,
        udata: token as *mut u8,
    }
}
