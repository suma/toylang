# RUNTIME_PORT.md — AOT ランタイムを C から Rust へ、最終的に toylang へ

`compiler/runtime/toylang_rt.c` (1121 行) を Rust で書き直し、さらに書ける
部分を toylang stdlib へ移すための設計。`ALLOCATOR_PLAN.md` /
`MEMORY_PROFILING.md` / `FFI_PLAN.md` と同じく「現状調査 → 論点決定 →
Phase 分割 → MVP 刻みで landing」で進める。

## Status snapshot

| Phase | Scope | Status |
|---|---|---|
| **R0** | 出力シンクの抽象化 (print だけ差し替え可能にする) | 未着手 |
| **R1** | `toylang_rt` crate 新設 + C 全機能の移植 + jit.rs ミラー削除 | 未着手 |
| **R2** | extern 宣言の一般化で `toy_io_*` を廃止 (FFI_PLAN P1 に相乗り) | 未着手 |
| **R3** | str / 整数整形 / 集計を `core/std/` (toylang) へ | 未着手 |
| **R4** | f64 整形・allocator・profiler も toylang へ (任意) | 検討のみ |

---

## なぜ設計文書が要るか

**言い直すと、これは「C を Rust にする」話ではなく「同じ意味論の実装を
3 本から減らす」話である。** 移植そのものは機械的だが、減らし方を間違えると
「Rust になったが実装は 3 本のまま」で終わる。

現在、ランタイムの意味論は独立に 3 回実装されている:

| 実装 | 場所 | 行数の目安 |
|---|---|---|
| AOT | `compiler/runtime/toylang_rt.c` | 1121 |
| compiler 側 JIT | `compiler/src/jit.rs` の `toy_*` ミラー | ~880 (L380 以降) |
| interpreter | `heap.rs` + `output` + `evaluation/extern_io.rs` | — |

C を編集するたびに jit.rs を同じ形に編集する運用になっており、
`toylang_rt.c` 自身のコメントもそれを認めている:

> Three implementations is one more than anybody wants, but the alternative —
> linking this translation unit into the compiler binary so the JIT can call
> it — would collide with the print helpers, which the JIT deliberately
> reimplements so it can capture stdout.

この「衝突」は **print の出力先が固定されていること**だけが原因で、そこを
関数ポインタ 1 個に抽象すれば消える (論点 1)。

---

## 現状調査 (2026-08-16 実測)

### 棚卸し: ランタイムが提供する 53 シンボル

| グループ | 数 | 中身 | 性質 |
|---|---|---|---|
| print / println | 24 | 各幅の整数 / bool / str / f64 | libc 出力 + 整形 |
| allocator stack | 3 | `toy_alloc_push` / `_pop` / `_current` | 純粋な状態管理 |
| dispatched alloc | 3 | bump region (never-reuse) + 冪等 free | malloc のみ必要 |
| profiler | 10 | カウンタ / ptr→size 表 / site 表 / layout / text + JSON レポート | 純粋な集計 + `atexit` |
| str / to_string | 14 | `[bytes][NUL][u64 len]` レイアウト操作、数値整形 | 純粋 + malloc |
| io externs | 8 | argv / env / read_file / stdin / time / random | syscall 境界 |

### 実測 1: f64 の出力が既に 3 者で食い違う

```
$ cargo run -q -p compiler -- f64chk.t --all-backends
```

| 式 | interpreter | compiler JIT | AOT |
|---|---|---|---|
| `0.1f64 + 0.2f64` | `0.30000000000000004` | 同左 | **`0.3`** |
| `1234567.75f64` | `1234567.75` | 同左 | **`1.23457e+06`** |
| `123456789.0f64 * 10000000000000.0f64` | `1234567890000000057344.0` | **`1234567890000000000000`** | **`1.23457e+21`** |

C 側が `%g` / `%.1f` (`emit_f64`)、Rust 側が `Display` ベースなのが原因。
`example_consistency` がこのケースを踏んでいないだけで、**現時点の既知の
不具合**である。R1 で実装が 1 本になれば構造的に解消する (テストで押さえる
のではなく、同じコードだから一致する)。

### 実測 2: rustc 単発で staticlib を作り、cranelift の `main` とリンクできる

`#![no_std]` + `extern crate alloc` + malloc backed `GlobalAlloc` の 1 ファイルを:

```
$ rustc --edition 2024 --crate-type staticlib -C opt-level=2 -C panic=abort \
        rt.rs -o libtoyrt.a
$ cc cmain.c libtoyrt.a -Wl,-dead_strip -o demo && ./demo
```

