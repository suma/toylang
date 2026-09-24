# Compaction: many cold segments, one archive.
#
# A segment is written when the ingest buffer fills, so a busy day
# leaves hundreds of small ones. They cost three things
# (DATA_MODEL.md §5): compression (a 4 MiB window cannot see a
# repeated line that landed in the next file), pruning (the catalog
# has a row per file and a query walks them), and open calls.
#
# Merging is the answer, and the format makes it cheap: **an archive
# is a segment**. Same header, same sections, same reader. The only
# differences are the name (`<segid>.arc.seg`) and the catalog row's
# kind, which exists so an operator can tell a merged file from a
# written one — nothing in the read path branches on it.
#
# The records are re-parsed rather than copied section by section.
# Copying would mean merging two term dictionaries, two link
# sections, two stream tables and the frame layout, each with its own
# rules; re-adding them puts one code path (`ArchiveWriter::add`,
# which ingest already uses) in charge of all of it. The cost is
# parsing lines again, once, for files that are by definition cold.
#
# **The inputs go away only after the output is on disk and in the
# catalog** (STORAGE_FORMAT.md §8). A crash in the middle leaves both
# copies and a catalog that lists both; the next `repair` sees two
# files that each hold the same records, which is wasteful and not
# wrong. A crash the other way round would lose data.

# What one pass did. `merged` is how many inputs went in — 0 means
# there was nothing cold enough to be worth it, which is the normal
# answer and not a failure.
pub struct CompactReport {
    ok: bool,
    merged: u64,
    records: u64,
    bytes_in: u64,
    bytes_out: u64,
    segid: u64,
}

impl CompactReport {
    pub fn nothing() -> Self {
        CompactReport {
            ok: true, merged: 0u64, records: 0u64,
            bytes_in: 0u64, bytes_out: 0u64, segid: 0u64,
        }
    }
    pub fn is_ok(&self) -> bool { self.ok }
    pub fn merged(&self) -> u64 { self.merged }
    pub fn records(&self) -> u64 { self.records }
    pub fn bytes_in(&self) -> u64 { self.bytes_in }
    pub fn bytes_out(&self) -> u64 { self.bytes_out }
    pub fn segid(&self) -> u64 { self.segid }
}

# How old a segment has to be before it is worth merging. A file
# still being queried by "the last hour" dashboards is a file whose
# pages are warm; merging it spends I/O to make the common case
# slower for a while.
pub const COLD_SECS: i64 = 86400i64

# How many inputs one pass takes. A pass is meant to be interruptible
# — `/v1/admin/compact` advances it by one, a cron loop calls it until
# it answers 0 — so the bound is on the work, not on the result size.
pub const MAX_INPUTS: u64 = 32u64

# Merge one run of cold segments on `mount`. Answers what it did.
pub fn compact_once(mount: str, now: i64, crc: &Crc32) -> CompactReport {
    var out = CompactReport::nothing()
    val c = catalog::load(mount, crc)
    if c.size() < 2u64 { return out }
    val cutoff = now - COLD_SECS

    # The oldest first, so a run of merges walks the archive forward
    # in time rather than leaving holes.
    var picked: Vec<u64> = Vec::new()
    var best_first = true
    while best_first && picked.size() < MAX_INPUTS {
        var at: i64 = -1i64
        var oldest: i64 = 0i64
        var i: u64 = 0u64
        while i < c.size() {
            val r: CatRow = c.row(i)
            if r.kind == catalog::kind_segment() && r.ts_max < cutoff && !taken(&picked, i) {
                if at < 0i64 || r.ts_min < oldest {
                    at = i as i64
                    oldest = r.ts_min
                }
            }
            i = i + 1u64
        }
        if at < 0i64 { best_first = false } else { picked.push(at as u64) }
    }
    # One segment is not a merge. Two is the smallest thing that can
    # pay for itself.
    if picked.size() < 2u64 { return out }

    var w = ArchiveWriter::new()
    var head_buf = ByteWriter::with_capacity(512u64)
    var raw = ByteWriter::with_capacity(1048576u64)
    var arena_buf = ByteWriter::with_capacity(4194304u64)
    var recs = ByteWriter::with_capacity(1048576u64)
    var rec = ParsedLine::new()
    var total_in: u64 = 0u64
    var records: u64 = 0u64
    var read_ok = true

    var p: u64 = 0u64
    while p < picked.size() && read_ok {
        val row: CatRow = c.row(picked.get(p))
        val path = catalog::seg_path(mount, &row)
        total_in = total_in + row.seg_bytes
        val opened = File::open(path.to_str())
        match opened {
            Result::Ok(f) => {
                val h = segfile::head_of(&f, &mut head_buf)
                if !h.ok { read_ok = false }
                if read_ok {
                    arena_buf.clear()
                    if !segfile::expand_all(&f, &h, crc, &mut raw, &mut arena_buf) {
                        read_ok = false
                    }
                }
                if read_ok {
                    if !segfile::read_range(&f, h.recs_off, h.recs_len, &mut recs) {
                        read_ok = false
                    }
                }
                if read_ok {
                    records = records + take_records(&recs, &arena_buf, h.records,
                                                     &mut rec, &mut w)
                }
            }
            Result::Err(e) => { read_ok = false }
        }
        p = p + 1u64
    }
    if !read_ok {
        # Nothing has been published and nothing removed, so a bad
        # input costs this pass and no data.
        out.ok = false
        return out
    }

    val segid = next_id(&c)
    val stamp = archive_stamp(&w)
    val dir = store::day_dir(mount, stamp)
    val dir_str = dir.to_str()
    val made = fs::mkdir_all(dir_str)
    match made {
        Result::Ok(u) => { }
        Result::Err(e) => { out.ok = false  return out }
    }
    val base = "{dir_str}/{segid:012}.arc"
    val wrote = w.finish(base, segid, crc)
    var produced: u64 = 0u64
    match wrote {
        Result::Ok(k) => { produced = k }
        Result::Err(e) => { out.ok = false  return out }
    }

    # In the catalog before the inputs leave it.
    val gen = catalog::latest_gen(mount)
    if !record_archive(mount, base, gen, crc) {
        out.ok = false
        return out
    }
    var q: u64 = 0u64
    while q < picked.size() {
        val row: CatRow = c.row(picked.get(q))
        val path = catalog::seg_path(mount, &row)
        val why = catalog::why_merged()
        if catalog::append_remove(mount, gen, row.segid, why, crc) {
            # The label dictionary counts records per segment; the
            # records did not change, so what leaves the inputs is
            # exactly what the archive brought in.
            val forgot = labels::forget_segment(mount, &path, gen, crc)
            val rm = fs::remove_file(path.to_str())
            match rm {
                Result::Ok(u) => { }
                Result::Err(e) => { }
            }
        }
        q = q + 1u64
    }

    out.ok = true
    out.merged = picked.size()
    out.records = records
    out.bytes_in = total_in
    out.bytes_out = produced
    out.segid = segid
    out
}

