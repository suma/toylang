# Turning a line into a record: which shape it has, when it happened,
# and where its parts are.
#
# Everything here is an offset into the reader's buffer. A parsed line
# owns no memory and copies no bytes -- `ParsedLine` is filled through
# an out-parameter so that parsing a million lines allocates nothing
# (MEMORY.md D3).
#
# Four shapes cover what a Linux host actually writes, and the fifth
# is "none of the above", which is a legitimate answer rather than a
# failure:
#
#   1 syslog   2026-08-30T00:05:01.451669+09:00 host tag[pid]: message
#   2 datetime 2026-09-01 06:42:12 message                     (dpkg)
#   3 apache   1.2.3.4 - - [04/Sep/2026:00:00:52 +0900] "GET /" 200 11103
#   4 epoch    1756900810 message
#   0 plain    anything else
#
# Timestamps become **UNIX seconds**, which are by definition counted
# from the epoch in UTC. What a log line carries is a *local* time plus
# the **UTC offset** it was written at: `+09:00` is nine hours ahead of
# UTC, `-05:00` five behind, and `Z` is the offset `+00:00` -- the only
# spelling that is itself UTC. Normalising means subtracting that
# offset from the civil time, and it is done here so that nothing
# downstream ever sees a local time.
#
# A shape that carries **no** offset (`datetime`, which is what dpkg
# writes) is read as if it were `+00:00`. That is a guess, and it is
# the safe one: inventing the host's zone would put a wrong absolute
# time into the index, while reading it as UTC keeps every such line
# consistently shifted by the host's offset -- visible, and fixable
# later by a per-source offset setting.

pub fn fmt_plain() -> u32 { 0u32 }
pub fn fmt_syslog() -> u32 { 1u32 }
pub fn fmt_datetime() -> u32 { 2u32 }
pub fn fmt_apache() -> u32 { 3u32 }
pub fn fmt_epoch() -> u32 { 4u32 }

pub fn fmt_name(kind: u32) -> str {
    if kind == 1u32 { return "syslog" }
    if kind == 2u32 { return "datetime" }
    if kind == 3u32 { return "apache" }
    if kind == 4u32 { return "epoch" }
    "plain"
}

# An (offset, length) pair packed into one `u64`: offset in the high
# 32 bits, length in the low 32.
#
# The packing is not premature cleverness -- it is what keeps this
# struct *returnable*. The AOT backend can hand back at most 8 leaves
# from a function ("Too many return values to fit in registers"), and
# four separate start/len pairs plus the header fields come to eleven.
# Buffers here are capped at 16 MiB, so 32 bits is room to spare.
pub fn pack_span(start: u64, len: u64) -> u64 {
    (start << 32u64) | (len & 0xFFFFFFFFu64)
}
pub fn span_start(v: u64) -> u64 { v >> 32u64 }
pub fn span_len(v: u64) -> u64 { v & 0xFFFFFFFFu64 }

# Where each part of one line lives. Offsets are absolute into the
# reader's buffer; a zero length means "this shape has no such part".
pub struct ParsedLine {
    kind: u32,
    has_ts: bool,
    ts: i64,
    host: u64,
    tag: u64,
    labels: u64,
    body: u64,
}

impl ParsedLine {
    pub fn new() -> Self {
        ParsedLine {
            kind: 0u32, has_ts: false, ts: 0i64,
            host: 0u64, tag: 0u64, labels: 0u64, body: 0u64,
        }
    }

    pub fn host_start(&self) -> u64 { span_start(self.host) }
    pub fn host_len(&self) -> u64 { span_len(self.host) }
    pub fn tag_start(&self) -> u64 { span_start(self.tag) }
    pub fn tag_len(&self) -> u64 { span_len(self.tag) }
    pub fn labels_start(&self) -> u64 { span_start(self.labels) }
    pub fn labels_len(&self) -> u64 { span_len(self.labels) }
    pub fn body_start(&self) -> u64 { span_start(self.body) }
    pub fn body_len(&self) -> u64 { span_len(self.body) }

    pub fn has_labels(&self) -> bool { span_len(self.labels) > 0u64 }
    pub fn has_host(&self) -> bool { span_len(self.host) > 0u64 }
}

fn is_digit_byte(b: u8) -> bool { b >= '0' && b <= '9' }

fn digits_at(r: &LogReader, at: u64, n: u64, end: u64) -> bool {
    if at + n > end { return false }
    var i: u64 = 0u64
    while i < n {
        val b: u8 = r.byte(at + i)
        if !is_digit_byte(b) { return false }
        i = i + 1u64
    }
    true
}

fn num_at(r: &LogReader, at: u64, n: u64) -> u64 {
    var v: u64 = 0u64
    var i: u64 = 0u64
    while i < n {
        val b: u8 = r.byte(at + i)
        v = v * 10u64 + ((b - '0') as u64)
        i = i + 1u64
    }
    v
}

