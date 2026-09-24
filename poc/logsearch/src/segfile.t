# The segment file: one file, read a piece at a time
# (STORAGE_FORMAT.md v3).
#
# v2 wrote two files. It had to: the only way to read anything was to
# read a whole file, so putting the index next to the data meant that
# pruning a segment cost as much as scanning it. The index went into a
# `.idx` of its own, and segments were capped at 8 MiB so that reading
# one whole was still defensible.
#
# `core/std/fs.t` grew `File` (2026-09-05), and with `read_at` both
# reasons are gone. A segment is one `.seg` file again, and a reader
# takes what it needs out of it:
#
#   - 320 bytes to decide whether the segment is worth opening
#   - one section to answer a field query
#   - one frame at a time to scan bodies, so the memory a scan needs
#     no longer follows the size of the segment
#
# Layout:
#
#   0    header, 64 bytes -- the numbers a catalogue row is made of
#   64   section directory: count, then 8 slots of (kind, off, len, crc)
#   320  the sections themselves, frames first
#
# **Unknown section kinds are skipped**, as in v2: adding a section
# does not stop an older reader.

import lsz

pub const HEAD_BYTES: u64 = 64u64
pub const DIR_AT: u64 = 64u64
pub const DIR_SLOTS: u64 = 8u64
pub const DATA_AT: u64 = 320u64
pub const SEG_VERSION: u64 = 3u64

# What a directory entry points at. The number is what the directory
# stores, so it is part of the segment format (STORAGE_FORMAT.md): add
# a section with a new number, never renumber one. 4 and 5 are unused.
pub enum Section {
    Frames = 1,
    Records = 2,
    FieldTable = 3,
    Terms = 6,
    Links = 7,
    # ONTOLOGY O1: per-term first_seen / last_seen. A reader that does
    # not know this kind skips it, so segments written before it load.
    Objects = 8,
    # The stream table (DATA_MODEL.md section 3): one row per label set.
    Streams = 9,
}

# The section a directory entry's number names, or `None` for one this
# reader does not know -- which it skips, so a newer writer's segment
# still loads. The one place the numbers are read back.
pub fn section_of(kind: u64) -> Option<Section> {
    match kind {
        1u64 => Option::Some(Section::Frames),
        2u64 => Option::Some(Section::Records),
        3u64 => Option::Some(Section::FieldTable),
        6u64 => Option::Some(Section::Terms),
        7u64 => Option::Some(Section::Links),
        8u64 => Option::Some(Section::Objects),
        9u64 => Option::Some(Section::Streams),
        _ => Option::None,
    }
}

# Everything the header and the directory say, in one value.
#
# It is eighteen fields wide, which the AOT backend could not return
# until WIDE-RETURN (2026-09-04); before that this would have been an
# out-parameter (RUNTIME_GAPS.md §Z).
pub struct SegHead {
    ok: bool,
    segid: u64,
    ts_min: i64,
    ts_max: i64,
    records: u64,
    n_frames: u64,
    frame_raw: u64,
    arena_bytes: u64,
    frames_off: u64,
    frames_len: u64,
    recs_off: u64,
    recs_len: u64,
    recs_crc: u64,
    ftab_off: u64,
    ftab_len: u64,
    terms_off: u64,
    terms_len: u64,
    links_off: u64,
    links_len: u64,
    objs_off: u64,
    objs_len: u64,
    strs_off: u64,
    strs_len: u64,
}

impl SegHead {
    pub fn empty() -> Self {
        SegHead {
            ok: false, segid: 0u64, ts_min: 0i64, ts_max: 0i64,
            records: 0u64, n_frames: 0u64, frame_raw: 0u64,
            arena_bytes: 0u64,
            frames_off: 0u64, frames_len: 0u64,
            recs_off: 0u64, recs_len: 0u64, recs_crc: 0u64,
            ftab_off: 0u64, ftab_len: 0u64,
            terms_off: 0u64, terms_len: 0u64,
            links_off: 0u64, links_len: 0u64,
            objs_off: 0u64, objs_len: 0u64,
            strs_off: 0u64, strs_len: 0u64,
        }
    }

