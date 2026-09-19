# The catalog: which segments exist, and what is in them.
#
# **The catalog is a cache.** Everything in it can be recovered by
# walking `seg/` and reading 320 bytes per file (`rebuild`, below).
# It exists so that opening a mount with 100,000 segments costs one
# read of a 6.4 MB file instead of 100,000 `open` calls, and so that
# a query with a time range can throw away most of them before it
# touches a disk (STORAGE_FORMAT.md §6, QUERY.md §2 step 1).
#
# Because it is a cache, every failure mode here has the same answer:
# take what checks out, drop the rest, and rebuild when asked. There
# is no repair-in-place and no error that makes a mount unreadable.
#
# Two files hold it, per generation:
#
#   meta/catalog.<gen>.snap   all live rows, 64 bytes each
#   meta/catalog.<gen>.log    what changed since, append only
#
# A generation is replaced by writing the next one into `tmp/` and
# renaming it into `meta/`. The rename is the moment the new
# generation becomes real; a crash on either side of it leaves one
# consistent generation behind, never half of two.

import std.fs
import std.parse
import std.path
import std.time
import logdir
import segfile

# One row is 64 bytes: five u64 and six u32. Fixed length is what
# lets `file_size` check the row count before anything is decoded.
pub fn row_bytes() -> u64 { 64u64 }

# Magic, gen, rows, crc -- and room to grow without moving the rows.
pub fn snap_head_bytes() -> u64 { 32u64 }

# Magic, total length, op, body crc.
pub fn jrec_head_bytes() -> u64 { 16u64 }

# A row describes either a segment or the archive a run of segments
# was merged into (DATA_MODEL.md §5). Same format, different grain.
pub fn kind_segment() -> u64 { 0u64 }
pub fn kind_archive() -> u64 { 1u64 }

pub fn op_add() -> u64 { 1u64 }
pub fn op_remove() -> u64 { 2u64 }

# Why a segment left the catalog. Kept because "it is gone" and "we
# dropped it on purpose" want different answers from an operator.
pub fn why_retention() -> u64 { 1u64 }
pub fn why_missing() -> u64 { 2u64 }
pub fn why_merged() -> u64 { 3u64 }

# ---------------------------------------------------------------------

# One catalog row. `ok` is not on disk: it is what `take_row` reports
# after checking the row's own CRC.
pub struct CatRow {
    ok: bool,
    segid: u64,
    ts_min: i64,
    ts_max: i64,
    records: u64,
    seg_bytes: u64,
    index_bytes: u64,
    kind: u64,
    streams: u64,
    terms: u64,
    daykey: u64,
}

impl CatRow {
    pub fn empty() -> Self {
        CatRow {
            ok: false, segid: 0u64, ts_min: 0i64, ts_max: 0i64,
            records: 0u64, seg_bytes: 0u64, index_bytes: 0u64,
            kind: kind_segment(), streams: 0u64, terms: 0u64,
            daykey: 0u64,
        }
    }

    # Does this segment hold anything in `[from, to)`?
    #
    # `ts_max` is inclusive (it is the timestamp of a record that is
    # really there), so the upper test is `<=` on one side and `<` on
    # the other. Getting this wrong drops a segment whose only
    # matching record is its last one.
    pub fn overlaps(&self, from: i64, until: i64) -> bool {
        if self.ts_max < from { return false }
        if self.ts_min >= until { return false }
        true
    }
}

# `YYYYMMDD` for an instant, UTC, the way the directory tree is cut.
pub fn daykey_of(secs: i64) -> u64 {
    val dt = DateTime::from_unix(secs)
    val y = dt.year()
    val m = dt.month() as i64
    val d = dt.day() as i64
    val k = (y * 10000i64) + (m * 100i64) + d
    if k < 0i64 { return 0u64 }
    k as u64
}

# The day a segment actually lives under, read from its own path.
#
# The writer files a segment under its `ts_min` -- except that a
# segment whose records carry no date at all has nowhere to go but
# the day it was written. So **the path is the authority on the day
# key, not the header**: deriving it from `ts_min` a second time
# sends a reader to `1970/01/01` looking for a file that is under
# `2026/09/11`, and the segment silently disappears from every query.
#
# Returns 0 for a path that is not `.../YYYY/MM/DD/<name>`.
pub fn daykey_of_path(path: &String) -> u64 {
    var cuts: Vec<u64> = Vec::new()
    var i = path.len()
    while i > 0u64 && cuts.size() < 4u64 {
        i = i - 1u64
        val c: u8 = path.get(i)
        if c == '/' { cuts.push(i) }
    }
    if cuts.size() < 4u64 { return 0u64 }
    val y = number_in(path, cuts.get(3u64) + 1u64, cuts.get(2u64))
    val m = number_in(path, cuts.get(2u64) + 1u64, cuts.get(1u64))
    val d = number_in(path, cuts.get(1u64) + 1u64, cuts.get(0u64))
    if y == 0u64 || m == 0u64 || d == 0u64 { return 0u64 }
    if m > 12u64 || d > 31u64 { return 0u64 }
    (y * 10000u64) + (m * 100u64) + d
}

