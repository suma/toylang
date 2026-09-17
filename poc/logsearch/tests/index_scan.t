# ROADMAP §4-3 — 索引の答えと、元の行を数え直した答えが一致すること。
#
# 索引のバグは**答えが少ない**形で出る。クエリは語が無いセグメントを
# 開かないので、書かれなかった語は「該当なし」と区別がつかない —
# 実際 `proto` は `field_code` が知っていてクエリも語として引くのに、
# 索引が一度も書いていなかった。`proto=HTTP/1.1` は実ログで約 15.7 万行が
# 該当するのに 0 件を返し、12 セグメントすべてが「索引で除外」と報告
# されていた (2026-09-17 に修正)。
#
# だからオラクルは**索引を経由しない**。セグメントを書いたのと同じ
# ログ行を `record::parse_line` / `extract::http` で読み直し、
# 「その値を持つレコードの数」を数える。これを `query::tally`
# (`fields <key>` の索引版が使う経路) の答えと、キーごとに
# 名前と件数の両方で突き合わせる。
#
# 値はすべて合成で、アドレスはプライベート帯だけ (CLAUDE.md)。

# ---------------------------------------------------------------------
# 素材

fn line(text: str, out: &mut String) {
    out.push_str(text)
    out.push(10u8)
}

# 分岐を 1 つずつ踏む行を並べる:
#   - apache: proto が 1.1 / 1.0 / 2.0 / 欠けている / 壊れたリクエスト行
#   - syslog: host と tag。**ラベルの `host=` を同じ値で持つ行**を含む
#     (1 レコードに同じ語が 2 回出る形で、件数を 2 と数えてはならない)
#   - ラベル付き: `host=` を持つ行と持たない行
fn fixture() -> String {
    var s = String::new()
    line("10.0.0.1 - - [03/Sep/2026:12:00:01 +0000] \u{22}GET /a HTTP/1.1\u{22} 200 12 \u{22}-\u{22} \u{22}curl/8.0\u{22}", &mut s)
    line("10.0.0.1 - - [03/Sep/2026:12:00:02 +0000] \u{22}GET /b HTTP/1.1\u{22} 404 7 \u{22}-\u{22} \u{22}curl/8.0\u{22}", &mut s)
    line("10.0.0.2 - - [03/Sep/2026:12:00:03 +0000] \u{22}POST /a HTTP/1.0\u{22} 200 12 \u{22}-\u{22} \u{22}MJ12bot/1.4\u{22}", &mut s)
    line("10.0.0.3 - - [03/Sep/2026:12:00:04 +0000] \u{22}GET /c HTTP/2.0\u{22} 301 0 \u{22}-\u{22} \u{22}curl/8.0\u{22}", &mut s)
    line("10.0.0.3 - - [03/Sep/2026:12:00:05 +0000] \u{22}GET /\u{22} 400 0 \u{22}-\u{22} \u{22}-\u{22}", &mut s)
    line("10.0.0.4 - - [03/Sep/2026:12:00:06 +0000] \u{22}-\u{22} 408 0 \u{22}-\u{22} \u{22}-\u{22}", &mut s)
    line("192.168.1.9 - - [03/Sep/2026:12:00:07 +0000] \u{22}GET /a HTTP/1.1\u{22} 200 12 \u{22}-\u{22} \u{22}curl/8.0\u{22}", &mut s)
    line("2026-09-03T12:00:08Z web01 sshd[101]: Accepted publickey for deploy", &mut s)
    line("2026-09-03T12:00:09Z web01 cron[5]: host=web01 job ran", &mut s)
    line("2026-09-03T12:00:10Z db01 cron[6]: host=web02 job ran", &mut s)
    line("2026-09-03T12:00:11Z db01 kernel: disk ok", &mut s)
    line("2026-09-03T12:00:12Z host=web02 app=api level=error request timed out", &mut s)
    line("2026-09-03T12:00:13Z app=api level=info request served", &mut s)
    line("1756900810 plain epoch line", &mut s)
    s
}

fn joined(a: str, b: str) -> String {
    val head = String::from_str(a)
    val tail = String::from_str(b)
    val out = head.concat(&tail)
    out
}

fn write_fixture(stem: str) -> String {
    val body = fixture()
    val log = joined(stem, ".log")
    val wrote = io::write_file(log.to_str(), body.to_str())
    match wrote {
        Result::Ok(n) => { }
        Result::Err(e) => { panic("fixture: cannot write {log}: {e}") }
    }
    log
}

# ---------------------------------------------------------------------
# オラクル — 索引を通らずに数える

# 「`key:value` を持つレコードの数」。1 レコードの中の重複は
# `record_terms` の段で落としてから足す。
struct Oracle {
    names: Vec<String>,
    counts: Vec<u64>,
}

