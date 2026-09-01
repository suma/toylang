//! NETWORK_IO.md N0: the platform switch, seen from toylang.
//!
//! `toylang_rt` picks epoll or kqueue at compile time with one
//! `#[cfg_attr(path)] mod sys;`, and `net::backend_name()` is the only
//! thing that crosses back. That makes it the observable N0 is
//! accepted on: the four lanes each reach the runtime differently —
//! the tree-walker through its extern registry, the JIT through a
//! symbol-table entry, the AOT binary through the linker — and a
//! switch that resolved differently in any of them would show up here
//! rather than in whatever is built on top later.
//!
//! The *value* is host-dependent, so it is not pinned; that the lanes
//! agree, and that the answer is one of the two backends rather than
//! an empty string from a missing symbol, is.

use super::harness::{assert_consistent, interpreter_value};

#[test]
fn every_lane_reports_the_same_event_backend() {
    let src = r#"
        fn main() -> u64 {
            val name: str = net::backend_name()
            if name == "kqueue" {
                2u64
            } elif name == "epoll" {
                1u64
            } else {
                0u64
            }
        }
    "#;
    // 0 would mean the symbol resolved to something that is neither —
    // an empty string, most likely, which is what a missing runtime
    // entry point looks like from here.
    let name = interpreter_value(src) & 0xff;
    assert!(
        name == 1 || name == 2,
        "backend_name() answered neither `epoll` nor `kqueue`"
    );
    let expected = if cfg!(target_os = "linux") { 1 } else { 2 };
    assert_eq!(name, expected, "the switch selected the other platform's backend");
    assert_consistent(src, "net_backend_name");
}

// --- N1: the TCP client, on every lane -----------------------------
//
// The peer is a `std::net` echo server on a thread of the test
// process, bound to 127.0.0.1 on port 0 so the OS picks the port —
// the determinism rule from NETWORK_IO.md: loopback only, never a
// fixed port (the suite runs in parallel), and nothing pinned but
// what happened.
//
// The port reaches the program as a **literal in its source**, the
// way `extern_buf.rs` embeds a file path. That is what makes these
// four-lane: `assert_consistent` spawns an AOT binary, and there is
// no channel for arguments, but there is one for the text.
//
// Each lane runs the program, so each takes a connection. The server
// accepts until the process ends rather than counting them — a lane
// that is skipped (no `cc`, say) would otherwise leave the next lane
// waiting on a server that had stopped.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;

/// An echo server on an ephemeral loopback port. Returns the port.
fn spawn_echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local_addr").port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        tx.send(()).ok();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if stream.write_all(&buf[..n]).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    // The listener is bound before the thread starts, so a connect
    // cannot lose the race; this only waits for the thread to exist.
    rx.recv().expect("server thread started");
    port
}

/// Connect, send four bytes, read the echo. The buffer is allocated
/// once with `Vec::with_capacity` and filled in place by `recv` — the
/// no-allocation shape NETWORK_IO.md §5 is about, and the one where a
/// lane that copied through a staging buffer would still pass while a
/// lane that lost the bytes would not.
#[test]
fn every_lane_completes_a_tcp_round_trip() {
    let port = spawn_echo_server();
    let src = format!(
        r#"
        fn main() -> u64 {{
            val conn = TcpStream::connect("127.0.0.1", {port}u64)
            var st = match conn {{
                Result::Ok(s) => s,
                Result::Err(e) => {{ return 90u64 }}
            }}

            val msg = String::from_str("ping")
            val window = msg.as_span()
            val out: Span<u8> = match window {{
                Option::Some(w) => w,
                Option::None => {{ return 91u64 }}
            }}
            val wrote = st.write(out)
            val sent = match wrote {{
                Result::Ok(n) => n,
                Result::Err(e) => {{ return 92u64 }}
            }}

            var buf: Vec<u8> = Vec::with_capacity(64u64)
            val room = buf.capacity_span()
            var got: u64 = 0u64
            match room {{
                Option::Some(w) => {{
                    val r = st.read(w)
                    match r {{
                        Result::Ok(n) => {{ got = n }}
                        Result::Err(e) => {{ got = 0u64 }}
                    }}
                }}
                Option::None => {{ got = 0u64 }}
            }}
            buf.set_size(got)

            # Sum the bytes rather than count them, so a lane that
            # handed the syscall the wrong memory — the EXTERN-BUF
            # failure, which returned a right-length buffer of zeros —
            # gives a wrong answer instead of a right one.
            var sum: u64 = 0u64
            var i: u64 = 0u64
            while i < buf.size() {{
                sum = sum + buf.get(i) as u64
                i = i + 1u64
            }}
            sent + got + sum
        }}
    "#
    );
    // 4 sent + 4 received + 'p' + 'i' + 'n' + 'g' (430) = 438.
    assert_eq!(interpreter_value(&src) & 0xffff, 438);
    assert_consistent(&src, "net_round_trip");
}

