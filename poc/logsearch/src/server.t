# The HTTP server: one event loop, fixed buffers, no blocking call
# other than `Poller::wait`.
#
# `http.t` decides what the bytes mean; this file decides when to read
# them, when to write, and when to give up. Keeping the two apart is
# what let the protocol be tested without a socket.
#
# **128 connections at a time**, with the listener unhooked when the
# table fills (HTTP_API.md section 4). Extra clients then wait in the
# TCP backlog, which is easier on a sender than an accept followed by
# a close.
#
# The table holds the handles themselves (`Vec<Option<TcpStream>>`,
# `None` = free slot). It could not until 2026-09-20: reading a handle
# out of a container bound an alias whose drop glue closed the
# descriptor, so the table had to hold plain numbers and re-open each
# one for a turn. `borrow` lends a slot's socket without claiming it,
# and `Vec::replace` takes it back when the slot is freed.
#
# What is *not* given up: reads and writes are both partial-safe. A
# search result is megabytes and a socket takes what it takes, so
# `WouldBlock` in the middle of a response is the normal case, not an
# error (HTTP_API.md section 4).
#
# **Measured, because `/v1/stats` exists to be checked against.**
# 200 requests to `/healthz` moved `live_bytes` by 35 bytes in total
# (0.18 per request, which is noise). 100 requests to `/v1/stats`
# moved it by 3,535 -- about 35 bytes each, from the `MountSet` and
# `Catalog` that route builds to answer. That is a real drip: a UI
# polling once a second would find 3 MB a day there. It is not fixed,
# and it is written down here rather than left for someone to
# discover, because MEMORY.md section 5 makes a flat `live_bytes` the
# definition of this server being healthy.

import std.io
import std.net
import std.parse
import std.poll
import std.time
import catalog
import http
import mount
import query
import record
import store
import ui

# Tokens are names the poller stores and hands back without looking
# inside. Functions rather than `const`, because a module's top-level
# `const` is not visible to the module's own functions in the
# compiled lanes (todo.md MODULE-CONST) -- the same reason
# `core/std/poll.t` spells its interests as `pub fn`.
fn listener_token() -> u64 { 1u64 }

# One token per slot, so an event names its connection without a
# search. Slot 0 is token 2; the listener keeps 1.
fn conn_token(slot: u64) -> u64 { slot + 2u64 }
fn token_slot(tok: u64) -> u64 { tok - 2u64 }

# How many connections are served at once, and what each one costs.
#
# The table is allocated at startup (HTTP_API.md section 4): two
# arenas, carved into one region per slot, so a connection needs no
# allocation of its own and a slow client cannot make the server grow.
# The price is fixed and visible -- 128 * (64 KiB + 256 KiB) = 40 MiB --
# and `max_conns` is the knob if that is too much for a host.
#
# A request bigger than its slot gets `413`: 64 KiB is the record
# limit (`max_record_bytes`), so a body that does not fit is one this
# server was not going to accept anyway. A *response* bigger than its
# slot is different -- a query legitimately answers with megabytes --
# and that is what the shared `big` buffer below is for.
pub fn max_conns() -> u64 { 128u64 }
pub fn recv_slot_bytes() -> u64 { 65536u64 }
pub fn send_slot_bytes() -> u64 { 262144u64 }

# One `wait` is this long. It bounds how quickly a timeout is noticed,
# not how quickly a ready socket is served.
fn tick_ms() -> i64 { 250i64 }

# HTTP_API.md section 4. The receive buffer is the request limit
# rather than 64 KiB: `http.t` refuses anything larger, so this is the
# most that can ever be outstanding.
pub fn recv_bytes() -> u64 { http::max_request_bytes() }
pub fn send_bytes() -> u64 { 262144u64 }
pub fn header_timeout_ns() -> u64 { 10000000000u64 }
pub fn idle_timeout_ns() -> u64 { 60000000000u64 }

# One record cannot be longer than a connection's receive buffer
# (HTTP_API.md section 4). A line past this is rejected rather than
# truncated: half a log line is not a log line.
pub fn max_record_bytes() -> u64 { 65536u64 }

# How long an active segment may sit before it is written out, even
# if it never fills (DATA_MODEL.md section 4). A record that is only
# in memory is a record that a crash loses.
pub fn flush_after_ns() -> u64 { 60000000000u64 }

# What the server has done since it started. Scalars only, so it can
# be threaded through the loop as a `&mut`.
pub struct Stats {
    started_ns: u64,
    requests: u64,
    refused: u64,
    bytes_in: u64,
    bytes_out: u64,
    connections: u64,
    shutdown: bool,
    # Ingest. `segid` is the id the next flush will use, and `seq` the
    # number given to the next record -- both have to survive a
    # request, which is why they live with the counters rather than
    # in the handler.
    ready: bool,
    segid: u64,
    seq: u64,
    accepted: u64,
    rejected: u64,
    segments: u64,
}

impl Stats {
    pub fn new() -> Self {
        val s = Stats {
            started_ns: time::now_mono_ns(),
            requests: 0u64, refused: 0u64,
            bytes_in: 0u64, bytes_out: 0u64,
            connections: 0u64, shutdown: false,
            ready: false, segid: 1u64, seq: 1u64,
            accepted: 0u64, rejected: 0u64, segments: 0u64,
        }
        s
    }

    pub fn uptime_s(&self) -> u64 {
        val now = time::now_mono_ns()
        if now <= self.started_ns { return 0u64 }
        (now - self.started_ns) / 1000000000u64
    }
}

# ---------------------------------------------------------------------
# Socket plumbing
#
# Both directions report the same three things -- progress, "not now",
# and "done" -- and the loop has to tell them apart. `WouldBlock` is
# the one that is easy to get wrong: it is the socket being a socket.

pub fn io_again() -> i64 { 0i64 }
pub fn io_closed() -> i64 { -1i64 }
pub fn io_failed() -> i64 { -2i64 }
pub fn io_full() -> i64 { -3i64 }

fn recv_some(conn: &TcpStream, buf: &mut ByteWriter, cap: u64) -> i64 {
    val have = buf.len()
    if have >= cap { return io_full() }
    val want = cap - have
    buf.reserve(want)
    val room = buf.room()
    var out = io_failed()
    match room {
        Option::Some(all) => {
            val win = all.slice(have, want)
            val got = conn.read(win)
            match got {
                Result::Ok(n) => {
                    if n == 0u64 {
                        out = io_closed()
                    } else {
                        buf.set_len(have + n)
                        out = n as i64
                    }
                }
                Result::Err(e) => {
                    match e {
                        NetError::WouldBlock => { out = io_again() }
                        NetError::Interrupted => { out = io_again() }
                        _ => { out = io_failed() }
                    }
                }
            }
        }
        Option::None => { }
    }
    out
}

fn send_some(conn: &TcpStream, buf: &ByteWriter, from: u64) -> i64 {
    val total = buf.len()
    if from >= total { return 0i64 }
    val win = buf.span()
    var out = io_failed()
    match win {
        Option::Some(all) => {
            val piece = all.slice(from, total - from)
            val put = conn.write(piece)
            match put {
                Result::Ok(n) => { out = n as i64 }
                Result::Err(e) => {
                    match e {
                        NetError::WouldBlock => { out = io_again() }
                        NetError::Interrupted => { out = io_again() }
                        _ => { out = io_failed() }
                    }
                }
            }
        }
        Option::None => { }
    }
    out
}

# ---------------------------------------------------------------------
# The active segment

# Write what has been collected out to a mount and start a new
# segment. Returns the bytes written, or 0 if there was nothing to
# write or nowhere to put it.
#
# **The buffer is only reset when the write succeeded.** Throwing the
# records away because no mount would take them is the one outcome
# nobody can recover from; keeping them means the writer stays full
# and ingest starts refusing, which is a state an operator can see
# and fix.
fn flush_active(w: &mut ArchiveWriter, ms: &mut MountSet, gens: &Vec<u64>,
                st: &mut Stats, crc: &Crc32) -> u64 {
    if w.is_empty() { return 0u64 }
    val done = store::place_segment(w, ms, gens, st.segid, crc)
    if done > 0u64 {
        st.segid = st.segid + 1u64
        st.segments = st.segments + 1u64
        w.reset()
    }
    done
}

