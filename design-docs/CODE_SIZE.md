# CODE-SIZE — 生成バイナリが太る理由

> 実装: [`compiler/src/codegen/mod.rs`](../compiler/src/codegen/mod.rs)
> (`cranelift_signature_with_writeback`)、
> [`compiler/src/codegen/lower_inst.rs`](../compiler/src/codegen/lower_inst.rs)
> (`lower_self_writeback`)
> 計測対象: `poc/logsearch` (toylang 5,109 行) を AOT `--release` で
> aarch64 macOS に。2026-09-05。

## 要点

`&mut self` メソッドの ABI が **struct を leaf スカラーに展開して
引数と戻り値に並べる**。leaf 数が引数レジスタ本数 (aarch64 で 8) を
超えると、超えた分は呼び出しのたびにメモリを往復する。

`ArchiveWriter` (19 フィールド → **52 leaf**) の 16 メソッドだけで
**toylang 生成コードの 29%** を占めた。太い関数は全体の 10% しか
ないのに、コードの **49%** を持っていく。

コード品質そのものは良い — 8 leaf 以下なら 1 フィールドを +1 する
メソッドは **3 命令**に落ちる。問題は codegen ではなく署名の形。

**進捗**: 戻り側 (writeback) は 2026-09-05 に片付いた
(CODE-SIZE-WB-PRUNE、`__text` −10.3%)。**引数側は手つかず** —
`ts_min()` が 52 引数取るのは今も変わらない。

## 計測 — 331,832 B の内訳

| 部分 | bytes | 中身 |
|---|---:|---|
| `__text` | 191,080 | 機械語。toylang 生成 86% / Rust runtime 12% |
| `__const` | 37,776 | 診断文字列 (下記) |
| `__LINKEDIT` | 65,536 | シンボル表。`strip -x` で 291 KB (−40 KB) |
| その他 + ページ整列 | ~37,000 | macOS の 16 KB セグメント境界 |

**デッドコード削除は効いている。** auto-load は `core/` 下の `.t` を
全部読むが、`base64` / `hex` / `json` / `sha256` / `net` / `poll` の
シンボルは 1 つも入らない。下限は測れる:

| プログラム | ファイル | `__text` |
|---|---:|---:|
| `fn main() -> u64 { 0u64 }` | 16,792 | 8 |
| `println("hi")` を足す | 54,168 | 4,172 |
| `poc/logsearch` (5,109 行) | 331,832 | 191,080 |

空プログラムの 16.8 KB はページ整列で、コードではない。

## 主因 — `&mut self` writeback が O(struct) の呼出規約になる

`&mut self` メソッドは「self の全 leaf を引数で受け、全 leaf を
戻り値で返す」形に lower される
([`BACKEND.md`](BACKEND.md) の `CallWithSelfWriteback`)。CLIF を見ると:

```
toy_ArchiveWriter__write_seg:  58 params -> 53 returns
toy_ArchiveWriter__emit_terms: 63 params -> 52 returns
toy_ArchiveWriter__ts_min:     52 params ->  1 return   # 1 フィールド読むだけ
```

`ts_min()` が 52 引数取るのが症状を一番よく表している。**読むだけの
getter でも self 全体が呼出規約を通る**。

aarch64 の引数レジスタは 8 本。溢れた分は todo.md の
`enable_multi_ret_implicit_sret` (return-area ポインタ) を通る —
これは「署名が拒否される」のを直した変更で、**メモリ往復自体は
残っている**。

`write_seg` の命令内訳 (3,452 命令):

| | |
|---|---:|
| load / store | 2,413 (**70%**) |
| `bl` (呼び出し) | 249 |
| `add` (実際の計算) | 90 |
| スタックフレーム | 3,024 B |

toylang 生成コード全体では load/store が **50%** (Rust runtime 側は
24%)。

### 崖は leaf 8 個ちょうど

1 フィールドを `+1` するだけの `&mut self` メソッドの命令数:

| leaf 数 | 1 | 8 | 9 | 12 | 16 | 24 | 32 | 52 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 命令 | 3 | **3** | 8 | 14 | 24 | 48 | 80 | **162** |