fn number_in(s: &String, from: u64, end: u64) -> u64 {
    if end <= from { return 0u64 }
    val part = s.substring(from, end)
    val got = parse::to_u64(part.to_str())
    match got {
        Result::Ok(v) => { v }
        Result::Err(e) => { 0u64 }
    }
}

# Where a row's file lives under `mount`.
pub fn seg_path(mount: str, r: &CatRow) -> String {
    val y = r.daykey / 10000u64
    val m = (r.daykey / 100u64) % 100u64
    val d = r.daykey % 100u64
    val id = r.segid
    val ext = if r.kind == kind_archive() { ".arc.seg" } else { ".seg" }
    val s = "{mount}/seg/{y:04}/{m:02}/{d:02}/{id:012}{ext}"
    val out = String::from_str(s)
    out
}

# The directory that file lives in, for `mkdir_all` and for the
# `remove_dir` that retention does once a day empties out.
pub fn day_path(mount: str, daykey: u64) -> String {
    val y = daykey / 10000u64
    val m = (daykey / 100u64) % 100u64
    val d = daykey % 100u64
    val s = "{mount}/seg/{y:04}/{m:02}/{d:02}"
    val out = String::from_str(s)
    out
}

pub fn meta_path(mount: str) -> String {
    val s = "{mount}/meta"
    val out = String::from_str(s)
    out
}

pub fn tmp_path(mount: str) -> String {
    val s = "{mount}/tmp"
    val out = String::from_str(s)
    out
}

pub fn snap_path(mount: str, gen: u64) -> String {
    val s = "{mount}/meta/catalog.{gen:06}.snap"
    val out = String::from_str(s)
    out
}

pub fn log_path(mount: str, gen: u64) -> String {
    val s = "{mount}/meta/catalog.{gen:06}.log"
    val out = String::from_str(s)
    out
}

# ---------------------------------------------------------------------
# Rows on the wire

fn put_row(w: &mut ByteWriter, r: &CatRow, crc: &Crc32) {
    val at = w.len()
    w.put_u64(r.segid)
    w.put_i64(r.ts_min)
    w.put_i64(r.ts_max)
    w.put_u64(r.records)
    w.put_u64(r.seg_bytes)
    w.put_u32(r.index_bytes)
    w.put_u32(r.kind)
    w.put_u32(r.streams)
    w.put_u32(r.terms)
    w.put_u32(r.daykey)
    var sum: u64 = 0u64
    val sp = w.span()
    match sp {
        Option::Some(b) => { sum = crc.of(b, at, 60u64) }
        Option::None => { }
    }
    w.put_u32(sum)
}

fn take_row(rd: &mut ByteReader, b: Span<u8>, crc: &Crc32) -> CatRow {
    var r = CatRow::empty()
    val at = rd.position()
    r.segid = rd.take_u64(b)
    r.ts_min = rd.take_i64(b)
    r.ts_max = rd.take_i64(b)
    r.records = rd.take_u64(b)
    r.seg_bytes = rd.take_u64(b)
    r.index_bytes = rd.take_u32(b)
    r.kind = rd.take_u32(b)
    r.streams = rd.take_u32(b)
    r.terms = rd.take_u32(b)
    r.daykey = rd.take_u32(b)
    val want = rd.take_u32(b)
    val got = crc.of(b, at, 60u64)
    r.ok = got == want
    r
}

