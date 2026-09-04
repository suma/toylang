# MODULE SYSTEM — stdlib のディレクトリと名前空間

> **状態: P1 着手 (2026-09-04)。P2 / P3 は未着手。**
> 対象: `core/std/**.t` の配置と、`module::name(...)` の解決規則。
> 実装サイト: [`interpreter/src/module_integration.rs`](../interpreter/src/module_integration.rs)
> (発見・統合)、[`frontend/src/module_resolver.rs`](../frontend/src/module_resolver.rs)
> (`import` の探索)、[`frontend/src/type_checker/module_access.rs`](../frontend/src/type_checker/module_access.rs)
> (alias 登録)、[`frontend/src/parser/expr/primary.rs`](../frontend/src/parser/expr/primary.rs)
> (`::` パスの構文)、[`frontend/src/type_checker/context.rs`](../frontend/src/type_checker/context.rs)
> (`(qualifier, name)` の関数表)。
> 仕様の正本: [`../docs/language.md`](../docs/language.md) の「Modules」。
> 状態の正本: [`todo.md`](todo.md)。
> 実測: 2026-09-04

## なぜ

`core/std/*.t` が 46 ファイル・約 11k 行の平積みになっている。ここから
「ディレクトリを掘って `std.text.string` のように名前空間を階層化する」
という案が出たので、**その前提が成り立つかを実測した**。結果、前提は
2 つとも成り立っていなかった。

- **深さが足りないのではない。** フラットな `core/std/*.t` の並びは
  Rust `std` のトップレベルとほぼ同じ形をしていて、`std.collections.vec`
  は既に Rust の最大ネスト深度に並んでいる (後述の「参照系」)。
- **`std.` は解決に関与していない。** qualifier は**リーフのファイル名
  1 個だけ**で、`std` は捨てられている。深くしても現状の実装では
  意味を持たず、むしろ後述の衝突 panic を踏みやすくなる。

したがって本文書は 2 つを分けて扱う。**配置** (P1、言語変更ゼロ) と
**名前空間の意味論** (P2 / P3、パーサ + 型検査 + IR + ランタイム)。

## 現状 (実測)

### 仕組み

1. **発見**: `discover_core_modules` が core ディレクトリを再帰で歩き、
   `.t` ファイルごとに `segments = <ディレクトリ列> + <ファイル名>` を作る
   (`core/std/io.t -> ["std", "io"]`)。順序は segments のソート順。
2. **統合**: 全モジュールが**無条件に auto-load** され、AST がメインの
   pool に deep-copy される。`import` は stdlib に対しては no-op。
3. **alias**: `register_import` が**最後のセグメントだけ**を alias に
   する (`["std","io"] -> io`)。型検査器の関数表のキーは
   `(Option<DefaultSymbol>, name)` で、qualifier はこの 1 シンボル。
4. **型はモジュールに属さない。** `Vec` / `String` / `Option` は常に
   グローバルで、ユーザ定義と衝突したら stdlib 側が `__std_<name>` に
   退避する (`shadowed_stdlib_types`)。

### 実測したズレ 4 件

| # | 書いた形 | 実際 |
|---|---|---|
| 1 | `std::math::abs(-3i64)` | **通るが検査されていない。** パーサが 3 セグメント以上を「最後だけ」に潰し (`primary.rs` の qualified-path 分岐)、bare `abs` の一意フォールバックで当たっている。`zzz::math::abs` でも通る |
| 2 | 同じリーフ名 + 同じ関数名の 2 モジュール | **panic。** `std/a/dup.t` と `std/b/dup.t` が両方 `pub fn f` を持つと型検査を素通りし、`compiler_ir/src/lib.rs:447` で `function_index collision for symbol=... qualifier=...` |
| 3 | `<core>/foo/mod.t` の `foo::f()` | **`[E0003] Struct 'foo' not found`。** auto-load の walker は `mod` をリーフ名として扱うので alias は `mod`。`import` 側の `candidate_module_paths` だけが `mod.t` を知っていて、2 経路が食い違っている (`docs/language.md` の表は `["foo"]` と書いていて誤り) |
| 4 | `import my.helpers as h` の `h::add(...)` | **`[E0003] Struct 'h' not found`。** パーサは `as` を受理するが `visit_import` が alias を捨てている。ドキュメントには載っている |

2 は「今フラットだからリーフ名が一意で踏んでいない」だけで、
**ディレクトリを掘る変更はこの地雷原に入ることを意味する**。

### 副作用として見つかった配置の問題

- **`trait Abs` の宣言が `core/std/i64.t` にあり、`impl Abs for f64` が
  `core/std/f64.t` にある。** 統合順が segments のソート順
  (`f64` < `i64` < `math`) であることに依存していて、`f64.t:14-17` に
  そう書いてある。**ファイル名を変えるとソート順が動く**ので、再編で
  最初に壊れるのがここ。
