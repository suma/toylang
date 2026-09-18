# The HTTP server: one event loop, fixed buffers, no blocking call
# other than `Poller::wait`.
#
# `http.t` decides what the bytes mean; this file decides when to read
# them, when to write, and when to give up. Keeping the two apart is
# what let the protocol be tested without a socket.
#
# **One connection is served at a time.** That is a real limit, and
# not the one HTTP_API.md section 4 describes (128, with the listener
# unhooked when the table fills). The reason is a language gap rather
# than a decision: a connection table means a container of socket
# handles, `Vec<TcpStream>` hands back an alias whose drop glue closes
# the descriptor, and there is no `TcpStream::from_fd` to keep a table
# of plain numbers instead (RUNTIME_GAPS.md G16). Until one of those
# changes, extra clients wait in the TCP backlog -- which is the
# behaviour the design already asks for when the table is full, so
# raising the number later changes this file and nothing else.
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
fn connection_token() -> u64 { 2u64 }

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
                        val sp: String = paths.get(j)
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
# **This opens every segment's term section**, which is not what
# HTTP_API.md section 2 asks for -- it wants the answer out of a
# per-mount label dictionary in the catalog, so that nothing under
# `seg/` is touched. That dictionary does not exist yet. The cost is
# one section read per segment (24 ms over four segments holding
# 43,813 terms), so it is usable now and will not be at a hundred
# thousand.
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
    var prefix = ""
    if named { prefix = "{name}:" }
    val tal = query::tally(&segs, prefix, !named, &crc)

    # Biggest first: a list of labels is read top-down, and the one
    # with a million records is the one being looked for.
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
        val nm: String = tal.names.get(t.idx)
        body.put_str("{{\u{22}name\u{22}:")
        http::put_json_string(&mut body, &nm)
        body.put_str(",\u{22}records\u{22}:")
        body.put_str("{t.count}")
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
        val text: String = tal.texts.get(t.idx)
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
pub fn serve_connection(poller: &Poller, conn: &TcpStream, spec: str,
                        st: &mut Stats, w: &mut ArchiveWriter,
                        ms: &mut MountSet, gens: &Vec<u64>,
                        inbox: &mut ByteWriter,
                        outbox: &mut ByteWriter) -> bool {
    val fd = conn.as_fd()
    val local = is_local(conn)
    val reg = poller.register(fd, connection_token(), interest_read())
    match reg {
        Result::Ok(u) => { }
        Result::Err(e) => { return true }
    }
    st.connections = st.connections + 1u64

    inbox.clear()
    outbox.clear()
    var sent: u64 = 0u64
    var writing = false
    var alive = true
    var running = true
    var deadline = time::now_mono_ns() + header_timeout_ns()

    while alive {
        val now = time::now_mono_ns()
        if now > deadline {
            # A client that stopped mid-request gets the descriptor
            # back rather than an explanation: there is nowhere to put
            # one that it is listening to.
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
                if ev.token() == connection_token() {
                    if ev.is_error() || ev.is_hup() {
                        # Drain first: a peer that sent a whole
                        # request and closed its side is not an error,
                        # and half-closed uploads are how `curl
                        # --data-binary` finishes.
                        if !writing {
                            val got = recv_some(conn, inbox, recv_bytes())
                            if got > 0i64 { st.bytes_in = st.bytes_in + (got as u64) }
                        }
                        if inbox.len() == 0u64 { alive = false }
                    }
                    if alive && writing && ev.is_writable() {
                        val put = send_some(conn, outbox, sent)
                        if put > 0i64 {
                            sent = sent + (put as u64)
                            st.bytes_out = st.bytes_out + (put as u64)
                        }
                        if put == io_failed() { alive = false }
                        if alive && sent >= outbox.len() {
                            # The response is out. Keep the
                            # connection only if both sides said so.
                            if st.shutdown {
                                alive = false
                                running = false
                            } elif keep_open(outbox) {
                                writing = false
                                sent = 0u64
                                outbox.clear()
                                inbox.clear()
                                val back = poller.register(fd, connection_token(), interest_read())
                                match back {
                                    Result::Ok(u) => { }
                                    Result::Err(e) => { alive = false }
                                }
                                deadline = time::now_mono_ns() + idle_timeout_ns()
                            } else {
                                alive = false
                            }
                        }
                    }
                    if alive && !writing && ev.is_readable() {
                        val got = recv_some(conn, inbox, recv_bytes())
                        if got == io_closed() || got == io_failed() {
                            alive = false
                        } else {
                            if got > 0i64 { st.bytes_in = st.bytes_in + (got as u64) }
                            val seen = inbox.span()
                            match seen {
                                Option::Some(bytes) => {
                                    val req = http::parse_request(bytes, inbox.len())
                                    if req.status != 0u64 || req.complete {
                                        route(spec, bytes, &req, local, st, w, ms, gens, outbox)
                                        writing = true
                                        sent = 0u64
                                        val up = poller.register(fd, connection_token(), interest_write())
                                        match up {
                                            Result::Ok(u) => { }
                                            Result::Err(e) => { alive = false }
                                        }
                                        deadline = time::now_mono_ns() + idle_timeout_ns()
                                    }
                                }
                                Option::None => { }
                            }
                        }
                    }
                }
                i = i + 1u64
            }
        }
    }

    val off = poller.deregister(fd)
    match off {
        Result::Ok(u) => { }
        Result::Err(e) => { }
    }
    running
}

# Whether the response that was just written asked to keep going.
# Reading it back out of the bytes rather than carrying a flag keeps
# one answer: what the client was told is what happens.
fn keep_open(out: &ByteWriter) -> bool {
    val w = out.span()
    match w {
        Option::Some(b) => {
            val needle = String::from_str("\r\nconnection: keep-alive\r\n")
            val at = b.find_seq(span_of_string(&needle))
            match at {
                Option::Some(i) => { true }
                Option::None => { false }
            }
        }
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

    var idle_ns: u64 = 0u64
    val budget_ns = idle_s * 1000000000u64
    var running = true
    while running {
        val ready = poller.wait(tick_ms())
        var n: u64 = 0u64
        match ready {
            Result::Ok(k) => { n = k }
            Result::Err(e) => { running = false }
        }
        if running {
            if n == 0u64 {
                idle_ns = idle_ns + ((tick_ms() as u64) * 1000000u64)
                if budget_ns > 0u64 && idle_ns >= budget_ns { running = false }
                # A record that is only in memory is a record a crash
                # loses, so the segment goes out on a timer as well as
                # when it fills (DATA_MODEL.md section 4).
                val now_ns = time::now_mono_ns()
                if !w.is_empty() && now_ns - last_flush >= flush_after_ns() {
                    val put = flush_active(&mut w, &mut ms, &gens, &mut st, &crc)
                    last_flush = now_ns
                }
            } else {
                idle_ns = 0u64
                var i: u64 = 0u64
                while i < n && running {
                    val ev = poller.event(i)
                    if ev.token() == listener_token() {
                        val accepted = listener.accept()
                        match accepted {
                            Result::Ok(conn) => {
                                running = serve_connection(&poller, &conn, spec,
                                                           &mut st, &mut w,
                                                           &mut ms, &gens,
                                                           &mut inbox,
                                                           &mut outbox)
                            }
                            Result::Err(e) => { }
                        }
                    }
                    i = i + 1u64
                }
            }
        }
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