    pub fn has_terms(&self) -> bool { self.terms_len > 0u64 }
    pub fn has_links(&self) -> bool { self.links_len > 0u64 }
    pub fn has_streams(&self) -> bool { self.strs_len > 0u64 }
    pub fn has_objects(&self) -> bool { self.objs_len > 0u64 }
    pub fn has_records(&self) -> bool { self.recs_len > 0u64 }
}

# Read `len` bytes from `off` into `out`, replacing what was there.
#
# A short read is a failure here, unlike in `File::read`: every caller
# knows exactly how many bytes the thing it is reading occupies, so
# getting fewer means the file is truncated or the directory lies.
pub fn read_range(f: &File, off: u64, len: u64, out: &mut ByteWriter) -> bool {
    out.clear()
    if len == 0u64 { return true }
    out.reserve(len)
    var ok = false
    val room = out.room()
    match room {
        Option::Some(all) => {
            val win = all.slice(0u64, len)
            val got = f.read_at(off, win)
            match got {
                Result::Ok(n) => {
                    if n == len {
                        out.set_len(len)
                        ok = true
                    }
                }
                Result::Err(e) => { }
            }
        }
        Option::None => { }
    }
    ok
}

# The header and directory of an open segment, using `scratch` as the
# landing buffer (320 bytes; the caller owns it so that a scan over
# many segments allocates once).
pub fn head_of(f: &File, scratch: &mut ByteWriter) -> SegHead {
    var h = SegHead::empty()
    if !read_range(f, 0u64, DATA_AT, &mut scratch) { return h }
    val w = scratch.span()
    match w {
        Option::Some(b) => {
            var rd = ByteReader::new(DATA_AT)
            if rd.take_magic(b, "LSD3") {
                val version = rd.take_u32(b)
                h.segid = rd.take_u64(b)
                h.ts_min = rd.take_u64(b) as i64
                h.ts_max = rd.take_u64(b) as i64
                h.records = rd.take_u64(b)
                h.n_frames = rd.take_u32(b)
                h.frame_raw = rd.take_u32(b)
                h.arena_bytes = rd.take_u64(b)
                val kind = rd.take_u32(b)
                val hcrc = rd.take_u32(b)

                rd.seek(DIR_AT)
                val count = rd.take_u32(b)
                val reserved = rd.take_u32(b)
                var i: u64 = 0u64
                while i < count && i < DIR_SLOTS {
                    val k = rd.take_u32(b)
                    val off = rd.take_u64(b)
                    val len = rd.take_u64(b)
                    val sum = rd.take_u32(b)
                    val section = section_of(k)
                    match section {
                        Option::Some(Section::Frames) => { h.frames_off = off  h.frames_len = len }
                        Option::Some(Section::Records) => {
                            h.recs_off = off
                            h.recs_len = len
                            h.recs_crc = sum
                        }
                        Option::Some(Section::FieldTable) => { h.ftab_off = off  h.ftab_len = len }
                        Option::Some(Section::Terms) => { h.terms_off = off  h.terms_len = len }
                        Option::Some(Section::Links) => { h.links_off = off  h.links_len = len }
                        Option::Some(Section::Objects) => { h.objs_off = off  h.objs_len = len }
                        Option::Some(Section::Streams) => { h.strs_off = off  h.strs_len = len }
                        Option::None => { }
                    }
                    i = i + 1u64
                }
                if version == SEG_VERSION { h.ok = true }
            }
        }
        Option::None => { }
    }
    h
}

