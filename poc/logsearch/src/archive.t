# Turning parsed lines into a segment on disk, and reading one back.
#
# The layout is the one STORAGE_FORMAT.md describes (v3), cut down to
# what the reader can produce today. One file:
#
#   <seg>.seg   64-byte header, a section directory, then the frames
#               (the line bytes, compressed 256 KiB at a time with
#               LSZ1), the record table (varints, a row per record), the frame
#               table, the term dictionary and the link table
#
# v2 wrote the data and the index as two files, because a reader
# could only take a whole file and pruning had to be possible without
# dragging the data through memory. `File::read_at` (2026-09-05)
# removed that constraint, and one file removes the window in which
# the data is published and its index is not. The file layer lives in
# `segfile.t`; what is here is the writer and the meaning of the
# sections.
#
# **Frames are independent.** The compressor's window is reset at
# every frame boundary, so expanding frame 7 does not require frames
# 0..6. That costs a little compression and buys the ability to skip.
#
# Times are **UNIX seconds** here, not the nanoseconds DATA_MODEL.md
# specifies: the line parser reads whole seconds out of log text
# today, and writing a unit the producer cannot fill would be a
# fiction. The header carries a version so the change stays visible.

import std.fs
import extract
import record
import segfile

pub fn frame_raw_bytes() -> u64 { 262144u64 }

# How much raw log one segment holds before it is closed.
#
# In v2 this was a ceiling imposed by the runtime: a reader had to
# take a whole file, so a segment had to stay small enough to read
# whole. `File::read_at` removed that (RUNTIME_GAPS.md R2), and the
# number is now a trade -- bigger segments mean fewer files and a
# longer compression context, at the cost of the arena a scan holds
# while it walks one.
pub fn segment_target_bytes() -> u64 { 8388608u64 }

# The term table starts here and doubles. A segment of this corpus
# holds about twenty thousand distinct terms.
pub fn term_slot_start() -> u64 { 4096u64 }

# The id that means "this record had no such field".
pub fn term_none() -> u64 { 18446744073709551615u64 }

# splitmix64's finalizer.
#
# The link table is keyed by `(from << 32) | to`, and masking that
# key directly keeps only the low bits of `to` -- so every link that
# points at `status:200` lands in the same slot and linear probing
# degenerates into a linear scan. Archiving this corpus took 58
# seconds before this mixer and 0.8 after it.
pub fn mix64(x: u64) -> u64 {
    var z = x + 11400714819323198485u64
    z = (z ^ (z >> 30u64)) * 13787848793156543929u64
    z = (z ^ (z >> 27u64)) * 10723151780598845931u64
    z ^ (z >> 31u64)
}

# The label set of one record, spelled out: `key=value` pairs
# separated by a space, in the order the record carries them (the
# reserved `host` and `tag` first, then the leading label run). Only
# called when a stream is seen for the first time.
#
# A pair the record spells twice -- a syslog header host and a
# `host=` label naming the same host -- is written once, because the
# stream's identity dropped it too.
fn stream_text_of(w: Span<u8>, rec: &ParsedLine) -> String {
    var out = String::new()
    var seen: Vec<u64> = Vec::new()
    if rec.has_host() {
        val h = extract::hash_term("host", w, rec.host_start(), rec.host_len())
        seen.push(h)
        out.push_str("host=")
        push_span_text(&mut out, w, rec.host_start(), rec.host_len())
    }
    if rec.tag_len() > 0u64 {
        val h = extract::hash_term("tag", w, rec.tag_start(), rec.tag_len())
        var dup = false
        var i: u64 = 0u64
        while i < seen.size() && !dup {
            val have: u64 = seen.get(i)
            if have == h { dup = true }
            i = i + 1u64
        }
        if !dup {
            seen.push(h)
            if out.len() > 0u64 { out.push(32u8) }
            out.push_str("tag=")
            push_span_text(&mut out, w, rec.tag_start(), rec.tag_len())
        }
    }
    val from = rec.labels_start()
    val end = from + rec.labels_len()
    var p = from
    while p < end {
        var k = p
        while k < end && w.get(k) != '=' { k = k + 1u64 }
        var v = k + 1u64
        if v > end { v = end }
        var stop = v
        while stop < end && w.get(stop) != ' ' { stop = stop + 1u64 }
        if k < end && stop > v {
            val h = extract::hash_term_span(w, p, k - p, v, stop - v)
            var dup = false
            var i: u64 = 0u64
            while i < seen.size() && !dup {
                val have: u64 = seen.get(i)
                if have == h { dup = true }
                i = i + 1u64
            }
            if !dup {
                seen.push(h)
                if out.len() > 0u64 { out.push(32u8) }
                push_span_text(&mut out, w, p, k - p)
                out.push(61u8)          # '='
                push_span_text(&mut out, w, v, stop - v)
            }
        }
        p = stop + 1u64
    }
    out
}

fn push_span_text(out: &mut String, w: Span<u8>, at: u64, len: u64) {
    var i: u64 = 0u64
    while i < len {
        val b: u8 = w.get(at + i)
        out.push(b)
        i = i + 1u64
    }
}

pub struct ArchiveWriter {
    arena: ByteWriter,   # every line, back to back
    recs: ByteWriter,    # the record table, already varint-encoded
    z: Lsz,              # the compressor, owned rather than passed
    count: u64,
    ts_min: i64,
    ts_max: i64,
    dated: u64,
    # --- the typed term index (ONTOLOGY.md O0-b) ---
    #
    # Terms are identified by a hash of `key:value`; only a term seen
    # for the first time gets a name. Postings are collected as two
    # parallel columns in emission order and bucketed by a counting
    # sort at flush -- `Vec::sort` is an insertion sort, so anything
    # that needs 300,000 elements in order has to avoid it.
    # A small open-addressing table of its own rather than `Dict`.
    # It began as a workaround -- `Dict<u64, u64>` needs `K: Hash`
    # and the stdlib's `impl Hash for u64` was not visible from
    # inside another auto-loaded module ("expected Hash, got u64").
    # That is fixed (2026-09-05: `Dict` is a hash table and mixes the
    # key with splitmix64 itself), so the table stays for the reason
    # it earned afterwards: the probe *is* the id assignment (a miss
    # appends to `term_hash` / `term_names` / `term_counts` in the
    # same step), and holding the columns side by side is what this
    # never-reusing allocator rewards.
    term_slots: Vec<u64>,     # id + 1, or 0 for empty
    term_hash: Vec<u64>,      # the hash of each term, by id
    term_names: Vec<String>,
    term_counts: Vec<u64>,
    # ONTOLOGY O1 の object 表: 語ごとに**日付を持つ**レコードの
    # 時刻の下限と上限。日付の無い行は寄与しないので、一度も寄与が
    # 無ければ `first > last` のまま残る — 空区間が「不明」を表すので、
    # 3 本目の列 (寄与した件数) を持たずに済む。
    term_first: Vec<i64>,
    term_last: Vec<i64>,
    post_term: Vec<u32>,
    post_ord: Vec<u32>,
    # --- co-occurrence links (ONTOLOGY.md O1) ---
    #
    # A link is two field values seen on the same record: `ip:1.2.3.4`
    # and `path:/robots.txt`. Counting them at write time is what lets
    # "which paths did this address ask for" be answered without
    # intersecting posting lists at query time.
    link_slots: Vec<u64>,     # index + 1, or 0
    link_key: Vec<u64>,       # (from << 32) | to, by index
    link_count: Vec<u64>,
    # --- streams (DATA_MODEL.md section 3) ---
    #
    # A stream is a **label set**: the reserved `host` / `tag` the
    # framing found, plus the leading `key=value` run, taken as a set.
    # `/v1/streams` answers out of this, and the UI asks for it every
    # time somebody picks a label, so it is counted while writing
    # rather than by expanding segments later.
    #
    # Identity is the **sum of the pairs' hashes**, which does not
    # depend on the order they were written in -- a sender that swaps
    # two labels is still the same stream. Duplicate pairs are dropped
    # first (a syslog header host and a `host=` label with the same
    # value are one pair, the same way they are one term), which is
    # what `stream_pairs` is for: a scratch column, cleared per record
    # so that the common case allocates nothing.
    stream_slots: Vec<u64>,   # index + 1, or 0
    stream_hash: Vec<u64>,
    stream_text: Vec<String>, # `key=value` pairs, space separated
    stream_count: Vec<u64>,
    stream_first: Vec<i64>,
    stream_last: Vec<i64>,
    stream_pairs: Vec<u64>,   # scratch: this record's pair hashes
}

impl ArchiveWriter {
    pub fn new() -> Self {
        val a = ByteWriter::with_capacity(segment_target_bytes() + frame_raw_bytes())
        val r = ByteWriter::with_capacity(1048576u64)
        val zz = Lsz::new()
        var slots: Vec<u64> = Vec::with_capacity(term_slot_start())
        var si: u64 = 0u64
        while si < term_slot_start() {
            slots.push(0u64)
            si = si + 1u64
        }
        val hashes: Vec<u64> = Vec::new()
        val names: Vec<String> = Vec::new()
        val tcounts: Vec<u64> = Vec::new()
        val tfirst: Vec<i64> = Vec::new()
        val tlast: Vec<i64> = Vec::new()
        val pterm: Vec<u32> = Vec::new()
        val pord: Vec<u32> = Vec::new()
        var lslots: Vec<u64> = Vec::with_capacity(term_slot_start())
        var li: u64 = 0u64
        while li < term_slot_start() {
            lslots.push(0u64)
            li = li + 1u64
        }
        val lkeys: Vec<u64> = Vec::new()
        val lcounts: Vec<u64> = Vec::new()
        var sslots: Vec<u64> = Vec::with_capacity(term_slot_start())
        var ssi: u64 = 0u64
        while ssi < term_slot_start() {
            sslots.push(0u64)
            ssi = ssi + 1u64
        }
        val shashes: Vec<u64> = Vec::new()
        val stexts: Vec<String> = Vec::new()
        val scounts: Vec<u64> = Vec::new()
        val sfirst: Vec<i64> = Vec::new()
        val slast: Vec<i64> = Vec::new()
        val spairs: Vec<u64> = Vec::new()
        ArchiveWriter {
            arena: a, recs: r, z: zz, count: 0u64,
            ts_min: 0i64, ts_max: 0i64, dated: 0u64,
            term_slots: slots, term_hash: hashes,
            term_names: names, term_counts: tcounts,
            term_first: tfirst, term_last: tlast,
            post_term: pterm, post_ord: pord,
            link_slots: lslots, link_key: lkeys, link_count: lcounts,
            stream_slots: sslots, stream_hash: shashes,
            stream_text: stexts, stream_count: scounts,
            stream_first: sfirst, stream_last: slast,
            stream_pairs: spairs,
        }
    }

