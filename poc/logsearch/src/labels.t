# The label dictionary: which label keys a mount holds, which values
# they take, and how many records carry each.
#
# `GET /v1/labels` used to answer by opening every segment's term
# section (24 ms over four segments holding 43,813 terms — fine there,
# not at a hundred thousand). HTTP_API.md section 2 asks for the
# answer to come out of the catalog instead, so that a question about
# *what can be filtered on* never touches `seg/`.
#
# **It is a cache, like the catalog**, and it keeps the catalog's
# habits: one file per generation under `meta/`, written to `tmp/` and
# renamed into place, and every failure answered the same way — take
# what checks out, drop the rest, rebuild when asked.
#
#   meta/labels.dict
#
# One file, not one per generation: a generation exists so that the
# catalog's snapshot and its log can be replaced as a pair, and a
# dictionary is a single file a rename replaces on its own. The
# header still records the catalog generation it was last written
# beside, which is what a reader compares when it wants to know
# whether the two have drifted.
#
# The rows are not fixed-width (a value is a string), so this cannot
# live in `catalog.<gen>.snap`, whose 64-byte rows are what lets a
# reader check the count against the file size before decoding
# anything. A second file with its own magic and CRC costs one
# `open` at load and keeps both formats simple.
#
# **Kept up to date one segment at a time.** Writing a segment merges
# that segment's terms in; retention subtracts them before the file
# goes. Neither is a walk of the mount — the cost is proportional to
# what changed, which is what makes this cheaper than the query it
# replaces. `repair` rebuilds from `seg/` like everything else here.

pub fn dict_head_bytes() -> u64 { 24u64 }

# One (key, value) pair and the records carrying it. A row whose
# value is empty is the key's own total, so both forms of
# `/v1/labels` come out of one table.
pub struct LabelDict {
    keys: Vec<String>,
    values: Vec<String>,
    counts: Vec<u64>,
}

impl LabelDict {
    pub fn new() -> Self {
        var k: Vec<String> = Vec::new()
        var v: Vec<String> = Vec::new()
        var c: Vec<u64> = Vec::new()
        LabelDict { keys: k, values: v, counts: c }
    }

    pub fn size(&self) -> u64 { self.keys.size() }

    pub fn key_at(&self, i: u64) -> String {
        val k: &String = self.keys.borrow(i)
        val out = k.clone()
        out
    }

    pub fn value_at(&self, i: u64) -> String {
        val v: &String = self.values.borrow(i)
        val out = v.clone()
        out
    }

    pub fn count_at(&self, i: u64) -> u64 { self.counts.get(i) }

    # The row for this pair, or -1. Linear: a mount has tens of keys
    # and thousands of values, and the merge below is the only hot
    # caller (once per segment written).
    pub fn find(&self, key: &String, value: &String) -> i64 {
        var i: u64 = 0u64
        var out: i64 = -1i64
        while i < self.keys.size() && out < 0i64 {
            val k: &String = self.keys.borrow(i)
            val v: &String = self.values.borrow(i)
            if k.eq(key) && v.eq(value) { out = i as i64 }
            i = i + 1u64
        }
        out
    }

    # Move a row's count by `by`, creating the row when it is new.
    # A row that reaches zero is dropped: a value no segment carries
    # any more is not a value the mount has.
    pub fn bump(&mut self, key: &String, value: &String, by: i64) {
        val at = self.find(key, value)
        if at < 0i64 {
            if by <= 0i64 { return }
            val k = key.clone()
            val v = value.clone()
            self.keys.push(k)
            self.values.push(v)
            self.counts.push(by as u64)
            return
        }
        val i = at as u64
        val have: u64 = self.counts.get(i)
        var next: i64 = (have as i64) + by
        if next < 0i64 { next = 0i64 }
        if next == 0i64 {
            # The removed strings are values this scope now owns, so
            # they are bound rather than dropped on the floor — a
            # compound-returning method cannot stand as a statement
            # on the compiled lanes anyway.
            val gone_key: String = self.keys.remove(i)
            val gone_value: String = self.values.remove(i)
            val gone_count: u64 = self.counts.remove(i)
            return
        }
        self.counts.set(i, next as u64)
    }
}

pub fn dict_path(mount: str) -> String {
    val meta = catalog::meta_path(mount)
    val out = String::from_str("{meta.to_str()}/labels.dict")
    out
}

# ---------------------------------------------------------------------
# The file