- **`math::sqrt(x)` と `x.sqrt()` が二重にある** (`f64.t` の
  `impl Sqrt for f64` は `math::sqrt` へ転送するだけ)。
- **`limits.t` の 28 本の自由関数** (`limits::i64_max()`) は本来
  `i64::MAX` の associated const。[`todo.md`](todo.md) の
  **MODULE-CONST** が空いているせいで、名前ではなく機能の欠落。

## 参照系 — C++ と Rust の並び方

**Rust `std` はほぼ 1 階層フラット。** `alloc any array ascii borrow
boxed cell char clone cmp collections convert default env error f32 f64
ffi fmt fs hash hint io iter marker mem net num ops option os panic path
pin prelude process ptr rc result slice str string sync task thread time
vec` が全部トップレベル。ネストは実質 4 箇所
(`collections::{hash_map, btree_map}` / `sync::{atomic, mpsc}` /
`os::{unix, windows}` / `io::prelude`、ほかに `f64::consts`)。
`std::text::string` のようなカテゴリ層は**無い**。

**C++ はディレクトリがゼロ。** `<vector> <string> <optional> <memory>
<span> <limits> <random> <chrono> <filesystem> <bit> <compare>
<charconv>` … 全部フラットなヘッダ名で、namespace も `std::` フラット +
`chrono` / `filesystem` / `ranges` / `pmr` の少数。C++20 の named module
は最終的に **`import std;` の 1 本**。MSVC が実験した `std.core` /
`std.io` / `std.memory` / `std.regex` / `std.threading` という分割は
標準化されずに畳まれた — **どの機能がどのサブモジュールか利用者が
当てられない**ため。

この最後の点が、カテゴリ層を入れない直接の理由になる。`parse.t` を
`text/` に置くか `num/` に置くかは書いた人にしか分からない。

### 突き合わせ

| 我々 | Rust std | C++ |
|---|---|---|
| `option.t` / `result.t` | `option` / `result` | `<optional>` / `<expected>` |
| `vec.t` `dict.t` `set.t` `deque.t` `priority_queue.t` | `collections` | `<vector> <unordered_map> <set> <deque> <queue>` |
| `string.t` / `str.t` | `string` / `str` | `<string>` / `<string_view>` |
| `box.t` / `ptr.t` / `span.t` | `boxed` / `ptr` / `slice` | `<memory>` / `<span>` |
| `allocator.t` + `alloc.t` | `alloc` (`AllocError` も) | `<memory_resource>` |
| `clone default convert hash iter` | 同名がそのまま存在 | `<concepts>` `<type_traits>` |
| **`ord.t`** | **`cmp`** | `<compare>` |
| **`display.t`** | **`fmt`** | `<format>` |
| `limits.t` | (`i64::MAX` の associated const に畳んだ) | `<limits>` |
| `math.t` `bits.t` `random.t` `time.t` | `f64` の inherent method / `time` | `<cmath> <bit> <random> <chrono>` |
| `io.t fs.t path.t net.t char.t` | `io fs path net char` | `<iostream> <filesystem> <cctype>` |
| `parse.t` | `str::parse` (`FromStr`) | `<charconv>` |
| `json.t hex.t base64.t log.t poll.t` | (std に無い) | (std に無い) |

**名前は 9 割一致している。** 外れているのは `ord` / `display` と、
我々固有の分割 (`str_ops.t`) と、`i64.t` / `f64.t` の中身だけ。

## 設計判断

### D1. カテゴリ層を入れない (フラットを維持する)

`text/` `num/` `encoding/` `mem/` のような層は両参照に存在しないので
入れない。ネストは「1 つの概念に複数のメンバがある」ときだけ:

- `collections/` — 実装済み。Rust `std::collections` に対応
- 将来 `os/` — `poll.t` / `net.t` のプラットフォーム分岐が育ったら
  (Rust `std::os`)
- 将来 `sync/` — 並行が入ったら

### D2. モジュール名は「輸出する型」または「概念」

- 型を輸出するモジュールは**型の小文字名**: `vec` / `string` / `box` /
  `ptr` / `span` / `dict`。既にそうなっている
- trait 群を輸出するモジュールは**概念の名前**: `cmp` (Ord) / `fmt`
  (Display) / `clone` / `default` / `convert` / `hash` / `iter`。
  「型の名前」を使わない (`ord.t` / `display.t` が外れている)
- **receiver が取れるものは型のメソッド、取れないものだけ自由関数**。
  `x.sqrt()` は trait、`gcd(a, b)` / `clamp(x, lo, hi)` は `math`

### D3. 型を名前空間に属させない (やらない)

