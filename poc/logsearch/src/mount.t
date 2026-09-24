# Mounts: the directories this service is allowed to write to.
#
# **They are declared, never discovered** (DATA_MODEL.md section 6).
# `fs::list_dir` exists, so finding candidate directories would be
# easy; writing to a directory nobody named is the part that is not
# acceptable. The configuration file is the whole list.
#
#   # logsearch.conf
#   mount /var/log/logsearch/a  quota=100G
#   mount /mnt/disk2/logsearch  quota=400G
#   mount /mnt/disk3/logsearch  quota=400G  readonly
#
# A mount is self-contained: its `meta/` holds its own catalog, so a
# directory copied to another host still reads. Nothing binds the
# mounts together except this file and the process holding it, which
# is what makes losing one survivable.
#
# Placement picks the **least-used share**, not round robin, because
# mixing a 100G disk with a 400G one is the normal case and
# alternating between them fills the small one first. `used` is the
# sum of this mount's own catalog -- not `df` -- because there is no
# `statfs` (RUNTIME_GAPS.md G4). Another process eating the same disk
# is invisible here, and saying so is better than implying otherwise.

import std.fs
import std.io
import std.json
import std.parse
import std.time
import catalog
import logdir

# What a mount can take. Held in memory only, so no numbers.
pub enum MountState {
    Active,
    Full,
    Degraded,
}

# The format number written into `meta/mount.json`. It is the segment
# format's, because that is what a reader has to understand.
pub const META_FORMAT: u64 = 3u64

pub fn state_name(s: MountState) -> str {
    match s {
        MountState::Active => "active",
        MountState::Full => "full",
        MountState::Degraded => "degraded",
    }
}

# ---------------------------------------------------------------------
# Sizes

# `100G` -> bytes. Powers of 1024, because the number next to a disk
# is written that way.
pub fn parse_size(s: &String) -> Option<u64> {
    val n = s.len()
    if n == 0u64 { return Option::None }
    val last: u8 = s.get(n - 1u64)
    val mul: u64 = match last {
        'K' | 'k' => 1024u64,
        'M' | 'm' => 1048576u64,
        'G' | 'g' => 1073741824u64,
        'T' | 't' => 1099511627776u64,
        _ => 1u64,
    }
    # A suffix was read exactly when it multiplied.
    val end = if mul == 1u64 { n } else { n - 1u64 }
    if end == 0u64 { return Option::None }
    val digits = s.substring(0u64, end)
    val got = parse::to_u64(digits.to_str())
    match got {
        Result::Ok(v) => { Option::Some(v * mul) }
        Result::Err(e) => { Option::None }
    }
}

# ---------------------------------------------------------------------

# The declared mounts, as columns rather than a `Vec` of structs.
#
# A struct holding a `String` is an owning value, and reading one out
# of a container hands back an alias that the caller's drop glue then
# frees (CLAUDE.md, the STRING-NO-DROP pitfall). Columns sidestep the
# whole question: the only owning column is `paths`, and it is read
# through `path_of`, which clones.
pub struct MountSet {
    paths: Vec<String>,
    quotas: Vec<u64>,
    readonly: Vec<u64>,
    states: Vec<MountState>,
    used: Vec<u64>,
}

impl MountSet {
    pub fn new() -> Self {
        var paths: Vec<String> = Vec::new()
        var quotas: Vec<u64> = Vec::new()
        var readonly: Vec<u64> = Vec::new()
        var states: Vec<MountState> = Vec::new()
        var used: Vec<u64> = Vec::new()
        val out = MountSet {
            paths, quotas, readonly,
            states, used,
        }
        out
    }

    pub fn size(&self) -> u64 { self.paths.size() }
    pub fn is_empty(&self) -> bool { self.paths.size() == 0u64 }

    pub fn add(&mut self, path: &String, quota: u64, ro: bool) {
        val p = path.clone()
        self.paths.push(p)
        self.quotas.push(quota)
        val flag = if ro { 1u64 } else { 0u64 }
        self.readonly.push(flag)
        self.states.push(MountState::Active)
        self.used.push(0u64)
    }

    # A copy, not the stored one: handing out the container's own
    # `String` would let the caller's drop glue free it.
    pub fn path_of(&self, i: u64) -> String {
        val p: &String = self.paths.borrow(i)
        val c = p.clone()
        c
    }

    pub fn quota_of(&self, i: u64) -> u64 { self.quotas.get(i) }
    pub fn used_of(&self, i: u64) -> u64 { self.used.get(i) }
    pub fn state_of(&self, i: u64) -> MountState { self.states.get(i) }
    pub fn is_readonly(&self, i: u64) -> bool { self.readonly.get(i) == 1u64 }

