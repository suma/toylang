# An echo server driven by the event poller.
#
# This is a server and nothing else: it binds, watches its descriptors
# through `Poller`, and keeps serving until it is idle or a client says
# `quit`. Connect to it from another terminal.
#
# `Poller` is the one interface over the two kernels — `epoll` on Linux,
# `kqueue` on macOS and the BSDs (EVENT_POLLING.md). Nothing below names
# either: the platform is chosen when the runtime is built, and the
# behaviour the two disagree about is normalised underneath —
# a descriptor that is ready in several ways is **one** event with the
# flags merged, a timeout is `Ok(0)` rather than an error, and the token
# is a number the runtime never looks inside.
#
# Both the accept and the read go through the poller, which is the point:
# `wait` is the only place this program blocks.
#
# Build and run (AOT):
#   cargo run -q -p compiler -- interpreter/example/net_echo_server.t -o /tmp/net_echo_server
#   /tmp/net_echo_server
#
# It prints the port it was given to **stderr**, so stdout stays a clean
# transcript of what was served:
#
#   listening on 127.0.0.1:54321          <- stderr
#   waiting up to 30 seconds for a client <- stderr
#
# Then, from another terminal:
#
#   printf 'hello' | nc 127.0.0.1 54321   -> echoes `hello` back
#   printf 'quit'  | nc 127.0.0.1 54321   -> stops the server
#
# and stdout ends up as the transcript:
#
#   served: hello
#   served: quit
#   connections served: 2
#
# Pass a number to change the idle budget: `/tmp/net_echo_server 5`.
#
# `compiler/tests/example_consistency.rs` skips this one: it waits for a
# peer, and running it there would only prove that nobody called. The
# same `bind` / `accept` / `Poller` surface is pinned across four lanes
# by `compiler/tests/consistency/net.rs`, which puts the client in the
# same process to stay deterministic.

# Tokens are the caller's names for its descriptors — the runtime
# stores and returns them without interpretation. Two constants beat
# two magic numbers in the dispatch below.
const LISTENER: u64 = 1u64
const CONNECTION: u64 = 2u64

# One `wait` is this long; the idle budget is counted in whole of them.
const TICK_MS: i64 = 500i64

# How long to sit idle before giving up, unless argv says otherwise.
fn idle_seconds() -> u64 {
    # `argc` counts the program's own arguments only — the executable's
    # name is not among them — so the first one is index 0.
    if io::argc() < 1u64 {
        return 30u64
    }
    val raw = io::arg(0u64)
    val parsed = parse::to_u64(raw)
    match parsed {
        Result::Ok(n) => n,
        Result::Err(e) => 30u64,
    }
}

# Read what the peer sent, echo it straight back, and hand back the
# bytes so the caller can decide what they meant.
#
# The buffer is a `Vec<u8>`'s spare capacity: `capacity_span` is the
# window the socket writes into and `set_size` is the commit. Nothing
# is copied on the way in, and the echo goes out of that same buffer.
fn serve(conn: &TcpStream) -> String {
    var inbox: Vec<u8> = Vec::with_capacity(1024u64)
    val room = inbox.capacity_span()
    var got: u64 = 0u64
    match room {
        Option::Some(window) => {
            val read = conn.read(window)
            match read {
                Result::Ok(n) => { got = n }
                Result::Err(e) => { eprintln(e) }
            }
        }
        Option::None => { eprintln("no room to read into") }
    }
    inbox.set_size(got)

    val back = inbox.as_span()
    match back {
        Option::Some(window) => {
            val wrote = conn.write(window)
            match wrote {
                Result::Ok(n) => { }
                Result::Err(e) => { eprintln(e) }
            }
        }
        Option::None => { }
    }

    var text = String::new()
    var i: u64 = 0u64
    while i < inbox.size() {
        val byte: u8 = inbox.get(i)
        text.push(byte)
        i = i + 1u64
    }
    text
}

