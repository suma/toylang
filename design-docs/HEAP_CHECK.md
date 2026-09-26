# HEAP-CHECK — 解放済みメモリを毒化・再利用する検査モード

> **状態: 全フェーズ landing 済み (2026-09-26)** — H0 (二重 free の棚卸し)・H1 (interpreter
> レーンの `poison`)・H2 (compiled レーンの `poison`)・H3 (`reuse`)・H4 (redzone と
> `__builtin_heap_poison`)・H5 (二重 free をエラーに、example の常時検査)・`toy` の
> `--heap-check`。§6 の未決事項は推奨どおりに決まった。**使い方は §0、仕様の正本は
> [`docs/language.md`](../docs/language.md) の「Heap checks」**。§1〜§5 は着手前の設計
> (数字はその時点のもの)、§7〜§13 が各フェーズの実装と結果。
> 関連: [`MEMORY_PROFILING.md`](MEMORY_PROFILING.md) (計数の定義と site)、
> [`ALLOCATOR_PLAN.md`](ALLOCATOR_PLAN.md) (`with allocator` と stdlib `Arena`)、
> [`REGIONS.md`](REGIONS.md) / [`POINTER.md`](POINTER.md) (静的な脱出検査)。

## 0. 要約

ヒープは**解放したブロックを再利用せず、中身も残し、free は冪等**。そのため
use-after-free は静かに古い値を返し、二重 free は何も起こさず、バグが症状として
出ない。検査モードはこれを見えるようにする。

| モード | 何をするか |
|---|---|
| `report` | 二重 free を (確保位置, 1 回目の free, 2 回目の free, 呼んだ関数) ごとに数えて終了時に出す。止めない (§7) |
| `poison` | free したブロックを `0xDB` で埋めて永久に隔離。解放済みへの読み書き、ブロック末尾より後ろから**始まる**アクセス (16 バイトの redzone)、二重 free で止まる (§8・§9・§11・§12) |
| `reuse` | `poison` + 有限の隔離。溢れたブロックを size class ごとに次の確保へ渡し、「番地を再利用しない」ことへの依存を炙り出す (§10) |

```bash
cargo run -q -p interpreter -- --heap-check=poison prog.t                  # tree-walker / IR VM
cargo run -q -p interpreter -- --heap-check=reuse --heap-quarantine=0 --test prog.t
cargo run -q -p compiler -- prog.t --heap-check=poison -o prog && ./prog   # 計装入りの AOT
cargo run -q -p compiler -- prog.t --all-backends --heap-check=report      # 3 レーン突き合わせ
cargo run -q -p toy -- test mypkg --heap-check=poison                      # パッケージ (§13)
TOY_HEAP_CHECK=report ./prog                                               # 計装なしのバイナリ
```

- **4 レーンで同じ文言**。tree-walker はヒープの読み書き口で、IR VM と AOT / JIT は
  lowering が生メモリアクセスと free の前に置く検査命令 (`HeapCheck` / `HeapCheckFree`)
  で判定する。レーン間で食い違わないよう、判定はアクセスの**開始位置**だけで行い、
  メッセージにバイト数は出さない。
- **当初の壁は言語自身だった**: drop glue が冪等な free に依存していて、着手時点で
  consistency テストだけで 209 回の二重 free があった (§1.3)。H0 で出所を並べて潰し
  (§7)、今は example 全体が poison の 3 レーンで常時走っている (§12)。
- **見ないもの**: ブロック内から始まって末尾をまたぐアクセス、ヒープ外 (stack の配列、
  `str` リテラル) の範囲外、再利用された後の古いポインタ (ABA)。

## 1. 現状

### 1.1 ヒープは 2 実装

| 実装 | 使うレーン | 場所 |
|---|---|---|
| `HeapManager` | tree-walker、IR VM | `interpreter/src/heap.rs` (IR VM は `ir_vm/host.rs` の `with_heap` 経由) |
| bump region | AOT、compiler 側 JIT | `compiler/runtime/toylang_rt/src/lib.rs` (`toy_dispatched_alloc` / `_free` / `_realloc`) |

どちらも**番地を再利用しない bump**で、16 バイト境界に揃えて切り出す。
`HeapManager` は `Vec<u8>` のバイト列に加えて、書き込まれた値そのものを
`(base, offset)` で持つ「型付きスロット」を持つ。

### 1.2 冪等な free は設計上の前提

`toy_dispatched_free` のコメントがそのまま理由を述べている:

> Free is *idempotent*: ... That is what makes recursive drop glue safe
> under this language's aliasing (`val b = a`, a `get()` copy, a boxed
> node shared by two paths): the first free wins, later visits of the same
> address do nothing ... a later glue walk reads the block's original
> contents, not garbage.

つまり **drop glue が別名経由で同じノードに 2 回たどり着いたとき、
2 回目は解放済みブロックを読んで、解放済みの番地を free する**。
それが無害なのは、再利用しないから・中身が残るからでしかない。

その後、ELEMENT-BORROW (`[E0028]`)、MATCH-MOVE-OUT-DOUBLE-DROP、
LEND-* など「所有者を 1 人にする」作業が進み、別名経由の二重 drop は
大幅に減った。**減ったが、まだ残っている** (次節)。

