# The slice of HTTP/1.1 this server speaks.
#
# **The whole protocol is not the goal** (HTTP_API.md section 1). Every
# form accepted is a form that has to keep working, so the set is
# small and everything outside it gets a status code rather than a
# best effort: `GET` and `POST`, headers without folded continuation
# lines, a body measured by `Content-Length`, `keep-alive`, and
# `?a=b&c=d` with `%XX` and `+`. Chunked bodies are `411`, other
# methods are `405`.
#
# **Nothing here allocates per request.** A parsed request is offsets
# into the buffer the connection already owns, the way `extract.t`
# reports fields inside a log line; the caller copies out only what it
# decides to keep. That is what lets the connection table be a fixed
# set of buffers taken at start-up (MEMORY.md section 3).
#
# The parser is total: it never panics and never blocks. An
# incomplete request is not an error -- it is "read more" -- so
# `complete` and `status` are separate answers.

# A request is refused outright past this, because the read buffer is
# a fixed size and there is no second one to grow into.
pub fn max_request_bytes() -> u64 { 1048576u64 }
pub fn max_headers() -> u64 { 32u64 }
pub fn max_header_bytes() -> u64 { 1024u64 }

pub fn method_none() -> u64 { 0u64 }
pub fn method_get() -> u64 { 1u64 }
pub fn method_post() -> u64 { 2u64 }

# What a parse decided. `status` is 0 while nothing is wrong -- an
# incomplete request has `complete == false` and `status == 0`,
# because waiting for more bytes is not a failure.
pub struct Request {
    complete: bool,
    status: u64,
    method: u64,
    keep_alive: bool,
    # Offsets into the buffer that was parsed. The target is left
    # percent-encoded; `percent_decode` is the caller's decision,
    # because the caller knows how much room it wants to spend.
    path_at: u64,
    path_len: u64,
    query_at: u64,
    query_len: u64,
    body_at: u64,
    body_len: u64,
    # Everything through the blank line, so the caller can tell how
    # much of its buffer this request consumed.
    head_len: u64,
    total_len: u64,
}

impl Request {
    pub fn empty() -> Self {
        val r = Request {
            complete: false, status: 0u64, method: method_none(),
            keep_alive: true,
            path_at: 0u64, path_len: 0u64,
            query_at: 0u64, query_len: 0u64,
            body_at: 0u64, body_len: 0u64,
            head_len: 0u64, total_len: 0u64,
        }
        r
    }

    pub fn is_get(&self) -> bool { self.method == method_get() }
    pub fn is_post(&self) -> bool { self.method == method_post() }
    pub fn has_query(&self) -> bool { self.query_len > 0u64 }
}

# ---------------------------------------------------------------------
# Byte helpers

fn lower(c: u8) -> u8 {
    if c >= 'A' && c <= 'Z' { return c + 32u8 }
    c
}

# Whether `b[at .. at+len]` is exactly `s`.
pub fn eq_at(b: Span<u8>, at: u64, len: u64, s: str) -> bool {
    val w = String::from_str(s)
    if w.len() != len { return false }
    var i: u64 = 0u64
    while i < len {
        val x: u8 = b.get(at + i)
        val y: u8 = w.get(i)
        if x != y { return false }
        i = i + 1u64
    }
    true
}

# The same, ignoring ASCII case. Header names are case-insensitive;
# paths are not.
pub fn eq_at_fold(b: Span<u8>, at: u64, len: u64, s: str) -> bool {
    val w = String::from_str(s)
    if w.len() != len { return false }
    var i: u64 = 0u64
    while i < len {
        val x: u8 = b.get(at + i)
        val y: u8 = w.get(i)
        if lower(x) != lower(y) { return false }
        i = i + 1u64
    }
    true
}

# The index of the next `\n` at or after `from`, or `end` if there is
# none.
fn eol(b: Span<u8>, from: u64, end: u64) -> u64 {
    var i = from
    while i < end {
        val c: u8 = b.get(i)
        if c == '\n' { return i }
        i = i + 1u64
    }
    end
}

# The line's length with a trailing `\r` removed. Bare `\n` is
# accepted as well as `\r\n`: it costs two lines here and makes the
# server answer `nc` as well as `curl`.
fn line_len(b: Span<u8>, from: u64, nl: u64) -> u64 {
    if nl <= from { return 0u64 }
    val last: u8 = b.get(nl - 1u64)
    if last == '\r' { return nl - 1u64 - from }
    nl - from
}