    pub fn count(&self) -> u64 { self.count }
    pub fn arena_bytes(&self) -> u64 { self.arena.len() }
    pub fn is_full(&self) -> bool { self.arena.len() >= segment_target_bytes() }
    pub fn is_empty(&self) -> bool { self.count == 0u64 }
    pub fn ts_min(&self) -> i64 { self.ts_min }
    pub fn ts_max(&self) -> i64 { self.ts_max }

    pub fn reset(&mut self) {
        self.arena.clear()
        self.recs.clear()
        self.count = 0u64
        self.ts_min = 0i64
        self.ts_max = 0i64
        self.dated = 0u64
        var si: u64 = 0u64
        while si < self.term_slots.size() {
            self.term_slots.set(si, 0u64)
            si = si + 1u64
        }
        var ssr: u64 = 0u64
        while ssr < self.stream_slots.size() {
            self.stream_slots.set(ssr, 0u64)
            ssr = ssr + 1u64
        }
        self.stream_hash.clear()
        self.stream_text.clear()
        self.stream_count.clear()
        self.stream_first.clear()
        self.stream_last.clear()
        self.stream_pairs.clear()
        self.term_hash.clear()
        self.term_names.clear()
        self.term_counts.clear()
        self.term_first.clear()
        self.term_last.clear()
        self.post_term.clear()
        self.post_ord.clear()
        var lj: u64 = 0u64
        while lj < self.link_slots.size() {
            self.link_slots.set(lj, 0u64)
            lj = lj + 1u64
        }
        self.link_key.clear()
        self.link_count.clear()
    }

    pub fn term_count(&self) -> u64 { self.term_names.size() }
    pub fn posting_count(&self) -> u64 { self.post_ord.size() }

    # Record that this record carries `key = <the bytes at at..len>`.
    # Record `key = <bytes>` for this record, and answer the term's
    # id so the caller can link it to the record's other fields.
    # `term_none()` means the field was absent.
    # Find the slot for a term with this hash, making one if the hash
    # is new.
    #
    # The id is taken from `term_counts`, not from `term_names`, so
    # that a new term can be counted here and named by the caller one
    # step later: **`term_names` is shorter than `term_counts`
    # exactly when this made a new slot**, which is how the caller
    # knows without being told.
    #
    # The split exists because a label's key is bytes in the line
    # while a field's key is a literal, and building a `String` per
    # record to paper over that would allocate on the hot path
    # (MEMORY.md D4b).
    fn intern(&mut self, h: u64, has_ts: bool, ts: i64) -> u64 {
        val mask = self.term_slots.size() - 1u64
        var slot = h & mask
        var id: u64 = 0u64
        var placed = false
        while !placed {
            val cell: u64 = self.term_slots.get(slot)
            if cell == 0u64 {
                id = self.term_counts.size()
                self.term_counts.push(1u64)
                # Empty interval until a dated record says otherwise.
                if has_ts {
                    self.term_first.push(ts)
                    self.term_last.push(ts)
                } else {
                    self.term_first.push(limits::i64_max())
                    self.term_last.push(limits::i64_min())
                }
                self.term_hash.push(h)
                self.term_slots.set(slot, id + 1u64)
                placed = true
            } else {
                val cand = cell - 1u64
                val ch: u64 = self.term_hash.get(cand)
                if ch == h {
                    id = cand
                    val c: u64 = self.term_counts.get(cand)
                    self.term_counts.set(cand, c + 1u64)
                    if has_ts {
                        val f: i64 = self.term_first.get(cand)
                        val l: i64 = self.term_last.get(cand)
                        if ts < f { self.term_first.set(cand, ts) }
                        if ts > l { self.term_last.set(cand, ts) }
                    }
                    placed = true
                } else {
                    slot = (slot + 1u64) & mask
                }
            }
        }
        id
    }

    # Whether `intern` just made a slot that has no name yet.
    fn needs_name(&self) -> bool {
        self.term_names.size() < self.term_counts.size()
    }

    fn record_posting(&mut self, id: u64) {
        self.post_term.push(id as u32)
        self.post_ord.push(self.count as u32)
        # Grow before the table fills: linear probing degrades badly
        # past seven eighths, and a segment can hold tens of thousands
        # of distinct paths.
        if self.term_names.size() * 8u64 >= self.term_slots.size() * 7u64 {
            self.rehash()
        }
    }

    # Post `id` for the current record unless it already has been.
    #
    # One record can spell the same term twice: a syslog line whose
    # header host is `web01` and which also starts its message with
    # `host=web01` gives `host:web01` from both (they are one key by
    # design, DATA_MODEL.md section 2). Posting it twice made the
    # record count as two in `fields host` and put its ordinal in the
    # list twice. A record's postings are the run at the end of the
    # list, so the check walks back only over those -- a dozen at most.
    fn post_once(&mut self, id: u64) {
        val me = self.count as u32
        var k = self.post_ord.size()
        var dup = false
        var within = true
        while within && !dup && k > 0u64 {
            val o: u32 = self.post_ord.get(k - 1u64)
            if o != me {
                within = false
            } else {
                val t: u32 = self.post_term.get(k - 1u64)
                if t == (id as u32) { dup = true }
                k = k - 1u64
            }
        }
        if dup {
            # `intern` already counted this sighting; take it back.
            val c: u64 = self.term_counts.get(id)
            self.term_counts.set(id, c - 1u64)
        } else {
            self.record_posting(id)
        }
    }

    # A term for a key this program knows by name (`status`, `host`).
    fn emit(&mut self, w: Span<u8>, key: str, at: u64, len: u64,
            has_ts: bool, ts: i64) -> u64 {
        if len == 0u64 { return term_none() }
        val h = extract::hash_term(key, w, at, len)
        val id = self.intern(h, has_ts, ts)
        if self.needs_name() {
            val name = extract::term_text(key, w, at, len)
            self.term_names.push(name)
        }
        self.post_once(id)
        id
    }

    # A term for a key that is **in the line**: a label (`app=api`).
    # The hash has to match `emit`'s for the same `key:value`, or the
    # syslog framing's `host` and a `host=` label become two terms.
    fn emit_labelled(&mut self, w: Span<u8>, key_at: u64, key_len: u64,
                     at: u64, len: u64, has_ts: bool, ts: i64) -> u64 {
        if len == 0u64 || key_len == 0u64 { return term_none() }
        val h = extract::hash_term_span(w, key_at, key_len, at, len)
        val id = self.intern(h, has_ts, ts)
        if self.needs_name() {
            val name = extract::term_text_span(w, key_at, key_len, at, len)
            self.term_names.push(name)
        }
        self.post_once(id)
        id
    }

    # One co-occurrence: `from` and `to` were on the same record.
    fn link(&mut self, from_id: u64, to_id: u64) {
        if from_id == term_none() { return }
        if to_id == term_none() { return }
        val key = (from_id << 32u64) | to_id
        val mask = self.link_slots.size() - 1u64
        var slot = mix64(key) & mask
        var placed = false
        while !placed {
            val cell: u64 = self.link_slots.get(slot)
            if cell == 0u64 {
                val idx = self.link_key.size()
                self.link_key.push(key)
                self.link_count.push(1u64)
                self.link_slots.set(slot, idx + 1u64)
                placed = true
            } else {
                val at = cell - 1u64
                val k: u64 = self.link_key.get(at)
                if k == key {
                    val c: u64 = self.link_count.get(at)
                    self.link_count.set(at, c + 1u64)
                    placed = true
                } else {
                    slot = (slot + 1u64) & mask
                }
            }
        }
        if self.link_key.size() * 8u64 >= self.link_slots.size() * 7u64 {
            self.rehash_links()
        }
    }

    fn rehash_links(&mut self) {
        val bigger = self.link_slots.size() * 2u64
        var i = self.link_slots.size()
        while i < bigger {
            self.link_slots.push(0u64)
            i = i + 1u64
        }
        var k: u64 = 0u64
        while k < bigger {
            self.link_slots.set(k, 0u64)
            k = k + 1u64
        }
        val mask = bigger - 1u64
        var idx: u64 = 0u64
        while idx < self.link_key.size() {
            val key: u64 = self.link_key.get(idx)
            var slot = mix64(key) & mask
            var placed = false
            while !placed {
                val cell: u64 = self.link_slots.get(slot)
                if cell == 0u64 {
                    self.link_slots.set(slot, idx + 1u64)
                    placed = true
                } else {
                    slot = (slot + 1u64) & mask
                }
            }
            idx = idx + 1u64
        }
    }

