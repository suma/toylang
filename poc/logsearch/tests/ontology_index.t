# ONTOLOGY §4 / §5 — 語彙索引と共起リンクが引けるか。
#
# §2 の主張 (「部分一致は数として信用できない」) が成り立つのは、
# `status:404` がバイト数の `404` と**別の語**だからで、それを保って
# いるのは索引の書き出しと読み出しの両側である。ここはその往復を、
# 1 セグメントぶんの小さなアーカイブで固める。
#
# corpus は使わない。分布は corpus に依存するので回帰にならないし、
# 実ログから取れば実在のアドレスがリポジトリに入る (CLAUDE.md)。
# 代わりに、§2 が言う誤爆の形を**わざと仕込んだ**合成データを使う。
#
# 値を `val` に取り出さず `match` の腕の中で使い切る形なのは、compound
# を式の位置で返せないため (main.t の `cmd_query` も同じ形)。

fn span_of(s: &String) -> Span<u8> {
    val w = s.as_span()
    match w {
        Option::Some(sp) => sp,
        Option::None => { panic("span_of: empty string") }
    }
}

fn joined(a: str, b: str) -> String {
    val head = String::from_str(a)
    val tail = String::from_str(b)
    val out = head.concat(&tail)
    out
}

# 誤爆を仕込んである:
#   - 2 行目は**バイト数**が 404 (ステータスは 200)
#   - 3 行目は**パス**に 404 を含む (ステータスは 200)
# 部分一致なら 4 行すべてが `404` に当たり、`status:404` なら 2 行。
#
# 5 行目の `/ab` は `/a` の**真の接頭辞拡張**である。完全一致の境界は
# これが無いと検査できない — 語の長さを見ない実装でも、長さの違う組が
# 無ければ答えが変わらないので通ってしまう。
fn fixture() -> String {
    val out = String::from_str(
        "10.0.0.1 - - [03/Sep/2026:12:00:01 +0000] \"GET /a HTTP/1.1\" 404 12 \"-\" \"curl/8.0\"\n10.0.0.1 - - [03/Sep/2026:12:00:02 +0000] \"GET /b HTTP/1.1\" 200 404 \"-\" \"curl/8.0\"\n10.0.0.2 - - [03/Sep/2026:12:00:03 +0000] \"GET /404.html HTTP/1.1\" 200 55 \"-\" \"curl/8.0\"\n10.0.0.2 - - [03/Sep/2026:12:00:04 +0000] \"GET /a HTTP/1.1\" 404 12 \"-\" \"MJ12bot/1.4\"\n10.0.0.3 - - [03/Sep/2026:12:00:05 +0000] \"GET /ab HTTP/1.1\" 200 7 \"-\" \"curl/8.0\"\n")
    out
}