# Seconds since the epoch for civil fields taken at offset `+00:00`.
# The caller subtracts the line's own UTC offset afterwards; keeping
# the two steps apart is what makes `Z` (`+00:00`) need no special
# case. A date the calendar rejects gives `false`, which the caller
# turns into "no timestamp" rather than a wrong one.
fn civil_secs(y: u64, mo: u64, d: u64, h: u64, mi: u64, s: u64, out: &mut i64) -> bool {
    val dt = DateTime::from_civil(y as i64, mo as u32, d as u32, h as u32, mi as u32, s as u32, 0u32)
    match dt {
        Result::Ok(v) => {
            out = v.to_unix()
            true
        }
        Result::Err(e) => { false }
    }
}

# `Jan` .. `Dec` -> 1 .. 12, or 0 when it is not a month.
fn month_at(r: &LogReader, at: u64) -> u64 {
    if r.matches(at, "Jan") { return 1u64 }
    if r.matches(at, "Feb") { return 2u64 }
    if r.matches(at, "Mar") { return 3u64 }
    if r.matches(at, "Apr") { return 4u64 }
    if r.matches(at, "May") { return 5u64 }
    if r.matches(at, "Jun") { return 6u64 }
    if r.matches(at, "Jul") { return 7u64 }
    if r.matches(at, "Aug") { return 8u64 }
    if r.matches(at, "Sep") { return 9u64 }
    if r.matches(at, "Oct") { return 10u64 }
    if r.matches(at, "Nov") { return 11u64 }
    if r.matches(at, "Dec") { return 12u64 }
    0u64
}

# The **UTC offset** that follows a time -- `+09:00`, `-0500`, or `Z`
# for `+00:00` -- as the number of seconds to subtract to reach UTC.
# `colon` says which of the two spellings to expect. An offset that
# is absent or unreadable answers 0, which reads the time as UTC.
fn offset_secs(r: &LogReader, at: u64, end: u64, colon: bool) -> i64 {
    if at >= end { return 0i64 }
    val sign: u8 = r.byte(at)
    if sign == 'Z' { return 0i64 }
    if sign != '+' && sign != '-' { return 0i64 }
    val hh_at = at + 1u64
    val mm_at = if colon { at + 4u64 } else { at + 3u64 }
    if !digits_at(r, hh_at, 2u64, end) { return 0i64 }
    if !digits_at(r, mm_at, 2u64, end) { return 0i64 }
    val hh = num_at(r, hh_at, 2u64)
    val mm = num_at(r, mm_at, 2u64)
    val mag = (hh * 3600u64 + mm * 60u64) as i64
    if sign == '-' { return 0i64 - mag }
    mag
}

# One `key=value` run at `from`: the offset just past the last token,
# which equals `from` when the line does not start with labels.
# A key is `[a-z0-9_]` of 1..32 bytes -- the rule from DATA_MODEL.md,
# kept narrow so that `[UFW BLOCK] IN=eth0` is body, not labels.
fn scan_labels(r: &LogReader, from: u64, end: u64) -> u64 {
    var p = from
    var last_good = from
    var scanning = true
    while scanning {
        var i = p
        var key_len: u64 = 0u64
        while i < end {
            val b: u8 = r.byte(i)
            val ok = (b >= 'a' && b <= 'z') || (b >= '0' && b <= '9') || b == '_'
            if !ok { break }
            key_len = key_len + 1u64
            i = i + 1u64
        }
        if key_len == 0u64 || key_len > 32u64 || i >= end {
            scanning = false
        } else {
            val eq: u8 = r.byte(i)
            if eq != '=' {
                scanning = false
            } else {
                var j = i + 1u64
                while j < end {
                    val c: u8 = r.byte(j)
                    if c == ' ' { break }
                    j = j + 1u64
                }
                last_good = j
                if j >= end {
                    scanning = false
                    p = j
                } else {
                    p = j + 1u64
                }
            }
        }
    }
    last_good
}

