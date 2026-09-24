# STORAGE_FORMAT.md §3 — `.seg` の形が黙って変わらないこと。
#
# ROADMAP §4 の「形式のバージョニング」は「凍結したら既存データを
# 読めなくする変更は入れない」と言っている。これは**バイト列を書き
# 留めるまで検査ではない**。2026-09-17 に索引の中身が 2 回変わった
# (`proto` の語を足した、重複 posting を落とした) が、それを知らせる
# テストは 1 つも無かった — 読めなくなったかどうかは誰も見ていない。
#
# ここは 2 つのことをする:
#
#   1. **ゴールデン** — 小さな合成ログから作った 1 セグメントを
#      バイト単位で固定する。変えたなら `--bless` で意図を示す
#   2. **読み側の約束** — セクション表・フレーム表・選択読み・
#      壊れたブロックの拒否。ゴールデンは「変わった」と言うだけで
#      「何が壊れた」とは言わないので、意味の側も別に押さえる
#
# 素材は合成で、アドレスはプライベート帯だけ (CLAUDE.md)。

fn line(text: str, out: &mut String) {
    out.push_str(text)
    out.push(10u8)
}

# ゴールデン用の素材は**小さく、全部の節を踏む**: HTTP 行 (語彙と
# リンク)、syslog 行 (host / tag)、ラベル行、日付の無い行。
fn small_fixture() -> String {
    var s = String::new()
    line(r#"10.0.0.1 - - [03/Sep/2026:12:00:01 +0000] "GET /a HTTP/1.1" 200 12 "-" "curl/8.0""#, &mut s)
    line(r#"10.0.0.2 - - [03/Sep/2026:12:00:02 +0000] "POST /b HTTP/1.0" 404 7 "-" "MJ12bot/1.4""#, &mut s)
    line("2026-09-03T12:00:03Z web01 sshd[101]: Accepted publickey for deploy", &mut s)
    line("2026-09-03T12:00:04Z host=web02 app=api level=error request timed out", &mut s)
    line("a line with no date at all", &mut s)
    s
}

fn joined(a: str, b: str) -> String {
    val head = String::from_str(a)
    val tail = String::from_str(b)
    val out = head.concat(&tail)
    out
}

fn write_text(path: &String, body: &String) {
    val wrote = io::write_file(path.to_str(), body.to_str())
    match wrote {
        Result::Ok(n) => { }
        Result::Err(e) => { panic("fixture: cannot write {path.to_str()}: {e}") }
    }
}

# `stem.log` を書き、`stem.seg` に 1 セグメントとして畳む。
fn build_segment(stem: str, body: &String) -> String {
    val log = joined(stem, ".log")
    write_text(&log, body)

    # The window has to hold the whole fixture: `load` fills it once,
    # and a smaller buffer quietly archives only the first lines (the
    # multi-frame fixture came out as one frame that way).
    var reader = LogReader::with_capacity(2097152u64)
    var rec = ParsedLine::new()
    var w = ArchiveWriter::new()
    val crc = Crc32::new()

    val loaded = reader.load(log.to_str())
    match loaded {
        Result::Ok(n) => { }
        Result::Err(e) => { panic("fixture: cannot read back {log.to_str()}: {e}") }
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
    val done = w.finish(stem, 7u64, &crc)
    match done {
        Result::Ok(n) => { }
        Result::Err(e) => { panic("fixture: cannot write segment: {e}") }
    }
    val seg = joined(stem, ".seg")
    seg
}

# ファイル全体を読む。ゴールデンと、バイトを 1 つ壊す検査に使う。
fn slurp_file(path: &String, out: &mut ByteWriter) -> u64 {
    val sized = fs::file_size(path.to_str())
    val n: u64 = match sized {
        Result::Ok(v) => v,
        Result::Err(e) => { panic("cannot size {path.to_str()}: {e}") }
    }
    out.clear()
    out.reserve(n)
    val room = out.room()
    val window: Span<u8> = match room {
        Option::Some(s) => s,
        Option::None => { panic("cannot hold {n} bytes") }
    }
    val got = io::read_file_into(path.to_str(), window)
    val read: u64 = match got {
        Result::Ok(v) => v,
        Result::Err(e) => { panic("cannot read {path.to_str()}: {e}") }
    }
    out.set_len(read)
    read
}

fn span_of(w: &ByteWriter) -> Span<u8> {
    val sp = w.span()
    match sp {
        Option::Some(s) => s,
        Option::None => { panic("span_of: empty writer") }
    }
}

# ---------------------------------------------------------------------
# 1. ゴールデン

# ここが落ちたときに問うべきは「意図した形式変更か」である。意図した
# ものなら `toy test poc/logsearch golden --bless`、そうでなければ
# **既存のアーカイブが読めなくなる変更**を入れたということ。
test "a segment's bytes are pinned, header and sections alike" {
    val body = small_fixture()
    val seg = build_segment("build/segment-golden", &body)
    var bytes = ByteWriter::with_capacity(65536u64)
    slurp_file(&seg, &mut bytes)
    assert_golden("tests/golden/segment-v3.seg", span_of(&bytes))
}

# ゴールデンは「動いた」とは言わない。同じセグメントを**読み側から**
# 見て、節がどこに在るかまで確かめる。
test "the pinned segment reads back with every section in place" {
    val body = small_fixture()
    val seg = build_segment("build/segment-head", &body)
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(4096u64)
            val h = segfile::head_of(&f, &mut scratch)
            assert(h.ok, "the header should read")
            assert_eq(h.segid, 7u64)
            assert_eq(h.records, 5u64)
            assert(h.has_records(), "records section")
            assert(h.has_terms(), "term section")
            assert(h.has_links(), "link section")
            assert(h.has_objects(), "object section")
            assert(h.has_streams(), "stream section")
            # 節は header の後ろに在り、互いに重ならない。
            assert(h.frames_off >= segfile::DATA_AT, "frames start after the directory")
            assert(h.recs_off >= h.frames_off + h.frames_len, "records follow the frames")
            assert(h.terms_off >= h.recs_off + h.recs_len, "terms follow the records")
            assert(h.strs_off >= h.terms_off + h.terms_len, "streams follow the terms")
            # 時刻の範囲は日付のある 4 行から取り、日付の無い行は動かさない。
            assert(h.ts_min < h.ts_max, "the segment should span time")
            assert_eq(h.n_frames, 1u64)
        }
        Result::Err(e) => { panic("cannot open {seg.to_str()}: {e}") }
    }
}

# 先頭 4 バイトはマジックで、**版が上がれば変わる**。v2 → v3 で
# `LSD2` → `LSD3` にしたときの約束 (STORAGE_FORMAT.md §3)。
test "the magic says which version wrote this" {
    val body = small_fixture()
    val seg = build_segment("build/segment-magic", &body)
    var bytes = ByteWriter::with_capacity(65536u64)
    slurp_file(&seg, &mut bytes)
    val b = span_of(&bytes)
    # The comparison is spelled with `u8` bindings: a bare `'L'` in an
    # `assert_eq` has no type naming it, so it stays `u32`.
    val m0: u8 = 'L'
    val m1: u8 = 'S'
    val m2: u8 = 'D'
    val m3: u8 = '3'
    assert_eq(b.get(0u64), m0)
    assert_eq(b.get(1u64), m1)
    assert_eq(b.get(2u64), m2)
    assert_eq(b.get(3u64), m3)
}

# ---------------------------------------------------------------------
# 2. 読み側の約束

# 256 KiB のフレームを跨ぐ素材。ゴールデンには大きすぎるので、
# ここでだけ作る。
fn big_fixture() -> String {
    var s = String::new()
    var i = 0u64
    while i < 9000u64 {
        val ip = 1u64 + i % 200u64
        val path = i % 37u64
        line("10.0.0.{ip} - - [03/Sep/2026:12:00:01 +0000] \"GET /p{path} HTTP/1.1\" 200 {i} \"-\" \"curl/8.0\"", &mut s)
        i = i + 1u64
    }
    s
}

test "the frame table describes the frames that are there" {
    val body = big_fixture()
    val seg = build_segment("build/segment-frames", &body)
    val crc = Crc32::new()
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(65536u64)
            val h = segfile::head_of(&f, &mut scratch)
            assert(h.ok, "header")
            assert(h.n_frames > 1u64, "the fixture should need more than one frame")

            var starts: Vec<u64> = Vec::new()
            var lens: Vec<u64> = Vec::new()
            assert(segfile::frame_extents(&f, &h, &mut scratch, &mut starts, &mut lens),
                   "the frame table should read")
            assert_eq(starts.size(), h.n_frames)
            assert_eq(lens.size(), h.n_frames)
            # 各フレームの raw 長を足すと arena になる。**足りない**形は
            # レコードの番地が全部ずれるので、ここが最初に気づく場所。
            var total = 0u64
            var i = 0u64
            while i < lens.size() {
                val l: u64 = lens.get(i)
                total = total + l
                i = i + 1u64
            }
            assert_eq(total, h.arena_bytes)
        }
        Result::Err(e) => { panic("cannot open {seg.to_str()}: {e}") }
    }
}

