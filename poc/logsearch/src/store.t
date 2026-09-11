# Placing a finished segment: which mount it goes to, and the row
# that says where it went.
#
# The name is the one `ARCHITECTURE.md` section 3 gives this job --
# "the cross-mount inventory: pruning and placement". It exists as a
# file because two callers need it and had started to copy it: the
# `archive` subcommand, which fills a segment from log files, and the
# server, which fills one from `POST /v1/ingest`. Two copies of "put
# it somewhere and write it down" is two chances to write it down
# differently.

import std.fs
import std.time
import catalog
import mount
import archive
import segfile

# `YYYY/MM/DD` for the segment's own day, so that retention can drop
# one directory instead of hunting for files (STORAGE_FORMAT.md §1).
pub fn day_dir(out: str, secs: i64) -> String {
    val dt = DateTime::from_unix(secs)
    val ymd = time::format(dt, "%Y/%m/%d")
    val head = String::from_str(out)
    val seg = String::from_str("/seg/")
    # Each step is bound: a compound-returning method cannot sit in
    # an expression position in the compiled lanes.
    val with_seg = head.concat(&seg)
    val tail = String::from_str(ymd)
    val full = with_seg.concat(&tail)
    full
}

# Write one segment and say how many bytes its `.seg` came to, or 0
# when it could not be written.
pub fn flush_segment(w: &mut ArchiveWriter, out: str, segid: u64, crc: &Crc32,
                 gen: u64) -> u64 {
    var stamp = w.ts_min()
    if stamp == 0i64 { stamp = time::now_unix_secs() }
    val dir = day_dir(out, stamp)
    val dir_str = dir.to_str()
    val made = fs::mkdir_all(dir_str)
    match made {
        Result::Ok(u) => { }
        Result::Err(e) => { println("  mkdir {dir_str}: {e}")  return 0u64 }
    }
    val base = "{dir_str}/{segid:012}"
    val wrote = w.finish(base, segid, crc)
    var n: u64 = 0u64
    match wrote {
        Result::Ok(k) => { n = k }
        Result::Err(e) => { println("  write {base}: {e}")  return 0u64 }
    }
    # Record it. The row is read back out of the file that was just
    # written rather than built from what the writer believed, so the
    # catalog cannot describe a segment the segment does not.
    #
    # A failure here is a warning, not a failure of the archive: the
    # file is published, and a catalog that misses it is a catalog
    # that `repair` rebuilds (STORAGE_FORMAT.md section 2, step 9).
    if gen > 0u64 {
        if !record_segment(out, base, gen, crc) {
            println("  {base}.seg: written, but not recorded in the catalog")
        }
    }
    val records = w.count()
    val arena = w.arena_bytes()
    val pct = if arena > 0u64 { (n * 100u64) / arena } else { 0u64 }
    println("  {base}.seg  {records} records  {arena} B -> {n} B ({pct}%)")
    n
}

# Append one `ADD` row for the segment at `<base>.seg`.
pub fn record_segment(mount_dir: str, base: str, gen: u64, crc: &Crc32) -> bool {
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
                var r = catalog::row_of_head(&h, size, false)
                # Where it went, not where its timestamps say it
                # should have: a segment with no dated records at all
                # is filed under the day it was written.
                val name = String::from_str(path)
                val key = catalog::daykey_of_path(&name)
                if key > 0u64 { r.daykey = key }
                ok = catalog::append_add(mount_dir, gen, &r, crc)
            }
        }
        Result::Err(e) => { }
    }
    ok
}

# Put one finished segment on the least-used writable mount.
#
# Returns the bytes it came to, or 0 when there was nowhere to put it
# -- which is a real outcome (every mount full or degraded), not an
# internal error, so the caller counts it rather than stopping.
pub fn place_segment(w: &mut ArchiveWriter, ms: &mut MountSet, gens: &Vec<u64>,
                 segid: u64, crc: &Crc32) -> u64 {
    val at = ms.pick()
    var done: u64 = 0u64
    match at {
        Option::Some(mi) => {
            val p = ms.path_of(mi)
            val mp = p.to_str()
            val g = gens.get(mi)
            done = flush_segment(w, mp, segid, crc, g)
            if done > 0u64 {
                val now = ms.used_of(mi)
                ms.set_used(mi, now + done)
            } else {
                ms.mark(mi, mount::state_degraded())
            }
        }
        Option::None => {
            println("  segment {segid} has nowhere to go: every mount is full, readonly or degraded")
        }
    }
    done
}

# Get each mount ready to take segments: give it an identity if it
# has none, and find the catalog generation its journal appends to.
#
# A mount that cannot do either is marked degraded rather than fatal
# -- the others still take writes, which is the whole reason there is
# more than one. `gens` comes back index-aligned with `ms`, holding 0
# for a mount that is not usable.
pub fn open_for_write(spec: str, ms: &mut MountSet, gens: &mut Vec<u64>,
                      crc: &Crc32, loud: bool) -> bool {
    if !mount::open_spec(spec, ms) { return false }
    ms.refresh_used(crc)
    gens.clear()
    var i: u64 = 0u64
    while i < ms.size() {
        val p = ms.path_of(i)
        val ps = p.to_str()
        var gen: u64 = 0u64
        val meta = mount::ensure_meta(ps, "logsearch")
        if !meta.ok {
            if loud { println("  {ps}: cannot read or write meta/mount.json") }
            ms.mark(i, mount::state_degraded())
        } else {
            var c = catalog::load(ps, crc)
            gen = c.generation()
            if gen == 0u64 {
                if catalog::write_generation(ps, &c, 1u64, crc) {
                    gen = 1u64
                } else {
                    if loud { println("  {ps}: cannot publish a catalog") }
                    ms.mark(i, mount::state_degraded())
                }
            }
            if loud {
                val share = ms.permille(i)
                val used = ms.used_of(i)
                val quota = ms.quota_of(i)
                println("  mount {ps}  {used} / {quota} B ({share} permille)")
            }
        }
        gens.push(gen)
        i = i + 1u64
    }
    true
}

# Fold each mount's journal into a fresh generation, so the next
# start reads one file instead of replaying a run.
pub fn compact_all(ms: &MountSet, crc: &Crc32, loud: bool) {
    var i: u64 = 0u64
    while i < ms.size() {
        val p = ms.path_of(i)
        val ps = p.to_str()
        var c = catalog::load(ps, crc)
        if c.applied() > 0u64 {
            if !catalog::compact(ps, &mut c, crc) {
                if loud {
                    println("  {ps}: could not fold the journal into a new generation")
                }
            }
        }
        i = i + 1u64
    }
}
