# Stdlib networking (NETWORK_IO.md).
#
# **Phases N0 and N1.** The compile-time platform switch
# (`backend_name`) and the TCP *client*: connect, send, receive,
# close. The listener side (`TcpListener`) arrives in N2 and the
# poller in N3.
#
# Three decisions shape everything below.
#
# **Sockets are non-blocking from birth** (論点 1). Blocking is a
# property of the fd, not of the call, so `set_blocking(true)` is how
# a program asks for the other mode — and the default is the one that
# cannot wedge a single-threaded program. The consequence is that
# `NetError::WouldBlock` and `NetError::InProgress` are *ordinary
# answers*, not failures: an event loop sees them constantly.
#
# **The caller owns the bytes.** `read` / `write` take a `Span<u8>`
# and return a count; nothing here or in the runtime allocates, and
# the bytes are never staged through an intermediate buffer — the
# syscall reads and writes toylang memory directly, on every backend
# (NETWORK_IO.md §5). `recv() -> Vec<u8>` would have been the friendlier
# signature and was rejected for allocating on every call.
#
# **toylang never sees a `sockaddr`.** Addresses cross as `str` plus a
# port; the runtime's `sys` layer builds the struct, which is what
# keeps macOS's `sin_len` and Linux's two-byte `sin_family` from
# reaching this file.
#
# A round trip, with the buffer allocated once and reused:
#
#     var s = TcpStream::connect("127.0.0.1", port)?
#     val msg = String::from_str("ping")
#     val sent = s.write(msg.as_span())?
#     var buf: Vec<u8> = Vec::with_capacity(1024u64)
#     match buf.capacity_span() {
#         Option::Some(room) => {
#             val n = s.read(room)?
#             buf.set_size(n)
#         }
#         Option::None => { }
#     }

extern fn __extern_net_backend_name() -> str from "toylang_rt" as "toy_net_backend_name"
extern fn __extern_net_socket() -> i32 from "toylang_rt" as "toy_net_socket"
extern fn __extern_net_connect(fd: i32, addr: str, port: u64) -> u64 from "toylang_rt" as "toy_net_connect"
extern fn __extern_net_send(fd: i32, buf: ptr, len: u64) -> u64 from "toylang_rt" as "toy_net_send"
extern fn __extern_net_recv(fd: i32, buf: ptr, len: u64) -> u64 from "toylang_rt" as "toy_net_recv"
extern fn __extern_net_close(fd: i32) -> u64 from "toylang_rt" as "toy_net_close"
extern fn __extern_net_set_blocking(fd: i32, on: bool) -> u64 from "toylang_rt" as "toy_net_set_blocking"
extern fn __extern_net_take_error(fd: i32) -> u64 from "toylang_rt" as "toy_net_take_error"
extern fn __extern_net_shutdown_write(fd: i32) -> u64 from "toylang_rt" as "toy_net_shutdown_write"
extern fn __extern_net_status() -> u64 from "toylang_rt" as "toy_net_status"
extern fn __extern_net_bind(addr: str, port: u64, backlog: i32) -> i32 from "toylang_rt" as "toy_net_bind"
extern fn __extern_net_local_port(fd: i32) -> u64 from "toylang_rt" as "toy_net_local_port"
extern fn __extern_net_accept(fd: i32) -> i32 from "toylang_rt" as "toy_net_accept"

# Which event-notification backend this build uses: `"epoll"` on
# Linux, `"kqueue"` on macOS and the other BSDs.
#
# The value is fixed when the runtime is compiled, not looked up at
# run time — the two are never both present. Programs should not
# branch on it (the whole point of the layer above is that they do not
# have to); it is here so the choice is *visible*, which is what makes
# "the switch works" a thing a test can assert rather than a thing to
# hope for.
pub fn backend_name() -> str {
    __extern_net_backend_name()
}

# What a socket call can answer with instead of a result.
#
# A separate enum from `IoError` on purpose: `WouldBlock` and
# `InProgress` are the normal course of events on a non-blocking
# socket, and folding them into the file-I/O vocabulary would give
# every `io::read_file` match two arms that cannot happen.
pub enum NetError {
    # Not a failure — nothing is ready yet. Wait and ask again.
    WouldBlock,
    # Not a failure either — a non-blocking `connect` is under way.
    # Wait for writability, then call `take_error`.
    InProgress,
    # A signal interrupted the call. Retrying is correct.
    Interrupted,
    ConnectionRefused,
    ConnectionReset,
    ConnectionAborted,
    BrokenPipe,
    NotConnected,
    AddrInUse,
    AddrNotAvailable,
    NetworkUnreachable,
    HostUnreachable,
    TimedOut,
    TooManyOpenFiles,
    InvalidInput,
    Unknown,
}

