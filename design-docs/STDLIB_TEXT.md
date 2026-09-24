# STDLIB TEXT — `str` / `String` / `char` / `u8` の境界と Unicode の線引き

> **状態: T0〜T5 すべて landing 済み (2026-09-03)。** 以下は決定の記録と
> して残す。非目標 (grapheme / 正規化 / Unicode 大小変換 / 照合順序 /
> 正規表現 / SSO / interning) は末尾の節のとおり据え置き。

> 対象: `core/std/str.t` / `core/std/string.t` / `core/std/char.t` (と `frontend` の `BuiltinMethod::Str*`)
> 状態の正本: [`todo.md`](todo.md) の **STDLIB-TEXT**
> 俯瞰と優先順位: [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md)
> 実測: 2026-09-03 (この文書の数値と診断文はすべてこの日に取った)

## Status snapshot

| 項目 | 状態 |
|---|---|
| `str` | 不変ハンドル。`len` / `concat` / `hash` / `as_ptr` は 3 レーン、**`substring` / `trim` / `to_upper` / `to_lower` / `contains` / `split` は interpreter だけ** |
| `String` | heap byte buffer。上記 6 相当を**純 toylang で 3 レーン**持つ (SIMD 化済み) |
| `char` | `type char = u32` の 1 行だけ。分類 (`is_digit` 等) も変換も無い |
| codepoint → バイト | `push_char` (UTF-8 encode、RFC 3629) |
| バイト → codepoint | **無い** (`chars()` に相当するものが 1 つも無い) |
| `Ord for str` | 無い (`Vec<str>::sort()` は `[E0010]` bound violation) |
| 非 UTF-8 バイト列 | **tree-walker と compiled lane で割れる** (下記 実測 4) |
| Unicode の線引き | **未決** — どの文書にも書かれていない |

## なぜ今これを設計するか

**この分野は「機能が足りない」より先に「規約が無い」。** 3 つの型
(`str` / `String` / `char`) と 1 つの primitive (`u8`) が同じ「文字列」の
仕事を分け合っているのに、**どれが何を担当するかを書いた場所が無い**。
その結果が下の実測で、**4 件はドキュメントの不足ではなく今日壊れている
挙動**になっている。

そして TEXT はこの後に続く分野の土台でもある — FS-PATH (path は文字列の
走査)、SERIALIZE (JSON は文字列の生成と走査)、LOG (メッセージの整形)、
TIME (日付のパースと整形) はすべてこの上に載る。**境界を決めずに 4 つ
足すと、4 通りの流儀で `str` と `String` を混ぜた API ができる。**

## 測ったこと (2026-09-03)

すべて `--all-backends` (interpreter / cranelift JIT / AOT) と、4 件目だけ
`compiler/tests/consistency/` の 4 レーン harness で確認した。

1. **`str` の変換系 6 method は compiled lane に存在しない。**
   `val u: str = s.to_upper()` は型検査を通り interpreter で動くが、JIT と
   AOT は `compiler MVP requires the method receiver to be a struct or
   enum binding` で拒否する。同じ形で落ちるのは
   `substring` / `trim` / `to_upper` / `to_lower` / `contains` / `split`。
   通るのは `len` / `concat` / `hash` / `as_ptr` の 4 つだけ。
   理由は IR にある: `InstKind` には **`StrLen` と `StrConcat` しか無い**
   (`compiler/src/codegen/lower_inst.rs`)。つまり「MVP の制限」ではなく
   **6 つは最初から compiled lane に無い**。型検査器の
   `BuiltinMethod` 表 (`frontend/src/type_checker/builtin.rs`) だけが
   全部持っている、典型的な TYPECHECK-LIES。

2. **同じ 6 つは `String` では 3 レーンで動く。**
   `String::from_str("hello world")` に対する `to_upper` / `substring` /
   `contains` は `--all-backends` が一致した。純 toylang で書いてあり
   (`core/std/string.t`)、`fold_ascii_case` は SIMD 化までされている。
   **足りないのは実装ではなく、どちらを正面に置くかの決定。**

3. **`str` 同士の順序比較は型エラーで、しかも診断が壊れている。**
   ```
   [E0002] Type mismatch in comparison operation: incompatible types str and str
   ```
   C0(a) で直した `expected Struct(...), found Struct(...同じ...)` と
   同じ「**同じ型を不一致と言う**」形。`Vec<str>::sort()` の方は正しく
   `[E0010] Method 'sort' generic parameter 'T' bound violation: expected
   Ord, got str` と言う。