# `POST /v1/ingest` -- newline-separated lines, one record each.
#
# **Partial success is the point** (HTTP_API.md section 2): three bad
# lines out of a hundred must not make the sender retry the
# ninety-seven that were fine. So a line that cannot be taken is
# counted and the rest go in.
#
# A line carrying its own timestamp keeps it; anything else gets the
# time it arrived. That is the only way a line with no date can be
# found by a time range at all, and pretending it has no time would
# make it invisible to every query that names one.
fn ingest_route(b: Span<u8>, r: &Request, st: &mut Stats,
                w: &mut ArchiveWriter, ms: &mut MountSet, gens: &Vec<u64>,
                alive: bool, out: &mut ByteWriter) {
    if !st.ready {
        http::respond_error(out, 503u64, "no writable mount",
                            "the mount directory has to exist before serving",
                            alive)
        return
    }
    val crc = Crc32::new()
    val now = time::now_unix_secs()
    val body = b.slice(r.body_at, r.body_len)
    var sc = LineScan::new(r.body_len)
    var rec = ParsedLine::new()

    val first = st.seq
    var taken: u64 = 0u64
    var bad: u64 = 0u64
    var stalled = false
    var more = true
    while more {
        val nx = sc.next(body)
        match nx {
            Option::Some(l) => {
                if l.len > max_record_bytes() {
                    bad = bad + 1u64
                } elif l.len > 0u64 {
                    if w.is_full() && !stalled {
                        val done = flush_active(w, ms, gens, st, &crc)
                        if done == 0u64 { stalled = true }
                    }
                    if stalled {
                        bad = bad + 1u64
                    } else {
                        record::parse_line(body, l, &mut rec)
                        if !rec.has_ts {
                            rec.has_ts = true
                            rec.ts = now
                        }
                        w.add(body, l, &rec)
                        taken = taken + 1u64
                        st.seq = st.seq + 1u64
                    }
                }
            }
            Option::None => { more = false }
        }
    }
    st.accepted = st.accepted + taken
    st.rejected = st.rejected + bad

    # Nothing got in and the reason was nowhere to put it: that is
    # this server's failure, not the sender's.
    if taken == 0u64 && stalled {
        http::respond_error(out, 503u64, "no writable mount",
                            "every mount is full, readonly or degraded", alive)
        return
    }

    var last = first
    if taken > 0u64 { last = first + taken - 1u64 } else { last = 0u64 }
    var seq_first = first
    if taken == 0u64 { seq_first = 0u64 }
    var body_out = ByteWriter::with_capacity(128u64)
    body_out.put_str("{{\u{22}accepted\u{22}:")
    body_out.put_str("{taken}")
    body_out.put_str(",\u{22}rejected\u{22}:")
    body_out.put_str("{bad}")
    body_out.put_str(",\u{22}seq_first\u{22}:")
    body_out.put_str("{seq_first}")
    body_out.put_str(",\u{22}seq_last\u{22}:")
    body_out.put_str("{last}")
    body_out.put_str("}}\n")
    http::begin_response(out, 200u64, "application/json", body_out.len(), alive)
    http::end_headers(out)
    out.put_all(&body_out)
}

# ---------------------------------------------------------------------
# Routing

fn path_is(b: Span<u8>, r: &Request, want: str) -> bool {
    http::eq_at(b, r.path_at, r.path_len, want)
}

# Administration answers only to the loopback, because there is no
# authentication anywhere in this server and that is the only line
# left to draw (HTTP_API.md section 2).
fn is_local(conn: &TcpStream) -> bool {
    val peer = conn.peer_addr()
    match peer {
        Result::Ok(a) => {
            val s = String::from_str(a)
            val v4 = String::from_str("127.0.0.1")
            if s.eq(&v4) { return true }
            val v6 = String::from_str("::1")
            s.eq(&v6)
        }
        Result::Err(e) => { false }
    }
}

fn put_json_str(out: &mut ByteWriter, s: str) {
    # No escaping: every string this writes is a path, a state name or
    # a uuid, all of them from the configuration or from hex. A value
    # that could carry a quote does not reach here, and pretending to
    # escape without doing it properly would be worse than saying so.
    out.put_str("\u{22}")
    out.put_str(s)
    out.put_str("\u{22}")
}

# `GET /v1/stats` -- what the process is doing and what the disks hold.
#
# The memory block is the allocation counters straight through: a
# `live_bytes` that does not grow over a day is this server's
# definition of healthy (MEMORY.md section 5), and a server that will
# not say its own number cannot be checked against it.
fn stats_body(spec: str, st: &Stats, body: &mut ByteWriter) {
    val crc = Crc32::new()
    var ms = MountSet::new()
    val have = mount::open_spec(spec, &mut ms)

    val up = st.uptime_s()
    val reqs = st.requests
    val refused = st.refused
    val conns = st.connections
    val bin = st.bytes_in
    val bout = st.bytes_out
    body.put_str("{{\u{22}uptime_s\u{22}:")
    body.put_str("{up}")
    body.put_str(",\u{22}requests\u{22}:{{\u{22}served\u{22}:")
    body.put_str("{reqs}")
    body.put_str(",\u{22}refused\u{22}:")
    body.put_str("{refused}")
    body.put_str(",\u{22}connections\u{22}:")
    body.put_str("{conns}")
    body.put_str(",\u{22}bytes_in\u{22}:")
    body.put_str("{bin}")
    body.put_str(",\u{22}bytes_out\u{22}:")
    body.put_str("{bout}")
    body.put_str("}},\u{22}mounts\u{22}:[")

    if have {
        var i: u64 = 0u64
        while i < ms.size() {
            if i > 0u64 { body.put_str(",") }
            val p = ms.path_of(i)
            val ps = p.to_str()
            val c = catalog::load(ps, &crc)
            val segs = c.size()
            val recs = c.total_records()
            val used = c.total_bytes()
            val quota = ms.quota_of(i)
            val state = mount::state_name(ms.state_of(i))
            val ro = ms.is_readonly(i)
            body.put_str("{{\u{22}path\u{22}:")
            put_json_str(body, ps)
            body.put_str(",\u{22}state\u{22}:")
            put_json_str(body, state)
            body.put_str(",\u{22}readonly\u{22}:")
            if ro { body.put_str("true") } else { body.put_str("false") }
            body.put_str(",\u{22}quota\u{22}:")
            body.put_str("{quota}")
            body.put_str(",\u{22}used\u{22}:")
            body.put_str("{used}")
            body.put_str(",\u{22}segments\u{22}:")
            body.put_str("{segs}")
            body.put_str(",\u{22}records\u{22}:")
            body.put_str("{recs}")
            body.put_str("}}")
            i = i + 1u64
        }
    }

    val live = __builtin_live_bytes()
    val cumulative = __builtin_cumulative_bytes()
    val allocs = __builtin_alloc_count()
    body.put_str("],\u{22}memory\u{22}:{{\u{22}live_bytes\u{22}:")
    body.put_str("{live}")
    body.put_str(",\u{22}cumulative_bytes\u{22}:")
    body.put_str("{cumulative}")
    body.put_str(",\u{22}alloc_count\u{22}:")
    body.put_str("{allocs}")
    body.put_str("}}}}\n")
}

# Rebuild every writable mount's catalog from `seg/`.
# `POST /v1/admin/compact` — merge one run of cold segments per
# mount and say what moved. One pass per request: the caller decides
# how much work to ask for by asking again (HTTP_API.md section 2).
fn admin_compact(spec: str, body: &mut ByteWriter) -> u64 {
    val crc = Crc32::new()
    var ms = MountSet::new()
    if !mount::open_spec(spec, &mut ms) { return 503u64 }
    val now = time::now_unix_secs()
    var merged: u64 = 0u64
    var records: u64 = 0u64
    var bytes_in: u64 = 0u64
    var bytes_out: u64 = 0u64
    var failed: u64 = 0u64
    var i: u64 = 0u64
    while i < ms.size() {
        if !ms.is_readonly(i) {
            val p = ms.path_of(i)
            val done = compact::compact_once(p.to_str(), now, &crc)
            if !done.is_ok() {
                failed = failed + 1u64
            } else {
                merged = merged + done.merged()
                records = records + done.records()
                bytes_in = bytes_in + done.bytes_in()
                bytes_out = bytes_out + done.bytes_out()
            }
        }
        i = i + 1u64
    }
    body.put_str("{{\u{22}merged\u{22}:")
    body.put_str("{merged}")
    body.put_str(",\u{22}records\u{22}:")
    body.put_str("{records}")
    body.put_str(",\u{22}bytes_in\u{22}:")
    body.put_str("{bytes_in}")
    body.put_str(",\u{22}bytes_out\u{22}:")
    body.put_str("{bytes_out}")
    body.put_str(",\u{22}failed\u{22}:")
    body.put_str("{failed}")
    body.put_str("}}\n")
    200u64
}

