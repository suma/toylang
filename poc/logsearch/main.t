# logsearch -- read logs, and turn what was read into an archive.
#
# Three subcommands, all of them read-only towards the logs:
#
#   scan    <dir> [limit]          what is there, and how it framed
#   archive <dir> <spec> [limit]   parse it and compress it into segments
#   query   <spec> "<query>"       search the segments
#   fields  <spec> <field> [limit]  count the values of one field
#   object  <spec> "<key>=<value>" one object: count, first / last seen
#   verify  <spec>                 read every segment back and check it
#   catalog <spec> [repair|compact] what the catalog holds, or rebuild it
#   retain  <spec> [days]          drop segments older than the window
#   serve   <spec> [port] [idle]   answer HTTP on 127.0.0.1
#
# `<spec>` is a mount configuration (`*.conf`) or a single directory
# used as one mount. Several directories mean several mounts, and a
# new segment goes to the least-used share of the ones that can take
# it (DATA_MODEL.md section 6).
#
# The two that matter are `archive` and `query`: one turns a log
# directory into segments, the other answers questions about them.
#   /tmp/logread archive poc/logsearch/log /tmp/logarchive
#   /tmp/logread query /tmp/logarchive "tag=CRON session limit=5"
#
# Run it (from the repository root):
#
#   cargo run -q -p toy -- run poc/logsearch --release -- \
#       archive poc/logsearch/log /tmp/logarchive
#
# `toy` assembles the module roots itself (stdlib, then this
# package's `src/`). The equivalent compiler call, if the tool is in
# the way, is what `toy -v` prints:
#
#   ./target/release/compiler --core-modules core \
#       --core-modules poc/logsearch/src \
#       poc/logsearch/main.t --release -o /tmp/logread
#
# `poc/logsearch/build/root` -- a symlink farm that used to stand in
# for repeatable `--core-modules` -- is gone (BUILD-TOOL B0 made the
# flag repeatable).
#
# `--release` turns the `requires` clauses off. Run without it while
# developing: the contracts in `lsz` / `crc` / `bytes` are what catch
# a bad offset at the call that made it, instead of three frames
# later (design_by_contract.md).

import std.fs
import std.io
import std.parse
import std.time
import archive
import catalog
import extract
import logdir
import mount
import query
import record
import server
import segfile
import store

# The largest log file this reads whole, and so the largest one the
# service can index. 16 MiB covers a rotated `kern.log`.
#
# Segments are read a piece at a time now (`segfile.t`), but the
# *input* side still takes a log file whole: `io::read_file_into`
# fills one buffer and stops. `File::read` could stream it, at the
# cost of carrying a line across chunk boundaries -- worth doing, not
# done (RUNTIME_GAPS.md).
const BUF_BYTES: u64 = 16777216u64

fn arg_or(i: u64, fallback: str) -> str {
    var d = fallback
    if io::argc() > i {
        val a = io::arg(i)
        if a.len() > 0u64 { d = a }
    }
    d
}

fn arg_u64(i: u64, fallback: u64) -> u64 {
    if io::argc() > i {
        # `??` keeps the fallback for anything that is not a number:
        # a missing argument and an unparsable one mean the same
        # thing here, and the default is lazy so it costs nothing.
        parse::to_u64(io::arg(i)) ?? fallback
    } else {
        fallback
    }
}

# ---------------------------------------------------------------------

fn cmd_scan(dir: str, limit: u64) -> u64 {
    println("scanning {dir}")
    val files = logdir::scan(dir)
    val n_files = files.size()
    if n_files == 0u64 {
        println("no log files found")
        return 1u64
    }

    var reader = LogReader::with_capacity(BUF_BYTES)
    var rec = ParsedLine::new()
    val watch = Stopwatch::start()

    var total_bytes: u64 = 0u64
    var total_lines: u64 = 0u64
    var total_ts: u64 = 0u64
    var total_labels: u64 = 0u64
    var total_hosts: u64 = 0u64
    var k_plain: u64 = 0u64
    var k_syslog: u64 = 0u64
    var k_datetime: u64 = 0u64
    var k_apache: u64 = 0u64
    var k_epoch: u64 = 0u64
    var ts_min: i64 = 0i64
    var ts_max: i64 = 0i64
    var failed: u64 = 0u64
    var truncated: u64 = 0u64
    var read_files: u64 = 0u64

    var fi: u64 = 0u64
    while fi < n_files && read_files < limit {
        val path: &String = files.borrow(fi)
        val path_str = path.to_str()
        fi = fi + 1u64

        val loaded = reader.load(path_str)
        var bytes: u64 = 0u64
        var ok = true
        match loaded {
            Result::Ok(n) => { bytes = n }
            Result::Err(e) => {
                println("  {path_str}: {e}")
                failed = failed + 1u64
                ok = false
            }
        }
        if ok {
            read_files = read_files + 1u64
            if reader.was_truncated() { truncated = truncated + 1u64 }

            var lines: u64 = 0u64
            var with_ts: u64 = 0u64
            var f_syslog: u64 = 0u64
            var f_apache: u64 = 0u64
            var more = true
            while more {
                val nx = reader.next_line()
                match nx {
                    Option::Some(l) => {
                        if l.len > 0u64 {
                            val scan_win = reader.span()
                            match scan_win {
                                Option::Some(sp) => { record::parse_line(sp, l, &mut rec) }
                                Option::None => { }
                            }
                            lines = lines + 1u64
                            if rec.has_ts {
                                with_ts = with_ts + 1u64
                                if ts_min == 0i64 || rec.ts < ts_min { ts_min = rec.ts }
                                if rec.ts > ts_max { ts_max = rec.ts }
                            }
                            if rec.has_labels() { total_labels = total_labels + 1u64 }
                            if rec.has_host() { total_hosts = total_hosts + 1u64 }
                            if rec.kind == 0u32 { k_plain = k_plain + 1u64 }
                            if rec.kind == 1u32 { k_syslog = k_syslog + 1u64  f_syslog = f_syslog + 1u64 }
                            if rec.kind == 2u32 { k_datetime = k_datetime + 1u64 }
                            if rec.kind == 3u32 { k_apache = k_apache + 1u64  f_apache = f_apache + 1u64 }
                            if rec.kind == 4u32 { k_epoch = k_epoch + 1u64 }
                        }
                    }
                    Option::None => { more = false }
                }
            }
            total_bytes = total_bytes + bytes
            total_lines = total_lines + lines
            total_ts = total_ts + with_ts

            var shape = "mixed"
            if lines == 0u64 { shape = "empty" }
            if lines > 0u64 && f_syslog == lines { shape = "syslog" }
            if lines > 0u64 && f_apache == lines { shape = "apache" }
            println("  {path_str}  {bytes} B  {lines} lines  {with_ts} dated  {shape}")
        }
    }

    val ms = watch.elapsed_ms()
    println("")
    println("files read       {read_files} of {n_files} ({failed} failed, {truncated} truncated)")
    println("bytes            {total_bytes}")
    println("lines            {total_lines} ({total_ts} dated, {total_labels} labelled, {total_hosts} with a host)")
    println("shapes           syslog={k_syslog} apache={k_apache} datetime={k_datetime} epoch={k_epoch} plain={k_plain}")
    if total_ts > 0u64 {
        val dt_lo = DateTime::from_unix(ts_min)
        val dt_hi = DateTime::from_unix(ts_max)
        val lo_txt = time::format(dt_lo, "%Y-%m-%dT%H:%M:%SZ")
        val hi_txt = time::format(dt_hi, "%Y-%m-%dT%H:%M:%SZ")
        println("time span        {lo_txt} .. {hi_txt}")
    }
    println("elapsed          {ms} ms")
    0u64
}

# ---------------------------------------------------------------------