/// A payload no `str` could carry on the tree-walker — a NUL, a lone
/// 0xFF — which is why `read` / `write` take a `Span<u8>`. A
/// `str`-shaped API would have split the lanes on exactly the
/// payloads a network carries.
#[test]
fn every_lane_carries_bytes_that_are_not_utf8() {
    let port = spawn_echo_server();
    let src = format!(
        r#"
        fn main() -> u64 {{
            val conn = TcpStream::connect("127.0.0.1", {port}u64)
            var st = match conn {{
                Result::Ok(s) => s,
                Result::Err(e) => {{ return 90u64 }}
            }}

            var out: Vec<u8> = Vec::with_capacity(3u64)
            val space = out.capacity_span()
            match space {{
                Option::Some(w) => {{
                    w.set(0u64, 0u8)
                    w.set(1u64, 255u8)
                    w.set(2u64, 128u8)
                }}
                Option::None => {{ }}
            }}
            out.set_size(3u64)
            val body = out.as_span()
            val payload: Span<u8> = match body {{
                Option::Some(w) => w,
                Option::None => {{ return 91u64 }}
            }}
            val wrote = st.write(payload)
            match wrote {{
                Result::Ok(n) => {{ }}
                Result::Err(e) => {{ return 92u64 }}
            }}

            var back: Vec<u8> = Vec::with_capacity(16u64)
            val room = back.capacity_span()
            var got: u64 = 0u64
            match room {{
                Option::Some(w) => {{
                    val r = st.read(w)
                    match r {{
                        Result::Ok(n) => {{ got = n }}
                        Result::Err(e) => {{ got = 0u64 }}
                    }}
                }}
                Option::None => {{ got = 0u64 }}
            }}
            back.set_size(got)
            var sum: u64 = 0u64
            var i: u64 = 0u64
            while i < back.size() {{
                sum = sum + back.get(i) as u64
                i = i + 1u64
            }}
            got + sum
        }}
    "#
    );
    // 3 bytes back, 0 + 255 + 128.
    assert_eq!(interpreter_value(&src) & 0xffff, 386);
    assert_consistent(&src, "net_non_utf8");
}

/// Nothing is listening, so the connect is refused — by name, not as
/// a generic failure. The port comes from a listener closed before the
/// program runs, which is the only way to name one that is free.
#[test]
fn every_lane_names_a_refused_connection() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        l.local_addr().expect("local_addr").port()
    };
    let src = format!(
        r#"
        fn main() -> u64 {{
            val conn = TcpStream::connect("127.0.0.1", {port}u64)
            match conn {{
                Result::Ok(s) => 1u64,
                Result::Err(e) => {{
                    match e {{
                        NetError::ConnectionRefused => 2u64,
                        _ => 3u64,
                    }}
                }}
            }}
        }}
    "#
    );
    // 1 = it connected to something, 3 = some other errno.
    assert_eq!(interpreter_value(&src) & 0xff, 2);
    assert_consistent(&src, "net_refused");
}

// --- N2: the TCP server ---------------------------------------------
//
// These need no peer at all. One program is both listener and client,
// which is what makes them fully deterministic: no thread, no external
// process, nothing to synchronise with. `bind` asks for port 0 and
// `local_port` reads back what the OS chose, so nothing is hardcoded
// and any number of these can run at once.

