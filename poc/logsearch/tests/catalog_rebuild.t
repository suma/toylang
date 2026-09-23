# DATA_MODEL.md §6 / STORAGE_FORMAT.md §1 — カタログはキャッシュである。
#
# 台帳が無くても答えは出せなければならない。`tests/catalog.t` は
# 「壊れた journal の尾」「カタログを持たないマウント」を押さえて
# いるが、**セグメントのファイルだけが在る**状態から台帳を作り直す
# 経路 (`catalog repair` が呼ぶ `rebuild`) は固定されていなかった。
# ここはその 1 本道 — ファイル → rebuild → 書き出し → load — が
# 同じ答えに着くことを言う。
#
# 素材は合成で、アドレスはプライベート帯だけ (CLAUDE.md)。

fn cr_line(text: str, out: &mut String) {
    out.push_str(text)
    out.push(10u8)
}

fn cr_joined(a: str, b: str) -> String {
    val head = String::from_str(a)
    val tail = String::from_str(b)
    val out = head.concat(&tail)
    out
}

# 1 セグメントぶんの素材。`n` 行で、時刻は 1 秒ずつ進む。
fn cr_fixture(n: u64, first_sec: u64) -> String {
    var s = String::new()
    var i = 0u64
    while i < n {
        val sec = first_sec + i
        cr_line("10.0.0.{1u64 + i % 7u64} - - [03/Sep/2026:12:00:{sec} +0000] \"GET /p{i % 5u64} HTTP/1.1\" 200 {i} \"-\" \"curl/8.0\"", &mut s)
        i = i + 1u64
    }
    s
}

# `path` (拡張子なし) に 1 セグメントを書き、行数を返す。
fn cr_build(stem: str, n: u64, first_sec: u64, segid: u64) -> u64 {
    val body = cr_fixture(n, first_sec)
    val log = cr_joined(stem, ".log")
    val wrote = io::write_file(log.to_str(), body.to_str())
    match wrote {
        Result::Ok(k) => { }
        Result::Err(e) => { panic("fixture: cannot write {log.to_str()}: {e}") }
    }
    var reader = LogReader::with_capacity(262144u64)
    var rec = ParsedLine::new()
    var w = ArchiveWriter::new()
    val crc = Crc32::new()
    val loaded = reader.load(log.to_str())
    match loaded {
        Result::Ok(k) => { }
        Result::Err(e) => { panic("fixture: cannot read back: {e}") }
    }
    var count = 0u64
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
                            count = count + 1u64
                        }
                        Option::None => { }
                    }
                }
            }
            Option::None => { more = false }
        }
    }
    val done = w.finish(stem, segid, &crc)
    match done {
        Result::Ok(k) => { }
        Result::Err(e) => { panic("fixture: cannot write segment: {e}") }
    }
    count
}

# セグメントのファイルだけを持つマウントを作る (`meta/` は作らない)。
fn cr_mount_with_segments(mount: str) -> u64 {
    val day = "{mount}/seg/2026/09/03"
    val made = fs::mkdir_all(day)
    match made {
        Result::Ok(u) => { }
        Result::Err(e) => { panic("cannot make {day}: {e}") }
    }
    var total = 0u64
    total = total + cr_build("{day}/000000000001", 40u64, 10u64, 1u64)
    total = total + cr_build("{day}/000000000002", 25u64, 30u64, 2u64)
    total
}

# ---------------------------------------------------------------------

test "a mount that has only segment files rebuilds its catalog from them" {
    val mount = "build/catalog-rebuilt"
    val records = cr_mount_with_segments(mount)
    val crc = Crc32::new()

    # 台帳は無い。まず「無い」ことを言う — 下の rebuild が
    # 「元から在った」ものを読んで通るのでは検査にならない。
    val before = catalog::load(mount, &crc)
    assert(before.is_empty(), "the mount should start without a catalog")

    val c = catalog::rebuild(mount, &crc)
    assert_eq(c.size(), 2u64)
    assert_eq(c.total_records(), records)
    # 行はセグメントの中身から来る: segid / 件数 / 時刻の範囲、
    # そして daykey は**パス**から (ファイルがどこに在るか)。
    val first = c.find(1u64)
    val at: u64 = match first {
        Option::Some(i) => i,
        Option::None => { panic("segment 1 should be in the rebuilt catalog") }
    }
    val row = c.row(at)
    assert_eq(row.records, 40u64)
    assert(row.seg_bytes > 0u64, "the row should know the file size")
    assert(row.ts_min < row.ts_max, "the row should span time")
    assert_eq(row.daykey, catalog::daykey_of(row.ts_min))
}

# 作り直した台帳を書き出せば、次の `load` が同じものを読む。
# ここが繋がっていないと、`catalog repair` は毎回走り直しになる。
test "a rebuilt catalog survives being written and loaded again" {
    val mount = "build/catalog-rebuilt-saved"
    cr_mount_with_segments(mount)
    val crc = Crc32::new()
    val c = catalog::rebuild(mount, &crc)
    assert(catalog::write_generation(mount, &c, 1u64, &crc), "cannot write generation 1")

    val back = catalog::load(mount, &crc)
    assert_eq(back.size(), c.size())
    assert_eq(back.total_records(), c.total_records())
    assert_eq(back.total_bytes(), c.total_bytes())
    var i = 0u64
    while i < c.size() {
        val a = c.row(i)
        val found = back.find(a.segid)
        val j: u64 = match found {
            Option::Some(k) => k,
            Option::None => { panic("segment {a.segid} did not survive the round trip") }
        }
        val b = back.row(j)
        assert_eq(b.records, a.records)
        assert_eq(b.seg_bytes, a.seg_bytes)
        assert_eq(b.ts_min, a.ts_min)
        assert_eq(b.ts_max, a.ts_max)
        assert_eq(b.daykey, a.daykey)
        i = i + 1u64
    }
}

# セグメントでないファイルが `seg/` に紛れても、rebuild は残りを返す。
# 「1 つ読めないから台帳ごと作れない」では、復旧の道具にならない。
test "a file that is not a segment is skipped, not fatal" {
    val mount = "build/catalog-rebuilt-junk"
    cr_mount_with_segments(mount)
    val junk = "{mount}/seg/2026/09/03/000000000009.seg"
    val wrote = io::write_file(junk, "this is not a segment\n")
    match wrote {
        Result::Ok(n) => { }
        Result::Err(e) => { panic("cannot write the junk file: {e}") }
    }
    val crc = Crc32::new()
    val c = catalog::rebuild(mount, &crc)
    assert_eq(c.size(), 2u64)
    val missing = c.find(9u64)
    match missing {
        Option::Some(i) => { panic("the junk file should not have become a row") }
        Option::None => { }
    }
}