8 以下は完璧。9 から線形に増え、1 leaf あたり ~3.2 命令。

**狭い関数の codegen は良い**ことを押さえておく (同じコンパイラ、
同じ `--release`):

```
fn sum3(a,b,c) -> u64 { a+b+c }     -> add, add, ret        (3)
fn addp(p: &P, q: &P) -> u64        -> add, add, add, ret   (4)
fn bump(p: &mut P)                  -> add, ret             (2)
```

つまり直すべきは署名であって、命令選択でも cranelift の設定でもない。

### 全体への寄与

`poc/logsearch` の 326 関数を params+returns の幅で分けると:

| | 関数数 | bytes | 割合 | 1 関数平均 |
|---|---:|---:|---:|---:|
| 幅 > 16 | 34 (10%) | 76,380 | **49%** | 2,246 B |
| 幅 ≤ 16 | 292 (90%) | 78,504 | 51% | 269 B |

`ArchiveWriter` の 16 メソッドだけで 44,880 B = toylang コードの 29%。

### cranelift の opt level は無関係

`TOYLANG_CRANELIFT_OPT_LEVEL` を振っても `write_seg` は変わらない:

| | `none` | `speed` |
|---|---:|---:|
| `ldr` | 1,507 | 1,500 |
| `str` | 578 | 527 |

cranelift は展開済みの署名を受け取っているだけなので、この段では
直せない。**`.cargo/config.toml` の `[env]` が `none` を輸出している**
ので `cargo run -p compiler` は最適化なしで走るが、サイズには効かない
(速度には効く)。

## 副因 — 事前レンダリングされた診断文字列 (~39 KB, 12%)

DEBUG-OBS D3 は panic サイトごとに文面を `.rodata` に置く
(`declare_frame_strings`):

| シンボル | 個数 | bytes | 1 個あたり |
|---|---:|---:|---:|
| `toy_panic_msg_*` | 102 | 16,952 | 166 |
| `toy_frame_pre_*` | 89 | 14,817 | 166 |
| `toy_print_str_*` | 180 | 7,260 | 40 |
| `toy_frame_suf_*` | 89 | 534 | 6 |

機能としては正しい (行番号とソース断片が出るのはこの設計の眼目)。
詰めるなら共通接頭辞の共有か、サイトを ID にして表を 1 つにする。
主因を直すまでは優先度は低い。

## 直し方

### 済: 書かない leaf は返さない (CODE-SIZE-WB-PRUNE, 2026-09-05)

`&mut self` メソッドが返す leaf のうち、**body が一度も書かないもの**は
呼び出し側が既に持っている値なので、返しても情報が増えない。
[`compiler_lower/src/writeback_prune.rs`](../compiler_lower/src/writeback_prune.rs)
が lowering 後に落とす。

**不動点まで回す**のが要点。`&mut self` メソッドが別の `&mut self`
メソッドを呼ぶと、その call の `self_dests` が全 leaf を覆うので
「全部書く」ように見える。callee を細くすると caller の dests も
細くなり、caller が今度は細くなる — この連鎖が効く:

| | 戻り値 (before → after) |
|---|---|
| `ArchiveWriter::write_seg` | 53 → **1** |
| `flush_segment` | 53 → 1 |
| `ArchiveWriter::write_frames` | 57 → 5 |
| `ArchiveWriter::finish` | 55 → 3 |
| `ArchiveWriter::rehash` / `rehash_links` | 52 → 4 |
| `ArchiveWriter::link` | 52 → 12 |
| `ArchiveWriter::emit` | 53 → 25 |
| `Vec::clear` / `Vec::set_size` | 4 → 1 |

`poc/logsearch` で **`__text` 191,080 → 171,424 B (−10.3%)**、
ファイル 331,832 → 315,336 B (−5.0%、差は段の整列と symbol 表)。
`write_seg` は 3,453 → 2,369 命令 (−31%)。archive の出力は
**変更前のコンパイラと byte 単位で一致**、`verify` は 12 segments ok。

**安全側の作り**。落とすと lost update になるので、判定は
`InstKind::writes_locals` の**網羅 match** (新しい命令が local を書くなら
分類しないとビルドが通らない)。加えて次は全 slot を残す:

