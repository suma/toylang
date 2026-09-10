# The query engine: reading segments back and answering a question
# about them.
#
# The question arrives as one string, because that is what a command
# line and a URL both hand over:
#
#   host=web01 timeout from=2026-09-03T00:00:00Z limit=20 order=desc
#
# Tokens are separated by spaces. A token with a **known** key sets a
# filter; every other token is a substring the line must contain.
# That last rule is what makes `level=error` and `SRC=1.2.3.4` work
# without either being a field: they are searched for literally, which
# is what a person typing them means.
#
#   from=<t> to=<t>   time range, half-open. ISO 8601, unix seconds,
#                     or relative (`-1h`, `-30m`, `-2d`)
#   host=<s>          the syslog host, compared whole
#   tag=<s>           the syslog tag (`CRON`, `kernel`, ...)
#   kind=<s>          syslog | apache | datetime | epoch | plain
#   limit=<n>         how many lines to print (default 20)
#   order=asc|desc    by time; desc is the default
#   <anything else>   a substring the line must contain (AND)
#
# **Filters are AND across kinds.** There is no `or`, no parentheses
# and no negation: the shapes a log search actually needs are a
# conjunction, and a query language is a large thing to maintain
# (QUERY.md §1).
#
# What it does *not* do yet: the term index and the bloom filter
# (STORAGE_FORMAT.md §4) are not written, so every candidate segment
# is expanded and scanned. The time range still prunes whole segments
# from the header alone, which is what keeps a narrow query cheap.

import std.parse
import std.time
import archive
import logdir
import search
import segfile

pub fn kind_any() -> u32 { 255u32 }
pub fn default_limit() -> u64 { 20u64 }

pub struct Query {
    ts_from: i64,
    ts_to: i64,
    kind: u32,
    limit: u64,
    desc: bool,
    # `host` and `tag` hold at most one element each.
    #
    # A plain `String` field would read better, but a whole struct
    # cannot be assigned into a field ("compiler MVP cannot assign
    # whole struct to nested field"), and the parser fills these in
    # as it goes. A one-element `Vec` is filled by a method call on
    # the field, which is allowed.
    host: Vec<String>,
    tag: Vec<String>,
    needles: Vec<String>,
    # `key=value` for a key the index knows, spelled the way the term
    # dictionary spells it (`status:404`). These are answered from
    # postings; everything else is a substring.
    terms: Vec<String>,
    # `key~needle` for a key the index knows, spelled `ua:MJ12bot`.
    # Answered from postings too, but by scanning the dictionary for
    # every value that contains the needle and taking their union --
    # the term is not known in advance the way `terms` is.
    subs: Vec<String>,
}

impl Query {
    pub fn new() -> Self {
        val h: Vec<String> = Vec::new()
        val t: Vec<String> = Vec::new()
        val n: Vec<String> = Vec::new()
        val tm: Vec<String> = Vec::new()
        val sb: Vec<String> = Vec::new()
        Query {
            ts_from: 0i64, ts_to: 0i64, kind: kind_any(),
            limit: default_limit(), desc: true,
            host: h, tag: t, needles: n, terms: tm, subs: sb,
        }
    }
    pub fn needle_count(&self) -> u64 { self.needles.size() }
    pub fn term_count(&self) -> u64 { self.terms.size() }
    pub fn sub_count(&self) -> u64 { self.subs.size() }
    # Whether anything at all is answered from the index.
    pub fn indexed_count(&self) -> u64 { self.terms.size() + self.subs.size() }
    pub fn has_host(&self) -> bool { self.host.size() > 0u64 }
    pub fn has_tag(&self) -> bool { self.tag.size() > 0u64 }
}

# One line that matched, kept small so that sorting is cheap: the text
# lives in a parallel array and is fetched by `ord`.
pub struct Hit {
    ts: i64,
    ord: u64,
}

impl Ord for Hit {
    fn lt(&self, other: &Self) -> bool {
        if self.ts != other.ts { return self.ts < other.ts }
        self.ord < other.ord
    }
}

