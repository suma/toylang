# Pulling typed fields out of a line (ONTOLOGY.md O0-a).
#
# `record.t` frames a line -- where the time is, where the host and
# tag are, where the body starts. This file goes one level in and
# says what the *fields* are: for an HTTP request, which bytes are
# the status and which are the path.
#
# The split is deliberate. Framing is one state machine that every
# line goes through; extraction is one function per format, and
# formats are added by adding functions. Mixing them would grow the
# framing every time a new log shape shows up.
#
# **Nothing here copies bytes.** Every field is an (offset, length)
# pair packed into a `u64` the way `record::ParsedLine` does it, so a
# line with nine fields costs nine integers and no allocation.

import record

pub fn field_start(v: u64) -> u64 { record::span_start(v) }
pub fn field_len(v: u64) -> u64 { record::span_len(v) }
pub fn has_field(v: u64) -> bool { record::span_len(v) > 0u64 }

# One access-log line, taken apart.
#
# Two shapes appear in the same directory, which is the whole reason
# this is not a `$1`-and-`$9` job:
#
#   1.2.3.4 - - [..] "GET / HTTP/1.1" 200 11103 "-" "curl/8"
#   blog.example:80 1.2.3.4 - - [..] "GET /robots.txt .." 301 612 "-" ".."
#
# The second is `vhost_combined`. Reading the first token as the
# client would file 6,682 of this corpus's lines under a client named
# `blog.obfuscatism.net:80`. The tokens *before* the timestamp are
# therefore counted rather than assumed: three means no vhost, four
# means the first one is it.
pub struct HttpFields {
    ok: bool,
    vhost: u64,
    client: u64,
    method: u64,
    path: u64,
    proto: u64,
    status: u64,
    bytes: u64,
    referer: u64,
    ua: u64,
}

pub fn no_http() -> HttpFields {
    HttpFields {
        ok: false, vhost: 0u64, client: 0u64, method: 0u64, path: 0u64,
        proto: 0u64, status: 0u64, bytes: 0u64, referer: 0u64, ua: 0u64,
    }
}

# The end of the token starting at `at` (space-separated).
fn token_end(w: Span<u8>, at: u64, end: u64) -> u64 {
    var i = at
    while i < end {
        val b: u8 = w.get(i)
        if b == ' ' { break }
        i = i + 1u64
    }
    i
}

fn skip_spaces(w: Span<u8>, at: u64, end: u64) -> u64 {
    var i = at
    while i < end {
        val b: u8 = w.get(i)
        if b != ' ' { break }
        i = i + 1u64
    }
    i
}

# The contents of the quoted run starting at `at` (which must be the
# opening quote), and the offset just past the closing one.
#
# **A backslash escapes the next byte.** Apache writes `\"` inside the
# request and the user agent, and a scan that stops at the first `"`
# lands in the middle of the line: the status then comes out of
# whatever token followed, which is how an injection payload ends up
# counted as a status code. Nineteen lines of this corpus differ on
# exactly that.
fn quoted(w: Span<u8>, at: u64, end: u64, out_span: &mut u64) -> u64 {
    out_span = 0u64
    if at >= end { return at }
    val q: u8 = w.get(at)
    if q != '"' { return at }
    var i = at + 1u64
    while i < end {
        val b: u8 = w.get(i)
        if b == 92u8 {
            # a backslash: the next byte is data, whatever it is
            i = i + 2u64
        } else {
            if b == '"' { break }
            i = i + 1u64
        }
    }
    if i > end { i = end }
    out_span = record::pack_span(at + 1u64, i - at - 1u64)
    if i < end { return i + 1u64 }
    i
}

