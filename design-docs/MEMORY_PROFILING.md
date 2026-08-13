# MEMORY_PROFILING.md — メモリ使用量と断片化を計測できる言語にする

toylang で書いたプログラムの**メモリ確保を、実行後にレポートとして出し、
機械可読な数値として取り出せる**ようにするための設計。
`ALLOCATOR_PLAN.md` / `FFI_PLAN.md` / `LLM_FEEDBACK_LOOP.md` と同じく
「現状調査 → 論点決定 → Phase 分割 → MVP 刻みで landing」で進める。

## Status snapshot

| Phase | Scope | Status |
|---|---|---|
| **M0** | 用語の固定 + interpreter 側のイベント計数 | ✅ 2026-08-13 |
| **M1** | AOT 側の同一計数 + `--profile=mem` テキスト出力 | ✅ 2026-08-13 |
| **M2** | サイト帰属 + リーク検出 | ✅ 2026-08-13 |
| **M3** | `trait Alloc` の layout 報告 (ここで初めて断片化が出る) | ✅ 2026-08-13 |
| **M4** | JSON 出力 + 契約 / `test` ブロックとの連携 | ✅ 2026-08-13 |

---

## なぜ設計文書が要るか

「メモリ使用量を記録する」は一見単純だが、このリポジトリでは
**同じ意味論を 4 実行系が独立に実装している**ため、計測層を間違えると
**バックエンドごとに違う数字を出す計測器**ができる。数値は診断と同じで、
**間違っているなら無い方がまし**である。

下の実測 1 が示すとおり、その差は既に観測可能な形で存在する。

---

## 現状調査 (2026-08-13 実測)

### 実測 1: interpreter と AOT で allocator の挙動が既に違う

```rust
fn main() -> u64 {
    val a: ptr = __builtin_heap_alloc(64u64)
    __builtin_heap_free(a)
    val b: ptr = __builtin_heap_alloc(64u64)
    if __builtin_ptr_eq(a, b) { 1u64 } else { 0u64 }
}
```

| バックエンド | 結果 |
|---|---|
| interpreter | **0** — アドレスを再利用しない |
| AOT | **1** — 同じブロックが返る (libc malloc) |

`HeapManager` は bump allocator である。

```rust
pub fn alloc(&mut self, size: usize) -> usize {
    let addr = self.next_addr;
    self.memory.resize(self.memory.len() + size, 0);
    self.next_addr += size;          // 単調増加。free しても戻らない
    addr
}
pub fn free(&mut self, addr: usize) -> bool {
    self.allocations.remove(&addr).is_some()   // 領域は返らない
}
```

**帰結**: interpreter には測るべき断片化が存在しない。ここから
「断片化率」を出せば、それは実装の副産物であって
プログラムの性質ではない。**アドレスや領域レイアウトを土台にした
メトリクスは、この時点で全バックエンド共通にはできない。**

### 実測 2: AOT の allocator handle は現状無視されている

```c
void *toy_dispatched_alloc(uint64_t handle, uint64_t size) {
    (void)handle;                 /* ← 現状すべて libc へ */
    return malloc((size_t)size);
}
```

IR は handle を運んでいるが、ネイティブ側は使っていない。
`Arena` / `FixedBuffer` の policy は **toylang 空間** (`core/std/allocator.t`)
に実装されているのでプログラムの意味は保たれるが、
`with allocator = arena { __builtin_heap_alloc(...) }` のように
**raw builtin を直接呼ぶ経路は wrapper の追跡を通らない**
(この点は `CLAUDE.md` に既出)。

**帰結**: 「現在の allocator ごとの集計」を AOT でも出すには、
少なくとも handle を計測に使う必要がある (実行の dispatch を変える
必要はない)。

### 実測 3: 同名のメトリクスが allocator ごとに違う意味を持っている

| allocator | `free` の実装 | 既存メトリクスの実際の意味 |
|---|---|---|
| `Arena` | **no-op** (bulk free のみ) | `bytes_used` は reset までの **live** であり、同時に累積でもある |
| `FixedBuffer` | `used_bytes` を減算 | `used()` は純粋に **live** |

`CLAUDE.md` は `Arena::bytes_used` を「累積追跡バイト数」と書いているが、
`free` が no-op なので reset までは live と一致する。両者が乖離するのは
**arena に free を実装した瞬間**であり、そのとき既存の記述は静かに嘘になる。

