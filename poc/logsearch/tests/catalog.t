# STORAGE_FORMAT.md §6〜§8 — カタログが「捨てても作り直せるキャッシュ」
# として振る舞うか。
#
# ここで固めたいのは速さではなく**壊れ方**である。カタログは
# `fsync` せずに追記され、途中で落ちる。落ちたあとに読めるものが
# 「落ちる前に書けたところまで」で、それより多くも少なくもないこと —
# それが無ければ `--repair` にしか頼れなくなる。
#
# 素材は合成で、アドレスはすべてプライベート (CLAUDE.md の規約)。

import std.fs
import std.io
import std.path
import catalog
import logdir

fn row(segid: u64, ts_min: i64, ts_max: i64, records: u64) -> CatRow {
    var r = CatRow::empty()
    r.ok = true
    r.segid = segid
    r.ts_min = ts_min
    r.ts_max = ts_max
    r.records = records
    r.seg_bytes = records * 64u64
    r.index_bytes = records * 3u64
    r.streams = 2u64
    r.terms = 7u64
    r.daykey = catalog::daykey_of(ts_min)
    r
}

fn same(a: &CatRow, b: &CatRow) -> bool {
    if a.segid != b.segid { return false }
    if a.ts_min != b.ts_min { return false }
    if a.ts_max != b.ts_max { return false }
    if a.records != b.records { return false }
    if a.seg_bytes != b.seg_bytes { return false }
    if a.index_bytes != b.index_bytes { return false }
    if a.kind != b.kind { return false }
    if a.streams != b.streams { return false }
    if a.terms != b.terms { return false }
    if a.daykey != b.daykey { return false }
    true
}

# 前回の実行が残したものを消す。テストは決まった場所に書くので、
# 世代が残っていると `latest_gen` が別の答えを出す。
fn wipe(dir: str) {
    val listing = fs::list_dir(dir)
    match listing {
        Result::Ok(names) => {
            var i: u64 = 0u64
            while i < names.size() {
                val nm: String = names.get(i)
                val full = path::join(dir, nm.to_str())
                val gone = fs::remove_file(full.to_str())
                match gone {
                    Result::Ok(u) => { }
                    Result::Err(e) => { }
                }
                i = i + 1u64
            }
        }
        Result::Err(e) => { }
    }
}

fn fresh_mount(name: str) -> String {
    val mount = "build/{name}"
    val meta = "{mount}/meta"
    val tmp = "{mount}/tmp"
    val a = fs::mkdir_all(meta)
    match a {
        Result::Ok(u) => { }
        Result::Err(e) => { panic("mkdir {meta}: {e}") }
    }
    val b = fs::mkdir_all(tmp)
    match b {
        Result::Ok(u) => { }
        Result::Err(e) => { panic("mkdir {tmp}: {e}") }
    }
    wipe(meta)
    wipe(tmp)
    val out = String::from_str(mount)
    out
}

# ---------------------------------------------------------------------

# 往復。64 バイト固定長なので、フィールドが 1 つずれれば全部ずれる。
test "a snapshot gives back every row it was given" {
    val crc = Crc32::new()
    var c = Catalog::new()
    val r1 = row(1u64, 1756900000i64, 1756900100i64, 1000u64)
    val r2 = row(2u64, 1756900100i64, 1756900200i64, 2000u64)
    val r3 = row(3u64, 1756900200i64, 1756900300i64, 3000u64)
    c.add(&r1)
    c.add(&r2)
    c.add(&r3)

    var w = ByteWriter::with_capacity(512u64)
    catalog::encode_snapshot(&c, 4u64, &mut w, &crc)
    assert_eq(w.len(), catalog::snap_head_bytes() + 3u64 * catalog::row_bytes())

    var back = Catalog::new()
    val sp = w.span()
    match sp {
        Option::Some(b) => {
            val gen = catalog::decode_snapshot(b, w.len(), &mut back, &crc)
            assert_eq(gen, 4u64)
        }
        Option::None => { panic("snapshot buffer should not be empty") }
    }
    assert_eq(back.size(), 3u64)
    val g1 = back.row(0u64)
    val g3 = back.row(2u64)
    assert(same(&g1, &r1), "row 1 should come back unchanged")
    assert(same(&g3, &r3), "row 3 should come back unchanged")
}