# Take one access-log line apart. `start` / `len` are the line's
# extent in `w`; the answer's `ok` says whether it looked like one.
pub fn http(w: Span<u8>, start: u64, len: u64) -> HttpFields {
    var f = no_http()
    val end = start + len
    if len < 20u64 { return f }

    # The timestamp's `[` is the anchor: everything before it is the
    # client (and possibly a vhost), everything after is the request.
    var bracket = end
    var i = start
    while i < end && i < start + 200u64 {
        val b: u8 = w.get(i)
        if b == '[' { bracket = i  break }
        i = i + 1u64
    }
    if bracket >= end { return f }

    # --- before the bracket: [vhost] client - -
    var toks: u64 = 0u64
    var t0: u64 = 0u64
    var t1: u64 = 0u64
    var p = start
    while p < bracket {
        p = skip_spaces(w, p, bracket)
        if p >= bracket { break }
        val e = token_end(w, p, bracket)
        if toks == 0u64 { t0 = record::pack_span(p, e - p) }
        if toks == 1u64 { t1 = record::pack_span(p, e - p) }
        toks = toks + 1u64
        p = e
    }
    if toks == 3u64 {
        f.client = t0
    } else {
        if toks == 4u64 {
            f.vhost = t0
            f.client = t1
        } else {
            return f
        }
    }

    # --- after the bracket: ] "METHOD path PROTO" status bytes "ref" "ua"
    var q = bracket
    while q < end {
        val b: u8 = w.get(q)
        if b == ']' { break }
        q = q + 1u64
    }
    if q >= end { return f }
    q = skip_spaces(w, q + 1u64, end)

    var request: u64 = 0u64
    q = quoted(w, q, end, &mut request)
    if !has_field(request) { return f }

    # The request splits into method / path / protocol. A request line
    # with no spaces at all (they exist: `"GET"` from broken clients)
    # leaves path and proto empty rather than failing the line.
    val rs = field_start(request)
    val re = rs + field_len(request)
    var rp = rs
    var rt: u64 = 0u64
    while rp < re {
        rp = skip_spaces(w, rp, re)
        if rp >= re { break }
        val e = token_end(w, rp, re)
        if rt == 0u64 { f.method = record::pack_span(rp, e - rp) }
        if rt == 1u64 { f.path = record::pack_span(rp, e - rp) }
        if rt == 2u64 { f.proto = record::pack_span(rp, e - rp) }
        rt = rt + 1u64
        rp = e
    }

    q = skip_spaces(w, q, end)
    val se = token_end(w, q, end)
    f.status = record::pack_span(q, se - q)
    q = skip_spaces(w, se, end)
    val be = token_end(w, q, end)
    f.bytes = record::pack_span(q, be - q)

    q = skip_spaces(w, be, end)
    var ref_span: u64 = 0u64
    q = quoted(w, q, end, &mut ref_span)
    f.referer = ref_span
    q = skip_spaces(w, q, end)
    var ua_span: u64 = 0u64
    q = quoted(w, q, end, &mut ua_span)
    f.ua = ua_span

    f.ok = field_len(f.status) > 0u64
    f
}

# --- `KEY=value` runs -------------------------------------------------
#
# What the kernel writes for a firewall block, and what a labelled
# line carries: `IN=eth0 OUT= SRC=1.2.3.4 DST=5.6.7.8 PROTO=TCP`.
# The count varies, so this is a cursor rather than a struct.

pub struct KvPair {
    key: u64,
    value: u64,
}

pub struct KvIter {
    pos: u64,
    end: u64,
}

impl KvIter {
    pub fn over(start: u64, len: u64) -> Self {
        KvIter { pos: start, end: start + len }
    }

    # The next `KEY=value` in the window, or false at the end.
    # A token without `=` is skipped, which is what makes this work on
    # `[UFW BLOCK] IN=eth0 ...` -- the prose in front is passed over.
    pub fn next(&mut self, w: Span<u8>, out: &mut KvPair) -> bool {
        out.key = 0u64
        out.value = 0u64
        var p = self.pos
        var found = false
        while p < self.end && !found {
            p = skip_spaces(w, p, self.end)
            if p >= self.end {
                self.pos = p
            } else {
                val e = token_end(w, p, self.end)
                var eq = p
                var has_eq = false
                var i = p
                while i < e {
                    val b: u8 = w.get(i)
                    if b == '=' { eq = i  has_eq = true  break }
                    i = i + 1u64
                }
                if has_eq && eq > p {
                    out.key = record::pack_span(p, eq - p)
                    out.value = record::pack_span(eq + 1u64, e - eq - 1u64)
                    found = true
                }
                p = e
                self.pos = p
            }
        }
        found
    }
}