# A row read straight off a segment's own header. This is the only
# place that knows how to turn one into the other, so `rebuild` and
# the archiver cannot drift apart.
pub fn row_of_head(h: &SegHead, seg_bytes: u64, is_archive: bool) -> CatRow {
    var r = CatRow::empty()
    r.ok = h.ok
    r.segid = h.segid
    r.ts_min = h.ts_min
    r.ts_max = h.ts_max
    r.records = h.records
    r.seg_bytes = seg_bytes
    r.index_bytes = h.recs_len + h.ftab_len + h.terms_len + h.links_len
        + h.objs_len + h.strs_len
    r.kind = if is_archive { kind_archive() } else { kind_segment() }
    # How many streams the segment holds is in its stream table (kind
    # 9), not in the 320 bytes this reads, and a row built from the
    # header alone must not invent it. It held the *frame* count,
    # which is a different number wearing this one's name.
    r.streams = 0u64
    r.terms = h.terms_len
    r.daykey = daykey_of(h.ts_min)
    r
}

# ---------------------------------------------------------------------
# Whole-file helpers. Small enough to keep here rather than grow a
# module that only these three callers would use.

fn read_whole(path: str, out: &mut ByteWriter) -> bool {
    val opened = File::open(path)
    var ok = false
    match opened {
        Result::Ok(f) => {
            val sz = f.size()
            match sz {
                Result::Ok(n) => {
                    if n == 0u64 {
                        out.clear()
                        ok = true
                    } else {
                        ok = segfile::read_range(&f, 0u64, n, out)
                    }
                }
                Result::Err(e) => { }
            }
        }
        Result::Err(e) => { }
    }
    ok
}

fn spill(f: &File, w: &ByteWriter) -> bool {
    if w.len() == 0u64 { return true }
    val sp = w.span()
    var ok = false
    match sp {
        Option::Some(b) => { ok = segfile::put_bytes(f, b, w.len()) }
        Option::None => { }
    }
    ok
}

fn write_whole(path: str, w: &ByteWriter) -> bool {
    val created = File::create(path)
    var ok = false
    match created {
        Result::Ok(f) => {
            if spill(&f, w) {
                val s = f.sync()
                match s {
                    Result::Ok(u) => { ok = true }
                    Result::Err(e) => { }
                }
            }
        }
        Result::Err(e) => { }
    }
    ok
}

fn append_whole(path: str, w: &ByteWriter) -> bool {
    val opened = File::append(path)
    var ok = false
    match opened {
        Result::Ok(f) => { ok = spill(&f, w) }
        Result::Err(e) => { }
    }
    ok
}

fn ensure_dirs(mount: str) -> bool {
    val meta = meta_path(mount)
    val tmp = tmp_path(mount)
    var ok = true
    val a = fs::mkdir_all(meta.to_str())
    match a {
        Result::Ok(u) => { }
        Result::Err(e) => { ok = false }
    }
    val b = fs::mkdir_all(tmp.to_str())
    match b {
        Result::Ok(u) => { }
        Result::Err(e) => { ok = false }
    }
    ok
}

# ---------------------------------------------------------------------

# The live rows of one mount, plus the generation they came from.
pub struct Catalog {
    rows: Vec<CatRow>,
    gen: u64,
    # How many journal records have been applied on top of the
    # snapshot. Compaction is worth doing once this is large.
    applied: u64,
}

impl Catalog {
    pub fn new() -> Self {
        var rows: Vec<CatRow> = Vec::new()
        val c = Catalog { rows: rows, gen: 0u64, applied: 0u64 }
        c
    }

    pub fn size(&self) -> u64 { self.rows.size() }
    pub fn generation(&self) -> u64 { self.gen }
    pub fn applied(&self) -> u64 { self.applied }
    pub fn is_empty(&self) -> bool { self.rows.size() == 0u64 }

    # Take over an existing generation number without reading it.
    #
    # `rebuild` starts from nothing and so calls itself generation 0,
    # which would publish generation 1 next to an already-published
    # generation 7 and be ignored (the reader takes the highest). A
    # repaired catalog has to be published *after* the one it
    # replaces, and that is what this says.
    pub fn adopt_generation(&mut self, gen: u64) { self.gen = gen }

    pub fn row(&self, i: u64) -> CatRow {
        val r: CatRow = self.rows.get(i)
        r
    }

    pub fn find(&self, segid: u64) -> Option<u64> {
        var i: u64 = 0u64
        while i < self.rows.size() {
            val r: CatRow = self.rows.get(i)
            if r.segid == segid { return Option::Some(i) }
            i = i + 1u64
        }
        Option::None
    }

    # Add, or replace the row with the same `segid`. Replacing rather
    # than appending is what makes replaying a journal idempotent: a
    # crash between the rename and the journal append leaves the same
    # segment described twice, and the second description wins.
    pub fn add(&mut self, r: &CatRow) {
        val at = self.find(r.segid)
        match at {
            Option::Some(i) => { self.rows.set(i, r) }
            Option::None => { self.rows.push(r) }
        }
    }