### 1.3 実測 (2026-09-26)

`HeapManager::free` に「一度解放した番地がもう一度来たら数える」計測を
一時的に入れて測った (コミットしていない)。

| 対象 | 解放済み番地の再 free |
|---|---|
| `interpreter/example/*.t` | 4 本: `box_binary_tree.t` 16 回、`json_config.t` 6 回、`box_linked_list.t` 6 回、`crypto_sha256.t` 3 回 |
| `compiler/tests/consistency/` (in-process のレーン) | 計 **209 回** |

`Box` の連結リスト / 二分木が典型で、再帰的な drop glue が同じノードを
2 経路から訪れている。**再 free が起きている以上、その直前の glue の読みは
解放済みブロックの読み (use-after-free) でもある**。

### 1.4 今は何が見えないか

| 誤り | 今の挙動 |
|---|---|
| 解放済みブロックの読み | 解放前の値が返る (静かに正しい) |
| 解放済みブロックへの書き込み | 誰も読まないので無害に見える |
| 二重 free | 何もしない (冪等) |
| **`push` をまたいで保持した窓** | 再確保前の古いバッファを読み続ける。`core/std/span.t:39` に「未検査の穴」として書いてある |
| ヒープ内の範囲外 (`Ptr` の未検査添字) | 隣のブロックか、16 バイト境界のパディングを読み書きする |

最後の 2 つは静的検査 (`[E0026]` WINDOW-ESCAPE、`Span` の境界検査) が
意図的に覆っていない範囲で、**動的な検査が最後の砦になる**場所でもある。

### 1.5 計数は影響を受けない

`MemoryStats` は「プログラムが**要求したもの**」で定義されている
(MEMORY_PROFILING M0)。番地の再利用や毒埋めは要求ではないので、
検査モードを入れても `--profile=mem` の数字と `leaks` は変わらない。
これは検査モードと計数を同時に使える根拠になる。

## 2. 目的と非目的

**目的**

1. 解放済みブロックの読み書き (use-after-free) を、**アクセスした位置で**止める。
2. 二重 free を止める。
3. 報告は toylang の言葉で出す — アクセスした位置、ブロックを確保した位置、
   解放した位置、backtrace。
4. **4 レーンで同じ文言** (`--all-backends` で突き合わせられること)。
5. 検査を切っているときのコストはゼロ (命令を出さない)。

**非目的**

- stack への `&T` / `&mut T` (lowering で消える、寿命は静的検査の担当)。
- データ競合 (`parallel for` はスレッドを持たない、CONCURRENCY)。
- 未初期化読み (`HeapManager` の「一度も書かれていない」エラーが既に一部を担う)。
- **再利用された後の古いポインタの完全な検出** (ABA)。`reuse` モードは
  「揺さぶって症状を出す」ためのもので、隔離中を除いて精密ではない。

## 3. 2 つのモード

| | `poison` | `reuse` |
|---|---|---|
| free したブロック | 毒で埋めて**永久に隔離** | 毒で埋めて**有限の隔離**に入れ、溢れたら再利用 |
| 解放済みへのアクセス | 必ず報告 | 隔離中なら報告、再利用後は他のブロックを壊す |
| 二重 free | 報告 | 隔離中なら報告 |
| 用途 | 精密な検出 (テスト、CI) | 「再利用しない」ことへの依存を炙り出す |

`reuse` が要る理由: 実際の malloc は番地を再利用する。今のプログラムが
「解放しても中身は残る」ことに依存していても、それは bump の性質であって
言語の約束ではない。**再利用を起こしてはじめて依存が症状になる**
(古い窓が別の値を読む → テストが落ちる、またはレーン間で答えが割れる)。

## 4. 設計の論点

### 論点 1: どこで検出するか

| 案 | 内容 | 評価 |
|---|---|---|
| (a) 毒埋めだけ | free で中身を毒で上書きする | 実装は最小。ただし検出ではない — 読んだ値がおかしくなるだけで、どこで起きたかは言えない |
| (b) ガードページ | ブロックごとにページを割り、free で `PROT_NONE` | 計装なしでネイティブコードの UAF が SIGSEGV で止まる。だが 1 確保 1 ページで重く、compiled レーンにしか効かず、IR VM / tree-walker では使えない。報告もシグナルハンドラ頼みになる |
| **(c) 計装 + shadow** | lowering が生メモリアクセスの前に検査命令を挟み、ランタイムが shadow で判定する | 精密で、アクセス位置を名指せる。IR VM / AOT / JIT が同じ lowering を通るので**報告が揃う**。検査を切れば命令を出さない |

**推奨: (c)。(a) はその一部として使う** (毒は「計装が漏れた経路」の保険になる)。
(b) は採らない。

### 論点 2: shadow の表現

両ヒープとも 16 バイト境界で切り出すので、**16 バイト (1 granule) ごとに
1 バイトの shadow** を持つ:

