# MODULE IMPORTS — stdlib も `import std.hex` を要求する

> **状態: 提案。D1 の alias 束縛のみ landing 済み (2026-09-05)、
> 残りは未着手。**
> 対象: 「auto-load が全モジュールを 1 つの名前空間に流し込む」現状を、
> **ファイル単位の明示 import** に置き換える。
> 前段: [`todo.md`](todo.md) の BARE-NAME-COLLISION / TYPE-NAME-COLLISION
> (2026-09-05 の調査。**シンボル名のマングリングは解決にならない**という
> 結論の続き)。
> 隣接: [`MODULE_SYSTEM.md`](MODULE_SYSTEM.md) — 配置と qualifier の解決規則。
> 本文書は同 D5 (「暗黙に入る集合は stdlib 全部」) を**覆す**提案なので、
> 採否が決まるまで両方を読むこと。
> 実装サイト: [`interpreter/src/lib.rs`](../interpreter/src/lib.rs)
> (`integrate_modules`)、
> [`interpreter/src/module_integration.rs`](../interpreter/src/module_integration.rs)
> (発見・統合・preparse)、
> [`frontend/src/module_resolver.rs`](../frontend/src/module_resolver.rs)
> (`import` の探索)、
> [`frontend/src/type_checker/context.rs`](../frontend/src/type_checker/context.rs)
> (`lookup_fn_detailed`)、
> [`frontend/src/type_checker/module_access.rs`](../frontend/src/type_checker/module_access.rs)
> (`check_function_access`)、
> [`interpreter/src/evaluation/mod.rs`](../interpreter/src/evaluation/mod.rs)
> (`lookup_function_qualified`)、
> [`compiler_ir/src/lib.rs`](../compiler_ir/src/lib.rs) (`lookup_function`)。
> 仕様の正本: [`../docs/language.md`](../docs/language.md) の「Modules」。
> 状態の正本: [`todo.md`](todo.md)。
> 実測: 2026-09-05 (debug ビルド、warm `.toycache`、`core/` 46 ファイル)

## なぜ

### 1. 名前空間が 1 つしかない

`core/` の **323 個のトップレベル名** (うち自由関数 242、`pub` でないもの
40) が、ユーザのあらゆるファイルと同じ 1 つの bare 名前空間に居る。

- **`pub` は関数について今は飾り** — `check_function_access` の
  `is_same_module_access()` が `true` 固定なので、非 `pub` の関数も
  他モジュールから修飾付きで呼べる (`a::helper`)。
- **private な helper が名前を占める** — `core/std/base64.t` の
  `fn value(b: u8)` (非 `pub`)、`core/std/log.t` の `pub fn at(l, msg)`。
  ユーザが `fn value` を書ける保証は、今は root の rank しかない。
- **rank (BUILD-TOOL B0) が消したのは半分だけ** — 別 root なら後勝ちだが、
  **同じ root の 2 モジュール**は同 rank なので今も落ちる。`src/a.t` と
  `src/b.t` がそれぞれ private な `fn helper` を持ち、**各自が自分のを
  呼んでいる**だけで `[E0010] ambiguous call` になる。
- **型には曖昧性検査すら無い** (TYPE-NAME-COLLISION) — 2 モジュールが
  同じ `struct Item` を宣言すると後勝ちで黙って上書きされる。

**マングリングでは直らない** (前段の結論)。定義側の一意化は既に済んで
いる (`toy_std_math__add` / `function_index` の `(path, rank)` エントリ)。
残っているのは**使用側の解決規則**で、`import` はまさにその規則である。

### 2. hello world が 145 ms 払っている

| 構成 | 中央値 |
|---|---|
| 既定 (46 モジュールを auto-load) | **145 ms** |
| 15 モジュールだけの root | 32 ms |
| `TOYLANG_CORE_MODULES=` (prelude のみ) | **5.7 ms** |

`println("hello")` しかしないプログラムの話で、コストはほぼモジュール数に
比例する (15 モジュールの run は依存不足で `rc=1` なので上限寄りの目安)。
codegen は `Module::reachable_from` で刈られているので**これは解析コスト
だけ**であり、出力バイナリの話ではない。

### 3. 依存関係がファイルから読めない

どのモジュールが何に依存しているかは、今はソースのどこにも書いていない。
`std/f64.t` の `impl Abs` が `std/i64.t` の `trait Abs` に依存し、しかも
**統合順 (ファイル名のソート順) に依存している**という事故は
[`MODULE_SYSTEM.md`](MODULE_SYSTEM.md) が記録している。