fn main() -> u64 {
    val bound = TcpListener::bind("127.0.0.1", 0u64)
    var listener = match bound {
        Result::Ok(l) => l,
        Result::Err(e) => { eprintln(e)  return 1u64 }
    }
    val p = listener.local_port()
    val port = match p {
        Result::Ok(n) => n,
        Result::Err(e) => { eprintln(e)  return 1u64 }
    }

    val made = Poller::new()
    var poller = match made {
        Result::Ok(x) => x,
        Result::Err(e) => { eprintln(e)  return 1u64 }
    }
    val watch = poller.register(listener.as_fd(), LISTENER, interest_read())
    match watch {
        Result::Ok(_) => { }
        Result::Err(e) => { eprintln(e)  return 1u64 }
    }

    # The port goes to stderr: it differs every run, and stdout is the
    # transcript of what was served.
    val budget = idle_seconds()
    eprint("listening on 127.0.0.1:")
    eprintln(port)
    eprint("waiting up to ")
    eprint(budget)
    eprintln(" seconds for a client")

    val ticks_allowed = budget * 1000u64 / (TICK_MS as u64)
    var idle_ticks: u64 = 0u64
    var served: u64 = 0u64
    var running: bool = true

    while running {
        # The only blocking call in the program. `Ok(0)` is a timeout,
        # which is the server being idle — not a failure. A loop that
        # treated it as one would exit the first quiet moment.
        val ready = poller.wait(TICK_MS)
        val n = match ready {
            Result::Ok(k) => k,
            Result::Err(e) => { eprintln(e)  return 1u64 }
        }
        if n == 0u64 {
            idle_ticks = idle_ticks + 1u64
            if idle_ticks >= ticks_allowed { running = false }
        } else {
            idle_ticks = 0u64
            var i: u64 = 0u64
            while i < n {
                val ev = poller.event(i)
                if ev.token() == LISTENER {
                    # A readable listener means a connection is queued,
                    # so this `accept` will not block.
                    val accepted = listener.accept()
                    match accepted {
                        Result::Ok(conn) => {
                            val line: String = handle(&poller, conn)
                            served = served + 1u64
                            print("served: ")
                            println(line)
                            # Bound rather than nested: an associated
                            # function call in an argument list is not
                            # lowered by the compiled lanes yet.
                            val stop = String::from_str("quit")
                            if line.eq(stop) { running = false }
                        }
                        Result::Err(e) => { eprintln(e) }
                    }
                }
                i = i + 1u64
            }
        }
    }

    print("connections served: ")
    println(served)
    0u64
}

# Wait — through the poller — for the accepted connection to have
# something to say, then serve it. `register` on the new descriptor and
# `deregister` on the way out are the two halves `epoll` splits into
# ADD / DEL and `kqueue` into EV_ADD / EV_DELETE.
fn handle(poller: &Poller, conn: TcpStream) -> String {
    val watch = poller.register(conn.as_fd(), CONNECTION, interest_read())
    match watch {
        Result::Ok(_) => { }
        Result::Err(e) => { eprintln(e) }
    }

    # Block here, not in `read`: the connection is served only once the
    # poller says it has something to say. `Ok(0)` is the timeout — a
    # peer that connected and then said nothing.
    val ready = poller.wait(TICK_MS)
    val n = match ready {
        Result::Ok(k) => k,
        Result::Err(e) => { eprintln(e)  0u64 },
    }
    val readable = if n > 0u64 {
        val ev = poller.event(0u64)
        ev.token() == CONNECTION
    } else {
        false
    }
    val text: String = if readable { serve(&conn) } else { String::new() }

    # `register` on a new descriptor and `deregister` on the way out
    # are the two halves `epoll` splits into ADD / DEL and `kqueue`
    # into EV_ADD / EV_DELETE.
    val drop = poller.deregister(conn.as_fd())
    match drop {
        Result::Ok(_) => { }
        Result::Err(e) => { eprintln(e) }
    }
    text
}