`std::collections::Vec` のように型をモジュールに属させるのは**やらない**。
stdlib が輸出しているものの大半は型で、名前空間化すると prelude の
概念・`__std_` shadow 機構・全バックエンドの名前解決に波及する。一方
auto-load が全部読む設計なので得られるものは名前衝突回避だけで、費用対
効果が合わない。**モジュールは自由関数の名前空間**と割り切る。

### D4. qualifier はパス、解決は suffix 一致

関数表のキーを 1 シンボルからフルパス (`Vec<DefaultSymbol>`) に変え、
呼び出し側のパスは**末尾一致**で解決する。

- `math::abs` → `std.math.abs` の suffix として一意 → 従来どおり通る
  (後方互換)
- `std::math::abs` → 完全一致で通る (現状 #1 の素通りが検査される)
- 候補が複数 → `ambiguous module path` の型エラー (現状 #2 の panic が
  ここで消える)

Rust の絶対パス強制ではなく Go の import 末尾規則寄りにするのは、
既存コードを壊さずに深いパスを**書けるようにする**ため。フラット
(D1) と組み合わせると、日常的には従来と同じ 1 セグメントで書ける。

### D5. 暗黙に入る集合は「stdlib 全部」(C++20 方式)

Rust は `std::prelude` を明示して残りに `use` を要求する。C++20 は
`import std;` で全部入る。我々の auto-load-everything は**後者と同型**
なので、これを維持して `docs/language.md` に「stdlib は全部暗黙」と
明記する。Rust 方式 (prelude を切って残りは `import` 必須) に寄せると
既存の全ユーザコードが壊れるので採らない。

## フェーズ

### P1 — 配置と名前 (言語変更ゼロ)

リネーム 4 件 + 統合 2 件。移動 (ディレクトリ間) は無い。

| 変更 | 理由 |
|---|---|
| `ord.t` → `cmp.t` | D2 (Rust `std::cmp`) |
| `display.t` → `fmt.t` | D2 (Rust `std::fmt`) |
| `str_ops.t` → `str.t` に統合 | Rust は 6 trait 全部 `str` にある |
| `i64.t` + `f64.t` → `num.t` に統合 | trait 宣言と impl が別ファイルで統合順に依存している問題を消す。`trait Abs` / `trait Sqrt` を宣言と同じファイルに置く |

結果:

```
core/std/
  option.t result.t box.t ptr.t span.t column.t          # 型 = モジュール名
  string.t str.t char.t
  cmp.t clone.t default.t convert.t hash.t iter.t fmt.t drop.t   # trait 群
  num.t math.t bits.t checked.t limits.t random.t
  alloc.t allocator.t
  io.t fs.t path.t net.t poll.t time.t log.t
  json.t hex.t base64.t codec.t parse.t
  collections/  vec.t soa_vec.t dict.t set.t deque.t priority_queue.t
```

**注意点**:

- **`FULL_AST_CACHE_SCHEMA_VERSION` を上げる。** 統合順は segments の
  ソート順なので、リネームで intern 順が動く。`frontend/src/cache.rs` の
  v9 / v29 / v33 / v37 と同じ理由 (上げ忘れると無関係な
  `val a: u64 = 5u64` が stdlib 由来の型エラーで落ちる)
- パス文字列をアサートしているテストは 3 箇所
  (`diagnostics_location_tests.rs` / `diagnostics_json_tests.rs` /
  `runtime_observability_tests.rs`) で、いずれも今回動かすファイルを
  指していない
- `str_ops` / `ord` / `display` / `i64` / `f64` を qualifier として
  呼んでいるコードは無い (これらのファイルに自由関数が無いため)

### P2 — qualifier をパスにする (D4)

- 関数表のキー (型検査 `context.functions`、IR `function_index`、
  ランタイム `function_qualified`) を `Vec<DefaultSymbol>` に
- suffix 一致解決 + 曖昧なら型エラー。**現状 #2 の panic が消える**
- AOT のシンボルマングル (`toy_<qualifier>__<name>`) をパス全体に

### P3 — 構文と `import` の穴埋め

- パーサの `a::b::c(...)`: 現状 2 セグメントだけ `AssociatedFunctionCall`、
  3 以上は黙って最後だけ残す。パス表現に一本化して型検査で
  「先頭がモジュールパス / 末尾が型・関数」に振り分ける (現状 #1)
- auto-load の walker に `mod.t` / `<name>/<name>.t` を入れて
  `import` 側の `candidate_module_paths` と揃える (現状 #3)
- `import a.b as h` の alias を `register_import` に通す (現状 #4)

## 非目標

- **型の名前空間化** (D3)
- **prelude を絞る** (D5) — auto-load-everything を維持
- **可視性の強化** — `pub` は現状ほぼ素通り (`check_function_access` は
  同一モジュール判定を持たない)。名前空間の話とは独立なので別項目
- **循環 import の実運用** — `ModuleResolver` に検出はあるが stdlib は
  全部 auto-load なので出番が無い