## 現状 (計測)

| 事実 | 値 |
|---|---|
| `core/**/*.t` | 46 ファイル |
| トップレベルの輸出名 (fn/struct/enum/trait/type) | 323 |
| 自由関数 | 242 (非 `pub` 40) |
| stdlib 内の `import` 行 | **0** (`package` 行も 6 ファイルだけ) |
| モジュール間依存 (型参照 + `mod::` 修飾) | 119 辺、平均 2.6、**非循環** |
| stdlib 内の bare な他モジュール関数呼び出し | 33 か所 / 5 モジュール |
| `interpreter/example` + `poc` | 200 ファイル。`mod::` を使うのは 21 (最大 4 モジュール)、bare な stdlib 関数呼び出しは 3 か所 |

**構文はもう在る。** `import std.hex` も `import b` も今日 parse を通り、
auto-load 済みのモジュールに対する no-op として黙って受理される
(`as` だけはパーサが受けて `visit_import` が捨てる)。したがって本提案が
変えるのは**意味論**であって構文ではない。

**受け皿も在る。** `interpreter/src/prelude.t` (今は空) が常に統合され、
`check_typing_with_core_modules(..., &[])` は既に「prelude + 明示 import
だけ」の経路になっている。

依存グラフが**非循環**だったことは大きい — 明示 import に切り替えても、
モジュールを並べ直す作業は発生しない。

## 設計

### D1. import はモジュールを名前に束縛する (Go 型)。名前は持ってこない

```rust
import std.hex             # hex:: が使えるようになる
import std.hex as codec    # codec:: にする
```

`import std.hex` が入れるのは **alias 1 個**であって `encode` / `decode`
ではない。Rust の `use std::collections::HashMap` 型 (名前を持ち込む形)
を採らないのは、**それが今壊れている bare 名前空間をファイル単位で
作り直すから**。項目単位の import は非目標 (下)。

呼び出しは今と同じ `hex::encode(bytes)`。**書き方は変わらず、書ける条件が
変わる。**

> **landing 済み (2026-09-05)**: `as` の alias が効くようになった。
> 実装は**パーサでの置換** — `import a.b as h` を読んだ時点で
> `h -> b` を記録し、`h::f(...)` を組み立てる際に先頭セグメントを
> 差し替える (`frontend/src/parser/expr/primary.rs`)。qualifier は
> 元から末尾一致で解決される (MODULE-SYSTEM P2) ので、**型検査器 /
> tree-walker / IR lowerer は alias を知らないまま動く** — D9 が言う
> 「解決規則を増やさない」を、この小さな一歩でも守る形。alias は
> ファイル局所なので、1 ファイルしか見ないパーサが唯一「完全に」
> 解決できる場所でもある (`alias_resolution.rs` が型 alias で
> 逆に苦労しているのは、あちらがファイルを跨ぐため)。
>
> **まだ効いていないのは (c) の「bare 名を持ち込まない」**。これは
> 可視性の規則そのもの (D3 / D4) なので P1 に属する。今の段階では
> `import` は**足すと使えるものが増えるだけ**で、何も禁じない。
>
> 副産物として `X::f(...)` の未解決 qualifier の診断を
> `Struct 'X' not found` → `Type or module 'X' not found` にした。
> 逆に、**修飾したのに bare で引き直す**フォールバックが在ることも
> 判明した (`hex::abs(...)` が `std::math::abs` を返す) —
> alias 以前からある誤答で、[`todo.md`](todo.md) の
> QUALIFIER-BARE-FALLBACK に分けた。**D3 を入れるときに一緒に消える**
> はずのもの (呼び出し元の import 集合の外は候補にならないため)。

### D2. prelude を宣言する

import 無しで使える集合を `core/prelude.t` に**import 行だけのファイル**
として置く。コンパイラは「prelude が import しているものは全ファイルで
可視」とだけ知る (集合はデータで、Rust 側に焼かない)。

初期案 (Rust の prelude と対応):

```
option result           # Option / Result
collections/vec string str char     # Vec / String / str 拡張
box ptr span            # Box<T> / Ptr<T> / Span<T>
cmp clone default convert hash iter fmt drop   # trait 群
alloc num checked       # AllocError / 数値拡張トレイト
```

