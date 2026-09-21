# STDLIB CRYPTO — 暗号ハッシュ関数

> 対象: 新設する `core/std/crypto/` (`digest.t` / `sha256.t`、以降 `sha512.t` /
> `hmac.t` / `sha1.t` / `md5.t`)
> 状態の正本: [`todo.md`](todo.md) の **STDLIB-CRYPTO**
> 俯瞰と優先順位: [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md)
> 契約の書き方: [`../docs/design_by_contract.md`](../docs/design_by_contract.md)
> 実測: 2026-09-04 (本文の「実測」はすべてこの日に手元で走らせた結果)

> **状態: C0 / C1 landing 済み (2026-09-04)。`digest.t` と `sha256.t`
> (SHA-256 / SHA-224) が 3 レーンで動く。C2 以降は未着手。**

## 0. 先に結論

- **入れる**: SHA-256 / SHA-224 (C1) → SHA-512 族 (C2) → HMAC (C3) →
  相互運用専用の SHA-1 / MD5 (C4)。`trait Digest` (C0) を先に置いて
  C2 以降が同じ形に乗るようにする
- **入れない**: SHA-3 / Keccak・BLAKE3・パスワード KDF (Argon2 等)・
  共通鍵暗号。理由は §5
- **非暗号ハッシュ (CRC32 / xxHash) を `crypto/` に置かない。**
  `crypto::` に非暗号ハッシュがあること自体が誤用を誘う。要るなら
  `core/std/checksum.t` として別に置く
- **経路は純 toylang。** OS 境界を持たない純計算なので
  [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) の規則 1 は extern を
  要求しない。代償は interpreter の速度で、**64 KB のハッシュが
  interpreter 6.2s / AOT 7ms (~900x)** (実測 3)。この数字を承知の上で
  純 toylang を採る — 理由は §4
- **契約は inherent method と自由関数に置く。** `trait` の method に
  書けなかったため (実測 2、2026-09-21 に解消)。trait 側の契約は
  `output_size` / `block_size` の出力長から入れ始めた

## 1. なぜ crypto が要るか

`STDLIB_SERIALIZE` で hex / base64 / JSON が入り、`STDLIB_FS_PATH` で
ファイルが読み書きできるようになった。その次に来るのが
**「このバイト列は前に見たものと同じか」を言う手段**で、これが無いと:

- 内容アドレス (キャッシュのキー、ビルド成果物の同一性) が書けない。
  `INCREMENTAL_COMPILATION` の `.toycache` がソースのハッシュを
  キーにしているのと同じことを、この言語で書くプログラムができない
- ダウンロードしたファイルの検証ができない
- 鍵付きの完全性 (HMAC) が無いので、`net.t` の上に何かを載せる時に
  「送られてきたものが途中で変わっていないか」を言えない

いずれも「暗号」というより**完全性**の道具で、そこが優先度の理由になる。

## 2. 何を入れるか

| 段階 | モジュール | 中身 | 理由 |
|---|---|---|---|
| **C0** | `crypto/digest.t` | `trait Digest` + `struct Sum` + `ct_eq` | 先に形を決めないと C1 の API が C2 以降を縛る |
| **C1** | `crypto/sha256.t` | SHA-256 / SHA-224 | 一番使う。test vector が豊富で 3 レーン pin が安い |
| **C2** | `crypto/sha512.t` | SHA-512 / SHA-384 / SHA-512-256 | SHA-256 と同型で lane が u64 になるだけ。C1 の骨格の後ならほぼタダ |
| **C3** | `crypto/hmac.t` | HMAC (RFC 2104) | `trait Digest` が元を取るのはここ。鍵付きが無いと「ハッシュがある」で終わる |
| **C4** | `crypto/sha1.t` / `crypto/md5.t` | 相互運用専用 | git の object id、古い形式。**壊れていることを名前とコメントで言う** |

### 配置

`crypto/` は [`MODULE_SYSTEM.md`](MODULE_SYSTEM.md) の D1 (カテゴリ層を
入れない) に反しない。D1 が許すネストは「1 つの概念に複数のメンバが
ある」ときで、digest の実装群は `collections/` と同じ形をしている。
qualifier はリーフ名なので `sha256::sum(&bytes)` になる (Go の
`sha256.Sum256` と同じ読み方)。

## 3. API の形

### 3.1 streaming が原始、one-shot はその上