fn cmd_archive(dir: str, spec: str, limit: u64) -> u64 {
    println("archiving {dir} -> {spec}")
    val files = logdir::scan(dir)
    val n_files = files.size()
    if n_files == 0u64 {
        println("no log files found")
        return 1u64
    }

    var ms = MountSet::new()
    var gens: Vec<u64> = Vec::new()
    val crc0 = Crc32::new()
    if !store::open_for_write(spec, &mut ms, &mut gens, &crc0, true, false) {
        return 1u64
    }

    var reader = LogReader::with_capacity(BUF_BYTES)
    var rec = ParsedLine::new()
    var w = ArchiveWriter::new()
    val crc = Crc32::new()
    val watch = Stopwatch::start()

    # Where this run's ids start. **Not 1**: a mount that already
    # holds segments has those ids in its catalog, and re-using one
    # overwrites the row (the file lands under a different day, so
    # the records are still there — they are simply no longer
    # listed, until a `repair` finds them again). `next_segid` is
    # what the server has always used; `archive` predates it and
    # kept counting from 1, which was fine only for a directory it
    # owned alone.
    var segid: u64 = store::next_segid(&ms, &crc)
    var raw_total: u64 = 0u64
    var dat_total: u64 = 0u64
    var records_total: u64 = 0u64
    var segments: u64 = 0u64
    var bad: u64 = 0u64
    var read_files: u64 = 0u64

    var fi: u64 = 0u64
    while fi < n_files && read_files < limit {
        val path: &String = files.borrow(fi)
        val path_str = path.to_str()
        fi = fi + 1u64

        val loaded = reader.load(path_str)
        var ok = true
        match loaded {
            Result::Ok(n) => { }
            Result::Err(e) => { println("  {path_str}: {e}")  ok = false }
        }
        if ok {
            read_files = read_files + 1u64
            var more = true
            while more {
                val nx = reader.next_line()
                match nx {
                    Option::Some(l) => {
                        if l.len > 0u64 {
                            val win = reader.span()
                            match win {
                                Option::Some(sp) => {
                                    record::parse_line(sp, l, &mut rec)
                                    w.add(sp, l, &rec)
                                }
                                Option::None => { }
                            }
                            if w.is_full() {
                                val done = store::place_segment(&mut w, &mut ms, &gens, segid, &crc)
                                if done == 0u64 { bad = bad + 1u64 }
                                raw_total = raw_total + w.arena_bytes()
                                records_total = records_total + w.count()
                                dat_total = dat_total + done
                                segments = segments + 1u64
                                segid = segid + 1u64
                                w.reset()
                            }
                        }
                    }
                    Option::None => { more = false }
                }
            }
        }
    }

    if !w.is_empty() {
        val done = store::place_segment(&mut w, &mut ms, &gens, segid, &crc)
        if done == 0u64 { bad = bad + 1u64 }
        raw_total = raw_total + w.arena_bytes()
        records_total = records_total + w.count()
        dat_total = dat_total + done
        segments = segments + 1u64
        w.reset()
    }

    # Fold what was appended into a new generation, so the next start
    # reads one file instead of replaying the run.
    store::compact_all(&ms, &crc, true)

    val took = watch.elapsed_ms()
    println("")
    println("segments         {segments} ({bad} failed)")
    println("records          {records_total}")
    println("arena bytes      {raw_total}")
    println("archive bytes    {dat_total}")
    if raw_total > 0u64 {
        val pct = (dat_total * 100u64) / raw_total
        println("ratio            {pct}% of raw")
    }
    println("elapsed          {took} ms")
    0u64
}

# ---------------------------------------------------------------------

fn cmd_verify(out: str) -> u64 {
    println("verifying {out}")
    var segs: Vec<String> = Vec::new()
    mount::segments_of(out, &mut segs)
    val n = segs.size()
    if n == 0u64 {
        println("no segments found")
        return 1u64
    }
    val crc = Crc32::new()
    val watch = Stopwatch::start()
    var ok_count: u64 = 0u64
    var bad_count: u64 = 0u64
    var records: u64 = 0u64
    var raw: u64 = 0u64
    var stored: u64 = 0u64

    var i: u64 = 0u64
    while i < n {
        val p: &String = segs.borrow(i)
        val full = p.to_str()
        # `verify` takes the base name and adds the extension back.
        val base = p.substring(0u64, p.len() - 4u64)
        val base_str = base.to_str()
        val got = archive::verify(base_str, &crc)
        match got {
            Result::Ok(rep) => {
                if rep.ok {
                    ok_count = ok_count + 1u64
                } else {
                    bad_count = bad_count + 1u64
                    println("  {base_str}: FAILED ({rep.bad_frames} bad frames)")
                }
                records = records + rep.records
                raw = raw + rep.raw_bytes
                stored = stored + rep.seg_bytes
            }
            Result::Err(e) => {
                bad_count = bad_count + 1u64
                println("  {base_str}: {e}")
            }
        }
        i = i + 1u64
    }

    val ms = watch.elapsed_ms()
    println("")
    println("segments         {ok_count} ok, {bad_count} bad")
    println("records          {records}")
    println("expanded bytes   {raw}")
    println("stored bytes     {stored}")
    if raw > 0u64 {
        val pct = (stored * 100u64) / raw
        println("ratio            {pct}% of raw")
    }
    println("elapsed          {ms} ms")
    if bad_count > 0u64 { return 1u64 }
    0u64
}

# ---------------------------------------------------------------------

# What the catalog says, and the two ways of rewriting it.
#
#   catalog <spec>            what each mount holds
#   catalog <spec> repair     rebuild from `seg/` and publish
#   catalog <spec> compact    fold the journal into a new generation
#
# `repair` is not a last resort (STORAGE_FORMAT.md section 7): it
# reads 320 bytes per segment, so it is something to run when in
# doubt rather than something to fear.
fn cmd_catalog(spec: str, action: str) -> u64 {
    var ms = MountSet::new()
    if !mount::open_spec(spec, &mut ms) { return 1u64 }
    val crc = Crc32::new()
    var rc: u64 = 0u64
    var i: u64 = 0u64
    while i < ms.size() {
        val p = ms.path_of(i)
        val ps = p.to_str()
        if action == "repair" {
            var built = catalog::rebuild(ps, &crc)
            val rows = built.size()
            # Publish *after* the generation being replaced, or the
            # reader keeps taking the highest, which is the old one.
            val at = catalog::latest_gen(ps)
            built.adopt_generation(at)
            if catalog::compact(ps, &mut built, &crc) {
                val gen = built.generation()
                # The label dictionary caches the same segments.
                val relabelled = labels::repair(ps, gen, &crc)
                var words = "label dictionary rebuilt"
                if !relabelled { words = "label dictionary NOT written" }
                println("{ps}: rebuilt {rows} row(s) as generation {gen}, {words}")
            } else {
                println("{ps}: could not publish the rebuilt catalog")
                rc = 1u64
            }
        } elif action == "compact" {
            var c = catalog::load(ps, &crc)
            val was = c.applied()
            if catalog::compact(ps, &mut c, &crc) {
                val gen = c.generation()
                println("{ps}: folded {was} journal record(s) into generation {gen}")
            } else {
                println("{ps}: could not compact")
                rc = 1u64
            }
        } else {
            val meta = mount::read_meta(ps)
            if meta.ok {
                val u = meta.uuid.clone()
                println("{ps}  uuid {u}")
            } else {
                println("{ps}  (no meta/mount.json)")
            }
            val c = catalog::load(ps, &crc)
            val gen = c.generation()
            if gen == 0u64 {
                println("  no catalog -- `catalog {spec} repair` builds one from seg/")
            } else {
                val rows = c.size()
                val applied = c.applied()
                val records = c.total_records()
                val bytes = c.total_bytes()
                println("  generation {gen}, {rows} segment(s), {applied} journal record(s)")
                println("  {records} records, {bytes} bytes")
                if rows > 0u64 {
                    var lo: i64 = 0i64
                    var hi: i64 = 0i64
                    var k: u64 = 0u64
                    while k < rows {
                        val r = c.row(k)
                        if k == 0u64 {
                            lo = r.ts_min
                            hi = r.ts_max
                        } else {
                            if r.ts_min < lo { lo = r.ts_min }
                            if r.ts_max > hi { hi = r.ts_max }
                        }
                        k = k + 1u64
                    }
                    val dt_lo = DateTime::from_unix(lo)
                    val dt_hi = DateTime::from_unix(hi)
                    val lo_txt = time::format(dt_lo, "%Y-%m-%dT%H:%M:%SZ")
                    val hi_txt = time::format(dt_hi, "%Y-%m-%dT%H:%M:%SZ")
                    println("  {lo_txt} .. {hi_txt}")
                }
            }
        }
        i = i + 1u64
    }
    rc
}

