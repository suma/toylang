package std.poll

# Readiness notification — one interface over epoll and kqueue
# (EVENT_POLLING.md, NETWORK_IO.md N3).
#
# A `Poller` answers "which of these descriptors can I act on now?".
# It is what lets one thread serve many connections without a thread
# each and without spinning, and it is the reason `net`'s sockets are
# non-blocking from birth: `WouldBlock` is the answer you get between
# events, and the poller is what tells you when to ask again.
#
# Four decisions from the design are visible in this API.
#
# **One event per descriptor.** kqueue reports a readable and a
# writable side as two events; the runtime merges them, so a loop
# written on macOS iterates the same number of times on Linux — and,
# more importantly, a handler that closes its descriptor cannot then be
# handed a second event for it.
#
# **Level-triggered by default.** Read what you like and come back;
# the event is still there if you left data behind. `interest_edge()`
# switches to the other discipline, where you must read until
# `WouldBlock` or the connection quietly stalls — which is among the
# hardest bugs to see.
#
# **A signal is not hidden.** `wait` interrupted by one answers
# `Err(NetError::Interrupted)` rather than retrying inside, because
# retrying would take away the only way out of the loop.
#
# **Events are read back by index.** `wait` gives a count and
# `event(i)` reads the i-th, because an extern boundary here cannot
# carry a pointer to an array. They stay valid until the next `wait`
# on this thread — a documented lifetime, not a checked one, the same
# way `Span<T>` does not check its own.
#
#     val poller = match Poller::new() { .. }
#     val ok = poller.register(listener.as_fd(), 0u64, interest_read())
#     val n = match poller.wait(1000i64) { .. }
#     var i: u64 = 0u64
#     while i < n {
#         val ev = poller.event(i)
#         if ev.is_readable() { .. }
#         i = i + 1u64
#     }

extern fn __extern_poll_create() -> i32 from "toylang_rt" as "toy_poll_create"
extern fn __extern_poll_ctl(pfd: i32, fd: i32, token: u64, interest: u32) -> u64 from "toylang_rt" as "toy_poll_ctl"
extern fn __extern_poll_wait(pfd: i32, timeout_ms: i64) -> u64 from "toylang_rt" as "toy_poll_wait"
extern fn __extern_poll_event_token(i: u64) -> u64 from "toylang_rt" as "toy_poll_event_token"
extern fn __extern_poll_event_flags(i: u64) -> u32 from "toylang_rt" as "toy_poll_event_flags"
extern fn __extern_poll_event_error(i: u64) -> u64 from "toylang_rt" as "toy_poll_event_error"
extern fn __extern_poll_error_status(errno: u64) -> u64 from "toylang_rt" as "toy_poll_error_status"

# What to watch a descriptor for. Combine with `|`.
#
# Plain `u32` values rather than a type with an overloaded `|`:
# operator overloading only lowers in let-rhs position on the compiled
# lanes (todo OP-OVERLOAD-CHAIN), so `interest_read() | interest_write()`
# inside a call would work in the interpreter and fail in AOT. Bind the
# combination with `val` first. A number that works everywhere beats a
# type that does not.
#
# **Functions rather than `const`, and literals in the bodies.** A
# top-level `const` in a module reaches nothing — not another module
# (qualified, imported or auto-loaded) and not even this module's own
# function bodies, because integration does not carry a module's
# consts across at all (todo: MODULE-CONST). These read the same at
# the call site and actually resolve.
pub fn interest_read() -> u32 {
    1u32
}

pub fn interest_write() -> u32 {
    2u32
}

# Report only what changed since the last event, instead of whatever
# is true now. Faster, and unforgiving: you must drain until
# `WouldBlock` or the descriptor goes quiet forever.
pub fn interest_edge() -> u32 {
    4u32
}

# Report once, then unregister. Saves a `deregister` in accept-once
# and connect-once flows.
pub fn interest_oneshot() -> u32 {
    8u32
}

# One ready descriptor.
#
# `token` is whatever was handed to `register` — the poller never
# interprets it, so it can be the fd itself, an index into a table of
# connections, or anything else that fits in a `u64`.
pub struct Event {
    token: u64,
    flags: u32,
    error: u32,
}

