# QUERY.md — 検索そのもの。バイト探索から、1 本のクエリの答えまで。
#
# ここまで検査されていたのは**クエリの読み方** (`tests/query.t` の
# 時刻境界) と**索引の中身** (`tests/index_scan.t`) で、「その索引を
# 使って実際に何行返るか」は誰も見ていなかった。索引は候補を絞る
# だけなので、絞ったあとの行ごとの判定 (`matches`) が間違っても
# **件数が減るだけ**で、エラーにはならない。
#
# 2 段で押さえる:
#
#   1. `search::find` — SIMD 版とスカラー版が 1 バイトも違わないこと
#      (`lsz` の `match_len` と同じ理由。速い方だけが間違っても
#      「見つからなかった」としか出ない)
#   2. `query::search` — 素材の行を数えた答えと、実際の件数が
#      一致すること。索引で引く形 (`status=404`) と本文を舐める形
#      (`needle`) の**両方**を同じ素材に対して
#
# 素材は合成で、アドレスはプライベート帯だけ (CLAUDE.md)。

# ---------------------------------------------------------------------
# 1. バイト探索

fn sq_bytes(text: str) -> String {
    val out = String::from_str(text)
    out
}

fn sq_span(s: &String) -> Span<u8> {
    val w = s.as_span()
    match w {
        Option::Some(sp) => sp,
        Option::None => { panic("sq_span: empty string") }
    }
}

# SIMD 版とスカラー版は同じ答えでなければならない。`--check` が
# ランダムな干し草と針でこれを掃く (ファイル末尾のコメント)。
pub unsafe fn find_agrees(seed: u64, hay_len: u64, needle_len: u64) -> bool
    ensures result
{
    val n = 1u64 + hay_len % 400u64
    var hay = String::new()
    var s = seed + 1u64
    var i = 0u64
    while i < n {
        s = (s * 6364136223846793005u64 + 1442695040888963407u64) & 0xFFFFFFFFFFFFFFFFu64
        hay.push((97u8 + ((s >> 33u64) % 4u64) as u8))
        i = i + 1u64
    }
    val m = 1u64 + needle_len % 6u64
    var needle = String::new()
    var k = 0u64
    while k < m {
        s = (s * 6364136223846793005u64 + 1442695040888963407u64) & 0xFFFFFFFFFFFFFFFFu64
        needle.push((97u8 + ((s >> 33u64) % 4u64) as u8))
        k = k + 1u64
    }
    val h = sq_span(&hay)
    val nd = sq_span(&needle)
    search::find(h, 0u64, n, nd, m) == search::find_scalar(h, 0u64, n, nd, m)
}

test "the vector search and the scalar search answer alike" {
    # 境界を名指しで: 16 バイトのブロック境界、干し草より長い針、
    # 端に在る針、1 バイトの針。
    val hay = sq_bytes("abcdefghijklmnopqrstuvwxyz0123456789")
    val h = sq_span(&hay)
    val at_start = sq_bytes("abc")
    val at_end = sq_bytes("789")
    val across16 = sq_bytes("opqrs")
    val absent = sq_bytes("zzz")
    val one = sq_bytes("q")
    assert(search::find(h, 0u64, hay.len(), sq_span(&at_start), 3u64), "at the start")
    assert(search::find(h, 0u64, hay.len(), sq_span(&at_end), 3u64), "at the end")
    assert(search::find(h, 0u64, hay.len(), sq_span(&across16), 5u64), "across the block")
    assert(!search::find(h, 0u64, hay.len(), sq_span(&absent), 3u64), "absent")
    assert(search::find(h, 0u64, hay.len(), sq_span(&one), 1u64), "one byte")
    # 窓の外は探さない。`[0, 5)` に `q` は無い。
    assert(!search::find(h, 0u64, 5u64, sq_span(&one), 1u64), "outside the window")

    var i = 0u64
    while i < 40u64 {
        assert(find_agrees(i, i * 7u64, i), "random {i}")
        i = i + 1u64
    }
}

# `equals` は**全体**の一致。`path:/a` が `/ab` を飲まない
# (ONTOLOGY.md §6 の「`=` は完全一致」) のはこれが根拠。
test "equals is the whole value, not a prefix of it" {
    val hay = sq_bytes("/ab")
    val h = sq_span(&hay)
    val short = sq_bytes("/a")
    val whole = sq_bytes("/ab")
    assert(!search::equals(h, 0u64, 3u64, sq_span(&short), 2u64), "a prefix is not the value")
    assert(search::equals(h, 0u64, 3u64, sq_span(&whole), 3u64), "the value itself")
    assert(search::equals(h, 0u64, 2u64, sq_span(&short), 2u64), "the first two bytes are `/a`")
}

# ---------------------------------------------------------------------
# 2. 1 本のクエリの答え

fn sq_line(text: str, out: &mut String) {
    out.push_str(text)
    out.push(10u8)
}