# Expand every frame into `out`, checking each frame's CRC on the way.
#
# One frame is in memory at a time: the compressed bytes land in
# `raw`, and the expansion is appended straight to `out`. Handing a
# `&mut` on to `lsz::decode_frame` is what v2 could not do -- the writes
# were dropped silently (METHOD-ARG-UNCHECKED, fixed 2026-09-05), and
# the copy through a scratch buffer that worked around it is gone.
pub fn expand_all(f: &File, h: &SegHead, crc: &Crc32,
                  raw: &mut ByteWriter, out: &mut ByteWriter) -> bool {
    var ok = true
    var at = h.frames_off
    val end = h.frames_off + h.frames_len
    var i: u64 = 0u64
    while i < h.n_frames && ok {
        if at + 24u64 > end {
            ok = false
        } else {
            if !read_range(f, at, 24u64, &mut raw) {
                ok = false
            } else {
                var codec: u64 = 0u64
                var rawlen: u64 = 0u64
                var clen: u64 = 0u64
                var want: u64 = 0u64
                val hw = raw.span()
                match hw {
                    Option::Some(hb) => {
                        var rd = ByteReader::new(24u64)
                        if !rd.take_magic(hb, "LSF1") { ok = false }
                        codec = rd.take_u32(hb)
                        rawlen = rd.take_u32(hb)
                        clen = rd.take_u32(hb)
                        want = rd.take_u32(hb)
                    }
                    Option::None => { ok = false }
                }
                if ok && at + 24u64 + clen > end { ok = false }
                if ok {
                    val before = out.len()
                    if !read_range(f, at + 24u64, clen, &mut raw) {
                        ok = false
                    } else {
                        val bw = raw.span()
                        match bw {
                            Option::Some(body) => {
                                if codec == 0u64 {
                                    out.reserve(clen)
                                    out.put_span_fast(body, 0u64, clen)
                                } else {
                                    if !lsz::decode_frame(body, 0u64, clen, rawlen, &mut out) { ok = false }
                                }
                            }
                            Option::None => { ok = false }
                        }
                    }
                    if ok {
                        val ow = out.span()
                        match ow {
                            Option::Some(got) => {
                                if crc.of(got, before, rawlen) != want { ok = false }
                            }
                            Option::None => { ok = false }
                        }
                    }
                }
                at = at + 24u64 + clen
            }
        }
        i = i + 1u64
    }
    if out.len() != h.arena_bytes { ok = false }
    ok
}

# Expand only the frames `need` marks, leaving the rest of `out` at
# the right length but unwritten.
#
# A query that the index has already narrowed usually wants a handful
# of lines out of a segment, and expanding the whole arena to reach
# them is most of what such a query costs. Arena offsets have to stay
# what the record table says, so a skipped frame still advances `out`
# by its raw length -- the bytes there are whatever was in the buffer,
# and are never read, because a record that would read them is a
# record whose frame was needed.
#
# `need` is one byte per frame; anything non-zero expands. A `need`
# shorter than the frame count expands the rest, so a caller that
# cannot work out what it wants degrades to `expand_all`.
pub fn expand_selected(f: &File, h: &SegHead, crc: &Crc32,
                       raw: &mut ByteWriter, out: &mut ByteWriter,
                       need: &Vec<u8>) -> bool {
    var ok = true
    var at = h.frames_off
    val end = h.frames_off + h.frames_len
    var i: u64 = 0u64
    while i < h.n_frames && ok {
        if at + 24u64 > end {
            ok = false
        } else {
            if !read_range(f, at, 24u64, &mut raw) {
                ok = false
            } else {
                var codec: u64 = 0u64
                var rawlen: u64 = 0u64
                var clen: u64 = 0u64
                var want: u64 = 0u64
                val hw = raw.span()
                match hw {
                    Option::Some(hb) => {
                        var rd = ByteReader::new(24u64)
                        if !rd.take_magic(hb, "LSF1") { ok = false }
                        codec = rd.take_u32(hb)
                        rawlen = rd.take_u32(hb)
                        clen = rd.take_u32(hb)
                        want = rd.take_u32(hb)
                    }
                    Option::None => { ok = false }
                }
                if ok && at + 24u64 + clen > end { ok = false }
                var wanted = true
                if i < need.size() {
                    val flag: u8 = need.get(i)
                    wanted = flag != 0u8
                }
                if ok && !wanted {
                    # Skip the body entirely: no read, no decode, no
                    # CRC. The arena keeps its shape.
                    val n = out.len() + rawlen
                    out.reserve(rawlen)
                    out.set_len(n)
                }
                if ok && wanted {
                    val before = out.len()
                    if !read_range(f, at + 24u64, clen, &mut raw) {
                        ok = false
                    } else {
                        val bw = raw.span()
                        match bw {
                            Option::Some(body) => {
                                if codec == 0u64 {
                                    out.reserve(clen)
                                    out.put_span_fast(body, 0u64, clen)
                                } else {
                                    if !lsz::decode_frame(body, 0u64, clen, rawlen, &mut out) { ok = false }
                                }
                            }
                            Option::None => { ok = false }
                        }
                    }
                    if ok {
                        val ow = out.span()
                        match ow {
                            Option::Some(got) => {
                                if crc.of(got, before, rawlen) != want { ok = false }
                            }
                            Option::None => { ok = false }
                        }
                    }
                }
                at = at + 24u64 + clen
            }
        }
        i = i + 1u64
    }
    if out.len() != h.arena_bytes { ok = false }
    ok
}

