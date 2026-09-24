# The query engine: reading segments back and answering a question
# about them.
#
# The question arrives as one string, because that is what a command
# line and a URL both hand over:
#
#   host=web01 timeout from=2026-09-03T00:00:00Z limit=20 order=desc
#
# Tokens are separated by spaces. A few keys control the search
# itself; a `key=value` whose key is label-shaped is answered from the
# term index; everything else is a substring the line must contain.
# That last rule is what makes `SRC=1.2.3.4` work without being a
# field: it is searched for literally, which is what a person typing
# it means.
#
#   from=<t> to=<t>   time range, half-open. ISO 8601, unix seconds,
#                     or relative (`-1h`, `-30m`, `-2d`)
#   kind=<s>          syslog | apache | datetime | epoch | plain
#   limit=<n>         how many lines to print (default 20)
#   order=asc|desc    by time; desc is the default
#   <key>=<value>     a term, out of the index (`status=404`,
#                     `host=web01`, `app=api`)
#   <anything else>   a substring the line must contain (AND)
#
# **`host` is not the syslog header, it is the sending host.** It is a
# reserved label (DATA_MODEL.md section 2), so the archive writes the
# header and a `host=` label as one term and `fields host` counts them
# together. This used to compare the header here and nowhere else,
# which missed every ingested record and, because it never reached the
# index, opened every segment on the way (`host=web01` went from 0
# hits in 558 ms to 3 in 32 ms when it was folded in). `tag` moved the
# same way.
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
import http
import logdir
import search
import segfile

pub const KIND_ANY: u32 = 255u32
pub const DEFAULT_LIMIT: u64 = 20u64

pub struct Query {
    ts_from: i64,
    ts_to: i64,
    kind: u32,
    limit: u64,
    desc: bool,
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
        val n: Vec<String> = Vec::new()
        val tm: Vec<String> = Vec::new()
        val sb: Vec<String> = Vec::new()
        Query {
            ts_from: 0i64, ts_to: 0i64, kind: KIND_ANY,
            limit: DEFAULT_LIMIT, desc: true,
            needles: n, terms: tm, subs: sb,
        }
    }
    pub fn needle_count(&self) -> u64 { self.needles.size() }
    pub fn term_count(&self) -> u64 { self.terms.size() }
    pub fn sub_count(&self) -> u64 { self.subs.size() }
    # Whether anything at all is answered from the index.
    pub fn indexed_count(&self) -> u64 { self.terms.size() + self.subs.size() }
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
fn only_digits(s: &String) -> bool {
    val n = s.len()
    if n == 0u64 { return false }
    var i: u64 = 0u64
    while i < n {
        val c: u8 = s.get(i)
        if c < '0' || c > '9' { return false }
        i = i + 1u64
    }
    true
}

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
        val secs = match unit {
            'm' => mag * 60u64,
            'h' => mag * 3600u64,
            'd' => mag * 86400u64,
            _ => mag,
        }
        return now - (secs as i64)
    }
    # Unix seconds only when the whole thing is digits. Testing the
    # first character alone swallowed `2030-01-01`: ten characters
    # starting with a digit, so it took the numeric path, failed to
    # parse, and came back 0 -- which reads as "no bound". A time
    # filter that silently is not there looks exactly like an answer.
    if only_digits(&s) && s.len() <= 11u64 {
        return (parse::to_u64(text) ?? 0u64) as i64
    }
    val dt = time::parse_iso8601(text)
    match dt {
        Result::Ok(v) => { v.to_unix() }
        Result::Err(e) => { 0i64 }
    }
}