**帰結**: 用語 (cumulative / live / peak) を先に固定する。これは M0 の
主目的であって、付随作業ではない。

### 参考: 既にある足場

- **`RuntimeState { heap, registry, active }`** — tree-walker / IR VM /
  interpreter JIT が共有する。**interpreter 系の単一チョークポイント**
- **`toy_dispatched_alloc/free/realloc(handle, ...)`** — AOT 側の同じ位置
- **`with allocator = ...`** — レキシカルスコープ。push/pop は全バックエンドで
  実装済みなので、**帰属の単位としてそのまま使える**
- **`trait Alloc`** — allocator が toylang 空間にある。ユーザ定義 allocator も
  同じ仕組みに乗せられる
- **P4 `test` ブロック / P5 契約** — 数値を assert する足場が既にある
- **`--all-backends` (D6)** — バックエンド間の数値一致をコマンド 1 本で検査できる

---

## 論点と決定

### 論点 1: どの層で計測するか

| 層 | 見えるもの | 却下理由 |
|---|---|---|
| A. 言語 (stdlib wrapper) | wrapper 経由の確保のみ | 生の `__builtin_heap_alloc` を取りこぼす |
| **B. ランタイム** | **全確保** | — |
| C. ホスト (malloc interposition) | 本物の断片化 | バックエンド依存・移植不能。測っているのは**ホストの** allocator であってプログラムではない |

**決定: B を正とする。** `HeapManager` と `toy_dispatched_*` の 2 箇所に
同一のカウンタを置き、**一致を `--all-backends` で強制する**。

C は言語機能としては採らない。ホストの RSS が知りたい場面はあるので、
レポートの末尾に参考値として 1 行出すのは可 (M4)。ただし
**厳密系メトリクスと同じ表に混ぜない** — 精度が違うものを並べると、
読み手は両方を同じ信頼度で読む。

### 論点 2: 断片化をどこの責務にするか

**決定: 断片化は allocator の性質であり、`trait Alloc` に属させる。**

プロファイラが断片化を「計算」できるのは、領域レイアウトを知っている
場合だけである。実測 1 のとおり、それを知っているのは allocator であって
ランタイムではない。したがって allocator 自身に報告させる。

```rust
pub trait Alloc {
    fn alloc(&mut self, size: u64) -> ptr
    fn free(&mut self, p: ptr)
    fn realloc(&mut self, p: ptr, new_size: u64) -> ptr

    # 領域を管理しない allocator は既定実装で「報告しない」を返す。
    # 実装しないことと「断片化ゼロ」は別物なので、区別できる形にする。
    fn layout_report(&self) -> AllocLayout { AllocLayout::opaque() }
}

pub struct AllocLayout {
    known: bool,          # false なら以下は無意味
    managed_bytes: u64,   # 管理下の総バイト
    live_bytes: u64,      # うち使用中
    free_blocks: u64,     # 空きブロック数
    largest_free: u64,    # 最大連続空き
}
```

外部断片化は allocator の報告から導出する:

```
external_fragmentation = 1 - largest_free / (managed_bytes - live_bytes)
```

この形にすると **ユーザが自分で書いた allocator も同じレポートに乗る**。
ランタイムに閉じ込めればそれは単なるツールだが、`trait` に置けば
**言語機能**になる。toylang が「メモリをプロファイルできる言語」を
名乗れるかどうかはここで決まる。

### 論点 3: メトリクスを 1 種類にするか 2 種類にするか

**決定: 厳密系と allocator 依存系を分けて表示する。**

**厳密系** — 確保イベントの列だけから決まる。全バックエンドで一致し、
再現可能:

- 確保回数 / 解放回数 / 総確保バイト (cumulative)
- **live bytes の推移**と **peak live bytes**
- サイズヒストグラム (2 冪ビン)
- 寿命分布 (確保シーケンス番号の差)
- 解放されなかった確保 = リーク (サイト別)

**allocator 依存系** — `layout_report()` を実装した allocator のみ:

- 外部断片化 / 内部断片化 (要求サイズ vs 実確保サイズ)
- 空きブロック分布

