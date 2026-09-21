# STDLIB SERIALIZE — JSON・hex・base64

> 対象: 新設する `core/std/json.t` / `core/std/hex.t` /
> `core/std/base64.t`
> 状態の正本: [`todo.md`](todo.md) の **STDLIB-SERIALIZE**
> 俯瞰と優先順位: [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) の P4
> 実測: 2026-09-03

## Status snapshot

| 項目 | 状態 |
|---|---|
| JSON | 無い |
| hex / base64 | 無い |
| 土台の値の木 | **書ける** — `Vec` / `Dict` payload の再帰 enum が 3 レーンで動く (実測 1) |
| 数値 → 文字列 | `__builtin_to_string` が f64 を**往復する綴り**で出す (実測 2)。**指数形は出ない** |
| 文字列 → 数値 | `parse::to_u64` / `to_i64` / `to_f64` (`1e10` を受理する) |
| バイト列 | `Vec<u8>` / `Span<u8>` / `String` (EXTERN-BUF 経由でバイナリ安全) |

## なぜ今これを設計するか

**外に出す形式が 1 つも無い。** ファイルは読み書きでき、path も
(FS-PATH で) 扱えるようになるが、**構造を持ったデータを保存して読み
戻す方法が無い**。設定ファイルも、キャッシュも、`--format=json`
のようなツール間の受け渡しも、全部この分野の上に載る。

そして **土台はもう揃っている** — 実測 1 の再帰 enum、挿入順を保つ
`Dict` (COLLECTIONS C1)、UTF-8 encode (`push_char`)、
`parse::to_f64`。足りないのは**形式の規約**だけ。

## 測ったこと (2026-09-03)

1. **JSON の値の木は今日書ける。**
   ```
   enum Json { Null, Bool(bool), Num(f64), Text(String),
               Array(Vec<Json>), Object(Dict<String, Json>) }
   ```
   が型検査を通り、`Json::Array(kids)` の構築と `match` が
   **3 レーン一致**した。E0013 (間接化なしの再帰型) に当たらないのは、
   「型引数が containment になるのは渡し先がそのパラメータを by-value で
   持つときだけ」という規則のため (`Vec<Json>` / `Dict<String, Json>` は
   ヒープの向こう側)。**`Box` を挟む必要が無い。**

2. **f64 の綴りは往復するが、指数形が無い。**

   | 値 | `println` の出力 |
   |---|---|
   | `0.1f64` | `0.1` |
   | `0.1 + 0.2` | `0.30000000000000004` |
   | `1.0 / 3.0` | `0.3333333333333333` |
   | `-0.0f64` | `-0.0` |
   | `1.0f64` | `1.0` |
   | `1234567890123456789.0` | `1234567890123456768.0` |
   | `1e30` 相当 | `1000000000000000019884624838656.0` (**31 桁**) |

   最短往復の綴り (Rust の `Display`) なので**読み戻せば同じ値**に
   なるが、大きい値・小さい値は長大な文字列になる。

3. **ソースに指数リテラルが書けない。**
   `1e300f64` は `[E0012] invalid number ...: digits cannot be followed
   by letters` で lexer が拒否する。**読む側 (`parse::to_f64("1e10")`)
   は指数を受理する**ので、入力にはあるのに文法には無い、という
   非対称がある。

4. **module 修飾の型名が generic 型引数に書けない。**
   `val r: Result<f64, parse::ParseError> = ...` は
   `Expected ',' or '>' in generic type arguments` で parse error。
   裸の `ParseError` (auto-load 済み) なら通る。**JSON の API は
   `Result<Json, JsonError>` を返す**ので、この形は user のコードに
   毎回出てくる。

5. **バイトを 1 つずつ回すコストは interpreter で高い。**
   STDLIB_TIME の実測 3 より、IR VM のループは 1 反復 ≒ 6 µs。
   **base64 の 1 MB は約 6 秒**になる。純 toylang で書くこと自体は
   変えないが (下の制約 1)、それが分かった上で書く。

## 既存の決定から引く制約

1. **純 toylang で書く** (COLLECTIONS 制約 1)。そうすれば
   `--profile=mem` / `ensures allocates(N)` / REGION 検査が効く。
   実測 5 のコストは、必要になったら SIMD kernel か extern に
   昇格する余地として残す (`CaseConvert` が既に u8x16 で書かれている)。
2. **文法の判定は toylang 側** (`parse::to_f64` の切り分け)。
   JSON のパーサを extern に投げない。
3. **厳しい側に倒す** (`parse` の既定)。曖昧な入力を「親切に」
   受理しない。