# 素材を 1 セグメントに書き、その `.seg` の名前を返す。
fn build(stem: str) -> String {
    val body = fixture()
    val log = joined(stem, ".log")
    val wrote = io::write_file(log.to_str(), body.to_str())
    match wrote {
        Result::Ok(n) => { }
        Result::Err(e) => { panic("fixture: cannot write {log}: {e}") }
    }

    var reader = LogReader::with_capacity(65536u64)
    var rec = ParsedLine::new()
    var w = ArchiveWriter::new()
    val crc = Crc32::new()

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

# `status:404` は 2 件で、バイト数やパスの `404` を拾わない。
# §2 の「1,910 行 (3.4%) は誤爆」を小さく再現したもの。
test "a keyed term does not match the same digits elsewhere" {
    val seg = build("build/ontology-fixture-terms")
    val crc = Crc32::new()
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(1024u64)
            val h = segfile::head_of(&f, &mut scratch)
            assert(h.ok, "segment header should read")
            assert(h.has_terms(), "the segment should carry a term index")

            var raw = ByteWriter::with_capacity(4096u64)
            var tsec = ByteWriter::with_capacity(4096u64)
            assert(segfile::load_block(&f, h.terms_off, h.terms_len, &crc, &mut raw, &mut tsec),
                   "the term section should decode")
            val tw = tsec.span()
            match tw {
                Option::Some(traw) => {
                    val want = String::from_str("status:404")
                    val post = archive::term_postings(traw, tsec.len(), span_of(&want), want.len())
                    assert(post.found, "status:404 should be in the dictionary")
                    assert_eq(post.doc_count, 2u64)

                    # 200 も 2 件。合わせて 4 行ぶんで取りこぼしが無い。
                    val w200 = String::from_str("status:200")
                    val p200 = archive::term_postings(traw, tsec.len(), span_of(&w200), w200.len())
                    assert(p200.found, "status:200 should be in the dictionary")
                    assert_eq(p200.doc_count, 3u64)

                    # postings は昇順の ordinal。404 は 1 行目と 4 行目。
                    var ords: Vec<u32> = Vec::new()
                    archive::decode_postings(traw, post.at, post.len, &mut ords)
                    assert_eq(ords.size(), 2u64)
                    assert_eq(ords.get(0u64), 0u32)
                    assert_eq(ords.get(1u64), 3u32)

                    # 完全一致の境界。`path:/a` は `/a` の 2 行だけで、
                    # 接頭辞拡張の `/ab` を飲み込まない。逆向きも見る。
                    val pa = String::from_str("path:/a")
                    val ppa = archive::term_postings(traw, tsec.len(), span_of(&pa), pa.len())
                    assert(ppa.found, "path:/a should be in the dictionary")
                    assert_eq(ppa.doc_count, 2u64)

                    val pab = String::from_str("path:/ab")
                    val ppab = archive::term_postings(traw, tsec.len(), span_of(&pab), pab.len())
                    assert(ppab.found, "path:/ab should be in the dictionary")
                    assert_eq(ppab.doc_count, 1u64)
                }
                Option::None => { panic("empty term section") }
            }
        }
        Result::Err(e) => { panic("cannot open {seg}: {e}") }
    }
}

# §4 の「集計が索引の副産物になる」— 語ごとに doc_count を持つので、
# 上位 N は postings を読まずに出る。`top=status` がこれで動く。
test "a tally comes out of the dictionary without reading postings" {
    val seg = build("build/ontology-fixture-tally")
    val crc = Crc32::new()
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(1024u64)
            val h = segfile::head_of(&f, &mut scratch)
            var raw = ByteWriter::with_capacity(4096u64)
            var tsec = ByteWriter::with_capacity(4096u64)
            assert(segfile::load_block(&f, h.terms_off, h.terms_len, &crc, &mut raw, &mut tsec),
                   "the term section should decode")
            val tw = tsec.span()
            match tw {
                Option::Some(traw) => {
                    val hits = archive::terms_with_prefix(traw, 0u64, tsec.len(), "status:")
                    # status は 2 種 (404 / 200)、合計は 5 行。
                    assert_eq(hits.names.size(), 2u64)
                    var total: u64 = 0u64
                    var i: u64 = 0u64
                    while i < hits.names.size() {
                        val c: u64 = hits.counts.get(i)
                        total = total + c
                        i = i + 1u64
                    }
                    assert_eq(total, 5u64)
                }
                Option::None => { panic("empty term section") }
            }
        }
        Result::Err(e) => { panic("cannot open {seg}: {e}") }
    }
}