```rust
var h = Sha256::new()
h.update(&chunk1)
h.update(&chunk2)
val d = h.finalize()          # Sum

val d2 = sha256::sum(&bytes)  # 一発版
```

streaming を原始に採る理由は 2 つ。**ファイルを丸ごとメモリに載せずに
ハッシュできる**こと (`io::read_file` は全部読むので、大きな入力は
`Span<u8>` の窓を回すことになる) と、**HMAC が内側のハッシュを 2 回
別々に回す**こと。one-shot だけ提供すると C3 で作り直しになる。

### 3.2 出力は nominal な `Sum`、裸の `Vec<u8>` ではない

```rust
pub struct Sum { bytes: Vec<u8> }
```

`Vec<u8>` を直接返さない理由は**取り違えを型で止める**ため。
ハッシュを取る対象も `Vec<u8>`、結果も `Vec<u8>` だと、
`sha256::sum(&digest)` (二重ハッシュ) と `sha256::sum(&message)` が
同じ型で見分けが付かない。

associated type が無い (`STDLIB_TRAIT_BASE` A4) ので `trait Digest` の
`finalize` は**全アルゴリズムで同じ型**を返すしかなく、`Sum` は長さを
実行時に持つ。つまり `Sum` が区別するのは「ダイジェストかそうでないか」
までで、「SHA-256 の 32 バイトか SHA-512 の 64 バイトか」ではない。
そこは承知の上で、前者だけでも取り違えの大半は止まる。

`Sum` に乗せるもの: `size` / `get` / `to_hex` / `ct_eq` / `eq` / `to_str`
(Display)。**`to_hex` は `hex::encode` に委譲する** — 16 進の綴りは
`STDLIB_SERIALIZE` が既に決めていて、crypto 側に複製すると 2 つの
綴り規則ができる。

### 3.3 定数時間比較は `ct_eq`、ただし best-effort

`==` (`eq`) は**最初に違うバイトで抜ける**。MAC の検証にそれを使うと
比較にかかった時間が「何バイト目まで合っていたか」を漏らす。なので
`ct_eq` を別に置き、全バイトを OR で畳んで長さも分岐なしで混ぜる。

**保証はできない、と書く。** この言語には最適化バリアが無く、
cranelift が `ct_eq` の畳み込みを分岐に戻すことを止める手段が無い。
「定数時間**を意図した**比較」であって「定数時間である」ではない。
保証できないものを保証しているように見せるほうが、無いことより悪い。

## 4. 純 toylang か extern か