/// Bind, connect to yourself, accept, and carry bytes across — every
/// lane, in one process.
#[test]
fn every_lane_serves_itself_over_a_loopback_socket() {
    let src = r#"
        fn main() -> u64 {
            val bound = TcpListener::bind("127.0.0.1", 0u64)
            var l = match bound {
                Result::Ok(x) => x,
                Result::Err(e) => { return 80u64 }
            }
            val p = l.local_port()
            val port = match p {
                Result::Ok(n) => n,
                Result::Err(e) => { return 81u64 }
            }
            if port == 0u64 { return 82u64 }

            # Nothing has connected, so a non-blocking accept says so.
            # `WouldBlock` is the idle state of a server, not an error,
            # and a lane that reported it as something else would be
            # unable to run an event loop at all.
            val idle = l.accept()
            val idle_ok = match idle {
                Result::Ok(s) => 0u64,
                Result::Err(e) => match e { NetError::WouldBlock => 1u64, _ => 0u64 },
            }

            val conn = TcpStream::connect("127.0.0.1", port)
            var client = match conn {
                Result::Ok(s) => s,
                Result::Err(e) => { return 83u64 }
            }

            # The handshake is done, but the connection reaching the
            # listener's queue is the kernel's business: a non-blocking
            # accept can still say WouldBlock for a moment. Wait rather
            # than race.
            val listener_blocking = l.set_blocking(true)
            val a = l.accept()
            var server = match a {
                Result::Ok(s) => s,
                Result::Err(e) => { return 84u64 }
            }

            val msg = String::from_str("hey")
            val window = msg.as_span()
            val out: Span<u8> = match window {
                Option::Some(w) => w,
                Option::None => { return 85u64 }
            }
            val wrote = client.write(out)
            match wrote {
                Result::Ok(n) => { }
                Result::Err(e) => { return 86u64 }
            }

            val server_blocking = server.set_blocking(true)
            var buf: Vec<u8> = Vec::with_capacity(32u64)
            val room = buf.capacity_span()
            var got: u64 = 0u64
            match room {
                Option::Some(w) => {
                    val r = server.read(w)
                    match r {
                        Result::Ok(n) => { got = n }
                        Result::Err(e) => { got = 0u64 }
                    }
                }
                Option::None => { got = 0u64 }
            }
            buf.set_size(got)
            var sum: u64 = 0u64
            var i: u64 = 0u64
            while i < buf.size() {
                sum = sum + buf.get(i) as u64
                i = i + 1u64
            }
            idle_ok + got + sum
        }
    "#;
    // 1 (idle accept blocked) + 3 bytes + 'h' + 'e' + 'y' (326).
    assert_eq!(interpreter_value(src) & 0xffff, 330);
    assert_consistent(src, "net_self_serve");
}

/// Two listeners, both asking for port 0, get two different ports —
/// which is the property that lets these tests run in parallel and
/// the reason nothing here names a number.
#[test]
fn every_lane_gets_a_distinct_ephemeral_port() {
    let src = r#"
        fn main() -> u64 {
            val first = TcpListener::bind("127.0.0.1", 0u64)
            var a = match first {
                Result::Ok(x) => x,
                Result::Err(e) => { return 80u64 }
            }
            val second = TcpListener::bind("127.0.0.1", 0u64)
            var b = match second {
                Result::Ok(x) => x,
                Result::Err(e) => { return 81u64 }
            }
            val pa = a.local_port()
            val port_a = match pa { Result::Ok(n) => n, Result::Err(e) => { return 82u64 } }
            val pb = b.local_port()
            val port_b = match pb { Result::Ok(n) => n, Result::Err(e) => { return 83u64 } }
            if port_a == 0u64 { return 84u64 }
            if port_b == 0u64 { return 85u64 }
            if port_a == port_b { return 86u64 }
            7u64
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 7);
    assert_consistent(src, "net_ephemeral_ports");
}

