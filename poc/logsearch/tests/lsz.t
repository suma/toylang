# STORAGE_FORMAT.md §3 — LSZ1 のラウンドトリップ。
#
# この設計で**唯一取り返しのつかない箇所**である。索引が壊れても
# 作り直せるし、カタログはキャッシュだが、LSZ1 が読めなくなれば
# 過去のログがそのまま失われる。だからここは 3 つの別々の約束を
# 別々に固める:
#
#   1. 圧縮して展開すれば元に戻る (どんな入力でも)
#   2. 壊れた / 途中で切れたフレームは `false` で止まる (黙って別の
#      バイト列を返さない)
#   3. 同じ入力は同じバイト列に圧縮される (ゴールデン)。3 を破る変更は
#      1 を保っていても、**書かれた `.seg` のバイトが変わる**ことを意味する
#
# 実ログは使わない (CLAUDE.md — 実在のアドレスがゴールデンに入る)。
# 代わりに、エンコーダの分岐を 1 つずつ踏む**形**を合成する: トークンの
# nibble が 15 に達して varint に溢れる長さ、16 バイトのブロック比較の
# 境界、64 KiB の窓の端、距離 1 の run。
#
# プロパティ版 (ランダムな形・長さ) は `lsz_props` 関数に契約として
# 書いてあり、`--check` が掃く (ファイル末尾のコメント)。

import std.testing
import bytes
import lsz

# ---------------------------------------------------------------------
# 入力の合成

# xorshift64。0 は不動点なので避ける。
fn next_rand(s: u64) -> u64 {
    var x = s
    if x == 0u64 { x = 0x9E3779B97F4A7C15u64 }
    x = x ^ (x << 13u64)
    x = x ^ (x >> 7u64)
    x = x ^ (x << 17u64)
    x
}

# 形 `shape` の入力を `len` バイト作る。
#
#   0  一様乱数 — 圧縮されない。リテラル長の varint だけを通る
#   1  4 文字のアルファベット — 短い一致が密に出る
#   2  1 バイトの run — 距離 1、一致長の varint が長くなる
#   3  周期 `1 + seed % 40` の繰り返し — 距離 < 16 と >= 16 の両方の複写
#   4  ログ風の行 — 共通の接頭辞と変わる数字
fn synth(shape: u64, seed: u64, len: u64) -> ByteWriter {
    var w = ByteWriter::with_capacity(len + 16u64)
    var s = next_rand(seed + 1u64)
    var i = 0u64
    val period = 1u64 + seed % 40u64
    while i < len {
        s = next_rand(s)
        var b = 0u64
        if shape == 0u64 {
            b = s & 0xFFu64
        } elif shape == 1u64 {
            b = 97u64 + (s & 3u64)
        } elif shape == 2u64 {
            b = 120u64
        } elif shape == 3u64 {
            b = 65u64 + ((i % period) * 7u64) % 26u64
        } else {
            # 60 バイトの行のうち、先頭 48 バイトは固定で残りが数字。
            val col = i % 60u64
            if col == 59u64 {
                b = 10u64
            } elif col < 48u64 {
                b = 97u64 + (col % 26u64)
            } else {
                b = 48u64 + (s % 10u64)
            }
        }
        w.put_u8(b as u8)
        i = i + 1u64
    }
    w
}

fn span_of(w: &ByteWriter) -> Span<u8> {
    val sp = w.span()
    match sp {
        Option::Some(s) => s,
        Option::None => { panic("span_of: empty writer") }
    }
}

# `w` の [from, from + len) を圧縮する。
fn compress(w: &ByteWriter, from: u64, len: u64) -> ByteWriter {
    var out = ByteWriter::with_capacity(len + len / 2u64 + 64u64)
    var z = Lsz::new()
    val src = span_of(w)
    val n = z.encode(src, from, len, &mut out)
    assert_eq(n, out.len())
    out
}

# `c` を `raw_len` バイトへ展開できれば、それが `w` の
# [from, from + raw_len) と一致するか。展開に失敗したら false。
unsafe fn expands_to(c: &ByteWriter, w: &ByteWriter, from: u64, raw_len: u64) -> bool {
    var out = ByteWriter::with_capacity(raw_len + 16u64)
    var ok = true
    if c.len() == 0u64 {
        # 空の圧縮列を Span にできないので、長さ 0 のときだけ別扱い。
        ok = raw_len == 0u64
    } else {
        val csp = span_of(c)
        ok = lsz::decode_frame(csp, 0u64, c.len(), raw_len, &mut out)
    }
    if ok && raw_len > 0u64 {
        ok = out.len() == raw_len
        if ok {
            val got = span_of(&out)
            val want = span_of(w)
            var i = 0u64
            while i < raw_len && ok {
                val a: u8 = got.get(i)
                val b: u8 = want.get(from + i)
                ok = a == b
                i = i + 1u64
            }
        }
    }
    ok
}

