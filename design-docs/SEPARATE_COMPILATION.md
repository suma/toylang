# AOT の中間オブジェクト (分離コンパイル) 検討メモ

**日付**: 2026-08-19
**問い**: AOT (compiler) が「パース・意味解析済みデータを持つ中間オブジェクト
ファイル」を生成し、それをリンクする形にすべきか。

**結論**: **今は作らない。** 中間オブジェクト化で削れるのは 1 コンパイル
あたり最大 ~7ms で、その隣に `cc` のリンク 55ms が居る。さらに大きな
プログラムの遅さは分離コンパイルの欠如ではなく **frontend の O(n²)** が
原因で、そちらは局所的な修正で 5x 縮む (実測)。

この文書は測定の記録であり、[`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md)
の Phase 5 再スコープ (2026-08-10) の続き。前回と同じ判断が別の角度からも
成り立つことを確認し、代わりに直すべき箇所を特定した。

## 0. 前提: 「パース済み中間ファイル」は既に半分ある

`.toycache/<prefix>/<hash>.full` が **モジュール単位の AST + interner** を
ソースハッシュ键で保存している (`frontend/src/cache.rs`)。未実装なのは
「**型検査済み / IR 済み**を保存し、リンクする」層だけ。つまり検討対象は
「中間ファイルを持つか」ではなく「**どこまで進んだ表現**を持つか」。

## 1. 測定 (release build, macOS/M 系, warm, 10 回平均)

再現:

```bash
cargo build --release -p compiler
export TOYLANG_CORE_MODULES=<repo>/core
export TOY_LINK_CACHE_DIR=/tmp/linkcache   # リンクキャッシュ有効時のみ
target/release/compiler interpreter/example/std_iter_adapt.t --emit=ir -o /tmp/a.ir
```

### 小さいプログラム (`interpreter/example/std_iter_adapt.t`)

| 計測 | 時間 |
|---|---|
| プロセス起動 (`--help`) | 4 ms |
| `--emit=ir` (起動 + frontend + lower) | 13 ms |
| `--emit=obj` (+ cranelift codegen) | 12 ms |
| exe (link cache ヒット) | 15 ms |
| **exe (実 `cc`)** | **67 ms** |
| `--emit=ir` + `TOY_CACHE_DISABLE=1` | 28 ms |

分解すると **frontend+lower ~9ms / codegen ~0ms / リンク 55ms**。
既存の AST キャッシュが 15ms 稼いでいる (28 → 13)。

### stdlib の固定費

| 計測 | 時間 |
|---|---|
| `fn main() -> u64 { 0u64 }` (stdlib auto-load 有効) | 11 ms |
| 同上・`TOYLANG_CORE_MODULES=` (auto-load 無効) | 5 ms |

→ **stdlib の固定費は ~6-7ms**。AST キャッシュ込みでこの値なので、
「型検査済み / IR 済み stdlib」を持ち込んで削れる上限がこれ。**リンクの
55ms より 1 桁小さい**。

## 2. 大きいプログラムは壊れているが、原因は別

自動生成 (`fn funK(x: u64) -> u64` を N 個 + 100 個ずつ束ねる `aggK` +
`main`) で測ると:

| 規模 | exe | `--emit=ir` |
|---|---|---|
| 1,204 行 | 98 ms | 98 ms |
| 6,004 行 | 1,693 ms | 1,690 ms |
| 24,004 行 | 26,165 ms | 26,175 ms |

`--emit=ir` と exe がほぼ同じ = **時間は全部 IR より前 (frontend)**。
かつ n^2 (規模 4x で 15.5x)。`sample(1)` で犯人は 2 つ。

### (a) `Parser::offset_to_line_col` — `frontend/src/parser/core.rs:393`

offset → (line, column) をソース先頭からの線形走査で毎回計算する。
`current_source_location` 経由で **141 箇所**から呼ばれるので、パース全体が
O(n²) になる。24,004 行のプロファイルでは `parse_program` が実行時間の
ほぼ 100%、その葉が `current_source_location`。

**プロトタイプ**: `Parser` に行頭 offset の `Vec<usize>` を持ち、二分探索 +
行内の char 数で列を出す (計測後 revert 済み):

| 規模 | before | after |
|---|---|---|
| 1,204 行 | 98 ms | 44 ms |
| 6,004 行 | 1,693 ms | 380 ms |
| 24,004 行 | 26,165 ms | **4,885 ms (5.3x)** |

3 サイズとも**生成バイナリはバイト一致** (`cmp` で確認)。列は char 単位の
まま計算しているので診断の出力も変わらない。

### (b) `finalize_number_types` — `frontend/src/type_checker/type_conversion.rs:329`

(a) を直した後の残り時間の **83%** がここ。`visitor.rs:831` で
**関数 1 個の型検査が終わるたびに呼ばれ**、その中で stdlib を含む
**全プログラムの `ExprPool` を端から端まで走査**する。さらに Number ごとに
`context_info.iter().any(...)` の線形検索が入る。関数数 × 全ノード数。

同じファイルの `record_number_usage_context` (l.253) と
`propagate_to_number_variable` (l.315) にも全プール走査がある。こちらは
`is_number_for_variable` が `variable_expr_mapping[var]` との一致しか見ない
ので、**mapping の 1 回引きと等価**に書き換えられる (プロトタイプで
バイト一致を確認。ただし今回のベンチでは経路に乗らず時間は変わらなかった)。

### (c) 付随して見つかった規模の壁

- `parse_block_impl` の `MAX_ITERATIONS = 1000`
  (`frontend/src/parser/expr/mod.rs:175`) により、**1000 文を超えるブロックは
  パースエラー**になる (`Maximum parse iterations reached in block`)。
  無限ループ検知のつもりが言語の制限として観測される。検知は反復回数ではなく
  「トークン位置が進んでいないこと」で行うべき。
- 関数名に primitive type キーワードが使えない (`fn f64(...)` が
  `expected function name`)。ベンチ生成で踏んだ。既知制限に追記済み。

## 3. 分離コンパイル自体が今の設計と衝突する点

仮に着手するとして、以下は全部「先に解く」必要がある:

- **型検査が whole-program** — stdlib を 1 プールに merge してから検査する。
  `ModuleInterface` はあるが「モジュール単位で封印された検査」ではない。
- **単相化が whole-program** — lower 時に全プログラムから収集する。
- **到達可能性の刈り込みが whole-program** — f0aa50d で入った
  「`main` から到達可能な関数だけ lower / codegen」。モジュール単位 `.o` は
  到達不能コードを抱えるか、リンク時 GC が要る。
- **`dyn Trait` の vtable FuncId / drop glue / `Display`・`==` の型検査時
  rewrite** が全プログラム情報に依存する。
- **キャッシュ健全性のリスクが既に高い** — interner の intern 順が変わると
  古い `.full` が別の意味に化ける (`FULL_AST_CACHE_SCHEMA_VERSION` が存在する
  理由)。IR まで載せると invalidation の面積が広がる。
- **cranelift-object に COMDAT / linkonce 相当が無い** — C++ 式の
  「各 `.o` に単相化を出してリンカで重複排除」が素直にできない。

## 4. 推奨する順序

> **2026-08-20 更新**: 1〜3 は実施済み (完了済み節 / コミットの測定を参照)。
> 1 は `Parser::new` で行頭テーブルを構築して二分探索、2 は Number ノードの
> 遅延インデックス + `variable_expr_mapping` 逆引き、3 は進捗なし検知。
> 実測 (24k 行): 99.97s → **0.14s**、`--emit=ir` はバイト一致。残るは 4 のみ。

1. ~~**`offset_to_line_col` の行頭テーブル化**~~ — 実測 5.3x、出力バイト一致。
   小さいプログラムでも効く (1,204 行で 98 → 44 ms)。**実施済み (2026-08-20)**: 20k 行 99.97s → 0.14s。
2. ~~**`finalize_number_types` を検査中の関数の範囲に限定 (または Number
   ノードの索引を持つ)**~~ — (a) の後の支配項。**実施済み (2026-08-20)**: Number インデックス (遅延・インクリメンタル) と `variable_expr_mapping` の逆引きで、全プール走査とエントリごとのマップ clone を除去。
3. ~~**`MAX_ITERATIONS` 撤廃**~~ — 進捗なし検知に置き換える。**実施済み (2026-08-20)**: `current_position` 比較で無限ループだけを検知。1500 文ブロックがパース可能に。
4. **リンクが支配的な件** — `TOY_LINK_CACHE_DIR` の既定 on、`-fuse-ld=lld`
   等。小さいプログラムの 55ms はここ。
5. ここまでやって、なお**複数ファイルの実プログラムで lowering が支配的**に
   なることを再測定できたら、初めて「モジュール単位 IR + IR リンカ」。

## 5. 将来やるとしたら (条件と最小設計)

**着手条件**: 実プロジェクト規模で `--emit=ir` の時間から frontend を引いた
残り (= lowering) が、全体の 30% を超えること。今は ~4ms / 19ms。

**最小設計**: AST キャッシュの上に積むのではなく、

- 層 1: **型検査済み `ModuleInterface`** (公開シグネチャ + trait impl 表)
- 層 2: **モジュール単位 IR** (単相化前)
- 単相化は**リンク時単相化** — instantiation request を artifact に残し、
  リンカ側で dedupe して 1 回だけ具体化する
- invalidation は層 1 のハッシュで cascade (層 2 は自ソース + 依存 interface
  ハッシュ键)

**別の動機**: 「ソース非公開でライブラリを配布したい」という要求が出てきたら、
性能とは独立に中間オブジェクトの理由になる。現状 `core/std` はソース配布
なので今は薄い。
