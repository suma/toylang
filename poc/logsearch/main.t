# logsearch -- read logs, and turn what was read into an archive.
#
# Three subcommands, all of them read-only towards the logs:
#
#   scan    <dir> [limit]          what is there, and how it framed
#   archive <dir> <out> [limit]    parse it and compress it into segments
#   query   <out> "<query>"        search the segments
#   fields  <out> <field> [limit]   count the values of one field
#   object  <out> "<key>=<value>"  one object: count, first / last seen
#   verify  <out>                  read every segment back and check it
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
import extract
import logdir
import query
import record
import segfile

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

# `YYYY/MM/DD` for the segment's own day, so that retention can drop
# one directory instead of hunting for files (STORAGE_FORMAT.md §1).
fn day_dir(out: str, secs: i64) -> String {
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
        val path: String = files.get(fi)
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
                            record::parse_line(&reader, l, &mut rec)
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

fn cmd_archive(dir: str, out: str, limit: u64) -> u64 {
    println("archiving {dir} -> {out}")
    val files = logdir::scan(dir)
    val n_files = files.size()
    if n_files == 0u64 {
        println("no log files found")
        return 1u64
    }

    var reader = LogReader::with_capacity(BUF_BYTES)
    var rec = ParsedLine::new()
    var w = ArchiveWriter::new()
    val crc = Crc32::new()
    val watch = Stopwatch::start()

    var segid: u64 = 1u64
    var raw_total: u64 = 0u64
    var dat_total: u64 = 0u64
    var records_total: u64 = 0u64
    var segments: u64 = 0u64
    var bad: u64 = 0u64
    var read_files: u64 = 0u64

    var fi: u64 = 0u64
    while fi < n_files && read_files < limit {
        val path: String = files.get(fi)
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
                            record::parse_line(&reader, l, &mut rec)
                            val win = reader.span()
                            match win {
                                Option::Some(sp) => { w.add(sp, l, &rec) }
                                Option::None => { }
                            }
                            if w.is_full() {
                                val done = flush_segment(&mut w, out, segid, &crc)
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
        val done = flush_segment(&mut w, out, segid, &crc)
        if done == 0u64 { bad = bad + 1u64 }
        raw_total = raw_total + w.arena_bytes()
        records_total = records_total + w.count()
        dat_total = dat_total + done
        segments = segments + 1u64
        w.reset()
    }

    val ms = watch.elapsed_ms()
    println("")
    println("segments         {segments} ({bad} failed)")
    println("records          {records_total}")
    println("arena bytes      {raw_total}")
    println("archive bytes    {dat_total}")
    if raw_total > 0u64 {
        val pct = (dat_total * 100u64) / raw_total
        println("ratio            {pct}% of raw")
    }
    println("elapsed          {ms} ms")
    0u64
}

# Write one segment and say how many bytes its `.seg` came to, or 0
# when it could not be written.
fn flush_segment(w: &mut ArchiveWriter, out: str, segid: u64, crc: &Crc32) -> u64 {
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
    val records = w.count()
    val arena = w.arena_bytes()
    val pct = if arena > 0u64 { (n * 100u64) / arena } else { 0u64 }
    println("  {base}.seg  {records} records  {arena} B -> {n} B ({pct}%)")
    n
}

# ---------------------------------------------------------------------

fn cmd_verify(out: str) -> u64 {
    println("verifying {out}")
    val segs = logdir::scan_suffix(out, ".seg")
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
        val p: String = segs.get(i)
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

# `<key>=<value> top=<field>`: what this one value is linked to.
#
# The answer comes out of the link section -- pairs of field values
# that appeared on the same record, counted when the segment was
# written (ONTOLOGY.md O1). No records are read and no frames are
# expanded: the traversal is a walk of one group of link rows.
fn cmd_top_linked(dir: str, q: &Query, field: str, limit: u64) -> u64 {
    val segs = logdir::scan_suffix(dir, ".seg")
    if segs.size() == 0u64 {
        println("no segments under {dir}")
        return 1u64
    }
    val anchor: String = q.terms.get(0u64)
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
        val seg_path: String = segs.get(si)
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
        val nm: String = names.get(t.idx)
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
        val tok: String = parts.get(i)
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
        val chosen: String = top_fields.get(0u64)
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
    val n = query::run(dir, &q, &crc)
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
    val code = query::field_code(field)
    if code == query::field_none() {
        println("unknown field `{field}`")
        return 1u64
    }
    println("field {field} (index)")

    val segs = logdir::scan_suffix(dir, ".seg")
    val n_segs = segs.size()
    if n_segs == 0u64 {
        println("no segments under {dir}")
        return 1u64
    }

    val watch = Stopwatch::start()
    val crc = Crc32::new()
    var head_buf = ByteWriter::with_capacity(segfile::data_at() + 64u64)
    var raw = ByteWriter::with_capacity(4194304u64)
    var tsec = ByteWriter::with_capacity(4194304u64)
    # Values repeat across segments, so they are folded through an
    # open-addressing table. A linear scan over the values seen so far
    # was the first attempt and it is quadratic: for `ip` (11,293
    # distinct) it cost more than the full scan the index replaces.
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
    var terms_seen: u64 = 0u64
    var indexed_segs: u64 = 0u64

    var prefix = "status:"
    if code == query::field_method() { prefix = "method:" }
    if code == query::field_path() { prefix = "path:" }
    if code == query::field_client() { prefix = "ip:" }
    if code == query::field_vhost() { prefix = "vhost:" }
    if code == query::field_ua() { prefix = "ua:" }
    if code == query::field_host() { prefix = "host:" }
    if code == query::field_tag() { prefix = "tag:" }

    var si: u64 = 0u64
    while si < n_segs {
        val seg_path: String = segs.get(si)
        val seg_str = seg_path.to_str()
        si = si + 1u64
        val opened_f = File::open(seg_str)
        match opened_f {
            Result::Ok(f) => {
                # The dictionary answers this on its own: one section
                # is read, no frame is touched, and the arena is never
                # expanded.
                val h = segfile::head_of(&f, &mut head_buf)
                var got = h.ok && h.has_terms()
                if got {
                    indexed_segs = indexed_segs + 1u64
                    if !segfile::load_block(&f, h.terms_off, h.terms_len, &crc, &mut raw, &mut tsec) { got = false }
                }
                if got {
                        var raw_len: u64 = 0u64
                        val tw = tsec.span()
                        match tw {
                        Option::Some(traw) => {
                        raw_len = tsec.len()
                        var head = ByteReader::new(raw_len)
                        terms_seen = terms_seen + head.take_u32(traw)
                        val hits = archive::terms_with_prefix(traw, 0u64, raw_len, prefix)
                        var h: u64 = 0u64
                        while h < hits.names.size() {
                            val packed: u64 = hits.names.get(h)
                            val at = record::span_start(packed)
                            val vlen = record::span_len(packed)
                            val c: u64 = hits.counts.get(h)
                            # Values repeat across segments, so they
                            # are folded here by the same hash trick
                            # the scan uses.
                            val key = extract::hash_span(traw, at, vlen)
                            val mask = slot_bits - 1u64
                            var slot = key & mask
                            var placed = false
                            while !placed {
                                val cell: u64 = slots.get(slot)
                                if cell == 0u64 {
                                    val pos = hashes.size()
                                    hashes.push(key)
                                    counts.push(c)
                                    val nm = query::text_of(traw, at, vlen)
                                    names.push(nm)
                                    slots.set(slot, pos + 1u64)
                                    placed = true
                                } else {
                                    val pos = cell - 1u64
                                    val hv: u64 = hashes.get(pos)
                                    if hv == key {
                                        val prev: u64 = counts.get(pos)
                                        counts.set(pos, prev + c)
                                        placed = true
                                    } else {
                                        slot = (slot + 1u64) & mask
                                    }
                                }
                            }
                            h = h + 1u64
                        }
                        }
                        Option::None => { }
                        }
                }
            }
            Result::Err(e) => { println("  {seg_str}: {e}") }
        }
    }

    if indexed_segs == 0u64 {
        println("no term index in these segments -- rebuild the archive")
        return 1u64
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
        val nm: String = names.get(t.idx)
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

fn cmd_fields(dir: str, field: str, limit: u64) -> u64 {
    val code = query::field_code(field)
    if code == query::field_none() {
        println("unknown field `{field}` -- try status / method / path / ip / vhost / ua / proto / host / tag")
        return 1u64
    }
    println("field {field}")

    val segs = logdir::scan_suffix(dir, ".seg")
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
        val seg_path: String = segs.get(si)
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

                                                    if flen > 0u64 {
                                                        carried = carried + 1u64
                                                        val h = extract::hash_span(arena, at, flen)
                                                        val seen = index_of.get(h)
                                                        match seen {
                                                            Option::Some(pos) => {
                                                                val c: u64 = counts.get(pos)
                                                                counts.set(pos, c + 1u64)
                                                            }
                                                            Option::None => {
                                                                val pos = names.size()
                                                                val nm = query::text_of(arena, at, flen)
                                                                names.push(nm)
                                                                counts.push(1u64)
                                                                index_of.insert(h, pos)
                                                            }
                                                        }
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
        val nm: String = names.get(t.idx)
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
    val want: String = q.terms.get(0u64)
    println("object {want}")

    val segs = logdir::scan_suffix(dir, ".seg")
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
        val seg: String = segs.get(si)
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
    if mode == "scan" {
        val s_dir = arg_or(1u64, "poc/logsearch/log")
        val s_limit = arg_u64(2u64, 1000000u64)
        return cmd_scan(s_dir, s_limit)
    }
    # No subcommand: the first argument is the directory.
    val d_limit = arg_u64(1u64, 1000000u64)
    cmd_scan(mode, d_limit)
}