fn admin_repair(spec: str, body: &mut ByteWriter) -> u64 {
    val crc = Crc32::new()
    var ms = MountSet::new()
    if !mount::open_spec(spec, &mut ms) { return 503u64 }
    var rows: u64 = 0u64
    var mounts: u64 = 0u64
    var i: u64 = 0u64
    while i < ms.size() {
        val p = ms.path_of(i)
        val ps = p.to_str()
        var built = catalog::rebuild(ps, &crc)
        val at = catalog::latest_gen(ps)
        built.adopt_generation(at)
        if catalog::compact(ps, &mut built, &crc) {
            rows = rows + built.size()
            mounts = mounts + 1u64
            # The label dictionary is a cache of the same segments,
            # so it is rebuilt with them.
            val relabelled = labels::repair(ps, built.generation(), &crc)
        }
        i = i + 1u64
    }
    body.put_str("{{\u{22}mounts\u{22}:")
    body.put_str("{mounts}")
    body.put_str(",\u{22}segments\u{22}:")
    body.put_str("{rows}")
    body.put_str("}}\n")
    200u64
}

# Drop whatever the retention window has passed by.
fn admin_gc(spec: str, days: u64, body: &mut ByteWriter) -> u64 {
    val crc = Crc32::new()
    var ms = MountSet::new()
    if !mount::open_spec(spec, &mut ms) { return 503u64 }
    val now = time::now_unix_secs()
    val cutoff = now - ((days as i64) * 86400i64)
    var dropped: u64 = 0u64
    var freed: u64 = 0u64
    var i: u64 = 0u64
    while i < ms.size() {
        if !ms.is_readonly(i) {
            val p = ms.path_of(i)
            val ps = p.to_str()
            var c = catalog::load(ps, &crc)
            val gen = c.generation()
            if gen > 0u64 {
                var dead: Vec<u64> = Vec::new()
                c.expired(cutoff, &mut dead)
                var ids: Vec<u64> = Vec::new()
                var sizes: Vec<u64> = Vec::new()
                var paths: Vec<String> = Vec::new()
                var k: u64 = 0u64
                while k < dead.size() {
                    val idx = dead.get(k)
                    val r = c.row(idx)
                    ids.push(r.segid)
                    sizes.push(r.seg_bytes)
                    val sp = catalog::seg_path(ps, &r)
                    paths.push(sp.clone())
                    k = k + 1u64
                }
                var j: u64 = 0u64
                while j < ids.size() {
                    val segid = ids.get(j)
                    val why = catalog::why_retention()
                    if catalog::append_remove(ps, gen, segid, why, &crc) {
                        val gone = c.remove(segid)
                        val sp: &String = paths.borrow(j)
                        # Out of the label dictionary before the file
                        # goes: after the unlink there is nothing left
                        # to read the segment's terms from.
                        val forgot = labels::forget_segment(ps, sp, gen, &crc)
                        val rm = fs::remove_file(sp.to_str())
                        match rm {
                            Result::Ok(u) => {
                                dropped = dropped + 1u64
                                freed = freed + sizes.get(j)
                            }
                            Result::Err(e) => { }
                        }
                    }
                    j = j + 1u64
                }
                if ids.size() > 0u64 {
                    val folded = catalog::compact(ps, &mut c, &crc)
                }
            }
        }
        i = i + 1u64
    }
    body.put_str("{{\u{22}dropped\u{22}:")
    body.put_str("{dropped}")
    body.put_str(",\u{22}bytes_freed\u{22}:")
    body.put_str("{freed}")
    body.put_str("}}\n")
    200u64
}

# Turn one parsed request into a whole response in `out`.
#
# The response is built in memory before a byte of it is written.
# That is affordable because every body this server produces has a
# bound -- and it is what lets `content-length` always be right,
# which is what keeps keep-alive usable.
# `local` rather than the socket itself: whether the peer is the
# loopback is the only thing the router needs to know about the
# connection, and taking the answer instead of the socket is what
# makes every route testable without one.
pub fn route(spec: str, b: Span<u8>, r: &Request, local: bool,
             st: &mut Stats, w: &mut ArchiveWriter, ms: &mut MountSet,
             gens: &Vec<u64>, out: &mut ByteWriter) {
    out.clear()
    val alive = r.keep_alive

    if r.status != 0u64 {
        st.refused = st.refused + 1u64
        # A refused request has nothing further to say on the
        # connection: the parse stopped, so where the next one starts
        # is unknown.
        http::respond_error(out, r.status, "bad request", "", false)
        return
    }
    st.requests = st.requests + 1u64

    if r.is_get() && path_is(b, r, "/healthz") {
        http::respond_text(out, 200u64, "text/plain", "ok\n", alive)
        return
    }

    if r.is_get() && path_is(b, r, "/v1/stats") {
        var body = ByteWriter::with_capacity(4096u64)
        stats_body(spec, st, &mut body)
        http::begin_response(out, 200u64, "application/json", body.len(), alive)
        http::end_headers(out)
        out.put_all(&body)
        return
    }

    if r.is_get() && path_is(b, r, "/") {
        var page = ByteWriter::with_capacity(16384u64)
        ui::page(&mut page)
        http::begin_response(out, 200u64, ui::content_type(), page.len(), alive)
        http::end_headers(out)
        out.put_all(&page)
        return
    }

    if r.is_get() && path_is(b, r, "/v1/streams") {
        streams_route(spec, b, r, alive, out)
        return
    }

    if r.is_get() && path_is(b, r, "/v1/labels") {
        labels_route(spec, b, r, alive, out)
        return
    }

    if r.is_get() && path_is(b, r, "/v1/query") {
        query_route(spec, b, r, alive, out)
        return
    }

    if r.is_post() && path_is(b, r, "/v1/ingest") {
        ingest_route(b, r, st, w, ms, gens, alive, out)
        return
    }

    if r.is_post() {
        val admin = path_is(b, r, "/v1/admin/repair")
            || path_is(b, r, "/v1/admin/gc")
            || path_is(b, r, "/v1/admin/compact")
            || path_is(b, r, "/v1/admin/flush")
            || path_is(b, r, "/v1/admin/shutdown")
        if admin {
            if !local {
                http::respond_error(out, 403u64, "admin is loopback only", "", alive)
                return
            }
            if path_is(b, r, "/v1/admin/shutdown") {
                st.shutdown = true
                http::respond_text(out, 200u64, "application/json", "{{\u{22}stopping\u{22}:true}}\n", false)
                return
            }
            if path_is(b, r, "/v1/admin/flush") {
                val crc2 = Crc32::new()
                val held = w.count()
                val wrote = flush_active(w, ms, gens, st, &crc2)
                var body2 = ByteWriter::with_capacity(128u64)
                body2.put_str("{{\u{22}records\u{22}:")
                body2.put_str("{held}")
                body2.put_str(",\u{22}bytes\u{22}:")
                body2.put_str("{wrote}")
                body2.put_str("}}\n")
                http::begin_response(out, 200u64, "application/json", body2.len(), alive)
                http::end_headers(out)
                out.put_all(&body2)
                return
            }
            var body = ByteWriter::with_capacity(256u64)
            var code: u64 = 200u64
            if path_is(b, r, "/v1/admin/repair") {
                code = admin_repair(spec, &mut body)
            } elif path_is(b, r, "/v1/admin/compact") {
                code = admin_compact(spec, &mut body)
            } else {
                var days: u64 = 14u64
                var arg = ByteWriter::with_capacity(32u64)
                if http::query_param(b, r.query_at, r.query_len, "days", &mut arg) {
                    days = number_of(&arg) ?? days
                }
                code = admin_gc(spec, days, &mut body)
            }
            if code != 200u64 {
                http::respond_error(out, code, "no readable mount", "", alive)
                return
            }
            http::begin_response(out, 200u64, "application/json", body.len(), alive)
            http::end_headers(out)
            out.put_all(&body)
            return
        }
    }

    http::respond_error(out, 404u64, "no such endpoint", "", alive)
}


fn text_of_writer(w: &ByteWriter) -> String {
    var t = String::new()
    var i: u64 = 0u64
    while i < w.len() {
        t.push(w.byte_at(i))
        i = i + 1u64
    }
    t
}