| 状態 | 意味 |
|---|---|
| `UNTRACKED` | ヒープ外 (stack、静的領域、`str` リテラル) — 検査しない |
| `LIVE_START` / `LIVE` | 生きているブロックの先頭 / 続き |
| `FREED_START` / `FREED` | 解放済み (隔離中) のブロックの先頭 / 続き |
| `REDZONE` | H4 で足すブロック間の緩衝 |

ブロックの情報 (サイズ、確保 site、解放 site) は先頭番地をキーにした表に
置く。報告のときだけ、shadow を逆向きにたどって `*_START` を探す。
16 バイト未満の端 (サイズ 20 のブロックの 21〜32 バイト目) は、先頭の
granule に「有効バイト数」を持たせれば判定できる。

- `toylang_rt`: bump chunk ごとに shadow 配列を横に持つ。
- `HeapManager`: 自前の仮想番地空間 (`next_addr` を進める `Vec<u8>`) なので、
  同じ形の `Vec<u8>` を 1 本持てばよい。

**同じ表現にしておくことが、報告の文言を揃える前提になる。**

### 論点 3: 計装する命令

新しい IR 命令 `HeapCheck { ptr, offset, len, write, site }` を、
検査モードのときだけ次の命令の前に出す:

| 命令 | 検査する範囲 |
|---|---|
| `PtrRead` / `PtrWrite` | `offset` から要素幅 (compound の leaf 展開では leaf ごと) |
| `SimdLoad` / `SimdStore` | 16 バイト |
| `MemCopy` / `MemMove` | 転送元と転送先の両方 |
| `MemSet` / `MemEq` / `MemFind` / `MemFindSeq` | 範囲全体 |
| `StrFromBytes` | 範囲全体 |
| `LoadRef` / `StoreRef` | **含める** — `borrow(i)` は `__builtin_ptr_ref` でヒープ上の要素を指す `&T` を作るので、ヒープの番地が来る。stack の番地は shadow が `UNTRACKED` と答えて素通りする |

`Ptr` の `get` / `set` / `borrow` (MEMORY-ACCESS M5) と `Span` の範囲演算
(SPAN-RANGE-INTRINSIC) は intrinsic として上の命令に展開されるので、
**stdlib の窓を経由するアクセスも自動的に検査される**。

free 側 (二重 free、解放済みの realloc) は命令を増やさず、ランタイムの
`free` / `realloc` の中で判定する。

tree-walker は lowering を通らないので、`HeapManager` の読み書き口
(`read_bytes_raw` / `typed_read` / `write_bytes_raw` / `typed_write` /
`copy_memory` など) で同じ判定をする。アクセス位置は評価中の式から取る。

### 論点 4: drop glue の依存 (二重 free)

§1.3 のとおり、今のまま `poison` で二重 free を止めると、`Box` を使う
プログラムの大半がすぐ落ちる。選択肢:

| 案 | 内容 |
|---|---|
| (a) 最初からエラー | 検査モードが実用にならない。209 か所を全部直すまで使えない |
| **(b) 棚卸しから始める** | `--heap-check=report`: 落とさずに、(確保 site, 最初の free site, 2 回目の free site) の組を重複なしで列挙する。原因を潰してから (a) にする |
| (c) glue の再訪だけ許す | drop glue から来た free / 読みを見逃す。見逃す範囲が曖昧になり、検査の意味が薄れる |

**推奨: (b)**。H0 はこれだけで価値がある — **「別名経由の二重 drop」が
どこに残っているかの一覧**になり、所有権の静的検査 (move_check) の穴を
直接指す。CALLEE-DROP-GENERIC で「二重 free よりは漏れる方が安全」と
判断した箇所の検証にもなる。

### 論点 5: 毒の値

- `0xDB` で埋める (u64 で読むと `0xDBDBDBDBDBDBDBDB`)。print に出たら一目で分かる。
- ポインタとして読むと、x86-64 / aarch64 とも非正規番地になるので、
  計装が漏れた経路でも deref で即座に落ちる。
- `HeapManager` は型付きスロットも消す (でないと解放前の `RcObject` が返る)。

### 論点 6: realloc

`realloc` は古いブロックを free する。検査モードでは**古いブロックを毒で
埋めて隔離に入れる**。これで §1.4 の「`push` をまたいで保持した窓」が、
その窓で次に読んだ位置で止まる — 今は `span.t` のコメントでしか
警告されていない穴が、動的に塞がる。

### 論点 7: stdlib の `Arena` / `FixedBuffer`

`Arena::free(p)` は no-op で、`reset` / `drop` で一括して
`__builtin_heap_free` する。一括解放は既定のヒープを通るので**そのまま
検査対象になる**。個別の `free(p)` の後のアクセスは `reset` まで検出できない。
必要なら後のフェーズで `__builtin_heap_poison(p, len)` (解放せずに毒化して
shadow を `FREED` にする) を足し、`Arena::free` から呼ぶ。

### 論点 8: 起動方法と名前

**`--check` は既に使われている** (契約のプロパティテスト、LLM-LOOP P5)。
案:

```bash
cargo run -q -p interpreter -- --heap-check=poison prog.t
cargo run -q -p compiler -- prog.t --all-backends --heap-check=poison
cargo run -q -p compiler -- prog.t --heap-check=poison -o prog   # 計装入りでビルド
TOY_HEAP_CHECK=reuse ./prog                                       # モードは実行時に切り替え可
cargo run -q -p toy -- test mypkg --heap-check=poison
```