# `-1h` / `-30m` / `-2d`, ISO 8601, or plain unix seconds.
fn parse_time(text: str, now: i64) -> i64 {
    val s = String::from_str(text)
    if s.len() == 0u64 { return 0i64 }
    val first: u8 = s.get(0u64)
    if first == '-' {
        val unit: u8 = s.get(s.len() - 1u64)
        val digits = s.substring(1u64, s.len() - 1u64)
        val n = parse::to_u64(digits.to_str())
        var mag: u64 = 0u64
        match n {
            Result::Ok(v) => { mag = v }
            Result::Err(e) => { return 0i64 }
        }
        var secs = mag
        if unit == 'm' { secs = mag * 60u64 }
        if unit == 'h' { secs = mag * 3600u64 }
        if unit == 'd' { secs = mag * 86400u64 }
        return now - (secs as i64)
    }
    if first >= '0' && first <= '9' && s.len() <= 11u64 {
        return (parse::to_u64(text) ?? 0u64) as i64
    }
    val dt = time::parse_iso8601(text)
    match dt {
        Result::Ok(v) => { v.to_unix() }
        Result::Err(e) => { 0i64 }
    }
}

fn kind_code(text: str) -> u32 {
    val s = String::from_str(text)
    if s.eq_str("syslog") { return 1u32 }
    if s.eq_str("datetime") { return 2u32 }
    if s.eq_str("apache") { return 3u32 }
    if s.eq_str("epoch") { return 4u32 }
    if s.eq_str("plain") { return 0u32 }
    kind_any()
}

# Turn the query string into a `Query`.
#
# Parsing allocates -- it splits, and every token becomes a `String`.
# It happens once per run, which is the side of the program where
# allocation is allowed (MEMORY.md D4b).
pub fn parse_query(text: str, now: i64) -> Query {
    var q = Query::new()
    val whole = String::from_str(text)
    val sep = String::from_str(" ")
    val parts = whole.split(sep)
    var i: u64 = 0u64
    while i < parts.size() {
        val tok: String = parts.get(i)
        i = i + 1u64
        if tok.len() == 0u64 {
            # split leaves empty pieces where spaces repeat
        } else {
            val eq = String::from_str("=")
            val at = tok.find(eq)
            var handled = false
            match at {
                Option::Some(pos) => {
                    val key = tok.substring(0u64, pos)
                    val value = tok.substring(pos + 1u64, tok.len())
                    val vs = value.to_str()
                    if key.eq_str("from") { q.ts_from = parse_time(vs, now)  handled = true }
                    if key.eq_str("to") { q.ts_to = parse_time(vs, now)  handled = true }
                    if key.eq_str("host") { q.host.push(value.clone())  handled = true }
                    if key.eq_str("tag") { q.tag.push(value.clone())  handled = true }
                    if key.eq_str("kind") { q.kind = kind_code(vs)  handled = true }
                    # `top=` is read by the caller, which decides
                    # between a traversal and a whole distribution. It
                    # is consumed here so it does not fall through and
                    # become a body substring -- searching lines for
                    # the text `top=path` is nobody's intent.
                    if key.eq_str("top") { handled = true }
                    if key.eq_str("order") {
                        q.desc = value.eq_str("desc")
                        handled = true
                    }
                    if key.eq_str("limit") {
                        val n = parse::to_u64(vs)
                        match n {
                            Result::Ok(v) => { q.limit = v }
                            Result::Err(e) => { }
                        }
                        handled = true
                    }
                }
                Option::None => { }
            }
            # `key~needle` / `key^needle`: a search over the *values*
            # the index already holds -- anywhere, or at the start.
            # Checked before the `=` fallthrough below because a token
            # has one shape or the other, never both.
            #
            # The mode is stored as the spec's first byte rather than
            # in a parallel array, so the resolver stays one loop. It
            # cannot collide with a value: the marker sits in front of
            # the *key*, and position 0 is never part of the needle.
            if !handled {
                val tilde = String::from_str("~")
                val caret = String::from_str("^")
                val tat = tok.find(tilde)
                val cat = tok.find(caret)
                var mpos: u64 = 0u64
                var found = false
                var anchored = false
                match tat {
                    Option::Some(tp) => { mpos = tp  found = true }
                    Option::None => { }
                }
                if !found {
                    match cat {
                        Option::Some(cpx) => { mpos = cpx  found = true  anchored = true }
                        Option::None => { }
                    }
                }
                if found {
                    val tkey = tok.substring(0u64, mpos)
                    if is_index_key(&tkey) {
                        val needle = tok.substring(mpos + 1u64, tok.len())
                        val colon = String::from_str(":")
                        val keyed = tkey.concat(&colon)
                        val body = keyed.concat(&needle)
                        # The two spellings differ only in the marker,
                        # so the push is written twice rather than
                        # choosing a `String` in an expression.
                        if anchored {
                            val norm = caret.concat(&body)
                            q.subs.push(norm)
                        } else {
                            val norm = tilde.concat(&body)
                            q.subs.push(norm)
                        }
                        handled = true
                    }
                }
            }
            if !handled {
                # A key the index knows becomes a term; anything else
                # is a substring, spelled exactly as it was typed.
                # `level=error` stays a substring on purpose: there is
                # no `level` field, and searching for the text is what
                # the person meant.
                var as_term = false
                match at {
                    Option::Some(pos) => {
                        val key = tok.substring(0u64, pos)
                        if is_index_key(&key) {
                            # The dictionary spells terms with a colon
                            # (`status:404`); the query spells them
                            # with `=`. The value is cut again here --
                            # the binding in the branch above belongs
                            # to that branch.
                            val term_value = tok.substring(pos + 1u64, tok.len())
                            val colon = String::from_str(":")
                            val head = key.concat(&colon)
                            val norm = head.concat(&term_value)
                            q.terms.push(norm)
                            as_term = true
                        }
                    }
                    Option::None => { }
                }
                if !as_term { q.needles.push(tok.clone()) }
            }
        }
    }
    q
}