# Whether any whitespace-separated token is `top=...`.
fn has_top(text: &String) -> bool {
    val n = text.len()
    var at: u64 = 0u64
    while at < n {
        val c: u8 = text.get(at)
        if c == ' ' || c == '\t' {
            at = at + 1u64
        } else {
            var end = at
            var scanning = true
            while scanning && end < n {
                val d: u8 = text.get(end)
                if d == ' ' || d == '\t' { scanning = false } else { end = end + 1u64 }
            }
            if end >= at + 4u64 {
                val head = text.substring(at, at + 4u64)
                val want = String::from_str("top=")
                if head.eq(&want) { return true }
            }
            at = end + 1u64
        }
    }
    false
}

# `GET /v1/labels` -- what can be filtered on, and what the values
# are.
#
# With no `name`, the keys the dictionaries hold; with `?name=host`,
# the values under that key. Both are counted by records.
#
# **Answered from the per-mount label dictionary** (`src/labels.t`),
# so nothing under `seg/` is opened -- HTTP_API.md section 2. The
# dictionary is kept current one segment at a time (a write folds
# its terms in, retention takes them out), and it is a cache, so a
# mount that has none yet falls back to reading the segments' term
# sections, which is what this route used to do for every request.
# The fallback also answers for a mount written before the
# dictionary existed; one `catalog <spec> repair` retires it.
fn labels_route(spec: str, b: Span<u8>, r: &Request, alive: bool,
                out: &mut ByteWriter) {
    var limit: u64 = 200u64
    var lbuf = ByteWriter::with_capacity(32u64)
    if http::query_param(b, r.query_at, r.query_len, "limit", &mut lbuf) {
        val got = number_of(&lbuf)
        var parsed: u64 = 0u64
        match got {
            Result::Ok(n) => { parsed = n }
            Result::Err(e) => { parsed = 0u64 }
        }
        if parsed == 0u64 || parsed > 1000u64 {
            http::respond_error(out, 400u64, "bad parameter",
                                "limit: a number from 1 to 1000", alive)
            return
        }
        limit = parsed
    }

    var namebuf = ByteWriter::with_capacity(64u64)
    val named = http::query_param(b, r.query_at, r.query_len, "name", &mut namebuf)
    val name = text_of_writer(&namebuf)
    if named {
        if !query::is_index_key(&name) {
            http::respond_error(out, 400u64, "bad parameter",
                                "name: a label key is [a-z0-9_], 1 to 32 bytes",
                                alive)
            return
        }
    }

    var ms = MountSet::new()
    var usable = mount::open_spec(spec, &mut ms)
    if usable { usable = mount::readable(&ms) > 0u64 }
    if !usable {
        http::respond_error(out, 503u64, "no readable mount", "", alive)
        return
    }
    var segs: Vec<String> = Vec::new()
    mount::segments_in(&ms, &mut segs)

    val crc = Crc32::new()

    # The answer: a name and a record count per row, however it was
    # obtained. `names` / `counts` are filled either from the
    # dictionary or, when a mount has none, from the segments.
    var names: Vec<String> = Vec::new()
    var counts: Vec<u64> = Vec::new()
    var segs_read: u64 = 0u64
    var from_dict = true
    var mi: u64 = 0u64
    while mi < ms.size() {
        val mp = ms.path_of(mi)
        val mps = mp.to_str()
        if labels::has_dict(mps) {
            collect_from_dict(mps, named, &name, &crc, &mut names, &mut counts)
        } else {
            from_dict = false
        }
        mi = mi + 1u64
    }
    if !from_dict {
        # No dictionary on at least one mount: fall back to the walk
        # this route used to do, for all of them, so the answer is
        # not half of each.
        names.clear()
        counts.clear()
        var prefix = ""
        if named { prefix = "{name}:" }
        val tal = query::tally(&segs, prefix, !named, &crc)
        segs_read = tal.segments
        var ti: u64 = 0u64
        while ti < tal.size() {
            val nm: &String = tal.names.borrow(ti)
            val copy = nm.clone()
            names.push(copy)
            counts.push(tal.counts.get(ti))
            ti = ti + 1u64
        }
    }

    # Biggest first: a list of labels is read top-down, and the one
    # with a million records is the one being looked for.
    var order: Vec<Tally> = Vec::new()
    var i: u64 = 0u64
    while i < names.size() {
        val c: u64 = counts.get(i)
        val one = Tally { count: c, idx: i }
        order.push(one)
        i = i + 1u64
    }
    order.sort()

    var body = ByteWriter::with_capacity(4096u64)
    if named {
        body.put_str("{{\u{22}name\u{22}:")
        http::put_json_string(&mut body, &name)
        body.put_str(",\u{22}values\u{22}:[")
    } else {
        body.put_str("{{\u{22}labels\u{22}:[")
    }
    val total = order.size()
    var shown: u64 = 0u64
    var k: u64 = 0u64
    while k < total && shown < limit {
        if shown > 0u64 { body.put_u8(',') }
        val t: Tally = order.get(total - 1u64 - k)
        val nm: &String = names.borrow(t.idx)
        body.put_str("{{\u{22}name\u{22}:")
        http::put_json_string(&mut body, &nm)
        body.put_str(",\u{22}records\u{22}:")
        body.put_str("{t.count}")
        body.put_str("}}")
        shown = shown + 1u64
        k = k + 1u64
    }
    body.put_str("],\u{22}distinct\u{22}:")
    body.put_str("{total}")
    body.put_str(",\u{22}shown\u{22}:")
    body.put_str("{shown}")
    body.put_str(",\u{22}segments\u{22}:")
    body.put_str("{segs_read}")
    body.put_str("}}\n")

    http::begin_response(out, 200u64, "application/json", body.len(), alive)
    http::end_headers(out)
    out.put_all(&body)
}

# Merge one mount's dictionary rows into the answer.
#
# With no `name`, the key rows (the ones whose value is empty); with
# `?name=host`, the value rows under that key. A name that appears on
# two mounts is one row with the counts added -- the question is
# about the archive, not about a disk.
fn collect_from_dict(mount: str, named: bool, name: &String, crc: &Crc32,
                     names: &mut Vec<String>, counts: &mut Vec<u64>) {
    val d = labels::load_dict(mount, crc)
    var i: u64 = 0u64
    while i < d.size() {
        val k = d.key_at(i)
        val v = d.value_at(i)
        val is_key_row = v.len() == 0u64
        var take = false
        if named {
            if !is_key_row && k.eq(name) { take = true }
        } elif is_key_row {
            take = true
        }
        if take {
            # The name this row contributes is the value under the
            # key that was asked for, or the key itself. It is built
            # where it is pushed: a binding cannot be handed away
            # from inside a branch it was declared outside of.
            val at = index_of_pair(names, named, &k, &v)
            if at < 0i64 {
                var label = String::new()
                if named { label.push_string(&v) } else { label.push_string(&k) }
                names.push(label)
                counts.push(d.count_at(i))
            } else {
                val j = at as u64
                val have: u64 = counts.get(j)
                counts.set(j, have + d.count_at(i))
            }
        }
        i = i + 1u64
    }
}

# Where this dictionary row's name already sits in the answer, or -1.
fn index_of_pair(names: &Vec<String>, named: bool, k: &String, v: &String) -> i64 {
    var i: u64 = 0u64
    var out: i64 = -1i64
    while i < names.size() && out < 0i64 {
        val n: &String = names.borrow(i)
        var hit = false
        if named { hit = n.eq(v) } else { hit = n.eq(k) }
        if hit { out = i as i64 }
        i = i + 1u64
    }
    out
}

