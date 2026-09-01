//! Platform layer for Linux: epoll, and the socket calls whose shape
//! differs from the BSDs' (NETWORK_IO.md §2).
//!
//! The counterpart of `sys_kqueue.rs`; see that file's header for why
//! the switch is one `#[cfg_attr(path)] mod sys;` and why every name
//! here has to exist there too.
//!
//! **Not verified on this host.** The repository's toolchain is
//! macOS-only, so this file is compiled by a Linux build alone, and
//! the ABI probe (`compiler/tests/net_abi_tests.rs`) checks whichever
//! backend is live — meaning it checks these numbers on Linux and the
//! kqueue ones here. Treat a Linux CI run as the first real test of
//! this file.

/// See `sys_kqueue::BACKEND_NAME`.
pub const BACKEND_NAME: &str = "epoll";

// ---------------------------------------------------------------------------
// Socket-level constants whose *values* differ from the BSDs'.
// ---------------------------------------------------------------------------

pub const AF_INET: i32 = 2;
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
/// The BSDs use 0xffff.
pub const SOL_SOCKET: i32 = 1;
/// BSD 0x0004.
pub const SO_REUSEADDR: i32 = 2;
/// BSD 0x1006 / 0x1005.
pub const SO_RCVTIMEO: i32 = 20;
pub const SO_SNDTIMEO: i32 = 21;
pub const SO_ERROR: i32 = 4;
/// BSD 0x0004.
pub const O_NONBLOCK: i32 = 0o4000;
pub const F_GETFL: i32 = 3;
pub const F_SETFL: i32 = 4;

/// `struct sockaddr_in` is 16 bytes on both platforms, but its first
/// two bytes mean different things (the BSDs split them into `sin_len`
/// and a one-byte `sin_family`). The runtime builds one internally so
/// toylang never sees a sockaddr; the size is what a stack buffer for
/// it has to be.
pub const SOCKADDR_IN_BYTES: usize = 16;

/// `struct timeval`'s microseconds field is 64-bit here and 32-bit on
/// the BSDs.
pub const TIMEVAL_USEC_BYTES: usize = 8;

/// Suppresses SIGPIPE per `send` call. The BSDs set `SO_NOSIGPIPE` on
/// the socket instead, which is why this is a constant on one side of
/// the switch and an option on the other.
pub const MSG_NOSIGNAL: i32 = 0x4000;

// ---------------------------------------------------------------------------
// epoll.
// ---------------------------------------------------------------------------

pub const EPOLLIN: u32 = 0x001;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLRDHUP: u32 = 0x2000;
pub const EPOLLONESHOT: u32 = 1 << 30;
pub const EPOLLET: u32 = 1 << 31;

pub const EPOLL_CTL_ADD: i32 = 1;
pub const EPOLL_CTL_DEL: i32 = 2;
pub const EPOLL_CTL_MOD: i32 = 3;

/// **Packed on x86_64 only.** glibc defines `EPOLL_PACKED` as
/// `__attribute__((packed))` there so the 64-bit layout matches the
/// 32-bit one; on aarch64 the struct takes its natural alignment.
/// Getting this wrong shifts every `data` field by four bytes and the
/// tokens come back as garbage, which is the single most likely way
/// this file is wrong — hence the probe row for its size.
#[cfg(target_arch = "x86_64")]
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

#[cfg(not(target_arch = "x86_64"))]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

/// The size the ABI probe checks this file's `EpollEvent` against.
#[cfg(target_arch = "x86_64")]
pub const EVENT_STRUCT_BYTES: usize = 12;
#[cfg(not(target_arch = "x86_64"))]
pub const EVENT_STRUCT_BYTES: usize = 16;

// See the kqueue file: catch a `#[repr]` mistake at compile time
// rather than as a probe mismatch. This is also the assertion that
// would fire if the `packed` cfg above were dropped on x86_64.
const _: () = assert!(core::mem::size_of::<EpollEvent>() == EVENT_STRUCT_BYTES);

