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
pub const MAX_REQUEST_BYTES: u64 = 1048576u64
pub const MAX_HEADERS: u64 = 32u64
pub const MAX_HEADER_BYTES: u64 = 1024u64

# The methods this server tells apart. Anything else is `Unknown`
# (and answered with 405).
pub enum Method {
    Unknown,
    Get,
    Post,
}

# What a parse decided. `status` is 0 while nothing is wrong -- an
# incomplete request has `complete == false` and `status == 0`,
# because waiting for more bytes is not a failure.
pub struct Request {
    complete: bool,
    status: u64,
    method: Method,
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
            complete: false, status: 0u64, method: Method::Unknown,
            keep_alive: true,
            path_at: 0u64, path_len: 0u64,
            query_at: 0u64, query_len: 0u64,
            body_at: 0u64, body_len: 0u64,
            head_len: 0u64, total_len: 0u64,
        }
        r
    }

    pub fn is_get(&self) -> bool { match self.method { Method::Get => true, _ => false } }
    pub fn is_post(&self) -> bool { match self.method { Method::Post => true, _ => false } }
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
        if v > 1844674407370955161u64 { return MAX_REQUEST_BYTES + 1u64 }
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
    if len > MAX_REQUEST_BYTES {
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
            if len - start > MAX_HEADER_BYTES {
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
                if n > MAX_HEADER_BYTES {
                    r.status = 431u64
                    return r
                }
                count = count + 1u64
                if count > MAX_HEADERS + 1u64 {
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
    var i: u64 = 0u64
    val sp1 = loop {
        if i >= rl_len {
            r.status = 400u64
            return r
        }
        if b.get(i) == ' ' { break i }
        i = i + 1u64
    }
    var j = sp1 + 1u64
    val sp2 = loop {
        if j >= rl_len {
            r.status = 400u64
            return r
        }
        if b.get(j) == ' ' { break j }
        j = j + 1u64
    }

    if eq_at(b, 0u64, sp1, "GET") {
        r.method = Method::Get
    } elif eq_at(b, 0u64, sp1, "POST") {
        r.method = Method::Post
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
    # The `?`, or the end of the target when there is none.
    var k = t_at
    val q_mark = loop {
        if k >= t_at + t_len || b.get(k) == '?' { break k }
        k = k + 1u64
    }
    r.path_at = t_at
    r.path_len = q_mark - t_at
    if q_mark < t_at + t_len {
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
    if head_end + content_length > MAX_REQUEST_BYTES {
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
        var i = start
        val stop = loop {
            if i >= len || b.get(at + i) == '&' { break i }
            i = i + 1u64
        }
        if stop > start {
            # The `=`, or `stop` when the run has none.
            var j = start
            val eq = loop {
                if j >= stop || b.get(at + j) == '=' { break j }
                j = j + 1u64
            }
            val has_eq = eq < stop
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
# JSON strings

# Append `s` as a quoted JSON string, escaped.
#
# **This one actually escapes**, unlike the small writers elsewhere
# that only ever see a path or a hex digit: log lines carry quotes and
# backslashes as a matter of course (every Apache request line has
# two), and a response that hands them through unescaped is a
# response no JSON parser will read.
pub fn put_json_string(out: &mut ByteWriter, s: &String) {
    out.put_u8('"')
    var i: u64 = 0u64
    val n = s.len()
    while i < n {
        val c: u8 = s.get(i)
        if c == '"' {
            out.put_u8('\\')
            out.put_u8('"')
        } elif c == '\\' {
            out.put_u8('\\')
            out.put_u8('\\')
        } elif c == '\n' {
            out.put_u8('\\')
            out.put_u8('n')
        } elif c == '\r' {
            out.put_u8('\\')
            out.put_u8('r')
        } elif c == '\t' {
            out.put_u8('\\')
            out.put_u8('t')
        } elif c < 32u8 {
            # Anything else below a space has no short form. A log
            # line really does contain these -- the TLS handshake
            # bytes that arrive at a plaintext port are the example
            # ONTOLOGY.md section 3 is built around.
            val v = c as u64
            val esc = "\\u{v:04x}"
            out.put_str(esc)
        } else {
            out.put_u8(c)
        }
        i = i + 1u64
    }
    out.put_u8('"')
}

# ---------------------------------------------------------------------
# Responses

pub fn reason(status: u64) -> str {
    match status {
        200u64 => "OK",
        400u64 => "Bad Request",
        404u64 => "Not Found",
        405u64 => "Method Not Allowed",
        411u64 => "Length Required",
        413u64 => "Payload Too Large",
        431u64 => "Request Header Fields Too Large",
        500u64 => "Internal Server Error",
        503u64 => "Service Unavailable",
        _ => "Unknown",
    }
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
    var body = String::new()
    body.push_str(r#"{"error":""#)
    body.push_str(message)
    body.push_str("\"")
    if detail.len() > 0u64 {
        body.push_str(",\"detail\":\"")
        body.push_str(detail)
        body.push_str("\"")
    }
    body.push_str("}\n")
    begin_response(out, status, "application/json", body.len(), keep_alive)
    end_headers(out)
    out.put_str(body.to_str())
}