4. **非 UTF-8 のバイト列は tree-walker と compiled lane で長さが違う。**
   `String` に `0xFF` `0xFE` を push して `to_str()` し `len()` を返す
   プログラムを 4 レーンで走らせた:

   | レーン | `s.len()` |
   |---|---|
   | tree-walker | **6** |
   | IR VM / JIT / AOT | **2** |

   tree-walker の `__builtin_str_from_bytes` は
   `String::from_utf8_lossy` (`interpreter/src/evaluation/builtin.rs`) で、
   2 バイトが U+FFFD 2 個 = 6 バイトに化ける。compiled lane はバイトを
   そのまま持つ。**`str` が妥当な UTF-8 かどうかを決めていない**ことが、
   そのまま 4 レーンの不一致になっている。

5. **`str.substring` の非境界 index は Rust の生 panic になる。**
   `"aébc".substring(0, 2)` (`é` の途中で切る) が
   ```
   thread 'main' panicked at interpreter/src/evaluation/call.rs:1530:53:
   end byte index 2 is not a char boundary; it is inside 'é' (bytes 1..3 of string)
   ```
   で落ちる。toylang の `panic` ではないので**行番号も backtrace も
   出ない** (DEBUG-OBS が全レーンで揃えた経路の外)。ちなみに
   `String::substring` は同じ位置で切っても panic せず、バイトを半分だけ
   持つ `String` を返す — **同じ名前の 2 つの method が別の規約で動いて
   いる**。

6. **`impl Ord for str` は user 空間で今日書ける。**
   `as_ptr` + `__builtin_ptr_read` のバイト比較で書いた `impl Ord for str`
   が、`fn smaller<T: Ord>(a: T, b: T)` という generic 位置経由で
   **3 レーンとも一致した**。todo.md の STDLIB-ORD が書いていた
   「generic context で AOT が表現できない」は**もう成り立っていない**
   (`&Self` の lowering と primitive receiver の dispatch が
   2026-08-29 / 08-31 に入ったため)。残る論点は表現力ではなく**確保**で、
   interpreter の `str::as_ptr()` は呼ぶたびに len+1 バイト確保する
   (`core/std/str.t`) ので、比較を toylang のバイトループで書くと
   sort が O(n log n) 回確保する。

## 既存の決定から引く制約

1. **str 系ヘルパの toylang 化は測って却下されている** (RUNTIME-PORT
   R3/R4: interpreter で 20〜1000 倍)。`Hash for str` が extern なのも
   同じ理由 (`core/std/hash.t` に記録がある)。→ **`str` に足すものは
   extern、`String` に足すものは純 toylang**、が既定。
2. **意味論の実装を増やさない。** 同じ意味が tree-walker / IR VM+AOT /
   JIT に独立実装されるので、IR 命令を 1 つ足すことは実装を 3 つ足す
   ことに近い (CLAUDE.md「横断的な変更をするとき」)。**既に 3 レーンで
   動いている純 toylang の実装があるなら、そちらを正面にする方が安い。**
3. **決定性** (`docs/language.md` の Output 規約)。ロケール依存の
   大文字小文字・照合順序は入れられない。`strftime` を UTC 固定に
   したのと同じ理由。
4. **`char` リテラルの narrowing (CHAR-LITERAL-NUM) は既にある** —
   `b == '0'` / `c - '0'` が u8 でも u32 でも書ける。分類関数を
   足すときにキャストを要求しない土台がもうある。2026-09-23 から
   **`match` の腕でも同じ** (`match b { '0'..':' => .. }`、
   CHAR-LITERAL-MATCH) で、文字の表を網羅性検査の内側で書ける。
   文字列リテラルは `\"` と raw 文字列 (`r#"..."#`) を持つ
   (STR-ESCAPE-HATCH)。2026-09-24 から **`String` も文字列リテラルの
   腕で match できる** (MATCH-STRING-LITERAL、`eq_str` の guard に
   書き換えるので確保なし)。
5. **受け入れは 4 レーン一致**。文字列は tree-walker だけ Rust の
   `String` で持っているので、**この分野は 3 レーンでは足りない**
   (実測 4 がまさにそれ)。

## 1. 役割 — 4 つの型に 1 行ずつ