# `key` and `value` are each a length byte plus the bytes; a count is
# a u64. Keys are `[a-z0-9_]` up to 32 bytes (`query::is_index_key`)
# and a value is capped at 255 here — a longer one is not a label
# anybody filters on, and the cap is what keeps the length field one
# byte.
pub fn encode_dict(d: &LabelDict, gen: u64, w: &mut ByteWriter, crc: &Crc32) {
    w.clear()
    w.put_magic("LSD1")
    w.put_u32(gen)
    w.put_u64(d.size())
    w.put_u32(0u64)
    w.put_u32(0u64)
    var i: u64 = 0u64
    while i < d.size() {
        val k = d.key_at(i)
        val v = d.value_at(i)
        put_short(w, &k)
        put_short(w, &v)
        w.put_u64(d.count_at(i))
        i = i + 1u64
    }
    var sum: u64 = 0u64
    val sp = w.span()
    match sp {
        Option::Some(b) => {
            val n = w.len() - dict_head_bytes()
            if n > 0u64 { sum = crc.of(b, dict_head_bytes(), n) }
        }
        Option::None => { }
    }
    w.patch_u32(16u64, sum)
}

fn put_short(w: &mut ByteWriter, s: &String) {
    var n = s.len()
    if n > 255u64 { n = 255u64 }
    w.put_u8(n as u8)
    var i: u64 = 0u64
    while i < n {
        w.put_u8(s.get(i))
        i = i + 1u64
    }
}

fn take_short(rd: &mut ByteReader, b: Span<u8>) -> String {
    val n = rd.take_u8(b) as u64
    var out = String::new()
    var i: u64 = 0u64
    while i < n {
        out.push(rd.take_u8(b))
        i = i + 1u64
    }
    out
}

# The generation the file names, or 0 when it is not one this reader
# understands. A bad CRC is the same answer as a bad magic: the
# dictionary is rebuildable, so there is nothing to salvage.
pub fn decode_dict(b: Span<u8>, len: u64, d: &mut LabelDict, crc: &Crc32) -> u64 {
    if len < dict_head_bytes() { return 0u64 }
    var rd = ByteReader::new(len)
    if !rd.take_magic(b, "LSD1") { return 0u64 }
    val gen = rd.take_u32(b)
    val rows = rd.take_u64(b)
    val want = rd.take_u32(b)
    val reserved = rd.take_u32(b)

    val body = len - dict_head_bytes()
    var got: u64 = 0u64
    if body > 0u64 { got = crc.of(b, dict_head_bytes(), body) }
    if got != want { return 0u64 }

    var i: u64 = 0u64
    while i < rows {
        val k = take_short(&mut rd, b)
        val v = take_short(&mut rd, b)
        val c = rd.take_u64(b)
        # A row is written only while it has a count (`bump` drops
        # one that reaches zero), so there is nothing to filter here.
        d.keys.push(k)
        d.values.push(v)
        d.counts.push(c)
        i = i + 1u64
    }
    gen
}

# ---------------------------------------------------------------------
# Keeping it current

# Merge one segment's label terms into `d`, or take them out again
# (`add = false`, which retention does before the file goes).
#
# One pass over the segment's term section. A label term is spelled
# `key:value`; a bare word has no colon and is not a label.
pub fn merge_segment(d: &mut LabelDict, seg: &String, add: bool, crc: &Crc32) {
    var segs: Vec<String> = Vec::new()
    val copy = seg.clone()
    segs.push(copy)
    val tal = query::tally(&segs, "", false, crc)
    var sign: i64 = 1i64
    if !add { sign = -1i64 }
    var i: u64 = 0u64
    while i < tal.size() {
        val term: &String = tal.names.borrow(i)
        val at = colon_in(term)
        if at > 0u64 {
            val key = substring_of(term, 0u64, at)
            val value = substring_of(term, at + 1u64, term.len())
            val n: u64 = tal.counts.get(i)
            val by = (n as i64) * sign
            d.bump(&key, &value, by)
            # The key's own row, so `/v1/labels` with no `name` is
            # one scan of the same table.
            val empty = String::new()
            d.bump(&key, &empty, by)
        }
        i = i + 1u64
    }
}

# Where the first `:` is, or 0 when there is none (a term cannot
# begin with one, so 0 is free to mean "no key").
fn colon_in(s: &String) -> u64 {
    var i: u64 = 0u64
    var at: u64 = 0u64
    while i < s.len() && at == 0u64 {
        if s.get(i) == 58u8 { at = i }
        i = i + 1u64
    }
    at
}

fn substring_of(s: &String, from: u64, until: u64) -> String {
    var out = String::new()
    var i = from
    while i < until && i < s.len() {
        out.push(s.get(i))
        i = i + 1u64
    }
    out
}

# Build one from scratch by walking every segment. What `repair`
# does, and what a mount with no dictionary file falls back to.
pub fn rebuild_dict(segs: &Vec<String>, crc: &Crc32) -> LabelDict {
    var d = LabelDict::new()
    var i: u64 = 0u64
    while i < segs.size() {
        val one: &String = segs.borrow(i)
        merge_segment(&mut d, one, true, crc)
        i = i + 1u64
    }
    d
}