# §5 — 同じレコードに現れた 2 つの値の組を書き出しの時点で数える。
# `ip=X top=path` がフレームを 1 つも展開せずに答えられるのはこれ。
test "co-occurrence links are countable without expanding a frame" {
    val seg = build("build/ontology-fixture-links")
    val crc = Crc32::new()
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(1024u64)
            val h = segfile::head_of(&f, &mut scratch)
            assert(h.has_links(), "the segment should carry a link section")

            var raw = ByteWriter::with_capacity(4096u64)
            var tsec = ByteWriter::with_capacity(4096u64)
            assert(segfile::load_block(&f, h.terms_off, h.terms_len, &crc, &mut raw, &mut tsec),
                   "the term section should decode")
            var lbuf = ByteWriter::with_capacity(4096u64)
            var lsec = ByteWriter::with_capacity(4096u64)
            assert(segfile::load_block(&f, h.links_off, h.links_len, &crc, &mut lbuf, &mut lsec),
                   "the link section should decode")
            val tw = tsec.span()
            match tw {
                Option::Some(traw) => {
                    val lw = lsec.span()
                    match lw {
                        Option::Some(lraw) => {
                            # 10.0.0.1 は /a と /b を 1 回ずつ引いている。
                            # id で引き当てるので名前を引き戻さずに済む。
                            val who = String::from_str("ip:10.0.0.1")
                            val from_id = archive::term_id_of(traw, tsec.len(), span_of(&who), who.len())
                            assert(from_id != archive::TERM_NONE, "ip:10.0.0.1 should be a term")

                            val pa_s = String::from_str("path:/a")
                            val pb_s = String::from_str("path:/b")
                            val pa = archive::term_id_of(traw, tsec.len(), span_of(&pa_s), pa_s.len())
                            val pb = archive::term_id_of(traw, tsec.len(), span_of(&pb_s), pb_s.len())
                            assert(pa != archive::TERM_NONE, "path:/a should be a term")
                            assert(pb != archive::TERM_NONE, "path:/b should be a term")

                            var to_ids: Vec<u32> = Vec::new()
                            var counts: Vec<u32> = Vec::new()
                            val found = archive::links_of(lraw, lsec.len(), from_id, &mut to_ids, &mut counts)
                            assert(found > 0u64, "the address should have links")

                            var saw_a = false
                            var saw_b = false
                            var i: u64 = 0u64
                            while i < to_ids.size() {
                                val id: u32 = to_ids.get(i)
                                val c: u32 = counts.get(i)
                                val idw = id as u64
                                if idw == pa { saw_a = true  assert_eq(c, 1u32) }
                                if idw == pb { saw_b = true  assert_eq(c, 1u32) }
                                i = i + 1u64
                            }
                            assert(saw_a, "10.0.0.1 -> /a should be linked")
                            assert(saw_b, "10.0.0.1 -> /b should be linked")
                        }
                        Option::None => { panic("empty link section") }
                    }
                }
                Option::None => { panic("empty term section") }
            }
        }
        Result::Err(e) => { panic("cannot open {seg}: {e}") }
    }
}

# §10 の「次の候補」— 語の部分一致。`ua=MJ12bot` が 0 件になるのは
# フィールドが**値の全体**に一致するからで (§6)、`~` はその値の中を
# 探す。走査は辞書 1 周で、postings にも arena にも触らない。
test "a needle finds the values that contain it" {
    val seg = build("build/ontology-fixture-sub")
    val crc = Crc32::new()
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(1024u64)
            val h = segfile::head_of(&f, &mut scratch)
            var raw = ByteWriter::with_capacity(4096u64)
            var tsec = ByteWriter::with_capacity(4096u64)
            assert(segfile::load_block(&f, h.terms_off, h.terms_len, &crc, &mut raw, &mut tsec),
                   "the term section should decode")
            val tw = tsec.span()
            match tw {
                Option::Some(traw) => {
                    # `MJ12` は 1 つの user agent の中にある。完全一致の
                    # `ua=MJ12bot` は 0 件で、これが §6 の言う差。
                    val n1 = String::from_str("MJ12")
                    val m1 = archive::terms_matching(traw, 0u64, tsec.len(), "ua:", span_of(&n1), 0u64, n1.len(), false)
                    assert_eq(m1.hits, 1u64)
                    var o1: Vec<u32> = Vec::new()
                    archive::decode_postings(traw, m1.at.get(0u64), m1.len.get(0u64), &mut o1)
                    assert_eq(o1.size(), 1u64)
                    assert_eq(o1.get(0u64), 3u32)

                    # `a` は `/a` と `/ab` に当たる。`/404.html` には
                    # 無いので、キーの中だけを見ていることも言える。
                    val n2 = String::from_str("a")
                    val m2 = archive::terms_matching(traw, 0u64, tsec.len(), "path:", span_of(&n2), 0u64, n2.len(), false)
                    assert_eq(m2.hits, 2u64)

                    # 当たらない needle は 0 件。空の集合であって、
                    # 「索引が知らないので本文を見る」ではない。
                    val n3 = String::from_str("nowhere")
                    val m3 = archive::terms_matching(traw, 0u64, tsec.len(), "path:", span_of(&n3), 0u64, n3.len(), false)
                    assert_eq(m3.hits, 0u64)

                    # 空の needle はそのキーの値すべて。`ua~` が
                    # 「user agent を持つ行すべて」になる。長さ 0 を
                    # 渡すので、どの span を指しても読まれない。
                    val m4 = archive::terms_matching(traw, 0u64, tsec.len(), "ua:", span_of(&n1), 0u64, 0u64, false)
                    assert_eq(m4.hits, 2u64)
                }
                Option::None => { panic("empty term section") }
            }
        }
        Result::Err(e) => { panic("cannot open {seg}: {e}") }
    }
}