[`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) の規則 1 は「ロジックは純
toylang、perf・OS 境界は extern」。crypto は**OS 境界を持たない純計算
だが perf に効く**ので、規則がそのままでは判定しない。

**純 toylang を採る。**

| | 純 toylang | extern |
|---|---|---|
| interpreter の速度 | **6.2s / 64 KB** (実測 3) | Rust 速度 |
| AOT の速度 | 7ms / 64 KB (実測 3) | ほぼ同じ |
| `--effects` / DbC / `--profile=mem` | **効く** | 効かない (境界の向こう) |
| 変更コスト | 1 ファイル | 4 箇所セット (規則 2) |
| 正しさの固定 | test vector で 3 レーン一致 | 同じ |

決め手は **AOT の速度が変わらないこと**と、**契約・エフェクト検査が
効くこと**。crypto は「入力を全部読んで固定長を返す純関数」で、
`never_allocates` や `ensures` の good citizen になる分野そのもの。
interpreter で MB 単位を回すユースケースは現状無い (あるなら
`--all-backends` か AOT を使う場面)。

**後から extern の fast path を同じ API の裏に足すことはできる。**
そのときは `Sha256::update` の中だけを差し替える形になり、
`trait Digest` と `Sum` は動かない。§3 の形を先に決めておく価値は
そこにもある。

## 5. 入れないもの

- **SHA-3 / Keccak** — Keccak-f[1600] は SHA-2 と全く別の置換で、
  25×u64 の状態 + 24 ラウンド + θχπρι の 5 ステップ。SHA-256 の
  骨格を一切共有しないので C2 のような「ほぼタダ」にならない。
  使う場面に対してコード量が見合わない
- **BLAKE3** — SIMD と木構造が前提の設計で、それを実装しないなら
  SHA-256 より選ぶ理由が無い。SIMD 版は `SIMD.md` の 128bit 制限と
  噛み合わせる必要があり、別の設計になる
- **Argon2 / bcrypt / scrypt** — パスワード用 KDF。定数時間も
  メモリ hardening も**この処理系では保証できない** (§3.3 と同じ理由)。
  保証できない土台の上にパスワード保管を書かせるべきではない
- **AES / ChaCha20 / Poly1305** — ハッシュではない。共通鍵暗号は
  鍵の扱い (ゼロ化・スワップアウト) という別の問題を連れてくるので、
  必要になったら別の設計文書で
- **非暗号ハッシュ (CRC32 / xxHash)** — 要るが `crypto/` ではない。
  `crypto::` の中に非暗号ハッシュがあると、名前空間そのものが
  誤用を誘う。`core/std/checksum.t` に分ける。なお `hash.t` の
  `trait Hash` (ハッシュ表用) とも別物で、こちらは既にある

## 6. 実装で踏んだ言語側の穴

SHA-256 を書き切るまでに 5 つ踏んだ。**どれも回避できたが、回避の跡が
コードに残っている**ので、直せば crypto のコードは短くなる。

### 実測 1 — narrow int が shift できない

```
val x: u32 = 0x12345678u32
x >> 3u32    # [E0002] incompatible types u64 and u32
x >> 3u64    # [E0002] incompatible types u32 and u64
```

`docs/language.md` は「rhs must be `u64`」としか書いていないが、実際は
**lhs も u64 限定**。SHA-256 の `ssig0` / `ssig1` (`x >> 3` / `x >> 10`)
と byte → word の組み立て (`b << 24`) で必ず踏む。

回避: `shr32` / `shl32` ヘルパで `((x as u64) >> n) as u32` に往復させた。
`Bits` trait が `rotate_left` / `rotate_right` を全 8 幅で提供している
のと非対称で、**shift だけが narrow で使えない**。todo に
NUM-W-SHIFT として既載 (2026-09-01 の NET N3 で `u8` について踏んだ
同じ穴) — 幅が `u32` でも同じことと、回避が持ち込む意味論のずれを
追記した。

### 実測 2 — body 無しの trait method に `requires` を書くと壊れた (**2026-09-21 に解消**)

```rust
trait T {
    fn a(self: Self, n: u64) -> u64
        requires n < 100u64
    fn b(self: Self) -> u64
}
```

→ `[E0010] requires clause must be of type bool, got Unknown` が
**impl 側の別の method を指して**出る。clause の ExprRef が
inheritance の先で別の節点に結び付いている。`requires true` でも同じ。

当時これは 2 つの記述と食い違っていた: CLAUDE.md の「trait 本体には
... `requires` / `ensures` 節も書ける」と、DBC-LISKOV の `[E0023]` が
出す**「move the clause to `trait Foo`」という指示** — 従えない指示に
なっていた。

**原因** (2026-09-21): 署名の節は**モジュールのプール**を指す
`ExprRef` なのに、統合がそれを写さずそのまま運んでいた。各節は本体側
プールの同じ添字に在る別の節点を指すことになり、型が合わないと
言われる。同じファイルに書いた trait では起きない (プールが 1 つ
なので添字が正しい) ため、最小再現が module をまたがないと出なかった。
関数の節と同じ `map_expr` を通して解決。

**設計への影響**: 解消したので `trait Digest` は契約を持てる。
`output_size` / `block_size` に `ensures result > 0u64` を入れた。
E0023 が impl 側での追加を禁じる規則は変わらないので、**強める向きの
契約は trait 側に書く**。

### 実測 3 — 性能

64 KB のメッセージ 1 本:

| レーン | 時間 |
|---|---|
| interpreter (IR VM, release build) | 6.2s |
| AOT | 7ms |

~900x。[`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) が RUNTIME-PORT R3/R4
で str ヘルパについて出したのと同じ結論で、§4 の判断の前提になっている。

### 実測 4 — 固定長の作業領域が持てなかった (**2026-09-21 に解消**)

当時の状況:

- `[0u8; 64]` (repeat array literal) が**無い** — 64 要素を並べる以外に
  書けない
- `const K: [u32; 64] = [...]` は**コンパイル系が拒否する**
  (`only literal values and references to earlier consts are supported`)
- モジュールの `const` はそのモジュール自身の関数からも見えない

結果、**ブロックバッファも `w[]` も K 表も `Vec`** = ヒープだった。