# ---------------------------------------------------------------------
# The mount

# Is there a dictionary at all? The difference between "no dictionary
# yet" and "a mount with no labels" decides whether a segment can be
# merged in or the whole thing has to be built first.
pub fn has_dict(mount: str) -> bool {
    val path = dict_path(mount)
    io::file_exists(path.to_str())
}

# Read the dictionary for `gen`. An empty one when there is no file,
# or when the file does not check out.
pub fn load_dict(mount: str, crc: &Crc32) -> LabelDict {
    var d = LabelDict::new()
    val path = dict_path(mount)
    val opened = File::open(path.to_str())
    match opened {
        Result::Ok(f) => {
            var size: u64 = 0u64
            val sz = f.size()
            match sz {
                Result::Ok(k) => { size = k }
                Result::Err(e) => { }
            }
            if size > 0u64 {
                var buf = ByteWriter::with_capacity(size + 16u64)
                buf.reserve(size)
                val room = buf.room()
                match room {
                    Option::Some(win) => {
                        val part = win.slice(0u64, size)
                        val got = f.read_at(0u64, part)
                        match got {
                            Result::Ok(n) => {
                                if n == size {
                                    buf.set_len(size)
                                    val sp = buf.span()
                                    match sp {
                                        Option::Some(all) => {
                                            val g = decode_dict(all, size, &mut d, crc)
                                        }
                                        Option::None => { }
                                    }
                                }
                            }
                            Result::Err(e) => { }
                        }
                    }
                    Option::None => { }
                }
            }
        }
        Result::Err(e) => { }
    }
    d
}

# Write the dictionary for `gen`, through `tmp/` and a rename. False
# when it could not be written — the caller carries on, because the
# next `/v1/labels` falls back to the segments and `repair` puts it
# right.
pub fn save_dict(mount: str, gen: u64, d: &LabelDict, crc: &Crc32) -> bool {
    var w = ByteWriter::with_capacity(4096u64)
    encode_dict(d, gen, &mut w, crc)
    val tmp = catalog::tmp_path(mount)
    val staged = String::from_str("{tmp.to_str()}/labels.dict")
    val final_path = dict_path(mount)
    var ok = false
    val text = w.span()
    match text {
        Option::Some(all) => {
            val made = File::create(staged.to_str())
            match made {
                Result::Ok(f) => {
                    val part = all.slice(0u64, w.len())
                    val put = f.write(part)
                    match put {
                        Result::Ok(n) => { ok = n == w.len() }
                        Result::Err(e) => { }
                    }
                    val synced = f.sync()
                    match synced {
                        Result::Ok(u) => { }
                        Result::Err(e) => { }
                    }
                }
                Result::Err(e) => { }
            }
        }
        Option::None => { }
    }
    if !ok { return false }
    val moved = fs::rename(staged.to_str(), final_path.to_str())
    match moved {
        Result::Ok(u) => { true }
        Result::Err(e) => { false }
    }
}

# ---------------------------------------------------------------------
# What the callers use

# Every segment the catalog lists for this mount, merged in from
# nothing. What `repair` does, and what the first write to a mount
# with no dictionary does — starting from empty and adding only the
# new segment would answer for that segment alone.
pub fn rebuild_for_mount(mount: str, crc: &Crc32) -> LabelDict {
    val c = catalog::load(mount, crc)
    var d = LabelDict::new()
    var i: u64 = 0u64
    while i < c.size() {
        val r: CatRow = c.row(i)
        val sp = catalog::seg_path(mount, &r)
        merge_segment(&mut d, &sp, true, crc)
        i = i + 1u64
    }
    d
}

# Fold one freshly written segment in. `gen` is only recorded in the
# header, so a reader can see which catalog generation the dictionary
# was last written beside.
pub fn note_segment(mount: str, seg: &String, gen: u64, crc: &Crc32) -> bool {
    if !has_dict(mount) {
        val fresh = rebuild_for_mount(mount, crc)
        return save_dict(mount, gen, &fresh, crc)
    }
    var d = load_dict(mount, crc)
    merge_segment(&mut d, seg, true, crc)
    save_dict(mount, gen, &d, crc)
}

# Take one segment's labels back out, before the file goes. Nothing
# to do when there is no dictionary — the next write builds one that
# does not include this segment anyway.
pub fn forget_segment(mount: str, seg: &String, gen: u64, crc: &Crc32) -> bool {
    if !has_dict(mount) { return false }
    var d = load_dict(mount, crc)
    merge_segment(&mut d, seg, false, crc)
    save_dict(mount, gen, &d, crc)
}

# Throw the dictionary away and build it again from `seg/`.
pub fn repair(mount: str, gen: u64, crc: &Crc32) -> bool {
    val d = rebuild_for_mount(mount, crc)
    save_dict(mount, gen, &d, crc)
}