fn taken(picked: &Vec<u64>, i: u64) -> bool {
    var k: u64 = 0u64
    var hit = false
    while k < picked.size() && !hit {
        if picked.get(k) == i { hit = true }
        k = k + 1u64
    }
    hit
}

fn next_id(c: &Catalog) -> u64 {
    var top: u64 = 0u64
    var i: u64 = 0u64
    while i < c.size() {
        val r: CatRow = c.row(i)
        if r.segid > top { top = r.segid }
        i = i + 1u64
    }
    top + 1u64
}

# The day an archive is filed under: the oldest record it holds, or
# now when it holds nothing dated (the rule `flush_segment` uses).
fn archive_stamp(w: &ArchiveWriter) -> i64 {
    var stamp = w.ts_min()
    if stamp == 0i64 { stamp = time::now_unix_secs() }
    stamp
}

# Re-add every record of one expanded segment. Answers how many.
#
# The record table is the same varint run the reader walks; only the
# line's extent is needed here, because `parse_line` derives the rest
# and `ArchiveWriter::add` indexes from what it derives.
fn take_records(recs: &ByteWriter, arena_buf: &ByteWriter, count: u64,
                rec: &mut ParsedLine, w: &mut ArchiveWriter) -> u64 {
    var added: u64 = 0u64
    val idx_w = recs.span()
    val aw = arena_buf.span()
    match idx_w {
        Option::Some(idx) => {
            match aw {
                Option::Some(arena) => {
                    var rd = ByteReader::new(recs.len())
                    var line_at: u64 = 0u64
                    var r: u64 = 0u64
                    while r < count && rd.remaining() > 0u64 {
                        val flags = rd.take_varint(idx)
                        val line_len = rd.take_varint(idx)
                        val ts = rd.take_varint(idx)
                        val host_rel = rd.take_varint(idx)
                        val host_len = rd.take_varint(idx)
                        val tag_rel = rd.take_varint(idx)
                        val tag_len = rd.take_varint(idx)
                        val labels_rel = rd.take_varint(idx)
                        val labels_len = rd.take_varint(idx)
                        val body_rel = rd.take_varint(idx)
                        val body_len = rd.take_varint(idx)
                        if line_len > 0u64 {
                            val ln = Line { start: line_at, len: line_len }
                            record::parse_line(arena, ln, rec)
                            w.add(arena, ln, rec)
                            added = added + 1u64
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
    added
}

# The catalog row for a freshly written archive, read back out of the
# file rather than built from what the writer believed — the same
# rule `store::record_segment` follows.
fn record_archive(mount: str, base: str, gen: u64, crc: &Crc32) -> bool {
    val path = "{base}.seg"
    val opened = File::open(path)
    var ok = false
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(512u64)
            val h = segfile::head_of(&f, &mut scratch)
            if h.ok {
                var size: u64 = 0u64
                val sz = f.size()
                match sz {
                    Result::Ok(k) => { size = k }
                    Result::Err(e) => { }
                }
                var r = catalog::row_of_head(&h, size, true)
                val name = String::from_str(path)
                val key = catalog::daykey_of_path(&name)
                if key > 0u64 { r.daykey = key }
                ok = catalog::append_add(mount, gen, &r, crc)
                if ok {
                    val noted = labels::note_segment(mount, &name, gen, crc)
                }
            }
        }
        Result::Err(e) => { }
    }
    ok
}