impl Event {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn is_readable(&self) -> bool {
        (self.flags & 1u32) != 0u32
    }

    pub fn is_writable(&self) -> bool {
        (self.flags & 2u32) != 0u32
    }

    # The peer closed its end. **Still readable** — what it sent
    # before closing is buffered and waiting, so drain before you act
    # on this.
    pub fn is_hup(&self) -> bool {
        (self.flags & 4u32) != 0u32
    }

    pub fn is_error(&self) -> bool {
        (self.flags & 8u32) != 0u32
    }

    # The failure behind `is_error()`. `NetError::Unknown` when the
    # platform reported an error without a code.
    pub fn error_reason(&self) -> NetError {
        val errno: u64 = self.error as u64
        val status: u64 = __extern_poll_error_status(errno)
        val err: NetError = net_error_from_status(status)
        err
    }
}

# A set of descriptors being watched.
#
# **Owns its fd**: `impl Drop` closes it at scope exit, and the move
# checker reports one handed away and then used again.
pub struct Poller {
    fd: i32,
}

impl Drop for Poller {
    fn drop(&mut self) {
        if self.fd >= 0i32 {
            val ignored: u64 = __extern_net_close(self.fd)
            self.fd = -1i32
        }
    }
}

impl Poller {
    pub fn new() -> Result<Poller, NetError> {
        val fd: i32 = __extern_poll_create()
        if fd < 0i32 {
            val status: u64 = __extern_net_status()
            val err: NetError = net_error_from_status(status)
            return Result::Err(err)
        }
        val p = Poller { fd: fd }
        Result::Ok(p)
    }

    # Watch `fd`, reporting `token` when it is ready.
    #
    # **Idempotent**: registering something already registered
    # replaces its interest. `interest == 0` stops watching, which is
    # what `deregister` spells.
    pub fn register(&self, fd: i32, token: u64, interest: u32) -> Result<(), NetError> {
        val status: u64 = __extern_poll_ctl(self.fd, fd, token, interest)
        if status == 0u64 {
            Result::Ok(())
        } else {
            val err: NetError = net_error_from_status(status)
            Result::Err(err)
        }
    }

    # Stop watching `fd`. Not an error if it was never watched — the
    # intent is "make sure this is not registered", and it is
    # satisfied either way.
    pub fn deregister(&self, fd: i32) -> Result<(), NetError> {
        val status: u64 = __extern_poll_ctl(self.fd, fd, 0u64, 0u32)
        if status == 0u64 {
            Result::Ok(())
        } else {
            val err: NetError = net_error_from_status(status)
            Result::Err(err)
        }
    }

    # Wait until at least one descriptor is ready, `timeout_ms`
    # elapses, or a signal arrives.
    #
    # `timeout_ms` of 0 polls and returns at once; negative waits
    # forever. `Ok(0)` is a timeout — nothing became ready, which is
    # not a failure. A signal is `Err(NetError::Interrupted)` and
    # calling again is correct.
    #
    # Read the events with `event(i)` for `i` in `0..n`. They stay
    # valid until this thread's next `wait`.
    pub fn wait(&self, timeout_ms: i64) -> Result<u64, NetError> {
        val n: u64 = __extern_poll_wait(self.fd, timeout_ms)
        val status: u64 = __extern_net_status()
        if status == 0u64 {
            Result::Ok(n)
        } else {
            val err: NetError = net_error_from_status(status)
            Result::Err(err)
        }
    }

    # The i-th event of the most recent `wait`. An index past the end
    # answers an empty event rather than failing — the count from
    # `wait` is the bound, and checking it twice buys nothing.
    pub fn event(&self, i: u64) -> Event {
        val token: u64 = __extern_poll_event_token(i)
        val flags: u32 = __extern_poll_event_flags(i)
        val error: u64 = __extern_poll_event_error(i)
        Event { token: token, flags: flags, error: error as u32 }
    }

    # The underlying descriptor. A poller is pollable: on both
    # backends this can be registered with another one.
    pub fn as_fd(&self) -> i32 {
        self.fd
    }

    # Close now rather than at scope exit. Idempotent.
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
