# ROADMAP §4-4 — 増えないことは、増えたときにしか気づけない。
#
# このサーバは 1 プロセスで長く走る。要求ごとに少しずつ掴んだまま
# 離さないものがあれば、症状が出るのは**何時間か経ってから**で、
# そのときには原因の要求はもう終わっている。だから「同じことを
# 繰り返しても live バイトが戻る」を、繰り返しの形で書いておく。
#
# 使うのは `testing::heap_mark` / `assert_no_growth` (live バイト、
# 累積ではない)。バッファを取って返す関数は**増やしていない** —
# 確保そのものを禁じると、実装を試験することになって約束を試験
# しなくなる。
#
# **書いた日に 1 つ出た。** tree-walker だけが `str::as_ptr` の受け皿を
# 確保カウンタに載せ、しかも解放していなかったので、`String::from_str`
# を呼ぶたびに live バイトが増えていた (本体側 todo の
# STR-PTR-UNCOUNTED、2026-09-18 に修正)。今は 4 レーンとも測れる。
#
# 1 周目は測らない。初回だけは器 (Vec の backing store、読み取り
# バッファ) を確保するのが当たり前で、そこを含めると「2 周目以降は
# 一定」という肝心の性質が見えなくなる。
#
# 素材は合成で、アドレスはプライベート帯だけ (CLAUDE.md)。

fn st_line(text: str, out: &mut String) {
    out.push_str(text)
    out.push(10u8)
}

fn st_fixture() -> String {
    var s = String::new()
    var i = 0u64
    while i < 600u64 {
        st_line("10.0.0.{1u64 + i % 9u64} - - [03/Sep/2026:12:00:{i % 60u64} +0000] \u{22}GET /p{i % 11u64} HTTP/1.1\u{22} {200u64 + i % 3u64} {i} \u{22}-\u{22} \u{22}curl/8.0\u{22}", &mut s)
        i = i + 1u64
    }
    st_line("2026-09-03T12:30:00Z web01 cron[5]: job ran with a timeout of 30s", &mut s)
    s
}

fn st_build(stem: str) -> String {
    val body = st_fixture()
    val log = "{stem}.log"
    val wrote = io::write_file(log, body.to_str())
    match wrote {
        Result::Ok(n) => { }
        Result::Err(e) => { panic("fixture: cannot write {log}: {e}") }
    }
    var reader = LogReader::with_capacity(262144u64)
    var rec = ParsedLine::new()
    var w = ArchiveWriter::new()
    val crc = Crc32::new()
    val loaded = reader.load(log)
    match loaded {
        Result::Ok(n) => { }
        Result::Err(e) => { panic("fixture: cannot read back {log}: {e}") }
    }
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
                }
            }
            Option::None => { more = false }
        }
    }
    val done = w.finish(stem, 1u64, &crc)
    match done {
        Result::Ok(n) => { }
        Result::Err(e) => { panic("fixture: cannot write segment: {e}") }
    }
    val seg = String::from_str("{stem}.seg")
    seg
}


# ---------------------------------------------------------------------

# 同じクエリを何度も投げる。これがサーバの定常状態そのもので、
# 1 回ぶんでも残れば時間とともに積み上がる。
test "answering the same query again and again does not grow the heap" {
    val seg = st_build("build/steady-query")
    var segs: Vec<String> = Vec::new()
    segs.push(seg)
    val crc = Crc32::new()

    # 1 周目は器を作る。
    var warm: Vec<Hit> = Vec::new()
    var warm_texts: Vec<String> = Vec::new()
    var warm_st = SearchStats::new()
    val warm_q = query::parse_query("status=200 limit=5", 0i64)
    query::search("build", &segs, &warm_q, &crc, &mut warm, &mut warm_texts, &mut warm_st)
    assert(warm.size() > 0u64, "the warm-up query should match something")

    val mark = testing::heap_mark()
    var i = 0u64
    while i < 20u64 {
        val q = query::parse_query("status=200 limit=5", 0i64)
        var hits: Vec<Hit> = Vec::new()
        var texts: Vec<String> = Vec::new()
        var st = SearchStats::new()
        query::search("build", &segs, &q, &crc, &mut hits, &mut texts, &mut st)
        assert(hits.size() > 0u64, "round {i} should match something")
        i = i + 1u64
    }
    testing::assert_no_growth(mark)
}