**外すもの**: `dict` / `set` / `deque` / `priority_queue` /
`collections/soa_vec` / `column` / `allocator` (Rust が `HashMap` を
prelude に入れないのと同じ理由)。`interpreter/example` + `poc` での実測
影響は `Dict` 5 ファイル・`Arena` 6・`FixedBuffer` 2・`SoaVec` 2・
`Column` 1 で、それぞれ import 1 行で済む。

### D3. 解決はファイル (= モジュール) 単位

| 書いた形 | 見える候補 |
|---|---|
| bare `f(...)` | **自分のファイルの `f`** → prelude が輸出する `f` |
| `m::f(...)` | 自分が `import` した alias `m` が指すモジュールの `pub fn f` |

これで BARE-NAME-COLLISION は**構造的に消える** — 2 つのモジュールが
private な `fn helper` を持っていても、互いの候補集合に入らない。
前段の調査で挙げた 2 つの規則 (「非 pub は自分のモジュールだけ」
「呼び出し元モジュールを rank より優先」) は、この規則の系になる。

### D4. `pub` を実効化する ✅ (関数のみ、2026-09-26)

(landing 済み: 非 `pub` の module 関数は自分の module からしか呼べない。
実際には stdlib の `poll.t` が `net.t` の非 pub な `extern fn` を 2 本
呼んでいたので、その 2 本を `pub` にした。import との結び付き
(「import で届くのは pub だけ」) は D5 と一緒。)

import で届くのは `pub` だけ。`check_function_access` の
`is_same_module_access()` を本物にする。**stdlib の非 pub 40 本を他
モジュールから呼んでいる箇所は 0 件**なので、この変更単体では stdlib は
壊れない。

### D5. 読み込むのは prelude + import の推移閉包だけ

145 ms の出所はここ。preparse は既に並列で走っているので、閉包の計算
(「発見 → 必要なものだけ preparse → 統合」) を挟むだけで、モジュール数に
比例していたコストが**プログラムが実際に触る分**になる。

未 import の名前は「無い」ではなく**在り処を言う**:

```
[E0003] `Dict` is not in scope
  = note: `Dict` is exported by module `std.dict`
  = help: add `import std.dict` at the top of this file
```

名前 → モジュールの索引は `extract_stdlib_type_names` /
`collect_top_level_type_names` が既に作っているものと同じ形で、
`.toycache` から復元できる (**全部を parse し直さずに索引だけ引ける**
ことが D5 の前提条件)。

### D6. 型は Phase 1 ではグローバルのまま。ただし重複はエラーにする

型をモジュールに属させる ([`MODULE_SYSTEM.md`](MODULE_SYSTEM.md) D3 が
「やらない」としたもの) は Phase 3 に置く。移行量が桁で違うため。
ただし **TYPE-NAME-COLLISION の「黙って後勝ち」は Phase 1 で潰す** —
2 モジュールが同じ型名を宣言したら、両方のパスを名指すエラーにする。

### D7. 循環 import は禁止しない

統合後は 1 つの `File` に畳まれ、初期化順という概念が無いので、循環は
実害を持たない (`ModuleResolver::detect_cycles` は診断のために残す)。
現状の依存グラフは非循環なので、禁止しても今は通るが、**禁止すると
`trait` と `impl` を別ファイルに置く自由が消える**ので採らない。

### D8. rank は「どの root のファイルを読むか」だけに縮む

`--core-modules` を複数指定したときに同じモジュールパスが複数の root に
在れば、後の root のファイルを読む (現状維持)。一方 **bare 名の勝敗を
rank で決める規則は不要になる** — D3 が先に効くため。3 つの表
(型検査 / interpreter / IR) から同じ規則を消せる。

### D9. 解決は型検査で 1 回だけ行い、結果を焼く

前段の調査で踏んだ罠がここに直結する: 同じ解決規則が**型検査器 /
tree-walker / lowering の 3 か所**に実装されていて、1 つ落とすと
「型検査した関数と実行される関数が食い違う」(rank を interpreter 側に
届け忘れて実際に踏んでいる)。import が入ると規則は今より複雑になるので、
**3 か所に配る形のまま拡張してはいけない**。