impl Display for NetError {
    fn to_str(&self) -> str {
        match self {
            NetError::WouldBlock => "would block",
            NetError::InProgress => "in progress",
            NetError::Interrupted => "interrupted",
            NetError::ConnectionRefused => "connection refused",
            NetError::ConnectionReset => "connection reset",
            NetError::ConnectionAborted => "connection aborted",
            NetError::BrokenPipe => "broken pipe",
            NetError::NotConnected => "not connected",
            NetError::AddrInUse => "address in use",
            NetError::AddrNotAvailable => "address not available",
            NetError::NetworkUnreachable => "network unreachable",
            NetError::HostUnreachable => "host unreachable",
            NetError::TimedOut => "timed out",
            NetError::TooManyOpenFiles => "too many open files",
            NetError::InvalidInput => "invalid input",
            NetError::Unknown => "unknown error",
        }
    }
}

# Map the runtime's status code to a variant.
#
# The codes are OS-independent by construction — the errno values
# behind them are not (EAGAIN is 11 on Linux and 35 on the BSDs), so
# the translation from errno happens in the runtime's per-platform
# `sys` layer and this side only ever sees the settled vocabulary.
fn net_error_from_status(status: u64) -> NetError {
    if status == 1u64 { NetError::WouldBlock }
    elif status == 2u64 { NetError::InProgress }
    elif status == 3u64 { NetError::Interrupted }
    elif status == 4u64 { NetError::ConnectionRefused }
    elif status == 5u64 { NetError::ConnectionReset }
    elif status == 6u64 { NetError::ConnectionAborted }
    elif status == 7u64 { NetError::BrokenPipe }
    elif status == 8u64 { NetError::NotConnected }
    elif status == 9u64 { NetError::AddrInUse }
    elif status == 10u64 { NetError::AddrNotAvailable }
    elif status == 11u64 { NetError::NetworkUnreachable }
    elif status == 12u64 { NetError::HostUnreachable }
    elif status == 13u64 { NetError::TimedOut }
    elif status == 14u64 { NetError::TooManyOpenFiles }
    elif status == 15u64 { NetError::InvalidInput }
    else { NetError::Unknown }
}

# A listening TCP socket (N2).
#
# **Owns its fd**, the same way `TcpStream` does: `impl Drop` closes
# it at scope exit and the move checker (E0014) reports a listener
# that was handed away and then used again.
#
# The listener is non-blocking, so `accept` normally answers
# `Err(NetError::WouldBlock)` — that is "no connection is pending yet",
# the ordinary state of an idle server, not a failure. Call
# `set_blocking(true)` to wait instead.
pub struct TcpListener {
    fd: i32,
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        if self.fd >= 0i32 {
            val ignored: u64 = __extern_net_close(self.fd)
            self.fd = -1i32
        }
    }
}

impl TcpListener {
    # Bind to `addr:port` and start listening.
    #
    # **`port = 0` asks the OS for a free port**; read it back with
    # `local_port`. That is how a server binds without naming a
    # number, and the only way several can run at once — which is
    # what makes a test of one deterministic.
    #
    # `SO_REUSEADDR` is set before the bind, so a listener that has
    # just closed does not hold the address through TIME_WAIT.
    pub fn bind(addr: str, port: u64) -> Result<TcpListener, NetError> {
        val fd: i32 = __extern_net_bind(addr, port, 128i32)
        if fd < 0i32 {
            val status: u64 = __extern_net_status()
            val err: NetError = net_error_from_status(status)
            return Result::Err(err)
        }
        val l = TcpListener { fd: fd }
        Result::Ok(l)
    }

    # The port this listener actually bound to — the one the OS chose
    # when `bind` was asked for 0.
    pub fn local_port(&self) -> Result<u64, NetError> {
        val port: u64 = __extern_net_local_port(self.fd)
        val status: u64 = __extern_net_status()
        if status == 0u64 {
            Result::Ok(port)
        } else {
            val err: NetError = net_error_from_status(status)
            Result::Err(err)
        }
    }