# §4 が先送りしていた前方一致。線形走査なので辞書順は要らない —
# 必要なのは「先頭でだけ比べる」ことだけ。実データでは差が大きく、
# `/.env` は 7,860 のパスに現れるが始めるのは 3,027 だけである。
test "an anchored needle only matches at the start of a value" {
    val seg = build("build/ontology-fixture-anchor")
    val crc = Crc32::new()
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(1024u64)
            val h = segfile::head_of(&f, &mut scratch)
            var raw = ByteWriter::with_capacity(4096u64)
            var tsec = ByteWriter::with_capacity(4096u64)
            assert(segfile::load_block(&f, h.terms_off, h.terms_len, &crc, &mut raw, &mut tsec),
                   "the term section should decode")
            val tw = tsec.span()
            match tw {
                Option::Some(traw) => {
                    # `html` は `/404.html` の**中**にある。含むなら 1 件、
                    # 先頭でだけ見るなら 0 件。
                    val n = String::from_str("html")
                    val sub = archive::terms_matching(traw, 0u64, tsec.len(), "path:", span_of(&n), 0u64, n.len(), false)
                    assert_eq(sub.hits, 1u64)
                    val anc = archive::terms_matching(traw, 0u64, tsec.len(), "path:", span_of(&n), 0u64, n.len(), true)
                    assert_eq(anc.hits, 0u64)

                    # `/a` は `/a` と `/ab` の両方を**始める**ので、
                    # 前方一致でも 2 件のまま。接頭辞は完全一致より広い。
                    val p = String::from_str("/a")
                    val ap = archive::terms_matching(traw, 0u64, tsec.len(), "path:", span_of(&p), 0u64, p.len(), true)
                    assert_eq(ap.hits, 2u64)

                    # 空の needle は錨があっても全件。
                    val ae = archive::terms_matching(traw, 0u64, tsec.len(), "path:", span_of(&p), 0u64, 0u64, true)
                    assert_eq(ae.hits, 4u64)
                }
                Option::None => { panic("empty term section") }
            }
        }
        Result::Err(e) => { panic("cannot open {seg}: {e}") }
    }
}

# 索引側の解決そのもの — `=` の積、`~` の合併、両者の混在。
# 素材の 5 行 (ordinal 0..4):
#   0  ip 10.0.0.1  path /a         status 404  ua curl/8.0
#   1  ip 10.0.0.1  path /b         status 200  ua curl/8.0
#   2  ip 10.0.0.2  path /404.html  status 200  ua curl/8.0
#   3  ip 10.0.0.2  path /a         status 404  ua MJ12bot/1.4
#   4  ip 10.0.0.3  path /ab        status 200  ua curl/8.0
fn allow(traw: Span<u8>, tlen: u64, text: str, out: &mut Vec<u32>) -> bool {
    val q = query::parse_query(text, 0i64)
    val empty = query::resolve_indexed(traw, tlen, &q, out)
    empty
}