    # Double the table and re-place every id.
    #
    # The table grows **in place**: a fresh `Vec` cannot be assigned
    # into the field ("cannot assign whole struct to nested field"),
    # so the slots are pushed, cleared, and refilled instead.
    fn rehash(&mut self) {
        val bigger = self.term_slots.size() * 2u64
        var i = self.term_slots.size()
        while i < bigger {
            self.term_slots.push(0u64)
            i = i + 1u64
        }
        var k: u64 = 0u64
        while k < bigger {
            self.term_slots.set(k, 0u64)
            k = k + 1u64
        }
        val mask = bigger - 1u64
        var id: u64 = 0u64
        while id < self.term_hash.size() {
            val h: u64 = self.term_hash.get(id)
            var slot = h & mask
            var placed = false
            while !placed {
                val cell: u64 = self.term_slots.get(slot)
                if cell == 0u64 {
                    self.term_slots.set(slot, id + 1u64)
                    placed = true
                } else {
                    slot = (slot + 1u64) & mask
                }
            }
            id = id + 1u64
        }
    }

    # Every term this record contributes. Shapes the ontology knows
    # get their fields named; everything else contributes only what
    # the framing already found (ONTOLOGY.md §2).
    # Every `key=value` the line began with, as its own term.
    #
    # Without this the labels are framed and then dropped: a record
    # ingested as `level=error app=api ...` could only be found by
    # searching its text, and `/v1/labels` would have nothing to
    # list. The rules are DATA_MODEL.md section 2's -- a key of
    # `[a-z0-9_]{1,32}`, a value that ends at a space -- and the
    # framing already decided where the run of labels stops, so this
    # only has to split what it was handed.
    fn emit_labels(&mut self, w: Span<u8>, rec: &ParsedLine) {
        val from = rec.labels_start()
        val end = from + rec.labels_len()
        var p = from
        while p < end {
            var k = p
            var has_eq = false
            var scanning = true
            while scanning && k < end {
                val b: u8 = w.get(k)
                if b == '=' {
                    has_eq = true
                    scanning = false
                } else {
                    k = k + 1u64
                }
            }
            if !has_eq {
                # The framing said this run is labels, so this cannot
                # normally happen; stopping is the safe reading.
                p = end
            } else {
                val v = k + 1u64
                var stop = v
                var running = true
                while running && stop < end {
                    val b: u8 = w.get(stop)
                    if b == ' ' { running = false } else { stop = stop + 1u64 }
                }
                val id = self.emit_labelled(w, p, k - p, v, stop - v,
                                            rec.has_ts, rec.ts)
                p = stop + 1u64
            }
        }
    }

    # The label-set of one record, noted against its stream.
    #
    # The pairs are hashed (not spelled) so that a record costs no
    # allocation; only a stream seen for the first time is written
    # out, which is the same bargain the term dictionary makes.
    fn note_stream(&mut self, w: Span<u8>, rec: &ParsedLine) {
        self.stream_pairs.clear()
        if rec.has_host() {
            val h = extract::hash_term("host", w, rec.host_start(), rec.host_len())
            self.push_pair(h)
        }
        if rec.tag_len() > 0u64 {
            val h = extract::hash_term("tag", w, rec.tag_start(), rec.tag_len())
            self.push_pair(h)
        }
        val from = rec.labels_start()
        val end = from + rec.labels_len()
        var p = from
        while p < end {
            var k = p
            while k < end && w.get(k) != '=' { k = k + 1u64 }
            var v = k + 1u64
            if v > end { v = end }
            var stop = v
            while stop < end && w.get(stop) != ' ' { stop = stop + 1u64 }
            if k < end && stop > v {
                val h = extract::hash_term_span(w, p, k - p, v, stop - v)
                self.push_pair(h)
            }
            p = stop + 1u64
        }

        # The key is order-free, so `app=api level=error` and
        # `level=error app=api` are one stream.
        var key: u64 = 14695981039346656037u64
        var i: u64 = 0u64
        while i < self.stream_pairs.size() {
            val h: u64 = self.stream_pairs.get(i)
            key = key + mix64(h)
            i = i + 1u64
        }
        self.intern_stream(w, rec, key)
    }

    # Add a pair unless this record already carries it.
    fn push_pair(&mut self, h: u64) {
        var i: u64 = 0u64
        var seen = false
        while i < self.stream_pairs.size() && !seen {
            val have: u64 = self.stream_pairs.get(i)
            if have == h { seen = true }
            i = i + 1u64
        }
        if !seen { self.stream_pairs.push(h) }
    }

    fn intern_stream(&mut self, w: Span<u8>, rec: &ParsedLine, key: u64) {
        val mask = self.stream_slots.size() - 1u64
        var slot = key & mask
        var id: u64 = 0u64
        var placed = false
        while !placed {
            val cell: u64 = self.stream_slots.get(slot)
            if cell == 0u64 {
                id = self.stream_hash.size()
                self.stream_hash.push(key)
                val text = stream_text_of(w, rec)
                self.stream_text.push(text)
                self.stream_count.push(1u64)
                if rec.has_ts {
                    self.stream_first.push(rec.ts)
                    self.stream_last.push(rec.ts)
                } else {
                    self.stream_first.push(limits::i64_max())
                    self.stream_last.push(limits::i64_min())
                }
                self.stream_slots.set(slot, id + 1u64)
                placed = true
            } else {
                val cand = cell - 1u64
                val ch: u64 = self.stream_hash.get(cand)
                if ch == key {
                    id = cand
                    val c: u64 = self.stream_count.get(cand)
                    self.stream_count.set(cand, c + 1u64)
                    if rec.has_ts {
                        val fi: i64 = self.stream_first.get(cand)
                        val la: i64 = self.stream_last.get(cand)
                        if rec.ts < fi { self.stream_first.set(cand, rec.ts) }
                        if rec.ts > la { self.stream_last.set(cand, rec.ts) }
                    }
                    placed = true
                } else {
                    slot = (slot + 1u64) & mask
                }
            }
        }
        # The table is small (streams are few next to terms), but a
        # busy ingest can still fill it; grow on the same rule the
        # term table uses.
        if self.stream_hash.size() * 8u64 >= self.stream_slots.size() * 7u64 {
            self.rehash_streams()
        }
    }

    fn rehash_streams(&mut self) {
        val bigger = self.stream_slots.size() * 2u64
        var i = self.stream_slots.size()
        while i < bigger {
            self.stream_slots.push(0u64)
            i = i + 1u64
        }
        var k: u64 = 0u64
        while k < bigger {
            self.stream_slots.set(k, 0u64)
            k = k + 1u64
        }
        val mask = bigger - 1u64
        var id: u64 = 0u64
        while id < self.stream_hash.size() {
            val h: u64 = self.stream_hash.get(id)
            var slot = h & mask
            var placed = false
            while !placed {
                val cell: u64 = self.stream_slots.get(slot)
                if cell == 0u64 {
                    self.stream_slots.set(slot, id + 1u64)
                    placed = true
                } else {
                    slot = (slot + 1u64) & mask
                }
            }
            id = id + 1u64
        }
    }

    fn emit_terms(&mut self, w: Span<u8>, ln: Line, rec: &ParsedLine) {
        if rec.labels_len() > 0u64 { self.emit_labels(w, rec) }
        var host_id = term_none()
        var tag_id = term_none()
        if rec.has_host() {
            host_id = self.emit(w, "host", rec.host_start(), rec.host_len(), rec.has_ts, rec.ts)
        }
        if rec.tag_len() > 0u64 {
            tag_id = self.emit(w, "tag", rec.tag_start(), rec.tag_len(), rec.has_ts, rec.ts)
        }
        self.link(host_id, tag_id)

        if rec.kind == 3u32 {
            val f = extract::http(w, ln.start, ln.len)
            if f.ok {
                val status_id = self.emit(w, "status", extract::field_start(f.status), extract::field_len(f.status), rec.has_ts, rec.ts)
                val method_id = self.emit(w, "method", extract::field_start(f.method), extract::field_len(f.method), rec.has_ts, rec.ts)
                val path_id = self.emit(w, "path", extract::field_start(f.path), extract::field_len(f.path), rec.has_ts, rec.ts)
                val ip_id = self.emit(w, "ip", extract::field_start(f.client), extract::field_len(f.client), rec.has_ts, rec.ts)
                val vhost_id = self.emit(w, "vhost", extract::field_start(f.vhost), extract::field_len(f.vhost), rec.has_ts, rec.ts)
                val ua_id = self.emit(w, "ua", extract::field_start(f.ua), extract::field_len(f.ua), rec.has_ts, rec.ts)
                # Not linked to anything, but it has to be a term: the
                # query reads `proto=HTTP/1.1` as one, and a term that
                # was never written prunes every segment.
                val proto_id = self.emit(w, "proto", extract::field_start(f.proto), extract::field_len(f.proto), rec.has_ts, rec.ts)
                # A fixed set of pairs, not every combination: six
                # fields would make thirty ordered pairs per record,
                # and the ones worth asking about are few.
                self.link(ip_id, path_id)
                self.link(ip_id, status_id)
                self.link(ip_id, ua_id)
                self.link(path_id, status_id)
                self.link(path_id, ip_id)
                self.link(vhost_id, path_id)
            }
        }
    }