fn header_all_digits(b: Span<u8>, at: u64, len: u64) -> bool {
    if len == 0u64 { return false }
    var i: u64 = 0u64
    while i < len {
        val c: u8 = b.get(at + i)
        if c < '0' || c > '9' { return false }
        i = i + 1u64
    }
    true
}

# `header_all_digits` first: this saturates rather than wrapping, so a
# number too large to be a length comes back as one too large to
# accept, which is the same refusal.
fn header_number(b: Span<u8>, at: u64, len: u64) -> u64 {
    var v: u64 = 0u64
    var i: u64 = 0u64
    while i < len {
        val c: u8 = b.get(at + i)
        if v > 1844674407370955161u64 { return max_request_bytes() + 1u64 }
        v = (v * 10u64) + ((c - '0') as u64)
        i = i + 1u64
    }
    v
}

# ---------------------------------------------------------------------

# Parse whatever has arrived so far.
#
# Three outcomes, and the caller has to tell them apart:
#
#   `status != 0`                 refuse with that code
#   `!complete && status == 0`    read more
#   `complete && status == 0`     serve it
pub fn parse_request(b: Span<u8>, len: u64) -> Request {
    var r = Request::empty()
    if len == 0u64 { return r }
    if len > max_request_bytes() {
        r.status = 413u64
        return r
    }

    # 1. Is the head there at all? A blank line ends it.
    var head_end: u64 = 0u64
    var have_head = false
    var start: u64 = 0u64
    var count: u64 = 0u64
    while !have_head && start < len {
        val nl = eol(b, start, len)
        if nl >= len {
            # No newline yet: everything so far is one unfinished line.
            if len - start > max_header_bytes() {
                r.status = 431u64
                return r
            }
            start = len
        } else {
            val n = line_len(b, start, nl)
            if n == 0u64 {
                head_end = nl + 1u64
                have_head = true
            } else {
                if n > max_header_bytes() {
                    r.status = 431u64
                    return r
                }
                count = count + 1u64
                if count > max_headers() + 1u64 {
                    r.status = 431u64
                    return r
                }
                start = nl + 1u64
            }
        }
    }
    if !have_head { return r }
    r.head_len = head_end

    # 2. The request line: METHOD SP target SP VERSION
    val rl_nl = eol(b, 0u64, head_end)
    val rl_len = line_len(b, 0u64, rl_nl)
    var sp1: u64 = 0u64
    var found1 = false
    var i: u64 = 0u64
    while i < rl_len && !found1 {
        val c: u8 = b.get(i)
        if c == ' ' {
            sp1 = i
            found1 = true
        }
        i = i + 1u64
    }
    if !found1 {
        r.status = 400u64
        return r
    }
    var sp2: u64 = 0u64
    var found2 = false
    var j = sp1 + 1u64
    while j < rl_len && !found2 {
        val c: u8 = b.get(j)
        if c == ' ' {
            sp2 = j
            found2 = true
        }
        j = j + 1u64
    }
    if !found2 {
        r.status = 400u64
        return r
    }

    if eq_at(b, 0u64, sp1, "GET") {
        r.method = method_get()
    } elif eq_at(b, 0u64, sp1, "POST") {
        r.method = method_post()
    } else {
        # A method that is understood but not served is 405; one that
        # is not a method at all is still 405, because the difference
        # is not useful to the sender.
        r.status = 405u64
        return r
    }

    # HTTP/1.0 defaults to closing; 1.1 defaults to keeping alive.
    val ver_at = sp2 + 1u64
    val ver_len = rl_len - ver_at
    if eq_at(b, ver_at, ver_len, "HTTP/1.0") { r.keep_alive = false }

    # 3. The target, split at the first `?`.
    val t_at = sp1 + 1u64
    val t_len = sp2 - t_at
    if t_len == 0u64 {
        r.status = 400u64
        return r
    }
    var q_mark = t_at + t_len
    var k = t_at
    var found_q = false
    while k < t_at + t_len && !found_q {
        val c: u8 = b.get(k)
        if c == '?' {
            q_mark = k
            found_q = true
        }
        k = k + 1u64
    }
    r.path_at = t_at
    r.path_len = q_mark - t_at
    if found_q {
        r.query_at = q_mark + 1u64
        r.query_len = (t_at + t_len) - (q_mark + 1u64)
    }

    # 4. The headers this server reads. Everything else is skipped,
    #    which is what lets a client send whatever it likes.
    var content_length: u64 = 0u64
    var chunked = false
    var hs = rl_nl + 1u64
    while hs < head_end {
        val nl = eol(b, hs, head_end)
        val n = line_len(b, hs, nl)
        if n == 0u64 {
            hs = head_end
        } else {
            val lead: u8 = b.get(hs)
            if lead == ' ' || lead == '\t' {
                # A folded continuation line. Refusing is the point:
                # accepting one means deciding what it means.
                r.status = 400u64
                return r
            }
            var colon = hs + n
            var has_colon = false
            var c2 = hs
            while c2 < hs + n && !has_colon {
                val c: u8 = b.get(c2)
                if c == ':' {
                    colon = c2
                    has_colon = true
                }
                c2 = c2 + 1u64
            }
            if !has_colon {
                r.status = 400u64
                return r
            }
            val name_len = colon - hs
            val line_end = hs + n
            # Optional whitespace either side of the value.
            var val_at = colon + 1u64
            var leading = true
            while leading && val_at < line_end {
                val c: u8 = b.get(val_at)
                if c == ' ' || c == '\t' { val_at = val_at + 1u64 } else { leading = false }
            }
            var val_end = line_end
            var trailing = true
            while trailing && val_end > val_at {
                val c: u8 = b.get(val_end - 1u64)
                if c == ' ' || c == '\t' { val_end = val_end - 1u64 } else { trailing = false }
            }
            val val_len = val_end - val_at

            if eq_at_fold(b, hs, name_len, "content-length") {
                if !header_all_digits(b, val_at, val_len) {
                    r.status = 400u64
                    return r
                }
                content_length = header_number(b, val_at, val_len)
            }
            if eq_at_fold(b, hs, name_len, "transfer-encoding") {
                chunked = true
            }
            if eq_at_fold(b, hs, name_len, "connection") {
                if eq_at_fold(b, val_at, val_len, "close") { r.keep_alive = false }
                if eq_at_fold(b, val_at, val_len, "keep-alive") { r.keep_alive = true }
            }
            hs = nl + 1u64
        }
    }

    if chunked {
        # Chunked would mean a second framing to get right, and
        # nothing this API takes needs it.
        r.status = 411u64
        return r
    }
    if r.is_get() && content_length > 0u64 {
        # A GET with a body is a request this server has no reading
        # for, and guessing is worse than refusing.
        r.status = 400u64
        return r
    }
    if head_end + content_length > max_request_bytes() {
        r.status = 413u64
        return r
    }

    r.body_at = head_end
    r.body_len = content_length
    r.total_len = head_end + content_length
    # The head is here; the body may not be.
    if len < r.total_len { return r }
    r.complete = true
    r
}