impl Oracle {
    fn new() -> Self {
        val n: Vec<String> = Vec::new()
        val c: Vec<u64> = Vec::new()
        Oracle { names: n, counts: c }
    }

    fn add(&mut self, term: &String) {
        var i = 0u64
        var found = false
        while i < self.names.size() && !found {
            val nm: String = self.names.get(i)
            if nm.eq(term) {
                val c: u64 = self.counts.get(i)
                self.counts.set(i, c + 1u64)
                found = true
            }
            i = i + 1u64
        }
        if !found {
            self.names.push(term.clone())
            self.counts.push(1u64)
        }
    }
}

fn term_of(key: str, w: Span<u8>, at: u64, len: u64) -> String {
    val head = joined(key, ":")
    val value = query::text_of(w, at, len)
    val out = head.concat(&value)
    out
}

fn push_unique(terms: &mut Vec<String>, t: &String) {
    var i = 0u64
    var dup = false
    while i < terms.size() && !dup {
        val have: String = terms.get(i)
        if have.eq(t) { dup = true }
        i = i + 1u64
    }
    if !dup { terms.push(t.clone()) }
}

fn push_field(terms: &mut Vec<String>, key: str, w: Span<u8>, packed: u64) {
    val len = extract::field_len(packed)
    if len > 0u64 {
        val t = term_of(key, w, extract::field_start(packed), len)
        push_unique(terms, &t)
    }
}

# 1 レコードが持つべき語。DATA_MODEL.md §2 の規則をここでもう一度
# 書く (`archive.t` の `emit_labels` を呼ぶと、オラクルが検査対象と
# 同じ誤りを持ちうる)。
fn record_terms(w: Span<u8>, ln: Line, rec: &ParsedLine, terms: &mut Vec<String>) {
    terms.clear()
    val from = rec.labels_start()
    val end = from + rec.labels_len()
    var p = from
    while p < end {
        var k = p
        while k < end && w.get(k) != '=' { k = k + 1u64 }
        var v = k + 1u64
        if v > end { v = end }
        var stop = v
        while stop < end && w.get(stop) != ' ' { stop = stop + 1u64 }
        if k < end && stop > v {
            val key = query::text_of(w, p, k - p)
            val colon = String::from_str(":")
            val head = key.concat(&colon)
            val value = query::text_of(w, v, stop - v)
            val t = head.concat(&value)
            push_unique(terms, &t)
        }
        p = stop + 1u64
    }
    if rec.has_host() {
        val t = term_of("host", w, rec.host_start(), rec.host_len())
        push_unique(terms, &t)
    }
    if rec.tag_len() > 0u64 {
        val t = term_of("tag", w, rec.tag_start(), rec.tag_len())
        push_unique(terms, &t)
    }
    if rec.kind == 3u32 {
        val f = extract::http(w, ln.start, ln.len)
        if f.ok {
            push_field(terms, "status", w, f.status)
            push_field(terms, "method", w, f.method)
            push_field(terms, "path", w, f.path)
            push_field(terms, "ip", w, f.client)
            push_field(terms, "vhost", w, f.vhost)
            push_field(terms, "ua", w, f.ua)
            push_field(terms, "proto", w, f.proto)
        }
    }
}

# ---------------------------------------------------------------------
# 書き出しと数え直しを 1 回の読みで行う

fn build_and_count(stem: str, oracle: &mut Oracle) -> String {
    val log = write_fixture(stem)
    var reader = LogReader::with_capacity(65536u64)
    var rec = ParsedLine::new()
    var w = ArchiveWriter::new()
    val crc = Crc32::new()
    var terms: Vec<String> = Vec::new()

    val loaded = reader.load(log.to_str())
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
                            record_terms(sp, l, &rec, &mut terms)
                            var i = 0u64
                            while i < terms.size() {
                                val t: String = terms.get(i)
                                oracle.add(&t)
                                i = i + 1u64
                            }
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
    val seg = joined(stem, ".seg")
    seg
}