    # Append one line and its framing. The bytes go into the arena;
    # everything else becomes varints in the record table.
    #
    # The record's offsets are stored **relative to its own line**, so
    # the table does not have to be rewritten if the arena moves, and
    # the numbers stay one byte each for the common case.
    pub fn add(&mut self, src: Span<u8>, ln: Line, rec: &ParsedLine) {
        val start = ln.start
        val len = ln.len

        # flags: bit 0 = dated, bits 1..3 = shape
        var flags: u64 = 0u64
        if rec.has_ts { flags = 1u64 }
        flags = flags | ((rec.kind as u64) << 1u64)

        var ts_off: u64 = 0u64
        if rec.has_ts {
            if self.dated == 0u64 {
                self.ts_min = rec.ts
                self.ts_max = rec.ts
            } else {
                if rec.ts < self.ts_min { self.ts_min = rec.ts }
                if rec.ts > self.ts_max { self.ts_max = rec.ts }
            }
            self.dated = self.dated + 1u64
        }

        # Terms are emitted before the arena grows, because they name
        # offsets in `src`, not in the arena.
        self.emit_terms(src, ln, rec)
        self.note_stream(src, rec)

        self.arena.put_span(src, start, len)

        self.recs.put_varint(flags)
        self.recs.put_varint(len)
        # The timestamp is stored whole. A delta against the segment's
        # minimum would be smaller, but the minimum is not known until
        # the segment closes, and rewriting the column at that point
        # costs more than the bytes it saves.
        self.recs.put_varint(rec.ts as u64)
        self.recs.put_varint(rel_of(rec.host_start(), start))
        self.recs.put_varint(rec.host_len())
        self.recs.put_varint(rel_of(rec.tag_start(), start))
        self.recs.put_varint(rec.tag_len())
        self.recs.put_varint(rel_of(rec.labels_start(), start))
        self.recs.put_varint(rec.labels_len())
        self.recs.put_varint(rel_of(rec.body_start(), start))
        self.recs.put_varint(rec.body_len())

        self.count = self.count + 1u64
    }

    # Compress the arena into the open file as frames, filling `ftab`
    # with (file offset, arena offset, raw length) for each. Answers
    # the number of bytes written, or zero if a write failed.
    #
    # **The frames go straight out.** v2 built the whole file in a
    # `ByteWriter` and handed it to `write_file_bytes`, so a segment
    # cost its own size in memory twice over -- once in the arena,
    # once in the output buffer. With `File` the second buffer is one
    # frame (256 KiB), whatever the segment grows to.
    fn write_frames(&mut self, f: &File, ftab: &mut ByteWriter, crc: &Crc32,
                    base_off: u64) -> u64 {
        val total = self.arena.len()
        val w = self.arena.span()
        var blk = ByteWriter::with_capacity(frame_raw_bytes() + 65536u64)
        var written: u64 = 0u64
        var ok = true
        match w {
            Option::Some(arena) => {
                var at: u64 = 0u64
                while at < total && ok {
                    var raw = frame_raw_bytes()
                    if at + raw > total { raw = total - at }
                    val sum = crc.of(arena, at, raw)

                    blk.clear()
                    blk.put_magic("LSF1")
                    val codec_at = blk.len()
                    blk.put_u32(1u64)          # codec, patched to 0 if stored
                    blk.put_u32(raw)
                    val clen_at = blk.len()
                    blk.put_u32(0u64)          # compressed length, patched below
                    blk.put_u32(sum)
                    blk.put_u32(0u64)          # reserved
                    val body_at = blk.len()

                    val clen = self.z.encode(arena, at, raw, &mut blk)
                    # Compression has to earn its place: a frame that
                    # does not shrink by an eighth is stored raw, so a
                    # blob of random or already-compressed bytes never
                    # comes out *larger* than it went in.
                    if clen * 8u64 >= raw * 7u64 {
                        blk.truncate(body_at)
                        blk.reserve(raw)
                        blk.put_span_fast(arena, at, raw)
                        blk.patch_u32(codec_at, 0u64)
                        blk.patch_u32(clen_at, raw)
                    } else {
                        blk.patch_u32(clen_at, clen)
                    }

                    ftab.put_u64(base_off + written)
                    ftab.put_u64(at)
                    ftab.put_u32(raw)

                    val bw = blk.span()
                    match bw {
                        Option::Some(bb) => {
                            if !segfile::put_bytes(f, bb, blk.len()) { ok = false }
                        }
                        Option::None => { ok = false }
                    }
                    written = written + blk.len()
                    at = at + raw
                }
            }
            Option::None => { }
        }
        if !ok { return 0u64 }
        written
    }

    # Write the segment out as `<base>.seg` (STORAGE_FORMAT.md v3).
    #
    # One file, written front to back: 320 zero bytes to hold the
    # header and the section directory, then the frames, then the
    # sections, then the header written back over the reservation
    # with `write_at` once every offset is known.
    #
    # v2 wrote `.dat` and `.idx` separately because a reader could
    # only take a whole file, and pruning had to be possible without
    # reading the data. `File::read_at` removed that reason
    # (RUNTIME_GAPS.md R2), and one file removes the state where the
    # data is published and its index is not.
    pub fn finish(&mut self, base: str, segid: u64, crc: &Crc32)
        -> Result<u64, IoError>
    {
        val stem = String::from_str(base)
        val ext = String::from_str(".seg")
        var path = stem.concat(&ext)
        val f = File::create(path.to_str())?
        val total: u64 = self.write_seg(&f, segid, crc)
        # Zero is the failure: a segment always carries at least a
        # header, so the count can carry the verdict without a second
        # return value.
        if total == 0u64 { return Result::Err(IoError::WriteError) }
        Result::Ok(total)
    }