| 型 | 役割 | 所有 | 中身 |
|---|---|---|---|
| `str` | **不変ハンドル**。読むだけ | 持たない | 妥当な UTF-8 (§2) |
| `String` | **所有する可変バッファ** | 持つ (`Drop`) | 任意のバイト列 |
| `char` | **Unicode scalar value 1 個** | — | `u32` (U+0000〜U+10FFFF、surrogate 除く) |
| `u8` | **バイト 1 個** | — | 0〜255 |

ここから 1 つの規則が出る:

> **`str` は所有しないので、新しい文字列を作る API を持たない。**

`to_upper` / `substring` / `trim` / `split` は**新しいバッファ**を要求する
から `String` の仕事で、`len` / `contains` / `starts_with` / `find` /
`lt` は**読むだけ**だから `str` に置ける。実測 1 が「6 個のうち 6 個とも
compiled lane に無い」と言っているのは偶然ではなく、**確保する 5 つ**を
言語の builtin に持たせようとしていたことの反映でもある
(`split` に至っては `[str]` を返すので配列の確保まで要る)。

## 2. `str` の不変 — **妥当な UTF-8 とする**

実測 4 の割れ方を見て 2 択:

- (a) `str` は**妥当な UTF-8**。入口で検証する。
- (b) `str` はただのバイト列。tree-walker の lossy 変換をやめる。

**(b) は取れない** — tree-walker は `str` を Rust の `String` で保持して
いて、Rust の `String` は UTF-8 でないバイト列を持てない。(b) を通すには
tree-walker の文字列表現を `Vec<u8>` にする必要があり、それは
「文字列を持つほぼ全ての場所」に触る変更で、得るものは「バイナリを
`str` に入れられる」だけ。**バイナリは `Vec<u8>` / `Span<u8>` で運ぶ**
という道が既にあり (EXTERN-BUF の `read_file_into` がまさにそれ)、
`str` にその役目は要らない。

→ **(a) を採る。** 具体的には:

- **入口は 3 つしかない**: 文字列リテラル (字句解析が保証済み)、
  `__builtin_str_from_bytes`、extern の返り値。
- `__builtin_str_from_bytes` は**検証する**。妥当でなければ
  **panic** (RUNTIME-TRAP と同じ「壊れた値を黙って通さない」)。
  tree-walker の lossy 変換はここで消える。
- `String::to_str()` (= `Display` の impl) はこの builtin の上にあるので、
  **非 UTF-8 の `String` を `to_str()` すると panic する**。先に訊きたい
  側のために `String::is_utf8(&self) -> bool` を置く
  (ERROR_MODEL の `try_reserve` と同じ「**要素ごとに `Result` を返さず、
  先に訊く**」規律)。
- extern の返り値: `read_file` は tree-walker が既に非 UTF-8 を read
  error にしている (`core/std/io.t`)。**compiled lane も揃える**
  — 揃えないと実測 4 が `read_file` 経由で復活する。

検証のコストは新規ではない。`str_from_bytes` は既にバイトをコピーして
いる (`toylang_rt::toy_str_from_bytes` / tree-walker とも) ので、O(n) の
走査はもう払っている。検証はその走査に乗る。

## 3. `str` の method 集合を絞る

**採用: 確保する 5 つを `str` から外し、`String` に集約する。読むだけの
ものを `str` に残し、足りない分は extern で足す。**

| method | 行き先 | 理由 |
|---|---|---|
| `len` / `concat` | `str` に残す | IR にあり 3 レーンで動く。`concat` は文字列補間の desugar が使う |
| `hash` / `as_ptr` | `str` に残す | 既に extern / builtin |
| `contains` | **`str` で extern 化** | 確保しない述語。`toy_str_find` の上に載る |
| `starts_with` / `ends_with` / `find` | **`str` に新設 (extern)** | 同上。`find` があれば他は 2 行 |
| `lt` (`Ord`) | **`str` に新設 (extern `toy_str_cmp`)** | §5 |
| `substring` / `trim` / `to_upper` / `to_lower` / `split` | **`String` へ** | 新しいバッファが要る = 所有する型の仕事。実測 2 で既に 3 レーン動いている |

外した 5 つは**型検査器の表から消して、診断で誘導する**:

```
[E0025] `str` has no method `to_upper`: it does not own a buffer to write into
  help: `String::from_str(s).to_ascii_upper()` produces an owned String
```

黙って動かなくなるのではなく、**受理していたものを名指しで断る**
(`--explain` に直し方を書く形は `E0024` / `E0022` で先例がある)。
利用者が自分だけの今のうちにやる。