分けない場合、実測 1 のせいで interpreter が嘘の断片化を出す。
**「測れない」と書くことは、測れないものを測ったふりをするより価値がある。**

### 論点 4: 帰属をどう取るか

**決定: lowering 時に確保サイトへ静的 ID を振る。**

`with allocator` スコープだけでは「どのコードが確保したか」が出ない。
かといって P6-1 で interpreter に入った `call_stack` を使うと、
**AOT にはコールスタックが無い**ので粒度がバックエンドで変わる。

```
alloc(handle, size)  →  alloc(handle, size, site_id)
```

`site_id` は lowering が確保サイトごとに採番する `u32`。
`Terminator::Panic` が interned symbol を運んでいるのと同じ手口で、
**プログラムに `site_id → ソース位置` の表を 1 つ持たせる**。
これで 4 バックエンドが同一の帰属を出す。

### 論点 5: 再現性をどう担保するか

**決定: レポートは同一プログラムに対して bit-identical にする。**

diff できないレポートはテストにもレビューにも使えない。具体的な規則:

1. **時間軸に wall clock を使わない。** 確保シーケンス番号を時間軸にする
2. **生アドレスをレポートに出さない。** 実測 1 のとおりアドレスは
   バックエンドで違う。必要なら別フラグ (`--profile-raw`) に隔離する
3. **レポート生成にハッシュ順の反復を入れない。** これは
   `INCREMENTAL_COMPILATION.md` に記録した「link cache が全ミスしていた」
   原因そのもので、同じ轍を踏まない

### 論点 6: オーバーヘッドをどう扱うか

**決定: 既定で常時 ON にはしない。`--profile=mem` を明示したときだけ。**

`HeapManager::alloc` はホットパスである。M0 では
**tree-walker にだけ入れて既存ベンチとの差を測ってから**他へ広げる
(P6-3 で u64 underflow trap を入れたときと同じ手順)。
差が測定誤差に収まるなら常時 ON も検討するが、**先に測る**。

---

## Phase 詳細

### M0 — 用語の固定 + interpreter 側の計数 (✅ 2026-08-13)

**用語は `MemoryStats` (`interpreter/src/heap.rs`) がコード側の正本**:

| 項目 | 定義 |
|---|---|
| `alloc_count` / `free_count` / `realloc_count` | 各**要求**の回数 |
| `cumulative_bytes` | 取得した総バイト。減らない。realloc は**増分のみ**寄与 |
| `live_bytes` | 取得済みかつ未解放のバイト |
| `peak_live_bytes` | `live_bytes` の最大値 |
| `peak_at_request` | peak を更新した時点の要求番号 (wall clock は使わない) |

**すべて「プログラムが要求した内容」で定義し、allocator が何をしたかでは
定義しない。** これが M1 以降で 4 バックエンドの数値を一致させる前提になる。
具体的には **realloc を 1 件の resize として数える** — この実装はブロックを
移動して処理するが、in-place で伸ばす allocator も同じ数値を報告しなければ
ならないので、`realloc` が内部で使う alloc / free は計数しない
(`alloc_uncounted` / `free_uncounted`)。

- `HeapManager` に計数を追加。**free list は入れていない** (bump allocator
  のまま — 変えると意味論が変わる)
- 実測 3 の用語ずれを修正: `Arena::bytes_used` は **live** (free が no-op
  なので reset までは累積と一致するだけ)、`FixedBuffer::used()` は真の live。
  `CLAUDE.md` も修正
- 実測 1 は **`assert_consistent` では pin できない** (一致しないことが
  要点なので)。`interpreter_heap_does_not_reuse_addresses_but_the_aot_heap_does`
  として、interpreter=0 / AOT=1 を直接 assert する形で記録した
  (`u64_addition_still_wraps` と同じ「現状の記録であって是認ではない」扱い)

**オーバーヘッド実測** (20 万回の alloc/write/read/free ループ、release):

| | 実測 |
|---|---|
| 計数なし | 270.2 / 267.6 ms |
| 計数あり | 252.4 / 253.2 ms |

計数ありの方が速く出ているが、これはコードレイアウトによる誤差。
**言えるのは「回帰なし」まで**で、高速化したとは言わない。
interpreter の確保は HashMap 挿入と Vec 拡張が支配的なので、
整数インクリメント数個は雑音以下に沈む。