- **cargo 不要・依存クレート 0・rustc 一発**。今の `cc -c` 一発と同じ構造で
  `compiler/build.rs` に収まる。
- `Vec` / `String` / `write!` / f64 整形まで `alloc` だけで使える。
- 追加で必要なのは **`rust_eh_personality` の空定義 1 個**だけ
  (sysroot の `alloc` rlib が unwind 前提でビルドされているため、
  `-C panic=abort` でも landing pad が参照する)。
- macOS で追加の framework リンクは**不要** (no_std なので依存は libc のみ)。
  std 版なら `rustc --print native-static-libs` の追従が要る。

サイズ実測 (arm64 macOS):

| バイナリ | サイズ |
|---|---|
| 現行 C ランタイム + hello (`println(42i64)`) の AOT 出力 | 84,680 B |
| Rust no_std ランタイム + C main、`-Wl,-dead_strip` | 61,128 B |
| 同上、`-C opt-level=z` | 61,192 B |

**サイズ・リンク時間とも実質同等**で、この軸では判断が付かない
(= 判断は保守性の軸で決めてよい)。

### 実測 3: AOT が要求するのは「libc しか要らない archive」だけ

- `driver.rs::link_executable_uncached` は cranelift 出力の `.o` (C ABI の
  `main` を export) と runtime object を `cc` に渡すだけ。C である必然性はない。
- `--target` オプションは無く**ホストのみ**なので、クロスコンパイル配慮は不要。
- `reproducible_build.rs` + link cache が入力バイト列をハッシュするので、
  runtime のビルドは決定的である必要がある (rustc は決定的。パス埋め込みは
  `--remap-path-prefix` で潰す)。

---

## 論点と決定

### 論点 1: JIT の stdout キャプチャをどう共有するか → 出力シンクを 1 個にする

ランタイム側に出力先を 1 箇所持たせ、JIT はプロセス内でそれを差し替える。

```rust
// The JIT swaps this out to capture stdout into its own buffer; the AOT
// binary never touches it and keeps the libc-write default.
type Sink = extern "C" fn(*const u8, usize);
static SINK: AtomicPtr<()> = AtomicPtr::new(default_sink as *mut ());
```

print 1 回あたり atomic load 1 回のコスト。これで **print helper も含めて
AOT と JIT が同一コードを共有できる**ようになり、`compiler/src/jit.rs` の
ミラーは消える。

### 論点 2: `no_std` か `std` か → **`no_std` + `alloc`**

| | no_std + alloc | std |
|---|---|---|
| 依存 | libc のみ。macOS の追加リンクフラグ不要 | `native-static-libs` の追従が要る |
| 使えるもの | `Vec` / `String` / `core::fmt` / f64 整形 | 上に加え `std::fs` / `std::env::args` |
| ビルド | rustc 一発 | 同左 (ただし std 由来でサイズ増) |

`std::env::args` / `std::fs` が欲しいのは io 層だけで、そこは R2 で
toylang + libc extern に移す方針なので、**no_std で足りる**。
R1 で詰まった箇所が出たら、その関数だけ std 版に切り替える判断をする
(crate 全体を std にする必要はない)。

### 論点 3: ビルド方式 → **build.rs から rustc を直接呼ぶ**

| 方式 | 評価 |
|---|---|
| **rustc 直呼び** | 現在の `cc` 直呼びと同型。依存 0 の crate なら cargo 不要。**採用** |
| nested cargo | target-dir 分離 / `CARGO_ENCODED_RUSTFLAGS` 継承 / ロック競合の配線が要る。std や外部 crate が必要になったときの退避先 |
| artifact dependency (`-Z bindeps`) | nightly 限定。不採用 |

crate は**ワークスペースの通常メンバとしても存在する** (`rlib`)。compiler は
それを普通に依存し、JIT はシンボル登録で同じ関数ポインタを使う:

```rust
jit_builder.symbol("toy_print_i64", toylang_rt::toy_print_i64 as *const u8);
```

つまり同じソースが 2 回コンパイルされる (compiler 内の rlib / AOT 用
staticlib) が、**ソースが同一なので意味論は定義上一致する**。

### 論点 4: f64 整形の正本をどちらにするか → **interpreter (Rust `Display`) に統一**

実測 1 の差は R1 で自動的に解消するが、**AOT の出力が変わる**
(`0.3` → `0.30000000000000004`)。これは仕様変更なので `docs/language.md` の
更新と golden の更新を伴う。C の `%g` に Rust 側を寄せる選択肢もあるが、
`%g` は 6 桁で丸めるので情報が落ち、interpreter / JIT / `--test` の出力を
全部変えることになる。**精度を保つ側 (Rust) を正本にする。**