- 計装 (アクセス検査) は**コンパイル時**に入れるので、AOT はビルドフラグが要る。
- 毒埋め・隔離・再利用はランタイム側なので、計装入りのバイナリなら
  `TOY_HEAP_CHECK` で `poison` / `reuse` / `report` を切り替えられる
  (`TOY_PROFILE_MEM` と同じ流儀)。
- interpreter の JIT は検査モードでは使わない (silent fallback)。

### 論点 9: 報告の形

panic と同じ経路・同じ書式に載せる (DEBUG-OBS で 4 レーンの書式は揃っている):

```
Runtime error occurred:
Error at main.t:12:9:
   |
12 |     val x = s.get(0u64)
   |             ^^^^^^^^^^^ heap check: read at offset 0 of a
   |                         32-byte block that was already freed
   |
   = allocated at core/std/collections/vec.t:157:26
   = freed at main.t:10:5
   = backtrace (innermost first):
       main
```

- **解放した位置を出すには `HeapFree` に site が要る** (今は持っていない。
  `HeapAlloc` / `HeapRealloc` は既に `site: Option<SiteId>` を持つ)。
  IR 命令とランタイムの `toy_dispatched_free` の引数を 1 つ増やす。
- `--format=json` では既存の実行時エラーの JSON (`backtrace` 付き) に
  `allocated_at` / `freed_at` を足す。
- 終了コードは panic と同じ。

### 論点 10: コスト

- **切っているとき**: 計装は出さない。ランタイムの free / alloc は
  「モードが有効か」を見る分岐 1 つだけ増える (`prof_enabled` と同じ形で
  1 回だけ環境変数を読んでキャッシュする)。
- **入れているとき**: アクセスごとに呼び出し 1 回と shadow の参照。
  数倍遅くなる見込みで、テスト・CI 用と割り切る。guard 除去 (GUARD_ELISION)
  のような静的な省略は、ブロックの生死が静的に言えないので効かない。

## 5. フェーズ

| フェーズ | 内容 | 検出できるもの |
|---|---|---|
| **H0** | `--heap-check=report`: 両ヒープで二重 free を落とさずに数え、(確保 site, 1 回目の free site, 2 回目の free site) を重複なしで並べる。計装は不要 | 二重 free の棚卸し (§1.3 の 209 か所の出所) |
| **H1** | `poison`: `HeapManager` 側 (tree-walker + IR VM) に shadow と毒埋めを入れ、読み書き口で検査する | interpreter レーンの UAF / 二重 free |
| **H2** (済) | `HeapCheck` 命令と lowering の計装、`toylang_rt` の shadow、`HeapFree` の site | AOT / JIT の UAF。IR VM の報告に位置が付く |
| **H3** (済) | `reuse`: 隔離を有限にし、size class ごとの free list で再利用する。**方針を両ヒープで一字一句同じにする** (size class = 16 バイト単位の切り上げ、LIFO、隔離は FIFO で N バイト) — でないと再利用の結果がレーンで割れる | 「再利用しない」ことへの依存 |
| **H4** (済) | redzone (ブロック間に 16 バイトの `REDZONE`) と `__builtin_heap_poison` | ヒープ内の範囲外、`Arena::free` 後のアクセス |
| **H5** (済) | H0 の一覧を潰す (別名経由の二重 drop の原因を move_check / drop glue 側で直す)。潰し終えたら検査モードの二重 free を既定でエラーにし、example と poc/logsearch を `--heap-check=poison` で回すテストを足す | — |

H0 → H1 の順にするのは、H0 だけで「今どこに二重 drop が残っているか」が
分かり、それ自体が todo になるから。H1 は `HeapManager` 1 か所で済むので、
H2 (IR 命令の追加と 2 つのランタイム) より先に価値が出る。

## 6. 未決事項 (決めてほしいこと)

1. **検査モードでの二重 free の扱い**。推奨は「H0 は report のみ、H5 で
   原因を潰してからエラー」。最初からエラーにする選択肢もある (§4 論点 4)。
2. **フラグ名**。`--heap-check=poison|reuse|report` を推奨 (`--check` は
   プロパティテストが使用中)。`--sanitize=heap` なども候補。
3. **`reuse` の隔離の既定サイズ**。小さいほど再利用が早く起きて依存が
   出やすいが、精密に検出できる範囲は狭くなる。1 MiB 程度から始めて調整する案。
4. **H2 の計装を常にビルドに入れるか**。推奨は「ビルドフラグのときだけ」
   (コストゼロを守る)。常に入れて実行時に切り替える案は、バイナリの
   サイズと速度に常に効く。

## 7. H0 の実装と結果 (2026-09-26)

### 使い方

```bash
cargo run -q -p interpreter -- --heap-check=report prog.t          # IR VM / tree-walker
cargo run -q -p compiler -- prog.t --all-backends --heap-check=report  # 3 レーンの報告を突き合わせる
TOY_HEAP_CHECK=report ./prog                                         # AOT バイナリ単体
```