4. **失敗は module ごとの enum、共通 trait は置かない** (ERROR_MODEL)。
5. **`Dict` は挿入順を保つ** (COLLECTIONS C1) — **JSON の出力が
   決定的になる**。これは仕様として書く価値がある性質で、
   `println` が struct のフィールドをソートするのと同じ
   「決定的な出力」の系譜。

## 1. JSON — 2 つの入口

**S1: 木を作らない writer (先に作る)**

```
struct JsonWriter { out: String, depth: u64, need_comma: bool }
impl JsonWriter {
    fn new() -> Self
    fn begin_object(&mut self) / fn end_object(&mut self)
    fn begin_array(&mut self)  / fn end_array(&mut self)
    fn key(&mut self, k: str)
    fn str_value(&mut self, v: str)
    fn u64_value(&mut self, v: u64) / fn i64_value / fn f64_value / fn bool_value / fn null_value
    fn finish(self: Self) -> String
}
```

構造体を 1 つ書き出すのに**値の木を組み立てる必要が無い** (確保が
`out` の伸長だけになる)。RUNTIME_LIBRARY が「writer 先行」と言った
のはこの形のこと。

**S2: 値の木 (reader と対)**

```
enum Json { Null, Bool(bool), Int(i64), Num(f64), Text(String),
            Array(Vec<Json>), Object(Dict<String, Json>) }
fn parse(s: str) -> Result<Json, JsonError>
impl Json { fn to_string(&self) -> String }
```

実測 1 で書けることを確認済み。

## 2. 数をどう持つか — `Int` と `Num` を分ける

JSON の数は 1 種類だが、**`u64` の id を f64 に通すと 2^53 で壊れる**
(実測 2 の `1234567890123456789.0` → `...768.0` がまさにそれ)。

- **reader**: トークンに `.` も `e` も無く `i64` に収まるなら
  **`Int(i64)`**、それ以外は `Num(f64)`。
- **writer**: `Int` は小数点なし、`Num` は実測 2 の綴りそのまま。
- **`u64` の 2^63 以上**は `Num` に落ちる (精度を失う)。
  doc comment に書く。`Int(u64)` を足すと 3 通りになるので足さない。

**指数形は出さない** (実測 2・3)。JSON としては合法だが、出す側の
綴りを 2 つ持つと「同じ値が 2 通りに書かれる」ので、**往復する
唯一の綴り**に寄せる。`1e30` が 31 桁になることを doc に書く。

**`NaN` / `inf` は JSON に無い** (STDLIB_NUMERIC 実測 3 で綴りを確認
済み)。writer は **panic する** — 書けない値を黙って `null` に
すり替えない (`str_from_bytes` が非 UTF-8 を黙って通さないのと
同じ規律)。`null` にしたい側は自分で分岐する。

## 3. 文字列のエスケープ

**writer**: `"` と `\` と 0x20 未満だけをエスケープする
(`\n` `\r` `\t` `\b` `\f` は短い形、その他の制御文字は `\u00XX`)。
**それ以外の UTF-8 はそのまま出す** — `\uXXXX` に展開すると
日本語のファイルが 6 倍になる。

**reader**: `\uXXXX` を受理し、**サロゲート対を組み立てる**
(`😀` → U+1F600)。組み立ての出口は
`String::push_char` (STDLIB_TEXT §7 の encode) で、対になっていない
サロゲートは `Invalid`。

**入力は妥当な UTF-8 であること**を要求する (STDLIB_TEXT §2 の
不変)。バイト列から読むときは `is_utf8()` で**先に訊く**。

## 4. reader の厳しさ

RFC 8259 の**部分集合**で、拡張は 1 つも受理しない:

| 入力 | 結果 |
|---|---|
| 末尾のカンマ `[1,]` | `Invalid` |
| コメント `// x` | `Invalid` |
| `NaN` / `Infinity` | `Invalid` |
| 先頭 0 `01` | `Invalid` |
| 単引用符 `'a'` | `Invalid` |
| 値の後ろの余分な文字 | `Trailing` |
| 同じキーが 2 回 | **後が勝つ** (`Dict::insert` の意味論。エラーにしない) |
| 深さ 128 超 | `TooDeep` |

**深さの上限を持つ理由**: 再帰下降のパーサは入力の深さだけ再帰する。
上限が無いと、深くネストした入力が
`panic: recursion limit exceeded (N frames deep)` になる — **user の
入力が panic になってはいけない** (`Result` を返す関数の中で)。

```
enum JsonError { Empty, Invalid(u64), Trailing(u64), TooDeep(u64) }
```

**payload は失敗した位置 (バイト offset)。** `ParseError` が位置を
持たないのは入力が数値 1 個だからで、JSON では位置が無いと直せない。
`Display` は `invalid JSON at byte 42` の形。