    # Everything `finish` does once the file is open.
    #
    # Separate because the open has to be `match`ed, and a body this
    # size inside the arm is unreadable -- and because the arm is
    # where the handle lives, so the file is closed by its `Drop` the
    # moment this returns.
    fn write_seg(&mut self, f: &File, segid: u64, crc: &Crc32) -> u64 {
        var hdr = ByteWriter::with_capacity(segfile::data_at() + 64u64)
        var ok = true

        # 1. Reserve the header and the directory.
        var z: u64 = 0u64
        while z < segfile::data_at() { hdr.put_u8(0u8)  z = z + 1u64 }
        val zw = hdr.span()
        match zw {
            Option::Some(zb) => {
                if !segfile::put_bytes(f, zb, hdr.len()) { ok = false }
            }
            Option::None => { ok = false }
        }
        if !ok { return 0u64 }

        # 2. The frames.
        var ftab = ByteWriter::with_capacity(65536u64)
        val frames_off = segfile::data_at()
        val frames_len = self.write_frames(f, &mut ftab, crc, frames_off)
        val n_frames = ftab.len() / 20u64
        if frames_len == 0u64 && self.arena.len() > 0u64 { return 0u64 }
        var at_off = frames_off + frames_len

        # 3. The record table, as it was built (uncompressed: a query
        #    reads it whole and walks it once).
        val recs_off = at_off
        val recs_len = self.recs.len()
        var recs_crc: u64 = 0u64
        val rw = self.recs.span()
        match rw {
            Option::Some(rb) => {
                recs_crc = crc.of(rb, 0u64, recs_len)
                if !segfile::put_bytes(f, rb, recs_len) { ok = false }
            }
            Option::None => { }
        }
        if !ok { return 0u64 }
        at_off = at_off + recs_len

        # 4. The frame table: (file offset, arena offset, raw length).
        val ftab_off = at_off
        val ftab_len = ftab.len()
        var ftab_crc: u64 = 0u64
        val fw = ftab.span()
        match fw {
            Option::Some(fb) => {
                ftab_crc = crc.of(fb, 0u64, ftab_len)
                if !segfile::put_bytes(f, fb, ftab_len) { ok = false }
            }
            Option::None => { }
        }
        if !ok { return 0u64 }
        at_off = at_off + ftab_len

        # ---- the typed term index (ONTOLOGY.md O0-b/O0-c) --------
        #
        # The section is built in a local buffer and then compressed
        # into the index. Term names repeat heavily -- twenty thousand
        # paths that share their prefixes -- so LSZ1 earns its place
        # here more than anywhere else in the format.
        var tsec = ByteWriter::with_capacity(1048576u64)
        val n_terms = self.term_names.size()
        tsec.put_u32(n_terms)

        # The names, in first-seen order.
        #
        # The design (STORAGE_FORMAT.md §4) calls for lexicographic
        # order so that a prefix can be found by bisection. First-seen
        # order is written instead because sorting twenty thousand
        # strings would go through `Vec::sort`, an insertion sort:
        # quadratic here. The reader builds a hash map in one pass, so
        # exact lookup is unaffected; prefix search is what this
        # postpones, and it arrives with a real sort.
        var t: u64 = 0u64
        while t < n_terms {
            val name: String = self.term_names.get(t)
            tsec.put_varint(name.len())
            val nw = name.as_span()
            match nw {
                Option::Some(nb) => { tsec.put_span(nb, 0u64, name.len()) }
                Option::None => { }
            }
            t = t + 1u64
        }

        # Postings are bucketed by counting sort: the emission order
        # is already ascending within each term, so a stable scatter
        # leaves every posting list sorted with no comparisons.
        val n_post = self.post_ord.size()
        var start_of: Vec<u64> = Vec::with_capacity(n_terms + 1u64)
        var cursor: Vec<u64> = Vec::with_capacity(n_terms + 1u64)
        var acc: u64 = 0u64
        t = 0u64
        while t < n_terms {
            start_of.push(acc)
            cursor.push(acc)
            val c: u64 = self.term_counts.get(t)
            acc = acc + c
            t = t + 1u64
        }
        var bucket: Vec<u32> = Vec::with_capacity(n_post + 1u64)
        bucket.set_size(n_post)
        var k: u64 = 0u64
        while k < n_post {
            val tid: u32 = self.post_term.get(k)
            val at: u64 = cursor.get(tid as u64)
            val ord: u32 = self.post_ord.get(k)
            bucket.set(at, ord)
            cursor.set(tid as u64, at + 1u64)
            k = k + 1u64
        }

        # Per-term header: doc_count, then where its postings sit in
        # the blob that follows.
        var blob = ByteWriter::with_capacity(n_post * 2u64 + 64u64)
        var heads: Vec<u64> = Vec::with_capacity(n_terms + 1u64)
        var lens: Vec<u64> = Vec::with_capacity(n_terms + 1u64)
        t = 0u64
        while t < n_terms {
            val from = start_of.get(t)
            val c: u64 = self.term_counts.get(t)
            val head = blob.len()
            var prev: u64 = 0u64
            var i: u64 = 0u64
            while i < c {
                val ord: u32 = bucket.get(from + i)
                val v = ord as u64
                blob.put_varint(v - prev)
                prev = v
                i = i + 1u64
            }
            heads.push(head)
            lens.push(blob.len() - head)
            t = t + 1u64
        }

        t = 0u64
        while t < n_terms {
            val c: u64 = self.term_counts.get(t)
            tsec.put_varint(c)
            tsec.put_varint(heads.get(t))
            tsec.put_varint(lens.get(t))
            t = t + 1u64
        }
        val blob_at = tsec.len()
        tsec.put_varint(blob.len())
        val bw = blob.span()
        match bw {
            Option::Some(bb) => { tsec.put_span(bb, 0u64, blob.len()) }
            Option::None => { }
        }
        # Wrap and compress the section, header first:
        #   "LST1", codec, raw length, compressed length, raw CRC
        var blk = ByteWriter::with_capacity(1048576u64)
        val terms_off = at_off
        val raw_len = tsec.len()
        var raw_crc: u64 = 0u64
        val tw = tsec.span()
        match tw {
            Option::Some(tb) => { raw_crc = crc.of(tb, 0u64, raw_len) }
            Option::None => { }
        }
        blk.put_magic("LST1")
        val tcodec_at = blk.len()
        blk.put_u32(1u64)
        blk.put_u32(raw_len)
        val tclen_at = blk.len()
        blk.put_u32(0u64)
        blk.put_u32(raw_crc)
        val tbody_at = blk.len()
        match tw {
            Option::Some(tb) => {
                val clen = self.z.encode(tb, 0u64, raw_len, &mut blk)
                if clen * 8u64 >= raw_len * 7u64 {
                    blk.truncate(tbody_at)
                    blk.put_span(tb, 0u64, raw_len)
                    blk.patch_u32(tcodec_at, 0u64)
                    blk.patch_u32(tclen_at, raw_len)
                } else {
                    blk.patch_u32(tclen_at, clen)
                }
            }
            Option::None => { }
        }
        val terms_len = blk.len()
        var terms_crc: u64 = 0u64
        val bw2 = blk.span()
        match bw2 {
            Option::Some(bb) => {
                terms_crc = crc.of(bb, 0u64, terms_len)
                if !segfile::put_bytes(f, bb, terms_len) { ok = false }
            }
            Option::None => { }
        }
        if !ok { return 0u64 }
        at_off = at_off + terms_len

        # ---- the link section (ONTOLOGY.md O1) -------------------
        #
        # Rows are grouped by their `from` term, which is a counting
        # sort over the term ids -- the same trick the postings use,
        # and for the same reason: `Vec::sort` is an insertion sort.
        # Within a group the `to` ids are in whatever order they were
        # first seen; nothing reads them in order.
        var lsec = ByteWriter::with_capacity(1048576u64)
        val n_links = self.link_key.size()
        var per_from: Vec<u64> = Vec::with_capacity(n_terms + 1u64)
        var lcursor: Vec<u64> = Vec::with_capacity(n_terms + 1u64)
        var lt: u64 = 0u64
        while lt < n_terms {
            per_from.push(0u64)
            lcursor.push(0u64)
            lt = lt + 1u64
        }
        var li: u64 = 0u64
        while li < n_links {
            val key: u64 = self.link_key.get(li)
            val from = key >> 32u64
            if from < n_terms {
                val c: u64 = per_from.get(from)
                per_from.set(from, c + 1u64)
            }
            li = li + 1u64
        }
        var lacc: u64 = 0u64
        var groups: u64 = 0u64
        lt = 0u64
        while lt < n_terms {
            val c: u64 = per_from.get(lt)
            lcursor.set(lt, lacc)
            lacc = lacc + c
            if c > 0u64 { groups = groups + 1u64 }
            lt = lt + 1u64
        }
        var lbucket: Vec<u32> = Vec::with_capacity(n_links + 1u64)
        lbucket.set_size(n_links)
        li = 0u64
        while li < n_links {
            val key: u64 = self.link_key.get(li)
            val from = key >> 32u64
            if from < n_terms {
                val at: u64 = lcursor.get(from)
                lbucket.set(at, li as u32)
                lcursor.set(from, at + 1u64)
            }
            li = li + 1u64
        }

        lsec.put_u32(groups)
        var start: u64 = 0u64
        lt = 0u64
        while lt < n_terms {
            val c: u64 = per_from.get(lt)
            if c > 0u64 {
                lsec.put_varint(lt)
                lsec.put_varint(c)
                var k: u64 = 0u64
                while k < c {
                    val which: u32 = lbucket.get(start + k)
                    val key: u64 = self.link_key.get(which as u64)
                    val cnt: u64 = self.link_count.get(which as u64)
                    lsec.put_varint(key & 0xFFFFFFFFu64)
                    lsec.put_varint(cnt)
                    k = k + 1u64
                }
            }
            start = start + c
            lt = lt + 1u64
        }

        blk.clear()
        val links_off = at_off
        val lraw_len = lsec.len()
        var lraw_crc: u64 = 0u64
        val lw = lsec.span()
        match lw {
            Option::Some(lb) => { lraw_crc = crc.of(lb, 0u64, lraw_len) }
            Option::None => { }
        }
        blk.put_magic("LST1")
        val lcodec_at = blk.len()
        blk.put_u32(1u64)
        blk.put_u32(lraw_len)
        val lclen_at = blk.len()
        blk.put_u32(0u64)
        blk.put_u32(lraw_crc)
        val lbody_at = blk.len()
        match lw {
            Option::Some(lb) => {
                val clen = self.z.encode(lb, 0u64, lraw_len, &mut blk)
                if clen * 8u64 >= lraw_len * 7u64 {
                    blk.truncate(lbody_at)
                    blk.put_span(lb, 0u64, lraw_len)
                    blk.patch_u32(lcodec_at, 0u64)
                    blk.patch_u32(lclen_at, lraw_len)
                } else {
                    blk.patch_u32(lclen_at, clen)
                }
            }
            Option::None => { }
        }
        val links_len = blk.len()
        var links_crc: u64 = 0u64
        val bw3 = blk.span()
        match bw3 {
            Option::Some(bb) => {
                links_crc = crc.of(bb, 0u64, links_len)
                if !segfile::put_bytes(f, bb, links_len) { ok = false }
            }
            Option::None => { }
        }
        if !ok { return 0u64 }
        at_off = at_off + links_len

        # ---- the object table (ONTOLOGY.md O1) -------------------
        #
        # Per term, the span of time it was seen over. The count is
        # already in the dictionary (`doc_count`), so this adds only
        # the two ends -- which is the whole of what O1 was missing.
        #
        # Terms are written in id order, so the table needs no names
        # and a lookup is the same id the dictionary already answers.
        # A term seen only on undated lines has no span; that is a
        # leading 0 rather than a sentinel timestamp, so the format
        # never has to name a value that means "not a value".
        var osec = ByteWriter::with_capacity(262144u64)
        val n_terms_o = self.term_names.size()
        osec.put_u32(n_terms_o)
        var oi: u64 = 0u64
        while oi < n_terms_o {
            val fi: i64 = self.term_first.get(oi)
            val la: i64 = self.term_last.get(oi)
            if fi <= la {
                osec.put_varint(1u64)
                osec.put_varint(fi as u64)
                osec.put_varint((la - fi) as u64)
            } else {
                osec.put_varint(0u64)
            }
            oi = oi + 1u64
        }

        blk.clear()
        val objs_off = at_off
        val oraw_len = osec.len()
        var oraw_crc: u64 = 0u64
        val ow = osec.span()
        match ow {
            Option::Some(ob) => { oraw_crc = crc.of(ob, 0u64, oraw_len) }
            Option::None => { }
        }
        blk.put_magic("LST1")
        val ocodec_at = blk.len()
        blk.put_u32(1u64)
        blk.put_u32(oraw_len)
        val oclen_at = blk.len()
        blk.put_u32(0u64)
        blk.put_u32(oraw_crc)
        val obody_at = blk.len()
        match ow {
            Option::Some(ob) => {
                val clen = self.z.encode(ob, 0u64, oraw_len, &mut blk)
                if clen * 8u64 >= oraw_len * 7u64 {
                    blk.truncate(obody_at)
                    blk.put_span(ob, 0u64, oraw_len)
                    blk.patch_u32(ocodec_at, 0u64)
                    blk.patch_u32(oclen_at, oraw_len)
                } else {
                    blk.patch_u32(oclen_at, clen)
                }
            }
            Option::None => { }
        }
        val objs_len = blk.len()
        var objs_crc: u64 = 0u64
        val bw4 = blk.span()
        match bw4 {
            Option::Some(bb) => {
                objs_crc = crc.of(bb, 0u64, objs_len)
                if !segfile::put_bytes(f, bb, objs_len) { ok = false }
            }
            Option::None => { ok = false }
        }
        if !ok { return 0u64 }
        at_off = at_off + objs_len

        # ---- the stream table (DATA_MODEL.md section 3) ----------
        #
        # One row per label set: the text, how many records carried
        # it, and the span of time they covered. Rows are few (a label
        # set per sender, not per value), so this is written plainly
        # in first-seen order and read whole.
        var ssec = ByteWriter::with_capacity(65536u64)
        val n_streams = self.stream_text.size()
        ssec.put_u32(n_streams)
        var sti: u64 = 0u64
        while sti < n_streams {
            val text: String = self.stream_text.get(sti)
            ssec.put_varint(text.len())
            val sw = text.as_span()
            match sw {
                Option::Some(sb) => { ssec.put_span(sb, 0u64, text.len()) }
                Option::None => { }
            }
            val c: u64 = self.stream_count.get(sti)
            ssec.put_varint(c)
            val fi: i64 = self.stream_first.get(sti)
            val la: i64 = self.stream_last.get(sti)
            # The same "no span" shape the object table uses: a
            # leading 0 rather than a timestamp that means "none".
            if fi <= la {
                ssec.put_varint(1u64)
                ssec.put_varint(fi as u64)
                ssec.put_varint((la - fi) as u64)
            } else {
                ssec.put_varint(0u64)
            }
            sti = sti + 1u64
        }

        blk.clear()
        val strs_off = at_off
        val sraw_len = ssec.len()
        var sraw_crc: u64 = 0u64
        val ssw = ssec.span()
        match ssw {
            Option::Some(sb) => { sraw_crc = crc.of(sb, 0u64, sraw_len) }
            Option::None => { }
        }
        blk.put_magic("LST1")
        val scodec_at = blk.len()
        blk.put_u32(1u64)
        blk.put_u32(sraw_len)
        val sclen_at = blk.len()
        blk.put_u32(0u64)
        blk.put_u32(sraw_crc)
        val sbody_at = blk.len()
        match ssw {
            Option::Some(sb) => {
                val clen = self.z.encode(sb, 0u64, sraw_len, &mut blk)
                if clen * 8u64 >= sraw_len * 7u64 {
                    blk.truncate(sbody_at)
                    blk.put_span(sb, 0u64, sraw_len)
                    blk.patch_u32(scodec_at, 0u64)
                    blk.patch_u32(sclen_at, sraw_len)
                } else {
                    blk.patch_u32(sclen_at, clen)
                }
            }
            Option::None => { }
        }
        val strs_len = blk.len()
        var strs_crc: u64 = 0u64
        val bw5 = blk.span()
        match bw5 {
            Option::Some(bb) => {
                strs_crc = crc.of(bb, 0u64, strs_len)
                if !segfile::put_bytes(f, bb, strs_len) { ok = false }
            }
            Option::None => { ok = false }
        }
        if !ok { return 0u64 }
        at_off = at_off + strs_len

        # ---- the header and the directory, written back ----------
        #
        # Everything above had to happen before these numbers
        # existed, which is why the file opens with 320 reserved
        # bytes rather than a header. `write_at` puts them where they
        # belong without disturbing the cursor.
        hdr.clear()
        hdr.put_magic("LSD3")
        hdr.put_u32(segfile::seg_version())
        hdr.put_u64(segid)
        hdr.put_u64(self.ts_min as u64)
        hdr.put_u64(self.ts_max as u64)
        hdr.put_u64(self.count)
        hdr.put_u32(n_frames)
        hdr.put_u32(frame_raw_bytes())
        hdr.put_u64(self.arena.len())
        hdr.put_u32(0u64)                  # kind: 0 = segment
        val hcrc_at = hdr.len()
        hdr.put_u32(0u64)                  # header crc, patched below
        val hsum_w = hdr.span()
        match hsum_w {
            Option::Some(hb) => {
                val hsum = crc.of(hb, 0u64, hcrc_at)
                hdr.patch_u32(hcrc_at, hsum)
            }
            Option::None => { }
        }

        hdr.put_u32(7u64)                  # section count
        hdr.put_u32(0u64)                  # reserved
        put_dir(&mut hdr, segfile::kind_frames(), frames_off, frames_len, 0u64)
        put_dir(&mut hdr, segfile::kind_records(), recs_off, recs_len, recs_crc)
        put_dir(&mut hdr, segfile::kind_ftable(), ftab_off, ftab_len, ftab_crc)
        put_dir(&mut hdr, segfile::kind_terms(), terms_off, terms_len, terms_crc)
        put_dir(&mut hdr, segfile::kind_links(), links_off, links_len, links_crc)
        put_dir(&mut hdr, segfile::kind_objects(), objs_off, objs_len, objs_crc)
        put_dir(&mut hdr, segfile::kind_streams(), strs_off, strs_len, strs_crc)
        while hdr.len() < segfile::data_at() { hdr.put_u8(0u8) }

        val hw2 = hdr.span()
        match hw2 {
            Option::Some(hb) => {
                val put = f.write_at(0u64, hb.slice(0u64, segfile::data_at()))
                match put {
                    Result::Ok(n) => { if n != segfile::data_at() { ok = false } }
                    Result::Err(e) => { ok = false }
                }
            }
            Option::None => { ok = false }
        }
        if !ok { return 0u64 }

        # The bytes are not on the disk until this returns. A segment
        # that is published by a rename and then lost to a power cut
        # would leave a catalogue entry pointing at nothing
        # (STORAGE_FORMAT.md §2).
        val synced = f.sync()
        match synced {
            Result::Ok(u) => { }
            Result::Err(e) => { ok = false }
        }
        if !ok { return 0u64 }
        at_off
    }
}