# Does this record pass every filter?
fn matches(q: &Query, arena: Span<u8>, line_at: u64, line_len: u64,
           kind: u32, ts: i64, dated: bool,
           host_rel: u64, host_len: u64, tag_rel: u64, tag_len: u64) -> bool {
    if q.kind != kind_any() && q.kind != kind { return false }
    if q.ts_from != 0i64 {
        if !dated { return false }
        if ts < q.ts_from { return false }
    }
    if q.ts_to != 0i64 {
        if !dated { return false }
        if ts >= q.ts_to { return false }
    }
    if q.has_host() {
        if host_len == 0u64 { return false }
        val want_host: String = q.host.get(0u64)
        val hw = want_host.as_span()
        match hw {
            Option::Some(want) => {
                if !search::equals(arena, line_at + host_rel, host_len, want, want_host.len()) {
                    return false
                }
            }
            Option::None => { return false }
        }
    }
    if q.has_tag() {
        if tag_len == 0u64 { return false }
        val want_tag: String = q.tag.get(0u64)
        val tw = want_tag.as_span()
        match tw {
            Option::Some(want) => {
                if !search::equals(arena, line_at + tag_rel, tag_len, want, want_tag.len()) {
                    return false
                }
            }
            Option::None => { return false }
        }
    }
    var k: u64 = 0u64
    while k < q.needles.size() {
        val needle: String = q.needles.get(k)
        val nw = needle.as_span()
        match nw {
            Option::Some(want) => {
                if !search::find(arena, line_at, line_len, want, needle.len()) {
                    return false
                }
            }
            Option::None => { }
        }
        k = k + 1u64
    }
    true
}

# The bytes of one line, as a `String`, for printing.
#
# Allocates, once per *hit* -- bounded by `limit` in the normal case
# and by `max_hits()` in the worst.
pub fn text_of(arena: Span<u8>, at: u64, len: u64) -> String {
    var out = String::with_capacity(len)
    var i: u64 = 0u64
    while i < len {
        val b: u8 = arena.get(at + i)
        out.push(b)
        i = i + 1u64
    }
    out
}

# How many matches are kept before the answer is called truncated.
# A query that matches a million lines is a query whose author wants
# a different query, not a million lines of output.
pub fn max_hits() -> u64 { 20000u64 }