# 選択読みは**要らないフレームを読まない**が、arena の形は変えない
# (STORAGE_FORMAT.md §6)。要ったフレームの中身は全展開と 1 バイトも
# 違わない — 違えば、その番地のレコードが別の行になる。
test "expanding one frame gives that frame's bytes and keeps the shape" {
    val body = big_fixture()
    val seg = build_segment("build/segment-selected", &body)
    val crc = Crc32::new()
    val opened = File::open(seg.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(65536u64)
            val h = segfile::head_of(&f, &mut scratch)
            var starts: Vec<u64> = Vec::new()
            var lens: Vec<u64> = Vec::new()
            assert(segfile::frame_extents(&f, &h, &mut scratch, &mut starts, &mut lens), "frame table")

            var raw = ByteWriter::with_capacity(524288u64)
            var all = ByteWriter::with_capacity(h.arena_bytes + 65536u64)
            assert(segfile::expand_all(&f, &h, &crc, &mut raw, &mut all), "expand_all")
            assert_eq(all.len(), h.arena_bytes)

            # 2 本目だけ要る、と言う。
            var need: Vec<u8> = Vec::new()
            var i = 0u64
            while i < h.n_frames {
                if i == 1u64 { need.push(1u8) } else { need.push(0u8) }
                i = i + 1u64
            }
            var some = ByteWriter::with_capacity(h.arena_bytes + 65536u64)
            assert(segfile::expand_selected(&f, &h, &crc, &mut raw, &mut some, &need), "expand_selected")
            assert_eq(some.len(), h.arena_bytes)

            val at: u64 = starts.get(1u64)
            val len: u64 = lens.get(1u64)
            val a = span_of(&all)
            val b = span_of(&some)
            var k = 0u64
            var same = true
            while k < len && same {
                val x: u8 = a.get(at + k)
                val y: u8 = b.get(at + k)
                same = x == y
                if !same { panic("frame 1 differs at byte {k} of {len}") }
                k = k + 1u64
            }
            assert(same, "the needed frame should match the full expansion")

            # `need` が短ければ残りは全部展開する (degrade to expand_all)。
            var short_need: Vec<u8> = Vec::new()
            short_need.push(0u8)
            var rest = ByteWriter::with_capacity(h.arena_bytes + 65536u64)
            assert(segfile::expand_selected(&f, &h, &crc, &mut raw, &mut rest, &short_need), "short need")
            assert_eq(rest.len(), h.arena_bytes)
        }
        Result::Err(e) => { panic("cannot open {seg.to_str()}: {e}") }
    }
}