# One directory slot: kind, offset, length, CRC of the stored bytes.
fn put_dir(head: &mut ByteWriter, kind: u64, off: u64, len: u64, sum: u64) {
    head.put_u32(kind)
    head.put_u64(off)
    head.put_u64(len)
    head.put_u32(sum)
}


# The offset of a part within its line, or 0 when the part is absent.
fn rel_of(abs: u64, line_start: u64) -> u64 {
    if abs <= line_start { 0u64 } else { abs - line_start }
}

# ---------------------------------------------------------------------
# The record table, decoded.
#
# On disk a record is eleven varints side by side (`add_line`), which
# is the only order a varint stream can be read in. A query, though,
# wants the table the other way round: `mark_frames` needs where each
# candidate's line sits, the walk needs a handful of fields for each
# candidate, and neither wants the fields it does not look at. So the
# table is decoded **once** per segment into columns and the candidates
# are looked up by ordinal -- before, both passes re-read all eleven
# varints of every record, including the ones the index had already
# ruled out.
#
# `soa Vec` puts each field in a column of its own width (37 bytes a
# row, with no padding between fields), so a pass that reads `line_at`
# touches only that column. `line_at` is not on disk:
# it is the running sum of `line_len`, and it is what lets a record be
# found without walking the ones before it.
#
# The labels and body offsets are dropped here: nothing that walks the
# table reads them yet.
pub struct RecRow {
    line_at: u64,
    ts: i64,
    line_len: u32,
    host_rel: u32,
    host_len: u32,
    tag_rel: u32,
    tag_len: u32,
    flags: u8,
}

# Decode up to `n` records of `rb` into `rows`, which is cleared first
# and keeps its capacity, so one `SoaVec` serves every segment of a
# walk. Stops early on a short table, as the per-record walks did; the
# caller reads the count off `rows.size()`.
#
# The varints themselves are what this costs (about 2 ns a byte,
# 20 ms for 444,549 records); the `push` is a tenth of it. Reading the
# bytes through the raw address instead of `take_varint` was tried and
# measured the same, so the safe form stays.
pub fn decode_records(rb: Span<u8>, recs_len: u64, n: u64,
                      rows: &mut SoaVec<RecRow>) {
    rows.clear()
    var rd = ByteReader::new(recs_len)
    var line_at: u64 = 0u64
    var r: u64 = 0u64
    while r < n && rd.remaining() > 0u64 {
        val flags = rd.take_varint(rb)
        val line_len = rd.take_varint(rb)
        val ts = rd.take_varint(rb) as i64
        val host_rel = rd.take_varint(rb)
        val host_len = rd.take_varint(rb)
        val tag_rel = rd.take_varint(rb)
        val tag_len = rd.take_varint(rb)
        # labels and body offsets
        rd.skip_varints(rb, 4u64)
        rows.push(RecRow {
            line_at: line_at, ts: ts,
            line_len: line_len as u32,
            host_rel: host_rel as u32, host_len: host_len as u32,
            tag_rel: tag_rel as u32, tag_len: tag_len as u32,
            flags: flags as u8,
        })
        line_at = line_at + line_len
        r = r + 1u64
    }
}

# ---------------------------------------------------------------------
# Reading a segment back.
#
# The file layer moved to `segfile.t` when the format became one file
# (v3): opening, the header and directory, `read_range`, `expand_all`
# and `load_block` all live there, because they are about *where the
# bytes are*, not about what they mean. What stays here is the
# meaning: term dictionaries, posting lists and links, all of which
# take an expanded section as a window and never touch a file.
#
# Verification is a separate pass on purpose: the writer proves
# nothing about itself, and the only claim worth making about an
# archive is that somebody else could read it. Every frame's CRC is
# recomputed by `segfile::expand_all`, so a bit that flipped between
# the write and now is caught rather than decoded into
# plausible-looking records.

pub fn term_spans(traw: Span<u8>, raw_len: u64) -> Vec<u64> {
    var spans: Vec<u64> = Vec::new()
    if raw_len >= 4u64 {
    var rd = ByteReader::new(raw_len)
    val n = rd.take_u32(traw)
    var i: u64 = 0u64
    while i < n {
        val len = rd.take_varint(traw)
        spans.push(record::pack_span(rd.position(), len))
        rd.seek(rd.position() + len)
        i = i + 1u64
    }
    }
    spans
}