    pub fn remove(&mut self, segid: u64) -> bool {
        val at = self.find(segid)
        match at {
            Option::Some(i) => {
                var j = i
                val n = self.rows.size()
                while j + 1u64 < n {
                    val nxt: CatRow = self.rows.get(j + 1u64)
                    self.rows.set(j, nxt)
                    j = j + 1u64
                }
                val gone: CatRow = self.rows.pop()
                true
            }
            Option::None => { false }
        }
    }

    pub fn total_bytes(&self) -> u64 {
        var t: u64 = 0u64
        var i: u64 = 0u64
        while i < self.rows.size() {
            val r: CatRow = self.rows.get(i)
            t = t + r.seg_bytes
            i = i + 1u64
        }
        t
    }

    pub fn total_records(&self) -> u64 {
        var t: u64 = 0u64
        var i: u64 = 0u64
        while i < self.rows.size() {
            val r: CatRow = self.rows.get(i)
            t = t + r.records
            i = i + 1u64
        }
        t
    }

    # Step 1 of every query: the rows whose span meets `[from, to)`.
    #
    # Indices, not rows: the caller usually wants the path next, and
    # building one `String` per candidate is the point at which this
    # stops being free.
    pub fn select(&self, from: i64, until: i64, out: &mut Vec<u64>) {
        out.clear()
        var i: u64 = 0u64
        while i < self.rows.size() {
            val r: CatRow = self.rows.get(i)
            if r.overlaps(from, until) { out.push(i) }
            i = i + 1u64
        }
    }

    # Rows that are entirely older than `cutoff`. Retention drops
    # whole segments, so a segment with one record newer than the
    # cutoff stays (DATA_MODEL.md §4: there is no row-level delete).
    pub fn expired(&self, cutoff: i64, out: &mut Vec<u64>) {
        out.clear()
        var i: u64 = 0u64
        while i < self.rows.size() {
            val r: CatRow = self.rows.get(i)
            if r.ts_max < cutoff { out.push(i) }
            i = i + 1u64
        }
    }
}

# ---------------------------------------------------------------------
# Snapshots

# Serialize the whole catalog into `w` -- header, then every row.
pub fn encode_snapshot(c: &Catalog, gen: u64, w: &mut ByteWriter, crc: &Crc32) {
    w.clear()
    w.put_magic("LSC2")
    w.put_u32(gen)
    w.put_u64(c.size())
    w.put_u32(0u64)
    w.put_u32(0u64)
    w.put_u64(0u64)
    var i: u64 = 0u64
    while i < c.size() {
        val r: CatRow = c.row(i)
        put_row(w, &r, crc)
        i = i + 1u64
    }
    # The whole-file CRC goes back into the header now that the rows
    # exist. It covers the rows only: the header cannot checksum
    # itself.
    var sum: u64 = 0u64
    val sp = w.span()
    match sp {
        Option::Some(b) => {
            val n = w.len() - snap_head_bytes()
            if n > 0u64 { sum = crc.of(b, snap_head_bytes(), n) }
        }
        Option::None => { }
    }
    w.patch_u32(16u64, sum)
}

# The reverse. Returns the generation the snapshot names, or 0 if it
# is not a snapshot this reader understands.
pub fn decode_snapshot(b: Span<u8>, len: u64, c: &mut Catalog, crc: &Crc32) -> u64 {
    if len < snap_head_bytes() { return 0u64 }
    var rd = ByteReader::new(len)
    if !rd.take_magic(b, "LSC2") { return 0u64 }
    val gen = rd.take_u32(b)
    val rows = rd.take_u64(b)
    val want = rd.take_u32(b)
    val reserved = rd.take_u32(b)
    val reserved2 = rd.take_u64(b)

    val body = len - snap_head_bytes()
    if body != rows * row_bytes() { return 0u64 }
    var got: u64 = 0u64
    if body > 0u64 { got = crc.of(b, snap_head_bytes(), body) }
    if got != want { return 0u64 }

    var i: u64 = 0u64
    while i < rows {
        val r = take_row(&mut rd, b, crc)
        if r.ok { c.add(&r) }
        i = i + 1u64
    }
    gen
}