## 5. hex

```
pub fn encode(bytes: &Vec<u8>) -> String          # 小文字
pub fn decode(s: str) -> Result<Vec<u8>, CodecError>
enum CodecError { Invalid(u64), BadLength }
```

- **出力は小文字**、**入力は大小どちらも受理**する
  (Postel は採らない方針だが、hex の大小は曖昧さが無いので例外。
  doc にそう書く)。
- 奇数長は `BadLength`。空文字列は `Ok` で空の `Vec`。
- 空白も改行も受理しない (制約 3)。

## 6. base64

```
pub fn encode(bytes: &Vec<u8>) -> String          # 標準アルファベット + `=` パディング
pub fn decode(s: str) -> Result<Vec<u8>, CodecError>
```

- **RFC 4648 の標準アルファベットのみ。** URL-safe (`-_`) は
  置かない — 要る場面 (URL / JWT) が来てから足す。
- **パディングは必須**。`=` の無い入力は `BadLength`。
- 空白・改行 (MIME の 76 桁折り返し) は**受理しない**。
- decode は 4 文字ごとに詰め、末尾の `=` の数で 1〜2 バイト落とす。

## 7. どこまでを stdlib に置くか

**置く**: JSON (writer / reader)、hex、base64。
**置かない**: struct ↔ JSON の自動変換 (derive 相当)。

自動変換には「型の情報を実行時に持つ」機構が要り、この言語には無い
(`__builtin_sizeof` は幅を答えるが、フィールド名は答えない)。
**`JsonWriter` に手で書く**のが今の形で、それは 1 struct につき
5〜10 行。`Display` の `to_str` を手で書くのと同じ手間なので、
規約として通っている。

## Phase 分割

| Phase | 内容 | 受け入れ |
|---|---|---|
| **S0** | `hex` (§5) | 4 レーン一致。RFC の test vector + 端 (空 / 奇数長 / 不正文字) |
| **S1** | `JsonWriter` (§1 の S1・§2・§3 の writer 側) | 4 レーン一致。**出力が決定的**であること (制約 5) を pin。`NaN` の panic も |
| **S2** | `base64` (§6) | 4 レーン一致。RFC 4648 の test vector 全部 |
| **S3** | `Json` の木 + `to_string` (§1 の S2) | 4 レーン一致。S1 の出力と**同じ文字列**になること |
| **S4** | `parse` (§3 の reader 側・§4) | 4 レーン一致。往復 (`parse(x.to_string()) == x`) と、§4 の拒否表 |
| **S5** | 深さ上限 / 位置つき診断の詰め | 深い入力が `TooDeep` であって panic でないこと |

**S0 (hex) が先頭**なのは、いちばん小さくて分野の型 (`CodecError` /
`Vec<u8>` ↔ `String` の受け渡し / test vector の置き方) を全部
確定させられるから。S3 が S1 の後なのは、**木の出力を writer で
書く**ため (綴りの実装が 2 つにならない)。

## 非目標

- **derive / 自動変換** — §7。
- **YAML / TOML / XML / MessagePack / CBOR** — JSON が入ってから、
  実需要で決める。**設定ファイルには JSON を使う**と決めておけば
  当面足りる。
- **ストリーミングパーサ** (巨大ファイルを木にせず読む) — reader の
  形が変わるので、必要になったら別の入口として足す (writer 側は
  S1 が既にその形)。
- **JSON Pointer / JSON Patch / JSON Schema** — 別の仕様。
- **URL-safe base64 / MIME の折り返し** — §6。
- **正準形 (JCS) / ソートしたキー順** — `Dict` の挿入順で決定的
  (制約 5) なので、署名用の正準形が要るまでは十分。
- **指数形の f64 出力** — §2。要るなら STDLIB_NUMERIC の
  format spec (`{x:e}`) 側に足す方が筋がよい。

## 関連

- [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) — P4 (writer 先行の判断)
- [`STDLIB_TEXT.md`](STDLIB_TEXT.md) — UTF-8 の encode / decode、
  `is_utf8` で先に訊く規律 (§3)
- [`STDLIB_NUMERIC.md`](STDLIB_NUMERIC.md) — `NaN` / `inf` の綴り (§2)
- [`COLLECTIONS.md`](COLLECTIONS.md) — `Dict` の挿入順 (制約 5)
- [`ERROR_MODEL.md`](ERROR_MODEL.md) — `JsonError` / `CodecError` の
  置き方
- [`STDLIB_FS_PATH.md`](STDLIB_FS_PATH.md) — 設定ファイルを読む側