# The id of a term, or `term_none()`.
pub fn term_id_of(traw: Span<u8>, raw_len: u64, want: Span<u8>, want_len: u64) -> u64 {
    var out = term_none()
    if raw_len < 4u64 { return out }
    var rd = ByteReader::new(raw_len)
    val n = rd.take_u32(traw)
    var i: u64 = 0u64
    while i < n {
        val len = rd.take_varint(traw)
        val at = rd.position()
        if len == want_len && out == term_none() {
            var same = true
            var k: u64 = 0u64
            while k < len && same {
                val a: u8 = traw.get(at + k)
                val b: u8 = want.get(k)
                if a != b { same = false }
                k = k + 1u64
            }
            if same { out = i }
        }
        rd.seek(at + len)
        i = i + 1u64
    }
    out
}

# The links whose `from` is `from_id`.
pub fn links_of(lraw: Span<u8>, lraw_len: u64, from_id: u64,
                out_to: &mut Vec<u32>, out_count: &mut Vec<u32>) -> u64 {
    if lraw_len < 4u64 { return 0u64 }
    var rd = ByteReader::new(lraw_len)
    val groups = rd.take_u32(lraw)
    var found: u64 = 0u64
    var g: u64 = 0u64
    while g < groups {
        val from = rd.take_varint(lraw)
        val n = rd.take_varint(lraw)
        var k: u64 = 0u64
        while k < n {
            # `to` is a keyword (the `for a to b` form), so the
            # binding is `dst`.
            val dst = rd.take_varint(lraw)
            val c = rd.take_varint(lraw)
            if from == from_id {
                out_to.push(dst as u32)
                out_count.push(c as u32)
                found = found + 1u64
            }
            k = k + 1u64
        }
        g = g + 1u64
    }
    found
}

# One term's posting list, located by name.
#
# The term dictionary is walked rather than searched: it is written in
# first-seen order (§ the note in `finish`), so exact lookup is a scan
# of at most a few tens of thousands of short names -- cheap next to
# expanding a segment, which is what finding the term lets a query
# skip.
pub struct Postings {
    found: bool,
    doc_count: u64,
    at: u64,
    len: u64,
}

pub fn term_postings(traw: Span<u8>, raw_len: u64, want: Span<u8>, want_len: u64) -> Postings {
    var out = Postings { found: false, doc_count: 0u64, at: 0u64, len: 0u64 }
    if raw_len < 4u64 { return out }
    var rd = ByteReader::new(raw_len)
    val n = rd.take_u32(traw)

    var hit: u64 = n
    var i: u64 = 0u64
    while i < n {
        val len = rd.take_varint(traw)
        val at = rd.position()
        if len == want_len && hit == n {
            var same = true
            var k: u64 = 0u64
            while k < len && same {
                val a: u8 = traw.get(at + k)
                val b: u8 = want.get(k)
                if a != b { same = false }
                k = k + 1u64
            }
            if same { hit = i }
        }
        rd.seek(at + len)
        i = i + 1u64
    }

    var head_off: u64 = 0u64
    var head_len: u64 = 0u64
    var head_count: u64 = 0u64
    i = 0u64
    while i < n {
        val doc_count = rd.take_varint(traw)
        val post_off = rd.take_varint(traw)
        val post_len = rd.take_varint(traw)
        if i == hit {
            head_count = doc_count
            head_off = post_off
            head_len = post_len
        }
        i = i + 1u64
    }
    val blob_len = rd.take_varint(traw)
    val blob_at = rd.position()
    if hit < n {
        out.found = true
        out.doc_count = head_count
        out.at = blob_at + head_off
        out.len = head_len
    }
    out
}

# Decode a posting list into ascending record ordinals.
pub fn decode_postings(traw: Span<u8>, at: u64, len: u64, out: &mut Vec<u32>) {
    var rd = ByteReader::new(at + len)
    rd.seek(at)
    var prev: u64 = 0u64
    while rd.remaining() > 0u64 {
        val d = rd.take_varint(traw)
        val v = prev + d
        out.push(v as u32)
        prev = v
    }
}

# The terms whose name starts with `prefix`, with their document
# counts.
#
# `names` holds the packed (offset, length) of the part *after* the
# prefix, pointing into the caller's `.idx` window -- the value, not
# a copy of it. Answering "how many of each status" therefore reads
# the term dictionary and never touches a posting list or the arena.
pub struct TermHits {
    names: Vec<u64>,
    counts: Vec<u64>,
    scanned: u64,
}

# The distinct **keys** a dictionary holds, with the records behind
# each.
#
# `/v1/labels` asks what can be filtered on, and that is the set of
# keys -- not the tens of thousands of values under them. A key is
# everything before the first `:`; the rest may hold more of them
# (`vhost:blog.example:80`), which is why only the first counts.
#
# The dedup is a linear scan over the keys found so far. That is
# quadratic in the number of *keys*, which is a handful -- unlike the
# values, where the same shape once cost more than the scan it
# replaced.
pub fn term_keys(idx: Span<u8>, sec_off: u64, sec_len: u64) -> TermHits {
    var names: Vec<u64> = Vec::new()
    var counts: Vec<u64> = Vec::new()
    var out = TermHits { names: names, counts: counts, scanned: 0u64 }
    if sec_len > 0u64 {
    var rd = ByteReader::new(sec_off + sec_len)
    rd.seek(sec_off)
    val n = rd.take_u32(idx)
    out.scanned = n

    var spans: Vec<u64> = Vec::with_capacity(n + 1u64)
    var i: u64 = 0u64
    while i < n {
        val len = rd.take_varint(idx)
        spans.push(record::pack_span(rd.position(), len))
        rd.seek(rd.position() + len)
        i = i + 1u64
    }

    i = 0u64
    while i < n {
        val doc_count = rd.take_varint(idx)
        val post_off = rd.take_varint(idx)
        val post_len = rd.take_varint(idx)
        val sp: u64 = spans.get(i)
        val at = record::span_start(sp)
        val len = record::span_len(sp)
        var klen: u64 = 0u64
        var scanning = true
        while scanning && klen < len {
            val b: u8 = idx.get(at + klen)
            if b == 58u8 { scanning = false } else { klen = klen + 1u64 }
        }
        if klen > 0u64 && klen < len {
            var found = false
            var k: u64 = 0u64
            while k < out.names.size() && !found {
                val other: u64 = out.names.get(k)
                val oat = record::span_start(other)
                val olen = record::span_len(other)
                if olen == klen {
                    var same = true
                    var j: u64 = 0u64
                    while j < klen && same {
                        val a: u8 = idx.get(at + j)
                        val b2: u8 = idx.get(oat + j)
                        if a != b2 { same = false }
                        j = j + 1u64
                    }
                    if same {
                        val prev: u64 = out.counts.get(k)
                        out.counts.set(k, prev + doc_count)
                        found = true
                    }
                }
                k = k + 1u64
            }
            if !found {
                out.names.push(record::pack_span(at, klen))
                out.counts.push(doc_count)
            }
        }
        i = i + 1u64
    }
    }
    out
}

# One segment's stream table (kind 9), decoded.
#
# Rows are few, so this reads all of them: the caller folds them
# across segments. `ts_min > ts_max` means every record in that
# stream was undated -- the same "no span" shape the object table
# uses, so no timestamp has to stand in for "none".
pub struct StreamRows {
    texts: Vec<String>,
    counts: Vec<u64>,
    ts_min: Vec<i64>,
    ts_max: Vec<i64>,
}

impl StreamRows {
    pub fn new() -> Self {
        val t: Vec<String> = Vec::new()
        val c: Vec<u64> = Vec::new()
        val a: Vec<i64> = Vec::new()
        val b: Vec<i64> = Vec::new()
        StreamRows { texts: t, counts: c, ts_min: a, ts_max: b }
    }
    pub fn size(&self) -> u64 { self.texts.size() }
}

pub fn streams_of(sec: Span<u8>, sec_len: u64) -> StreamRows {
    var out = StreamRows::new()
    if sec_len > 0u64 {
        var rd = ByteReader::new(sec_len)
        rd.seek(0u64)
        val n = rd.take_u32(sec)
        var i: u64 = 0u64
        while i < n && rd.remaining() > 0u64 {
            val tlen = rd.take_varint(sec)
            val text = query_text_of(sec, rd.position(), tlen)
            rd.seek(rd.position() + tlen)
            val count = rd.take_varint(sec)
            val spanned = rd.take_varint(sec)
            var lo: i64 = limits::i64_max()
            var hi: i64 = limits::i64_min()
            if spanned == 1u64 {
                val first = rd.take_varint(sec) as i64
                val width = rd.take_varint(sec) as i64
                lo = first
                hi = first + width
            }
            out.texts.push(text)
            out.counts.push(count)
            out.ts_min.push(lo)
            out.ts_max.push(hi)
            i = i + 1u64
        }
    }
    out
}

# The bytes of a span as a `String`. (`query::text_of` does the same
# thing, but `archive` is below `query` in the dependency order.)
fn query_text_of(w: Span<u8>, at: u64, len: u64) -> String {
    var out = String::with_capacity(len)
    var i: u64 = 0u64
    while i < len {
        val b: u8 = w.get(at + i)
        out.push(b)
        i = i + 1u64
    }
    out
}