# ---------------------------------------------------------------------
# Query strings

fn hex_val(c: u8) -> u64 {
    if c >= '0' && c <= '9' { return (c - '0') as u64 }
    if c >= 'a' && c <= 'f' { return ((c - 'a') as u64) + 10u64 }
    if c >= 'A' && c <= 'F' { return ((c - 'A') as u64) + 10u64 }
    16u64
}

# `%XX` and `+`, appended to `out`.
#
# **A broken escape is kept as written** rather than dropped: a `%` at
# the end of a value is far more likely to be someone's password than
# a truncated escape, and silently eating it makes the difference
# invisible.
pub fn percent_decode(b: Span<u8>, at: u64, len: u64, out: &mut ByteWriter) {
    var i: u64 = 0u64
    while i < len {
        val c: u8 = b.get(at + i)
        if c == '+' {
            out.put_u8(' ')
            i = i + 1u64
        } elif c == '%' && i + 2u64 < len {
            val hi = hex_val(b.get(at + i + 1u64))
            val lo = hex_val(b.get(at + i + 2u64))
            if hi < 16u64 && lo < 16u64 {
                out.put_u8(((hi * 16u64) + lo) as u8)
                i = i + 3u64
            } else {
                out.put_u8(c)
                i = i + 1u64
            }
        } else {
            out.put_u8(c)
            i = i + 1u64
        }
    }
}