# Run `q` over every segment under `dir` and print the answer.
#
# The order of work is the point: the `.dat` header prunes by time,
# the term index prunes by field, and only a segment that survives
# both is expanded. Decompressing 8 MiB to find nothing is the cost
# this avoids.
# The record ordinals this segment's **indexed** filters allow, or
# "nothing survives" so the caller can skip the segment whole.
#
# Pulled out of `run` because it is the part that grew: `=` looks one
# term up, `~` and `^` scan the dictionary and **union** every value
# that matches, and each of those is then intersected with what came
# before. Three merges over ascending lists, nested two deep, is not
# something to leave inline in a function that also does file I/O and
# printing -- and it is the only part a test can pin without a
# directory on disk.
#
# `allowed` is cleared first and left holding the surviving ordinals,
# ascending. The answer is `true` when the segment cannot match.
pub fn resolve_indexed(traw: Span<u8>, tlen: u64, q: &Query,
                       allowed: &mut Vec<u32>) -> bool {
    var tmp: Vec<u32> = Vec::new()
    var merged: Vec<u32> = Vec::new()
    var uni: Vec<u32> = Vec::new()
    var uni2: Vec<u32> = Vec::new()
    var empty = false
    allowed.clear()
    var first = true
    var t: u64 = 0u64
    while t < q.terms.size() && !empty {
        val name: String = q.terms.get(t)
        val nw = name.as_span()
        match nw {
            Option::Some(want) => {
                val post = archive::term_postings(traw, tlen, want, name.len())
                if !post.found {
                    empty = true
                } else {
                    tmp.clear()
                    archive::decode_postings(traw, post.at, post.len, &mut tmp)
                    if first {
                        var c: u64 = 0u64
                        while c < tmp.size() {
                            val v: u32 = tmp.get(c)
                            allowed.push(v)
                            c = c + 1u64
                        }
                        first = false
                    } else {
                        # Both lists are ascending, so the
                        # intersection is one merge pass.
                        merged.clear()
                        var a: u64 = 0u64
                        var b: u64 = 0u64
                        while a < allowed.size() && b < tmp.size() {
                            val x: u32 = allowed.get(a)
                            val y: u32 = tmp.get(b)
                            if x == y {
                                merged.push(x)
                                a = a + 1u64
                                b = b + 1u64
                            } else {
                                if x < y { a = a + 1u64 } else { b = b + 1u64 }
                            }
                        }
                        allowed.clear()
                        var m: u64 = 0u64
                        while m < merged.size() {
                            val v: u32 = merged.get(m)
                            allowed.push(v)
                            m = m + 1u64
                        }
                    }
                    if allowed.size() == 0u64 { empty = true }
                }
            }
            Option::None => { }
        }
        t = t + 1u64
    }

    # `key~needle`: every value of that
    # key containing the needle, unioned,
    # then intersected in like any other
    # term. The dictionary is scanned
    # once per needle.
    var u: u64 = 0u64
    while u < q.subs.size() && !empty {
        val spec: String = q.subs.get(u)
        val colon = String::from_str(":")
        val cpos = spec.find(colon)
        match cpos {
            Option::Some(cp) => {
                # spec is `<mark><key>:<needle>`.
                val mark = spec.substring(0u64, 1u64)
                val anchored = mark.eq_str("^")
                val pfx = spec.substring(1u64, cp + 1u64)
                # The needle is a slice of `spec`, not a
                # string of its own: `ua~` has an empty
                # needle and an empty `String` has no span,
                # which would drop the filter silently.
                val nw = spec.as_span()
                match nw {
                    Option::Some(nsp) => {
                        val hits = archive::terms_matching(
                            traw, 0u64, tlen,
                            pfx.to_str(), nsp,
                            cp + 1u64, spec.len() - (cp + 1u64),
                            anchored)
                        uni.clear()
                        var hi: u64 = 0u64
                        while hi < hits.at.size() {
                            val pat: u64 = hits.at.get(hi)
                            val plen: u64 = hits.len.get(hi)
                            tmp.clear()
                            archive::decode_postings(traw, pat, plen, &mut tmp)
                            # Union: both sides ascending, so
                            # one merge pass, dropping repeats.
                            uni2.clear()
                            var a: u64 = 0u64
                            var b: u64 = 0u64
                            while a < uni.size() || b < tmp.size() {
                                if a >= uni.size() {
                                    val y: u32 = tmp.get(b)
                                    uni2.push(y)
                                    b = b + 1u64
                                } else {
                                    if b >= tmp.size() {
                                        val x: u32 = uni.get(a)
                                        uni2.push(x)
                                        a = a + 1u64
                                    } else {
                                        val x: u32 = uni.get(a)
                                        val y: u32 = tmp.get(b)
                                        if x == y {
                                            uni2.push(x)
                                            a = a + 1u64
                                            b = b + 1u64
                                        } else {
                                            if x < y {
                                                uni2.push(x)
                                                a = a + 1u64
                                            } else {
                                                uni2.push(y)
                                                b = b + 1u64
                                            }
                                        }
                                    }
                                }
                            }
                            uni.clear()
                            var c2: u64 = 0u64
                            while c2 < uni2.size() {
                                val v: u32 = uni2.get(c2)
                                uni.push(v)
                                c2 = c2 + 1u64
                            }
                            hi = hi + 1u64
                        }
                        if uni.size() == 0u64 {
                            empty = true
                        } else {
                            if first {
                                var c3: u64 = 0u64
                                while c3 < uni.size() {
                                    val v: u32 = uni.get(c3)
                                    allowed.push(v)
                                    c3 = c3 + 1u64
                                }
                                first = false
                            } else {
                                merged.clear()
                                var a2: u64 = 0u64
                                var b2: u64 = 0u64
                                while a2 < allowed.size() && b2 < uni.size() {
                                    val x: u32 = allowed.get(a2)
                                    val y: u32 = uni.get(b2)
                                    if x == y {
                                        merged.push(x)
                                        a2 = a2 + 1u64
                                        b2 = b2 + 1u64
                                    } else {
                                        if x < y { a2 = a2 + 1u64 } else { b2 = b2 + 1u64 }
                                    }
                                }
                                allowed.clear()
                                var m2: u64 = 0u64
                                while m2 < merged.size() {
                                    val v: u32 = merged.get(m2)
                                    allowed.push(v)
                                    m2 = m2 + 1u64
                                }
                            }
                            if allowed.size() == 0u64 { empty = true }
                        }
                    }
                    Option::None => { }
                }
            }
            Option::None => { }
        }
        u = u + 1u64
    }
    empty
}