    pub fn set_used(&mut self, i: u64, bytes: u64) {
        self.used.set(i, bytes)
        if bytes >= self.quotas.get(i) {
            if val MountState::Active = self.states.get(i) {
                self.states.set(i, MountState::Full)
            }
        }
    }

    pub fn mark(&mut self, i: u64, state: MountState) { self.states.set(i, state) }

    # Usage in thousandths. Thousandths rather than the ratio itself
    # because there are no fractions here, and rather than
    # `used_a * quota_b < used_b * quota_a` because that product
    # overflows well inside the sizes this is for (400G squared does
    # not fit in a u64).
    pub fn permille(&self, i: u64) -> u64 {
        val q = self.quotas.get(i)
        if q == 0u64 { return 1000u64 }
        val u = self.used.get(i)
        if u >= q { return 1000u64 }
        (u * 1000u64) / q
    }

    # Where the next segment goes: the least-used writable mount, ties
    # going to the one declared first.
    pub fn pick(&self) -> Option<u64> {
        var best: u64 = 0u64
        var found = false
        var best_share: u64 = 0u64
        var i: u64 = 0u64
        while i < self.paths.size() {
            var usable = match self.states.get(i) { MountState::Active => true, _ => false }
            if self.readonly.get(i) == 1u64 { usable = false }
            if usable {
                val share = self.permille(i)
                if !found || share < best_share {
                    best = i
                    best_share = share
                    found = true
                }
            }
            i = i + 1u64
        }
        if !found { return Option::None }
        Option::Some(best)
    }

    # Re-read every mount's own catalog and total what it holds.
    #
    # A mount with no catalog is not an empty mount: it is one whose
    # cache is missing, so the segments get counted by walking `seg/`
    # (which also leaves a catalog worth keeping -- the caller decides
    # whether to publish it).
    pub fn refresh_used(&mut self, crc: &Crc32) {
        var i: u64 = 0u64
        while i < self.paths.size() {
            val p = self.path_of(i)
            val ps = p.to_str()
            var total: u64 = 0u64
            val c = catalog::load(ps, crc)
            if c.generation() > 0u64 {
                total = c.total_bytes()
            } else {
                val built = catalog::rebuild(ps, crc)
                total = built.total_bytes()
            }
            self.set_used(i, total)
            i = i + 1u64
        }
    }
}

# ---------------------------------------------------------------------
# The configuration file

fn tokens_of(line: &String, out: &mut Vec<String>) {
    out.clear()
    val n = line.len()
    var i: u64 = 0u64
    while i < n {
        val c: u8 = line.get(i)
        if c == ' ' || c == '\t' || c == '\r' {
            i = i + 1u64
        } else {
            var j = i + 1u64
            while j < n {
                val d: u8 = line.get(j)
                if d == ' ' || d == '\t' || d == '\r' { break }
                j = j + 1u64
            }
            val t = line.substring(i, j)
            out.push(t)
            i = j
        }
    }
}

# Read one `mount` line into `out`. Returns false and leaves `out`
# alone if the line does not say something this understands --
# **a line that is not understood is not half-applied**, because a
# mount with the wrong quota is worse than a mount that is missing.
fn apply_line(line: &String, out: &mut MountSet, toks: &mut Vec<String>) -> bool {
    tokens_of(line, toks)
    if toks.size() == 0u64 { return true }

    val head: &String = toks.borrow(0u64)
    val hash: u8 = head.get(0u64)
    if hash == '#' { return true }

    val want = String::from_str("mount")
    if !head.eq(&want) { return false }
    if toks.size() < 2u64 { return false }

    val path: &String = toks.borrow(1u64)
    var quota: u64 = 0u64
    var ro = false
    var ok = true

    var i: u64 = 2u64
    while i < toks.size() {
        val t: &String = toks.borrow(i)
        val qkey = String::from_str("quota=")
        val rokey = String::from_str("readonly")
        if t.starts_with(&qkey) {
            val v = t.substring(6u64, t.len())
            val got = parse_size(&v)
            match got {
                Option::Some(n) => { quota = n }
                Option::None => { ok = false }
            }
        } elif t.eq(&rokey) {
            ro = true
        } else {
            ok = false
        }
        i = i + 1u64
    }
    # A quota of zero would divide by zero in `permille` and, worse,
    # would read as "no limit" to whoever wrote the line.
    if quota == 0u64 { ok = false }
    if ok { out.add(&path, quota, ro) }
    ok
}