# Find `name` in a query string and decode its value into `out`.
#
# Returns false when the parameter is absent, which is different from
# present and empty (`?limit=` is a client saying something).
pub fn query_param(b: Span<u8>, at: u64, len: u64, name: str,
                   out: &mut ByteWriter) -> bool {
    out.clear()
    if len == 0u64 { return false }
    val want = String::from_str(name)
    val wn = want.len()
    if wn == 0u64 { return false }

    var start: u64 = 0u64
    while start <= len {
        # One `key=value` run, ending at `&` or the end.
        var stop = len
        var i = start
        var found = false
        while i < len && !found {
            val c: u8 = b.get(at + i)
            if c == '&' {
                stop = i
                found = true
            }
            i = i + 1u64
        }
        if stop > start {
            var eq = stop
            var has_eq = false
            var j = start
            while j < stop && !has_eq {
                val c: u8 = b.get(at + j)
                if c == '=' {
                    eq = j
                    has_eq = true
                }
                j = j + 1u64
            }
            val key_len = eq - start
            if key_len == wn {
                if eq_at(b, at + start, key_len, name) {
                    var v_at = stop
                    var v_len: u64 = 0u64
                    if has_eq {
                        v_at = at + eq + 1u64
                        v_len = stop - (eq + 1u64)
                    }
                    percent_decode(b, v_at, v_len, out)
                    return true
                }
            }
        }
        start = stop + 1u64
    }
    false
}

# ---------------------------------------------------------------------
# Responses

pub fn reason(status: u64) -> str {
    if status == 200u64 { return "OK" }
    if status == 400u64 { return "Bad Request" }
    if status == 404u64 { return "Not Found" }
    if status == 405u64 { return "Method Not Allowed" }
    if status == 411u64 { return "Length Required" }
    if status == 413u64 { return "Payload Too Large" }
    if status == 431u64 { return "Request Header Fields Too Large" }
    if status == 500u64 { return "Internal Server Error" }
    if status == 503u64 { return "Service Unavailable" }
    "Unknown"
}

# Status line and the headers every response carries.
#
# `Content-Length` is always written, so a client never has to guess
# where a body ends and keep-alive stays usable. That is also why
# there is no streaming response here: this server knows the length
# before it starts writing (HTTP_API.md section 4).
pub fn begin_response(out: &mut ByteWriter, status: u64, ctype: str,
                      body_len: u64, keep_alive: bool) {
    val why = reason(status)
    val head = "HTTP/1.1 {status} {why}\r\ncontent-type: {ctype}\r\ncontent-length: {body_len}\r\n"
    out.put_str(head)
    if keep_alive {
        out.put_str("connection: keep-alive\r\n")
    } else {
        out.put_str("connection: close\r\n")
    }
}

pub fn end_headers(out: &mut ByteWriter) {
    out.put_str("\r\n")
}

# One extra header line, for the few responses that carry one.
pub fn put_header(out: &mut ByteWriter, name: str, value: str) {
    val line = "{name}: {value}\r\n"
    out.put_str(line)
}

# A whole response whose body is a plain string.
pub fn respond_text(out: &mut ByteWriter, status: u64, ctype: str,
                    body: str, keep_alive: bool) {
    val s = String::from_str(body)
    begin_response(out, status, ctype, s.len(), keep_alive)
    end_headers(out)
    out.put_str(body)
}

# The error shape the API documents: a JSON object with `error` and an
# optional `detail`.
pub fn respond_error(out: &mut ByteWriter, status: u64, message: str,
                     detail: str, keep_alive: bool) {
    # `{{` and `}}` are how a literal brace survives interpolation.
    var body = String::new()
    body.push_str("{{\u{22}error\u{22}:\u{22}")
    body.push_str(message)
    body.push_str("\u{22}")
    if detail.len() > 0u64 {
        body.push_str(",\u{22}detail\u{22}:\u{22}")
        body.push_str(detail)
        body.push_str("\u{22}")
    }
    body.push_str("}}\n")
    begin_response(out, status, "application/json", body.len(), keep_alive)
    end_headers(out)
    out.put_str(body.to_str())
}