fn only(out: &Vec<u32>, a: u32) {
    assert_eq(out.size(), 1u64)
    assert_eq(out.get(0u64), a)
}

test "the index resolves unions and intersections together" {
    val seg = build("build/ontology-fixture-resolve")
    val crc = Crc32::new()
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(1024u64)
            val h = segfile::head_of(&f, &mut scratch)
            var raw = ByteWriter::with_capacity(4096u64)
            var tsec = ByteWriter::with_capacity(4096u64)
            assert(segfile::load_block(&f, h.terms_off, h.terms_len, &crc, &mut raw, &mut tsec),
                   "the term section should decode")
            val tw = tsec.span()
            match tw {
                Option::Some(traw) => {
                    val tlen = tsec.len()
                    var got: Vec<u32> = Vec::new()

                    # `=` ひとつ: 404 は 0 と 3。
                    assert(!allow(traw, tlen, "status=404", &mut got), "404 is present")
                    assert_eq(got.size(), 2u64)
                    assert_eq(got.get(0u64), 0u32)
                    assert_eq(got.get(1u64), 3u32)

                    # `=` ふたつの積: 404 かつ ip 10.0.0.2 は 3 だけ。
                    assert(!allow(traw, tlen, "status=404 ip=10.0.0.2", &mut got), "present")
                    only(&got, 3u32)

                    # 交わらない `=` の積は空 — セグメントごと飛ばせる。
                    assert(allow(traw, tlen, "status=404 ip=10.0.0.3", &mut got), "no such record")

                    # `~` の合併: path に `a` を含むのは /a (0,3) と /ab (4)。
                    # 3 つの posting list が昇順のまま 1 本になる。
                    assert(!allow(traw, tlen, "path~a", &mut got), "present")
                    assert_eq(got.size(), 3u64)
                    assert_eq(got.get(0u64), 0u32)
                    assert_eq(got.get(1u64), 3u32)
                    assert_eq(got.get(2u64), 4u32)

                    # `^` は先頭でだけ: `html` は /404.html の中にあるが
                    # 始めてはいないので空。`~` なら 2 が出る。
                    assert(!allow(traw, tlen, "path~html", &mut got), "contains")
                    only(&got, 2u32)
                    assert(allow(traw, tlen, "path^html", &mut got), "does not start")

                    # `~` と `=` の混在。合併してから積を取る。
                    assert(!allow(traw, tlen, "path~a status=404", &mut got), "present")
                    assert_eq(got.size(), 2u64)
                    assert_eq(got.get(0u64), 0u32)
                    assert_eq(got.get(1u64), 3u32)

                    # 順序が効かないこと。同じ答えでなければ合併か積の
                    # どちらかが順番に依存している。
                    assert(!allow(traw, tlen, "status=404 path~a", &mut got), "present")
                    assert_eq(got.size(), 2u64)
                    assert_eq(got.get(0u64), 0u32)
                    assert_eq(got.get(1u64), 3u32)

                    # `~` ふたつの積: `a` を含み、かつ ua に `curl`。
                    # /a(0,3) ∪ /ab(4) と curl(0,1,2,4) の積 = 0,4。
                    assert(!allow(traw, tlen, "path~a ua~curl", &mut got), "present")
                    assert_eq(got.size(), 2u64)
                    assert_eq(got.get(0u64), 0u32)
                    assert_eq(got.get(1u64), 4u32)

                }
                Option::None => { panic("empty term section") }
            }
        }
        Result::Err(e) => { panic("cannot open {seg}: {e}") }
    }
}