pub fn terms_with_prefix(idx: Span<u8>, sec_off: u64, sec_len: u64, prefix: str) -> TermHits {
    var names: Vec<u64> = Vec::new()
    var counts: Vec<u64> = Vec::new()
    var out = TermHits { names: names, counts: counts, scanned: 0u64 }
    # Single exit again: `return out` from inside the guard would be
    # a conditional move of an owned value.
    if sec_len > 0u64 {
    var rd = ByteReader::new(sec_off + sec_len)
    rd.seek(sec_off)
    val n = rd.take_u32(idx)
    out.scanned = n

    # Pass one: where each name is.
    var spans: Vec<u64> = Vec::with_capacity(n + 1u64)
    var i: u64 = 0u64
    while i < n {
        val len = rd.take_varint(idx)
        spans.push(record::pack_span(rd.position(), len))
        rd.seek(rd.position() + len)
        i = i + 1u64
    }

    # Pass two: the headers, in the same order.
    val plen = prefix.len()
    i = 0u64
    while i < n {
        val doc_count = rd.take_varint(idx)
        val post_off = rd.take_varint(idx)
        val post_len = rd.take_varint(idx)
        val sp: u64 = spans.get(i)
        val at = record::span_start(sp)
        val len = record::span_len(sp)
        if len > plen {
            if starts_with(idx, at, len, prefix) {
                out.names.push(record::pack_span(at + plen, len - plen))
                out.counts.push(doc_count)
            }
        }
        i = i + 1u64
    }
    }
    out
}

# The terms whose name starts with `prefix` and whose *value* -- the
# part after it -- contains `needle`, with where each one's postings
# live.
#
# `terms_with_prefix` answers "how many of each value" and needs only
# the names; this answers "which records" and needs the posting blob,
# which sits after the header table. Otherwise the walk is the same:
# names first, then headers in the same order.
#
# The scan is linear over the dictionary, which is what makes this
# affordable without a sorted term list (ONTOLOGY.md §4 gave up
# lexicographic order because sorting twenty thousand strings would go
# through `Vec::sort`). A few tens of thousands of short names is cheap
# next to expanding a segment, which is what a hit lets a query skip.
pub struct TermMatches {
    at: Vec<u64>,
    len: Vec<u64>,
    scanned: u64,
    hits: u64,
}

# `needle_at` / `needle_len` name a slice of `needle_w` rather than a
# span of its own, because an **empty** needle has no span: `ua~` means
# "every user agent", and `String::as_span` answers `None` for the empty
# string. Taking an offset lets the caller point into the query token it
# already holds, which is never empty.
# ONTOLOGY O1: the span of time term `id` was seen over.
#
# `found` is false when the term was only ever on undated lines --
# which is a real answer ("it is here, but nothing dates it"), not a
# missing one, so it is distinguished from the term not existing.
pub struct ObjectSpan {
    found: bool,
    first: i64,
    last: i64,
}

pub fn object_span(oraw: Span<u8>, oraw_len: u64, id: u64) -> ObjectSpan {
    var out = ObjectSpan { found: false, first: 0i64, last: 0i64 }
    if oraw_len >= 4u64 {
        var rd = ByteReader::new(oraw_len)
        val n = rd.take_u32(oraw)
        if id < n {
            var i: u64 = 0u64
            while i <= id {
                val has = rd.take_varint(oraw)
                if has == 1u64 {
                    val f = rd.take_varint(oraw)
                    val span = rd.take_varint(oraw)
                    if i == id {
                        out.found = true
                        out.first = f as i64
                        out.last = (f + span) as i64
                    }
                }
                i = i + 1u64
            }
        }
    }
    out
}

pub fn terms_matching(idx: Span<u8>, sec_off: u64, sec_len: u64,
                      prefix: str, needle_w: Span<u8>,
                      needle_at: u64, needle_len: u64,
                      anchored: bool) -> TermMatches {
    var ats: Vec<u64> = Vec::new()
    var lens: Vec<u64> = Vec::new()
    var out = TermMatches { at: ats, len: lens, scanned: 0u64, hits: 0u64 }
    if sec_len > 0u64 {
    var rd = ByteReader::new(sec_off + sec_len)
    rd.seek(sec_off)
    val n = rd.take_u32(idx)
    out.scanned = n

    # Pass one: where each name is.
    var spans: Vec<u64> = Vec::with_capacity(n + 1u64)
    var i: u64 = 0u64
    while i < n {
        val len = rd.take_varint(idx)
        spans.push(record::pack_span(rd.position(), len))
        rd.seek(rd.position() + len)
        i = i + 1u64
    }

    # Pass two: the headers, in the same order. The offsets are
    # relative to the blob, whose start is only known after the whole
    # table, so they are kept and fixed up below.
    var rel: Vec<u64> = Vec::new()
    val plen = prefix.len()
    i = 0u64
    while i < n {
        val doc_count = rd.take_varint(idx)
        val post_off = rd.take_varint(idx)
        val post_len = rd.take_varint(idx)
        val sp: u64 = spans.get(i)
        val at = record::span_start(sp)
        val len = record::span_len(sp)
        if len > plen {
            if starts_with(idx, at, len, prefix) {
                if value_matches(idx, at + plen, len - plen, needle_w, needle_at, needle_len, anchored) {
                    rel.push(post_off)
                    out.len.push(post_len)
                }
            }
        }
        i = i + 1u64
    }
    val blob_len = rd.take_varint(idx)
    val blob_at = rd.position()
    var k: u64 = 0u64
    while k < rel.size() {
        val off: u64 = rel.get(k)
        out.at.push(blob_at + off)
        k = k + 1u64
    }
    out.hits = out.at.size()
    }
    out
}

# Whether the value -- the `len` bytes at `at` -- matches the needle.
# `anchored` is the difference between `path^/wp-` and `path~/wp-`, and
# on real traffic it is a large one: scanners nest these paths, so
# `/wp-` appears 11,008 times in a path but starts only 9,251 of them,
# and `/.env` starts 3,027 of the 7,860 it appears in. "The site's own
# `.env`" and "somebody probing for one" are different questions.
#
# An empty needle matches either way, which is what makes `ua~` mean
# "every user agent" rather than nothing.
fn value_matches(w: Span<u8>, at: u64, len: u64,
                 needle_w: Span<u8>, needle_at: u64, needle_len: u64,
                 anchored: bool) -> bool {
    if needle_len == 0u64 { return true }
    if needle_len > len { return false }
    var last = len - needle_len
    if anchored { last = 0u64 }
    var i: u64 = 0u64
    while i <= last {
        var same = true
        var k: u64 = 0u64
        while k < needle_len && same {
            val a: u8 = w.get(at + i + k)
            val b: u8 = needle_w.get(needle_at + k)
            if a != b { same = false }
            k = k + 1u64
        }
        if same { return true }
        i = i + 1u64
    }
    false
}

unsafe fn starts_with(w: Span<u8>, at: u64, len: u64, prefix: str) -> bool {
    val n = prefix.len()
    if n > len { return false }
    val p = __builtin_str_to_ptr(prefix)
    var i: u64 = 0u64
    while i < n {
        val a: u8 = w.get(at + i)
        val b: u8 = __builtin_ptr_read::<u8>(p, i)
        if a != b { return false }
        i = i + 1u64
    }
    true
}

pub struct VerifyReport {
    ok: bool,
    frames: u64,
    bad_frames: u64,
    records: u64,
    raw_bytes: u64,
    seg_bytes: u64,
    index_bytes: u64,
}

# Read a segment back and check every claim it makes about itself.
#
# `base` is the path without the extension, as it was handed to
# `finish`. The pass costs one open and one sequential walk of the
# file: frames are expanded frame by frame (each CRC checked on the
# way), the record table is walked to prove it decodes into the
# number of records the header claims, and the two compressed
# sections are decompressed to prove they still can be.
pub fn verify(base: str, crc: &Crc32) -> Result<VerifyReport, IoError> {
    val stem = String::from_str(base)
    val ext = String::from_str(".seg")
    var path = stem.concat(&ext)

    var report = VerifyReport {
        ok: false, frames: 0u64, bad_frames: 0u64, records: 0u64,
        raw_bytes: 0u64, seg_bytes: 0u64, index_bytes: 0u64,
    }
    report.seg_bytes = fs::file_size(path.to_str())?

    val f = File::open(path.to_str())?
    var head = ByteWriter::with_capacity(segfile::data_at() + 64u64)
    var raw = ByteWriter::with_capacity(frame_raw_bytes() + 65536u64)
    var arena = ByteWriter::with_capacity(segment_target_bytes() + 65536u64)
    var sec = ByteWriter::with_capacity(1048576u64)

    val h = segfile::head_of(&f, &mut head)
    if !h.ok {
        report.ok = false
    } else {
        report.ok = true
        report.records = h.records
        report.frames = h.n_frames
        report.index_bytes = h.recs_len + h.ftab_len + h.terms_len + h.links_len

        if !segfile::expand_all(&f, &h, crc, &mut raw, &mut arena) {
            report.bad_frames = report.bad_frames + 1u64
            report.ok = false
        }
        report.raw_bytes = arena.len()
        if arena.len() != h.arena_bytes { report.ok = false }

        # The record table is walked as well as read: a
        # checksum says the bytes arrived, not that they
        # decode into the number of records claimed.
        if !segfile::read_range(&f, h.recs_off, h.recs_len, &mut sec) {
            report.ok = false
        } else {
            val rw = sec.span()
            match rw {
                Option::Some(rb) => {
                    if crc.of(rb, 0u64, h.recs_len) != h.recs_crc { report.ok = false }
                    var walk = ByteReader::new(h.recs_len)
                    var seen: u64 = 0u64
                    while walk.remaining() > 0u64 {
                        var k: u64 = 0u64
                        while k < 11u64 {
                            val v = walk.take_varint(rb)
                            k = k + 1u64
                        }
                        seen = seen + 1u64
                    }
                    if seen != h.records { report.ok = false }
                }
                Option::None => { report.ok = false }
            }
        }

        if h.has_terms() {
            if !segfile::load_block(&f, h.terms_off, h.terms_len, crc, &mut raw, &mut sec) {
                report.ok = false
            }
        }
        if h.has_links() {
            if !segfile::load_block(&f, h.links_off, h.links_len, crc, &mut raw, &mut sec) {
                report.ok = false
            }
        }
    }

    Result::Ok(report)
}