# 数えられる素材。**それぞれの数は下のテストが名前で言う**:
#   status=404 が 3、method=POST が 2、tag=cron が 2、
#   host=web01 (syslog ヘッダ) が 3、本文に `timeout` が 2
fn sq_fixture() -> String {
    var s = String::new()
    sq_line("10.0.0.1 - - [03/Sep/2026:12:00:01 +0000] \u{22}GET /a HTTP/1.1\u{22} 200 12 \u{22}-\u{22} \u{22}curl/8.0\u{22}", &mut s)
    sq_line("10.0.0.1 - - [03/Sep/2026:12:00:02 +0000] \u{22}GET /b HTTP/1.1\u{22} 404 7 \u{22}-\u{22} \u{22}curl/8.0\u{22}", &mut s)
    sq_line("10.0.0.2 - - [03/Sep/2026:12:00:03 +0000] \u{22}POST /a HTTP/1.1\u{22} 404 9 \u{22}-\u{22} \u{22}MJ12bot/1.4\u{22}", &mut s)
    sq_line("10.0.0.2 - - [03/Sep/2026:12:00:04 +0000] \u{22}POST /c HTTP/1.0\u{22} 200 3 \u{22}-\u{22} \u{22}curl/8.0\u{22}", &mut s)
    sq_line("10.0.0.3 - - [03/Sep/2026:12:00:05 +0000] \u{22}GET /d HTTP/1.1\u{22} 404 0 \u{22}-\u{22} \u{22}curl/8.0\u{22}", &mut s)
    sq_line("2026-09-03T12:10:00Z web01 cron[5]: job ran with a timeout of 30s", &mut s)
    sq_line("2026-09-03T12:10:01Z web01 cron[6]: job ran", &mut s)
    sq_line("2026-09-03T12:10:02Z web01 sshd[9]: connection timeout", &mut s)
    sq_line("2026-09-03T12:10:03Z db01 kernel: disk ok", &mut s)
    s
}

fn sq_build(stem: str) -> String {
    val body = sq_fixture()
    val seg = sq_build_from(stem, &body)
    seg
}

