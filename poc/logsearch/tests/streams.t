# DATA_MODEL.md §3 — ストリーム (ラベル集合) の表。
#
# ストリームは**ラベル集合が完全に一致するレコードの並び**である。
# 語彙索引はキーと値の組ごとなので、集合そのものは数えられない —
# `app=api` と `level=error` がそれぞれ 10 件でも、その 2 つを同時に
# 持つ行が何件かは言えない。だから書き出し時に別の表 (kind 9) を作る。
#
# ここで固めるのは 3 つ:
#
#   1. **集合であること** — ラベルの順が違っても同じストリーム
#   2. **同じ組を 2 回書いても 1 つ** — syslog ヘッダの host と
#      `host=` ラベルが同じ値なら、1 つの組
#   3. **セグメントを跨いで畳める** — 件数は合計、時刻は端の広い方
#
# 素材は合成で、アドレスはプライベート帯だけ (CLAUDE.md)。

fn sm_line(text: str, out: &mut String) {
    out.push_str(text)
    out.push(10u8)
}

fn sm_build(stem: str, body: &String) -> String {
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

# ラベル集合 `want` の件数。無ければ 0。
fn sm_count(tal: &StreamTally, want: str) -> u64 {
    val w = String::from_str(want)
    var i = 0u64
    var out = 0u64
    while i < tal.size() {
        val text: &String = tal.texts.borrow(i)
        if text.eq(&w) { out = tal.counts.get(i) }
        i = i + 1u64
    }
    out
}

# ---------------------------------------------------------------------

# ラベルの**順**は集合を変えない。`app=api level=error` と
# `level=error app=api` は同じストリームで、合わせて 3 件。
test "a stream is a set, so the order of its labels does not matter" {
    var body = String::new()
    sm_line("2026-09-03T12:00:01Z app=api level=error one", &mut body)
    sm_line("2026-09-03T12:00:02Z level=error app=api two", &mut body)
    sm_line("2026-09-03T12:00:03Z app=api level=error three", &mut body)
    sm_line("2026-09-03T12:00:04Z app=api level=info four", &mut body)
    val seg = sm_build("build/streams-order", &body)
    var segs: Vec<String> = Vec::new()
    segs.push(seg)

    val crc = Crc32::new()
    val tal = query::streams(&segs, &crc)
    assert_eq(tal.segments, 1u64)
    assert_eq(tal.size(), 2u64)
    assert_eq(sm_count(&tal, "app=api level=error"), 3u64)
    assert_eq(sm_count(&tal, "app=api level=info"), 1u64)
}

# syslog のヘッダと `host=` ラベルが同じ値なら 1 つの組。違う値なら
# 2 つの組で、別のストリームになる。
test "a host spelled twice is one pair, spelled differently is two" {
    var body = String::new()
    sm_line("2026-09-03T12:00:01Z web01 cron[5]: host=web01 same", &mut body)
    sm_line("2026-09-03T12:00:02Z web01 cron[6]: host=web01 same again", &mut body)
    sm_line("2026-09-03T12:00:03Z web01 cron[7]: host=web02 different", &mut body)
    val seg = sm_build("build/streams-host", &body)
    var segs: Vec<String> = Vec::new()
    segs.push(seg)

    val crc = Crc32::new()
    val tal = query::streams(&segs, &crc)
    assert_eq(tal.size(), 2u64)
    assert_eq(sm_count(&tal, "host=web01 tag=cron"), 2u64)
}

# ラベルを持たない行も 1 つのストリームである (空集合)。apache の
# ような行を「数えない」ことにすると、件数の合計がレコード数と
# 合わなくなる。
test "records with no labels are a stream of their own" {
    var body = String::new()
    sm_line("10.0.0.1 - - [03/Sep/2026:12:00:01 +0000] \u{22}GET /a HTTP/1.1\u{22} 200 12 \u{22}-\u{22} \u{22}curl/8.0\u{22}", &mut body)
    sm_line("10.0.0.2 - - [03/Sep/2026:12:00:02 +0000] \u{22}GET /b HTTP/1.1\u{22} 200 12 \u{22}-\u{22} \u{22}curl/8.0\u{22}", &mut body)
    sm_line("2026-09-03T12:00:03Z app=api labelled", &mut body)
    val seg = sm_build("build/streams-empty", &body)
    var segs: Vec<String> = Vec::new()
    segs.push(seg)

    val crc = Crc32::new()
    val tal = query::streams(&segs, &crc)
    assert_eq(tal.size(), 2u64)
    assert_eq(sm_count(&tal, ""), 2u64)
    assert_eq(sm_count(&tal, "app=api"), 1u64)
    # 合計はレコード数と一致する。
    var total = 0u64
    var i = 0u64
    while i < tal.size() {
        val c: u64 = tal.counts.get(i)
        total = total + c
        i = i + 1u64
    }
    assert_eq(total, 3u64)
}

# 2 つのセグメントに跨るストリームは、件数が足され、時刻の端は
# 広い方が残る。
test "streams fold across segments" {
    var a = String::new()
    sm_line("2026-09-03T12:00:01Z app=api one", &mut a)
    sm_line("2026-09-03T12:00:02Z app=api two", &mut a)
    val seg_a = sm_build("build/streams-fold-a", &a)

    var b = String::new()
    sm_line("2026-09-04T12:00:03Z app=api three", &mut b)
    sm_line("2026-09-04T12:00:04Z app=web four", &mut b)
    val seg_b = sm_build("build/streams-fold-b", &b)

    var segs: Vec<String> = Vec::new()
    segs.push(seg_a)
    segs.push(seg_b)

    val crc = Crc32::new()
    val tal = query::streams(&segs, &crc)
    assert_eq(tal.segments, 2u64)
    assert_eq(tal.size(), 2u64)
    assert_eq(sm_count(&tal, "app=api"), 3u64)
    assert_eq(sm_count(&tal, "app=web"), 1u64)

    # 時刻の端は両方のセグメントを覆う。
    var i = 0u64
    while i < tal.size() {
        val text: &String = tal.texts.borrow(i)
        if text.eq_str("app=api") {
            val lo: i64 = tal.ts_min.get(i)
            val hi: i64 = tal.ts_max.get(i)
            assert(hi - lo > 86000i64, "the span should cross the two days")
        }
        i = i + 1u64
    }
}