- `address_taken_locals` にある leaf (追えない store が来うる)
- 関数の番地が漏れている場合 (`FuncAddr` / `MakeClosure` / vtable) —
  直接呼び出し以外は署名を書き換えられない
- **`dyn` thunk が呼ぶ callee** — thunk は writeback を受けるためだけに
  新しい local を確保して `data_ptr` に書き戻すので、slot を落とすと
  その local が未定義のまま残る。「落とす dest が呼び出し側の他の場所で
  定義されているか」を確かめ、駄目なら callee ごと諦める

`CallStruct` / `CallTuple` / `CallEnum` は**戻り leaf と writeback leaf を
1 本の `dests` に連結する**ので、writeback 部分は位置ではなく長さで
切り出している (最初これを見落として arity 不一致で落ちた)。

テストは
[`compiler/tests/consistency/writeback_prune.rs`](../compiler/tests/consistency/writeback_prune.rs)
の 6 本。**書き換えた field 以外も全部読み戻す**形なので、落としすぎは
クラッシュではなく古い値として出る。pass をわざと壊す (全 slot を落とす)
と 6 本中 5 本が落ち、`dyn` の 1 本だけは veto が効いて通る。

### 残り: compound self をポインタで渡す (本命)

**leaf 数が閾値を超える compound `self` はポインタで渡す。**
展開はレジスタに乗る間だけの最適化で、既定にすべきものではない
(C ABI が構造体に対してやっているのと同じ判断)。閾値は
「引数レジスタ本数」= aarch64 / x86-64 とも 8 前後。

材料は既にある — `address_taken_locals` が explicit stack slot を
作る経路 (REF-Stage-2) を持っているので、`&mut self` をそこへ
接続する形になる。`&self` (writeback 無し) の方が先に効く上に
安全なので、そちらから分けて入れられる。

3 バックエンドに跨る変更なので、
[`CLAUDE.md`](../CLAUDE.md) の「横断的な変更をするとき」に従い
`compiler/tests/consistency/` にテストを足すこと。

### 書く側 (今できる回避)

**触るフィールドだけを渡す。** 実測:

```rust
struct Buf { a: u64, b: u64, c: u64 }
struct Big { buf: Buf, p0: u64, ... pb: u64 }   # 15 leaf

g.bump_self()   # &mut self  (15 leaf) -> 20 命令
g.buf.bump()    # &mut Buf   ( 3 leaf) ->  3 命令
```

大きい struct のメソッドが実際に触るのが 1〜2 フィールドなら、
その部分 struct の method にすると細い関数側へ移る。複数の
フィールドを同時に触るもの (`ArchiveWriter::write_seg` /
`emit_terms`) は素直には割れないので、回避策であって解ではない。

## 再現手順

```bash
# 1. サイズと内訳
size -m poc/logsearch/build/release/logsearch

# 2. 署名の幅 (CLIF を出して params/returns を数える)
./target/release/compiler --core-modules core \
    --core-modules poc/logsearch/src poc/logsearch/main.t \
    --release --emit clif -o /tmp/ls.clif
grep -A1 '^; --- toy_ArchiveWriter__write_seg' /tmp/ls.clif

# 3. 関数ごとのコードサイズ (シンボル間の差分)
nm -n poc/logsearch/build/release/logsearch | awk '$2=="t"||$2=="T"'

# 4. 命令内訳
objdump -d --disassemble-symbols=_toy_ArchiveWriter__write_seg \
    poc/logsearch/build/release/logsearch
```

## 関連

- [`BACKEND.md`](BACKEND.md) — `CallWithSelfWriteback` の現在の形
- [`DEBUG_OBSERVABILITY.md`](DEBUG_OBSERVABILITY.md) — D3 の診断文字列
- [`todo.md`](todo.md) の CODE-SIZE-SELF-ABI
- [`poc/logsearch/design-docs/RUNTIME_GAPS.md`](../poc/logsearch/design-docs/RUNTIME_GAPS.md)
  — この POC が踏んだ処理系側の穴の一覧