    # Take a pending connection.
    #
    # `Err(NetError::WouldBlock)` means none is waiting — the normal
    # answer while a non-blocking server idles. The stream that comes
    # back is non-blocking too, whatever this listener's mode is.
    pub fn accept(&self) -> Result<TcpStream, NetError> {
        val fd: i32 = __extern_net_accept(self.fd)
        if fd < 0i32 {
            val status: u64 = __extern_net_status()
            val err: NetError = net_error_from_status(status)
            return Result::Err(err)
        }
        val s = TcpStream { fd: fd }
        Result::Ok(s)
    }

    # Switch between blocking and non-blocking. With blocking on,
    # `accept` waits for a connection instead of answering
    # `WouldBlock`.
    pub fn set_blocking(&self, on: bool) -> Result<(), NetError> {
        val status: u64 = __extern_net_set_blocking(self.fd, on)
        if status == 0u64 {
            Result::Ok(())
        } else {
            val err: NetError = net_error_from_status(status)
            Result::Err(err)
        }
    }

    # The underlying descriptor, for registering with a poller (N3).
    pub fn as_fd(&self) -> i32 {
        self.fd
    }

    # Close now rather than at scope exit. Idempotent, like
    # `TcpStream::close`.
    pub fn close(&mut self) -> Result<(), NetError> {
        if self.fd < 0i32 {
            return Result::Ok(())
        }
        val status: u64 = __extern_net_close(self.fd)
        self.fd = -1i32
        if status == 0u64 {
            Result::Ok(())
        } else {
            val err: NetError = net_error_from_status(status)
            Result::Err(err)
        }
    }
}

# A connected TCP socket.
#
# **Owns its fd.** `impl Drop` closes it at scope exit, which also
# means the move checker (E0014) reports a stream that was handed
# away and then used again, rather than letting it become a read on a
# closed descriptor.
#
# `close` parks the field at `-1` so a second close is a no-op rather
# than a close of whatever unrelated file has since been handed that
# number (論点 2) — the failure mode this avoids is silent and
# extremely hard to trace.
pub struct TcpStream {
    fd: i32,
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        if self.fd >= 0i32 {
            val ignored: u64 = __extern_net_close(self.fd)
            self.fd = -1i32
        }
    }
}

impl TcpStream {
    # Connect to `addr:port` and **wait for the handshake**, where
    # `addr` is a numeric IPv4 address (`"127.0.0.1"`). Name
    # resolution is N5; until then a hostname is
    # `Err(NetError::InvalidInput)`.
    #
    # The returned stream is in blocking mode — `read` and `write`
    # wait rather than answering `WouldBlock`. Call
    # `set_blocking(false)` to switch it back.
    #
    # This is the form a client wants and the only one that is usable
    # before the poller (N3) exists: a non-blocking connect answers
    # "in progress" and needs something to tell it when the socket
    # became writable. `connect_nonblocking` below is that form.
    pub fn connect(addr: str, port: u64) -> Result<TcpStream, NetError> {
        val fd: i32 = __extern_net_socket()
        if fd < 0i32 {
            val status: u64 = __extern_net_status()
            val err: NetError = net_error_from_status(status)
            return Result::Err(err)
        }
        # Sockets are born non-blocking (論点 1), so blocking is what
        # has to be asked for — and it has to be asked for *before*
        # `connect`, which is the call being made to wait.
        val mode: u64 = __extern_net_set_blocking(fd, true)
        if mode != 0u64 {
            val ignored: u64 = __extern_net_close(fd)
            val err: NetError = net_error_from_status(mode)
            return Result::Err(err)
        }
        val status: u64 = __extern_net_connect(fd, addr, port)
        if status == 0u64 {
            val s = TcpStream { fd: fd }
            return Result::Ok(s)
        }
        # A socket that never connected still owns a descriptor.
        # Closing it here is what keeps a retry loop from running the
        # process out of file descriptors.
        val ignored: u64 = __extern_net_close(fd)
        val err: NetError = net_error_from_status(status)
        Result::Err(err)
    }