# The highest generation that has a snapshot under `mount`, or 0 when
# there is none. Generations start at 1 so that 0 can mean "no
# catalog yet" without a second flag.
pub fn latest_gen(mount: str) -> u64 {
    val meta = meta_path(mount)
    var best: u64 = 0u64
    val listing = fs::list_dir(meta.to_str())
    match listing {
        Result::Ok(names) => {
            var i: u64 = 0u64
            while i < names.size() {
                val nm: &String = names.borrow(i)
                val g = gen_of_snap(&nm)
                match g {
                    Option::Some(v) => { if v > best { best = v } }
                    Option::None => { }
                }
                i = i + 1u64
            }
        }
        Result::Err(e) => { }
    }
    best
}

# `catalog.000007.snap` -> 7. Anything else is not ours.
pub fn gen_of_snap(name: &String) -> Option<u64> {
    val head = String::from_str("catalog.")
    if !name.starts_with(&head) { return Option::None }
    val tail = String::from_str(".snap")
    if !name.ends_with(&tail) { return Option::None }
    val n = name.len()
    if n <= 13u64 { return Option::None }
    val mid = name.substring(8u64, n - 5u64)
    val got = parse::to_u64(mid.to_str())
    match got {
        Result::Ok(v) => { Option::Some(v) }
        Result::Err(e) => { Option::None }
    }
}

# ---------------------------------------------------------------------
# The journal

fn put_jrec(w: &mut ByteWriter, op: u64, crc: &Crc32, body_len: u64) {
    # Called after the body has been written at `jrec_head_bytes()`;
    # see `append_add`. Kept separate so the two record shapes share
    # one framing.
    w.patch_u32(4u64, jrec_head_bytes() + body_len)
    w.patch_u32(8u64, op)
    var sum: u64 = 0u64
    val sp = w.span()
    match sp {
        Option::Some(b) => { sum = crc.of(b, jrec_head_bytes(), body_len) }
        Option::None => { }
    }
    w.patch_u32(12u64, sum)
}

fn frame_start(w: &mut ByteWriter) {
    w.clear()
    w.put_magic("LSJ1")
    w.put_u32(0u64)
    w.put_u32(0u64)
    w.put_u32(0u64)
}

# Append `ADD <row>` to generation `gen`'s journal.
pub fn append_add(mount: str, gen: u64, r: &CatRow, crc: &Crc32) -> bool {
    if !ensure_dirs(mount) { return false }
    var w = ByteWriter::with_capacity(128u64)
    frame_start(&mut w)
    put_row(&mut w, r, crc)
    put_jrec(&mut w, op_add(), crc, row_bytes())
    val p = log_path(mount, gen)
    append_whole(p.to_str(), &w)
}

# Append `REMOVE <segid> <why>`.
pub fn append_remove(mount: str, gen: u64, segid: u64, why: u64,
                     crc: &Crc32) -> bool {
    if !ensure_dirs(mount) { return false }
    var w = ByteWriter::with_capacity(64u64)
    frame_start(&mut w)
    w.put_u64(segid)
    w.put_u32(why)
    w.put_u32(0u64)
    put_jrec(&mut w, op_remove(), crc, 16u64)
    val p = log_path(mount, gen)
    append_whole(p.to_str(), &w)
}

# Replay a journal onto `c`, and say how many records were applied.
#
# **Stops at the first record that does not check out.** A journal is
# appended to without `fsync`, so a torn tail is the expected way for
# this to end after a crash; everything before it is still good
# (STORAGE_FORMAT.md §6).
pub fn replay(b: Span<u8>, len: u64, c: &mut Catalog, crc: &Crc32) -> u64 {
    var rd = ByteReader::new(len)
    var applied: u64 = 0u64
    var going = true
    while going {
        val at = rd.position()
        if at + jrec_head_bytes() > len { going = false }
        if going {
            if !rd.take_magic(b, "LSJ1") {
                going = false
            } else {
                val total = rd.take_u32(b)
                val op = rd.take_u32(b)
                val want = rd.take_u32(b)
                if total < jrec_head_bytes() || at + total > len {
                    going = false
                } else {
                    val body = total - jrec_head_bytes()
                    val got = crc.of(b, at + jrec_head_bytes(), body)
                    if got != want {
                        going = false
                    } else {
                        if op == op_add() && body == row_bytes() {
                            val r = take_row(&mut rd, b, crc)
                            if r.ok { c.add(&r) }
                            applied = applied + 1u64
                        } elif op == op_remove() && body == 16u64 {
                            val segid = rd.take_u64(b)
                            val why = rd.take_u32(b)
                            val pad = rd.take_u32(b)
                            val gone = c.remove(segid)
                            applied = applied + 1u64
                        } else {
                            # A record kind this build does not know.
                            # Skipping it keeps a newer writer from
                            # making the catalog unreadable.
                            rd.seek(at + total)
                        }
                        if rd.position() != at + total { rd.seek(at + total) }
                    }
                }
            }
        }
    }
    applied
}