# Parse a whole configuration, and say how many lines were not
# understood. Everything that parsed is in `out`.
pub fn parse_config(text: &String, out: &mut MountSet) -> u64 {
    var bad: u64 = 0u64
    var toks: Vec<String> = Vec::new()
    val n = text.len()
    var start: u64 = 0u64
    var i: u64 = 0u64
    while i <= n {
        var cut = i == n
        if i < n {
            val c: u8 = text.get(i)
            if c == '\n' { cut = true }
        }
        if cut {
            if i > start {
                val line = text.substring(start, i)
                if !apply_line(&line, out, &mut toks) { bad = bad + 1u64 }
            }
            start = i + 1u64
        }
        i = i + 1u64
    }
    bad
}

# Read `path` and parse it. A missing file is an error, not an empty
# set: a service with no mounts has nowhere to put anything, and
# saying "0 mounts configured" for a typo in a path would be the
# wrong diagnosis.
pub fn load_config(path: str, out: &mut MountSet) -> Result<u64, IoError> {
    val raw = io::read_file(path)?
    val text = String::from_str(raw)
    val bad = parse_config(&text, out)
    Result::Ok(bad)
}

# ---------------------------------------------------------------------
# `meta/mount.json`

# What identifies a mount directory. `ok` is false when the file is
# absent or unreadable, which is different from a mismatch.
pub struct MountMeta {
    ok: bool,
    format: u64,
    created_unix_ns: i64,
    uuid: String,
}

impl MountMeta {
    pub fn empty() -> Self {
        val u = String::new()
        val m = MountMeta {
            ok: false, format: 0u64, created_unix_ns: 0i64, uuid: u,
        }
        m
    }
}

pub fn meta_file(mount: str) -> String {
    val s = "{mount}/meta/mount.json"
    val out = String::from_str(s)
    out
}

# 32 hex digits from the runtime's generator. Not an RFC 4122 UUID:
# it is an identifier that has to be hard to collide with, and the
# variant bits would say something about its construction that is not
# true.
pub fn fresh_uuid() -> String {
    var out = String::new()
    var k: u64 = 0u64
    while k < 4u64 {
        val v = io::random()
        val piece = "{v:016x}"
        out.push_str(piece)
        k = k + 1u64
    }
    val cut = out.substring(0u64, 32u64)
    cut
}

pub fn read_meta(mount: str) -> MountMeta {
    # The identity is built where it is known, and returned from
    # there. Filling a `String` field afterwards is not available in
    # the compiled lanes (neither a whole-struct field assignment nor
    # a move into an existing binding), and this shape does not want
    # either: a `MountMeta` is only ever complete or absent.
    val p = meta_file(mount)
    val text = io::read_file(p.to_str())
    match text {
        Result::Ok(body) => {
            val doc = json::parse(body)
            match doc {
                Result::Ok(j) => {
                    val root = j.root()
                    var fmt: u64 = 0u64
                    var born: i64 = 0i64
                    val f = j.get(root, "format")
                    match f {
                        Option::Some(id) => { fmt = j.as_int(id) as u64 }
                        Option::None => { }
                    }
                    val b = j.get(root, "created_unix_ns")
                    match b {
                        Option::Some(id2) => { born = j.as_int(id2) }
                        Option::None => { }
                    }
                    val u = j.get(root, "uuid")
                    match u {
                        Option::Some(id3) => {
                            val txt = j.as_text(id3)
                            val su = String::from_str(txt)
                            val m = MountMeta {
                                ok: true, format: fmt,
                                created_unix_ns: born, uuid: su,
                            }
                            return m
                        }
                        Option::None => { }
                    }
                }
                Result::Err(e) => { }
            }
        }
        Result::Err(e) => { }
    }
    var absent = MountMeta::empty()
    absent
}

pub fn write_meta(mount: str, uuid: &String, note: str) -> bool {
    val dir = "{mount}/meta"
    val made = fs::mkdir_all(dir)
    match made {
        Result::Ok(u) => { }
        Result::Err(e) => { return false }
    }
    var w = JsonWriter::new()
    w.begin_object()
    w.key("format")
    w.u64_value(META_FORMAT)
    w.key("uuid")
    w.str_value(uuid.to_str())
    w.key("created_unix_ns")
    w.i64_value(time::now_unix_ns())
    w.key("created_by")
    w.str_value("logsearchd 0.1")
    w.key("note")
    w.str_value(note)
    w.end_object()
    val body = w.finish()
    val p = meta_file(mount)
    val wrote = io::write_file(p.to_str(), body.to_str())
    match wrote {
        Result::Ok(n) => { true }
        Result::Err(e) => { false }
    }
}