**残**: 数値を外から観測する手段はまだ Rust 側の `HeapManager::stats()`
だけ。CLI 出力は M1、builtin は M4。

### M1 — AOT 側の同一計数 + テキスト出力 (✅ 2026-08-13)

```bash
interpreter --profile=mem prog.t                 # 実行後に stderr へ
compiler prog.t --all-backends --profile=mem     # 3 バックエンドの数値を突き合わせ
TOY_PROFILE_MEM=1 ./compiled_binary              # AOT バイナリ単体
```

```
memory profile
  alloc_count       2
  free_count        2
  realloc_count     1
  cumulative_bytes  224
  live_bytes        0
  peak_live_bytes   160
  peak_at_request   2
```

**計数の実装は 3 つある。** `interpreter/src/heap.rs` (tree-walker /
IR VM / interpreter JIT が共有)、`compiler/runtime/toylang_rt.c` (AOT)、
`compiler/src/jit.rs` (compiler 側 JIT)。1 つに寄せるには C の翻訳単位を
compiler バイナリにリンクする必要があるが、**print ヘルパと衝突する** —
JIT 側はまさに stdout をキャプチャするために別実装を持っている。
なので**一致は構造ではなくテストで強制する** (D7 と同じ方針)。
算術だけは `MemoryStats::record_obtained` / `record_released` を
公開して共有し、数える**場所**だけが実装ごとに違う形にした。

- 3 実装のレポートは **byte-identical** (`{name:<16}` と C の固定幅が一致)。
  数値を humanize しない — "38.2 KB" は 1 バイト違う 2 つの run を
  同じに見せる
- AOT のレポートは `atexit` で stderr へ。プロファイル無効時は
  サイズ追跡表も作らないので、**通常実行の挙動は変わらない**
- `realloc` の旧サイズは libc が返さないので、C 側に
  open-addressing のポインタ→サイズ表を書いた (自身の記憶域は
  malloc 直呼びなので計数に現れない)
- interpreter 側は run ごとの合算を thread-local に持つ。
  「その run のヒープ」を読む形にしなかったのは、**tree-walker と JIT が
  別々の `HeapManager` を持つ**ため、どちらが走ったかを推測することに
  なるから

**受け入れ基準の結果**: 生 heap builtin と `Vec` の成長 (realloc を
4 回踏む) で **4 バックエンドの厳密系メトリクスが完全一致**。
`allocation_totals_agree_for_*` で pin。

#### 初回の実プログラムで見つかった差異

`String::from_str("...")` を含むと **interpreter だけ 1 確保 (20 bytes) 多い**。
interpreter は `str` をヒープに実体化するが、コンパイル系は `.rodata` を
指すため (STR-PTR-LEN)。**表現の差であって計測のバグではない** — 同じ
プログラムから String を除くと完全に一致する。実測 1 と同じ扱いで
`string_literals_allocate_on_the_interpreter_but_not_when_compiled`
に記録した。

> この差は M1 の道具が**最初の実プログラムで**見つけたものである。
> 一致を目視で確認する運用だったら気づかなかった。

### M2 — サイト帰属 + リーク検出 (✅ 2026-08-13)

**設計を 1 点変えた。** 論点 4 は「lowering で静的サイト ID を採番し、
`site_id → ソース位置` の表をプログラムに持たせる」としていたが、
実装時に**位置そのものを ID にする**方が良いと分かった。

```
site = (line << 32) | column
```

- **表が要らない。** AOT バイナリに文字列テーブルを埋め込んで
  ランタイムに登録する、という M2 の一番面倒な部分が丸ごと消える
- **一致が構造的に保証される。** 全バックエンドが同じ `location_pool`
  を読むので、ID を突き合わせる仕組み自体が不要
- 単独で走る AOT バイナリも `3:18` と位置を出せる

`HeapAlloc` だけが site を運ぶ。`realloc` は**ブロックが既に持っている
site を維持**する (同じ論理的な確保なので、リークは「最後に伸ばした場所」
ではなく「どこから来たか」を指すべき)。`free` は解放する確保に帰属する。

```
$ interpreter --profile=mem leak.t
memory profile
  alloc_count       2
  ...
leaks (1 sites, 1 allocations, 32 bytes)
  3:18  1 allocations  32 bytes
```

