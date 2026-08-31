// NETWORK_IO N1: the TCP client, against a real peer.
//
// The peer is a `std::net` echo server on a thread of this process,
// bound to 127.0.0.1 on port 0 so the OS picks the port and the test
// reads it back. That is the determinism rule from NETWORK_IO.md:
// loopback only, never a fixed port (nextest runs one process per
// test, in parallel), and nothing pinned but *what happened* — the
// bytes, the `NetError` variant, whether the connection succeeded.
//
// **Tree-walker only.** The compiled lanes cannot run this code yet,
// and not for anything to do with sockets: binding a compound value
// out of a `match` arm (`var s = match conn { Result::Ok(s) => s, ...
// }`) is the COMPOUND-BLOCK-RHS gap, and it is the shape every
// `Result`-returning constructor has. `consistency/net.rs` keeps the
// part that *is* four-lane — `backend_name()` — and todo.md's NET
// entry records the rest.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;

use crate::common::core_modules_dir;

/// Run `source` with `args`, returning `main`'s exit code.
fn run_with_args(source: &str, args: Vec<String>) -> Result<i64, String> {
    let core = core_modules_dir();
    let mut options = interpreter::RunOptions::default();
    options.core_modules_dir = Some(&core);
    options.args = args;
    let outcome = interpreter::run_source(source, "net_test.t", &options)?;
    outcome.exit_code.map(|c| c as i64).ok_or_else(|| "no numeric exit code".to_string())
}

/// An echo server that serves exactly `connections` clients and then
/// stops. Returns the port it is listening on, so nothing has to
/// guess or retry.
///
/// Serving a bounded number and joining nothing is deliberate: the
/// thread ends on its own, and a test that fails does not leave a
/// listener behind for the next one to trip over.
fn spawn_echo_server(connections: usize) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local_addr").port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        tx.send(()).ok();
        for _ in 0..connections {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
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
        }
    });
    // The listener is already bound before the thread starts, so a
    // connect cannot lose the race; this only waits for the thread to
    // exist at all.
    rx.recv().expect("server thread started");
    port
}

/// Send four bytes and read them back. The buffer is allocated once
/// with `Vec::with_capacity` and filled in place by `recv`, which is
/// the no-allocation shape NETWORK_IO.md §5 is about.
#[test]
fn a_client_sends_bytes_and_reads_the_echo_back() {
    let port = spawn_echo_server(1);
    let source = r#"
fn main() -> u64 {
    val port_s = io::arg(0u64)
    val parsed = parse::to_u64(port_s)
    val port = match parsed { Result::Ok(p) => p, Result::Err(e) => 0u64 }

    val conn = TcpStream::connect("127.0.0.1", port)
    var st = match conn {
        Result::Ok(s) => s,
        Result::Err(e) => { println("connect: {e}")  return 90u64 }
    }

    val msg = String::from_str("ping")
    val window = msg.as_span()
    val out = match window {
        Option::Some(w) => w,
        Option::None => { return 91u64 }
    }
    val wrote = st.write(out)
    val sent = match wrote {
        Result::Ok(n) => n,
        Result::Err(e) => { println("write: {e}")  return 92u64 }
    }

    var buf: Vec<u8> = Vec::with_capacity(64u64)
    val room = buf.capacity_span()
    var got: u64 = 0u64
    match room {
        Option::Some(w) => {
            val r = st.read(w)
            match r {
                Result::Ok(n) => { got = n }
                Result::Err(e) => { println("read: {e}")  got = 0u64 }
            }
        }
        Option::None => { got = 0u64 }
    }
    buf.set_size(got)

    # 'p' 'i' 'n' 'g' summed, so a wrong byte is a wrong answer rather
    # than a right-length buffer of zeros — the failure the EXTERN-BUF
    # borrow used to produce.
    var sum: u64 = 0u64
    var i: u64 = 0u64
    while i < buf.size() {
        sum = sum + buf.get(i) as u64
        i = i + 1u64
    }
    sent * 1000u64 + got * 100u64 + sum
}
"#;
    let r = run_with_args(source, vec![port.to_string()]).expect("run");
    // 4 sent, 4 received, 112 + 105 + 110 + 103 = 430.
    assert_eq!(r, 4 * 1000 + 4 * 100 + 430);
}