# 1 バイトでも変われば、そのスナップショットは採らない。**部分的に
# 正しいカタログを返さない**のが要点で、疑わしければ `seg/` を歩き
# 直せばよい。
test "a snapshot that does not check out is refused whole" {
    val crc = Crc32::new()
    var c = Catalog::new()
    val r1 = row(1u64, 1756900000i64, 1756900100i64, 1000u64)
    val r2 = row(2u64, 1756900100i64, 1756900200i64, 2000u64)
    c.add(&r1)
    c.add(&r2)

    var w = ByteWriter::with_capacity(512u64)
    catalog::encode_snapshot(&c, 1u64, &mut w, &crc)
    # 2 行目のレコード数を書き換える。行の CRC も全体の CRC も外れる。
    w.patch_u64(catalog::snap_head_bytes() + catalog::row_bytes() + 24u64, 9u64)

    var back = Catalog::new()
    val sp = w.span()
    match sp {
        Option::Some(b) => {
            val gen = catalog::decode_snapshot(b, w.len(), &mut back, &crc)
            assert_eq(gen, 0u64)
        }
        Option::None => { panic("snapshot buffer should not be empty") }
    }
    assert_eq(back.size(), 0u64)
}

# QUERY.md §2 の枝刈り。`ts_max` は**実在するレコードの時刻**なので
# 上端は閉じている — `<` で書くと、最後の 1 行だけが当たるセグメントが
# 落ちる。
test "pruning keeps the segment whose only match is its last record" {
    var c = Catalog::new()
    val early = row(1u64, 100i64, 199i64, 10u64)
    val mid = row(2u64, 200i64, 299i64, 10u64)
    val late = row(3u64, 300i64, 399i64, 10u64)
    c.add(&early)
    c.add(&mid)
    c.add(&late)

    var hits: Vec<u64> = Vec::new()
    # [199, 300) — early の最後の 1 行と mid 全体。
    c.select(199i64, 300i64, &mut hits)
    assert_eq(hits.size(), 2u64)
    assert_eq(hits.get(0u64), 0u64)
    assert_eq(hits.get(1u64), 1u64)

    # 隙間だけを指す範囲は 1 本も残さない。
    c.select(200i64, 200i64, &mut hits)
    assert_eq(hits.size(), 0u64)
}

# 保持期限はセグメント単位。新しい行が 1 本でもあれば残る
# (DATA_MODEL.md §4: 行単位の削除は無い)。
test "retention will not drop a segment that still holds a fresh record" {
    var c = Catalog::new()
    val old = row(1u64, 100i64, 199i64, 10u64)
    val straddling = row(2u64, 150i64, 250i64, 10u64)
    c.add(&old)
    c.add(&straddling)

    var dead: Vec<u64> = Vec::new()
    c.expired(200i64, &mut dead)
    assert_eq(dead.size(), 1u64)
    assert_eq(dead.get(0u64), 0u64)
}

# 同じ `segid` を 2 度足すと**置き換わる**。公開の rename とジャーナル
# 追記の間で落ちた場合、次の起動が同じセグメントをもう一度足すので、
# ここが append だと行が二重になる。
test "adding the same segid twice replaces the row" {
    var c = Catalog::new()
    val first = row(7u64, 100i64, 199i64, 10u64)
    val again = row(7u64, 100i64, 199i64, 99u64)
    c.add(&first)
    c.add(&again)
    assert_eq(c.size(), 1u64)
    val got = c.row(0u64)
    assert_eq(got.records, 99u64)
}

# ---------------------------------------------------------------------

