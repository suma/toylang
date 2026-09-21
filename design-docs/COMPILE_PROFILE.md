# COMPILE-PROFILE — AOT コンパイルのどこに時間がかかっているか

> 状態: **landing 済み (2026-09-22)**。`compiler --profile=compile` /
> `toy build --profile=compile`。下の項目は実装と一致する。
> 設計時の案から、lowering の段 (`declare` / `bodies` / `finish`) と
> lowering のホットスポット表を足した — 最初の計測で lowering が
> 最大 (45%) なのに中身が見えなかったため。

## 動機

`compiler foo.t` の中身は、ソース読み込み → パース → モジュール統合 →
型検査 (+ 書き換え・後段検査) → lowering → cranelift codegen →
オブジェクト出力 → リンク、と 4 クレート (`frontend` / `interpreter` /
`compiler_lower` / `compiler`) を跨ぐ。時間を測る手段は
`compiler/examples/profile_e2e.rs` / `profile_jit.rs` の ad-hoc な
計時だけで、しかも「parse」「check」くらいの粒度しかない。

規模感 (2026-09-22、release ビルド、`interpreter/example/fib.t`):
全体 ~80ms。entry は数百バイトだが、**stdlib 46 ファイル・~475KB を
毎回統合している**。つまり「ユーザのソースが大きいから遅い」とは
限らず、**固定費がどこに居るか**を分けて見られることが要る。

## 使い方

```bash
compiler foo.t --profile=compile                 # stderr に表 (text)
compiler foo.t --profile=compile --format=json   # stderr に JSON 1 文書
compiler foo.t --profile=mem,compile             # `--profile` はカンマ区切り / 繰り返し可
toy build mypkg --profile=compile [--format=json]
```

- `--all-backends` とは併用できない (1 回の AOT ビルドを測るものなので)。
  `toy` では `build` だけが受ける
- **コンパイルが失敗しても出す** — どのフェーズまで進んで何 ms
  かかったかも答えの一部
- `--profile=mem` (実行時のメモリ) と同じ `--profile=` の値。
  出力の形は `--format` に従う
- 出力先は **stderr** (`--format=json` の結果文書が stdout に出るので
  衝突させない)
- 無効時のコストは atomic load 1 回/計測点

## 保存する項目

### A. 実行の条件 (結果を比べるときに揃っているべきもの)

| 項目 | 理由 |
|---|---|
| `input` / `emit` / `release` | 何をどうコンパイルしたか (`release` は toylang の `--release` = 契約を外す) |
| `compiler_build` (`debug` / `release`) | **コンパイラ自身**のビルド。debug ビルドは数倍遅く、他の差を全部埋もれさせる |
| `cranelift_opt_level` (`TOYLANG_CRANELIFT_OPT_LEVEL`) | codegen 時間が ~20x 変わる |
| `ast_cache` (有効 / 無効、ディレクトリ) | モジュールのパースが丸ごと消える |
| `link_cache` (有効 / 無効) | リンクが丸ごと消える |
| `codegen.threads` (counter。codegen の rayon プール) | 並列区間の wall と CPU の比を読むため |
| `total_ms` (wall) / `user_ms` / `sys_ms` / `peak_rss_bytes` | 全体。`getrusage` で取れる。CPU > wall なら並列が効いている |

### B. フェーズの木 (時間)

各ノードは `name` / `start_ms` (開始時刻、全体の先頭から) /
`wall_ms` / `children` (子があれば `self_ms` も)。**入れ子で持つ**のは、
親の時間から子の合計を引いた「どの子にも属さない時間」が見えるように
するため (測り漏れの検出)。トップレベルの外側の時間は text 表示の
`(outside any phase)` 行に出る。

モジュールは 1 個 1 ノードにしない (stdlib だけで 46 行になり木が
読めなくなる)。1 ファイルごとの時間は C のファイル表に持つ。

```
(total)
├─ read_source
├─ parse                         entry のみ
├─ modules
│  ├─ prelude
│  ├─ discover                   module root の走査 (全ファイルを読む)
│  ├─ preparse                   並列 (rayon)。AST キャッシュ読み or パース
│  ├─ integrate                  逐次。別 interner からの写し込み
│  └─ imports                    `import` で明示されたもの
├─ resolve_aliases
├─ recursive_types
├─ typecheck
│  ├─ setup / declarations / consts / impl_blocks / functions
│  ├─ rewrites                   newtype / ?? / Ord / ... の書き換え
│  ├─ post_checks                moves / never_allocates / const_fn /
│  │                             unsafe / module_paths / parallel / regions
│  ├─ const_fold                 const fold + 計算された配列長
│  │  └─ ctfe_lower              fold が IR VM で走らせるための 2 度目の
│  │                             lowering。`quiet()` の中なので lower の
│  │                             表・カウンタには入らない (時間だけ残る)
│  └─ lints                      contract_purity / unused_results
├─ lower                         compiler_lower::lower_program (+ test driver)
│  ├─ declare                    型定義の収集・全関数 / method の宣言
│  ├─ bodies                     到達可能な body を queue から drain
│  └─ finish                     dead const 除去・writeback の刈り込み
├─ codegen
│  ├─ declare
│  ├─ compile_functions          並列区間 (wall)。CPU 合計は counters に
│  ├─ define                     逐次 (ObjectModule への書き込み)
│  └─ emit_object
└─ link | write                  emit 種別による
```