interpreter / JIT / AOT で **byte-identical**。`--all-backends --profile=mem`
は総計とリーク節の両方を突き合わせる。

**帰属の粒度は「確保サイト」であって「呼び出しパス」ではない。**
`keep()` を 2 箇所から呼べば、両方の確保が `keep` 内の 1 サイトに
集約される。`allocations_from_one_site_reached_by_several_callers_aggregate_together`
で記録した — 呼び出しパス別にするなら、それは意図的な変更として
後のフェーズで行う。

**ABI 変更**: `toy_dispatched_alloc(handle, size)` →
`(handle, size, site)`。定数レジスタ 1 本を足す方が、site を設定する
別の呼び出しを挟むより安い (codegen は実行時にプロファイルが有効か
知らないので、常に何かを渡すしかない)。

### M3 — `trait Alloc::layout_report` (断片化) (✅ 2026-08-13)

**着手時の調査で前提が 1 つ崩れた。** 設計は `FixedBuffer` / `Arena` に
`layout_report` を実装する想定だったが、**どちらも領域を管理していない**。
両者とも個々の確保を default allocator に委譲して記帳するだけで、
`FixedBuffer` の `cap` はバッファではなく quota である。
したがって**両者に報告すべき断片化は存在しない**。

そこで:

- `Global` / `Arena` / `FixedBuffer` は既定実装のまま **「報告しない」**
  (`known == false`)。**断片化 0 とは書かない** — 「領域を持たないので
  報告するものがない」と「断片化していない」は別の主張である
- 実際に領域を管理する **`SlotRegion`** を stdlib に追加した。
  これが無いと `layout_report` は実装者ゼロの飾りになる

#### `SlotRegion` — なぜスロット方式か

当初は「1 ブロックから offset で切り出す free-list allocator」を書こうと
したが、**toylang にポインタ演算の builtin が無い** (`__builtin_ptr_read/write`
は base + offset を読み書きするだけで、内部ポインタを値として作れない)。
1 つの確保の内部を指すポインタを配れないので、この方式は現状書けない。

> 追記 (2026-08-13): `__builtin_ptr_offset(base, offset) -> ptr` を追加した
> (interior pointer を作る、AOT では単なる `iadd`)。interpreter の
> `HeapManager` も interior アドレスを `resolve_block` で逆引きして
> 読み書きできるようになったので、offset ベースの region allocator は
> もう書ける。SlotRegion のスロット方式はそのまま (動くものは変えない)。

代わりに **等サイズのスロット集合**を管理する。各スロットは独立した確保
なので既存 builtin で書け、**連続スロットの run** という形で本物の断片化が
起きる。

```
$ # 6 スロット中 1 つおきに解放 → 空き 48 バイト、最大 run は 16 バイト
leaks / layout:
  managed 96, live 48, free_blocks 3, largest_free 16
  external_fragmentation 666 permille
  48 バイトの確保は失敗する
```

**この「失敗する」が本質**。断片化の数値が飾りでないことは、
空きバイトが足りているのに確保が通らないことで示される
(`fragmentation_is_reported_and_actually_bites` で pin)。

外部断片化は permille (千分率) で返す。float ではないので**値が厳密で、
バックエンド間で完全に一致**する。

#### 自動レポートへの取り込み (✅ 2026-08-13)

allocator は toylang 空間のオブジェクトで、ランタイム側のプロファイラから
は届かない。**Drop フック方式**で解決した — 「ランタイム → toylang の
コールバック」は AOT/JIT では構造的に書けないため、代わりに allocator
自身が生存中に登録する:

- `__builtin_record_allocator_layout(name, managed, live, free_blocks, largest)`
  を追加 (既存の `__builtin_heap_alloc` と同じ builtin → スレッドローカル /
  C ランタイムのパターン)。
- `SlotRegion` に `impl Drop` を足し、破棄時に `layout_report()` の
  フィールドをこの builtin で登録 → `main` 終了時点の最終レイアウトが
  自動で入る。
- レポート (テキスト / JSON) に `allocator layouts` 節を追加。全バックエンド
  byte-identical (`--all-backends --profile=mem` が layout 節も突き合わせる)。