# ---------------------------------------------------------------------

# Drop the segments whose last record is older than `days`.
#
# **Retention is per segment, not per record** (DATA_MODEL.md section
# 4): a segment holding one record inside the window stays whole. The
# order is catalog first, file second (STORAGE_FORMAT.md section 8) --
# a file that is still listed but gone reads as `NotFound` at query
# time, while a file that is gone from the catalog but still on disk
# costs only space and is found again by `repair`.
#
# The 60-second grace period the design calls for is not here: it
# exists so a query that is already running does not lose a file out
# from under it, and this command is the whole process. A server
# needs it; a one-shot command does not have anyone to wait for.
# `compact <spec>` — merge one run of cold segments per mount.
#
# One pass, not a loop: compaction is meant to be interruptible, and
# an operator (or a cron line) that wants more calls it again. It
# says what it did so "nothing to do" is visible rather than silent.
fn cmd_compact(spec: str) -> u64 {
    var ms = MountSet::new()
    if !mount::open_spec(spec, &mut ms) { return 1u64 }
    val crc = Crc32::new()
    val now = time::now_unix_secs()
    var rc: u64 = 0u64
    var merged: u64 = 0u64
    var i: u64 = 0u64
    while i < ms.size() {
        val p = ms.path_of(i)
        val ps = p.to_str()
        if ms.is_readonly(i) {
            println("  {ps}: readonly, left alone")
        } else {
            val done = compact::compact_once(ps, now, &crc)
            if !done.is_ok() {
                println("  {ps}: could not compact")
                rc = 1u64
            } elif done.merged() == 0u64 {
                println("  {ps}: nothing cold enough to merge")
            } else {
                val pct = if done.bytes_in() > 0u64 {
                    (done.bytes_out() * 100u64) / done.bytes_in()
                } else { 0u64 }
                println("  {ps}: {done.merged()} segments -> archive {done.segid()}  {done.records()} records  {done.bytes_in()} B -> {done.bytes_out()} B ({pct}%)")
                merged = merged + done.merged()
            }
        }
        i = i + 1u64
    }
    println("segments merged {merged}")
    rc
}

fn cmd_retain(spec: str, days: u64) -> u64 {
    var ms = MountSet::new()
    if !mount::open_spec(spec, &mut ms) { return 1u64 }
    val crc = Crc32::new()
    val now = time::now_unix_secs()
    val cutoff = now - ((days as i64) * 86400i64)
    val when = DateTime::from_unix(cutoff)
    val when_txt = time::format(when, "%Y-%m-%dT%H:%M:%SZ")
    println("dropping segments whose last record is before {when_txt}")

    var rc: u64 = 0u64
    var dropped: u64 = 0u64
    var kept: u64 = 0u64
    var freed: u64 = 0u64
    var i: u64 = 0u64
    while i < ms.size() {
        val p = ms.path_of(i)
        val ps = p.to_str()
        if ms.is_readonly(i) {
            println("  {ps}: readonly, left alone")
        } else {
            var c = catalog::load(ps, &crc)
            val gen = c.generation()
            if gen == 0u64 {
                println("  {ps}: no catalog -- run `catalog {spec} repair` first")
                rc = 1u64
            } else {
                var dead: Vec<u64> = Vec::new()
                c.expired(cutoff, &mut dead)
                kept = kept + (c.size() - dead.size())

                # Read everything out before changing anything: the
                # indices `expired` hands back stop meaning what they
                # meant as soon as one row is removed.
                var ids: Vec<u64> = Vec::new()
                var sizes: Vec<u64> = Vec::new()
                var keys: Vec<u64> = Vec::new()
                var paths: Vec<String> = Vec::new()
                var k: u64 = 0u64
                while k < dead.size() {
                    val idx = dead.get(k)
                    val r = c.row(idx)
                    ids.push(r.segid)
                    sizes.push(r.seg_bytes)
                    keys.push(r.daykey)
                    val sp = catalog::seg_path(ps, &r)
                    paths.push(sp.clone())
                    k = k + 1u64
                }

                var j: u64 = 0u64
                while j < ids.size() {
                    val segid = ids.get(j)
                    val why = catalog::why_retention()
                    if catalog::append_remove(ps, gen, segid, why, &crc) {
                        val gone = c.remove(segid)
                        val sp: &String = paths.borrow(j)
                        # Out of the label dictionary before the file
                        # goes: after the unlink there is nothing left
                        # to read the segment's terms from.
                        val forgot = labels::forget_segment(ps, sp, gen, &crc)
                        val sps = sp.to_str()
                        val rm = fs::remove_file(sps)
                        match rm {
                            Result::Ok(u) => {
                                dropped = dropped + 1u64
                                freed = freed + sizes.get(j)
                            }
                            Result::Err(e) => {
                                # Already off the catalog, which is
                                # the state that matters; the file is
                                # space, and `repair` would put it
                                # back, so say so.
                                println("  {sps}: dropped from the catalog but not removed ({e})")
                                rc = 1u64
                            }
                        }
                    } else {
                        println("  {ps}: cannot append REMOVE for segment {segid}")
                        rc = 1u64
                    }
                    j = j + 1u64
                }

                # A day whose segments are all gone leaves an empty
                # directory, and so do the month and the year above it
                # once their last day goes. `remove_dir` failing *is*
                # the check that a level was not empty, so there is
                # nothing to test first and nothing to undo.
                var d: u64 = 0u64
                while d < keys.size() {
                    val key = keys.get(d)
                    val dir = catalog::day_path(ps, key)
                    prune_empty_days(dir.to_str())
                    d = d + 1u64
                }

                if ids.size() > 0u64 {
                    if !catalog::compact(ps, &mut c, &crc) {
                        println("  {ps}: could not fold the removals into a new generation")
                        rc = 1u64
                    }
                }
            }
        }
        i = i + 1u64
    }

    println("")
    println("segments dropped {dropped}")
    println("segments kept    {kept}")
    println("bytes freed      {freed}")
    rc
}

# Remove `<mount>/seg/YYYY/MM/DD` and each level above it that the
# removal emptied, stopping at the first one that is still in use.
fn prune_empty_days(day_dir: str) {
    # Unrolled rather than looped: the tree is exactly
    # `seg/YYYY/MM/DD`, and walking up it with a `var` would want to
    # move a `String` into an existing binding, which the compiled
    # lanes refuse (RUNTIME_GAPS.md G16).
    val day = String::from_str(day_dir)
    if !drop_dir(&day) { return }
    val month = parent_of(&day)
    if month.len() == 0u64 { return }
    if !drop_dir(&month) { return }
    val year = parent_of(&month)
    if year.len() == 0u64 { return }
    val done = drop_dir(&year)
}

# `remove_dir` succeeds only on an empty directory, which is exactly
# the question being asked.
fn drop_dir(path: &String) -> bool {
    val gone = fs::remove_dir(path.to_str())
    var ok = false
    match gone {
        Result::Ok(u) => { ok = true }
        Result::Err(e) => { }
    }
    ok
}

# Everything before the last `/`, or empty when there is none.
fn parent_of(path: &String) -> String {
    var cut = path.len()
    var found = false
    var i = path.len()
    while i > 0u64 && !found {
        i = i - 1u64
        val c: u8 = path.get(i)
        if c == '/' {
            cut = i
            found = true
        }
    }
    if !found {
        val empty = String::new()
        return empty
    }
    val out = path.substring(0u64, cut)
    out
}