/// A closed listener refuses the connections it used to take, and
/// `close` is idempotent here too.
#[test]
fn every_lane_stops_serving_once_the_listener_closes() {
    let src = r#"
        fn main() -> u64 {
            val bound = TcpListener::bind("127.0.0.1", 0u64)
            var l = match bound {
                Result::Ok(x) => x,
                Result::Err(e) => { return 80u64 }
            }
            val p = l.local_port()
            val port = match p { Result::Ok(n) => n, Result::Err(e) => { return 81u64 } }

            val first = l.close()
            val a = match first { Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }
            val second = l.close()
            val b = match second { Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }
            if l.as_fd() >= 0i32 { return 82u64 }

            # The port is free again, so connecting to it is refused
            # rather than accepted by a listener that should be gone.
            val conn = TcpStream::connect("127.0.0.1", port)
            val refused = match conn {
                Result::Ok(s) => 0u64,
                Result::Err(e) => match e { NetError::ConnectionRefused => 4u64, _ => 0u64 },
            }
            a + b + refused
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 6);
    assert_consistent(src, "net_listener_closed");
}

// --- N3: the poller -------------------------------------------------
//
// Also self-contained: one program is listener, client and event loop.
// What the tests pin is the *unified* behaviour — one event per ready
// descriptor, a timeout that is not a failure, a token the runtime
// never interprets — because that is what a program written on one
// platform relies on when it runs on the other, and the two backends
// underneath do not agree about any of it on their own.

/// A whole loop: watch a listener, see it become readable, accept,
/// watch the connection instead, and read what arrives.
#[test]
fn every_lane_runs_an_event_loop() {
    let src = r#"
        fn main() -> u64 {
            val bound = TcpListener::bind("127.0.0.1", 0u64)
            var l = match bound {
                Result::Ok(x) => x,
                Result::Err(e) => { return 80u64 }
            }
            val p = l.local_port()
            val port = match p { Result::Ok(n) => n, Result::Err(e) => { return 81u64 } }

            val made = Poller::new()
            var poller = match made {
                Result::Ok(x) => x,
                Result::Err(e) => { return 82u64 }
            }
            val reg = poller.register(l.as_fd(), 7u64, interest_read())
            match reg {
                Result::Ok(_) => { }
                Result::Err(e) => { return 83u64 }
            }

            # Nothing has connected, so a zero timeout answers 0. That
            # is not a failure — a loop that treated it as one would
            # exit the first time it was idle.
            val idle = poller.wait(0i64)
            val idle_n = match idle {
                Result::Ok(n) => n,
                Result::Err(e) => { return 84u64 }
            }
            if idle_n != 0u64 { return 85u64 }

            val conn = TcpStream::connect("127.0.0.1", port)
            var client = match conn {
                Result::Ok(s) => s,
                Result::Err(e) => { return 86u64 }
            }

            val ready = poller.wait(5000i64)
            val n = match ready {
                Result::Ok(k) => k,
                Result::Err(e) => { return 87u64 }
            }
            if n != 1u64 { return 88u64 }
            val ev = poller.event(0u64)
            if ev.token() != 7u64 { return 89u64 }
            if !ev.is_readable() { return 90u64 }

            val listener_blocking = l.set_blocking(true)
            val a = l.accept()
            var server = match a {
                Result::Ok(s) => s,
                Result::Err(e) => { return 91u64 }
            }

            # Swap what is watched: the connection in, the listener
            # out. `register` on something new and `deregister` on
            # something registered are the two halves epoll splits into
            # ADD / DEL and kqueue into EV_ADD / EV_DELETE.
            val reg2 = poller.register(server.as_fd(), 9u64, interest_read())
            match reg2 {
                Result::Ok(_) => { }
                Result::Err(e) => { return 92u64 }
            }
            val dereg = poller.deregister(l.as_fd())
            match dereg {
                Result::Ok(_) => { }
                Result::Err(e) => { return 93u64 }
            }

            val msg = String::from_str("hi")
            val window = msg.as_span()
            val out: Span<u8> = match window {
                Option::Some(w) => w,
                Option::None => { return 94u64 }
            }
            val wrote = client.write(out)
            match wrote {
                Result::Ok(k) => { }
                Result::Err(e) => { return 95u64 }
            }

            val ready2 = poller.wait(5000i64)
            val n2 = match ready2 {
                Result::Ok(k) => k,
                Result::Err(e) => { return 96u64 }
            }
            # Exactly one event, and for the connection — the listener
            # was deregistered, so a backend that kept reporting it
            # would answer 2 here.
            if n2 != 1u64 { return 97u64 }
            val ev2 = poller.event(0u64)
            if ev2.token() != 9u64 { return 98u64 }

            var buf: Vec<u8> = Vec::with_capacity(32u64)
            val room = buf.capacity_span()
            var got: u64 = 0u64
            match room {
                Option::Some(w) => {
                    val r = server.read(w)
                    match r {
                        Result::Ok(k) => { got = k }
                        Result::Err(e) => { got = 0u64 }
                    }
                }
                Option::None => { got = 0u64 }
            }
            buf.set_size(got)
            var sum: u64 = 0u64
            var i: u64 = 0u64
            while i < buf.size() {
                sum = sum + buf.get(i) as u64
                i = i + 1u64
            }
            got + sum
        }
    "#;
    // 2 bytes, 'h' + 'i' = 209.
    assert_eq!(interpreter_value(src) & 0xffff, 211);
    assert_consistent(src, "net_event_loop");
}