**実装で見つかった既存バグ 1 件** (Drop の free を書いた初回で露見):
`__builtin_heap_free` を `&mut self` フィールド (`self.ptrs`) に対して
Drop 内で呼ぶと、`&mut self` メソッド呼び出しが先行しない限り AOT が
segfault / IR VM が panic する。`SlotRegion` の Drop はレイアウト登録のみ
に留め、slots の解放は見送った (従来どおりプロセス終了に依存)。

### M4 — JSON + 契約 / テスト連携

#### `--profile-format=json` (✅ 2026-08-13)

```bash
interpreter --profile=mem --profile-format=json prog.t
compiler prog.t --all-backends --profile=mem --profile-format=json
TOY_PROFILE_MEM=json ./compiled_binary
```

```json
{
  "memory_profile": {
    "alloc_count": 2,
    ...
    "peak_at_request": 1
  },
  "leaks": [
    { "line": 2, "column": 18, "allocations": 1, "bytes": 32 }
  ]
}
```

- **`leaks` は常に出す**。何も漏れていなければ `[]`。テキスト版が節ごと
  省略するのは stderr を読む人間には正しいが、消費側には
  「漏れていない」と「リーク報告より前の生成器」の区別がつかなくなる
- **JSON も手書き**。テキスト版と同じ理由で、`toylang_rt.c` が同じバイト列を
  `fprintf` で出す必要があり、**両側が書き下されていて初めて突き合わせられる**。
  レポートに文字列値は 1 つも無いのでエスケープの食い違いは起きない
- `--all-backends` の**子プロセスは常にテキスト**を出す。境界を渡るのは
  数値であって、形は driver が自分の stderr について決めること。
  子に JSON を要求してもパーサが 1 つ増えるだけになる。C 側の JSON は
  `the_aot_json_report_is_byte_identical_to_the_shared_one` が
  **バイト単位で直接** pin する
- `--profile-format` を `--profile=mem` 無しで渡すとエラー。黙って無視すると
  来ない JSON を待つことになる

**受け入れ基準の結果**: `the_json_report_is_identical_between_runs` /
`the_aot_json_report_is_byte_identical_to_the_shared_one` が、同一プログラムの
2 回の run と 2 実装の間でレポートが**バイト単位で一致**することを pin。
期待値はテスト内に**書き下してある** — 導出すると、両方が同時に壊れたときに
気づけない。

#### 数値を読む builtin (✅ 2026-08-13)

`MemoryStats` の 6 フィールドを**レポートと同じ名前で**公開する
(`__builtin_live_bytes` / `_alloc_count` / `_free_count` /
`_realloc_count` / `_cumulative_bytes` / `_peak_live_bytes`)。同じ数値に
2 つの語彙があると、それは間違える箇所になる。`peak_at_request` だけは
出さない — レポートが「いつ」を再現可能に表すための軸であって、
プログラムが意見を持つ量ではない。

```rust
fn parse(s: str) -> Ast ensures __builtin_live_bytes() <= 4096u64 { ... }

test "tidy leaks nothing" {
    val before: u64 = __builtin_live_bytes()
    val r: u64 = tidy(64u64)
    assert_eq(__builtin_live_bytes(), before)
}
```

`AST` 側は `BuiltinFunction::MemStat(MemStat)` の**ペイロード付き 1 変種**。
6 変種にすると各バックエンドに 6 本の match arm が生えるが、この形なら
バックエンドごとに 1 本で済み、追加はテーブルの 1 行になる。

##### 論点: 計測していないランタイムに数値を聞いたらどうなるか

M1 は「プロファイル無効時はサイズ追跡表も作らない」と決めた。素直に
実装すると、**AOT で `__builtin_live_bytes()` が常に 0 を返す**。
`ensures __builtin_live_bytes() <= 4096u64` は通るが、何も検査していない。
**0 を返す方が、答えないより悪い。**

そこで `InstKind::MemStatEnable` を足し、**プログラムがカウンタを読む
ときだけ** `main` の先頭に置く。計数と出力は別で、これは計数だけを
入れる (レポートは `TOY_PROFILE_MEM` があるときだけ)。

- **モジュール全体を走査して決める**。カウンタはグローバルなので、
  どこか 1 箇所で読むなら最初から数えていなければならない
- **lowering 中のフラグではなく走査**で導出するので古くならない
- トップレベル `const` はコンパイル時評価なので `main` より前に確保は
  起きず、「先頭」が本当に先頭になる