### 論点 5: どこまで toylang に寄せるか → 層で分ける

シンボル単位の可否:

| 区分 | 対象 | 根拠 |
|---|---|---|
| **今すぐ移せる** | `str_concat` / `str_eq` / `to_string_{i64,u64,bool}` と narrow int 版 / bump allocator / profiler の集計部分 | `String` / `Vec<u8>` / `__builtin_ptr_read/write` / `__builtin_heap_alloc` / `__builtin_str_from_bytes` が揃っている |
| **移すのが重い** | f64 整形、profiler の `atexit` レポート出力、site id の扱い | f64 は interpreter とバイト一致が要件なので実質 Ryu/Grisu の移植 |
| **移せない** | `write` / `malloc` / `exit` / `getenv` / `fopen` / `time` / `getpid` | syscall 境界。ただし extern の一般化で toylang から libc を直接呼べば「C ランタイム」は不要 (R2) |

toylang 化の利点は決定的で、**3 バックエンドが同じ `.t` を実行するので二重
実装が原理的に消える** (Rust 化は 3→2 にするだけ、toylang 化は 1 にする)。

代償も正直に置いておく:

- **interpreter の速度低下** — `println` / 文字列補間が tree-walk のループになる。
  ホットな print 系は native に残す判断が要る。
- **ブートストラップ順序** — stdlib が自分自身の `to_string` / 補間に依存しない
  書き方を強制される。
- **安全地帯を失う** — 今のランタイムは言語機能に一切依存しないので、言語の
  バグと無関係に動く。toylang 化すると AOT の未対応構文を stdlib が踏んだ
  瞬間に全体が死ぬ。移す順序は「言語機能を使わない小さいもの」から。
- **デバッガビリティ** — native のスタックトレースが効かなくなる。

判断基準: **「その関数のバグを、言語のバグと切り離して調べたいか」が Yes の
ものは native に残す** (allocator と profiler が該当しやすい)。

---

## 目標アーキテクチャ

```
Layer 2  core/std/*.t          str ops / 整数整形 / io ラッパ / 集計    ← R3, R4
           │ extern fn / __builtin_*
Layer 1  toylang_rt (Rust)     出力シンク / f64 整形 / bump region /    ← R1
           │ libc               allocator stack / profiler
Layer 0  libc                  write / malloc / exit / getenv / fopen
```

- AOT: Layer 1 を staticlib としてリンク。
- compiler 側 JIT: Layer 1 を rlib として同一プロセスに持ち、シンボル登録。
  出力シンクだけ差し替える。
- interpreter: Layer 2 をそのまま評価。Layer 1 相当は既存の `heap.rs` /
  `output` が担う (ここだけは実装が別のまま残る — 詳細は「やらないこと」)。

---

## Phase 分割

### R0: 出力シンクの抽象化

**Scope**: C とその jit.rs ミラーの両方で、print 系の出力先を 1 箇所に集約する
(C なら関数ポインタ、Rust なら論点 1 の `SINK`)。振る舞いは変えない。

**完了条件**: `cargo nextest run` グリーン + `--all-backends` の出力不変。

**なぜ先に単体でやるか**: R1 の diff から「キャプチャ機構の設計」を切り離す
ため。ここを R1 に混ぜると、出力が壊れたときに移植ミスか設計ミスか切り分け
られなくなる。

### R1: `toylang_rt` crate 新設 + C 廃止

**Scope**:

1. ワークスペースに `toylang_rt` crate を追加 (`#![no_std]` + `alloc`、
   依存 0、`crate-type = ["lib"]`)。`toylang_rt.c` の全機能を移植。
2. `compiler/build.rs` を `cc -c` から `rustc --crate-type staticlib` に置換
   (`-C panic=abort` / `--remap-path-prefix` / `-C opt-level=2`)。
   `include_bytes!` → `.rt.a` を書き出して `cc` に渡す形は現状のまま。
3. `compiler` が `toylang_rt` を依存に追加し、`register_runtime_symbols` を
   crate の関数ポインタに向ける。**`compiler/src/jit.rs` の L380 以降の
   ミラー (~880 行) を削除**。
4. `compiler/runtime/toylang_rt.c` を削除。

**完了条件**:

- `cargo nextest run` グリーン (f64 の golden 更新を含む)。
- `--all-backends` で f64 が 3 者一致 (実測 1 の 3 式を consistency テストに追加)。
- `--profile=mem` / `--profile-format=json` が interpreter と一致。
- `reproducible_build.rs` グリーン (rustc 出力の決定性確認)。
- `cc` 依存が残るのはリンクのみ (C コンパイルは消える)。