# Read the mount's identity, writing one if the directory does not
# have one yet. A directory that answers with a *different* identity
# than the caller remembers is the expensive mistake this guards --
# see `identity_matches`.
pub fn ensure_meta(mount: str, note: str) -> MountMeta {
    val existing = read_meta(mount)
    if existing.ok { return existing }
    val u = fresh_uuid()
    if !write_meta(mount, &u, note) {
        var bad = MountMeta::empty()
        return bad
    }
    val again = read_meta(mount)
    again
}

# Whether this directory is still the one that was seen before.
#
# Overwriting the wrong mount is the costliest accident available
# here (DATA_MODEL.md section 6), so the answer to "I do not know" is
# **no**: an unreadable `mount.json` does not pass.
pub fn identity_matches(m: &MountMeta, remembered: &String) -> bool {
    if !m.ok { return false }
    if remembered.len() == 0u64 { return false }
    m.uuid.eq(remembered)
}

# ---------------------------------------------------------------------
# What a `<spec>` on a command line or in a request means

# Open the mounts a spec names: a configuration file (`*.conf`) or a
# single directory used as one mount.
#
# The single-directory form is what every command took before mounts
# existed, and it is still what the tests and the examples use. The
# quota it gets is a placeholder rather than a policy -- naming a
# directory says nothing about how much of the disk this service may
# have, and a real limit is declared in a `.conf`.
pub fn open_spec(spec: str, ms: &mut MountSet) -> bool {
    val s = String::from_str(spec)
    val conf = String::from_str(".conf")
    if s.ends_with(&conf) {
        val got = load_config(spec, ms)
        match got {
            Result::Ok(bad) => {
                if bad > 0u64 {
                    println("  {bad} line(s) of {spec} were not understood")
                }
            }
            Result::Err(e) => {
                println("cannot read {spec}: {e}")
                return false
            }
        }
        if ms.is_empty() {
            println("{spec} declares no mounts")
            return false
        }
        return true
    }
    val one = String::from_str(spec)
    ms.add(&one, DEFAULT_QUOTA, false)
    true
}

# The placeholder a bare directory gets. 1 TiB, which is a number
# chosen to stay out of the way rather than to mean anything.
pub const DEFAULT_QUOTA: u64 = 1099511627776u64

# How many of the declared mounts are directories this process can
# see.
#
# A path that is simply not there is a typo, and answering "no
# readable mount" for it is right; a directory that exists and holds
# nothing is a different thing and must not borrow that answer.
pub fn readable(ms: &MountSet) -> u64 {
    var n: u64 = 0u64
    var i: u64 = 0u64
    while i < ms.size() {
        val p = ms.path_of(i)
        if fs::is_dir(p.to_str()) { n = n + 1u64 }
        i = i + 1u64
    }
    n
}

# Every segment a spec covers, in a stable order.
#
# **The catalog answers, and the directory answers when the catalog
# cannot.** Falling back rather than failing is what keeps the
# catalog a cache: an archive written before catalogs existed still
# reads, and so does one whose `meta/` was deleted.
pub fn segments_of(spec: str, out: &mut Vec<String>) {
    var ms = MountSet::new()
    if !open_spec(spec, &mut ms) {
        out.clear()
        return
    }
    segments_in(&ms, out)
}

# The same, over mounts that are already open.
#
# Split out because **"no mount to read" and "no segments yet" are
# different answers** and a caller that collapses them sends someone
# to check disk permissions when the truth is that nothing has been
# archived. `segments_of` cannot tell them apart -- both come back as
# an empty list -- so a caller that has to distinguish opens the
# mounts itself and calls this.
pub fn segments_in(ms: &MountSet, out: &mut Vec<String>) {
    out.clear()
    val crc = Crc32::new()
    var i: u64 = 0u64
    while i < ms.size() {
        val p = ms.path_of(i)
        val ps = p.to_str()
        val c = catalog::load(ps, &crc)
        if c.size() > 0u64 {
            var k: u64 = 0u64
            while k < c.size() {
                val r = c.row(k)
                val path = catalog::seg_path(ps, &r)
                out.push(path.clone())
                k = k + 1u64
            }
        } else {
            val walked = logdir::scan_suffix(ps, ".seg")
            var k2: u64 = 0u64
            while k2 < walked.size() {
                val w: &String = walked.borrow(k2)
                out.push(w.clone())
                k2 = k2 + 1u64
            }
        }
        i = i + 1u64
    }
    out.sort()
}