# `GET /v1/streams` -- the label sets that have been seen, and how
# many records each carries.
#
# A stream is a label set (DATA_MODEL.md section 3), which the term
# dictionary cannot answer: it holds pairs, and "how many records
# carry *both* `app=api` and `level=error`" is not a pair. The archive
# writes a table of its own while building the segment (kind 9), so
# this reads one section per segment and expands nothing.
#
# `labels` is an object, the way HTTP_API.md section 2 spells it, so
# a client can index it. A record with no labels at all is a stream
# too -- `{}` -- because dropping it would make the counts stop
# adding up to the number of records.
fn streams_route(spec: str, b: Span<u8>, r: &Request, alive: bool,
                 out: &mut ByteWriter) {
    var limit: u64 = 200u64
    var lbuf = ByteWriter::with_capacity(32u64)
    if http::query_param(b, r.query_at, r.query_len, "limit", &mut lbuf) {
        val got = number_of(&lbuf)
        var parsed: u64 = 0u64
        match got {
            Result::Ok(n) => { parsed = n }
            Result::Err(e) => { parsed = 0u64 }
        }
        if parsed == 0u64 || parsed > 1000u64 {
            http::respond_error(out, 400u64, "bad parameter",
                                "limit: a number from 1 to 1000", alive)
            return
        }
        limit = parsed
    }

    var ms = MountSet::new()
    var usable = mount::open_spec(spec, &mut ms)
    if usable { usable = mount::readable(&ms) > 0u64 }
    if !usable {
        http::respond_error(out, 503u64, "no readable mount", "", alive)
        return
    }
    var segs: Vec<String> = Vec::new()
    mount::segments_in(&ms, &mut segs)

    val crc = Crc32::new()
    val tal = query::streams(&segs, &crc)

    # Biggest first, like the label list: the stream with a million
    # records is the one being looked for.
    var order: Vec<Tally> = Vec::new()
    var i: u64 = 0u64
    while i < tal.size() {
        val c: u64 = tal.counts.get(i)
        val one = Tally { count: c, idx: i }
        order.push(one)
        i = i + 1u64
    }
    order.sort()

    var body = ByteWriter::with_capacity(4096u64)
    body.put_str("{{\u{22}streams\u{22}:[")
    val total = order.size()
    var shown: u64 = 0u64
    var k: u64 = 0u64
    while k < total && shown < limit {
        if shown > 0u64 { body.put_u8(',') }
        val t: Tally = order.get(total - 1u64 - k)
        val text: &String = tal.texts.borrow(t.idx)
        body.put_str("{{\u{22}labels\u{22}:")
        put_label_object(&mut body, &text)
        body.put_str(",\u{22}records\u{22}:")
        body.put_str("{t.count}")
        val lo: i64 = tal.ts_min.get(t.idx)
        val hi: i64 = tal.ts_max.get(t.idx)
        if lo <= hi {
            val from = DateTime::from_unix(lo)
            val until = DateTime::from_unix(hi)
            body.put_str(",\u{22}ts_min\u{22}:\u{22}")
            body.put_str(time::format(from, "%Y-%m-%dT%H:%M:%SZ"))
            body.put_str("\u{22},\u{22}ts_max\u{22}:\u{22}")
            body.put_str(time::format(until, "%Y-%m-%dT%H:%M:%SZ"))
            body.put_str("\u{22}")
        }
        body.put_str("}}")
        shown = shown + 1u64
        k = k + 1u64
    }
    val segs_read = tal.segments
    body.put_str("],\u{22}distinct\u{22}:")
    body.put_str("{total}")
    body.put_str(",\u{22}shown\u{22}:")
    body.put_str("{shown}")
    body.put_str(",\u{22}segments\u{22}:")
    body.put_str("{segs_read}")
    body.put_str("}}\n")

    http::begin_response(out, 200u64, "application/json", body.len(), alive)
    http::end_headers(out)
    out.put_all(&body)
}

# `key=value key=value` as a JSON object. The text comes from the
# segment, so the values are whatever was logged -- quoting is
# `put_json_string`'s job, not this one's.
fn put_label_object(body: &mut ByteWriter, text: &String) {
    body.put_u8('{')
    val n = text.len()
    var at: u64 = 0u64
    var written: u64 = 0u64
    while at < n {
        var eq = at
        while eq < n && text.get(eq) != '=' { eq = eq + 1u64 }
        var stop = eq
        while stop < n && text.get(stop) != ' ' { stop = stop + 1u64 }
        if eq < n && stop > eq + 1u64 {
            if written > 0u64 { body.put_u8(',') }
            val key = text.substring(at, eq)
            val value = text.substring(eq + 1u64, stop)
            http::put_json_string(body, &key)
            body.put_u8(':')
            http::put_json_string(body, &value)
            written = written + 1u64
        }
        at = stop + 1u64
    }
    body.put_u8('}')
}

# `GET /v1/query` -- the same search the terminal runs, rendered for a
# client instead of a person.
#
# The parameters are checked before a segment is opened. A `limit`
# that is not a number is the client's mistake and costs nothing to
# say so; finding out after a 300 ms walk would be the same answer,
# later.
fn query_route(spec: str, b: Span<u8>, r: &Request, alive: bool,
               out: &mut ByteWriter) {
    var qbuf = ByteWriter::with_capacity(512u64)
    if !http::query_param(b, r.query_at, r.query_len, "q", &mut qbuf) {
        http::respond_error(out, 400u64, "bad parameter", "q: missing", alive)
        return
    }
    val text = text_of_writer(&qbuf)

    # `top=<field>` asks for a distribution, not for records, and the
    # tally path lives only in the command line so far. Left alone it
    # is **dropped** by the query parser: the answer comes back with
    # every record in the archive, which reads as a result rather than
    # as a missing feature. Refusing says which one it is.
    #
    # With the other parameter checks, and before a mount is opened:
    # a request that cannot be served should not cost a disk read,
    # and "no readable mount" would otherwise answer first and hide
    # this.
    if has_top(&text) {
        http::respond_error(out, 400u64, "unsupported parameter",
                            "top=: distributions are command-line only for now",
                            alive)
        return
    }

    var limit: u64 = 50u64
    var lbuf = ByteWriter::with_capacity(32u64)
    if http::query_param(b, r.query_at, r.query_len, "limit", &mut lbuf) {
        val got = number_of(&lbuf)
        var parsed: u64 = 0u64
        match got {
            Result::Ok(n) => { parsed = n }
            Result::Err(e) => { parsed = 0u64 }
        }
        # The cap is the response budget, not a preference: the whole
        # body is built in memory before any of it is written.
        if parsed == 0u64 || parsed > 1000u64 {
            http::respond_error(out, 400u64, "bad parameter",
                                "limit: a number from 1 to 1000", alive)
            return
        }
        limit = parsed
    }

    var fmt = query::format_ndjson()
    var fbuf = ByteWriter::with_capacity(32u64)
    if http::query_param(b, r.query_at, r.query_len, "format", &mut fbuf) {
        val f = text_of_writer(&fbuf)
        val as_json = String::from_str("json")
        val as_text = String::from_str("text")
        val as_nd = String::from_str("ndjson")
        if f.eq(&as_json) {
            fmt = query::format_json()
        } elif f.eq(&as_text) {
            fmt = query::format_text()
        } elif f.eq(&as_nd) {
            fmt = query::format_ndjson()
        } else {
            http::respond_error(out, 400u64, "bad parameter",
                                "format: json, ndjson or text", alive)
            return
        }
    }

    # **An archive with nothing in it is not a broken archive.**
    # 503 is for a spec that named no mount this process can read; a
    # mount that opened and holds no segments yet answers 200 with no
    # records, and `segments_considered: 0` in the statistics is what
    # says which of the two happened. Collapsing them sends someone to
    # check disk permissions when the truth is that nothing has been
    # archived -- which is what `serve` with no argument does, since
    # the default spec is an empty `/tmp/logarchive`.
    var ms = MountSet::new()
    var usable = mount::open_spec(spec, &mut ms)
    if usable { usable = mount::readable(&ms) > 0u64 }
    if !usable {
        http::respond_error(out, 503u64, "no readable mount", "", alive)
        return
    }
    var segs: Vec<String> = Vec::new()
    mount::segments_in(&ms, &mut segs)

    val now = time::now_unix_secs()
    var q = query::parse_query(text.to_str(), now)
    q.limit = limit

    val crc = Crc32::new()
    var hits: Vec<Hit> = Vec::new()
    var texts: Vec<String> = Vec::new()
    var qst = SearchStats::new()
    query::search(spec, &segs, &q, &crc, &mut hits, &mut texts, &mut qst)

    var body = ByteWriter::with_capacity(65536u64)
    var ctype = "application/x-ndjson"
    if fmt == query::format_json() {
        ctype = "application/json"
        val n = query::render_json(&hits, &texts, &q, &qst, &mut body)
    } elif fmt == query::format_text() {
        ctype = "text/plain"
        val n = query::render_text(&hits, &texts, &q, &mut body)
    } else {
        val n = query::render_ndjson(&hits, &texts, &q, &mut body)
    }

    # A search that ran out of budget is still an answer: the client
    # is told in the statistics, not by an error code.
    http::begin_response(out, 200u64, ctype, body.len(), alive)
    http::end_headers(out)
    out.put_all(&body)
}

fn number_of(w: &ByteWriter) -> Result<u64, ParseError> {
    var text = String::new()
    var i: u64 = 0u64
    while i < w.len() {
        text.push(w.byte_at(i))
        i = i + 1u64
    }
    # Bound rather than returned directly: a compound-returning
    # module call cannot sit in an expression position.
    val got = parse::to_u64(text.to_str())
    got
}

# ---------------------------------------------------------------------
# One connection, start to finish