# Fill `out` from the line `ln` of `r`.
pub fn parse(r: &LogReader, ln: Line, out: &mut ParsedLine) {
    val start = ln.start
    val end = ln.start + ln.len
    out.kind = 0u32
    out.has_ts = false
    out.ts = 0i64
    out.host = 0u64
    out.tag = 0u64
    out.labels = 0u64
    out.body = pack_span(start, ln.len)
    if ln.len == 0u64 { return }

    var cursor = start

    # --- shape 1 / 2: a date at the very start ---------------------
    val dated = digits_at(r, start, 4u64, end)
        && r.matches(start + 4u64, "-")
        && digits_at(r, start + 5u64, 2u64, end)
        && r.matches(start + 7u64, "-")
        && digits_at(r, start + 8u64, 2u64, end)
    if dated {
        val sep: u8 = r.byte(start + 10u64)
        val timed = (sep == 'T' || sep == ' ')
            && digits_at(r, start + 11u64, 2u64, end)
            && r.matches(start + 13u64, ":")
            && digits_at(r, start + 14u64, 2u64, end)
            && r.matches(start + 16u64, ":")
            && digits_at(r, start + 17u64, 2u64, end)
        if timed {
            val y = num_at(r, start, 4u64)
            val mo = num_at(r, start + 5u64, 2u64)
            val d = num_at(r, start + 8u64, 2u64)
            val h = num_at(r, start + 11u64, 2u64)
            val mi = num_at(r, start + 14u64, 2u64)
            val s = num_at(r, start + 17u64, 2u64)
            var secs: i64 = 0i64
            if civil_secs(y, mo, d, h, mi, s, &mut secs) {
                var p = start + 19u64
                # optional fractional seconds
                if p < end {
                    val dot: u8 = r.byte(p)
                    if dot == '.' {
                        p = p + 1u64
                        while p < end {
                            val c: u8 = r.byte(p)
                            if !is_digit_byte(c) { break }
                            p = p + 1u64
                        }
                    }
                }
                val off = offset_secs(r, p, end, true)
                out.has_ts = true
                out.ts = secs - off
                out.kind = if sep == 'T' { 1u32 } else { 2u32 }
                # step over the UTC offset (or `Z`) to the first space
                while p < end {
                    val c: u8 = r.byte(p)
                    if c == ' ' { break }
                    p = p + 1u64
                }
                while p < end {
                    val c: u8 = r.byte(p)
                    if c != ' ' { break }
                    p = p + 1u64
                }
                cursor = p
            }
        }
    }

    # --- shape 4: bare epoch seconds --------------------------------
    if !out.has_ts {
        var n: u64 = 0u64
        var i = start
        while i < end {
            val b: u8 = r.byte(i)
            if !is_digit_byte(b) { break }
            n = n + 1u64
            i = i + 1u64
        }
        val followed = i >= end || r.matches(i, " ")
        if n >= 9u64 && n <= 11u64 && followed {
            out.has_ts = true
            out.ts = num_at(r, start, n) as i64
            out.kind = 4u32
            cursor = if i < end { i + 1u64 } else { i }
        }
    }

    # --- shape 3: apache, whose date sits inside brackets ----------
    if !out.has_ts {
        var i = start
        var bracket = end
        while i < end && i < start + 128u64 {
            val b: u8 = r.byte(i)
            if b == '[' { bracket = i  break }
            i = i + 1u64
        }
        if bracket < end {
            val d0 = bracket + 1u64
            val ok = digits_at(r, d0, 2u64, end)
                && r.matches(d0 + 2u64, "/")
                && month_at(r, d0 + 3u64) != 0u64
                && r.matches(d0 + 6u64, "/")
                && digits_at(r, d0 + 7u64, 4u64, end)
                && r.matches(d0 + 11u64, ":")
                && digits_at(r, d0 + 12u64, 2u64, end)
            if ok {
                val d = num_at(r, d0, 2u64)
                val mo = month_at(r, d0 + 3u64)
                val y = num_at(r, d0 + 7u64, 4u64)
                val h = num_at(r, d0 + 12u64, 2u64)
                val mi = num_at(r, d0 + 15u64, 2u64)
                val s = num_at(r, d0 + 18u64, 2u64)
                var secs: i64 = 0i64
                if civil_secs(y, mo, d, h, mi, s, &mut secs) {
                    val off = offset_secs(r, d0 + 21u64, end, false)
                    out.has_ts = true
                    out.ts = secs - off
                    out.kind = 3u32
                    # The request and the rest stay in the body: this
                    # component frames lines, it does not take apart
                    # every field of every format.
                    return
                }
            }
        }
    }

    # --- leading `key=value` labels win over the syslog header -----
    #
    # `2026-09-03T11:59:58Z host=web01 app=api level=error ...` is a
    # labelled line, not a syslog line whose host happens to be
    # called `host=web01`. Labels are therefore probed first, and the
    # syslog header is only read when there are none.
    val labelled_first = scan_labels(r, cursor, end)

    # --- syslog: host and tag sit between the time and the message --
    if out.kind == 1u32 && labelled_first == cursor {
        var p = cursor
        var i = p
        while i < end {
            val b: u8 = r.byte(i)
            if b == ' ' { break }
            i = i + 1u64
        }
        if i > p {
            out.host = pack_span(p, i - p)
            p = i + 1u64
            var j = p
            while j < end {
                val b: u8 = r.byte(j)
                if b == '[' || b == ':' || b == ' ' { break }
                j = j + 1u64
            }
            if j > p {
                out.tag = pack_span(p, j - p)
                # skip `[pid]` and the `:` and the space after it
                while j < end {
                    val b: u8 = r.byte(j)
                    if b == ':' { break }
                    j = j + 1u64
                }
                if j < end { j = j + 1u64 }
                while j < end {
                    val b: u8 = r.byte(j)
                    if b != ' ' { break }
                    j = j + 1u64
                }
                p = j
            }
        }
        cursor = p
    }

    # --- leading `key=value` labels, whatever the shape ------------
    val after = scan_labels(r, cursor, end)
    if after > cursor {
        out.labels = pack_span(cursor, after - cursor)
        var p = after
        while p < end {
            val b: u8 = r.byte(p)
            if b != ' ' { break }
            p = p + 1u64
        }
        cursor = p
    }

    out.body = pack_span(cursor, end - cursor)
}