/// A descriptor that is both readable and writable is **one** event
/// with both flags, not two events (EVENT_POLLING.md 決定 1).
///
/// This is the decision that costs the BSD backend a merge pass, and
/// it is worth it twice over: a loop iterates the same number of times
/// on either platform, and a handler that closes its descriptor cannot
/// then be handed a second event for it.
#[test]
fn a_ready_descriptor_is_one_event_with_merged_flags() {
    let src = r#"
        fn main() -> u64 {
            val bound = TcpListener::bind("127.0.0.1", 0u64)
            var l = match bound {
                Result::Ok(x) => x,
                Result::Err(e) => { return 80u64 }
            }
            val p = l.local_port()
            val port = match p { Result::Ok(n) => n, Result::Err(e) => { return 81u64 } }
            val conn = TcpStream::connect("127.0.0.1", port)
            var client = match conn {
                Result::Ok(s) => s,
                Result::Err(e) => { return 82u64 }
            }
            val listener_blocking = l.set_blocking(true)
            val a = l.accept()
            var server = match a {
                Result::Ok(s) => s,
                Result::Err(e) => { return 83u64 }
            }

            val msg = String::from_str("x")
            val window = msg.as_span()
            val out: Span<u8> = match window {
                Option::Some(w) => w,
                Option::None => { return 84u64 }
            }
            val wrote = client.write(out)
            match wrote {
                Result::Ok(k) => { }
                Result::Err(e) => { return 85u64 }
            }

            val made = Poller::new()
            var poller = match made {
                Result::Ok(x) => x,
                Result::Err(e) => { return 86u64 }
            }
            # Watch the read side alone first and wait for it. A socket
            # is writable from the moment it exists, so asking for both
            # at once would let `wait` return before the byte had
            # arrived — and then there would be nothing to merge. This
            # step is what makes the assertion below about merging
            # rather than about timing.
            val warmup = poller.register(server.as_fd(), 5u64, interest_read())
            match warmup {
                Result::Ok(_) => { }
                Result::Err(e) => { return 87u64 }
            }
            val arrived = poller.wait(5000i64)
            match arrived {
                Result::Ok(k) => { }
                Result::Err(e) => { return 88u64 }
            }

            # Now ask for both sides. `register` on an already-watched
            # descriptor replaces its interest, which is epoll's
            # ADD/MOD split hidden.
            val both: u32 = interest_read() | interest_write()
            val reg = poller.register(server.as_fd(), 5u64, both)
            match reg {
                Result::Ok(_) => { }
                Result::Err(e) => { return 94u64 }
            }
            val ready = poller.wait(5000i64)
            val n = match ready {
                Result::Ok(k) => k,
                Result::Err(e) => { return 95u64 }
            }
            # One event, not two — this is the whole assertion.
            if n != 1u64 { return 89u64 }
            val ev = poller.event(0u64)
            if ev.token() != 5u64 { return 90u64 }
            if !ev.is_readable() { return 91u64 }
            if !ev.is_writable() { return 92u64 }
            if ev.is_error() { return 93u64 }
            7u64
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 7);
    assert_consistent(src, "net_merged_event");
}