    # Start connecting and come back at once, with the socket left
    # non-blocking.
    #
    # **`Ok` does not mean connected.** The handshake is usually still
    # under way; wait for the socket to become writable (N3) and then
    # ask `take_error`, which answers `Ok(())` once it is up and
    # names the failure otherwise. `Err` here is reserved for the
    # failures that are known immediately — a malformed address, no
    # descriptors left.
    #
    # NETWORK_IO.md described this as `connect` answering
    # `Err(NetError::InProgress)`, which cannot work: the error would
    # carry away the only reference to the socket that is connecting,
    # leaving nothing to call `take_error` on. The stream comes back
    # either way, and its state is a question you ask it.
    pub fn connect_nonblocking(addr: str, port: u64) -> Result<TcpStream, NetError> {
        val fd: i32 = __extern_net_socket()
        if fd < 0i32 {
            val status: u64 = __extern_net_status()
            val err: NetError = net_error_from_status(status)
            return Result::Err(err)
        }
        val status: u64 = __extern_net_connect(fd, addr, port)
        # 0 = connected already (common on loopback), 2 = in progress.
        # Everything else failed outright and the descriptor goes back.
        if status == 0u64 || status == 2u64 {
            val s = TcpStream { fd: fd }
            return Result::Ok(s)
        }
        val ignored: u64 = __extern_net_close(fd)
        val err: NetError = net_error_from_status(status)
        Result::Err(err)
    }

    # Fill `buf` from the connection, answering how many bytes landed
    # in it.
    #
    # **`Ok(0)` means the peer closed** — that is end of stream, not
    # an error, and it is why the count and the error are separate. A
    # short read is normal and is not a failure either.
    pub fn read(&self, buf: Span<u8>) -> Result<u64, NetError> {
        val n: u64 = __extern_net_recv(self.fd, buf.as_raw(), buf.len())
        val status: u64 = __extern_net_status()
        if status == 0u64 {
            Result::Ok(n)
        } else {
            val err: NetError = net_error_from_status(status)
            Result::Err(err)
        }
    }

    # Send `buf`'s bytes, answering how many the kernel took.
    #
    # **A short write is normal**, not an error: compare the count
    # against `buf.len()` and send the rest from a `Span::slice` of
    # what is left. Nothing is copied on the way out.
    pub fn write(&self, buf: Span<u8>) -> Result<u64, NetError> {
        val n: u64 = __extern_net_send(self.fd, buf.as_raw(), buf.len())
        val status: u64 = __extern_net_status()
        if status == 0u64 {
            Result::Ok(n)
        } else {
            val err: NetError = net_error_from_status(status)
            Result::Err(err)
        }
    }

    # Read and clear the socket's pending error — how a non-blocking
    # `connect` reports its outcome once the socket becomes writable.
    # `Ok(())` means the connection is up.
    pub fn take_error(&self) -> Result<(), NetError> {
        val status: u64 = __extern_net_take_error(self.fd)
        if status == 0u64 {
            Result::Ok(())
        } else {
            val err: NetError = net_error_from_status(status)
            Result::Err(err)
        }
    }

    # Half-close the write side. The peer's next read answers 0, which
    # is how a request ends without giving up the descriptor the reply
    # arrives on.
    pub fn shutdown_write(&self) -> Result<(), NetError> {
        val status: u64 = __extern_net_shutdown_write(self.fd)
        if status == 0u64 {
            Result::Ok(())
        } else {
            val err: NetError = net_error_from_status(status)
            Result::Err(err)
        }
    }

    # Switch between blocking and non-blocking. New sockets are
    # non-blocking; `set_blocking(true)` makes `read` / `write` wait
    # instead of answering `WouldBlock`.
    pub fn set_blocking(&self, on: bool) -> Result<(), NetError> {
        val status: u64 = __extern_net_set_blocking(self.fd, on)
        if status == 0u64 {
            Result::Ok(())
        } else {
            val err: NetError = net_error_from_status(status)
            Result::Err(err)
        }
    }

    # The underlying descriptor, for registering with a poller (N3).
    pub fn as_fd(&self) -> i32 {
        self.fd
    }

    # Close now rather than at scope exit. Idempotent: the field is
    # parked at `-1`, so the `Drop` that follows does nothing.
    pub fn close(&mut self) -> Result<(), NetError> {
        if self.fd < 0i32 {
            return Result::Ok(())
        }
        val status: u64 = __extern_net_close(self.fd)
        self.fd = -1i32
        if status == 0u64 {
            Result::Ok(())
        } else {
            val err: NetError = net_error_from_status(status)
            Result::Err(err)
        }
    }
}