/// Bytes that no `str` can hold on this engine. The whole reason
/// `read` / `write` take a `Span<u8>` rather than a `str` is that a
/// tree-walker `str` is a Rust `String` and cannot carry a NUL or a
/// lone 0xFF; the compiled lanes can, so a `str`-shaped API would
/// have split the lanes on exactly the payloads a network carries.
#[test]
fn a_payload_that_is_not_utf8_survives_the_round_trip() {
    let port = spawn_echo_server(1);
    let source = r#"
fn main() -> u64 {
    val port_s = io::arg(0u64)
    val parsed = parse::to_u64(port_s)
    val port = match parsed { Result::Ok(p) => p, Result::Err(e) => 0u64 }

    val conn = TcpStream::connect("127.0.0.1", port)
    var st = match conn {
        Result::Ok(s) => s,
        Result::Err(e) => { return 90u64 }
    }

    var out: Vec<u8> = Vec::with_capacity(3u64)
    val space = out.capacity_span()
    match space {
        Option::Some(w) => {
            w.set(0u64, 0u8)
            w.set(1u64, 255u8)
            w.set(2u64, 128u8)
        }
        Option::None => { }
    }
    out.set_size(3u64)
    val body = out.as_span()
    val payload = match body {
        Option::Some(w) => w,
        Option::None => { return 91u64 }
    }
    val wrote = st.write(payload)
    match wrote {
        Result::Ok(n) => { }
        Result::Err(e) => { return 92u64 }
    }

    var back: Vec<u8> = Vec::with_capacity(16u64)
    val room = back.capacity_span()
    var got: u64 = 0u64
    match room {
        Option::Some(w) => {
            val r = st.read(w)
            match r {
                Result::Ok(n) => { got = n }
                Result::Err(e) => { got = 0u64 }
            }
        }
        Option::None => { got = 0u64 }
    }
    back.set_size(got)
    var sum: u64 = 0u64
    var i: u64 = 0u64
    while i < back.size() {
        sum = sum + back.get(i) as u64
        i = i + 1u64
    }
    got * 1000u64 + sum
}
"#;
    let r = run_with_args(source, vec![port.to_string()]).expect("run");
    // 3 bytes back, 0 + 255 + 128.
    assert_eq!(r, 3 * 1000 + 383);
}

/// Nothing is listening, so the connect is refused — and the refusal
/// arrives as the `NetError` variant for it rather than as a generic
/// failure. The port comes from a listener that is closed before the
/// toylang program runs, which is the only way to name a port that is
/// reliably free.
#[test]
fn connecting_to_a_closed_port_is_refused_by_name() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        l.local_addr().expect("local_addr").port()
    };
    let source = r#"
fn main() -> u64 {
    val port_s = io::arg(0u64)
    val parsed = parse::to_u64(port_s)
    val port = match parsed { Result::Ok(p) => p, Result::Err(e) => 0u64 }
    val conn = TcpStream::connect("127.0.0.1", port)
    match conn {
        Result::Ok(s) => 1u64,
        Result::Err(e) => {
            match e {
                NetError::ConnectionRefused => 2u64,
                _ => 3u64,
            }
        }
    }
}
"#;
    let r = run_with_args(source, vec![port.to_string()]).expect("run");
    assert_eq!(r, 2, "expected ConnectionRefused (1 = connected, 3 = some other error)");
}

/// An address that is not a numeric IPv4 address. Name resolution is
/// N5; until it lands a hostname is an input error, not a lookup
/// failure, and it is reported before any descriptor is created.
#[test]
fn a_hostname_is_an_input_error_until_name_resolution_lands() {
    let source = r#"
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
    let r = run_with_args(source, vec![]).expect("run");
    assert_eq!(r, 2);
}

/// `close` is idempotent, and a closed stream reports rather than
/// reusing whatever descriptor the OS has since handed that number
/// to. The field is parked at -1 (NETWORK_IO.md 論点 2), so the
/// second close is a no-op and the read that follows fails.
#[test]
fn closing_twice_is_a_no_op_and_a_closed_stream_no_longer_reads() {
    let port = spawn_echo_server(1);
    let source = r#"
fn main() -> u64 {
    val port_s = io::arg(0u64)
    val parsed = parse::to_u64(port_s)
    val port = match parsed { Result::Ok(p) => p, Result::Err(e) => 0u64 }
    val conn = TcpStream::connect("127.0.0.1", port)
    var st = match conn {
        Result::Ok(s) => s,
        Result::Err(e) => { return 90u64 }
    }
    val first = st.close()
    val a = match first { Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }
    val second = st.close()
    val b = match second { Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }
    if st.as_fd() >= 0i32 { return 93u64 }

    var buf: Vec<u8> = Vec::with_capacity(8u64)
    val room = buf.capacity_span()
    var failed: u64 = 0u64
    match room {
        Option::Some(w) => {
            val r = st.read(w)
            match r {
                Result::Ok(n) => { failed = 0u64 }
                Result::Err(e) => { failed = 1u64 }
            }
        }
        Option::None => { failed = 0u64 }
    }
    a * 100u64 + b * 10u64 + failed
}
"#;
    let r = run_with_args(source, vec![port.to_string()]).expect("run");
    assert_eq!(r, 111, "both closes ok (1,1) and the read on a closed stream failed (1)");
}