/// A peer that closed shows up as HUP — and the bytes it sent before
/// closing are still there to read. A loop that acted on the hangup
/// without draining would lose the last message of every connection.
#[test]
fn a_closed_peer_is_a_hup_that_still_has_bytes() {
    let src = r#"
        fn main() -> u64 {
            val bound = TcpListener::bind("127.0.0.1", 0u64)
            var l = match bound {
                Result::Ok(x) => x,
                Result::Err(e) => { return 80u64 }
            }
            val p = l.local_port()
            val port = match p { Result::Ok(n) => n, Result::Err(e) => { return 81u64 } }
            val conn = TcpStream::connect("127.0.0.1", port)
            var client = match conn {
                Result::Ok(s) => s,
                Result::Err(e) => { return 82u64 }
            }
            val listener_blocking = l.set_blocking(true)
            val a = l.accept()
            var server = match a {
                Result::Ok(s) => s,
                Result::Err(e) => { return 83u64 }
            }

            val msg = String::from_str("bye")
            val window = msg.as_span()
            val out: Span<u8> = match window {
                Option::Some(w) => w,
                Option::None => { return 84u64 }
            }
            val wrote = client.write(out)
            match wrote {
                Result::Ok(k) => { }
                Result::Err(e) => { return 85u64 }
            }
            val closed = client.close()
            match closed {
                Result::Ok(_) => { }
                Result::Err(e) => { return 86u64 }
            }

            val made = Poller::new()
            var poller = match made {
                Result::Ok(x) => x,
                Result::Err(e) => { return 87u64 }
            }
            val reg = poller.register(server.as_fd(), 3u64, interest_read())
            match reg {
                Result::Ok(_) => { }
                Result::Err(e) => { return 88u64 }
            }
            val ready = poller.wait(5000i64)
            val n = match ready {
                Result::Ok(k) => k,
                Result::Err(e) => { return 89u64 }
            }
            if n != 1u64 { return 90u64 }
            val ev = poller.event(0u64)
            if !ev.is_readable() { return 91u64 }

            var buf: Vec<u8> = Vec::with_capacity(16u64)
            val room = buf.capacity_span()
            var got: u64 = 0u64
            match room {
                Option::Some(w) => {
                    val r = server.read(w)
                    match r {
                        Result::Ok(k) => { got = k }
                        Result::Err(e) => { got = 0u64 }
                    }
                }
                Option::None => { got = 0u64 }
            }
            buf.set_size(got)
            var sum: u64 = 0u64
            var i: u64 = 0u64
            while i < buf.size() {
                sum = sum + buf.get(i) as u64
                i = i + 1u64
            }
            got + sum
        }
    "#;
    // 3 bytes, 'b' + 'y' + 'e' = 320.
    assert_eq!(interpreter_value(src) & 0xffff, 323);
    assert_consistent(src, "net_hup_has_bytes");
}

// --- N4: UDP, addresses, socket options -----------------------------

/// Two UDP sockets in one process: one sends, the other receives and
/// can say who sent it.
///
/// A datagram is not a stream, and two of the differences are what
/// this pins. There is no connection to establish — a bound socket
/// hears from anyone at once — and every message carries its sender,
/// which is why `recv_from` has a `last_peer_*` to pair with while
/// `read` does not.
#[test]
fn every_lane_carries_a_datagram_between_two_sockets() {
    let src = r#"
        fn main() -> u64 {
            val abound = UdpSocket::bind("127.0.0.1", 0u64)
            var a = match abound {
                Result::Ok(x) => x,
                Result::Err(e) => { return 80u64 }
            }
            val bbound = UdpSocket::bind("127.0.0.1", 0u64)
            var b = match bbound {
                Result::Ok(x) => x,
                Result::Err(e) => { return 81u64 }
            }
            val bp = b.local_port()
            val bport = match bp { Result::Ok(n) => n, Result::Err(e) => { return 82u64 } }
            val ap = a.local_port()
            val aport = match ap { Result::Ok(n) => n, Result::Err(e) => { return 83u64 } }
            if aport == bport { return 84u64 }

            val msg = String::from_str("udp!")
            val window = msg.as_span()
            val out: Span<u8> = match window {
                Option::Some(w) => w,
                Option::None => { return 85u64 }
            }
            val sent = a.send_to(out, "127.0.0.1", bport)
            val nsent = match sent {
                Result::Ok(n) => n,
                Result::Err(e) => { return 86u64 }
            }
            # All-or-nothing: a datagram has no short send to resume.
            if nsent != 4u64 { return 87u64 }

            val blocking = b.set_blocking(true)
            var buf: Vec<u8> = Vec::with_capacity(64u64)
            val room = buf.capacity_span()
            var got: u64 = 0u64
            match room {
                Option::Some(w) => {
                    val r = b.recv_from(w)
                    match r {
                        Result::Ok(n) => { got = n }
                        Result::Err(e) => { got = 0u64 }
                    }
                }
                Option::None => { got = 0u64 }
            }
            buf.set_size(got)

            # The sender, which a stream read has no equivalent of.
            if b.last_peer_port() != aport { return 88u64 }
            if b.last_peer_addr() != "127.0.0.1" { return 89u64 }

            var sum: u64 = 0u64
            var i: u64 = 0u64
            while i < buf.size() {
                sum = sum + buf.get(i) as u64
                i = i + 1u64
            }
            got + sum
        }
    "#;
    // 4 bytes, 'u' + 'd' + 'p' + '!' = 362.
    assert_eq!(interpreter_value(src) & 0xffff, 366);
    assert_consistent(src, "net_udp_round_trip");
}