// ---------------------------------------------------------------------------
// errno. The network values disagree with the BSDs', unlike the four
// (ENOENT / EPERM / EACCES / EISDIR) the existing file I/O relies on.
// ---------------------------------------------------------------------------

pub const EINTR: i32 = 4;
pub const EAGAIN: i32 = 11;
pub const EINVAL: i32 = 22;
pub const EMFILE: i32 = 24;
pub const EPIPE: i32 = 32;
pub const EADDRINUSE: i32 = 98;
pub const EADDRNOTAVAIL: i32 = 99;
pub const ENETUNREACH: i32 = 101;
pub const ECONNABORTED: i32 = 103;
pub const ECONNRESET: i32 = 104;
pub const ENOTCONN: i32 = 107;
pub const ETIMEDOUT: i32 = 110;
pub const ECONNREFUSED: i32 = 111;
pub const EHOSTUNREACH: i32 = 113;
pub const EINPROGRESS: i32 = 115;

// ---------------------------------------------------------------------------
// Socket operations whose *shape* differs from the BSDs' (NETWORK_IO.md
// §2). Everything above this layer calls these names and never a
// syscall directly, so the two `sys_*.rs` files are the only place a
// platform difference is spelled.
// ---------------------------------------------------------------------------

use crate::{
    close, fcntl, inet_pton, send, NET_ADDR_IN_USE, NET_ADDR_NOT_AVAILABLE,
    NET_BROKEN_PIPE, NET_CONNECTION_ABORTED, NET_CONNECTION_REFUSED, NET_CONNECTION_RESET,
    NET_HOST_UNREACHABLE, NET_IN_PROGRESS, NET_INTERRUPTED, NET_INVALID_INPUT,
    NET_NETWORK_UNREACHABLE, NET_NOT_CONNECTED, NET_TIMED_OUT, NET_TOO_MANY_OPEN_FILES,
    NET_UNKNOWN, NET_WOULD_BLOCK,
};

/// `SOCK_NONBLOCK` / `SOCK_CLOEXEC` fold the two properties a fresh
/// socket needs into the `socket()` / `accept4()` type argument. The
/// BSDs have neither and pay for both with extra `fcntl` calls.
pub const SOCK_NONBLOCK: i32 = 0o4000;
pub const SOCK_CLOEXEC: i32 = 0o2000000;

/// A new stream socket, already non-blocking. Returns `-1` with errno
/// set, like the syscall it wraps.
///
/// One call, because Linux lets the type argument carry the flags.
/// SIGPIPE is not addressed here at all: there is no `SO_NOSIGPIPE`
/// on this platform, so [`send_nosignal`] passes `MSG_NOSIGNAL` per
/// call instead. Either way a `write` to a hung-up peer comes back as
/// `BrokenPipe` rather than killing the process, which is what
/// `net.t` relies on.
pub fn socket_stream(family: i32) -> i32 {
    unsafe { crate::socket(family, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0) }
}


/// Linux-only calls. `accept4` is the whole reason this is a separate
/// declaration from the shared socket surface in `lib.rs`: the BSDs
/// have no such entry point, which is why accepting a non-blocking
/// connection is one call here and three there.
unsafe extern "C" {
    fn accept4(fd: i32, addr: *mut u8, len: *mut u32, flags: i32) -> i32;
}

/// Accept a pending connection, handing back a non-blocking fd.
///
/// `accept4` applies the flags atomically. On the BSDs the accepted
/// socket inherits neither the listener's non-blocking flag nor its
/// `SO_NOSIGPIPE`, so that side has to set both again.
pub fn accept_nonblocking(listen_fd: i32) -> i32 {
    unsafe {
        accept4(
            listen_fd,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            SOCK_NONBLOCK | SOCK_CLOEXEC,
        )
    }
}