unsafe fn roundtrips(shape: u64, seed: u64, len: u64) -> bool {
    val w = synth(shape, seed, len)
    val c = compress(&w, 0u64, len)
    expands_to(&c, &w, 0u64, len)
}

# ---------------------------------------------------------------------
# プロパティ (`--check` が掃く)

# 1. どの形・長さ・種でも元に戻る。
#
# 範囲は `requires` ではなく剰余で絞る。生成器は u64 全域から引くので、
# `requires len <= 3000` だと大半が捨てられて THIN になる。
# 長さを 3000 で抑えているのは `--check` のループ予算のため
# (`Lsz::new` と `reset` だけで 65536 回回る)。長い入力は下の
# `test` が固定の値で踏む。
pub unsafe fn lsz_props(shape: u64, seed: u64, len: u64) -> bool
    ensures result
{
    roundtrips(shape % 5u64, seed, len % 3001u64)
}

# 2. SIMD の一致長はスカラー版と 1 バイトも違わない。違えば**展開は
#    通るのに圧縮結果が変わる** (lsz.t の `match_len_scalar` のコメント)
#    ので、ラウンドトリップでは見つからない。
pub unsafe fn match_len_agrees(shape: u64, seed: u64, cand: u64, gap: u64) -> bool
    ensures result
{
    val len = 600u64
    val w = synth(shape % 5u64, seed, len)
    val src = span_of(&w)
    val c = cand % 200u64
    val pos = c + 1u64 + gap % 199u64
    lsz::match_len(src, c, pos, len) == lsz::match_len_scalar(src, c, pos, len)
}

# ---------------------------------------------------------------------
# 境界

test "nothing compresses to nothing and expands back" {
    val w = synth(0u64, 1u64, 0u64)
    val c = compress(&w, 0u64, 0u64)
    assert_eq(c.len(), 0u64)
    assert(expands_to(&c, &w, 0u64, 0u64), "empty frame")
}

# min_match より短い入力は一致を探すループに入らず、末尾のリテラルだけになる。
test "inputs shorter than a match are literals only" {
    var n = 1u64
    while n < 8u64 {
        assert(roundtrips(2u64, n, n), "run of {n}")
        assert(roundtrips(0u64, n, n), "random {n}")
        n = n + 1u64
    }
}

# リテラル長 15 で nibble が溢れて varint が付く。14 / 15 / 16 はその境目。
# 16 / 17 はハッシュの 4 本まとめ読み (`pos + 16 <= end`) の境目でもある。
test "every length around the nibble and the sixteen-byte block" {
    var shape = 0u64
    while shape < 5u64 {
        var n = 12u64
        while n < 40u64 {
            assert(roundtrips(shape, 7u64, n), "shape {shape} len {n}")
            n = n + 1u64
        }
        shape = shape + 1u64
    }
}

# 一致長 - 4 が 15 で溢れる。run は 1 本の長い一致になるので、varint が
# 1 バイト (< 128) と 2 バイト以上 (長い run) の両方をここで踏む。
test "a long run is one match with a long length" {
    val w = synth(2u64, 3u64, 100000u64)
    val c = compress(&w, 0u64, 100000u64)
    assert(c.len() < 64u64, "a 100 KiB run compressed to {c.len()} bytes")
    assert(expands_to(&c, &w, 0u64, 100000u64), "long run")
    assert(roundtrips(2u64, 3u64, 4u64 + 15u64 + 127u64 + 1u64), "one-byte varint edge")
    assert(roundtrips(2u64, 3u64, 4u64 + 15u64 + 128u64 + 1u64), "two-byte varint edge")
}

# 圧縮されない入力はリテラルだけで、しかも 64 KiB を超える。
test "incompressible input survives, and does not grow much" {
    val len = 70000u64
    val w = synth(0u64, 11u64, len)
    val c = compress(&w, 0u64, len)
    assert(c.len() <= len + len / 200u64 + 16u64, "random grew to {c.len()}")
    assert(expands_to(&c, &w, 0u64, len), "random 70k")
}

# 周期 < 16 の繰り返しは距離 < 16 の複写 (1 バイトずつ)、
# 周期 >= 16 はブロック複写 (`append_from_self`) を通る。
test "short and long distances both copy correctly" {
    var seed = 0u64
    while seed < 40u64 {
        assert(roundtrips(3u64, seed, 2000u64), "period {1u64 + seed}")
        seed = seed + 1u64
    }
}

test "log-shaped lines round-trip and compress" {
    val len = 200000u64
    val w = synth(4u64, 5u64, len)
    val c = compress(&w, 0u64, len)
    assert(c.len() < len / 2u64, "log lines compressed to {c.len()} of {len}")
    assert(expands_to(&c, &w, 0u64, len), "log lines")
}