```
heap check: 6 double frees (1 distinct)
  x6  allocated at core/std/box.t:66:22, freed at core/std/box.t:108:9, freed again at core/std/box.t:108:9
```

- 報告は stderr で、二重 free が無くても `heap check: 0 double frees (0 distinct)` を出す。
  AOT はロード時の初期化関数で `TOY_HEAP_CHECK` を読んで `atexit` を登録する
  (free が 1 回も起きないプログラムでも報告が出るように)。
- `resize` が古いブロックを手放すのも「解放」として記録し、その古い番地の
  free は `moved by a resize, freed again at ...` と出る — `push` をまたいで
  保持した窓 (§1.4) の手掛かりになる形。
- 2 つのヒープ (`HeapManager` と `toylang_rt`) は同じ文言をバイト単位で出し、
  `--all-backends --heap-check=report` は報告が違えば食い違いとして扱う。
- 切っているときのコストは free ごとの分岐 1 つ (archive の所要時間は不変)。

### 実装の要点

- `HeapFree` 命令が `site` を持つ (`HeapAlloc` と同じ `(line << 32) | column`)。
  `toy_dispatched_free(handle, p, site, file)`、`Allocator::free_at`、
  VM ホストの `free_at` がそれを運ぶ。
- 報告の鍵は (確保 site, 1 回目の free site, 2 回目の free site)。
- IR VM が途中で失敗して tree-walker にやり直すときは、プロファイルと同じく
  記録も巻き戻す (`snapshot_profile` / `restore_profile`)。
- `realloc(p, 0)` と「解放済みブロックの resize」は `HeapManager` 側が位置を
  持たないので、両ヒープとも位置なし / 記録なしに揃えた。
- stdin から読んだプログラムの入口名が compiled レーンだけ一時ファイルのパスに
  なっていたので `CompilerOptions::display_name` を足した (`--profile=mem` の
  リーク報告も同じ理由で揃った)。

### 棚卸しの結果

`interpreter/example/*.t` を `--all-backends --heap-check=report` で回した結果と、
`poc/logsearch` の archive (AOT、実ログ 181,519 行):

| 対象 | 二重 free | 確保 → 解放 |
|---|---|---|
| `box_binary_tree.t` | 16 回 (3 レーン一致) | `box.t:66` (`Box::new`) → `box.t:108` (`Box::drop`) |
| `box_linked_list.t` | 6 回 (3 レーン一致) | 同上 |
| `json_config.t` | 6 回 (3 レーン一致) | `string.t:112` (`String` の伸長) → `string.t:508` (`String::drop`) |
| `crypto_sha256.t` | **interpreter だけ 3 回**、JIT / AOT は 0 | `vec.t:157` (`Vec` の伸長) → `vec.t:587` (`Vec::drop`) |
| `try_compound.t` | **JIT / AOT だけ 1 回**、interpreter は 0 | 同上 |
| `poc/logsearch` archive | 1 回 | `string.t:112` → `string.t:508` |
| `poc/logsearch` query | 0 回 | — |

分かったこと:

1. **二重 drop は `Box` の再帰構造と、`String` / `Vec` の値の受け渡しに残っている。**
2. **レーン間で drop の挙動が割れているものが 2 本ある** (`crypto_sha256.t` /
   `try_compound.t`)。どちらかのレーンが余計に drop しているか、片方が
   drop し損ねている。これまでの `--profile=mem` の突き合わせは件数
   (`free_count`) を「要求」で数えるので、二重 free の有無の差は見えていなかった。
3. **経路 (H0b、同日 landing)。** 解放の位置はいつも各型の `Drop` 実装なので、
   2 回目の free の backtrace から `*::drop` と `drop_glue_*` を飛ばした最初の
   フレームの**関数名**を鍵に足した (`  x3  in sum: allocated at ...`)。
   compiled レーンのフレーム記録は行と名前しか持たないので、名前だけを使う。
   3 つの実装 (tree-walker の `call_stack`、IR VM の `backtrace_frames`、
   `toylang_rt` の shadow stack) が同じ規則で選ぶ。`--release` はフレームを
   積まないので名前が出ない。

### tree-walker との比較 (H0b の後)

`--all-backends` は tree-walker を走らせないので、tree-walker 単独の報告と
並べ直した (panic する example など、`--all-backends` が完走しないものは除く):

| example | tree-walker | lowering 系 (IR VM / JIT / AOT) |
|---|---|---|
| `box_linked_list.t` / `box_binary_tree.t` | **0** | 6 / 16 |
| `try_compound.t` | 0 | JIT / AOT で 1 |
| `std_ord_sort.t` | **5** | 0 |
| `soa_column.t` | **1** | 0 |
| `crypto_sha256.t` | 3 | JIT / AOT は 0 (IR VM は 3) |
| `json_config.t` | 6 (2 か所) | 6 (1 か所) |

**2026-09-26 追記**: `Box` の行は同日に解消 — `fn sum(l: &List)` の
`match l { Cons(v, rest) => .. }` で、腕が借用の payload (`Box`) に drop を
登録していた。`&T` 引数と `&self` レシーバの leaf を「所有しない」として扱う。