# 本文を舐める形も同じ。索引で絞る形とは別の経路 (フレームを
# 展開して 1 行ずつ読む) を通るので、別に踏む。
test "scanning the text of every record leaves nothing behind" {
    val seg = st_build("build/steady-scan")
    var segs: Vec<String> = Vec::new()
    segs.push(seg)
    val crc = Crc32::new()

    var warm: Vec<Hit> = Vec::new()
    var warm_texts: Vec<String> = Vec::new()
    var warm_st = SearchStats::new()
    val warm_q = query::parse_query("timeout", 0i64)
    query::search("build", &segs, &warm_q, &crc, &mut warm, &mut warm_texts, &mut warm_st)

    val mark = testing::heap_mark()
    var i = 0u64
    while i < 20u64 {
        val q = query::parse_query("timeout", 0i64)
        var hits: Vec<Hit> = Vec::new()
        var texts: Vec<String> = Vec::new()
        var st = SearchStats::new()
        query::search("build", &segs, &q, &crc, &mut hits, &mut texts, &mut st)
        assert_eq(hits.size(), 1u64)
        i = i + 1u64
    }
    testing::assert_no_growth(mark)
}

# 集計 (`fields` / `/v1/labels`) は語彙をまるごと歩く。値ごとに
# `String` を作る経路なので、戻ってこなければここで見える。
test "tallying a field repeatedly returns to where it started" {
    val seg = st_build("build/steady-tally")
    var segs: Vec<String> = Vec::new()
    segs.push(seg)
    val crc = Crc32::new()

    val warm = query::tally(&segs, "status:", false, &crc)
    assert(warm.size() > 0u64, "the warm-up tally should find values")

    val mark = testing::heap_mark()
    var i = 0u64
    while i < 20u64 {
        val tal = query::tally(&segs, "path:", false, &crc)
        assert_eq(tal.size(), 11u64)
        i = i + 1u64
    }
    testing::assert_no_growth(mark)
}

# セグメントを開いて展開し直す経路。`raw` / `out` を使い回す形は
# **確保をしない**はずで、ここが増えるならバッファが毎回取り直されている。
test "expanding a segment with reused buffers allocates nothing" {
    val seg = st_build("build/steady-expand")
    val crc = Crc32::new()
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(65536u64)
            val h = segfile::head_of(&f, &mut scratch)
            assert(h.ok, "header")
            var raw = ByteWriter::with_capacity(524288u64)
            var out = ByteWriter::with_capacity(h.arena_bytes + 65536u64)

            # 1 周目でバッファは育ちきる。
            assert(segfile::expand_all(&f, &h, &crc, &mut raw, &mut out), "warm-up expand")

            val mark = testing::heap_mark()
            var i = 0u64
            while i < 20u64 {
                out.clear()
                assert(segfile::expand_all(&f, &h, &crc, &mut raw, &mut out), "expand {i}")
                assert_eq(out.len(), h.arena_bytes)
                i = i + 1u64
            }
            testing::assert_no_growth(mark)
        }
        Result::Err(e) => { panic("cannot open {seg.to_str()}: {e}") }
    }
}

# クエリの読み取りそのもの。`String` を何本も作って捨てる形なので、
# 「作って捨てた」が live に残らないことをここで言う。
test "parsing a query a hundred times holds nothing" {
    val warm = query::parse_query("status=404 method=POST ua~bot from=-1h limit=20", 0i64)
    assert_eq(warm.term_count(), 2u64)

    val mark = testing::heap_mark()
    var i = 0u64
    while i < 100u64 {
        val q = query::parse_query("status=404 method=POST ua~bot from=-1h limit=20", 0i64)
        assert_eq(q.indexed_count(), 3u64)
        i = i + 1u64
    }
    testing::assert_no_growth(mark)
}
