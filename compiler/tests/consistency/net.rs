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

/// A hostname is an input error until name resolution lands (N5), and
/// it is reported before any descriptor is created.
#[test]
fn every_lane_rejects_a_hostname_the_same_way() {
    let src = r#"
        fn main() -> u64 {
            val conn = TcpStream::connect("localhost", 80u64)
            match conn {
                Result::Ok(s) => 1u64,
                Result::Err(e) => {
                    match e {
                        NetError::InvalidInput => 2u64,
                        _ => 3u64,
                    }
                }
            }
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 2);
    assert_consistent(src, "net_hostname");
}

/// `close` is idempotent and a closed stream stops reading. The field
/// is parked at -1 (NETWORK_IO.md 論点 2), so the second close is a
/// no-op rather than a close of whatever unrelated file the OS has
/// since handed that number to — a failure that would be silent and
/// almost untraceable.
#[test]
fn every_lane_closes_idempotently() {
    let port = spawn_echo_server();
    let src = format!(
        r#"
        fn main() -> u64 {{
            val conn = TcpStream::connect("127.0.0.1", {port}u64)
            var st = match conn {{
                Result::Ok(s) => s,
                Result::Err(e) => {{ return 90u64 }}
            }}
            val first = st.close()
            val a = match first {{ Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }}
            val second = st.close()
            val b = match second {{ Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }}
            if st.as_fd() >= 0i32 {{ return 93u64 }}

            var buf: Vec<u8> = Vec::with_capacity(8u64)
            val room = buf.capacity_span()
            var failed: u64 = 0u64
            match room {{
                Option::Some(w) => {{
                    val r = st.read(w)
                    match r {{
                        Result::Ok(n) => {{ failed = 0u64 }}
                        Result::Err(e) => {{ failed = 1u64 }}
                    }}
                }}
                Option::None => {{ failed = 0u64 }}
            }}
            a * 100u64 + b * 10u64 + failed
        }}
    "#
    );
    // Both closes ok, and the read on a closed stream failed.
    assert_eq!(interpreter_value(&src) & 0xffff, 111);
    assert_consistent(&src, "net_close_idempotent");
}

/// `shutdown_write` ends the request without giving up the descriptor
/// the reply arrives on: the peer's read returns 0 and it stops, but
/// what it already sent is still there.
#[test]
fn every_lane_half_closes_and_still_reads_the_reply() {
    let port = spawn_echo_server();
    let src = format!(
        r#"
        fn main() -> u64 {{
            val conn = TcpStream::connect("127.0.0.1", {port}u64)
            var st = match conn {{
                Result::Ok(s) => s,
                Result::Err(e) => {{ return 90u64 }}
            }}
            val msg = String::from_str("hi")
            val window = msg.as_span()
            val out: Span<u8> = match window {{
                Option::Some(w) => w,
                Option::None => {{ return 91u64 }}
            }}
            val wrote = st.write(out)
            match wrote {{
                Result::Ok(n) => {{ }}
                Result::Err(e) => {{ return 92u64 }}
            }}
            val half = st.shutdown_write()
            val ok = match half {{ Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }}

            var back: Vec<u8> = Vec::with_capacity(8u64)
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
            ok * 1000u64 + got * 100u64 + sum
        }}
    "#
    );
    // shutdown ok, 2 bytes back, 'h' + 'i' = 209.
    assert_eq!(interpreter_value(&src) & 0xffff, 1000 + 200 + 209);
    assert_consistent(&src, "net_shutdown_write");
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