**二重 drop の原因は両側にある。** `Box` の再帰構造は lowering の drop glue
だけが二重に解放し (tree-walker は各ノード 1 回)、`sort` と `soa Vec` の列は
tree-walker だけが二重に解放する。どれも free が冪等なので出力には出ていない。

未実装節の todo (二重 drop の原因を潰す = H5) はこの一覧から切る。

### 棚卸しの結果を潰した (2026-09-26)

§7 の表の二重 drop は同日に全部直した。example 全体と poc/logsearch の archive で、
tree-walker を含むどのレーンも二重 free 0 回、`--all-backends --heap-check=report`
の食い違いも 0。原因は 7 つで、**どれも free が冪等だったので出力には一度も
出ていなかった**:

| 例 | 実装 | 原因 |
|---|---|---|
| `box_linked_list.t` / `box_binary_tree.t` | lowering 系 | `&List` を match した腕が payload の `Box` を drop |
| `std_ord_sort.t` | tree-walker | `val key: T = p.get(i)` (`Ptr::get` = 要素の別名) を持ち主として drop |
| `soa_column.t` | tree-walker | 列の窓 (`vs.mass`) が見ている元の `SoaVec` を drop |
| `try_compound.t` | lowering 系 | `val v = f()?` の desugar で payload が `v` に移っても `t` の drop flag が残った |
| `json_config.t` / poc/logsearch | 全レーン | `finish(self: Self)` がレシーバを消費していなかった (poc は同名の `&mut self` method があり、型で判定する必要があった) |
| `crypto_sha256.t` | tree-walker + IR VM | `&self` の method の `val b = self.bytes` を持ち主として drop (暗黙の `&self` が宣言されておらず、フィールドの連鎖も別名扱いでなかった) |

あわせて `crypto_sha256.t` の `--profile=mem` の `peak_live_bytes` の食い違い
(720 と 784) も消えた — 早すぎる二重 free がピークを下げていた。

これで H5 の前提 (二重 free を既定でエラーにできる状態) は example と poc の範囲で
満たされた。次は H1 (`poison`)。

## 8. H1 — interpreter レーンの `poison` (2026-09-26)

```bash
cargo run -q -p interpreter -- --heap-check=poison prog.t
```

```
Runtime error occurred:
Error at test.t:5:13:
 5 |     val x = __builtin_ptr_read::<u64>(p, 0u64)
   |             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ panic: heap check: read at offset 0
   |   of a 16-byte block that was already freed (allocated at test.t:2:18, freed at test.t:4:5)
   = backtrace (innermost first):
       main
```

- free したブロックを `0xDB` で埋め、型付きスロット (書いた値そのものの表) も消す。
  解放済みの範囲を「開始番地 → サイズ・確保位置・解放位置」の表に残す。
  resize で移動した古いブロックも同じ (`moved by a resize`)。
- `HeapManager` の読み書き口 (`typed_read` / `typed_write` / `read_bytes_raw` /
  `read_scalar_bytes` / `read_u64` / 各 `write_*` / `copy_memory` / `move_memory` /
  `set_memory`) が範囲を表と突き合わせ、重なれば「保留中の異常」を記録して失敗する。
  今まで `read_bytes_raw` は生死を見ておらず、型付きスロットは解放後も残っていたので、
  解放済みの値が静かに読めていた。
- tree-walker は builtin と `Ptr` / `Span` の intrinsic の後で、IR VM は命令ごとに
  保留中の異常を拾って panic として止める。
- 二重 free は H0 と同じく数えて終了時に報告する (止めない — §6 の決定)。
- **`push` をまたいで保持した `Span` の窓** (`core/std/span.t` が未検査と書く穴) は
  `moved by a resize` として止まる。
- example 全体を `poison` で走らせて、出力・終了コードとも通常の実行と一致
  (誤検出なし)。切っているときのコストは読み書きごとの `Cell<bool>` の読み 1 回。

**残り** (どちらも H2 で解消)

- IR VM の報告には位置 (`Error at`) が無い — IR の命令が位置を持たないため。
- compiled レーン (AOT / JIT) は未対応。

## 9. H2 — compiled レーンの `poison` (2026-09-26)

```bash
cargo run -q -p compiler -- prog.t --heap-check=poison -o prog && ./prog
```

- **計装はビルドフラグ** (§6 の 4 は推奨どおり)。`CompilerOptions.heap_check` が
  立つと lowering (`compiler_lower::lower_program_with`) が `emit()` の 1 か所で、
  生メモリに触る命令 (`PtrRead` / `PtrWrite` / `SimdLoad` / `SimdStore` /
  `MemCopy` / `MemMove` / `MemSet` / `MemEq` / `MemFind` / `MemFindSeq` /
  `StrFromBytes`) の前に `HeapCheck { ptr, offset, len, write, site }` を置く。
  コピーは読み元を先に検査する (interpreter と同じ順)。`LoadRef` / `StoreRef` は
  スタックなので対象外。フラグが無ければ命令は 1 つも出ない (IR で確認するテストあり)。
