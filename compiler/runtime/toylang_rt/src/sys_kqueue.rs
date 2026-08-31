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