/// A sanity check that the peer really is this process's own thread
/// and the port really is ephemeral: two servers, two ports, two
/// independent round trips.
#[test]
fn two_clients_on_two_ephemeral_ports_do_not_interfere() {
    let a = spawn_echo_server(1);
    let b = spawn_echo_server(1);
    assert_ne!(a, b);
    let source = r#"
fn one(port: u64, byte: u8) -> u64 {
    val conn = TcpStream::connect("127.0.0.1", port)
    var st = match conn {
        Result::Ok(s) => s,
        Result::Err(e) => { return 900u64 }
    }
    var out: Vec<u8> = Vec::with_capacity(1u64)
    val space = out.capacity_span()
    match space {
        Option::Some(w) => { w.set(0u64, byte) }
        Option::None => { }
    }
    out.set_size(1u64)
    val body = out.as_span()
    val payload = match body {
        Option::Some(w) => w,
        Option::None => { return 901u64 }
    }
    val wrote = st.write(payload)
    match wrote {
        Result::Ok(n) => { }
        Result::Err(e) => { return 902u64 }
    }
    var back: Vec<u8> = Vec::with_capacity(8u64)
    val room = back.capacity_span()
    var got: u64 = 0u64
    match room {
        Option::Some(w) => {
            val r = st.read(w)
            match r {
                Result::Ok(n) => { got = n }
                Result::Err(e) => { got = 0u64 }
            }
        }
        Option::None => { got = 0u64 }
    }
    back.set_size(got)
    if back.size() == 0u64 { return 903u64 }
    back.get(0u64) as u64
}

fn main() -> u64 {
    val pa_s = io::arg(0u64)
    val pb_s = io::arg(1u64)
    val pa = match parse::to_u64(pa_s) { Result::Ok(p) => p, Result::Err(e) => 0u64 }
    val pb = match parse::to_u64(pb_s) { Result::Ok(p) => p, Result::Err(e) => 0u64 }
    one(pa, 7u8) * 1000u64 + one(pb, 9u8)
}
"#;
    let r = run_with_args(source, vec![a.to_string(), b.to_string()]).expect("run");
    assert_eq!(r, 7 * 1000 + 9);
}

/// The connection stays usable after the peer has been told there is
/// nothing more coming. `shutdown_write` half-closes, so the echo
/// server's read returns 0 and it stops — but the reply it already
/// sent is still there to be read.
#[test]
fn shutdown_write_ends_the_request_without_giving_up_the_reply() {
    let port = spawn_echo_server(1);
    let source = r#"
fn main() -> u64 {
    val port_s = io::arg(0u64)
    val parsed = parse::to_u64(port_s)
    val port = match parsed { Result::Ok(p) => p, Result::Err(e) => 0u64 }
    val conn = TcpStream::connect("127.0.0.1", port)
    var st = match conn {
        Result::Ok(s) => s,
        Result::Err(e) => { return 90u64 }
    }
    val msg = String::from_str("hi")
    val window = msg.as_span()
    val out = match window {
        Option::Some(w) => w,
        Option::None => { return 91u64 }
    }
    val wrote = st.write(out)
    match wrote {
        Result::Ok(n) => { }
        Result::Err(e) => { return 92u64 }
    }
    val half = st.shutdown_write()
    val ok = match half { Result::Ok(_) => 1u64, Result::Err(e) => 0u64 }

    var back: Vec<u8> = Vec::with_capacity(8u64)
    val room = back.capacity_span()
    var got: u64 = 0u64
    match room {
        Option::Some(w) => {
            val r = st.read(w)
            match r {
                Result::Ok(n) => { got = n }
                Result::Err(e) => { got = 0u64 }
            }
        }
        Option::None => { got = 0u64 }
    }
    back.set_size(got)
    var sum: u64 = 0u64
    var i: u64 = 0u64
    while i < back.size() {
        sum = sum + back.get(i) as u64
        i = i + 1u64
    }
    ok * 10000u64 + got * 1000u64 + sum
}
"#;
    let r = run_with_args(source, vec![port.to_string()]).expect("run");
    // shutdown ok, 2 bytes back, 'h' + 'i' = 104 + 105.
    assert_eq!(r, 10000 + 2 * 1000 + 209);
}