fn sq_build_from(stem: str, body: &String) -> String {
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

# `text` のクエリが返す行数。**`limit` は表示の数**であって
# 一致の数ではないので、ここは `hits.size()` を見る。
fn sq_hits(segs: &Vec<String>, text: str) -> u64 {
    val crc = Crc32::new()
    val q = query::parse_query(text, 0i64)
    var hits: Vec<Hit> = Vec::new()
    var texts: Vec<String> = Vec::new()
    var st = SearchStats::new()
    query::search("build", segs, &q, &crc, &mut hits, &mut texts, &mut st)
    hits.size()
}

test "a query answers with the lines that actually match" {
    val seg = sq_build("build/search-query")
    var segs: Vec<String> = Vec::new()
    segs.push(seg)

    # 索引で引く形。
    assert_eq(sq_hits(&segs, "status=404"), 3u64)
    assert_eq(sq_hits(&segs, "method=POST"), 2u64)
    assert_eq(sq_hits(&segs, "proto=HTTP/1.0"), 1u64)
    # 本文を舐める形 (語ではないので needle になる)。
    assert_eq(sq_hits(&segs, "timeout"), 2u64)
    # syslog のヘッダを名指しする形。
    assert_eq(sq_hits(&segs, "host=web01"), 3u64)
    assert_eq(sq_hits(&segs, "tag=cron"), 2u64)
    # 何も書かなければ全件。
    assert_eq(sq_hits(&segs, ""), 9u64)
    # 当たらない語は 0 件 — ただし**索引に在る語で 0**と
    # **索引に無い語で 0** は別の経路なので、両方踏む。
    assert_eq(sq_hits(&segs, "status=500"), 0u64)
    assert_eq(sq_hits(&segs, "no-such-text-anywhere"), 0u64)
}

# 複数の項は AND。索引の項どうし、索引と本文、それぞれ。
test "two terms mean both, not either" {
    val seg = sq_build("build/search-query-and")
    var segs: Vec<String> = Vec::new()
    segs.push(seg)
    assert_eq(sq_hits(&segs, "status=404 method=POST"), 1u64)
    assert_eq(sq_hits(&segs, "status=404 method=GET"), 2u64)
    assert_eq(sq_hits(&segs, "tag=cron timeout"), 1u64)
    assert_eq(sq_hits(&segs, "status=404 timeout"), 0u64)
}

# 部分一致 (`~`) と前方一致 (`^`) は**索引の語の中**を探す
# (ONTOLOGY.md §6)。完全一致との違いをここで固定する。
test "substring and prefix match inside a key's values" {
    val seg = sq_build("build/search-query-sub")
    var segs: Vec<String> = Vec::new()
    segs.push(seg)
    assert_eq(sq_hits(&segs, "ua~bot"), 1u64)
    assert_eq(sq_hits(&segs, "ua=bot"), 0u64)
    assert_eq(sq_hits(&segs, "path^/a"), 2u64)
    assert_eq(sq_hits(&segs, "path=/a"), 2u64)
    assert_eq(sq_hits(&segs, "proto^HTTP"), 5u64)
}

# 時刻の窓は**枝刈りの入口**であり、答えの一部でもある。
# 素材は 12:00:0x の HTTP 5 行と 12:10:0x の syslog 4 行に割れている。
test "a time window keeps the records inside it" {
    val seg = sq_build("build/search-query-time")
    var segs: Vec<String> = Vec::new()
    segs.push(seg)
    assert_eq(sq_hits(&segs, "from=2026-09-03T12:05:00Z"), 4u64)
    assert_eq(sq_hits(&segs, "to=2026-09-03T12:05:00Z"), 5u64)
    assert_eq(sq_hits(&segs, "from=2026-09-03T12:00:00Z to=2026-09-03T12:20:00Z"), 9u64)
    # セグメント全体が窓の外なら、開かずに済ませる。
    val crc = Crc32::new()
    val q = query::parse_query("from=2030-01-01T00:00:00Z", 0i64)
    var hits: Vec<Hit> = Vec::new()
    var texts: Vec<String> = Vec::new()
    var st = SearchStats::new()
    query::search("build", &segs, &q, &crc, &mut hits, &mut texts, &mut st)
    assert_eq(hits.size(), 0u64)
    assert_eq(st.pruned_time, 1u64)
    assert_eq(st.opened, 0u64)
}

# 索引が当たらないセグメントは**開かない**。これが O0-b の眼目で、
# 「開かなかった」ことは件数からは見えないので stats で見る。
test "a segment without the term is pruned, not scanned" {
    val seg = sq_build("build/search-query-prune")
    var segs: Vec<String> = Vec::new()
    segs.push(seg)
    val crc = Crc32::new()
    val q = query::parse_query("status=500", 0i64)
    var hits: Vec<Hit> = Vec::new()
    var texts: Vec<String> = Vec::new()
    var st = SearchStats::new()
    query::search("build", &segs, &q, &crc, &mut hits, &mut texts, &mut st)
    assert_eq(hits.size(), 0u64)
    assert_eq(st.pruned_terms, 1u64)
    assert_eq(st.examined, 0u64)
}

# `host` は**予約ラベル** (DATA_MODEL.md §2) で、意味は「送信元ホスト」。
# syslog のヘッダで来ても `host=` ラベルで来ても同じ語なので、
# `host=web01` は**両方**に当たらなければならない。かつてクエリだけが
# ヘッダと直接比べていて、取り込んだレコードには当たらず、しかも索引を
# 通らないので 12 セグメント全部を展開していた (2026-09-18 に修正)。
test "a host is the sending host, however it arrived" {
    var body = String::new()
    sq_line("2026-09-03T12:00:01Z web01 cron[5]: header says web01", &mut body)
    sq_line("2026-09-03T12:00:02Z host=web01 app=api label says web01", &mut body)
    sq_line("2026-09-03T12:00:03Z host=web02 app=api label says web02", &mut body)
    sq_line("2026-09-03T12:00:04Z db01 cron[6]: header says db01", &mut body)
    val seg = sq_build_from("build/search-query-host", &body)
    var segs: Vec<String> = Vec::new()
    segs.push(seg)

    assert_eq(sq_hits(&segs, "host=web01"), 2u64)
    assert_eq(sq_hits(&segs, "host=web02"), 1u64)
    assert_eq(sq_hits(&segs, "host=db01"), 1u64)
    # ラベルの他のキーも同じ扱い。
    assert_eq(sq_hits(&segs, "app=api"), 2u64)
    assert_eq(sq_hits(&segs, "host=web01 app=api"), 1u64)
}

# 索引を通るということは、**当たらないセグメントを開かない**という
# ことでもある。ここが `host=` の食い違いで失われていた分。
test "a host query prunes segments like any other term" {
    var body = String::new()
    sq_line("2026-09-03T12:00:01Z web01 cron[5]: only web01 here", &mut body)
    val seg = sq_build_from("build/search-query-host-prune", &body)
    var segs: Vec<String> = Vec::new()
    segs.push(seg)

    val crc = Crc32::new()
    val q = query::parse_query("host=nowhere", 0i64)
    assert_eq(q.term_count(), 1u64)
    var hits: Vec<Hit> = Vec::new()
    var texts: Vec<String> = Vec::new()
    var st = SearchStats::new()
    query::search("build", &segs, &q, &crc, &mut hits, &mut texts, &mut st)
    assert_eq(hits.size(), 0u64)
    assert_eq(st.pruned_terms, 1u64)
    assert_eq(st.examined, 0u64)
}

# プロパティの実行 (リポジトリルートから):
#
#   ./target/release/interpreter --core-modules core \
#       --core-modules poc/logsearch/src --check poc/logsearch/tests/search_query.t