# The frame each arena offset falls in, as `(arena_at, raw)` pairs read
# from the frame table (kind 3). 20 bytes an entry, so this is a few
# hundred bytes even for a full segment.
pub fn frame_extents(f: &File, h: &SegHead, scratch: &mut ByteWriter,
                     starts: &mut Vec<u64>, lens: &mut Vec<u64>) -> bool {
    starts.clear()
    lens.clear()
    if h.ftab_len == 0u64 { return false }
    if !read_range(f, h.ftab_off, h.ftab_len, &mut scratch) { return false }
    var ok = true
    val sw = scratch.span()
    match sw {
        Option::Some(sb) => {
            var rd = ByteReader::new(h.ftab_len)
            var i: u64 = 0u64
            while i < h.n_frames && ok {
                if rd.remaining() < 20u64 {
                    ok = false
                } else {
                    val skip = rd.take_u64(sb)
                    val arena_at = rd.take_u64(sb)
                    val raw = rd.take_u32(sb)
                    starts.push(arena_at)
                    lens.push(raw)
                }
                i = i + 1u64
            }
        }
        Option::None => { ok = false }
    }
    ok
}

# Write `len` bytes of `b` to the file at the cursor, retrying a short
# write.
#
# `File::write` reports a partial write as `Ok(n)` rather than an
# error -- that is `write(2)`, and a full disk arrives here as a small
# number, not an `Err`. Answering `false` on zero progress is what
# turns it back into a failure the caller can report.
pub fn put_bytes(f: &File, b: Span<u8>, len: u64) -> bool {
    var done: u64 = 0u64
    var ok = true
    while done < len && ok {
        val win = b.slice(done, len - done)
        val put = f.write(win)
        match put {
            Result::Ok(n) => {
                if n == 0u64 { ok = false } else { done = done + n }
            }
            Result::Err(e) => { ok = false }
        }
    }
    ok
}

# Unwrap a section written with the "LST1" header -- magic, codec, raw
# length, stored length, CRC of the raw bytes -- into `out`.
#
# The CRC is over what comes *out*, so a section that decompresses to
# the wrong bytes is caught here rather than parsed into terms that do
# not exist.
pub fn decode_block(src: Span<u8>, len: u64, crc: &Crc32, out: &mut ByteWriter) -> bool {
    out.clear()
    if len < 20u64 { return false }
    var rd = ByteReader::new(len)
    if !rd.take_magic(src, "LST1") { return false }
    val codec = rd.take_u32(src)
    val raw_len = rd.take_u32(src)
    val clen = rd.take_u32(src)
    val want = rd.take_u32(src)
    val body = rd.position()
    if body + clen > len { return false }

    var ok = true
    if codec == 0u64 {
        out.reserve(clen)
        out.put_span_fast(src, body, clen)
    } else {
        if !lsz::decode_frame(src, body, clen, raw_len, &mut out) { ok = false }
    }
    if ok {
        val ow = out.span()
        match ow {
            Option::Some(got) => {
                if crc.of(got, 0u64, out.len()) != want { ok = false }
            }
            Option::None => { ok = false }
        }
    }
    ok
}

# Read one "LST1" section off the disk and expand it: `raw` takes the
# stored bytes, `out` the expanded ones.
pub fn load_block(f: &File, off: u64, len: u64, crc: &Crc32,
                  raw: &mut ByteWriter, out: &mut ByteWriter) -> bool {
    if len == 0u64 { return false }
    if !read_range(f, off, len, &mut raw) { return false }
    var ok = false
    val w = raw.span()
    match w {
        Option::Some(b) => { ok = decode_block(b, len, crc, &mut out) }
        Option::None => { }
    }
    ok
}