### C. 入力の量 (何バイトを処理したか)

| 項目 | 単位 |
|---|---|
| **ファイルごと**: `path` / `origin` (`entry` / `prelude` / `stdlib` / `package`) / `bytes` / `lines` / `ast_cache` (`hit` / `miss` / `off`) / `parse_ms` / `integrate_ms` | 1 行 1 ファイル。text は上位 10、JSON は全部 |
| 合計: `files` / `bytes` / `lines`、**`entry` と `modules` (それ以外) に分けて** | 固定費の大きさを見る |

`origin` の `stdlib` / `package` は module root の順位で決める (最初の
root が `stdlib`)。stdlib の `parse_ms` は並列に走った時間なので重なる。

### D. 各フェーズが処理した量 (時間を量で割れるように)

| フェーズ | 項目 |
|---|---|
| modules / AST | 統合後の関数数・文数・式数 (`ast.*`) |
| typecheck | 検査した関数数 (entry / それ以外)、impl block 数、error / warning 数 |
| lower | IR 関数の宣言数、body を lowering した数 (到達可能な分)、block 数、命令数 |
| codegen | compile した関数数、スレッド数、**関数ごとの compile 時間の合計 (`codegen.cpu_us`)**、機械語バイト数、reloc 数、object バイト数 |
| link | `link.cache_hit` / `link.cache_miss` (キャッシュ無効なら両方無い)、実行ファイルのバイト数 |

### E. ホットスポット (上位 N = 10)

| 表 | 1 行 |
|---|---|
| typecheck が重い関数 / impl block | 名前 / ms |
| lower が重い body | 名前 / ms / IR 命令数 |
| codegen が重い関数 | 名前 / ms / IR 命令数 / 機械語バイト数 |

typecheck の関数の時間には、**呼び出しが前倒しで検査させた callee の
body が含まれうる** (`type_check_forward_ref`)。impl block は method
単位ではなく block 単位 (検査器の入口がそうなっているため)。
codegen は並列なので、各行の時間は重なる。

フェーズの木だけだと「codegen が 60%」までしか言えない。**どの関数が**
まで降りられないと手が打てない。

## 保存しない項目 (と理由)

- **フェーズごとのメモリ確保量** — global allocator の差し替えが要り、
  それ自体が計測を歪める。ピーク RSS (A) で代える
- **トークン数** — lexer は rflex 生成でパーサに組み込まれており、
  パースと分けて測れない。バイト数・行数で代える
- **式 1 つ単位の時間** — 計測点のコストが本体を上回る
- **`--all-backends` / JIT / interpreter の実行** — 対象は AOT の
  コンパイル。実行時の性能は `--profile=mem` と既存の計時の担当

## 実装の形

- 記録器は **`frontend/src/compile_profile.rs`** (4 クレートすべてが
  依存する最下層)。プロセス全体で 1 つ、`Mutex` で守る (codegen と
  stdlib の preparse は rayon の複数スレッドから書くので thread-local
  では足りない)
- API は RAII の `phase("typecheck")` (drop で閉じる) と `count(key, n)` /
  `file_parsed` / `file_integrated` / `hot` / `hot_record`。無効時は
  `AtomicBool` の load 1 回で帰る。`timer()` は無効なら `None` を返すので、
  ループの中で時計を読まない
- **フェーズは `enable()` したスレッドでしか開かない** (他スレッドから
  開くと入れ子のスタックが混ざる)。counter / file / hot はどのスレッド
  からでもよい
- 表示 (text / JSON、見出しの A) は **`compiler/src/compile_profile.rs`**。
  記録器はデータだけを持つ
- 同じ pass を別目的でもう一度走らせるところは `quiet()` で包む
  (段は記録し、重い関数の表とカウンタには入れない)。でないと 1 つの
  関数が表に 2 回載り、本番の量と混ざる
- 計測点を足すときは、既存のフェーズの**中に**入れ子で置くこと。
  親の `self_ms` が大きいのは「名前の無い時間」がある印

## 最初の計測で分かったこと (2026-09-22)

| プログラム | 条件 | 最大の段 |
|---|---|---|
| `interpreter/example/fib.t` | release ビルドのコンパイラ、キャッシュ温 | **link 53%** (`cc` 起動)。次が `modules/discover` 10% (初回。stdlib 46 ファイル・~475KB を毎回全部読む) |
| `poc/logsearch` | debug ビルドのコンパイラ | **lower 45%** (うち `bodies` 36%)、codegen 27%。lowering の最大は `ArchiveWriter::write_seg` 1 本で 59ms (2809 IR 命令) |

小さいプログラムでは固定費 (リンク・stdlib の読み込み) が、大きい
プログラムでは lowering が支配的。