# Which frames hold the records the index left standing.
#
# The record table is a varint stream, so finding a record's place in
# the arena means decoding every record before it -- cheap, and it is
# the walk the body pass does anyway. What it buys is skipping the
# LSZ decode of every frame no surviving record lives in, which on a
# selective query is nearly all of them.
#
# A line can straddle a frame boundary (frames are cut at a fixed raw
# size, not at line ends), so the whole extent is marked, not just the
# offset it starts at.
fn mark_frames(rb: Span<u8>, recs_len: u64, n_records: u64,
               allowed: &Vec<u32>, starts: &Vec<u64>, lens: &Vec<u64>,
               need: &mut Vec<u8>) {
    need.clear()
    var k: u64 = 0u64
    while k < starts.size() {
        need.push(0u8)
        k = k + 1u64
    }
    var rd = ByteReader::new(recs_len)
    var line_at: u64 = 0u64
    var cursor: u64 = 0u64
    # Frames are in arena order and so are the records, so this only
    # moves forward.
    var fc: u64 = 0u64
    var r: u64 = 0u64
    while r < n_records && rd.remaining() > 0u64 {
        val flags = rd.take_varint(rb)
        val line_len = rd.take_varint(rb)
        val ts = rd.take_varint(rb)
        val a1 = rd.take_varint(rb)
        val a2 = rd.take_varint(rb)
        val a3 = rd.take_varint(rb)
        val a4 = rd.take_varint(rb)
        val a5 = rd.take_varint(rb)
        val a6 = rd.take_varint(rb)
        val a7 = rd.take_varint(rb)
        val a8 = rd.take_varint(rb)

        var in_set = false
        while cursor < allowed.size() {
            val a: u32 = allowed.get(cursor)
            if (a as u64) < r {
                cursor = cursor + 1u64
            } else {
                if (a as u64) == r { in_set = true }
                break
            }
        }
        if in_set {
            val last = line_at + line_len
            # Walk forward to the first frame that can hold `line_at`,
            # then mark every frame the line reaches into.
            while fc < starts.size() {
                val st: u64 = starts.get(fc)
                val ln: u64 = lens.get(fc)
                if st + ln <= line_at { fc = fc + 1u64 } else { break }
            }
            var g = fc
            while g < starts.size() {
                val st: u64 = starts.get(g)
                if st >= last { break }
                need.set(g, 1u8)
                g = g + 1u64
            }
        }
        line_at = line_at + line_len
        r = r + 1u64
    }
}