/// A new non-blocking datagram socket, or `-1`. One call, as with
/// [`socket_stream`].
pub fn socket_dgram(family: i32) -> i32 {
    unsafe { crate::socket(family, SOCK_DGRAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0) }
}

/// Write `SO_RCVTIMEO` / `SO_SNDTIMEO`, which take a `struct timeval`.
///
/// The whole set goes through here because the payload's *layout*
/// differs, not just the option number: `tv_usec` is 64-bit here and
/// 32-bit on the BSDs, so a shared struct would be wrong on one of
/// them.
pub fn set_timeout(fd: i32, which: i32, ms: i64) -> i32 {
    #[repr(C)]
    struct TimeVal {
        tv_sec: i64,
        tv_usec: i64,
    }
    const _: () = assert!(core::mem::size_of::<TimeVal>() == 16);
    let tv = TimeVal {
        tv_sec: ms / 1000,
        tv_usec: (ms % 1000) * 1000,
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

/// `send(2)` that cannot raise SIGPIPE. The flag is per call here;
/// the BSDs put the equivalent on the socket at creation.
pub fn send_nosignal(fd: i32, buf: *const u8, len: usize) -> isize {
    unsafe { send(fd, buf, len, MSG_NOSIGNAL) }
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
/// **This is why toylang never sees a sockaddr.** Linux opens the
/// struct with a two-byte `sin_family` and no length field; the BSDs
/// split those same two bytes into `sin_len` and a one-byte
/// `sin_family`. Both structs are 16 bytes, so a caller handing over
/// a buffer cannot tell — and only because the writing happens here.
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
        let fam = (AF_INET as u16).to_ne_bytes();
        *out = fam[0];
        *out.add(1) = fam[1];
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
/// EAGAIN is 11 here and 35 on the BSDs, EADDRINUSE 98 against 48,
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

/// Close `fd` without disturbing the errno the caller is about to
/// report. Unused on this platform — `socket_stream` and
/// `accept_nonblocking` are single calls that never have a
/// half-built fd to unwind — but named here so the porting contract
/// stays symmetric.
#[allow(dead_code)]
fn close_preserving_errno(fd: i32) {
    let saved = crate::current_errno();
    unsafe { close(fd) };
    crate::set_errno(saved);
}

// ---------------------------------------------------------------------------
// Event notification (EVENT_POLLING.md §3). `poll.rs` above calls only
// the four names below and never sees an `EPOLL*`.
// ---------------------------------------------------------------------------

use crate::RawEvent;

unsafe extern "C" {
    fn epoll_create1(flags: i32) -> i32;
    fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: *mut EpollEvent) -> i32;
    fn epoll_wait(epfd: i32, events: *mut EpollEvent, maxevents: i32, timeout: i32) -> i32;
}

const EPOLL_CLOEXEC: i32 = 0o2000000;
const EEXIST: i32 = 17;
const ENOENT: i32 = 2;

/// Platform-neutral interest bits. Same values as the kqueue file's,
/// and deliberately not the `EPOLL*` numbers: the layer above must not
/// be able to tell which backend it is talking to.
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
/// One call here; the BSDs need a second `fcntl` because `kqueue()`
/// has no CLOEXEC variant.
pub fn poll_create() -> i32 {
    unsafe { epoll_create1(EPOLL_CLOEXEC) }
}

/// Register, modify, or (with `interest == 0`) unregister `fd`.
/// `0` on success, `-1` with errno set.
///
/// epoll splits registration into ADD and MOD and fails the wrong one
/// with `EEXIST` / `ENOENT`; the unified `register` is idempotent, so
/// that split is hidden by retrying. `ENOENT` from a delete of
/// something never registered is swallowed — "make sure this is not
/// registered" is the caller's intent, and it is satisfied.
pub fn poll_ctl(pfd: i32, fd: i32, token: u64, interest: u32) -> i32 {
    if interest == 0 {
        let mut ev = EpollEvent { events: 0, data: token };
        if unsafe { epoll_ctl(pfd, EPOLL_CTL_DEL, fd, &mut ev) } != 0 {
            if crate::current_errno() == ENOENT {
                return 0;
            }
            return -1;
        }
        return 0;
    }
    let mut events: u32 = 0;
    if interest & INTEREST_READ != 0 {
        // RDHUP as well as IN: a peer that closed while we were not
        // looking has to wake the loop, which is what EV_EOF does on
        // the other side without being asked.
        events |= EPOLLIN | EPOLLRDHUP;
    }
    if interest & INTEREST_WRITE != 0 {
        events |= EPOLLOUT;
    }
    if interest & INTEREST_EDGE != 0 {
        events |= EPOLLET;
    }
    if interest & INTEREST_ONESHOT != 0 {
        events |= EPOLLONESHOT;
    }
    let mut ev = EpollEvent { events, data: token };
    if unsafe { epoll_ctl(pfd, EPOLL_CTL_ADD, fd, &mut ev) } == 0 {
        return 0;
    }
    if crate::current_errno() != EEXIST {
        return -1;
    }
    let mut ev = EpollEvent { events, data: token };
    if unsafe { epoll_ctl(pfd, EPOLL_CTL_MOD, fd, &mut ev) } != 0 {
        return -1;
    }
    0
}

/// Wait for readiness, writing at most `cap` events into `out`.
/// Returns the count, or `-1` with errno set.
///
/// No merging is needed here — epoll already reports one event per
/// descriptor, which is the shape 決定 1 makes the BSDs match.
pub fn poll_wait(pfd: i32, out: &mut [RawEvent], cap: usize, timeout_ms: i64) -> isize {
    let mut raw: [EpollEvent; crate::POLL_EVENTS_CAP] =
        [EpollEvent { events: 0, data: 0 }; crate::POLL_EVENTS_CAP];
    let want = core::cmp::min(cap, crate::POLL_EVENTS_CAP);
    // epoll's timeout is an i32 of milliseconds and spells "forever"
    // as -1, so a negative ask of any size becomes exactly that.
    let timeout: i32 = if timeout_ms < 0 {
        -1
    } else {
        timeout_ms.min(i32::MAX as i64) as i32
    };
    let n = unsafe { epoll_wait(pfd, raw.as_mut_ptr(), want as i32, timeout) };
    if n < 0 {
        return -1;
    }
    let mut len = 0usize;
    for ev in raw.iter().take(n as usize) {
        let bits = ev.events;
        let mut flags = 0u32;
        if bits & EPOLLIN != 0 {
            flags |= EVENT_READ;
        }
        if bits & EPOLLOUT != 0 {
            flags |= EVENT_WRITE;
        }
        if bits & (EPOLLHUP | EPOLLRDHUP) != 0 {
            flags |= EVENT_HUP;
        }
        let mut error = 0u32;
        if bits & EPOLLERR != 0 {
            flags |= EVENT_ERROR;
            // EPOLLERR carries no code, unlike kqueue's `data`, so the
            // errno has to be fetched from the socket itself.
            error = so_error(ev.data as i32) as u32;
        }
        out[len] = RawEvent { token: ev.data, flags, error };
        len += 1;
        if len == cap {
            break;
        }
    }
    len as isize
}

/// The pending socket error, for the `EPOLLERR` path. `token` is not
/// necessarily an fd — the caller chooses what to register — so a
/// failure here just means "no code available" rather than an error of
/// its own.
fn so_error(fd: i32) -> i32 {
    let mut err: i32 = 0;
    let mut len: u32 = core::mem::size_of::<i32>() as u32;
    let rc = unsafe {
        crate::getsockopt(
            fd,
            SOL_SOCKET,
            SO_ERROR,
            (&mut err as *mut i32).cast(),
            &mut len,
        )
    };
    if rc != 0 { 0 } else { err }
}