- AOT / JIT は `HeapCheck` を `toy_heap_check(addr, len, write, prefix, suffix)` の
  呼び出しにする。prefix / suffix は予算違反と同じ枠の静的な前後半
  (`frame_strings`) で、ヒットしたらその間に文を書き、backtrace を出して `exit(1)`。
  poison でなければ即 return。計装ビルドの `main` は冒頭で
  `toy_heap_check_start_mode(mode, quarantine)` を呼ぶので、環境変数は要らない
  (`TOY_HEAP_CHECK=poison` でも入る)。
- `toylang_rt` は poison で free したブロックを `0xDB` で埋め、解放済みの表に
  サイズも持つ。resize で移動した古いブロックは内容を写した**後で**埋める。
- **IR VM も同じ命令を実行する**。interpreter の lowering は poison のとき
  計装し、`HeapCheck` が位置つきで止めるので、H1 の「位置が無い」が解消した。
  報告は AOT とバイト一致する (テストで pin)。
- 文から**バイト数を外した** (`read of 8 bytes at offset 0` → `read at offset 0`)。
  tree-walker の型付きスロットは幅を知らず、レーンで数が割れるため。
  オフセットとブロックの大きさは全レーンで同じ。
- example 全体を計装ビルドで走らせ、出力・終了コードとも通常ビルドと一致
  (`main` が `str` を返す 2 例は終了コードがポインタの下位バイトなので除く)。

**残り**

- `--all-backends --heap-check=poison` は拒否する — poison の停止は `exit` で、
  in-process の JIT レーンが道連れになる。計装ビルドを走らせるか
  `interpreter --heap-check=poison` を使う。
- `toylang_rt` 内部の読み (文字列ヘルパなど) は検査しない。埋めた `0xDB` を読む。

## 10. H3 — `reuse` (2026-09-26)

```bash
cargo run -q -p interpreter -- --heap-check=reuse [--heap-quarantine=BYTES] prog.t
cargo run -q -p interpreter -- --heap-check=reuse --heap-quarantine=0 --test prog.t
cargo run -q -p compiler -- prog.t --all-backends --heap-check=reuse --heap-quarantine=0
cargo run -q -p compiler -- prog.t --heap-check=reuse -o prog && ./prog   # 計装入り
TOY_HEAP_CHECK=reuse TOY_HEAP_QUARANTINE=0 ./any_binary                  # 計装なし
```

- **`reuse` は poison に有限の隔離を足したもの**。free したブロックは毒で埋めて
  隔離 (FIFO) に入り、隔離の合計が上限 (既定 1 MiB、`--heap-quarantine` /
  `TOY_HEAP_QUARANTINE`) を超えたら**古い順に**出る。出たブロックは解放済みの表から
  消え (もう「解放済みへのアクセス」ではない)、size class ごとの free list に入り、
  **最後に入ったものから**次の同 class の確保に渡る。隔離中のブロックへのアクセスは
  poison と同じく止まる。
- **size class は 16 バイト単位の切り上げ**。compiled レーンの bump は元から class 分を
  確保している。interpreter のヒープは要求どおりの長さで切り出していたので、reuse の
  ときは class 分を確保する (でないと大きめの同 class の要求が隣にはみ出す)。
- **方針と順序を両ヒープで同じにした** — free は「毒埋め → 隔離 → 溢れを追い出す」、
  resize は「新ブロックを確保 → 写す → 旧ブロックを隔離」。報告のヘッダに
  `, N blocks reused` が付くので、`--all-backends` はレーンごとの再利用回数も突き合わせる。
- interpreter が自分の都合で取る領域 (IR VM のセル、文字列リテラル) は compiled レーンに
  対応物が無いので、**free list から取らない** (取ると次の確保の行き先がレーンで割れる)。
- `--all-backends --heap-check=reuse` は**計装なし**で全レーンを走らせる (poison と違い
  止まらないので in-process の JIT も走れる)。計装入りのビルドは `compiler --heap-check=reuse`。
- `interpreter --test` も heap check を受けるようにした (以前は黙って無視していた)。
- **結果**: plain で 3 レーン一致する example 155 本すべてが、隔離 0 (最も早く再利用する)
  でも出力・一致とも変わらない。実際に再利用が起きたのは 8 本 (最大 64 ブロック)。
  `poc/logsearch/tests` の 16 ファイルも `interpreter --test` で隔離 0 にして、各ファイルの
  合否は通常実行と同じ (1 ファイルあたり最大 12991 ブロックを再利用。`lsz` と
  `segment_format` は通常実行でも 1 本ずつ落ちるテストがあり、reuse でも同じ本数)。

**残り**

- 再利用した後の古いポインタは検出できない (ABA、§2 の非目的)。止まる代わりに
  他のブロックを壊す — それが症状として出るのを見るのがこのモードの用途。
- `toy test` / `toy run` はまだ `--heap-check` を受けない。

## 11. H4 — redzone と `__builtin_heap_poison` (2026-09-26)

