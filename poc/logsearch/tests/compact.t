# DATA_MODEL.md §5 — 冷えたセグメントの併合。
#
# 併合は**答えを変えない**書き換えである。だから固めるのは速さでも
# 圧縮率でもなく、次の 3 つ:
#
#   1. **同じレコードが同じ数だけ残る** — 入力を歩いた答えと、
#      出力を歩いた答えが一致する (ROADMAP §4-3 と同じ考え方)
#   2. **索引も作り直される** — 併合後のアーカイブに対する
#      `status=200` の件数が、併合前の合計と同じ
#   3. **元は出力が在ってから消える** — 台帳にアーカイブが載り、
#      入力の行が消え、ファイルも消える
#
# 熱いセグメント (`cold_secs()` より新しいもの) は触らない。それも
# ここで言う — 触ってしまうと「今読まれているファイル」を書き換える
# ことになる。
#
# 素材は合成で、アドレスはプライベート帯だけ (CLAUDE.md)。

fn cp_line(text: str, out: &mut String) {
    out.push_str(text)
    out.push(10u8)
}

# `n` 行、時刻は `first` 秒から 1 秒ずつ。
fn cp_fixture(n: u64, first: u64) -> String {
    var s = String::new()
    var i = 0u64
    while i < n {
        val sec = first + i
        cp_line("10.0.0.{1u64 + i % 7u64} - - [03/Sep/2026:12:00:{sec % 60u64} +0000] \"GET /p{i % 5u64} HTTP/1.1\" 200 {i} \"-\" \"curl/8.0\"", &mut s)
        i = i + 1u64
    }
    s
}