/// Both ends of a connection can name themselves and each other, and
/// the numbers line up: what the client calls its peer is what the
/// listener bound.
///
/// Address and port come back separately rather than as one
/// `"127.0.0.1:8080"` string — a port is a number, and nothing should
/// have to find the colon (least of all once IPv6, whose text form is
/// full of them, arrives).
#[test]
fn every_lane_reports_local_and_peer_addresses() {
    let src = r#"
        fn main() -> u64 {
            val bound = TcpListener::bind("127.0.0.1", 0u64)
            var l = match bound {
                Result::Ok(x) => x,
                Result::Err(e) => { return 80u64 }
            }
            val la = l.local_addr()
            val laddr = match la { Result::Ok(t) => t, Result::Err(e) => { return 81u64 } }
            if laddr != "127.0.0.1" { return 82u64 }
            val lp = l.local_port()
            val lport = match lp { Result::Ok(n) => n, Result::Err(e) => { return 83u64 } }

            val conn = TcpStream::connect("127.0.0.1", lport)
            var c = match conn {
                Result::Ok(s) => s,
                Result::Err(e) => { return 84u64 }
            }
            val pa = c.peer_addr()
            val paddr = match pa { Result::Ok(t) => t, Result::Err(e) => { return 85u64 } }
            if paddr != "127.0.0.1" { return 86u64 }
            val pp = c.peer_port()
            val pport = match pp { Result::Ok(n) => n, Result::Err(e) => { return 87u64 } }
            # The client's peer is the listener.
            if pport != lport { return 88u64 }

            val ca = c.local_addr()
            val caddr = match ca { Result::Ok(t) => t, Result::Err(e) => { return 89u64 } }
            if caddr != "127.0.0.1" { return 90u64 }
            val cp = c.local_port()
            val cport = match cp { Result::Ok(n) => n, Result::Err(e) => { return 91u64 } }
            # ...and the client's own port is an ephemeral one, not the
            # listener's.
            if cport == 0u64 { return 92u64 }
            if cport == lport { return 93u64 }
            5u64
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 5);
    assert_consistent(src, "net_addresses");
}

/// The socket options. What they *do* is the kernel's business and
/// not observable from here; that they are accepted, on both
/// platforms, through payloads whose layout differs, is.
///
/// `SO_RCVTIMEO` is the one worth the test: its `struct timeval` has
/// a 32-bit `tv_usec` on the BSDs and a 64-bit one on Linux, so the
/// whole set/get goes through `sys` and a shared struct would be
/// wrong on one of them.
#[test]
fn every_lane_accepts_the_socket_options() {
    let src = r#"
        fn main() -> u64 {
            val bound = TcpListener::bind("127.0.0.1", 0u64)
            var l = match bound {
                Result::Ok(x) => x,
                Result::Err(e) => { return 80u64 }
            }
            val lp = l.local_port()
            val lport = match lp { Result::Ok(n) => n, Result::Err(e) => { return 81u64 } }
            val conn = TcpStream::connect("127.0.0.1", lport)
            var c = match conn {
                Result::Ok(s) => s,
                Result::Err(e) => { return 82u64 }
            }
            val nd = c.set_nodelay(true)
            val a = match nd { Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }
            val ndoff = c.set_nodelay(false)
            val b = match ndoff { Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }
            val rt = c.set_read_timeout(500u64)
            val d = match rt { Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }
            val wt = c.set_write_timeout(500u64)
            val e2 = match wt { Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }
            # 0 removes the bound rather than meaning "expire at once".
            val rt0 = c.set_read_timeout(0u64)
            val f = match rt0 { Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }
            a + b + d + e2 + f
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 5);
    assert_consistent(src, "net_socket_options");
}

