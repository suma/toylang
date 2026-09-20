# HTTP_API.md §2 — ラベル辞書。
#
# `/v1/labels` は「何で絞れるか」を答える。セグメントを毎回開いて
# 語彙を集めるのではなく、マウントごとの辞書から答える形にした。
# 辞書はカタログと同じく**キャッシュ**で、いつでも `seg/` から作り
# 直せる。だから固めるべきは「速い」ことではなく:
#
#   1. **全走査と一致する** — 辞書の答えは、セグメントを歩いた
#      答えと同じでなければ意味がない (ROADMAP §4-3 と同じ考え方)
#   2. **足し引きが効く** — 書いたら足され、消したら引かれる。
#      引いて 0 になった値は消える (もう無い値は無い)
#   3. **ファイルが往復する** — 書いて読み直して同じ表になる。
#      壊れていたら**空**として扱う (直す価値のあるものは無い)
#
# 素材は合成で、アドレスはプライベート帯だけ (CLAUDE.md)。

fn lb_line(text: str, out: &mut String) {
    out.push_str(text)
    out.push(10u8)
}

fn lb_build(stem: str, body: &String) -> String {
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

# 辞書が答える件数。キーそのものは値が空の行に入っている。
fn lb_count(d: &LabelDict, key: str, value: str) -> u64 {
    val k = String::from_str(key)
    val v = String::from_str(value)
    var i: u64 = 0u64
    var out: u64 = 0u64
    while i < d.size() {
        val dk = d.key_at(i)
        val dv = d.value_at(i)
        if dk.eq(&k) && dv.eq(&v) { out = d.count_at(i) }
        i = i + 1u64
    }
    out
}

# 全走査の答え (オラクル)。`tally` は語彙をそのまま数える。
fn lb_oracle(seg: &String, term: str) -> u64 {
    val crc = Crc32::new()
    var segs: Vec<String> = Vec::new()
    val copy = seg.clone()
    segs.push(copy)
    val tal = query::tally(&segs, "", false, &crc)
    val want = String::from_str(term)
    var i: u64 = 0u64
    var out: u64 = 0u64
    while i < tal.size() {
        val nm: &String = tal.names.borrow(i)
        if nm.eq(&want) { out = tal.counts.get(i) }
        i = i + 1u64
    }
    out
}

# ---------------------------------------------------------------------

# 辞書と全走査が食い違ったら、辞書は嘘をついている。**値ごとに**
# 突き合わせる — 合計だけ合っていても分布が違えば役に立たない。
test "the dictionary says what a walk of the segment says" {
    var body = String::new()
    lb_line("2026-09-03T12:00:01Z app=api level=error one", &mut body)
    lb_line("2026-09-03T12:00:02Z app=api level=error two", &mut body)
    lb_line("2026-09-03T12:00:03Z app=api level=info three", &mut body)
    lb_line("2026-09-03T12:00:04Z app=web level=info four", &mut body)
    val seg = lb_build("build/labels-agree", &body)
    val crc = Crc32::new()

    var d = LabelDict::new()
    labels::merge_segment(&mut d, &seg, true, &crc)

    assert_eq(lb_count(&d, "app", "api"), lb_oracle(&seg, "app:api"))
    assert_eq(lb_count(&d, "app", "web"), lb_oracle(&seg, "app:web"))
    assert_eq(lb_count(&d, "level", "error"), lb_oracle(&seg, "level:error"))
    assert_eq(lb_count(&d, "level", "info"), lb_oracle(&seg, "level:info"))

    # 値が 3 件と 1 件なら、キーは 4 件。
    assert_eq(lb_count(&d, "app", "api"), 3u64)
    assert_eq(lb_count(&d, "app", "web"), 1u64)
    assert_eq(lb_count(&d, "app", ""), 4u64)
    assert_eq(lb_count(&d, "level", ""), 4u64)

    # 語 (`one` / `two`) はラベルではない — `:` を持たないものは
    # 辞書に入らない。
    assert_eq(lb_count(&d, "one", ""), 0u64)
}

# 2 本ぶん足して、1 本ぶん引けば、1 本ぶんが残る。**引いたあとが
# 最初の 1 本と一致する**ことまで見る。
test "what was added can be taken back out" {
    var b1 = String::new()
    lb_line("2026-09-03T12:00:01Z app=api level=error one", &mut b1)
    lb_line("2026-09-03T12:00:02Z app=api level=info two", &mut b1)
    val seg1 = lb_build("build/labels-add-a", &b1)

    var b2 = String::new()
    lb_line("2026-09-03T12:10:01Z app=web level=error three", &mut b2)
    lb_line("2026-09-03T12:10:02Z app=api level=error four", &mut b2)
    val seg2 = lb_build("build/labels-add-b", &b2)

    val crc = Crc32::new()
    var both = LabelDict::new()
    labels::merge_segment(&mut both, &seg1, true, &crc)
    labels::merge_segment(&mut both, &seg2, true, &crc)
    assert_eq(lb_count(&both, "app", "api"), 3u64)
    assert_eq(lb_count(&both, "app", "web"), 1u64)
    assert_eq(lb_count(&both, "level", "error"), 3u64)

    labels::merge_segment(&mut both, &seg2, false, &crc)
    var only1 = LabelDict::new()
    labels::merge_segment(&mut only1, &seg1, true, &crc)

    assert_eq(lb_count(&both, "app", "api"), lb_count(&only1, "app", "api"))
    assert_eq(lb_count(&both, "level", "error"), lb_count(&only1, "level", "error"))
    assert_eq(lb_count(&both, "level", "info"), lb_count(&only1, "level", "info"))

    # `app=web` はもうどこにも無い。0 件の行を残すと、辞書は
    # 「値はあるが 1 件も無い」という言えない状態を持つことになる。
    assert_eq(lb_count(&both, "app", "web"), 0u64)
    assert_eq(both.size(), only1.size())
}

# 書いて読み直して同じ表になる。
test "the dictionary file round-trips" {
    var body = String::new()
    lb_line("2026-09-03T12:00:01Z app=api level=error one", &mut body)
    lb_line("2026-09-03T12:00:02Z app=web level=info two", &mut body)
    val seg = lb_build("build/labels-roundtrip", &body)
    val crc = Crc32::new()

    var d = LabelDict::new()
    labels::merge_segment(&mut d, &seg, true, &crc)

    var w = ByteWriter::with_capacity(1024u64)
    labels::encode_dict(&d, 7u64, &mut w, &crc)

    var back = LabelDict::new()
    val sp = w.span()
    match sp {
        Option::Some(b) => {
            val gen = labels::decode_dict(b, w.len(), &mut back, &crc)
            assert_eq(gen, 7u64)
        }
        Option::None => { panic("the encoder produced nothing") }
    }
    assert_eq(back.size(), d.size())
    assert_eq(lb_count(&back, "app", "api"), lb_count(&d, "app", "api"))
    assert_eq(lb_count(&back, "app", ""), lb_count(&d, "app", ""))
    assert_eq(lb_count(&back, "level", "info"), lb_count(&d, "level", "info"))
}

# 1 バイト壊れていたら**空**として読む。辞書は作り直せるので、
# 半分だけ信じる理由が無い。
test "a damaged dictionary reads as empty, not as half a dictionary" {
    var body = String::new()
    lb_line("2026-09-03T12:00:01Z app=api level=error one", &mut body)
    val seg = lb_build("build/labels-damaged", &body)
    val crc = Crc32::new()

    var d = LabelDict::new()
    labels::merge_segment(&mut d, &seg, true, &crc)
    var w = ByteWriter::with_capacity(1024u64)
    labels::encode_dict(&d, 3u64, &mut w, &crc)
    assert(w.len() > labels::dict_head_bytes(), "the fixture should have rows")

    # 本文の 1 バイトを反転する。CRC はヘッダに入っているので、
    # 反転は必ず見つかる。
    var flipped = ByteWriter::with_capacity(w.len() + 8u64)
    val sp = w.span()
    match sp {
        Option::Some(b) => {
            var i: u64 = 0u64
            while i < w.len() {
                var byte = b.get(i)
                if i == labels::dict_head_bytes() { byte = byte ^ 0x01u8 }
                flipped.put_u8(byte)
                i = i + 1u64
            }
        }
        Option::None => { panic("the encoder produced nothing") }
    }

    var back = LabelDict::new()
    val bad = flipped.span()
    match bad {
        Option::Some(b) => {
            val gen = labels::decode_dict(b, flipped.len(), &mut back, &crc)
            assert_eq(gen, 0u64)
        }
        Option::None => { panic("the copy produced nothing") }
    }
    assert_eq(back.size(), 0u64)
}