# Serve `conn` until it closes, times out, or asks the server to stop.
# Returns false when the server should stop.
# ---------------------------------------------------------------------
# The connection table
#
# Parallel columns rather than a `Vec<Conn>`: a struct per connection
# would put nine values behind one index, and every turn reads two or
# three of them. The columns are also what let the handle column be
# the only owning one.
#
# `sock` owns every handle it holds. A turn `borrow`s the slot and
# reads or writes through the reference — `read` / `write` /
# `shutdown_write` all take `&self` — and `conn_close` takes the
# handle back with `Vec::replace`, which is what closes it.
pub struct Conns {
    sock: Vec<Option<TcpStream>>,   # None = free slot
    writing: Vec<bool>,
    local: Vec<bool>,
    in_len: Vec<u64>,
    out_len: Vec<u64>,
    sent: Vec<u64>,
    deadline: Vec<u64>,
    # One request that is parsed but not answered yet, because the
    # shared `big` buffer was busy. Retried on the next tick.
    pending: Vec<bool>,
    inbox: ByteWriter,     # max_conns * recv_slot_bytes()
    outbox: ByteWriter,    # max_conns * send_slot_bytes()
    # Which slot owns the shared oversize buffer, if any. The buffer
    # itself is a separate binding the caller holds: a field cannot be
    # handed to a function as `&mut` on the compiled lanes
    # (`RUNTIME_GAPS.md` G16), and every route needs it as one.
    big_owner: i64,        # -1 = free
    live: u64,
}

impl Conns {
    pub fn new() -> Self {
        val n = max_conns()
        var socks: Vec<Option<TcpStream>> = Vec::with_capacity(n)
        var wr: Vec<bool> = Vec::with_capacity(n)
        var lo: Vec<bool> = Vec::with_capacity(n)
        var il: Vec<u64> = Vec::with_capacity(n)
        var ol: Vec<u64> = Vec::with_capacity(n)
        var sn: Vec<u64> = Vec::with_capacity(n)
        var dl: Vec<u64> = Vec::with_capacity(n)
        var pd: Vec<bool> = Vec::with_capacity(n)
        var i: u64 = 0u64
        while i < n {
            val free: Option<TcpStream> = Option::None
            socks.push(free)
            wr.push(false)
            lo.push(false)
            il.push(0u64)
            ol.push(0u64)
            sn.push(0u64)
            dl.push(0u64)
            pd.push(false)
            i = i + 1u64
        }
        val ib = ByteWriter::with_capacity(n * recv_slot_bytes())
        val ob = ByteWriter::with_capacity(n * send_slot_bytes())
        Conns {
            sock: socks, writing: wr, local: lo,
            in_len: il, out_len: ol, sent: sn, deadline: dl,
            pending: pd, inbox: ib, outbox: ob,
            big_owner: -1i64, live: 0u64,
        }
    }

    pub fn live(&self) -> u64 { self.live }
    pub fn is_full(&self) -> bool { self.live >= max_conns() }

    # The descriptor a slot holds, or -1 when the slot is free.
    #
    # The poller speaks in descriptors, so every turn needs this even
    # though the table holds handles. Borrowing the slot answers it
    # without taking the handle out.
    pub fn fd_of(&self, slot: u64) -> i32 {
        val held: &Option<TcpStream> = self.sock.borrow(slot)
        var out: i32 = -1i32
        match held {
            Option::Some(s) => { out = s.as_fd() }
            Option::None => { }
        }
        out
    }

    # The first free slot, or -1.
    fn free_slot(&self) -> i64 {
        var i: u64 = 0u64
        var out: i64 = -1i64
        while i < max_conns() && out < 0i64 {
            if self.fd_of(i) < 0i32 { out = i as i64 }
            i = i + 1u64
        }
        out
    }
}

# Take a descriptor into the table. Answers the slot, or -1 when full.
#
# `conn_open` / `serve_slot` / `conn_close` are public because the
# tests drive a table without a `serve` loop around it: starting the
# real loop would mean waiting on its own poller, and a test that
# waits is a test that hangs when it breaks.
# **The handle is handed over**, and the table closes it from then on.
#
# The socket goes into the slot **before** the poller is asked, and a
# refusal takes it back out again. That order is not decoration: a
# by-value parameter registers no drop, so a path that neither stores
# the handle nor closes it would leak the descriptor — and the move
# check refuses to hand it away from inside a branch, because whether
# the binding still owns anything would depend on the path. One
# unconditional move, and the undo goes through the table.
#
# The caller checks `is_full()` first. Arriving here with no free slot
# is a bug in the caller, not a runtime condition, so it says so.
pub fn conn_open(c: &mut Conns, poller: &Poller, sock: TcpStream,
             local: bool) -> i64 {
    val fd = sock.as_fd()
    val got = c.free_slot()
    if got < 0i64 { panic("conn_open: no free slot; the caller must check is_full()") }
    val slot = got as u64
    val held: Option<TcpStream> = Option::Some(sock)
    c.sock.set(slot, held)
    val reg = poller.register(fd, conn_token(slot), interest_read())
    match reg {
        Result::Ok(u) => { }
        Result::Err(e) => {
            val free: Option<TcpStream> = Option::None
            val back: Option<TcpStream> = c.sock.replace(slot, free)
            match back {
                Option::Some(s) => { }
                Option::None => { }
            }
            return -1i64
        }
    }
    c.local.set(slot, local)
    c.writing.set(slot, false)
    c.in_len.set(slot, 0u64)
    c.out_len.set(slot, 0u64)
    c.sent.set(slot, 0u64)
    c.pending.set(slot, false)
    c.deadline.set(slot, time::now_mono_ns() + header_timeout_ns())
    c.live = c.live + 1u64
    got
}

# Give the slot back, closing the handle it held.
pub fn conn_close(c: &mut Conns, poller: &Poller, slot: u64, big: &mut ByteWriter) {
    val fd = c.fd_of(slot)
    if fd < 0i32 { return }
    val off = poller.deregister(fd)
    match off {
        Result::Ok(u) => { }
        Result::Err(e) => { }
    }
    # Taking the handle out of the slot makes this binding its owner,
    # and the drop at the end of the function closes the descriptor.
    # `set` would not do: it overwrites, and the handle it covered
    # would never be closed.
    val free: Option<TcpStream> = Option::None
    val was: Option<TcpStream> = c.sock.replace(slot, free)
    match was {
        Option::Some(s) => { }
        Option::None => { }
    }
    c.writing.set(slot, false)
    c.pending.set(slot, false)
    c.in_len.set(slot, 0u64)
    c.out_len.set(slot, 0u64)
    c.sent.set(slot, 0u64)
    if c.big_owner == (slot as i64) {
        c.big_owner = -1i64
        big.clear()
    }
    c.live = c.live - 1u64
}

# Read what is waiting into this slot's region. `false` means the
# connection is finished (closed, failed, or over its limit).
fn conn_read(c: &mut Conns, slot: u64, st: &mut Stats) -> bool {
    val base = slot * recv_slot_bytes()
    val have: u64 = c.in_len.get(slot)
    if have >= recv_slot_bytes() { return false }
    val room = c.inbox.room()
    var out = true
    match room {
        Option::Some(all) => {
            val win = all.slice(base + have, recv_slot_bytes() - have)
            # Borrowed, not taken: `read` is `&self`, and the table
            # goes on owning the handle.
            val held: &Option<TcpStream> = c.sock.borrow(slot)
            match held {
                Option::Some(s) => {
                    val got = s.read(win)
                    match got {
                        Result::Ok(n) => {
                            if n == 0u64 {
                                out = false
                            } else {
                                c.in_len.set(slot, have + n)
                                st.bytes_in = st.bytes_in + n
                            }
                        }
                        Result::Err(e) => {
                            match e {
                                NetError::WouldBlock => { }
                                NetError::Interrupted => { }
                                _ => { out = false }
                            }
                        }
                    }
                }
                Option::None => { out = false }
            }
        }
        Option::None => { out = false }
    }
    out
}

# Copy a finished response into the slot's region, or keep it in the
# shared buffer when it does not fit.
fn conn_hold_response(c: &mut Conns, slot: u64, big: &ByteWriter) {
    val n = big.len()
    if n <= send_slot_bytes() {
        val src = big.span()
        val dst = c.outbox.room()
        match src {
            Option::Some(srcw) => {
                match dst {
                    Option::Some(dstw) => {
                        val base = slot * send_slot_bytes()
                        val win = dstw.slice(base, n)
                        win.copy_from(srcw)
                    }
                    Option::None => { }
                }
            }
            Option::None => { }
        }
        c.out_len.set(slot, n)
        c.big_owner = -1i64
    } else {
        # Sent straight out of `big`; nobody else may route until it
        # has gone.
        c.out_len.set(slot, n)
        c.big_owner = slot as i64
    }
}