- 読まないプログラムには何も足さない (M1 の「無効時は以前と同じだけ
  確保する」を保つ)

`the_compiled_binary_counts_without_being_asked_to_profile` が
`TOY_PROFILE_MEM` 無しで pin する。この経路が壊れると 0 が返るので、
「バックエンドが不一致」ではなく「AOT が 0 と答えた」と読める形で
書いてある。

##### 実装中に見つかった 2 つの数え間違い

どちらも **builtin を入れて初めて観測可能になった**もので、`--profile=mem`
だけの頃は表に出なかった。

1. **run をまたいで累積していた。** インタプリタは 1 プロセスで何度も
   run できる (テストスイート、`--test` の複数ブロック、`--check` の試行)。
   カウンタはプロセス生存期間で積み上がっていたので、2 回目の run の
   `__builtin_alloc_count()` は 1 回目の分を含んでいた。コンパイル済み
   バイナリはプロセス = run なので**必ず食い違う**。
   `execute_entry` 冒頭で reset するようにした
2. **放棄した実行の分を数えていた。** run は JIT → IR VM → tree-walker の
   順に試し、途中で失敗したエンジンは既に確保を済ませている。その分は
   どの run のものでもない (答えを出したのは完走したエンジン)。
   fallback 時に `snapshot_profile` / `restore_profile` で巻き戻す。
   これが無いと、`ensures __builtin_live_bytes() <= N` の違反が
   **漏らしていない方の関数**に帰属した (IR VM で漏らした 4096 バイトが
   live のまま tree-walker が再実行し、先に通りかかった健全な関数の
   契約が落ちる)。`an_abandoned_execution_attempt_is_not_counted_against_the_next_one`
   が、違反メッセージが漏らした関数を名指しすることで pin する

> 2 番目は M4 の道具が見つけたものである。`--profile=mem` は run の
> 最後に読むので、run の途中で二重に数えていても最終値には出ない。

##### 副産物: AST キャッシュのスキーマ版を上げる必要があった

`BuiltinFunctionSymbols::new` が 6 個の名前を追加で intern するので、
以後の全シンボル ID がずれる。`.toycache` は **`DefaultSymbol` を
そのまま保存**していて、キーはソースのハッシュだけなので、古い
エントリは別の意味に化ける。実際 `val a: u64 = 5u64` が stdlib 由来の
型エラー 3 件で落ちた。`FULL_AST_CACHE_SCHEMA_VERSION` を 3 に上げ、
**「interner に事前投入する名前の集合」も bump 対象**であることを
その doc コメントに書いた (今まで書かれていたのは構造体レイアウトだけ)。

**受け入れ基準**: レポートが run 間で bit-identical (論点 5)。

---

## 非目標

- **ホスト allocator の内部計測** (jemalloc stats、malloc interposition) —
  測っているのはホストであってプログラムではない。論点 1 参照
- **GC** — toylang は GC を持たない。本文書はあくまで計測であり、
  管理方式の変更ではない
- **常時 ON のプロファイル** — 論点 6。まず測ってから判断する
- **アロケーション削減の自動提案** — レポートは事実を出すところまで。
  どう直すかは書き手の判断

---

## テスト戦略

1. **バックエンド一致** — `compiler/tests/consistency.rs` に
   「同一プログラムの厳密系メトリクスが 4 バックエンドで一致する」を追加。
   これが本機能の中心的な性質
2. **再現性** — 同一プログラムを 2 回プロファイルしてレポートが
   bit-identical。`reproducible_build.rs` と同じ発想で、
   **プロセスを分けて**比較する (ハッシュシードはプロセスごとに変わる)
3. **既知のプログラムでの正しさ** — 確保回数とバイト数が手計算で
   分かる小さなプログラムを固定し、数値を直接 assert する
4. **オーバーヘッド** — `--profile=mem` 無しの実行が回帰していないこと

---

## メンテナンス

`CLAUDE.md` の Allocator 節と `docs/language.md` は、M0 で用語を固定した
時点で同時に直すこと。実測 3 のとおり、**同名で意味の違うメトリクスが
既に存在している**状態から始めるので、ここを放置すると
用語の食い違いがそのまま数値の食い違いになる。
