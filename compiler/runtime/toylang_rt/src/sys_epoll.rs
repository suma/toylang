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