- **redzone**: poison / reuse のとき、両ヒープとも各ブロックに size class 分 +
  16 バイト (`HEAP_REDZONE`) を取る。ブロック末尾より後ろ (class の余りと redzone) から
  **始まる**アクセスは次のブロックに届かず止まる:

  ```
  panic: heap check: read at offset 24 of a 24-byte block, past its end (allocated at core/std/ptr.t:72:22)
  ```

  判定は**アクセスの開始位置だけ**で行う。tree-walker の型付きスロットはアクセス幅を
  知らないので、ブロック内から始まって末尾をまたぐアクセスを数えると、レーンで結果が
  割れる。探索には生きているブロックの表 (開始番地 → サイズ・確保位置) を両ヒープに
  足した (poison / reuse のときだけ保つ)。
- **`__builtin_heap_poison(p, size)`**: 検査モードのとき、範囲を「解放済み」として
  毒で埋め、以後のアクセスを止める。ブロックは生きたままなので、後の本当の free は
  二重 free にならない。検査モードでなければ何もしない。IR 命令 `HeapPoison` →
  `toy_heap_poison` / `HeapManager::poison_range`。**`Arena::free` と
  `Arena::realloc(p, 0)` がこれを呼ぶ**ので、個別 free の後のアクセスが `reset` を
  待たずに止まる (§4 論点 7)。報告の「freed at」は `allocator.t` の中の位置で、
  呼び出し元は backtrace に出る。
- **新しいヒープは新しい番地空間**: `--test` はテストごとにヒープを作り、番地は 1 から
  やり直す。heap check が番地で覚えていた表 (生存・解放済み) を引き継いでいたので、
  前のテストのブロックが今のテストのブロックに見え、reuse で「末尾越え」を誤検出した
  (poc の mount / ontology_* で発覚)。`HeapManager::new` で番地の表だけ捨て、回数は残す。
- example 全体を計装ビルドと interpreter の poison で走らせて誤検出なし。reuse (隔離 0)
  でも 155 本が不変。

**残り**

- ブロック内から始まって末尾をまたぐアクセス (上の理由で見ない)。
- ヒープ外 (stack の配列、`str` リテラル) の範囲外。
- `FixedBuffer::free` は本当に free するので対象外 (既に検査される)。

## 12. H5 — 二重 free をエラーに、example を常時検査 (2026-09-26)

- H0 の一覧は DOUBLE-DROP-LANE-DIVERGENCE で空になった (§7) ので、**poison / reuse では
  二重 free を止める** (§6 の 1 は推奨どおり)。`report` は従来どおり数えるだけ:

  ```
  panic: heap check: free of a 16-byte block that was already freed (allocated at test.t:5:18, freed at test.t:2:5)
  ```

  位置は 2 回目の free、backtrace に呼び出し元が出る。
- 計装は IR 命令 **`HeapCheckFree { ptr, site }`** で、lowering が `HeapFree` の前に置く
  (IR VM と AOT / JIT が free の**前**に止まる)。tree-walker はヒープの free の中で止まる。
  どちらも止めたものは数えないので、停止の後に出る報告は全レーンで `0 double frees`。
  `__builtin_heap_poison` で毒化したブロックは生きているので、その free は二重 free では
  ない。計装なしの AOT (`TOY_HEAP_CHECK=poison`) は止まらず数える。
- **`example_consistency` に poison のスイープを足した** (12 シャード)。各 example を
  tree-walker と IR VM を poison で、AOT を `--heap-check=poison` の計装ビルドで走らせ、
  通常実行と答えが一致すること・止まらないことを見る。解放済みへのアクセス、末尾越え、
  二重 free のどれかが example か stdlib に入れば落ちる。
- **poc/logsearch** は 1 ファイルに数分かかるので `#[ignore]` のテスト
  (`poc_logsearch_tests_run_clean_under_heap_poison`)。各テストの合否が通常実行と同じか
  を見る。`--run-ignored only` で回す。2026-09-26 に全 16 ファイルで通過
  (debug ビルドで約 1 時間)。

## 13. `toy` の `--heap-check` (2026-09-26)

| コマンド | 受けるモード | どう効くか |
|---|---|---|
| `toy build` | poison / reuse | 計装ビルド (`compiler --heap-check=...` と同じ) |
| `toy run --backend vm\|tree` | report / poison / reuse | interpreter の検査モード。報告は stderr |
| `toy run --backend aot` | report / poison / reuse | report は `TOY_HEAP_CHECK`、他は計装ビルド |
| `toy run --backend all` | report / reuse | `compiler --all-backends --heap-check=...` |
| `toy test` | poison / reuse | AOT は計装した driver、VM はワーカーごとに検査モードを開始 |

- `--heap-quarantine=N` は reuse のときだけ。意味の無い組み合わせ (`build` の report、
  `check` / `api` 等、`run --backend jit` / `all` の poison、`test` の report) は理由つきで拒否。
- VM の検査状態はスレッドローカルなので、`toy test` は各ワーカーが最初のテストを
  **準備する前**に開始する (準備が IR VM 向けの lowering を含み、計装は poison が
  既に有効なときだけ入るため)。
- 例: `push` をまたいで持った `Span` の窓を読むテストは、**通常の AOT では黙って通り**、
  `toy test --heap-check=poison` では `moved by a resize` で落ちる (toy のテストで pin)。