pub fn run(dir: str, q: &Query, crc: &Crc32) -> u64 {
    val segs = logdir::scan_suffix(dir, ".seg")
    val n_segs = segs.size()
    if n_segs == 0u64 {
        println("no segments under {dir}")
        return 1u64
    }

    val watch = Stopwatch::start()
    # Five buffers for the whole run rather than one per segment.
    # v2 allocated a fresh arena inside `expand` for every segment it
    # opened, which on a runtime that never reuses a freed byte meant
    # a scan's footprint grew with the corpus (MEMORY.md). `clear`
    # keeps the room and drops the contents.
    var head_buf = ByteWriter::with_capacity(segfile::data_at() + 64u64)
    var raw = ByteWriter::with_capacity(archive::frame_raw_bytes() + 65536u64)
    var arena = ByteWriter::with_capacity(archive::segment_target_bytes() + 65536u64)
    var recs = ByteWriter::with_capacity(4194304u64)
    var tsec = ByteWriter::with_capacity(4194304u64)

    var hits: Vec<Hit> = Vec::new()
    var texts: Vec<String> = Vec::new()
    var allowed: Vec<u32> = Vec::new()
    # Frame selection: the table's extents, and one byte per frame.
    var ftbuf = ByteWriter::with_capacity(4096u64)
    var fstarts: Vec<u64> = Vec::new()
    var flens: Vec<u64> = Vec::new()
    var need: Vec<u8> = Vec::new()
    var frames_read: u64 = 0u64
    var frames_total: u64 = 0u64

    var considered: u64 = 0u64
    var opened: u64 = 0u64
    var pruned_time: u64 = 0u64
    var pruned_terms: u64 = 0u64
    var examined: u64 = 0u64
    var matched: u64 = 0u64
    var scanned_bytes: u64 = 0u64
    var read_bytes: u64 = 0u64
    var truncated = false

    var si: u64 = 0u64
    while si < n_segs {
        val seg_path: String = segs.get(si)
        val seg_str = seg_path.to_str()
        si = si + 1u64
        considered = considered + 1u64

        val opened_f = File::open(seg_str)
        match opened_f {
            Result::Ok(f) => {
                # 320 bytes decide whether this segment is worth
                # anything else. In v2 the same decision cost a whole
                # `.dat` read, because the header could only be
                # reached by reading the file it starts (R2).
                val h = segfile::head_of(&f, &mut head_buf)
                read_bytes = read_bytes + segfile::data_at()
                var wanted = h.ok
                if !h.ok { println("  {seg_str}: not a segment") }
                if wanted && q.ts_from != 0i64 && h.ts_max < q.ts_from { wanted = false }
                if wanted && q.ts_to != 0i64 && h.ts_min >= q.ts_to { wanted = false }
                if h.ok && !wanted { pruned_time = pruned_time + 1u64 }

                # --- field filters, out of the term index -----------
                var use_terms = false
                var empty = false
                if wanted && q.indexed_count() > 0u64 {
                    if !h.has_terms() {
                        # No index in this segment: the field filters
                        # cannot be answered, so it is skipped rather
                        # than silently ignored.
                        empty = true
                        println("  {seg_str}: no term index, skipped")
                    } else {
                        if !segfile::load_block(&f, h.terms_off, h.terms_len, crc, &mut raw, &mut tsec) {
                            empty = true
                            println("  {seg_str}: term index unreadable")
                        } else {
                            read_bytes = read_bytes + h.terms_len
                            val tw = tsec.span()
                            match tw {
                                Option::Some(traw) => {
                                    use_terms = true
                                    empty = resolve_indexed(traw, tsec.len(), q, &mut allowed)
                                }
                                Option::None => { empty = true }
                            }
                        }
                    }
                }
                if wanted && empty { pruned_terms = pruned_terms + 1u64 }

                # --- the bodies -------------------------------------
                if wanted && !empty {
                    opened = opened + 1u64
                    # The record table comes first now: it says where
                    # each surviving record sits in the arena, which is
                    # what decides the frames worth expanding.
                    var good = true
                    if !segfile::read_range(&f, h.recs_off, h.recs_len, &mut recs) {
                        println("  {seg_str}: record table unreadable")
                        good = false
                    }
                    need.clear()
                    if good && use_terms {
                        if segfile::frame_extents(&f, &h, &mut ftbuf, &mut fstarts, &mut flens) {
                            val rw0 = recs.span()
                            match rw0 {
                                Option::Some(rb0) => {
                                    mark_frames(rb0, recs.len(), h.records, &allowed,
                                                &fstarts, &flens, &mut need)
                                }
                                Option::None => { }
                            }
                        }
                    }
                    # What was actually expanded, which is the point of
                    # the selection: counting the arena's length would
                    # count the frames that were skipped.
                    var expanded_here = h.arena_bytes
                    if need.size() > 0u64 {
                        expanded_here = 0u64
                        var fi2: u64 = 0u64
                        while fi2 < need.size() {
                            val fl: u8 = need.get(fi2)
                            if fl != 0u8 {
                                val fb: u64 = flens.get(fi2)
                                expanded_here = expanded_here + fb
                                frames_read = frames_read + 1u64
                            }
                            fi2 = fi2 + 1u64
                        }
                        frames_total = frames_total + need.size()
                    } else {
                        frames_read = frames_read + h.n_frames
                        frames_total = frames_total + h.n_frames
                    }
                    arena.clear()
                    if good {
                        # An empty `need` expands everything, which is
                        # what a query with no index filter wants.
                        good = segfile::expand_selected(&f, &h, crc, &mut raw, &mut arena, &need)
                        if !good { println("  {seg_str}: frames unreadable") }
                    }
                    if good {
                        scanned_bytes = scanned_bytes + expanded_here
                        read_bytes = read_bytes + h.frames_len + h.recs_len
                        val aw = arena.span()
                        val rw = recs.span()
                        match aw {
                            Option::Some(body) => {
                                match rw {
                                    Option::Some(rb) => {
                                        var rd = ByteReader::new(recs.len())
                                        var line_at: u64 = 0u64
                                        var cursor: u64 = 0u64
                                        var r: u64 = 0u64
                                        while r < h.records && rd.remaining() > 0u64 {
                                            val flags = rd.take_varint(rb)
                                            val line_len = rd.take_varint(rb)
                                            val ts = rd.take_varint(rb) as i64
                                            val host_rel = rd.take_varint(rb)
                                            val host_len = rd.take_varint(rb)
                                            val tag_rel = rd.take_varint(rb)
                                            val tag_len = rd.take_varint(rb)
                                            val labels_rel = rd.take_varint(rb)
                                            val labels_len = rd.take_varint(rb)
                                            val body_rel = rd.take_varint(rb)
                                            val body_len = rd.take_varint(rb)

                                            val dated = (flags & 1u64) != 0u64
                                            val kind = ((flags >> 1u64) & 7u64) as u32

                                            # The postings are ascending and so is
                                            # this walk, so membership is a cursor.
                                            var in_set = true
                                            if use_terms {
                                                in_set = false
                                                while cursor < allowed.size() {
                                                    val a: u32 = allowed.get(cursor)
                                                    if (a as u64) < r {
                                                        cursor = cursor + 1u64
                                                    } else {
                                                        if (a as u64) == r { in_set = true }
                                                        break
                                                    }
                                                }
                                            }

                                            if in_set {
                                                examined = examined + 1u64
                                                if matches(q, body, line_at, line_len, kind, ts, dated,
                                                           host_rel, host_len, tag_rel, tag_len) {
                                                    matched = matched + 1u64
                                                    if hits.size() < max_hits() {
                                                        val hit = Hit { ts: ts, ord: hits.size() }
                                                        hits.push(hit)
                                                        val line = text_of(body, line_at, line_len)
                                                        texts.push(line)
                                                    } else {
                                                        truncated = true
                                                    }
                                                }
                                            }
                                            line_at = line_at + line_len
                                            r = r + 1u64
                                        }
                                    }
                                    Option::None => { }
                                }
                            }
                            Option::None => { }
                        }
                    }
                }
            }
            Result::Err(e) => { println("  {seg_str}: {e}") }
        }
    }

    hits.sort()
    val total = hits.size()
    var shown: u64 = 0u64
    var i: u64 = 0u64
    while i < total && shown < q.limit {
        var pick = i
        if q.desc { pick = total - 1u64 - i }
        val h: Hit = hits.get(pick)
        val line: String = texts.get(h.ord)
        val dt = DateTime::from_unix(h.ts)
        val stamp = time::format(dt, "%Y-%m-%dT%H:%M:%SZ")
        if h.ts == 0i64 {
            println("  -                     {line}")
        } else {
            println("  {stamp}  {line}")
        }
        shown = shown + 1u64
        i = i + 1u64
    }

    val ms = watch.elapsed_ms()
    println("")
    if matched == 0u64 && pruned_terms > 0u64 {
        # A field filter that matches nothing usually means the value
        # is not spelled the way the index spells it: `=` is **exact**,
        # so `ua=MJ12bot` finds nothing while the full user agent
        # string would. Saying so beats an empty answer -- and now
        # there is somewhere to point.
        if q.term_count() > 0u64 {
            println("  (no segment holds every field value -- `=` matches whole values; try `~` for a part)")
        } else {
            println("  (no segment holds a value matching every `~` needle)")
        }
    }
    println("segments         {opened} opened of {considered} ({pruned_time} pruned by time, {pruned_terms} by the index)")
    println("records          {examined} examined, {matched} matched")
    println("bytes read       {read_bytes} off the disk, {scanned_bytes} expanded")
    println("frames           {frames_read} expanded of {frames_total}")
    println("shown            {shown} (limit {q.limit})")
    if truncated { println("truncated        yes -- more than {max_hits()} matches were kept") }
    println("elapsed          {ms} ms")
    0u64
}