# トークンの振り分け。`resolve_indexed` は索引の項があるときにしか
# 呼ばれないので、どれが索引の項になるかはここで決まる。
test "the parser sorts tokens into index terms and body needles" {
    # `=` は完全一致の項。
    val a = query::parse_query("status=404", 0i64)
    assert_eq(a.term_count(), 1u64)
    assert_eq(a.sub_count(), 0u64)
    assert_eq(a.needle_count(), 0u64)

    # `~` と `^` は同じ枠 (`subs`) に入り、モードだけが違う。
    val b = query::parse_query("ua~MJ12bot", 0i64)
    assert_eq(b.term_count(), 0u64)
    assert_eq(b.sub_count(), 1u64)
    assert_eq(b.needle_count(), 0u64)

    val c = query::parse_query("path^/wp-", 0i64)
    assert_eq(c.sub_count(), 1u64)
    assert_eq(c.needle_count(), 0u64)

    # **ラベルの形をしたキーはすべて索引の項になる** (2026-09-11)。
    # 以前はキーが 8 つの決め打ちで、`level=error` は本文検索に
    # 落ちていた — 辞書に `level` が無かったからである。取り込みが
    # ラベルを索引するようになったので、問いは「その名前を知っているか」
    # から「ラベルの形をしているか」に変わった。
    val d = query::parse_query("level=error level~err", 0i64)
    assert_eq(d.indexed_count(), 2u64)
    assert_eq(d.needle_count(), 0u64)

    # ラベルの形をしていないキーは本文のまま。大文字も、点も、
    # 32 バイト超も、ラベルのキーにはなりえない (DATA_MODEL.md §2)。
    val g = query::parse_query("Host=web01 a.b=c", 0i64)
    assert_eq(g.indexed_count(), 0u64)
    assert_eq(g.needle_count(), 2u64)

    # 制御語は本文検索に落ちない。`top=path` を含む行を探すのは
    # 誰の意図でもない。
    val e = query::parse_query("top=path limit=5 order=asc", 0i64)
    assert_eq(e.indexed_count(), 0u64)
    assert_eq(e.needle_count(), 0u64)
    assert_eq(e.limit, 5u64)
    assert(!e.desc, "order=asc should clear desc")

    # 混在。索引 3 つと本文 1 つ。
    val f = query::parse_query("status=404 ua~bot path^/wp- timeout", 0i64)
    assert_eq(f.term_count(), 1u64)
    assert_eq(f.sub_count(), 2u64)
    assert_eq(f.indexed_count(), 3u64)
    assert_eq(f.needle_count(), 1u64)
}

# ONTOLOGY O1 の残り — object 表 (first_seen / last_seen)。
# 件数は語彙索引の `doc_count` に既にあるので、新規はこの 2 つだけ。
# 素材の 5 行は 12:00:01 から 12:00:05 まで 1 秒刻み。
test "the object table records when a value was seen" {
    val seg = build("build/ontology-fixture-object")
    val crc = Crc32::new()
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(1024u64)
            val h = segfile::head_of(&f, &mut scratch)
            assert(h.has_objects(), "the segment should carry an object table")

            var raw = ByteWriter::with_capacity(4096u64)
            var tsec = ByteWriter::with_capacity(4096u64)
            assert(segfile::load_block(&f, h.terms_off, h.terms_len, &crc, &mut raw, &mut tsec),
                   "the term section should decode")
            var obuf = ByteWriter::with_capacity(4096u64)
            var osec = ByteWriter::with_capacity(4096u64)
            assert(segfile::load_block(&f, h.objs_off, h.objs_len, &crc, &mut obuf, &mut osec),
                   "the object table should decode")
            val tw = tsec.span()
            match tw {
                Option::Some(traw) => {
                    val ow = osec.span()
                    match ow {
                        Option::Some(oraw) => {
                            # `/a` は 1 行目 (12:00:01) と 4 行目 (12:00:04)。
                            # 端は最小と最大であって、出現順ではない。
                            val pa = String::from_str("path:/a")
                            val ida = archive::term_id_of(traw, tsec.len(), span_of(&pa), pa.len())
                            val sa = archive::object_span(oraw, osec.len(), ida)
                            assert(sa.found, "path:/a should have a span")
                            assert_eq(sa.last - sa.first, 3i64)

                            # 1 行しか無い語は端が一致する。
                            val pb = String::from_str("path:/b")
                            val idb = archive::term_id_of(traw, tsec.len(), span_of(&pb), pb.len())
                            val sb = archive::object_span(oraw, osec.len(), idb)
                            assert(sb.found, "path:/b should have a span")
                            assert_eq(sb.last - sb.first, 0i64)

                            # /a のほうが早く始まる。
                            assert(sa.first < sb.first, "/a is seen before /b")

                            # 語彙索引の件数と食い違わないこと。
                            val post = archive::term_postings(traw, tsec.len(), span_of(&pa), pa.len())
                            assert_eq(post.doc_count, 2u64)
                        }
                        Option::None => { panic("empty object table") }
                    }
                }
                Option::None => { panic("empty term section") }
            }
        }
        Result::Err(e) => { panic("cannot open {seg}: {e}") }
    }
}

