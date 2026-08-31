# Stdlib networking (NETWORK_IO.md).
#
# **Phase N0 only.** Nothing connects yet: this module exists so the
# compile-time platform switch has an observable end. `sys_epoll.rs`
# and `sys_kqueue.rs` are selected by one `#[cfg_attr(path)] mod sys;`
# in `toylang_rt`, and until something reads across that boundary from
# toylang there is no way to tell — from a program or from a test —
# that the right half was compiled and reached.
#
# The socket types (`TcpListener` / `TcpStream` / `NetError`) arrive in
# N1, the poller in N3. Their design is in
# `design-docs/NETWORK_IO.md` and `design-docs/EVENT_POLLING.md`.

extern fn __extern_net_backend_name() -> str from "toylang_rt" as "toy_net_backend_name"

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