# 窓は 65535 バイト。ちょうど 65535 前と 65536 前に同じ 64 バイトを置く。
# 前者は一致として符号化してよく、後者は**してはならない** (u16 に入らない)。
test "a match reaches back exactly as far as the window and no further" {
    var d = 65534u64
    while d < 65538u64 {
        val len = d + 64u64
        var w = ByteWriter::with_capacity(len + 16u64)
        var s = next_rand(d)
        var i = 0u64
        while i < len {
            if i >= d {
                w.put_u8(w.byte_at(i - d))
            } else {
                s = next_rand(s)
                w.put_u8((s & 0xFFu64) as u8)
            }
            i = i + 1u64
        }
        val c = compress(&w, 0u64, len)
        assert(expands_to(&c, &w, 0u64, len), "repeat at distance {d}")
        d = d + 1u64
    }
}

# アーカイブはフレームを大きなバッファの途中から圧縮する。`from` が 0 で
# ないとき、窓の手前 (別フレーム) のバイトを一致に使ってはならない。
test "a frame that starts mid-buffer does not reach before its start" {
    val w = synth(4u64, 9u64, 12000u64)
    val from = 6000u64
    val len = 6000u64
    val c = compress(&w, from, len)
    assert(expands_to(&c, &w, from, len), "second half on its own")
}

# 同じ Lsz を使い回すと、前の encode のハッシュ表が残っている。
# `encode` が入口で `reset` しなければ、2 回目が 1 回目のバイトを指す。
test "a reused compressor gives the same bytes as a fresh one" {
    val a = synth(4u64, 1u64, 5000u64)
    val b = synth(4u64, 2u64, 5000u64)
    var z = Lsz::new()
    var first = ByteWriter::with_capacity(8000u64)
    z.encode(span_of(&a), 0u64, 5000u64, &mut first)
    var second = ByteWriter::with_capacity(8000u64)
    z.encode(span_of(&b), 0u64, 5000u64, &mut second)
    val fresh = compress(&b, 0u64, 5000u64)
    # `span_of(..).bytes_eq(..)` is refused by the compiled lanes (a
    # method call needs a bare receiver), so the spans are bound first.
    val got = span_of(&second)
    val want = span_of(&fresh)
    assert(got.bytes_eq(want), "reused compressor differs")
}

# ---------------------------------------------------------------------
# 壊れたフレーム

test "a truncated frame is refused, at every cut" {
    val len = 3000u64
    val w = synth(4u64, 13u64, len)
    val c = compress(&w, 0u64, len)
    var cut = 1u64
    while cut < c.len() {
        var out = ByteWriter::with_capacity(len + 16u64)
        val ok = lsz::decode_frame(span_of(&c), 0u64, cut, len, &mut out)
        assert(!ok, "cut at {cut} of {c.len()} was accepted")
        cut = cut + 1u64
    }
}

test "a frame that claims more or fewer bytes than it holds is refused" {
    val len = 3000u64
    val w = synth(4u64, 17u64, len)
    val c = compress(&w, 0u64, len)
    var more = ByteWriter::with_capacity(len + 64u64)
    assert(!lsz::decode_frame(span_of(&c), 0u64, c.len(), len + 1u64, &mut more), "claimed one more")
    var fewer = ByteWriter::with_capacity(len + 64u64)
    assert(!lsz::decode_frame(span_of(&c), 0u64, c.len(), len - 1u64, &mut fewer), "claimed one fewer")
}

# 距離 0 と、まだ書いていない位置を指す距離。どちらも「前のフレームの
# バイト」や未初期化のバイトを読みに行く形なので、展開前に止める。
test "a match that points before the start of the output is refused" {
    # token: literal 1, match 4 / 'a' / distance 2 (> 1 produced)
    var bad = ByteWriter::with_capacity(16u64)
    bad.put_u8(0x10u8)
    bad.put_u8(97u8)
    bad.put_u16(2u64)
    var out = ByteWriter::with_capacity(64u64)
    assert(!lsz::decode_frame(span_of(&bad), 0u64, bad.len(), 5u64, &mut out), "distance past start")

    var zero = ByteWriter::with_capacity(16u64)
    zero.put_u8(0x10u8)
    zero.put_u8(97u8)
    zero.put_u16(0u64)
    var out2 = ByteWriter::with_capacity(64u64)
    assert(!lsz::decode_frame(span_of(&zero), 0u64, zero.len(), 5u64, &mut out2), "distance 0")
}

# ---------------------------------------------------------------------
# ゴールデン — 書かれた `.seg` のバイトが黙って変わらないこと

# 形ごとに 1 本ずつ、エンコーダの出力を固定する。ここが落ちたら、
# 展開できるかではなく「**既存のアーカイブと新しいアーカイブが別の
# バイト列になる**」ことを意味する。意図した変更なら `--bless`。
test "the encoder's output is pinned byte for byte" {
    var shape = 0u64
    while shape < 5u64 {
        val w = synth(shape, 42u64, 4096u64)
        val c = compress(&w, 0u64, 4096u64)
        assert_golden("tests/golden/lsz-shape{shape}.lsz", span_of(&c))
        shape = shape + 1u64
    }
}

# プロパティの実行 (リポジトリルートから):
#
#   ./target/release/interpreter --core-modules core \
#       --core-modules poc/logsearch/src --check poc/logsearch/tests/lsz.t