# ROADMAP 5 — フレーム単位の選択読み。飛ばしたフレームは**展開せず
# 長さだけ進める**ので、アリーナのオフセットは表が言うとおりのまま
# 残る。そこに古いバイトが居ても、読む記録が居ないから読まれない。
test "a skipped frame keeps the arena's shape without expanding" {
    val seg = build("build/ontology-fixture-frames")
    val crc = Crc32::new()
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(1024u64)
            val h = segfile::head_of(&f, &mut scratch)
            assert(h.ok, "segment header should read")

            # フレーム表は書いてあったが、これまで誰も読んでいなかった。
            var ftbuf = ByteWriter::with_capacity(1024u64)
            var starts: Vec<u64> = Vec::new()
            var lens: Vec<u64> = Vec::new()
            assert(segfile::frame_extents(&f, &h, &mut ftbuf, &mut starts, &mut lens),
                   "the frame table should read")
            assert_eq(starts.size(), h.n_frames)
            # 最初のフレームはアリーナの先頭から、全部で arena_bytes。
            assert_eq(starts.get(0u64), 0u64)
            var total: u64 = 0u64
            var i: u64 = 0u64
            while i < lens.size() {
                val l: u64 = lens.get(i)
                total = total + l
                i = i + 1u64
            }
            assert_eq(total, h.arena_bytes)

            var raw = ByteWriter::with_capacity(1048576u64)
            var full = ByteWriter::with_capacity(1048576u64)
            assert(segfile::expand_all(&f, &h, &crc, &mut raw, &mut full),
                   "expand_all should succeed")

            # 全部を要求したら expand_all と同じバイトになる。
            var want_all: Vec<u8> = Vec::new()
            var k: u64 = 0u64
            while k < h.n_frames {
                want_all.push(1u8)
                k = k + 1u64
            }
            var sel = ByteWriter::with_capacity(1048576u64)
            assert(segfile::expand_selected(&f, &h, &crc, &mut raw, &mut sel, &want_all),
                   "expand_selected(all) should succeed")
            assert_eq(sel.len(), full.len())
            val fw = full.span()
            val sw = sel.span()
            match fw {
                Option::Some(fb) => {
                    match sw {
                        Option::Some(sb) => {
                            assert(fb.bytes_eq(sb), "expanding every frame must match expand_all")
                        }
                        Option::None => { panic("no selected bytes") }
                    }
                }
                Option::None => { panic("no full bytes") }
            }

            # 何も要求しなければ、長さだけが正しく残る。
            var want_none: Vec<u8> = Vec::new()
            k = 0u64
            while k < h.n_frames {
                want_none.push(0u8)
                k = k + 1u64
            }
            var skipped = ByteWriter::with_capacity(1048576u64)
            assert(segfile::expand_selected(&f, &h, &crc, &mut raw, &mut skipped, &want_none),
                   "expand_selected(none) should still succeed")
            assert_eq(skipped.len(), h.arena_bytes)
        }
        Result::Err(e) => { panic("cannot open {seg}: {e}") }
    }
}