# ---------------------------------------------------------------------

# `<key>=<value> top=<field>`: what this one value is linked to.
#
# The answer comes out of the link section -- pairs of field values
# that appeared on the same record, counted when the segment was
# written (ONTOLOGY.md O1). No records are read and no frames are
# expanded: the traversal is a walk of one group of link rows.
fn cmd_top_linked(dir: str, q: &Query, field: str, limit: u64) -> u64 {
    var segs: Vec<String> = Vec::new()
    mount::segments_of(dir, &mut segs)
    if segs.size() == 0u64 {
        println("no segments under {dir}")
        return 1u64
    }
    val anchor: &String = q.terms.borrow(0u64)
    println("linked {field} for {anchor}")

    val watch = Stopwatch::start()
    val crc = Crc32::new()
    var head_buf = ByteWriter::with_capacity(segfile::data_at() + 64u64)
    var raw = ByteWriter::with_capacity(4194304u64)
    var tsec = ByteWriter::with_capacity(4194304u64)
    var lsec = ByteWriter::with_capacity(4194304u64)
    val slot_bits: u64 = 65536u64
    var slots: Vec<u64> = Vec::with_capacity(slot_bits)
    var sz: u64 = 0u64
    while sz < slot_bits {
        slots.push(0u64)
        sz = sz + 1u64
    }
    var hashes: Vec<u64> = Vec::new()
    var names: Vec<String> = Vec::new()
    var counts: Vec<u64> = Vec::new()
    var rows: u64 = 0u64
    var segs_hit: u64 = 0u64

    var prefix = String::from_str(field)
    val colon = String::from_str(":")
    val want_prefix = prefix.concat(&colon)
    val plen = want_prefix.len()

    var si: u64 = 0u64
    while si < segs.size() {
        val seg_path: &String = segs.borrow(si)
        val seg_str = seg_path.to_str()
        si = si + 1u64
        val opened_f = File::open(seg_str)
        match opened_f {
            Result::Ok(f) => {
                # Two sections and nothing else: the traversal never
                # reads a frame, so a segment costs its directory plus
                # the term and link sections.
                val h = segfile::head_of(&f, &mut head_buf)
                var got = h.ok && h.has_terms() && h.has_links()
                if got {
                    if !segfile::load_block(&f, h.terms_off, h.terms_len, &crc, &mut raw, &mut tsec) { got = false }
                }
                if got {
                    if !segfile::load_block(&f, h.links_off, h.links_len, &crc, &mut raw, &mut lsec) { got = false }
                }
                if got {
                        val tw = tsec.span()
                        val lw = lsec.span()
                        match tw {
                        Option::Some(traw) => {
                        match lw {
                        Option::Some(lraw) => {
                            val aw = anchor.as_span()
                            match aw {
                                Option::Some(want) => {
                                    val id = archive::term_id_of(traw, tsec.len(), want, anchor.len())
                                    if id != archive::term_none() {
                                        segs_hit = segs_hit + 1u64
                                        var to_ids: Vec<u32> = Vec::new()
                                        var to_counts: Vec<u32> = Vec::new()
                                        val n = archive::links_of(lraw, lsec.len(), id, &mut to_ids, &mut to_counts)
                                        rows = rows + n
                                        val spans = archive::term_spans(traw, tsec.len())
                                        var i: u64 = 0u64
                                        while i < to_ids.size() {
                                            val tid: u32 = to_ids.get(i)
                                            val cnt: u32 = to_counts.get(i)
                                            val sp: u64 = spans.get(tid as u64)
                                            val at = record::span_start(sp)
                                            val nlen = record::span_len(sp)
                                            if nlen > plen {
                                                var same = true
                                                var k: u64 = 0u64
                                                while k < plen && same {
                                                    val a: u8 = traw.get(at + k)
                                                    val b: u8 = want_prefix.get(k)
                                                    if a != b { same = false }
                                                    k = k + 1u64
                                                }
                                                if same {
                                                    val vat = at + plen
                                                    val vlen = nlen - plen
                                                    val key = extract::hash_span(traw, vat, vlen)
                                                    val mask = slot_bits - 1u64
                                                    var slot = key & mask
                                                    var placed = false
                                                    while !placed {
                                                        val cell: u64 = slots.get(slot)
                                                        if cell == 0u64 {
                                                            val pos = hashes.size()
                                                            hashes.push(key)
                                                            counts.push(cnt as u64)
                                                            val nm = query::text_of(traw, vat, vlen)
                                                            names.push(nm)
                                                            slots.set(slot, pos + 1u64)
                                                            placed = true
                                                        } else {
                                                            val pos = cell - 1u64
                                                            val hv: u64 = hashes.get(pos)
                                                            if hv == key {
                                                                val prev: u64 = counts.get(pos)
                                                                counts.set(pos, prev + (cnt as u64))
                                                                placed = true
                                                            } else {
                                                                slot = (slot + 1u64) & mask
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            i = i + 1u64
                                        }
                                    }
                                }
                                Option::None => { }
                            }
                        }
                        Option::None => { }
                        }
                        }
                        Option::None => { }
                        }
                }
            }
            Result::Err(e) => { }
        }
    }

    var tallies: Vec<Tally> = Vec::new()
    var i: u64 = 0u64
    while i < names.size() {
        val c: u64 = counts.get(i)
        val t = Tally { count: c, idx: i }
        tallies.push(t)
        i = i + 1u64
    }
    tallies.sort()
    val total = tallies.size()
    var shown: u64 = 0u64
    var k: u64 = 0u64
    while k < total && shown < limit {
        val t: Tally = tallies.get(total - 1u64 - k)
        val nm: &String = names.borrow(t.idx)
        println("  {t.count}  {nm}")
        shown = shown + 1u64
        k = k + 1u64
    }
    val ms = watch.elapsed_ms()
    println("")
    println("segments         {segs_hit} hold this value")
    println("link rows        {rows}")
    println("distinct values  {total}")
    println("elapsed          {ms} ms")
    0u64
}

fn cmd_query(dir: str, text: str) -> u64 {
    # `top=<field>` asks a different question -- how many of each,
    # rather than which lines -- so it answers from the term index
    # instead of running the record walk.
    val whole = String::from_str(text)
    val sep = String::from_str(" ")
    val parts = whole.split(&sep)
    # A one-element vector rather than a `String`: a compound cannot
    # be assigned into an existing binding, and the loop discovers the
    # value as it goes.
    var top_fields: Vec<String> = Vec::new()
    var top_limit: u64 = 10u64
    var i: u64 = 0u64
    while i < parts.size() {
        val tok: &String = parts.borrow(i)
        if tok.len() > 4u64 {
            val head = tok.substring(0u64, 4u64)
            if head.eq_str("top=") {
                val f = tok.substring(4u64, tok.len())
                top_fields.push(f)
            }
            if tok.len() > 6u64 {
                val h2 = tok.substring(0u64, 6u64)
                if h2.eq_str("limit=") {
                    val rest = tok.substring(6u64, tok.len())
                    top_limit = parse::to_u64(rest.to_str()) ?? top_limit
                }
            }
        }
        i = i + 1u64
    }
    if top_fields.size() > 0u64 {
        val chosen: &String = top_fields.borrow(0u64)
        # `ip=1.2.3.4 top=path` is a traversal: the field filter picks
        # one object and `top=` asks what it is linked to. Without a
        # filter the same word means "the whole distribution", which
        # is what the term index alone can answer (O0).
        val now2 = time::now_unix_secs()
        val q2 = query::parse_query(text, now2)
        if q2.term_count() == 1u64 && q2.sub_count() == 0u64 && q2.needle_count() == 0u64 {
            return cmd_top_linked(dir, &q2, chosen.to_str(), top_limit)
        }
        # Anything else would be answered by dropping the filter, which
        # is a different question than the one asked -- and it looks
        # like an answer. Traversal starts from **one** value, because
        # a link row names one `from`; a `~` matches several, and two
        # `=` name two objects.
        if q2.indexed_count() > 0u64 || q2.needle_count() > 0u64 {
            println("`top=` traverses from exactly one `key=value`")
            println("  got {q2.term_count()} exact, {q2.sub_count()} `~`, {q2.needle_count()} substring")
            println("  drop the extra filters for the whole distribution, or name one value")
            return 1u64
        }
        return cmd_fields_indexed(dir, chosen.to_str(), top_limit)
    }

    println("query {text}")
    val now = time::now_unix_secs()
    val q = query::parse_query(text, now)
    val crc = Crc32::new()
    var segs: Vec<String> = Vec::new()
    mount::segments_of(dir, &mut segs)
    val n = query::run(dir, &segs, &q, &crc)
    n
}

# Count the values of one field across an archive.
#
# The counting keeps one `String` per *distinct* value, not per
# record: values are looked up by a hash of their bytes, and only a
# value seen for the first time is copied out of the arena. On this
# runtime that is the difference between a few kilobytes and a
# permanent 11 MB (MEMORY.md D3).
# Count the values of one field **out of the term index**.
#
# This is the answer O0-b exists for: the index already knows how many
# records carry each `key:value`, so the count is a walk of the term
# dictionary -- no frames expanded, no arena scanned, no records
# decoded. `cmd_fields_scan` below is kept as the reference it has to
# agree with.
fn cmd_fields_indexed(dir: str, field: str, limit: u64) -> u64 {
    # Any label key, not the eight `extract.t` knows: the dictionary
    # holds whatever was written, and a `fields app` over ingested
    # records is the same walk as a `fields status` over apache ones.
    val name = String::from_str(field)
    if !query::is_index_key(&name) {
        println("`{field}` is not a label key ([a-z0-9_], 1 to 32 bytes)")
        return 1u64
    }
    println("field {field} (index)")

    var segs: Vec<String> = Vec::new()
    mount::segments_of(dir, &mut segs)
    val n_segs = segs.size()
    if n_segs == 0u64 {
        println("no segments under {dir}")
        return 1u64
    }

    val watch = Stopwatch::start()
    val crc = Crc32::new()
    # The dictionary spells a term `key:value`, so the key the caller
    # named is the prefix. There is no table of eight any more: a
    # label key is whatever was written.
    val prefix = "{field}:"
    val tal = query::tally(&segs, prefix, false, &crc)
    val indexed_segs = tal.segments
    val terms_seen = tal.terms

    if indexed_segs == 0u64 {
        println("no term index in these segments -- rebuild the archive")
        return 1u64
    }

    var tallies: Vec<Tally> = Vec::new()
    var i: u64 = 0u64
    while i < tal.size() {
        val c: u64 = tal.counts.get(i)
        val one = Tally { count: c, idx: i }
        tallies.push(one)
        i = i + 1u64
    }
    tallies.sort()
    val total = tallies.size()
    var shown: u64 = 0u64
    var k: u64 = 0u64
    while k < total && shown < limit {
        val t: Tally = tallies.get(total - 1u64 - k)
        val nm: &String = tal.names.borrow(t.idx)
        println("  {t.count}  {nm}")
        shown = shown + 1u64
        k = k + 1u64
    }
    val ms = watch.elapsed_ms()
    println("")
    println("segments         {indexed_segs} with an index")
    println("terms            {terms_seen} in the dictionary")
    println("distinct values  {total}")
    println("elapsed          {ms} ms")
    0u64
}

# The value of label `key` in the run of labels at [from, from + len),
# packed with `record::pack_span`; length 0 when the run has no such
# label. The rules are DATA_MODEL.md section 2's: `key=value`
# tokens separated by one space, a value that ends at a space.
fn label_value(w: Span<u8>, from: u64, len: u64, key: str) -> u64 {
    val want = String::from_str(key)
    val klen = want.len()
    val end = from + len
    var p = from
    var found: u64 = record::pack_span(0u64, 0u64)
    var done = false
    while p < end && !done {
        var stop = p
        while stop < end && w.get(stop) != ' ' { stop = stop + 1u64 }
        if stop - p > klen && w.get(p + klen) == '=' {
            var same = true
            var i = 0u64
            while i < klen && same {
                val a: u8 = w.get(p + i)
                val b: u8 = want.get(i)
                same = a == b
                i = i + 1u64
            }
            if same {
                found = record::pack_span(p + klen + 1u64, stop - p - klen - 1u64)
                done = true
            }
        }
        p = stop + 1u64
    }
    found
}

fn same_bytes(w: Span<u8>, a: u64, a_len: u64, b: u64, b_len: u64) -> bool {
    if a_len != b_len { return false }
    var i = 0u64
    var same = true
    while i < a_len && same {
        val x: u8 = w.get(a + i)
        val y: u8 = w.get(b + i)
        same = x == y
        i = i + 1u64
    }
    same
}

fn cmd_fields(dir: str, field: str, limit: u64) -> u64 {
    val code = query::field_code(field)
    if code == query::field_none() {
        println("unknown field `{field}` -- try status / method / path / ip / vhost / ua / proto / host / tag")
        return 1u64
    }
    println("field {field}")

    var segs: Vec<String> = Vec::new()
    mount::segments_of(dir, &mut segs)
    val n_segs = segs.size()
    if n_segs == 0u64 {
        println("no segments under {dir}")
        return 1u64
    }

    val watch = Stopwatch::start()
    val crc = Crc32::new()
    var head_buf = ByteWriter::with_capacity(segfile::data_at() + 64u64)
    var raw = ByteWriter::with_capacity(archive::frame_raw_bytes() + 65536u64)
    var arena_buf = ByteWriter::with_capacity(archive::segment_target_bytes() + 65536u64)
    var recs = ByteWriter::with_capacity(4194304u64)
    var index_of: Dict<u64, u64> = Dict::new()
    var names: Vec<String> = Vec::new()
    var counts: Vec<u64> = Vec::new()
    var examined: u64 = 0u64
    var carried: u64 = 0u64

    var si: u64 = 0u64
    while si < n_segs {
        val seg_path: &String = segs.borrow(si)
        val seg_str = seg_path.to_str()
        si = si + 1u64

        val opened_f = File::open(seg_str)
        match opened_f {
            Result::Ok(f) => {
                val h = segfile::head_of(&f, &mut head_buf)
                var good = h.ok
                if !good { println("  {seg_str}: not a segment") }
                if good {
                    arena_buf.clear()
                    if !segfile::expand_all(&f, &h, &crc, &mut raw, &mut arena_buf) {
                        println("  {seg_str}: frames unreadable")
                        good = false
                    }
                }
                if good {
                    if !segfile::read_range(&f, h.recs_off, h.recs_len, &mut recs) {
                        println("  {seg_str}: record table unreadable")
                        good = false
                    }
                }
                if good {
                                val idx_w = recs.span()
                                val aw = arena_buf.span()
                                match idx_w {
                                    Option::Some(idx) => {
                                        match aw {
                                            Option::Some(arena) => {
                                                val count = h.records
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
                                                    val kind = ((flags >> 1u64) & 7u64) as u32
                                                    examined = examined + 1u64

                                                    var at: u64 = 0u64
                                                    var flen: u64 = 0u64
                                                    if code == query::field_host() {
                                                        at = line_at + host_rel
                                                        flen = host_len
                                                    } elif code == query::field_tag() {
                                                        at = line_at + tag_rel
                                                        flen = tag_len
                                                    } else {
                                                        if kind == 3u32 {
                                                            val f = extract::http(arena, line_at, line_len)
                                                            if f.ok {
                                                                var packed: u64 = 0u64
                                                                if code == query::field_status() { packed = f.status }
                                                                if code == query::field_method() { packed = f.method }
                                                                if code == query::field_path() { packed = f.path }
                                                                if code == query::field_client() { packed = f.client }
                                                                if code == query::field_vhost() { packed = f.vhost }
                                                                if code == query::field_ua() { packed = f.ua }
                                                                if code == query::field_proto() { packed = f.proto }
                                                                at = extract::field_start(packed)
                                                                flen = extract::field_len(packed)
                                                            }
                                                        }
                                                    }

                                                    # `host` is also a reserved label (DATA_MODEL.md
                                                    # section 2), and the index folds `host=web01`
                                                    # into the same term as a syslog header host.
                                                    # The reference has to do the same, or it is the
                                                    # one that comes up short. A record that says the
                                                    # same host both ways is one record.
                                                    var at2: u64 = 0u64
                                                    var flen2: u64 = 0u64
                                                    if code == query::field_host() && labels_len > 0u64 {
                                                        val lv = label_value(arena, line_at + labels_rel, labels_len, "host")
                                                        val l_at = record::span_start(lv)
                                                        val l_len = record::span_len(lv)
                                                        if l_len > 0u64 && !same_bytes(arena, at, flen, l_at, l_len) {
                                                            at2 = l_at
                                                            flen2 = l_len
                                                        }
                                                    }
                                                    var pass: u64 = 0u64
                                                    while pass < 2u64 {
                                                        var v_at = at
                                                        var v_len = flen
                                                        if pass == 1u64 {
                                                            v_at = at2
                                                            v_len = flen2
                                                        }
                                                        if v_len > 0u64 {
                                                            carried = carried + 1u64
                                                            val h = extract::hash_span(arena, v_at, v_len)
                                                            val seen = index_of.get(h)
                                                            match seen {
                                                                Option::Some(pos) => {
                                                                    val c: u64 = counts.get(pos)
                                                                    counts.set(pos, c + 1u64)
                                                                }
                                                                Option::None => {
                                                                    val pos = names.size()
                                                                    val nm = query::text_of(arena, v_at, v_len)
                                                                    names.push(nm)
                                                                    counts.push(1u64)
                                                                    index_of.insert(h, pos)
                                                                }
                                                            }
                                                        }
                                                        pass = pass + 1u64
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
            Result::Err(e) => { println("  {seg_str}: {e}") }
        }
    }

    var tallies: Vec<Tally> = Vec::new()
    var i: u64 = 0u64
    while i < names.size() {
        val c: u64 = counts.get(i)
        val t = Tally { count: c, idx: i }
        tallies.push(t)
        i = i + 1u64
    }
    tallies.sort()

    val total = tallies.size()
    var shown: u64 = 0u64
    var k: u64 = 0u64
    while k < total && shown < limit {
        val t: Tally = tallies.get(total - 1u64 - k)
        val nm: &String = names.borrow(t.idx)
        println("  {t.count}  {nm}")
        shown = shown + 1u64
        k = k + 1u64
    }

    val ms = watch.elapsed_ms()
    println("")
    println("records          {examined} examined, {carried} carried the field")
    println("distinct values  {total}")
    println("elapsed          {ms} ms")
    0u64
}

# ONTOLOGY O1 — one object, and when it was seen.
#
# The count comes from the dictionary and the ends from the object
# table, so this reads two sections per segment and expands nothing.
# Segments are folded: the count adds up, the ends take the outer
# bounds of whichever segments hold the value.
fn cmd_object(dir: str, spec: str) -> u64 {
    val q = query::parse_query(spec, 0i64)
    # One object means one value, and nothing else. Extra tokens would
    # have to be dropped to answer at all, and a dropped filter looks
    # like an answer -- `ua=Mozilla/5.0 (compatible; ...)` splits on
    # its spaces, and the tail must not be silently discarded.
    if q.term_count() != 1u64 || q.sub_count() > 0u64 || q.needle_count() > 0u64 {
        println("object takes exactly one `key=value` and nothing else")
        println("  got {q.term_count()} exact, {q.sub_count()} `~`, {q.needle_count()} substring")
        println("  a value with spaces cannot be named here -- use `fields` or `query`")
        return 1u64
    }
    val want: &String = q.terms.borrow(0u64)
    println("object {want}")

    var segs: Vec<String> = Vec::new()
    mount::segments_of(dir, &mut segs)
    val crc = Crc32::new()
    var head_buf = ByteWriter::with_capacity(segfile::data_at() + 64u64)
    var raw = ByteWriter::with_capacity(1048576u64)
    var tsec = ByteWriter::with_capacity(1048576u64)
    var osec = ByteWriter::with_capacity(1048576u64)

    var count: u64 = 0u64
    var first: i64 = limits::i64_max()
    var last: i64 = limits::i64_min()
    var held: u64 = 0u64
    var dated: u64 = 0u64
    # Segments that carry an object table at all. An archive written
    # before O1 has none, and that is a different thing from a value
    # whose lines are undated -- saying the wrong one sends the reader
    # looking at their data instead of at their archive.
    var tabled: u64 = 0u64

    var si: u64 = 0u64
    while si < segs.size() {
        val seg: &String = segs.borrow(si)
        si = si + 1u64
        val opened = File::open(seg.to_str())
        match opened {
            Result::Ok(f) => {
                val h = segfile::head_of(&f, &mut head_buf)
                if h.ok && h.has_terms() {
                    if segfile::load_block(&f, h.terms_off, h.terms_len, &crc, &mut raw, &mut tsec) {
                        val tw = tsec.span()
                        match tw {
                            Option::Some(traw) => {
                                val nw = want.as_span()
                                match nw {
                                    Option::Some(wsp) => {
                                        val post = archive::term_postings(traw, tsec.len(), wsp, want.len())
                                        if post.found {
                                            held = held + 1u64
                                            count = count + post.doc_count
                                            val id = archive::term_id_of(traw, tsec.len(), wsp, want.len())
                                            if h.has_objects() {
                                                tabled = tabled + 1u64
                                                if segfile::load_block(&f, h.objs_off, h.objs_len, &crc, &mut raw, &mut osec) {
                                                    val ow = osec.span()
                                                    match ow {
                                                        Option::Some(oraw) => {
                                                            val sp = archive::object_span(oraw, osec.len(), id)
                                                            if sp.found {
                                                                dated = dated + 1u64
                                                                if sp.first < first { first = sp.first }
                                                                if sp.last > last { last = sp.last }
                                                            }
                                                        }
                                                        Option::None => { }
                                                    }
                                                }
                                            }
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
            Result::Err(e) => { println("  {seg}: {e}") }
        }
    }

    if held == 0u64 {
        println("  not in this archive")
        return 1u64
    }
    println("  records        {count}")
    println("  segments       {held}")
    if tabled == 0u64 {
        println("  first seen     -- (this archive has no object table)")
        println("  last seen      -- (re-archive to record one)")
    } elif dated == 0u64 {
        # Real answer, not a missing one: the value is here, but every
        # line carrying it was undated.
        println("  first seen     -- (no dated line carries it)")
        println("  last seen      --")
    } else {
        val fs = io::strftime("%Y-%m-%dT%H:%M:%SZ", first as u64)
        val ls = io::strftime("%Y-%m-%dT%H:%M:%SZ", last as u64)
        println("  first seen     {fs}")
        println("  last seen      {ls}")
        println("  span           {last - first} s")
    }
    0u64
}

fn main() -> u64 {
    val mode = arg_or(0u64, "scan")
    # Each branch names its bindings differently. Two sibling
    # branches that both bind `out` are read as one binding moved
    # twice ("cannot be moved inside a branch"), even though only one
    # of them can run.
    if mode == "archive" {
        val a_dir = arg_or(1u64, "poc/logsearch/log")
        val a_out = arg_or(2u64, "/tmp/logarchive")
        val a_limit = arg_u64(3u64, 1000000u64)
        return cmd_archive(a_dir, a_out, a_limit)
    }
    if mode == "query" {
        val q_out = arg_or(1u64, "/tmp/logarchive")
        val q_text = arg_or(2u64, "")
        return cmd_query(q_out, q_text)
    }
    if mode == "fields" {
        # A fourth argument of `scan` forces the full walk, which is
        # how the index is checked against the thing it replaces.
        val how = arg_or(4u64, "index")
        val idx_mode = String::from_str(how)
        val f_out = arg_or(1u64, "/tmp/logarchive")
        val f_field = arg_or(2u64, "status")
        val f_limit = arg_u64(3u64, 20u64)
        if idx_mode.eq_str("scan") {
            return cmd_fields(f_out, f_field, f_limit)
        }
        return cmd_fields_indexed(f_out, f_field, f_limit)
    }
    if mode == "object" {
        val o_out = arg_or(1u64, "/tmp/logarchive")
        val o_spec = arg_or(2u64, "")
        return cmd_object(o_out, o_spec)
    }
    if mode == "verify" {
        val v_out = arg_or(1u64, "/tmp/logarchive")
        return cmd_verify(v_out)
    }
    if mode == "catalog" {
        val c_spec = arg_or(1u64, "/tmp/logarchive")
        val c_action = arg_or(2u64, "list")
        return cmd_catalog(c_spec, c_action)
    }
    if mode == "compact" {
        val k_spec = arg_or(1u64, "/tmp/logarchive")
        return cmd_compact(k_spec)
    }
    if mode == "retain" {
        val r_spec = arg_or(1u64, "/tmp/logarchive")
        val r_days = arg_u64(2u64, 14u64)
        return cmd_retain(r_spec, r_days)
    }
    if mode == "serve" {
        val s_spec = arg_or(1u64, "/tmp/logarchive")
        val s_port = arg_u64(2u64, 8080u64)
        # An idle budget of 0 means "until told to stop". A test binds
        # port 0 and passes a small one so a forgotten server cannot
        # outlive the run.
        val s_idle = arg_u64(3u64, 0u64)
        return server::serve(s_spec, "127.0.0.1", s_port, s_idle)
    }
    if mode == "scan" {
        val s_dir = arg_or(1u64, "poc/logsearch/log")
        val s_limit = arg_u64(2u64, 1000000u64)
        return cmd_scan(s_dir, s_limit)
    }
    # No subcommand: the first argument is the directory.
    val d_limit = arg_u64(1u64, 1000000u64)
    cmd_scan(mode, d_limit)
}

# ---------------------------------------------------------------------
# サブコマンドの通し検査
#
# モジュール単位のテストは `tests/` に在るが、**`main.t` の
# サブコマンド自身**はここまで手で叩いて確かめていた。1 本の道
# (ログを読む → セグメントを書く → 検証する → 引く → 台帳を直す →
# 保持期限で捨てる) が繋がっていることは、部品が全部通っていても
# 言えない — 繋ぎ目はここにしか無いため。
#
# `test` ブロックが `main.t` に在るのは、サブコマンドが関数だから
# である (`toy test` はパッケージの entry も拾う)。
#
# 素材は合成で、アドレスはプライベート帯だけ (CLAUDE.md)。

fn e2e_line(text: str, out: &mut String) {
    out.push_str(text)
    out.push(10u8)
}

# apache 行と syslog 行を混ぜる。日付は 2026-09-03 で固定なので、
# 保持期限の検査が「今から見て古い」を安定して踏める。
fn e2e_fixture(n: u64) -> String {
    var s = String::new()
    var i = 0u64
    while i < n {
        e2e_line("10.0.0.{1u64 + i % 5u64} - - [03/Sep/2026:12:00:{i % 60u64} +0000] \"GET /p{i % 7u64} HTTP/1.1\" {200u64 + 4u64 * (i % 2u64)} {i} \"-\" \"curl/8.0\"", &mut s)
        i = i + 1u64
    }
    e2e_line("2026-09-03T12:30:00Z web01 cron[5]: job ran", &mut s)
    s
}

# 再帰削除。`fs` は `remove_dir_all` を持たない (意図的に —
# 要る人が `list_dir` + `remove_file` で書く形) ので、ここで書く。
fn e2e_wipe(path: str) {
    if fs::is_dir(path) {
        val listed = fs::list_dir(path)
        match listed {
            Result::Ok(names) => {
                var i = 0u64
                while i < names.size() {
                    val nm: &String = names.borrow(i)
                    e2e_wipe("{path}/{nm.to_str()}")
                    i = i + 1u64
                }
            }
            Result::Err(e) => { }
        }
        val gone = fs::remove_dir(path)
        match gone {
            Result::Ok(u) => { }
            Result::Err(e) => { panic("e2e: cannot remove {path}: {e}") }
        }
    } elif fs::is_file(path) {
        val gone = fs::remove_file(path)
        match gone {
            Result::Ok(u) => { }
            Result::Err(e) => { panic("e2e: cannot remove {path}: {e}") }
        }
    }
}

# 空のログディレクトリを作り、1 ファイル書く。返すのはそのディレクトリ。
fn e2e_logs(stem: str, lines: u64) -> String {
    val dir = "{stem}-logs"
    e2e_wipe(dir)
    val made = fs::mkdir_all(dir)
    match made {
        Result::Ok(u) => { }
        Result::Err(e) => { panic("e2e: cannot make {dir}: {e}") }
    }
    val body = e2e_fixture(lines)
    val wrote = io::write_file("{dir}/app.log", body.to_str())
    match wrote {
        Result::Ok(k) => { }
        Result::Err(e) => { panic("e2e: cannot write the fixture log: {e}") }
    }
    val out = String::from_str(dir)
    out
}

fn e2e_segments(spec: str) -> u64 {
    var segs: Vec<String> = Vec::new()
    mount::segments_of(spec, &mut segs)
    segs.size()
}

# ---------------------------------------------------------------------

test "archive, verify and query run as one road" {
    val logs = e2e_logs("build/e2e-road", 200u64)
    val arch = "build/e2e-road-arch"
    e2e_wipe(arch)

    assert_eq(cmd_archive(logs.to_str(), arch, 100u64), 0u64)
    assert(e2e_segments(arch) > 0u64, "archiving should leave a segment behind")
    # 書いたものを読み返して CRC まで見る。
    assert_eq(cmd_verify(arch), 0u64)
    # 索引で引く形と本文を舐める形、どちらも 0 で返る (件数は
    # `tests/search_query.t` の担当。ここは道が繋がっていること)。
    assert_eq(cmd_query(arch, "status=404 limit=0"), 0u64)
    assert_eq(cmd_query(arch, "job limit=0"), 0u64)
    # 台帳もこの 1 本の道の一部で、archive が書く。
    val crc = Crc32::new()
    val c = catalog::load(arch, &crc)
    assert_eq(c.size(), e2e_segments(arch))
    assert(c.total_records() >= 201u64, "the catalog should count every record")
}

# 同じマウントへ 2 回 `archive` しても、1 回目が消えない。
#
# `archive` は id を 1 から振っていたので、2 回目が 1 回目の行を
# **上書き**していた。ファイルは別の日ディレクトリに残るので、
# 消えるのは台帳の行だけ — `repair` するまで「在るのに載っていない」
# という、いちばん気づきにくい形になる。
test "archiving twice into one mount keeps both runs in the catalog" {
    val first = e2e_logs("build/e2e-twice-a", 30u64)
    val second = e2e_logs("build/e2e-twice-b", 30u64)
    val arch = "build/e2e-twice-arch"
    e2e_wipe(arch)

    assert_eq(cmd_archive(first.to_str(), arch, 100u64), 0u64)
    val crc = Crc32::new()
    val after_one = catalog::load(arch, &crc)
    assert_eq(after_one.size(), 1u64)
    val records_one = after_one.total_records()

    assert_eq(cmd_archive(second.to_str(), arch, 100u64), 0u64)
    val after_two = catalog::load(arch, &crc)
    assert_eq(after_two.size(), 2u64)
    assert_eq(after_two.total_records(), records_one * 2u64)
    # 台帳の行数とファイルの本数が一致する — どちらかだけが増えて
    # いたら、そのずれこそがこのテストの対象である。
    assert_eq(after_two.size(), e2e_segments(arch))
}

# ログが 1 つも無いディレクトリは**失敗で返る**。0 を返すと、
# cron から呼んだ人が「何も無いのに成功した」と読む。
test "archiving a directory with no logs is not a success" {
    val dir = "build/e2e-empty-logs"
    e2e_wipe(dir)
    val made = fs::mkdir_all(dir)
    match made {
        Result::Ok(u) => { }
        Result::Err(e) => { panic("cannot make {dir}: {e}") }
    }
    assert_eq(cmd_archive(dir, "build/e2e-empty-arch", 10u64), 1u64)
}

# 台帳を丸ごと捨てても `catalog repair` が作り直す。カタログが
# キャッシュであるという主張は、この 1 コマンドで支えられている。
test "a repair rebuilds the catalog after the metadata is thrown away" {
    val logs = e2e_logs("build/e2e-repair", 120u64)
    val arch = "build/e2e-repair-arch"
    e2e_wipe(arch)
    assert_eq(cmd_archive(logs.to_str(), arch, 100u64), 0u64)
    val segs = e2e_segments(arch)
    assert(segs > 0u64, "there should be something to rebuild from")

    e2e_wipe("{arch}/meta")
    val crc = Crc32::new()
    val empty = catalog::load(arch, &crc)
    assert(empty.is_empty(), "the catalog should be gone")
    # セグメントは残っているので、クエリは台帳が無くても答えられる。
    assert_eq(e2e_segments(arch), segs)

    assert_eq(cmd_catalog(arch, "repair"), 0u64)
    val back = catalog::load(arch, &crc)
    assert_eq(back.size(), segs)
}

# 保持期限はセグメント単位。素材は 2026-09-03 なので、窓を 1 日に
# すれば全部が古い。**ファイルが消え、台帳からも消える**こと。
test "retention drops the segments that fell out of the window" {
    val logs = e2e_logs("build/e2e-retain", 120u64)
    val arch = "build/e2e-retain-arch"
    e2e_wipe(arch)
    assert_eq(cmd_archive(logs.to_str(), arch, 100u64), 0u64)
    assert(e2e_segments(arch) > 0u64, "there should be something to drop")

    assert_eq(cmd_retain(arch, 1u64), 0u64)
    assert_eq(e2e_segments(arch), 0u64)
    val crc = Crc32::new()
    val c = catalog::load(arch, &crc)
    assert(c.is_empty(), "the catalog should not keep rows for files that are gone")

    # 窓の広い保持は何も落とさない (2 回目が冪等であること)。
    assert_eq(cmd_retain(arch, 36500u64), 0u64)
    assert_eq(e2e_segments(arch), 0u64)
}

# 健全なアーカイブしか通らないなら、`verify` は「読めた」としか
# 言っていない。**壊れているものを壊れていると言う**ところまでが
# このコマンドの仕事なので、1 バイト書き換えた写しを食わせる。
# (ブロック単位の拒否そのものは `tests/segment_format.t` の担当。
# ここは `verify` の**報告と終了コード**。)
fn e2e_first_segment(spec: str) -> String {
    var segs: Vec<String> = Vec::new()
    mount::segments_of(spec, &mut segs)
    if segs.size() == 0u64 { panic("e2e: no segment under {spec}") }
    # 借りたままでは返せない — `segs` はこの関数で死ぬ ([E0026])。
    # 呼び出し側が持ち続ける 1 本なので、写しを渡す。
    val first: &String = segs.borrow(0u64)
    val mine: String = first.clone()
    mine
}

# `path` の `at` バイト目を 1 ビット反転して書き戻す。
unsafe fn e2e_flip_byte(path: &String, at: u64) {
    val ps = path.to_str()
    val sized = fs::file_size(ps)
    val n: u64 = match sized {
        Result::Ok(v) => v,
        Result::Err(e) => { panic("e2e: cannot size {ps}: {e}") }
    }
    var buf = ByteWriter::with_capacity(n + 16u64)
    buf.reserve(n)
    val room = buf.room()
    val window: Span<u8> = match room {
        Option::Some(s) => s,
        Option::None => { panic("e2e: cannot hold {n} bytes") }
    }
    val got = io::read_file_into(ps, window)
    val read: u64 = match got {
        Result::Ok(v) => v,
        Result::Err(e) => { panic("e2e: cannot read {ps}: {e}") }
    }
    buf.set_len(read)

    var out = ByteWriter::with_capacity(read + 16u64)
    var i = 0u64
    while i < read {
        val b = buf.byte_at(i)
        if i == at { out.put_u8(b ^ 1u8) } else { out.put_u8(b) }
        i = i + 1u64
    }
    val sp = out.span()
    match sp {
        Option::Some(bytes) => {
            val wrote = io::write_file_bytes(ps, bytes)
            match wrote {
                Result::Ok(k) => { }
                Result::Err(e) => { panic("e2e: cannot write {ps}: {e}") }
            }
        }
        Option::None => { panic("e2e: empty segment") }
    }
}

test "verify says so when a segment's bytes changed under it" {
    val logs = e2e_logs("build/e2e-bad", 120u64)
    val arch = "build/e2e-bad-arch"
    e2e_wipe(arch)
    assert_eq(cmd_archive(logs.to_str(), arch, 100u64), 0u64)
    assert_eq(cmd_verify(arch), 0u64)

    # レコード表の途中を 1 ビット。ヘッダは無傷なので、セグメントは
    # 開けるが中身が合わない — 「読めない」ではなく「合わない」を
    # 見つけられるかが要点。
    val seg = e2e_first_segment(arch)
    val opened = File::open(seg.to_str())
    var at = 0u64
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(4096u64)
            val h = segfile::head_of(&f, &mut scratch)
            assert(h.ok, "the fixture segment should read")
            at = h.recs_off + h.recs_len / 2u64
        }
        Result::Err(e) => { panic("cannot open {seg.to_str()}: {e}") }
    }
    e2e_flip_byte(&seg, at)

    assert_eq(cmd_verify(arch), 1u64)
}

# 残りのサブコマンド。`scan` は書き出す前のログを、`fields` は
# 値の分布を、`object` は 1 つの値の素性を答える。どれも
# **索引と全走査の 2 経路**を持つか、引数の検査を持つので、
# 「通る形」と「断る形」を 1 つずつ踏む。
test "scan, fields and object answer about what was archived" {
    val logs = e2e_logs("build/e2e-rest", 120u64)
    val arch = "build/e2e-rest-arch"
    e2e_wipe(arch)
    assert_eq(cmd_scan(logs.to_str(), 10u64), 0u64)
    assert_eq(cmd_archive(logs.to_str(), arch, 100u64), 0u64)

    # `fields` は索引版と全走査版の両方が同じ道を通る
    # (答えが一致することは `tests/index_scan.t` の担当)。
    assert_eq(cmd_fields_indexed(arch, "status", 5u64), 0u64)
    assert_eq(cmd_fields(arch, "status", 5u64), 0u64)
    # ラベルの形をしていないキーは索引版が断る。
    assert_eq(cmd_fields_indexed(arch, "Status", 5u64), 1u64)
    # 全走査版が知っているのは 9 つの名前だけ。
    assert_eq(cmd_fields(arch, "app", 5u64), 1u64)

    # 1 つの値の素性。素材のステータスは 200 と 204 だけなので、
    # **在る値**と**無い値**で答えが分かれることまで見る
    # ("not in this archive" は 1 で返る)。
    assert_eq(cmd_object(arch, "status=200"), 0u64)
    assert_eq(cmd_object(arch, "status=404"), 1u64)
    # 2 つ以上のトークンは断る — 落とした方が答えに見えてしまう。
    assert_eq(cmd_object(arch, "status=200 method=GET"), 1u64)
    assert_eq(cmd_object(arch, "just-some-text"), 1u64)
}
