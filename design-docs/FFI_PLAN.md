# FFI_PLAN.md — 外部関数インターフェース (FFI) と動的ロード設計

toylang から任意の C ABI 関数を呼べるようにする (静的 FFI)、および実行時に
共有ライブラリをロードして呼べるようにする (動的ロード) ための設計ドキュメント。
ALLOCATOR_PLAN.md / DYN_TRAIT_AOT.md と同じく「論点決定 → Phase 分割 →
MVP 刻みで landing」のスタイルで進める。

## Status snapshot

| Phase | Scope | Status |
|---|---|---|
| **P1-MVP-A** | 構文拡張 + AOT (`-l` リンク + import) | 未着手 |
| **P1-MVP-B** | interpreter (libloading + trampoline) | 未着手 |
| **P1-MVP-C** | compiler-side JIT (JITModule symbol 解決) | 未着手 |
| **P2** | 動的ロード (`dlopen` / `dlsym` builtin + indirect call) | 未着手 |
| **P3** | toylang モジュールの実行時ロード | 検討のみ (本ドキュメント範囲外) |

## 背景 — 現状の足場

調査結果 (2026-06-09) の要約。詳細な場所は各ファイル参照。

- **`extern fn` 宣言**: `frontend/src/parser/program_parser.rs:76-150` で
  `extern fn name(params) -> ret` をパース、`Function { is_extern: true }` に
  落ちる。body は placeholder で backend は walk しない。
- **シンボル解決はハードコード**: `compiler_lower/src/program.rs:51-74` の
  `libm_import_name_for()` が `__extern_sin_f64 → "sin"` のような固定対応表。
  ユーザが任意ライブラリの関数を宣言する経路はない。
- **AOT の import 機構は既にある**: `Linkage::Import` 宣言 + 通常の
  `InstKind::Call` で libc/libm シンボルを呼んでいる
  (`compiler/src/codegen/imports.rs`)。呼ぶ側は内部関数と外部関数を区別しない。
- **リンクは `cc` 直叩き**: `compiler/src/driver.rs::link_executable_uncached`
  が toylang object + 同梱 runtime object を `cc` でリンク。
  `compute_link_hash` (driver.rs:70) が link cache のキー。
- **indirect call IR は実装済み**: `InstKind::CallIndirectFn`
  (dyn Trait A5-P2 で導入) が closure env を持たない関数ポインタ呼び出しを
  サポート。動的ロードの dlsym 結果呼び出しにそのまま流用できる。
- **interpreter の extern dispatch**: `interpreter/src/evaluation/extern_math.rs`
  の registry (`HashMap<&'static str, ExternFn>`) に Rust closure を静的登録。
  実 dlopen はしていない。

### C# (MSIL / P/Invoke) からの設計参照

| 観点 | C#/.NET | toylang への示唆 |
|---|---|---|
| 宣言 | `[DllImport("lib", EntryPoint="sym")] static extern ...` | lib 名 / シンボル名 / ABI を**宣言時メタデータ**で持つ |
| MSIL | 呼ぶ側は通常 `call`、リンク情報は ImplMap テーブルに分離 | toylang も `Call` + import 宣言の分離が既にできている |
| 実行 | IL Stub でマーシャリング + 遅延 dlopen/dlsym | toylang は GC なし・8-byte scalar なのでマーシャリング層が**ほぼ不要** |
| 動的 | `NativeLibrary.Load/GetExport` + `calli` | `__builtin_dlopen/dlsym` + `CallIndirectFn` が対応物 |

C# FFI の複雑さの大半 (GC pinning、string 変換、レイアウトマーシャリング) は
toylang の値モデルでは発生しない。最小の宣言構文とリンク情報の伝播経路が本体。

## 設計論点と決定

### 論点 1: 構文

**決定 (案): per-function の `from` / `as` 節** — 既存 `extern fn` の後置拡張。

```rust
# ライブラリ指定のみ (シンボル名 = 関数名)
extern fn add(a: i64, b: i64) -> i64 from "mylib"

# シンボル名を別名にする (C# の EntryPoint 相当)
extern fn my_sin(x: f64) -> f64 from "m" as "sin"

# from なし: 従来どおり (builtin registry / libm 対応表 / runtime helper)
extern fn sin(x: f64) -> f64
```

- `from` は新キーワード不要なら contextual keyword として導入
  (識別子 `from` のパース衝突を確認する。衝突するなら `link("mylib")` 形式に
  フォールバック)。`as` は既存キーワードの再利用。
- `"mylib"` は **`-lmylib` に渡す名前** (`lib` prefix / 拡張子なし)。
  パス直接指定 (`from "./libfoo.dylib"`) は P2 の dlopen 側で扱い、
  静的 FFI では受けない (リンカ挙動がプラットフォーム依存になるため)。