// --- N5: name resolution --------------------------------------------
//
// Only `localhost` is pinned, and only that it comes back as a
// loopback address. Anything else would be pinning the machine's DNS,
// which is not the compiler's to guarantee — a test that fails on a
// train is worse than no test.

/// `localhost` resolves, a numeric address resolves to itself, and a
/// name that cannot resolve says so by name rather than as a generic
/// failure.
#[test]
fn every_lane_resolves_a_name_to_an_address() {
    let src = r#"
        fn main() -> u64 {
            val r1 = resolve("localhost")
            val a1 = match r1 { Result::Ok(t) => t, Result::Err(e) => { return 80u64 } }
            # The loopback *range*, not a specific number: a machine
            # may map `localhost` anywhere in 127/8, and pinning the
            # exact address would be pinning its /etc/hosts.
            #
            # Through `String` rather than `str.substring`, which the
            # compiled lanes do not support (todo TYPECHECK-LIES).
            val s1 = String::from_str(a1)
            val head = s1.substring(0u64, 4u64)
            val want = String::from_str("127.")
            if !head.eq(want) { return 81u64 }

            # A numeric address is its own answer, so a caller never
            # has to ask first whether the text is a name.
            val r2 = resolve("127.0.0.1")
            val a2 = match r2 { Result::Ok(t) => t, Result::Err(e) => { return 82u64 } }
            if a2 != "127.0.0.1" { return 83u64 }

            # `.invalid` is reserved by RFC 2606 precisely so that it
            # never resolves — the one name a test may rely on failing.
            val r3 = resolve("no-such-host.invalid")
            val named = match r3 {
                Result::Ok(t) => 0u64,
                Result::Err(e) => match e {
                    NetError::NameNotFound => 1u64,
                    _ => 0u64,
                },
            }
            if named != 1u64 { return 84u64 }
            9u64
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 9);
    assert_consistent(src, "net_resolve");
}

/// `connect` takes a name. It blocks on the handshake already, so
/// blocking on a lookup changes nothing about what it promises —
/// unlike `connect_nonblocking`, which promises to return at once and
/// therefore still wants a numeric address.
#[test]
fn every_lane_connects_by_name() {
    let src = r#"
        fn main() -> u64 {
            val bound = TcpListener::bind("127.0.0.1", 0u64)
            var l = match bound {
                Result::Ok(x) => x,
                Result::Err(e) => { return 80u64 }
            }
            val lp = l.local_port()
            val port = match lp { Result::Ok(n) => n, Result::Err(e) => { return 81u64 } }

            val conn = TcpStream::connect("localhost", port)
            var c = match conn {
                Result::Ok(s) => s,
                Result::Err(e) => { return 82u64 }
            }
            val pa = c.peer_addr()
            val paddr = match pa { Result::Ok(t) => t, Result::Err(e) => { return 83u64 } }
            val sp = String::from_str(paddr)
            val head = sp.substring(0u64, 4u64)
            val want = String::from_str("127.")
            if !head.eq(want) { return 84u64 }

            # A name that does not resolve fails before a descriptor is
            # ever created, and says which kind of failure it was.
            val bad = TcpStream::connect("no-such-host.invalid", port)
            val named = match bad {
                Result::Ok(s) => 0u64,
                Result::Err(e) => match e {
                    NetError::NameNotFound => 1u64,
                    _ => 0u64,
                },
            }
            if named != 1u64 { return 85u64 }
            6u64
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 6);
    assert_consistent(src, "net_connect_by_name");
}