fn kind_code(text: str) -> u32 {
    match text {
        "syslog" => LineShape::Syslog as u32,
        "datetime" => LineShape::Datetime as u32,
        "apache" => LineShape::Apache as u32,
        "epoch" => LineShape::Epoch as u32,
        "plain" => LineShape::Plain as u32,
        _ => KIND_ANY,
    }
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
        val tok: &String = parts.borrow(i)
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
                    val ks = key.to_str()
                    # `host` and `tag` are not special: they are keys
                    # the index knows, and they fall through to the
                    # term path below. `host` is a *reserved label*
                    # (DATA_MODEL.md section 2) — the sending host,
                    # however it arrived — so the syslog header and a
                    # `host=` label are one key, which is already how
                    # the archive writes them and how `fields host`
                    # counts them. Comparing only the syslog header
                    # here meant `host=web01` missed every ingested
                    # record and opened all twelve segments on the way.
                    #
                    # `top=` is read by the caller, which decides
                    # between a traversal and a whole distribution. It
                    # is consumed here so it does not fall through and
                    # become a body substring -- searching lines for
                    # the text `top=path` is nobody's intent.
                    match ks {
                        "from" => { q.ts_from = parse_time(vs, now)  handled = true }
                        "to" => { q.ts_to = parse_time(vs, now)  handled = true }
                        "kind" => { q.kind = kind_code(vs)  handled = true }
                        "top" => { handled = true }
                        "order" => {
                            q.desc = vs == "desc"
                            handled = true
                        }
                        "limit" => {
                            val n = parse::to_u64(vs)
                            match n {
                                Result::Ok(v) => { q.limit = v }
                                Result::Err(e) => { }
                            }
                            handled = true
                        }
                        _ => {}
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
                # A label-shaped key becomes a term; anything else is
                # a substring, spelled exactly as it was typed. A key
                # with a dot or a capital in it is not a label, so
                # `Host=x` searches the text.
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
           kind: u32, ts: i64, dated: bool) -> bool {
    if q.kind != KIND_ANY && q.kind != kind { return false }
    if q.ts_from != 0i64 {
        if !dated { return false }
        if ts < q.ts_from { return false }
    }
    if q.ts_to != 0i64 {
        if !dated { return false }
        if ts >= q.ts_to { return false }
    }
    var k: u64 = 0u64
    while k < q.needles.size() {
        val needle: &String = q.needles.borrow(k)
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
# and by `MAX_HITS` in the worst.
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
pub const MAX_HITS: u64 = 20000u64

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
        val name: &String = q.terms.borrow(t)
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
        val spec: &String = q.subs.borrow(u)
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
# Which frames hold a record the index let through.
#
# The rows are already decoded (`archive::decode_records`), so this
# visits the candidates and nothing else: each one's place in the arena
# is read off the `line_at` column by ordinal. What it buys is skipping
# the LSZ decode of every frame no surviving record lives in, which on
# a selective query is nearly all of them.
#
# A line can straddle a frame boundary (frames are cut at a fixed raw
# size, not at line ends), so the whole extent is marked, not just the
# offset it starts at.
fn mark_frames(rows: &SoaVec<RecRow>, allowed: &Vec<u32>,
               starts: &Vec<u64>, lens: &Vec<u64>, need: &mut Vec<u8>) {
    need.clear()
    val n_frames = starts.size()
    for k in 0u64..n_frames { need.push(0u8) }
    val n = rows.size()
    val ats = rows.line_at
    val line_lens = rows.line_len
    # Candidates ascend, and so do their offsets, so the frame cursor
    # only moves forward.
    var fc: u64 = 0u64
    for k in 0u64..allowed.size() {
        val a: u32 = allowed.get(k)
        val r = a as u64
        if r >= n { break }
        val line_at: u64 = ats.get(r)
        val ll: u32 = line_lens.get(r)
        val last = line_at + (ll as u64)
        # Walk forward to the first frame that can hold `line_at`,
        # then mark every frame the line reaches into.
        while fc < n_frames {
            val st: u64 = starts.get(fc)
            val ln: u64 = lens.get(fc)
            if st + ln <= line_at { fc = fc + 1u64 } else { break }
        }
        var g = fc
        while g < n_frames {
            val st: u64 = starts.get(g)
            if st >= last { break }
            need.set(g, 1u8)
            g = g + 1u64
        }
    }
}

# What the walk cost, so a caller can report it without having
# watched. Scalars only: the search hands it back through a `&mut`,
# and every field here is a number somebody asked for in QUERY.md
# section 7 -- "why was that slow" is meant to be answerable from the
# answer itself.
pub struct SearchStats {
    considered: u64,
    opened: u64,
    pruned_time: u64,
    pruned_terms: u64,
    examined: u64,
    matched: u64,
    read_bytes: u64,
    scanned_bytes: u64,
    frames_read: u64,
    frames_total: u64,
    truncated: bool,
    ms: u64,
}

impl SearchStats {
    pub fn new() -> Self {
        val s = SearchStats {
            considered: 0u64, opened: 0u64,
            pruned_time: 0u64, pruned_terms: 0u64,
            examined: 0u64, matched: 0u64,
            read_bytes: 0u64, scanned_bytes: 0u64,
            frames_read: 0u64, frames_total: 0u64,
            truncated: false, ms: 0u64,
        }
        s
    }
}

# Walk the segments the caller found and collect what matches.
#
# **Collecting and reporting are separate.** The walk fills `hits`,
# `texts` and `st`; who renders them and where decides nothing here.
# That split is what lets the same search answer a terminal and an
# HTTP response without the engine knowing which.
#
# The segment list is handed in rather than walked: which files exist
# is a question about mounts and catalogs (`main.t::segments_of`), and
# a query has no business knowing how that was answered.
pub fn search(dir: str, segs: &Vec<String>, q: &Query, crc: &Crc32,
              hits: &mut Vec<Hit>, texts: &mut Vec<String>,
              st: &mut SearchStats) {
    val n_segs = segs.size()
    val watch = Stopwatch::start()
    # Five buffers for the whole run rather than one per segment.
    # v2 allocated a fresh arena inside `expand` for every segment it
    # opened, which on a runtime that never reuses a freed byte meant
    # a scan's footprint grew with the corpus (MEMORY.md). `clear`
    # keeps the room and drops the contents.
    var head_buf = ByteWriter::with_capacity(segfile::DATA_AT + 64u64)
    var raw = ByteWriter::with_capacity(archive::FRAME_RAW_BYTES + 65536u64)
    var arena = ByteWriter::with_capacity(archive::SEGMENT_TARGET_BYTES + 65536u64)
    var recs = ByteWriter::with_capacity(4194304u64)
    var tsec = ByteWriter::with_capacity(4194304u64)
    # The record table of the segment in hand, by column. Cleared per
    # segment rather than rebuilt, for the same reason as the buffers.
    var rows: soa Vec<RecRow> = SoaVec::new()

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
        val seg_path: &String = segs.borrow(si)
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
                read_bytes = read_bytes + segfile::DATA_AT
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
                    rows.clear()
                    if good {
                        val rw0 = recs.span()
                        match rw0 {
                            Option::Some(rb0) => {
                                # Rows past the last candidate are never
                                # looked at, so they are not decoded.
                                var upto = h.records
                                if use_terms && allowed.size() > 0u64 {
                                    val last: u32 = allowed.get(allowed.size() - 1u64)
                                    if (last as u64) + 1u64 < upto { upto = (last as u64) + 1u64 }
                                }
                                archive::decode_records(rb0, recs.len(), upto, &mut rows)
                            }
                            Option::None => { }
                        }
                    }
                    need.clear()
                    if good && use_terms {
                        if segfile::frame_extents(&f, &h, &mut ftbuf, &mut fstarts, &mut flens) {
                            mark_frames(&rows, &allowed, &fstarts, &flens, &mut need)
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
                        match aw {
                            Option::Some(body) => {
                                # With an index filter only the candidates
                                # are visited, by ordinal; without one,
                                # every row is. Either way a record is
                                # read out of the columns it needs.
                                val n_rows = rows.size()
                                var n_visit = n_rows
                                if use_terms { n_visit = allowed.size() }
                                val ats = rows.line_at
                                val line_lens = rows.line_len
                                val stamps = rows.ts
                                val flag_col = rows.flags
                                val host_rels = rows.host_rel
                                val host_lens = rows.host_len
                                val tag_rels = rows.tag_rel
                                val tag_lens = rows.tag_len
                                for k in 0u64..n_visit {
                                    var r = k
                                    if use_terms {
                                        val a: u32 = allowed.get(k)
                                        r = a as u64
                                    }
                                    if r >= n_rows { break }
                                    val line_at: u64 = ats.get(r)
                                    val ll: u32 = line_lens.get(r)
                                    val line_len = ll as u64
                                    val ts: i64 = stamps.get(r)
                                    val fl: u8 = flag_col.get(r)
                                    val flags = fl as u64
                                    val hr: u32 = host_rels.get(r)
                                    val hl: u32 = host_lens.get(r)
                                    val tr: u32 = tag_rels.get(r)
                                    val tl: u32 = tag_lens.get(r)

                                    val dated = (flags & 1u64) != 0u64
                                    val kind = ((flags >> 1u64) & 7u64) as u32

                                    examined = examined + 1u64
                                    if matches(q, body, line_at, line_len, kind, ts, dated) {
                                        matched = matched + 1u64
                                        if hits.size() < MAX_HITS {
                                            val hit = Hit { ts: ts, ord: hits.size() }
                                            hits.push(hit)
                                            val line = text_of(body, line_at, line_len)
                                            texts.push(line)
                                        } else {
                                            truncated = true
                                        }
                                    }
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
    st.considered = considered
    st.opened = opened
    st.pruned_time = pruned_time
    st.pruned_terms = pruned_terms
    st.examined = examined
    st.matched = matched
    st.read_bytes = read_bytes
    st.scanned_bytes = scanned_bytes
    st.frames_read = frames_read
    st.frames_total = frames_total
    st.truncated = truncated
    st.ms = watch.elapsed_ms()
}

# The terminal's rendering of a search: the matching lines, then why
# it cost what it did.
pub fn run(dir: str, segs: &Vec<String>, q: &Query, crc: &Crc32) -> u64 {
    if segs.size() == 0u64 {
        println("no segments under {dir}")
        return 1u64
    }
    var hits: Vec<Hit> = Vec::new()
    var texts: Vec<String> = Vec::new()
    var st = SearchStats::new()
    search(dir, segs, q, crc, &mut hits, &mut texts, &mut st)

    val total = hits.size()
    var shown: u64 = 0u64
    var i: u64 = 0u64
    while i < total && shown < q.limit {
        var pick = i
        if q.desc { pick = total - 1u64 - i }
        val h: Hit = hits.get(pick)
        val line: &String = texts.borrow(h.ord)
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

    println("")
    if st.matched == 0u64 && st.pruned_terms > 0u64 {
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
    println("segments         {st.opened} opened of {st.considered} ({st.pruned_time} pruned by time, {st.pruned_terms} by the index)")
    println("records          {st.examined} examined, {st.matched} matched")
    println("bytes read       {st.read_bytes} off the disk, {st.scanned_bytes} expanded")
    println("frames           {st.frames_read} expanded of {st.frames_total}")
    println("shown            {shown} (limit {q.limit})")
    if st.truncated { println("truncated        yes -- more than {MAX_HITS} matches were kept") }
    println("elapsed          {st.ms} ms")
    0u64
}

# ---------------------------------------------------------------------
# Tallies over the term dictionary
#
# "How many of each?" is answered without opening a frame: one
# section per segment, folded across segments. The same walk serves
# `fields <key>` on the command line and `/v1/labels` over HTTP,
# which is why it lives here rather than in either caller.

pub struct FieldTally {
    names: Vec<String>,
    counts: Vec<u64>,
    terms: u64,
    segments: u64,
}

impl FieldTally {
    pub fn new() -> Self {
        var names: Vec<String> = Vec::new()
        var counts: Vec<u64> = Vec::new()
        val t = FieldTally {
            names: names, counts: counts, terms: 0u64, segments: 0u64,
        }
        t
    }
    pub fn size(&self) -> u64 { self.names.size() }
}

# Every value under `prefix` (`status:`), or -- with `keys_only` --
# every key the dictionary holds.
#
# Values repeat across segments, so they are folded through an
# open-addressing table. A linear scan over the values seen so far
# was the first attempt and it is quadratic: for `ip` (11,293
# distinct) it cost more than the full scan the index replaces.
# Every stream (label set) the segments hold, folded across them.
#
# `/v1/streams` is the caller: the UI asks for it whenever somebody
# picks a label, so the work is a section read per segment and no
# frame expansion at all (DATA_MODEL.md section 3, the table is
# written while archiving).
pub struct StreamTally {
    texts: Vec<String>,
    counts: Vec<u64>,
    ts_min: Vec<i64>,
    ts_max: Vec<i64>,
    segments: u64,
}

impl StreamTally {
    pub fn new() -> Self {
        val t: Vec<String> = Vec::new()
        val c: Vec<u64> = Vec::new()
        val a: Vec<i64> = Vec::new()
        val b: Vec<i64> = Vec::new()
        StreamTally { texts: t, counts: c, ts_min: a, ts_max: b, segments: 0u64 }
    }
    pub fn size(&self) -> u64 { self.texts.size() }
}

pub fn streams(segs: &Vec<String>, crc: &Crc32) -> StreamTally {
    var out = StreamTally::new()
    var head_buf = ByteWriter::with_capacity(segfile::DATA_AT + 64u64)
    var raw = ByteWriter::with_capacity(1048576u64)
    var ssec = ByteWriter::with_capacity(1048576u64)

    var si: u64 = 0u64
    while si < segs.size() {
        val seg_path: &String = segs.borrow(si)
        val seg_str = seg_path.to_str()
        si = si + 1u64
        val opened_f = File::open(seg_str)
        match opened_f {
            Result::Ok(f) => {
                val h = segfile::head_of(&f, &mut head_buf)
                var got = h.ok && h.has_streams()
                if got {
                    if !segfile::load_block(&f, h.strs_off, h.strs_len, crc, &mut raw, &mut ssec) { got = false }
                }
                if got {
                    out.segments = out.segments + 1u64
                    val sw = ssec.span()
                    match sw {
                        Option::Some(sraw) => {
                            val rows = archive::streams_of(sraw, ssec.len())
                            fold_streams(&rows, &mut out)
                        }
                        Option::None => { }
                    }
                }
            }
            Result::Err(e) => { }
        }
    }
    out
}

# Streams are few, so the fold is a linear search over what is held
# so far -- the quadratic blow-up that made `tally` use a hash table
# needs thousands of distinct values, and a label set per sender is
# not that.
fn fold_streams(rows: &StreamRows, out: &mut StreamTally) {
    var i: u64 = 0u64
    while i < rows.size() {
        val text: &String = rows.texts.borrow(i)
        val count: u64 = rows.counts.get(i)
        val lo: i64 = rows.ts_min.get(i)
        val hi: i64 = rows.ts_max.get(i)
        var at = out.texts.size()
        var found = false
        var k: u64 = 0u64
        while k < out.texts.size() && !found {
            val have: &String = out.texts.borrow(k)
            if have.eq(&text) {
                at = k
                found = true
            }
            k = k + 1u64
        }
        if found {
            val c: u64 = out.counts.get(at)
            out.counts.set(at, c + count)
            if lo <= hi {
                val was_lo: i64 = out.ts_min.get(at)
                val was_hi: i64 = out.ts_max.get(at)
                if lo < was_lo { out.ts_min.set(at, lo) }
                if hi > was_hi { out.ts_max.set(at, hi) }
            }
        } else {
            out.texts.push(text.clone())
            out.counts.push(count)
            out.ts_min.push(lo)
            out.ts_max.push(hi)
        }
        i = i + 1u64
    }
}

pub fn tally(segs: &Vec<String>, prefix: str, keys_only: bool,
             crc: &Crc32) -> FieldTally {
    var out = FieldTally::new()
    var head_buf = ByteWriter::with_capacity(segfile::DATA_AT + 64u64)
    var raw = ByteWriter::with_capacity(4194304u64)
    var tsec = ByteWriter::with_capacity(4194304u64)

    val slot_bits: u64 = 65536u64
    var slots: Vec<u64> = Vec::with_capacity(slot_bits)
    var sz: u64 = 0u64
    while sz < slot_bits {
        slots.push(0u64)
        sz = sz + 1u64
    }
    var hashes: Vec<u64> = Vec::new()

    var si: u64 = 0u64
    while si < segs.size() {
        val seg_path: &String = segs.borrow(si)
        val seg_str = seg_path.to_str()
        si = si + 1u64
        val opened_f = File::open(seg_str)
        match opened_f {
            Result::Ok(f) => {
                val h = segfile::head_of(&f, &mut head_buf)
                var got = h.ok && h.has_terms()
                if got {
                    out.segments = out.segments + 1u64
                    if !segfile::load_block(&f, h.terms_off, h.terms_len, crc, &mut raw, &mut tsec) { got = false }
                }
                if got {
                    val tw = tsec.span()
                    match tw {
                        Option::Some(traw) => {
                            val raw_len = tsec.len()
                            var head = ByteReader::new(raw_len)
                            out.terms = out.terms + head.take_u32(traw)
                            if keys_only {
                                val hits = archive::term_keys(traw, 0u64, raw_len)
                                fold(&hits, traw, &mut slots, &mut hashes, &mut out)
                            } else {
                                val hits = archive::terms_with_prefix(traw, 0u64, raw_len, prefix)
                                fold(&hits, traw, &mut slots, &mut hashes, &mut out)
                            }
                        }
                        Option::None => { }
                    }
                }
            }
            Result::Err(e) => { }
        }
    }
    out
}

fn fold(hits: &TermHits, traw: Span<u8>, slots: &mut Vec<u64>,
        hashes: &mut Vec<u64>, out: &mut FieldTally) {
    val mask = slots.size() - 1u64
    var h: u64 = 0u64
    while h < hits.names.size() {
        val packed: u64 = hits.names.get(h)
        val at = record::span_start(packed)
        val vlen = record::span_len(packed)
        val c: u64 = hits.counts.get(h)
        val key = extract::hash_span(traw, at, vlen)
        var slot = key & mask
        var placed = false
        while !placed {
            val cell: u64 = slots.get(slot)
            if cell == 0u64 {
                val pos = hashes.size()
                hashes.push(key)
                out.counts.push(c)
                val nm = text_of(traw, at, vlen)
                out.names.push(nm)
                slots.set(slot, pos + 1u64)
                placed = true
            } else {
                val pos = cell - 1u64
                val hv: u64 = hashes.get(pos)
                if hv == key {
                    val prev: u64 = out.counts.get(pos)
                    out.counts.set(pos, prev + c)
                    placed = true
                } else {
                    slot = (slot + 1u64) & mask
                }
            }
        }
        h = h + 1u64
    }
}

# ---------------------------------------------------------------------
# Renderings
#
# Three shapes over the same result (HTTP_API.md section 2): lines for
# a terminal or `grep`, one object per line for a stream, and one
# object for a UI that wants the statistics with the records. None of
# them re-reads a segment -- the walk already produced everything.

pub enum OutputFormat {
    Text,
    Ndjson,
    Json,
}

# Which record to show `i`-th, honouring `desc`.
fn pick_at(hits: &Vec<Hit>, q: &Query, i: u64) -> u64 {
    if q.desc { return hits.size() - 1u64 - i }
    i
}

fn put_stamp(out: &mut ByteWriter, ts: i64) {
    val dt = DateTime::from_unix(ts)
    val stamp = time::format(dt, "%Y-%m-%dT%H:%M:%SZ")
    out.put_str(stamp)
}

pub fn render_text(hits: &Vec<Hit>, texts: &Vec<String>, q: &Query,
                   out: &mut ByteWriter) -> u64 {
    val total = hits.size()
    var shown: u64 = 0u64
    var i: u64 = 0u64
    while i < total && shown < q.limit {
        val h: Hit = hits.get(pick_at(hits, q, i))
        val line: &String = texts.borrow(h.ord)
        if h.ts == 0i64 {
            out.put_str("-                     ")
        } else {
            put_stamp(out, h.ts)
            out.put_str("  ")
        }
        out.put_str(line.to_str())
        out.put_u8('\n')
        shown = shown + 1u64
        i = i + 1u64
    }
    shown
}

fn put_record(out: &mut ByteWriter, ts: i64, line: &String) {
    out.put_str(r#"{"ts":"#)
    if ts == 0i64 {
        out.put_str("null")
    } else {
        out.put_u8('"')
        put_stamp(out, ts)
        out.put_u8('"')
    }
    out.put_str(",\"body\":")
    http::put_json_string(out, line)
    out.put_str("}")
}

pub fn render_ndjson(hits: &Vec<Hit>, texts: &Vec<String>, q: &Query,
                     out: &mut ByteWriter) -> u64 {
    val total = hits.size()
    var shown: u64 = 0u64
    var i: u64 = 0u64
    while i < total && shown < q.limit {
        val h: Hit = hits.get(pick_at(hits, q, i))
        val line: &String = texts.borrow(h.ord)
        put_record(out, h.ts, &line)
        out.put_u8('\n')
        shown = shown + 1u64
        i = i + 1u64
    }
    shown
}

# One object, records and statistics together. **The statistics are
# not decoration**: QUERY.md section 7 makes "why was that slow" part
# of the answer, and a UI that cannot show the reason cannot tell a
# selective query from an unselective one.
pub fn render_json(hits: &Vec<Hit>, texts: &Vec<String>, q: &Query,
                   st: &SearchStats, out: &mut ByteWriter) -> u64 {
    out.put_str(r#"{"records":["#)
    val total = hits.size()
    var shown: u64 = 0u64
    var i: u64 = 0u64
    while i < total && shown < q.limit {
        if shown > 0u64 { out.put_u8(',') }
        val h: Hit = hits.get(pick_at(hits, q, i))
        val line: &String = texts.borrow(h.ord)
        put_record(out, h.ts, &line)
        shown = shown + 1u64
        i = i + 1u64
    }
    out.put_str(r#"],"stats":{"segments_opened":"#)
    out.put_str("{st.opened}")
    out.put_str(",\"segments_considered\":")
    out.put_str("{st.considered}")
    out.put_str(",\"pruned_by_time\":")
    out.put_str("{st.pruned_time}")
    out.put_str(",\"pruned_by_index\":")
    out.put_str("{st.pruned_terms}")
    out.put_str(",\"records_examined\":")
    out.put_str("{st.examined}")
    out.put_str(",\"records_matched\":")
    out.put_str("{st.matched}")
    out.put_str(",\"bytes_read\":")
    out.put_str("{st.read_bytes}")
    out.put_str(",\"bytes_expanded\":")
    out.put_str("{st.scanned_bytes}")
    out.put_str(",\"frames_expanded\":")
    out.put_str("{st.frames_read}")
    out.put_str(",\"frames_total\":")
    out.put_str("{st.frames_total}")
    out.put_str(",\"shown\":")
    out.put_str("{shown}")
    out.put_str(",\"truncated\":")
    if st.truncated { out.put_str("true") } else { out.put_str("false") }
    out.put_str(",\"elapsed_ms\":")
    out.put_str("{st.ms}")
    out.put_str("}}}}\n")
    shown
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

# A field `fields` can count. `Unknown` is a name that is none of them.
pub enum Field {
    Unknown,
    Status,
    Method,
    Path,
    Client,
    Vhost,
    Ua,
    Proto,
    Host,
    Tag,
}

# Whether `key=value` names a term rather than a piece of text.
#
# **Any label key does.** It used to be a list of eight -- the fields
# `extract.t` pulls out of an apache line -- because nothing else was
# ever in the dictionary. Labels are indexed now (`app=api` on an
# ingested record), and the dictionary holds whatever key was
# written, so the question is no longer "do I know this name" but "is
# this shaped like a label": `[a-z0-9_]{1,32}`, the rule from
# DATA_MODEL.md section 2.
#
# The consequence is worth stating: `level=error` against an archive
# with no `level` label now matches nothing instead of searching the
# text for `level=error`. That is what `=` means -- a value in full
# -- and `~` is the one that looks inside. The empty answer says so
# ("no segment holds every field value").
#
# The control words are refused explicitly. They are read before this
# is reached, but `limit=5` must never become a term by another road.
pub fn is_index_key(key: &String) -> bool {
    val n = key.len()
    if n == 0u64 || n > 32u64 { return false }
    val word = key.to_str()
    match word {
        "from" | "to" | "limit" | "order" | "kind" | "top" => { return false }
        _ => {}
    }
    var i: u64 = 0u64
    while i < n {
        val c: u8 = key.get(i)
        val ok = (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '_'
        if !ok { return false }
        i = i + 1u64
    }
    true
}

pub fn field_code(name: str) -> Field {
    match name {
        "status" => Field::Status,
        "method" => Field::Method,
        "path" => Field::Path,
        "client" | "ip" => Field::Client,
        "vhost" => Field::Vhost,
        "ua" => Field::Ua,
        "proto" => Field::Proto,
        "host" => Field::Host,
        "tag" => Field::Tag,
        _ => Field::Unknown,
    }
}