test "a journal replays the adds and removes that were appended" {
    val crc = Crc32::new()
    val m = fresh_mount("catalog-journal")
    val mount = m.to_str()

    var c = Catalog::new()
    assert(catalog::write_generation(mount, &c, 1u64, &crc), "gen 1 should publish")

    val r1 = row(1u64, 100i64, 199i64, 10u64)
    val r2 = row(2u64, 200i64, 299i64, 20u64)
    assert(catalog::append_add(mount, 1u64, &r1, &crc), "ADD 1 should append")
    assert(catalog::append_add(mount, 1u64, &r2, &crc), "ADD 2 should append")
    assert(catalog::append_remove(mount, 1u64, 1u64, catalog::why_retention(), &crc),
           "REMOVE 1 should append")

    val back = catalog::load(mount, &crc)
    assert_eq(back.generation(), 1u64)
    assert_eq(back.applied(), 3u64)
    assert_eq(back.size(), 1u64)
    val got = back.row(0u64)
    assert(same(&got, &r2), "the surviving row should be segment 2")
}

# 追記は `fsync` していないので、落ちた直後の末尾は欠ける。欠けた
# ところで止まり、**その手前までは残る**。
test "a torn journal tail stops the replay where the bytes stop" {
    val crc = Crc32::new()
    val m = fresh_mount("catalog-torn")
    val mount = m.to_str()

    var c = Catalog::new()
    assert(catalog::write_generation(mount, &c, 1u64, &crc), "gen 1 should publish")
    val r1 = row(1u64, 100i64, 199i64, 10u64)
    val r2 = row(2u64, 200i64, 299i64, 20u64)
    assert(catalog::append_add(mount, 1u64, &r1, &crc), "ADD 1 should append")
    assert(catalog::append_add(mount, 1u64, &r2, &crc), "ADD 2 should append")

    # 3 本目を書き始めたところで電源が落ちた形。
    val lp = catalog::log_path(mount, 1u64)
    val torn = io::append_file(lp.to_str(), "LSJ1\u{00}\u{00}")
    match torn {
        Result::Ok(n) => { }
        Result::Err(e) => { panic("cannot tear the journal: {e}") }
    }

    val back = catalog::load(mount, &crc)
    assert_eq(back.applied(), 2u64)
    assert_eq(back.size(), 2u64)
}

# 世代交代。新しい世代が公開されてから古い世代を消すので、途中で
# 落ちてもどちらか一方が完全な形で残る。
test "compaction folds the journal into the next generation" {
    val crc = Crc32::new()
    val m = fresh_mount("catalog-compact")
    val mount = m.to_str()

    var c = Catalog::new()
    assert(catalog::write_generation(mount, &c, 1u64, &crc), "gen 1 should publish")
    val r1 = row(1u64, 100i64, 199i64, 10u64)
    assert(catalog::append_add(mount, 1u64, &r1, &crc), "ADD should append")

    var live = catalog::load(mount, &crc)
    assert_eq(live.applied(), 1u64)
    assert(catalog::compact(mount, &mut live, &crc), "compaction should publish gen 2")
    assert_eq(live.generation(), 2u64)
    assert_eq(live.applied(), 0u64)
    assert_eq(catalog::latest_gen(mount), 2u64)

    # 古い世代は消えている。
    val old = catalog::snap_path(mount, 1u64)
    assert(!fs::is_file(old.to_str()), "the old snapshot should be gone")
    val oldlog = catalog::log_path(mount, 1u64)
    assert(!fs::is_file(oldlog.to_str()), "the old journal should be gone")

    # 新しい世代だけを読んで、同じ 1 行が出る。
    val again = catalog::load(mount, &crc)
    assert_eq(again.generation(), 2u64)
    assert_eq(again.applied(), 0u64)
    assert_eq(again.size(), 1u64)
    val got = again.row(0u64)
    assert(same(&got, &r1), "the row should survive the generation change")
}

# カタログが無くても開ける。これが「キャッシュである」の運用上の意味。
test "a mount with no catalog opens empty rather than failing" {
    val crc = Crc32::new()
    val m = fresh_mount("catalog-absent")
    val mount = m.to_str()
    assert_eq(catalog::latest_gen(mount), 0u64)
    val c = catalog::load(mount, &crc)
    assert_eq(c.generation(), 0u64)
    assert_eq(c.size(), 0u64)
}