# 1 バイト書き換えたセグメントは**読めないと言う**。CRC を持っている
# のは、黙って別のレコードを返さないためである。
test "a flipped byte in a block is caught, not decoded" {
    val body = small_fixture()
    val seg = build_segment("build/segment-crc", &body)
    val crc = Crc32::new()
    var bytes = ByteWriter::with_capacity(65536u64)
    slurp_file(&seg, &mut bytes)

    # レコード表の途中を 1 ビット反転する。
    val opened = File::open(seg.to_str())
    var recs_at = 0u64
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(4096u64)
            val h = segfile::head_of(&f, &mut scratch)
            recs_at = h.recs_off + h.recs_len / 2u64
        }
        Result::Err(e) => { panic("cannot open {seg.to_str()}: {e}") }
    }
    # `ByteWriter` は途中の 1 バイトを書き換える口を持たないので、
    # 1 バイトだけ違う写しを作る。
    var flipped = ByteWriter::with_capacity(bytes.len() + 16u64)
    var i = 0u64
    while i < bytes.len() {
        val b = bytes.byte_at(i)
        if i == recs_at { flipped.put_u8(b ^ 1u8) } else { flipped.put_u8(b) }
        i = i + 1u64
    }
    val broken = String::from_str("build/segment-crc-broken.seg")
    val wrote = io::write_file_bytes(broken.to_str(), span_of(&flipped))
    match wrote {
        Result::Ok(n) => { }
        Result::Err(e) => { panic("cannot write the broken copy: {e}") }
    }

    val reopened = File::open(broken.to_str())
    match reopened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(4096u64)
            val h = segfile::head_of(&f, &mut scratch)
            assert(h.ok, "the header itself is still intact")
            var raw = ByteWriter::with_capacity(65536u64)
            var out = ByteWriter::with_capacity(65536u64)
            assert(!segfile::load_block(&f, h.recs_off, h.recs_len, &crc, &mut raw, &mut out),
                   "a corrupt record table should be refused")
        }
        Result::Err(e) => { panic("cannot open the broken copy: {e}") }
    }
}

# 途中で切れたファイルはヘッダの段で止まる。
test "a truncated segment is not a segment" {
    val body = small_fixture()
    val seg = build_segment("build/segment-short", &body)
    var bytes = ByteWriter::with_capacity(65536u64)
    slurp_file(&seg, &mut bytes)
    bytes.truncate(segfile::HEAD_BYTES / 2u64)
    val short = String::from_str("build/segment-short-cut.seg")
    val wrote = io::write_file_bytes(short.to_str(), span_of(&bytes))
    match wrote {
        Result::Ok(n) => { }
        Result::Err(e) => { panic("cannot write the short copy: {e}") }
    }
    val opened = File::open(short.to_str())
    match opened {
        Result::Ok(f) => {
            var scratch = ByteWriter::with_capacity(4096u64)
            val h = segfile::head_of(&f, &mut scratch)
            assert(!h.ok, "half a header should not read as a segment")
        }
        Result::Err(e) => { panic("cannot open the short copy: {e}") }
    }
}