- extern ブロック形式 (`extern "C" from "m" { fn ... fn ... }`) は関数が
  増えたら欲しくなるが、P1 では見送り (parser 変更を最小にする)。
  後方互換に足せる。
- ABI 文字列 (`extern "C"`) も P1 では省略 — C ABI 一択なので。将来
  ABI を増やすときに optional に足す。

**AST 変更**: `Function` に `extern_link: Option<ExternLink>` を追加。

```rust
pub struct ExternLink {
    pub lib: DefaultSymbol,            // "mylib"
    pub symbol: Option<DefaultSymbol>, // `as "sym"` (None = fn 名)
}
```

### 論点 2: interpreter の扱い (3-backend 一貫性)

**決定 (案): interpreter も libloading で実 dlopen する。** ただし P1-MVP-A
時点では「AOT のみ対応、interpreter / JIT は明確なエラー」で先に landing し、
MVP-B で interpreter を追いつかせる。

理由: `assert_consistent` (interpreter / AOT / JIT 3-way 一致) がプロジェクトの
柱なので、FFI だけ一貫性テストから外れるのは避けたい。

**技術的な核心 — 実行時シグネチャでの呼び出し方法**: dlsym で得た生ポインタを
「実行時にしか分からないシグネチャ」で呼ぶには、整数系 (i64/u64/bool/ptr) と
f64 でレジスタクラスが違うため、単純 transmute では済まない。選択肢:

- **(a) trampoline 列挙** (推奨): 引数を「整数系 or f64」の 2 クラスに分類し、
  引数 ≤ 4 個 × 戻り値 2 クラスの組合せを match で列挙して transmute する。
  2^4 × 2 × (arity 0..=4) ≒ 60 arm 程度、マクロで生成すれば小さい。
  純 Rust で依存なし。引数 5 個以上はエラー (P1 制約として明記)。
- (b) `libffi` crate: 任意シグネチャ対応だが C 依存が増える。
  制約が実害になったら移行。

interpreter-side JIT (`interpreter/src/jit/`) は既存方針どおり silent fallback
(eligibility reject) でよい。

### 論点 3: 型の制約 (P1)

**決定 (案): scalar のみ。**

- 引数・戻り値とも `i64` / `u64` / `f64` / `bool` / `ptr` / `usize` に限定。
  narrow int (u8〜i32) は P1 では不可 (C ABI の整数昇格を考えなくて済む)。
- `str` を直接渡すのは不可。`__builtin_str_to_ptr(s)` / `s.as_ptr()` で
  `ptr` にして渡す (NUL 終端は STR-PTR-LEN layout で保証済み)。
- struct by-value / 配列 / 可変長引数 (printf) は**対象外**と明記。
  struct はレジスタ分割規則 (SysV / AAPCS64) が複雑で、cranelift 側も
  追加作業が要る。必要になったら別 Phase。
- 戻り値 `()` (void) は許可。
- 型チェッカは extern + `from` の関数に対して上記制約を検査し、
  違反はコンパイルエラーにする (backend に来る前に弾く)。

### 論点 4: 安全性の建付け

**決定 (案): `unsafe` キーワードは導入しない。** toylang は既に
`__builtin_ptr_read/write` 等の生ポインタ操作をキーワードなしで許しており、
FFI だけ unsafe を要求しても一貫しない。`from` 付き extern fn の呼び出しは
暗黙に unsafe (型シグネチャの正しさはユーザ責任) とドキュメントに明記する。
言語全体の unsafe 設計をやるならそれは独立した検討項目。

### 論点 5: リンク情報の伝播経路

```
AST Function.extern_link
  → compiler_ir::Module.link_libs: Vec<String>   (dedup 済み、C# の ImplMap 相当)
  → compiler_ir::Function 側はシンボル名だけ持つ (Linkage::Import の名前に使用)
  → driver.rs link_executable(..., link_libs) が `-l<lib>` を cc に追加
  → compute_link_hash に link_libs を必ず含める (link cache のキー汚染防止)
```

- `compute_link_hash` への追加を忘れると、`-l` 有無の違うバイナリが
  キャッシュ衝突する。**LINK_CACHE_VERSION も bump する。**
- ライブラリ探索パスは P1 では環境任せ (`LIBRARY_PATH` / `-L` は
  `TOYLANG_LINK_PATHS` env var で渡せるようにする程度)。

## Phase 分割

### Phase 1: 静的 FFI の一般化