3 つとも埋まった (ARRAY-REPEAT-LITERAL / CONST-ARRAY / MODULE-CONST)。
**K 表は `const SHA256_K: [u32; 64]` になり** — `.rodata` の 256 バイトを
全ハッシャが読む、誰も組み立てない — **`compress` は
`never_allocates` を名乗る**。ハッシャごとの 64 回の `push` と 256
バイトが消えた。

残っているのは `buf` と `w` で、どちらも `&mut self` から使う可変の
作業領域である。固定長配列を struct のフィールドに置けるようになれば
こちらも落とせる。

### 実測 5 — compound の `result` に触る `ensures` が書けない

```rust
pub fn sum(data: &Vec<u8>) -> Sum
    ensures result.size() == 32u64      # compiler MVP requires the method
{ ... }                                 # receiver to be a struct or enum binding
```

field 形 (`ensures result.bytes.size() == 32u64`) は
`field access on a non-struct value`。**interpreter では両方通る**ので、
インタプリタで書いた契約が AOT で落ちる形になっている。既載の
DBC-RESULT-FIELD (field 形) と同じ根で、method 形もそこに追記した。

`Sum` が長さを実行時に持つ設計 (§3.2) と噛み合って、**出力長を契約で
言う手段が無い**。§7 は `Sha256::fresh` の入口の scalar に倒して回避
している。

## 7. 契約をどこに置いたか (C0 / C1)

`Vec` と同じ方針 ([`VEC_CONTRACTS.md`](VEC_CONTRACTS.md) の A2):
**`requires` を足しても `panic` は消さない**。契約は `--release` で
消えるので、消すと release で無検査になる。

| 場所 | 契約 | 何を守るか |
|---|---|---|
| `Sum::get(i)` | `requires i < self.size()` | 添字。違反時に実値が出る |
| `Sum::from_bytes(b)` | `requires b.size() > 0u64` | 空のダイジェストを作らせない |
| `shr32` / `shl32` | `requires n < 32u64` | shift 量。32 以上は実測 1 の往復で**静かに 0 を返す**ので、契約が無いと気付けない |
| `Sha256::compress(off)` | `requires off + 64u64 <= self.buf.size()` | ブロックの読み出しが確保の内側に居ること |
| `Sha256::fresh(..., out)` | `requires out == 32u64 \|\| out == 28u64` | SHA-256 / SHA-224 以外の切り詰めを作らせない |
| `Sha256::pad_byte(b)` | `requires self.nbuf < 64u64` | バッファ不変条件。書き込み位置が枠の内側に居ること |

**出力長は契約にできなかった。** `sum` に
`ensures result.size() == 32u64` と書くのが素直だが、compound の
`result` に触る契約は compiled lane が拒否する (実測 5)。同じ事実を
`Sha256::fresh` の入口で `out` について言う形に倒してある — そちらは
まだ scalar なので通る。

`shr32` の `requires n < 32u64` が一番効く。u64 に広げてから shift する
回避策は、**32 以上の shift でも trap せず 0 を返す** — 元の u32 の
shift なら未定義側に倒れるところが、静かに正しくない値になる。契約が
その差を埋めている。

## 8. テスト

- **NIST / RFC の test vector** で pin する。SHA-256 は FIPS 180-4 の
  3 本 (`"abc"` / 空 / 56 バイト) + 複数ブロックにまたがる 1 本。
  SHA-224 は同じメッセージの別 IV
- **streaming と one-shot が一致すること** — 1 バイトずつ `update` した
  結果が `sum` と同じであること。ブロック境界をまたぐ入力で
  バッファリングのバグが出る
- `compiler/tests/consistency/crypto.rs` で **3 レーン一致**
- `interpreter/example/crypto_sha256.t` は `example_consistency` が
  自動で 3 レーンに掛ける

## 9. 関連

- [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) — 分野の俯瞰と経路の規則
- [`STDLIB_SERIALIZE.md`](STDLIB_SERIALIZE.md) — `hex::encode` (`to_hex`
  の委譲先) と `CodecError`
- [`MODULE_SYSTEM.md`](MODULE_SYSTEM.md) — D1 (ネストの条件) と
  qualifier の解決
- [`VEC_CONTRACTS.md`](VEC_CONTRACTS.md) — 「契約を足しても panic は
  消さない」の先例
- [`../docs/design_by_contract.md`](../docs/design_by_contract.md) — 契約の書き方