型検査器が bare / 修飾付き呼び出しを解決したら、その結果 (どのモジュールの
どの関数か) を AST 側に記録し、tree-walker と lowering は**記録を読むだけ**
にする。型検査器は既に `?` / `Display` / char リテラル / tuple struct で
AST を書き換えているので、経路としては新設ではない。`Expr::Call` は
bare 名しか持たないため、`File` に側テーブル (`ExprRef` → 解決先) を足すか、
呼び出しノード自体を修飾付きの形へ書き換えるかの選択になる。

### D10. 移行期は「解決できたが import が無い」を警告にする

いきなりエラーにすると既存プログラムが全部止まるので、1 段階挟む:

```
[W00xx] `math::sqrt` resolved without an import
  = help: add `import std.math`
```

`--format=json` の `suggestions` に載るので、修正は機械的に当てられる
(LLM ループがそのまま直せる)。既定を warning → error に倒すのが Phase 2 の
最後。

## 移行

| 対象 | 量 |
|---|---|
| stdlib に足す `import` 行 | **30 行 / 21 ファイル** (D2 の prelude を引いた後) |
| stdlib の bare な他モジュール呼び出しの修飾 | **33 か所 / 5 ファイル** (`hash_mix` / `abs` / `sqrt` / `join` / `dict_slot_empty` / `net_error_from_status` / `net_unit_result` / `net_u64_result`) |
| `interpreter/example` + `poc` | `mod::` を使う 21 ファイルに 1〜4 行、prelude 外の型を使う 16 ファイルに 1 行、bare 呼び出し 3 か所 |

依存グラフが非循環なので、import 行は**機械的に生成できる** (本文書の
数値を出したスクリプトと同じ解析)。手で書き起こす作業ではない。

## フェーズ

| | 内容 | 解消するもの |
|---|---|---|
| **P0** | D1 の alias 束縛 (**2026-09-05 landing**) | `import ... as` が捨てられていた件 |
| **P1** | D3 / D4 / D6 の重複エラー / D9 の焼き込み / D10 の警告。**読み込みは今のまま全部** | BARE-NAME-COLLISION、TYPE-NAME-COLLISION、`pub` の無効化、QUALIFIER-BARE-FALLBACK |
| **P2** | D5 の遅延読み込み + 「import を足せ」診断。D10 を error に倒す | 145 ms |
| **P3** | 型の名前空間化。`shadowed_stdlib_types` (`__std_<name>` 退避) の撤去 | 型の衝突を規則で消す |
| **P4** | 後片付け: D8 (bare 名の rank 規則を削除)、`import ... as` を honour ([`MODULE_SYSTEM.md`](MODULE_SYSTEM.md) P3 #4)、多セグメント qualifier (#1)、`mod.t` の 2 経路統一 (#3) | MODULE_SYSTEM P3 |

P1 と P2 を分けるのは、**名前解決の正しさ**と**読み込み量の最適化**が
別の失敗をするため。P1 だけで止めても意味がある (衝突が消える)。

## リスク

- **`FULL_AST_CACHE_SCHEMA_VERSION`**: `File` に import 情報を足すので必ず
  上げる。上げ忘れると無関係な `val a: u64 = 5u64` が stdlib 由来の型
  エラーで落ちる (`frontend/src/cache.rs` の前例 v9 / v29 / v33 / v37)。
- **4 レーン一致**: D9 を守らないと、型検査と実行で違う関数に行く。
  `compiler/tests/consistency/` に「同名 private helper を持つ 2 モジュール」
  のテストを置く。
- **P2 の遅延読み込みは method を消しうる**: trait impl が居るモジュールを
  読まないと `v.sort()` が突然「そんな method は無い」になる。D2 の prelude
  にどれを入れるかが実質この問題で、**判断材料は「型と impl がどこに在るか」
  であって使用頻度ではない**。
- **`--effects` は型検査を通す**ので import の影響を受ける。`--api` は
  1 ファイルを parse するだけなので影響しない。
- **`toy`**: `src/` を root に積む規約は変わらない。パッケージ内の相互
  参照は `import <モジュール名>` (root 相対) になる。

## 非目標

- **項目単位の import** (`import std.hex::{encode, decode}`) — D1 の理由。
  必要になったら別途。
- **glob import / re-export** — prelude だけが「まとめて可視にする」形を
  持ち、それ以外は無い。
- **可視性の階層** (`pub(crate)` 相当) — `pub` か否かの 2 値のまま。
- **モジュールごとの分割コンパイル** — 統合して 1 つの `File` にする形は
  変えない。本提案は「何を統合するか」だけを変える。