**P1-MVP-A — 構文 + AOT** (最小で end-to-end を通す):
1. parser: `from "lib"` / `as "sym"` 節 (`program_parser.rs` の extern 分岐に追加)
2. AST: `ExternLink` / type checker: 論点 3 の型制約検査
3. compiler_lower: `extern_link` ありの fn は `libm_import_name_for` を経由せず
   `symbol or fn名` で `Linkage::Import` 宣言、`Module.link_libs` に lib を積む
4. driver: `-l` フラグ + `TOYLANG_LINK_PATHS` → `-L`、link hash 更新
5. interpreter / JIT: `from` 付き extern fn の呼び出しは
   「FFI is not yet supported on this backend」の明確なエラー
6. テスト: `compiler/tests/` にテスト用 C ライブラリ fixture
   (`tests/fixtures/ffi/libtoytest.c`、`add(i64,i64)->i64` /
   `scale(f64,f64)->f64` 等) を test 時に `cc -shared` でビルドして検証

**P1-MVP-B — interpreter 対応** (3-way 一致へ):
1. `libloading` crate 追加、`extern_link` ありの fn を初回呼び出し時に
   `Library::new("libX.dylib/so")` + `get(symbol)` で解決してキャッシュ
   (C# の遅延バインドと同じ)
2. trampoline 列挙 (論点 2 (a)) で `Vec<Value>` → ネイティブ呼び出し
3. ライブラリ探索: `TOYLANG_LINK_PATHS` を interpreter も参照して
   AOT と同じ解決順にする
4. `assert_consistent` 系テストに FFI fixture を追加 (interpreter / AOT 2-way、
   JIT は MVP-C まで fallback)

**P1-MVP-C — compiler-side JIT**:
1. `JITModule` の symbol 解決: `JITBuilder::symbol_lookup_fn` で
   事前に dlopen したハンドルから dlsym するクロージャを登録
   (process-global 解決でも可だが、`-l` 相当の明示ロードが対称)
2. `compiler/tests/jit_smoke.rs` に FFI ケース追加 → 3-way 一致

### Phase 2: 動的ロード (`NativeLibrary` 相当)

P1 完了後に詳細設計。骨子のみ:

- `__builtin_dlopen(path: str) -> ptr` / `__builtin_dlsym(h: ptr, name: str) -> ptr`
  / `__builtin_dlclose(h: ptr)` builtin (AOT は libc にそのまま import、
  interpreter は libloading、JIT は helper)
- 得た `ptr` を関数として呼ぶ仕組みが本体。既存 closure 型 `fn (T) -> R` は
  env-based ABI (`[fn_ptr, caps...]` heap block) なので**生ポインタと ABI が
  違う**。選択肢: (a) `__builtin_fn_from_ptr<F>(p: ptr) -> F` で env なし
  wrapper を合成、(b) ネイティブ関数ポインタ型 `extern fn (T) -> R` を別型で
  導入。→ P2 設計時に決定 (`CallIndirectFn` がそのまま使えるのは確認済み)
- コールバック (toylang 関数を C に渡す) は P2 でも対象外。capture なし
  closure の fn_ptr 取り出しから検討する後続項目

### Phase 3: toylang モジュールの実行時ロード (将来)

C# の `Assembly.Load` 相当。IR VM (BACKEND.md Phase 4) が「実行時に
compiler_ir::Module をロードして走らせる」基盤になるため、tree-walker 撤去後に
検討するのが筋。本ドキュメントでは範囲外。

## リスクと既知の注意点

- **macOS 固有**: Apple ld の reloc バグ (DYN_TRAIT_AOT.md 参照) のような
  toolchain 起因の罠がありうる。fixture dylib は code signing 不要な
  ad-hoc ローカルビルドなので SIP は問題にならない見込み。
- **テストの移植性**: fixture C ライブラリのビルドは `cc -shared` で
  macOS (.dylib) / Linux (.so) を分岐。driver.rs が既に `cc` 前提なので
  追加依存はない。
- **可変長引数は永続的に対象外** (cranelift も variadic 呼び出しを
  サポートしない)。printf が欲しい場合は固定 arity の wrapper を C 側に書く。
- **link cache**: `compute_link_hash` 更新漏れが静かな誤キャッシュになる。
  MVP-A のレビュー観点として明記。
- **シンボル衝突**: ユーザ lib のシンボルが runtime (`toy_*`) や libc と
  衝突した場合はリンカエラーに任せる (P1 では検出しない)。

## 関連ドキュメント

- バックエンド構成: [`design-docs/BACKEND.md`](BACKEND.md)
- ビルトイン関数: [`design-docs/BUILTIN_ARCHITECTURE.md`](BUILTIN_ARCHITECTURE.md)
- dyn Trait (CallIndirectFn の出自): [`design-docs/DYN_TRAIT_AOT.md`](DYN_TRAIT_AOT.md)
- 進捗・タスク: [`design-docs/todo.md`](todo.md)