**名前も直す**: `to_upper` / `to_lower` は ASCII しか畳まない
(`fold_ascii_case` がそう書いてある) ので **`to_ascii_upper` /
`to_ascii_lower`** に改名する。§4 の線引きを名前に出すのがいちばん安い
ドキュメントになる。

## 4. Unicode をどこまでやるか

**採用: Unicode scalar value (codepoint) 止まり。**

| やる | やらない |
|---|---|
| UTF-8 の encode / decode (RFC 3629、surrogate 拒否) | grapheme cluster (`"e\u{301}"` を 1 文字と数える) |
| codepoint 単位の反復 (`chars()`) | 正規化 (NFC / NFD / NFKC) |
| ASCII の分類・大小変換 | Unicode の大小変換 (`ß` → `SS`、トルコ語の `i`) |
| バイト単位の比較・順序 | 照合順序 (collation、ロケール) |
| — | UTF-16 / その他エンコーディングとの変換 |

理由は 2 つ。**テーブルの重さ** — 正規化も Unicode 大小変換も数十 KB の
表を持つので、`core/std` を純 toylang で書くという既定 (COLLECTIONS の
制約 1) と正面から衝突する。**決定性の規約** — 照合順序はロケールを
参照した時点で `strftime` を UTC に固定した理由に反する。

grapheme / 正規化が要るプログラムはこの言語で書かない、と**書いておく**。
書いておかないと「無い」と「まだ無い」の区別がつかない。

## 5. `Ord for str` — extern の 3 値比較

実測 6 の通り toylang でも書けるが、interpreter の `as_ptr()` が呼ぶ
たびに確保するので、`Vec<str>::sort()` が O(n log n) 回確保する。
`Hash for str` を extern にしたのと**同じ理由・同じ形**にする:

```
extern fn __extern_str_cmp(a: str, b: str) -> i64 from "toylang_rt" as "toy_str_cmp"

impl Ord for str {
    fn lt(self: Self, other: Self) -> bool { __extern_str_cmp(self, other) < 0i64 }
}
```

- **バイト辞書順** (`memcmp` + 短い方が先)。UTF-8 のバイト順は codepoint
  順と一致するので、これは「codepoint 順」でもある。照合順序ではない
  ことを doc comment に書く。
- 3 値を返すのは、将来 `cmp` が要るときに extern を増やさないため。
  `Ord` 自体は `lt` だけの trait のまま (STDLIB-ORD の決定を動かさない)。
- 受け入れは `Vec<str>::sort()` の 4 レーン一致。実測 6 の
  bound violation がそのまま消える。

extern を足す箇所は 4 つ: `.t` の宣言 / `toylang_rt` / `compiler/src/jit.rs`
のシンボル登録 / interpreter の `extern_io.rs` レジストリ。`toy_str_hash`
が同じ 4 箇所にある。

## 6. `char` の分類 — `u8` と `char` の両方に

`core/std/char.t` は今 `type char = u32` の 1 行しか無く、`parse.t` は
`c < '0' || c > '9'` を手で書いている。ASCII 分類を trait で置く:

```
pub trait AsciiClass {
    fn is_ascii_digit(self: Self) -> bool
    fn is_ascii_alpha(self: Self) -> bool
    fn is_ascii_alnum(self: Self) -> bool
    fn is_ascii_space(self: Self) -> bool
    fn is_ascii_upper(self: Self) -> bool
    fn is_ascii_lower(self: Self) -> bool
    fn to_ascii_upper(self: Self) -> Self
    fn to_ascii_lower(self: Self) -> Self
}
```

**`u8` と `u32` の両方に impl する** (`Hash` / `Ord` が全幅に impl して
いるのと同じ流儀)。`String::get(i)` は `u8` を返し `push_char` は `u32` を
取る、という既存の非対称をキャストで埋めさせないため。
`digit_value(self, radix: u32) -> Option<u32>` も同じ trait に置く
(`parse.t` と将来の SERIALIZE / TIME のパーサが 3 者とも要る)。

**ASCII 以外は `false` を返す。** 名前に `ascii` が入っているので嘘は
言っていない。

## 7. バイト → codepoint (`chars()`)

無いのはこれだけが理由で: encode (`push_char`) は「書く側」に要るので
先に書かれ、decode は「読む側」に要るのに読む側の API がまだ無かった。

```
struct CharsIter { data: ptr, len: u64, index: u64 }
impl String { fn chars(&self) -> CharsIter }
impl CharsIter { fn next(&mut self) -> Option<char> }
```