# FNV-1a over a window. Used to count values without building a
# `String` for every record: only a value seen for the *first* time
# needs a name, and on this runtime that matters -- a `String` per
# record would be 181,519 allocations that never come back
# (MEMORY.md).
pub fn hash_span(w: Span<u8>, at: u64, len: u64) -> u64
    requires at + len <= w.len()
{
    var h: u64 = 14695981039346656037u64
    var i: u64 = 0u64
    while i < len {
        val b: u8 = w.get(at + i)
        h = h ^ (b as u64)
        h = h * 1099511628211u64
        i = i + 1u64
    }
    h
}

# The hash of a term written as `key:value`, without building the
# string. Terms are emitted once per field per record -- 300,000 times
# for a segment of this corpus -- so the cost of a `String` there
# would be permanent (MEMORY.md D3). Only a term seen for the *first*
# time is ever spelled out.
pub unsafe fn hash_term(key: str, w: Span<u8>, at: u64, len: u64) -> u64
    requires at + len <= w.len()
{
    var h: u64 = 14695981039346656037u64
    val p = __builtin_str_to_ptr(key)
    var i: u64 = 0u64
    while i < key.len() {
        val b: u8 = __builtin_ptr_read::<u8>(p, i)
        h = h ^ (b as u64)
        h = h * 1099511628211u64
        i = i + 1u64
    }
    h = h ^ 58u64          # ':'
    h = h * 1099511628211u64
    var j: u64 = 0u64
    while j < len {
        val b: u8 = w.get(at + j)
        h = h ^ (b as u64)
        h = h * 1099511628211u64
        j = j + 1u64
    }
    h
}

# The same hash for a key that is **in the window** rather than a
# literal.
#
# A label's key is bytes in the line (`app=api`), not a name this
# program knows in advance, so the two spellings have to produce the
# same number for the same `key:value` -- otherwise `host` written by
# the syslog framing and `host` written as a label are two different
# terms.
pub unsafe fn hash_term_span(w: Span<u8>, key_at: u64, key_len: u64,
                             at: u64, len: u64) -> u64
    requires at + len <= w.len()
    requires key_at + key_len <= w.len()
{
    var h: u64 = 14695981039346656037u64
    var i: u64 = 0u64
    while i < key_len {
        val b: u8 = w.get(key_at + i)
        h = h ^ (b as u64)
        h = h * 1099511628211u64
        i = i + 1u64
    }
    h = h ^ 58u64          # ':'
    h = h * 1099511628211u64
    var j: u64 = 0u64
    while j < len {
        val b: u8 = w.get(at + j)
        h = h ^ (b as u64)
        h = h * 1099511628211u64
        j = j + 1u64
    }
    h
}

# `key:value` where both halves are in the window.
pub fn term_text_span(w: Span<u8>, key_at: u64, key_len: u64,
                      at: u64, len: u64) -> String
    requires at + len <= w.len()
    requires key_at + key_len <= w.len()
{
    var out = String::with_capacity(key_len + len + 1u64)
    var i: u64 = 0u64
    while i < key_len {
        val b: u8 = w.get(key_at + i)
        out.push(b)
        i = i + 1u64
    }
    out.push(58u8)
    var j: u64 = 0u64
    while j < len {
        val b: u8 = w.get(at + j)
        out.push(b)
        j = j + 1u64
    }
    out
}

# `key:value` as a `String`, for the term dictionary on disk.
pub fn term_text(key: str, w: Span<u8>, at: u64, len: u64) -> String
    requires at + len <= w.len()
{
    var out = String::with_capacity(key.len() + len + 1u64)
    # `push_str` takes a `str` (the `String` version is
    # `push_string`), so the key needs no temporary.
    out.push_str(key)
    out.push(58u8)
    var i: u64 = 0u64
    while i < len {
        val b: u8 = w.get(at + i)
        out.push(b)
        i = i + 1u64
    }
    out
}