# --- field tallies (ONTOLOGY.md O0-a) --------------------------------
#
# "How many of each?" is a different answer shape from "which lines?":
# it returns values, not records. Until the typed term index exists
# (O0-b) the count comes from a full scan, which is exactly the cost
# the index is meant to remove -- so the numbers this prints are also
# the baseline the index will be measured against.

pub struct Tally {
    count: u64,
    idx: u64,
}

impl Ord for Tally {
    fn lt(&self, other: &Self) -> bool {
        if self.count != other.count { return self.count < other.count }
        self.idx > other.idx
    }
}

pub fn field_status() -> u32 { 1u32 }
pub fn field_method() -> u32 { 2u32 }
pub fn field_path() -> u32 { 3u32 }
pub fn field_client() -> u32 { 4u32 }
pub fn field_vhost() -> u32 { 5u32 }
pub fn field_ua() -> u32 { 6u32 }
pub fn field_proto() -> u32 { 7u32 }
pub fn field_host() -> u32 { 8u32 }
pub fn field_tag() -> u32 { 9u32 }
pub fn field_none() -> u32 { 0u32 }

# Whether this key has its own column in the term index.
pub fn is_index_key(key: &String) -> bool {
    if key.eq_str("status") { return true }
    if key.eq_str("method") { return true }
    if key.eq_str("path") { return true }
    if key.eq_str("ip") { return true }
    if key.eq_str("vhost") { return true }
    if key.eq_str("ua") { return true }
    if key.eq_str("host") { return true }
    if key.eq_str("tag") { return true }
    false
}

pub fn field_code(name: str) -> u32 {
    val s = String::from_str(name)
    if s.eq_str("status") { return field_status() }
    if s.eq_str("method") { return field_method() }
    if s.eq_str("path") { return field_path() }
    if s.eq_str("client") { return field_client() }
    if s.eq_str("ip") { return field_client() }
    if s.eq_str("vhost") { return field_vhost() }
    if s.eq_str("ua") { return field_ua() }
    if s.eq_str("proto") { return field_proto() }
    if s.eq_str("host") { return field_host() }
    if s.eq_str("tag") { return field_tag() }
    field_none()
}