**リスク**: `rust_eh_personality` / `panic_handler` の扱いを間違えるとリンク
エラー。実測 2 で解決済み。

### R2: extern 宣言の一般化 → `toy_io_*` の廃止

**Scope**: `compiler_lower::program::libm_import_name_for` のホワイトリスト
方式を、`extern fn` の宣言からシンボル名を決める形に一般化する。これは
**FFI_PLAN.md の P1-MVP-A/B/C そのもの**なので、本ドキュメントでは
「ランタイム側の受益者」としてのみ扱い、設計は FFI_PLAN に従う。

**受益**: `core/std/io.t` が libc を直接宣言できるようになり、
`toy_io_read_line` / `_argc` / `_arg` / `_env` / `_read_file` /
`_file_exists` / `_now` / `_random` の 8 シンボルと、その JIT ミラー、
Linux の `/proc/self/cmdline` 手書きパーサ、macOS の `_NSGetArgv` が消える。

**注意**: interpreter は libc を直接呼ばない (出力キャプチャと決定性のため)。
`extern_io` の registry は残り、**宣言だけが一般化される**。

### R3: str / 整数整形 / 集計を toylang へ

**Scope**: 論点 5 の「今すぐ移せる」列を `core/std/` に移す。1 関数 1 コミット、
各コミットに example + `assert_consistent` を付ける。

移す順序 (依存の浅い順):

1. `str_eq` (`==` オーバーロード経由で既に toylang 側の入口がある)
2. `str_concat`
3. `to_string_{u64,i64,bool}` + narrow int 版
4. profiler の集計 (レポート出力は Layer 1 に残す)

**完了条件**: 各移動後に `--all-backends` と `--profile=mem` が不変。
interpreter のベンチ (`println` ループ) が許容範囲内 (基準は移動時に決める)。

**中止条件**: interpreter が目に見えて遅くなる、または AOT の未対応構文に
当たった時点でその関数は Layer 1 に戻す。**戻すのは失敗ではなく設計判断**。

### R4 (任意): f64 整形 / allocator / profiler も toylang へ

自己ホストの象徴的価値は大きいが、interpreter 速度とデバッガビリティを
確実に犠牲にする。R3 の結果を見て判断する。着手するなら f64 整形は
「interpreter とバイト一致」を先にテストで固定してから。

---

## テスト戦略

新しい仕組みは要らない。既存資産で全部押さえられる:

| 何を守るか | 手段 |
|---|---|
| 3 バックエンドの出力一致 | `compiler/tests/consistency.rs` / `example_consistency.rs` |
| メモリ計数の一致 | `--profile=mem` / `--profile-format=json` の 3 者比較 |
| リンクキャッシュが効き続けること | `compiler/tests/reproducible_build.rs` |
| f64 の乖離が再発しないこと | 実測 1 の 3 式を R1 で consistency に追加 (**新規**) |
| ランタイム単体の性質 | `toylang_rt` crate の `#[test]` (C では書けなかった) |

最後の行は移植の副産物として大きい: bump region の冪等 free や profiler の
ハッシュ表を、AOT バイナリを作らずに unit test できるようになる。

## やらないこと

- **interpreter の `heap.rs` / `output` を `toylang_rt` に統合すること** —
  interpreter は自前のアドレス空間 (`Vec<u8>` ベース) を持ち、値も
  `Rc<RefCell<Object>>` で表現している。native ランタイムと統合しても
  型が合わず、薄いアダプタが増えるだけ。実装が 2 本残るのはここまでを許容する。
- **std 版への全面移行** (論点 2)。
- **クロスコンパイル対応** — `--target` が無いので前提から外す。
- **`cc` 依存の完全排除** — リンクは引き続き `cc` に任せる。自前リンカは範囲外。

## 未決事項

1. ~~**f64 の出力仕様変更を受け入れるか** (論点 4)~~ — **2026-08-16 決定: 受け入れる。**
   R1 で `docs/language.md` の print / 補間の節に「Rust の `Display` と同じ、
   ただし整数値の f64 は `.0` を付ける」を明記し、golden の更新を同じ
   コミットに含める。
2. **R4 まで行くか** — R2/R3 で「ランタイムを toylang で書く」意図はかなり
   満たせる。R4 は費用対効果で判断。
3. **`toylang_rt` の置き場所** — ワークスペース直下 (`toylang_rt/`) か
   `compiler/runtime/` を crate 化するか。後者なら `compiler/runtime/toylang_rt.c`
   の履歴と場所が繋がる。