# `key` について、索引の集計とオラクルが名前も件数も一致すること。
# 答える件数 (値の種類数) を返す — 0 のキーで「一致した」と言っても
# 何も確かめていないので、呼び出し側が下限を確かめる。
fn agree_on(key: str, segs: &Vec<String>, oracle: &Oracle) -> u64 {
    val crc = Crc32::new()
    val prefix = joined(key, ":")
    val tal = query::tally(segs, prefix.to_str(), false, &crc)
    assert_eq(tal.segments, 1u64)

    # オラクル側の、このキーの値。
    var expected = 0u64
    var i = 0u64
    while i < oracle.names.size() {
        val nm: String = oracle.names.get(i)
        var keyed = false
        if nm.len() > prefix.len() {
            val head = nm.substring(0u64, prefix.len())
            keyed = head.eq(&prefix)
        }
        if keyed {
            expected = expected + 1u64
            val want: u64 = oracle.counts.get(i)
            val value = nm.substring(prefix.len(), nm.len())
            var got = 0u64
            var seen = false
            var j = 0u64
            while j < tal.size() {
                val tn: String = tal.names.get(j)
                if tn.eq(&value) {
                    got = tal.counts.get(j)
                    seen = true
                }
                j = j + 1u64
            }
            assert(seen, "index has no `{nm.to_str()}` ({want} record(s) carry it)")
            assert(got == want, "`{nm.to_str()}`: index says {got}, the lines say {want}")
        }
        i = i + 1u64
    }
    # 索引にだけある値 (オラクルより多い) も不一致。
    assert(tal.size() == expected, "`{key}`: index has {tal.size()} value(s), the lines have {expected}")
    expected
}

# ---------------------------------------------------------------------

test "every indexed key agrees with a recount of the lines" {
    var oracle = Oracle::new()
    val seg = build_and_count("build/index-scan-keys", &mut oracle)
    var segs: Vec<String> = Vec::new()
    segs.push(seg)

    # 下限は素材から数えた値の種類。「両方 0 で一致」を通さない。
    assert_eq(agree_on("status", &segs, &oracle), 5u64)
    # GET / POST と、リクエスト行が `"-"` の行から取れる `-`。
    assert_eq(agree_on("method", &segs, &oracle), 3u64)
    assert(agree_on("path", &segs, &oracle) >= 3u64, "path")
    assert_eq(agree_on("ip", &segs, &oracle), 5u64)
    assert(agree_on("ua", &segs, &oracle) >= 2u64, "ua")
    agree_on("vhost", &segs, &oracle)
    assert_eq(agree_on("proto", &segs, &oracle), 3u64)
    assert_eq(agree_on("tag", &segs, &oracle), 3u64)
    assert_eq(agree_on("app", &segs, &oracle), 1u64)
    assert_eq(agree_on("level", &segs, &oracle), 2u64)
}

# syslog の host とラベルの `host=` は**同じ語**である (DATA_MODEL.md §2
# の予約ラベル、`archive.t` の `emit_labelled`)。同じ値を両方に持つ行は
# 1 レコードとして数え、違う値なら両方の値に 1 件ずつ入る。
test "a syslog host and a host label are one key, counted once per record" {
    var oracle = Oracle::new()
    val seg = build_and_count("build/index-scan-host", &mut oracle)
    var segs: Vec<String> = Vec::new()
    segs.push(seg)
    # web01: sshd 行と cron 行 (host と label が同じ値) で 2。
    # web02: db01 の cron 行の label と、ラベル付き行で 2。db01: 2。
    assert_eq(agree_on("host", &segs, &oracle), 3u64)

    val crc = Crc32::new()
    val tal = query::tally(&segs, "host:", false, &crc)
    var i = 0u64
    while i < tal.size() {
        val nm: String = tal.names.get(i)
        val c: u64 = tal.counts.get(i)
        assert_eq(c, 2u64)
        i = i + 1u64
    }
}

# 索引で解決するクエリが、索引の語の欠落で**黙って 0 件**にならないこと。
# `proto=HTTP/1.1` は語として引かれ、postings が 3 行を残す。
test "a query on proto is answered by the index, not pruned away" {
    var oracle = Oracle::new()
    val seg = build_and_count("build/index-scan-query", &mut oracle)
    val q = query::parse_query("proto=HTTP/1.1", 0i64)
    assert_eq(q.terms.size(), 1u64)

    val crc = Crc32::new()
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(1024u64)
            val h = segfile::head_of(&f, &mut scratch)
            assert(h.has_terms(), "the segment should carry a term index")
            var raw = ByteWriter::with_capacity(4096u64)
            var tsec = ByteWriter::with_capacity(4096u64)
            assert(segfile::load_block(&f, h.terms_off, h.terms_len, &crc, &mut raw, &mut tsec),
                   "the term section should decode")
            val tw = tsec.span()
            match tw {
                Option::Some(traw) => {
                    var allowed: Vec<u32> = Vec::new()
                    val pruned = query::resolve_indexed(traw, tsec.len(), &q, &mut allowed)
                    assert(!pruned, "proto=HTTP/1.1 pruned a segment that holds it")
                    assert_eq(allowed.size(), 3u64)
                }
                Option::None => { panic("empty term section") }
            }
        }
        Result::Err(e) => { panic("cannot open {seg}: {e}") }
    }
}