# If this slot holds a complete request, answer it. `false` means the
# connection is finished.
fn conn_route(c: &mut Conns, slot: u64, poller: &Poller, spec: str,
              st: &mut Stats, w: &mut ArchiveWriter, ms: &mut MountSet,
              gens: &Vec<u64>, big: &mut ByteWriter) -> bool {
    val have: u64 = c.in_len.get(slot)
    if have == 0u64 { return true }
    # Another connection is still sending a big answer. Try next tick
    # rather than dropping the request.
    if c.big_owner >= 0i64 && c.big_owner != (slot as i64) {
        c.pending.set(slot, true)
        return true
    }
    val base = slot * recv_slot_bytes()
    val room = c.inbox.room()
    var out = true
    match room {
        Option::Some(all) => {
            val bytes = all.slice(base, have)
            val req = http::parse_request(bytes, have)
            if req.status != 0u64 || req.complete {
                val local: bool = c.local.get(slot)
                big.clear()
                route(spec, bytes, &req, local, st, w, ms, gens, big)
                conn_hold_response(c, slot, big)
                c.pending.set(slot, false)
                c.writing.set(slot, true)
                c.sent.set(slot, 0u64)
                c.in_len.set(slot, 0u64)
                val fd = c.fd_of(slot)
                val up = poller.register(fd, conn_token(slot), interest_write())
                match up {
                    Result::Ok(u) => { }
                    Result::Err(e) => { out = false }
                }
                c.deadline.set(slot, time::now_mono_ns() + idle_timeout_ns())
            } elif have >= recv_slot_bytes() {
                # The slot filled without a request ending in it.
                out = false
            } else {
                c.pending.set(slot, false)
            }
        }
        Option::None => { out = false }
    }
    out
}

# Push out what is left of this slot's response. `false` means the
# connection is finished.
fn conn_write(c: &mut Conns, slot: u64, poller: &Poller, st: &mut Stats,
              big: &ByteWriter) -> bool {
    val total: u64 = c.out_len.get(slot)
    val sent: u64 = c.sent.get(slot)
    if sent >= total { return true }
    val from_big = c.big_owner == (slot as i64)
    var window = big.span()
    if !from_big { window = c.outbox.room() }
    var base = 0u64
    if !from_big { base = slot * send_slot_bytes() }
    var out = true
    match window {
        Option::Some(all) => {
            val piece = all.slice(base + sent, total - sent)
            val held: &Option<TcpStream> = c.sock.borrow(slot)
            match held {
                Option::Some(s) => {
                    val put = s.write(piece)
                    match put {
                        Result::Ok(n) => {
                            c.sent.set(slot, sent + n)
                            st.bytes_out = st.bytes_out + n
                        }
                        Result::Err(e) => {
                            match e {
                                NetError::WouldBlock => { }
                                NetError::Interrupted => { }
                                _ => { out = false }
                            }
                        }
                    }
                }
                Option::None => { out = false }
            }
        }
        Option::None => { out = false }
    }
    out
}

# The response has gone out. Either park the slot for the next request
# or finish the connection.
fn conn_sent_all(c: &mut Conns, slot: u64, poller: &Poller,
                 big: &mut ByteWriter) -> bool {
    var keep = false
    if c.big_owner == (slot as i64) {
        keep = keep_open(big)
        big.clear()
        c.big_owner = -1i64
    } else {
        val base = slot * send_slot_bytes()
        val n: u64 = c.out_len.get(slot)
        val room = c.outbox.room()
        match room {
            Option::Some(all) => {
                val win = all.slice(base, n)
                keep = keep_open_bytes(win, n)
            }
            Option::None => { }
        }
    }
    if !keep { return false }
    c.writing.set(slot, false)
    c.sent.set(slot, 0u64)
    c.out_len.set(slot, 0u64)
    c.in_len.set(slot, 0u64)
    val fd = c.fd_of(slot)
    val back = poller.register(fd, conn_token(slot), interest_read())
    match back {
        Result::Ok(u) => { }
        Result::Err(e) => { return false }
    }
    c.deadline.set(slot, time::now_mono_ns() + idle_timeout_ns())
    true
}

# Requests that were parsed while the shared buffer was busy.
#
# An event already came and went for these, so nothing else will wake
# them: the sweep is what keeps a deferred request from waiting for a
# client that has no reason to send anything more.
fn sweep_pending(c: &mut Conns, poller: &Poller, spec: str, st: &mut Stats,
                 w: &mut ArchiveWriter, ms: &mut MountSet, gens: &Vec<u64>,
                 big: &mut ByteWriter) {
    if c.big_owner >= 0i64 { return }
    var i: u64 = 0u64
    while i < max_conns() {
        val waiting: bool = c.pending.get(i)
        if c.fd_of(i) >= 0i32 && waiting {
            if !conn_route(c, i, poller, spec, st, w, ms, gens, big) {
                conn_close(c, poller, i, big)
            }
        }
        i = i + 1u64
    }
}

# Connections that stopped talking. The header timeout answers
# slow-loris; the idle timeout reclaims a keep-alive nobody is using.
fn sweep_deadlines(c: &mut Conns, poller: &Poller, big: &mut ByteWriter) {
    val now = time::now_mono_ns()
    var i: u64 = 0u64
    while i < max_conns() {
        if c.fd_of(i) >= 0i32 {
            val due: u64 = c.deadline.get(i)
            if now > due { conn_close(c, poller, i, big) }
        }
        i = i + 1u64
    }
}

# Serve exactly one connection until it is done, on a table of one.
#
# The server proper (`serve`) runs many slots at once; this is the
# single-connection driver, kept because a caller that has already
# accepted a socket -- a test, or a one-shot tool -- should not have
# to build a table. **The socket is handed over**: the table owns what
# it holds, so the connection is closed when it is finished with.
#
# Answers false when the request asked the server to stop.
pub fn serve_connection(poller: &Poller, conn: TcpStream, spec: str,
                        st: &mut Stats, w: &mut ArchiveWriter,
                        ms: &mut MountSet, gens: &Vec<u64>,
                        inbox: &mut ByteWriter,
                        outbox: &mut ByteWriter) -> bool {
    var c = Conns::new()
    val local = is_local(&conn)
    val got = conn_open(&mut c, poller, conn, local)
    if got < 0i64 { return true }
    val slot = got as u64
    st.connections = st.connections + 1u64

    var running = true
    var alive = true
    while alive {
        val now = time::now_mono_ns()
        val due: u64 = c.deadline.get(slot)
        if now > due {
            alive = false
        } else {
            val ready = poller.wait(tick_ms())
            var n: u64 = 0u64
            match ready {
                Result::Ok(k) => { n = k }
                Result::Err(e) => { alive = false }
            }
            var i: u64 = 0u64
            while i < n && alive {
                val ev = poller.event(i)
                if ev.token() == conn_token(slot) {
                    val bad = ev.is_error() || ev.is_hup()
                    if !serve_slot(&mut c, slot, ev.is_readable(), ev.is_writable(),
                                   bad, poller, spec, st, w, ms, gens, outbox) {
                        alive = false
                    }
                    val writing: bool = c.writing.get(slot)
                    val sent: u64 = c.sent.get(slot)
                    val total: u64 = c.out_len.get(slot)
                    if alive && writing && sent >= total && st.shutdown {
                        alive = false
                        running = false
                    }
                }
                i = i + 1u64
            }
        }
    }
    conn_close(&mut c, poller, slot, outbox)
    running
}

# One event on one slot. `false` means the connection is finished.
#
# The order is the one an event loop has to keep: drain a peer that
# hung up before believing it, write before reading (a response half
# out is the thing holding the connection), and only then take more
# bytes in.
pub fn serve_slot(c: &mut Conns, slot: u64, readable: bool, writable: bool,
              gone: bool, poller: &Poller,
              spec: str, st: &mut Stats, w: &mut ArchiveWriter,
              ms: &mut MountSet, gens: &Vec<u64>, big: &mut ByteWriter) -> bool {
    var alive = true
    val writing: bool = c.writing.get(slot)
    if gone {
        if !writing {
            val more = conn_read(c, slot, st)
            val have: u64 = c.in_len.get(slot)
            if have == 0u64 { alive = false }
        }
    }
    if alive && writing && writable {
        if !conn_write(c, slot, poller, st, big) {
            alive = false
        } else {
            val sent: u64 = c.sent.get(slot)
            val total: u64 = c.out_len.get(slot)
            if sent >= total {
                if !conn_sent_all(c, slot, poller, big) { alive = false }
            }
        }
    }
    val still_writing: bool = c.writing.get(slot)
    if alive && !still_writing && readable {
        if !conn_read(c, slot, st) {
            alive = false
        } else {
            if !conn_route(c, slot, poller, spec, st, w, ms, gens, big) { alive = false }
        }
    }
    alive
}

