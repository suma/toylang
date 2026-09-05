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

pub fn head_bytes() -> u64 { 64u64 }
pub fn dir_at() -> u64 { 64u64 }
pub fn dir_slots() -> u64 { 8u64 }
pub fn data_at() -> u64 { 320u64 }
pub fn seg_version() -> u64 { 3u64 }

pub fn kind_frames() -> u64 { 1u64 }
pub fn kind_records() -> u64 { 2u64 }
pub fn kind_ftable() -> u64 { 3u64 }
pub fn kind_terms() -> u64 { 6u64 }
pub fn kind_links() -> u64 { 7u64 }

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
        }
    }

    pub fn has_terms(&self) -> bool { self.terms_len > 0u64 }
    pub fn has_links(&self) -> bool { self.links_len > 0u64 }
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
    if !read_range(f, 0u64, data_at(), &mut scratch) { return h }
    val w = scratch.span()
    match w {
        Option::Some(b) => {
            var rd = ByteReader::new(data_at())
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

                rd.seek(dir_at())
                val count = rd.take_u32(b)
                val reserved = rd.take_u32(b)
                var i: u64 = 0u64
                while i < count && i < dir_slots() {
                    val k = rd.take_u32(b)
                    val off = rd.take_u64(b)
                    val len = rd.take_u64(b)
                    val sum = rd.take_u32(b)
                    if k == kind_frames() { h.frames_off = off  h.frames_len = len }
                    if k == kind_records() {
                        h.recs_off = off
                        h.recs_len = len
                        h.recs_crc = sum
                    }
                    if k == kind_ftable() { h.ftab_off = off  h.ftab_len = len }
                    if k == kind_terms() { h.terms_off = off  h.terms_len = len }
                    if k == kind_links() { h.links_off = off  h.links_len = len }
                    i = i + 1u64
                }
                if version == seg_version() { h.ok = true }
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
# `&mut` on to `lsz::decode` is what v2 could not do -- the writes
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
                                    if !lsz::decode(body, 0u64, clen, rawlen, &mut out) { ok = false }
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
        if !lsz::decode(src, body, clen, raw_len, &mut out) { ok = false }
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