# ---------------------------------------------------------------------
# Opening and replacing a catalog

# Read the newest generation of `mount`: its snapshot, then its
# journal. An absent or unreadable catalog comes back empty with
# generation 0, which is the caller's cue to rebuild.
pub fn load(mount: str, crc: &Crc32) -> Catalog {
    var c = Catalog::new()
    val gen = latest_gen(mount)
    if gen == 0u64 { return c }

    var buf = ByteWriter::with_capacity(65536u64)
    val sp = snap_path(mount, gen)
    if read_whole(sp.to_str(), &mut buf) {
        val b = buf.span()
        match b {
            Option::Some(w) => {
                val got = decode_snapshot(w, buf.len(), &mut c, crc)
                if got == gen { c.gen = gen }
            }
            Option::None => { }
        }
    }
    if c.gen != gen {
        # The snapshot did not check out. Everything it held is still
        # on disk under `seg/`, so say so rather than pretending.
        var fresh = Catalog::new()
        return fresh
    }

    val lp = log_path(mount, gen)
    if read_whole(lp.to_str(), &mut buf) {
        val b2 = buf.span()
        match b2 {
            Option::Some(w2) => {
                c.applied = replay(w2, buf.len(), &mut c, crc)
            }
            Option::None => { }
        }
    }
    c
}

# Publish `c` as generation `gen` and drop the generation before it.
#
# tmp -> sync -> rename is the same dance the segment writer does,
# and for the same reason: a reader must never see half a file.
pub fn write_generation(mount: str, c: &Catalog, gen: u64,
                        crc: &Crc32) -> bool {
    if !ensure_dirs(mount) { return false }
    var w = ByteWriter::with_capacity(snap_head_bytes() + c.size() * row_bytes() + 64u64)
    encode_snapshot(c, gen, &mut w, crc)

    val part = "{mount}/tmp/catalog.{gen:06}.snap.part"
    if !write_whole(part, &w) { return false }
    val dst = snap_path(mount, gen)
    val moved = fs::rename(part, dst.to_str())
    var ok = false
    match moved {
        Result::Ok(u) => { ok = true }
        Result::Err(e) => { }
    }
    ok
}

# Fold the journal into a new generation. Safe to call at any time:
# if anything fails, the generation that is already published stays.
pub fn compact(mount: str, c: &mut Catalog, crc: &Crc32) -> bool {
    val old = c.gen
    val next = old + 1u64
    if !write_generation(mount, c, next, crc) { return false }
    c.gen = next
    c.applied = 0u64
    if old > 0u64 {
        val os = snap_path(mount, old)
        val od = fs::remove_file(os.to_str())
        match od {
            Result::Ok(u) => { }
            Result::Err(e) => { }
        }
        val ol = log_path(mount, old)
        val ld = fs::remove_file(ol.to_str())
        match ld {
            Result::Ok(u) => { }
            Result::Err(e) => { }
        }
    }
    true
}

# ---------------------------------------------------------------------

# Build a catalog from what is actually on disk (`--repair`).
#
# 320 bytes per segment, which is why this is a thing anyone can run
# rather than a last resort: 100,000 segments is 32 MB of reads, not
# the whole archive.
pub fn rebuild(mount: str, crc: &Crc32) -> Catalog {
    var c = Catalog::new()
    val root = "{mount}/seg"
    val segs = logdir::scan_suffix(root, ".seg")
    var scratch = ByteWriter::with_capacity(segfile::data_at())
    var i: u64 = 0u64
    while i < segs.size() {
        val p: &String = segs.borrow(i)
        val ps = p.to_str()
        val arc = String::from_str(".arc.seg")
        val is_arc = p.ends_with(&arc)
        val opened = File::open(ps)
        match opened {
            Result::Ok(f) => {
                val h = segfile::head_of(&f, &mut scratch)
                if h.ok {
                    var size: u64 = 0u64
                    val sz = f.size()
                    match sz {
                        Result::Ok(n) => { size = n }
                        Result::Err(e) => { }
                    }
                    var r = row_of_head(&h, size, is_arc)
                    val key = daykey_of_path(&p)
                    if key > 0u64 { r.daykey = key }
                    c.add(&r)
                }
            }
            Result::Err(e) => { }
        }
        i = i + 1u64
    }
    c
}