# Whether the response that was just written asked to keep going.
# Reading it back out of the bytes rather than carrying a flag keeps
# one answer: what the client was told is what happens.
fn keep_open(out: &ByteWriter) -> bool {
    val w = out.span()
    match w {
        Option::Some(b) => { keep_open_bytes(b, out.len()) }
        Option::None => { false }
    }
}

# The same question asked of a window, for a response that lives in a
# slot of the table rather than in a buffer of its own.
fn keep_open_bytes(b: Span<u8>, len: u64) -> bool {
    val needle = String::from_str("\r\nconnection: keep-alive\r\n")
    val win = b.slice(0u64, len)
    val at = win.find_seq(span_of_string(&needle))
    match at {
        Option::Some(i) => { true }
        Option::None => { false }
    }
}

fn span_of_string(s: &String) -> Span<u8> {
    val w = s.as_span()
    match w {
        Option::Some(sp) => sp,
        Option::None => { panic("span_of_string: empty") }
    }
}

# ---------------------------------------------------------------------

# Bind, then serve until asked to stop or until `idle_s` passes with
# nobody connecting. An `idle_s` of 0 means "until told to stop".
pub fn serve(spec: str, addr: str, port: u64, idle_s: u64) -> u64 {
    val bound = TcpListener::bind(addr, port)
    var listener = match bound {
        Result::Ok(l) => l,
        Result::Err(e) => { eprintln("cannot bind: {e}")  return 1u64 }
    }
    val got_port = listener.local_port()
    val real_port = match got_port {
        Result::Ok(n) => n,
        Result::Err(e) => { eprintln("cannot read the port: {e}")  return 1u64 }
    }

    val made = Poller::new()
    var poller = match made {
        Result::Ok(p) => p,
        Result::Err(e) => { eprintln("cannot create a poller: {e}")  return 1u64 }
    }
    val watch = poller.register(listener.as_fd(), listener_token(), interest_read())
    match watch {
        Result::Ok(u) => { }
        Result::Err(e) => { eprintln("cannot watch the listener: {e}")  return 1u64 }
    }

    # The buffers are taken once, here, and reused for every
    # connection: that is the whole memory discipline (MEMORY.md
    # section 3), and `live_bytes` in `/v1/stats` is what says whether
    # it held.
    var inbox = ByteWriter::with_capacity(65536u64)
    var outbox = ByteWriter::with_capacity(send_bytes())
    var st = Stats::new()

    # The active segment, and the mounts it can be written to.
    #
    # **Ingest is only enabled when the mount directories already
    # exist.** Creating them here would mean a typo in the spec
    # silently becomes a new archive, and would hide the difference
    # between "nothing readable" and "nothing yet" that the query
    # path depends on. An operator makes the directory; this fills
    # it.
    var w = ArchiveWriter::new()
    var ms = MountSet::new()
    var gens: Vec<u64> = Vec::new()
    val crc = Crc32::new()
    st.ready = store::open_for_write(spec, &mut ms, &mut gens, &crc, false, true)
    if st.ready { st.segid = store::next_segid(&ms, &crc) }
    if !st.ready {
        eprintln("ingest is off: no writable mount under {spec}")
    }
    var last_flush = time::now_mono_ns()

    # The port goes to stderr so a caller can read it while stdout
    # stays whatever the server prints about its work. Binding port 0
    # and reading it back is how the tests avoid naming a number.
    eprintln("listening on {addr}:{real_port}")

    var conns = Conns::new()
    var idle_ns: u64 = 0u64
    val budget_ns = idle_s * 1000000000u64
    var running = true
    var watching = true          # is the listener still in the poller?
    while running {
        val ready = poller.wait(tick_ms())
        var n: u64 = 0u64
        match ready {
            Result::Ok(k) => { n = k }
            Result::Err(e) => { running = false }
        }
        if running {
            if n == 0u64 && conns.live() == 0u64 {
                idle_ns = idle_ns + ((tick_ms() as u64) * 1000000u64)
                if budget_ns > 0u64 && idle_ns >= budget_ns { running = false }
            } else {
                idle_ns = 0u64
            }

            var i: u64 = 0u64
            while i < n && running {
                val ev = poller.event(i)
                val tok = ev.token()
                if tok == listener_token() {
                    # Take as many as the table will hold. A slot that
                    # cannot be opened means the listener comes out of
                    # the poller until one frees up -- the clients wait
                    # in the TCP backlog, which is easier on a sender
                    # than an accept followed by a close.
                    var taking = true
                    while taking {
                        if conns.is_full() {
                            taking = false
                        } else {
                            # `accept_fd` rather than `accept`: the
                            # handle is built here, inside the arm,
                            # from the number. Binding the accepted
                            # socket out of the `Result` instead would
                            # put the arm's alias and the temporary
                            # `Result` in the same conversation about
                            # who closes it, and there is nothing to
                            # gain from having it.
                            val accepted = listener.accept_fd()
                            match accepted {
                                Result::Ok(fd) => {
                                    var sock = TcpStream::from_fd(fd)
                                    val local = is_local(&sock)
                                    # The table takes the handle. A
                                    # full table closes it for us.
                                    val slot = conn_open(&mut conns, &poller, sock, local)
                                    if slot < 0i64 {
                                        taking = false
                                    } else {
                                        st.connections = st.connections + 1u64
                                    }
                                }
                                Result::Err(e) => { taking = false }
                            }
                        }
                    }
                } else {
                    val slot = token_slot(tok)
                    if conns.fd_of(slot) >= 0i32 {
                        val bad = ev.is_error() || ev.is_hup()
                        val keep = serve_slot(&mut conns, slot, ev.is_readable(),
                                              ev.is_writable(), bad, &poller, spec,
                                              &mut st, &mut w, &mut ms, &gens,
                                              &mut outbox)
                        if !keep { conn_close(&mut conns, &poller, slot, &mut outbox) }
                        if st.shutdown { running = false }
                    }
                }
                i = i + 1u64
            }

            # Requests that had to wait for the shared buffer, and
            # connections that stopped talking.
            if running {
                sweep_pending(&mut conns, &poller, spec, &mut st, &mut w,
                              &mut ms, &gens, &mut outbox)
                sweep_deadlines(&mut conns, &poller, &mut outbox)
            }

            # The listener goes out of the poller while the table is
            # full, and comes back when it is not.
            if running {
                if conns.is_full() && watching {
                    val off = poller.deregister(listener.as_fd())
                    match off {
                        Result::Ok(u) => { watching = false }
                        Result::Err(e) => { }
                    }
                } elif !conns.is_full() && !watching {
                    val on = poller.register(listener.as_fd(), listener_token(), interest_read())
                    match on {
                        Result::Ok(u) => { watching = true }
                        Result::Err(e) => { }
                    }
                }
            }

            # A record that is only in memory is a record a crash
            # loses, so the segment goes out on a timer as well as
            # when it fills (DATA_MODEL.md section 4).
            val now_ns = time::now_mono_ns()
            if !w.is_empty() && now_ns - last_flush >= flush_after_ns() {
                val put = flush_active(&mut w, &mut ms, &gens, &mut st, &crc)
                last_flush = now_ns
            }
        }
    }

    # Whoever is still connected is told nothing: the process is
    # going away, and a half-written answer is worse than none.
    var k: u64 = 0u64
    while k < max_conns() {
        if conns.fd_of(k) >= 0i32 { conn_close(&mut conns, &poller, k, &mut outbox) }
        k = k + 1u64
    }

    # Whatever is still held goes out before the process does. This
    # is step 2 of the stop sequence in ARCHITECTURE.md section 5,
    # and the reason `shutdown` is not just an exit.
    if !w.is_empty() {
        val put = flush_active(&mut w, &mut ms, &gens, &mut st, &crc)
        if put == 0u64 {
            eprintln("the active segment could not be written; its records are lost")
        }
    }
    store::compact_all(&ms, &crc, false)

    val served = st.requests
    val conns = st.connections
    val took = st.accepted
    val segs = st.segments
    eprintln("served {served} request(s) over {conns} connection(s)")
    eprintln("ingested {took} record(s) into {segs} segment(s)")
    0u64
}