# 1 セグメントを `stem` に書き、行数を返す。
fn cp_build(stem: str, n: u64, first: u64, segid: u64) -> u64 {
    val body = cp_fixture(n, first)
    val log = "{stem}.log"
    val wrote = io::write_file(log, body.to_str())
    match wrote {
        Result::Ok(k) => { }
        Result::Err(e) => { panic("fixture: cannot write {log}: {e}") }
    }
    var reader = LogReader::with_capacity(262144u64)
    var rec = ParsedLine::new()
    var w = ArchiveWriter::new()
    val crc = Crc32::new()
    val loaded = reader.load(log)
    match loaded {
        Result::Ok(k) => { }
        Result::Err(e) => { panic("fixture: cannot read back {log}: {e}") }
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

# 前の実行が残したものを消す。
#
# このテストは**マウントを書き換える** (入力が消え、アーカイブが
# 増える) ので、素材を置き直すだけでは前回のアーカイブが残り、
# 2 回目の実行が 4 本のセグメントを見ることになる。`build/` は
# 実行を跨いで残るという `toy test` の前提の裏返しで、書き換える
# 側のテストは自分で掃除する。
fn cp_wipe(mount: str) {
    var segs: Vec<String> = Vec::new()
    mount::segments_of(mount, &mut segs)
    var i: u64 = 0u64
    while i < segs.size() {
        val p: &String = segs.borrow(i)
        val rm = fs::remove_file(p.to_str())
        match rm {
            Result::Ok(u) => { }
            Result::Err(e) => { }
        }
        i = i + 1u64
    }
    # 台帳も (世代とジャーナルが前回の segid を覚えている)。
    val meta = catalog::meta_path(mount)
    val listing = fs::list_dir(meta.to_str())
    match listing {
        Result::Ok(names) => {
            var k: u64 = 0u64
            while k < names.size() {
                val nm: &String = names.borrow(k)
                val full = "{meta.to_str()}/{nm.to_str()}"
                val rm2 = fs::remove_file(full)
                match rm2 {
                    Result::Ok(u) => { }
                    Result::Err(e) => { }
                }
                k = k + 1u64
            }
        }
        Result::Err(e) => { }
    }
}

# セグメント 3 本を持つマウントを作り、台帳に載せる。行数を返す。
fn cp_mount(mount: str) -> u64 {
    cp_wipe(mount)
    val day = "{mount}/seg/2026/09/03"
    val made = fs::mkdir_all(day)
    match made {
        Result::Ok(u) => { }
        Result::Err(e) => { panic("cannot make {day}: {e}") }
    }
    var total = 0u64
    total = total + cp_build("{day}/000000000001", 30u64, 0u64, 1u64)
    total = total + cp_build("{day}/000000000002", 25u64, 100u64, 2u64)
    total = total + cp_build("{day}/000000000003", 20u64, 200u64, 3u64)
    val crc = Crc32::new()
    val c = catalog::rebuild(mount, &crc)
    assert_eq(c.size(), 3u64)
    assert(catalog::write_generation(mount, &c, 1u64, &crc), "the catalog should publish")
    total
}

# マウントの全セグメントに対する語の件数。併合の前後で変わっては
# ならない。
fn cp_hits(mount: str, term: str) -> u64 {
    val crc = Crc32::new()
    var segs: Vec<String> = Vec::new()
    val c = catalog::load(mount, &crc)
    var i: u64 = 0u64
    while i < c.size() {
        val r: CatRow = c.row(i)
        val p = catalog::seg_path(mount, &r)
        segs.push(p)
        i = i + 1u64
    }
    val q = query::parse_query("{term} limit=1000", 0i64)
    var hits: Vec<Hit> = Vec::new()
    var texts: Vec<String> = Vec::new()
    var st = SearchStats::new()
    query::search(mount, &segs, &q, &crc, &mut hits, &mut texts, &mut st)
    hits.size()
}

fn cp_files(mount: str) -> u64 {
    var segs: Vec<String> = Vec::new()
    mount::segments_of(mount, &mut segs)
    segs.size()
}

# ---------------------------------------------------------------------

# 併合しても答えは変わらない。**索引を作り直している**ので、
# アーカイブに対する語の検索が元の合計と一致しなければならない。
test "merging three segments keeps every record and every answer" {
    val mount = "build/compact-agree"
    val records = cp_mount(mount)
    val crc = Crc32::new()
    # 「併合の前後で同じ」が主張なので、基準は実測しておく
    # (`limit` や重複の扱いを固定するのはここの仕事ではない)。
    val before = cp_hits(mount, "status=200")
    val before_p3 = cp_hits(mount, "path=/p3")
    assert(before > 0u64, "the fixture should answer something")
    assert(before_p3 > 0u64, "and something narrower")
    assert_eq(cp_files(mount), 3u64)

    # 冷えているとみなすために、時計を十分先に進めて渡す。
    val now = 4000000000i64
    val done = compact::compact_once(mount, now, &crc)
    assert(done.is_ok(), "the merge should succeed")
    assert_eq(done.merged(), 3u64)
    assert_eq(done.records(), records)

    # 台帳はアーカイブ 1 本だけ。
    val c = catalog::load(mount, &crc)
    assert_eq(c.size(), 1u64)
    val r: CatRow = c.row(0u64)
    assert_eq(r.kind, catalog::kind_archive())
    assert_eq(r.records, records)

    # ファイルも 1 本 — 入力は消えている。
    assert_eq(cp_files(mount), 1u64)

    # そして答えが変わっていない。
    assert_eq(cp_hits(mount, "status=200"), before)
    assert_eq(cp_hits(mount, "path=/p3"), before_p3)
}

# 熱いものは触らない。**今読まれているファイル**を書き換えないのが
# 併合の前提で、`cold_secs()` はそのための境界である。
test "a segment that is still warm is left alone" {
    val mount = "build/compact-warm"
    val records = cp_mount(mount)
    val crc = Crc32::new()

    # 素材の時刻は 2026-09-03。その直後なら、どれもまだ冷えていない。
    val just_after = 1757000000i64
    val done = compact::compact_once(mount, just_after, &crc)
    assert(done.is_ok(), "doing nothing is not a failure")
    assert_eq(done.merged(), 0u64)
    assert_eq(cp_files(mount), 3u64)
}

# 1 本しか候補が無いなら、併合は何も得ない。
test "one cold segment is not a merge" {
    val mount = "build/compact-single"
    cp_wipe(mount)
    val day = "{mount}/seg/2026/09/03"
    val made = fs::mkdir_all(day)
    match made {
        Result::Ok(u) => { }
        Result::Err(e) => { panic("cannot make {day}: {e}") }
    }
    val n = cp_build("{day}/000000000001", 12u64, 0u64, 1u64)
    val crc = Crc32::new()
    val c = catalog::rebuild(mount, &crc)
    assert(catalog::write_generation(mount, &c, 1u64, &crc), "the catalog should publish")

    val done = compact::compact_once(mount, 4000000000i64, &crc)
    assert(done.is_ok(), "nothing to do is not a failure")
    assert_eq(done.merged(), 0u64)
    assert_eq(cp_files(mount), 1u64)
}

# ラベル辞書は同じレコードを数え直したものなので、併合の前後で
# 一致する。入力を引いて出力を足す順序が合っていなければここで割れる。
test "the label dictionary is the same after a merge" {
    val mount = "build/compact-labels"
    val records = cp_mount(mount)
    val crc = Crc32::new()
    val rebuilt = labels::rebuild_for_mount(mount, &crc)
    assert(labels::save_dict(mount, 1u64, &rebuilt, &crc), "the dictionary should publish")
    val before = rebuilt.size()
    assert(before > 0u64, "the fixture should have labels")

    val done = compact::compact_once(mount, 4000000000i64, &crc)
    assert(done.is_ok(), "the merge should succeed")
    assert_eq(done.merged(), 3u64)

    val after = labels::load_dict(mount, &crc)
    assert_eq(after.size(), before)
    val fresh = labels::rebuild_for_mount(mount, &crc)
    assert_eq(after.size(), fresh.size())
}