`StringIter` (バイト) の隣に置く。フィールドは同じ 3 つなので 8 レジスタ
予算 (COLLECTIONS の制約 3) にも収まる。

**不正なバイト列に出会ったら U+FFFD を 1 個返して 1 バイト進む**
(`from_utf8_lossy` と同じ規約で、決定的)。`Option` の `None` は終端の
意味に取ってあるので、失敗をそこに乗せられない。厳密に扱いたい側は
`is_utf8()` で**先に訊く** (§2 と同じ規律)。

`str` は §2 で妥当な UTF-8 が不変なので、`str` 側の `chars()` は
U+FFFD を返しえない — 同じコードで両方に impl できる。

## 8. 足りない API

上の決定で置き場所が決まるので、あとは並べるだけ。すべて純 toylang
(`String` 側) か extern 1 本 (`str` 側)。

- `String`: `find` / `rfind` / `starts_with` / `ends_with` / `replace` /
  `repeat` / `lines` / `split_whitespace` / `is_utf8` / `chars` /
  `eq_str(s: str)` (確保せずに `str` と比べる)
- `String::join(parts: Vec<String>, sep: &String) -> String`
  — `Vec<T>` に文字列専用 method を生やさない
- `push_str(s: str)` に**改名**し、`String` を取る今の `push_str` は
  `push_string(&String)` にする (名前が引数の型を言う)
- `str`: `contains` / `starts_with` / `ends_with` / `find` / `lt`

## Phase 分割

| Phase | 内容 | 受け入れ |
|---|---|---|
| **T0** | 今日壊れている 4 件 (実測 1・3・4・5) | 4 レーン一致 + 診断の文言 pin。`str_from_bytes` の検証で実測 4 が消える |
| **T1** | §1 の役割表と §2 の不変を `docs/language.md` に書く | 文書のみ。T2 以降が参照する正本 |
| **T2** | `str` の method 集合を絞る (§3) + `Ord for str` (§5) | 外した 5 つが `[E0025]` になる pin、`Vec<str>::sort()` の 4 レーン一致 |
| **T3** | `char.t` の `AsciiClass` (§6) | 4 レーン一致。`parse.t` を書き換えて利用者にする |
| **T4** | `chars()` / `is_utf8` (§7) | 4 レーン一致 + 不正バイトの U+FFFD 規約を pin |
| **T5** | 足りない API (§8) | method ごとに 4 レーン一致 |

**T0 が先頭なのは ERROR_MODEL と同じ理由** — 規約より先に、今日
壊れているものを直す。T1 は文書だけなので T0 と同時に landing できる。

T3 は T4 と T5 の両方が使うので先。T2 は breaking change を含むので、
T0 で「compiled lane で動かない」が判明した直後 = 誰も compiled lane で
使えていないことが確定している今がいちばん安い。

## 非目標

- **grapheme cluster / 正規化 / Unicode 大小変換 / 照合順序** — §4。
- **正規表現** — テキストの分野ではあるが、エンジンは別の設計文書に
  値する規模で、`find` / `split` / `starts_with` が入れば実プログラムの
  大半は書ける。実需要が出てから。
- **エンコーディング変換** (UTF-16 / Shift_JIS 等) — §4。
- **`String` の SSO (short string optimization)** — layout を変えると
  `Vec<u8>` と同 layout という現在の性質 (と `String` を `Vec<u8>` として
  扱う既存コード) が壊れる。速度が問題になったと**測って**からやる。
- **`str` の interning** — `Dict<str, V>` が hash を毎回計算する件は
  COLLECTIONS の領分。

## 関連

- [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) — 分野の俯瞰と優先順位
- [`COLLECTIONS.md`](COLLECTIONS.md) — `Hash for str` を extern にした
  判断 (§5 はその先例をそのまま引いている)
- [`ERROR_MODEL.md`](ERROR_MODEL.md) — 「要素ごとに `Result` を返さず
  先に訊く」規律 (§2 の `is_utf8`、§7 の U+FFFD)
- [`POINTER.md`](POINTER.md) — `Span<u8>` (バイナリはこちらで運ぶ)
- [`RUNTIME_PORT.md`](RUNTIME_PORT.md) — str 系ヘルパの toylang 化を
  却下した実測 (R3/R4)
- [`docs/language.md`](../docs/language.md) — T1 で役割表が入る正本
