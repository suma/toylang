# compiler

toylang の AOT コンパイラ。`.t` ソースから cranelift 経由で native 実行
ファイルを生成する。

## 使い方

```bash
cargo run -p compiler -- input.t -o output
./output; echo $?
```

`main` の戻り値 (`u64` / `i64`) がプロセス終了コードになる。POSIX シェル
は下位 8 bit に切り詰めるので 256 以上は wrap する。

### CLI フラグ

| フラグ | 意味 |
|---|---|
| `<file>` | 入力ソース (必須) |
| `-o <path>` | 出力パス |
| `--emit <kind>` | `exe`(default) / `obj` / `ir` / `clif` |
| `--release` | DbC (`requires` / `ensures`) を skip (= `INTERPRETER_CONTRACTS=off`) |
| `-v` / `--verbose` | 進行ログを stderr に出す |
| `--core-modules <DIR>` | core modules ディレクトリを上書き |

### 環境変数

| 変数 | 意味 |
|---|---|
| `TOYLANG_CORE_MODULES` | core modules ディレクトリ (空文字で opt-out) |
| `TOY_CACHE_DIR` | インクリメンタル cache のルート (default `.toycache/`) |
| `TOY_CACHE_DISABLE=<non-empty>` | cache の load / save を両方 skip |
| `TOYLANG_CRANELIFT_OPT_LEVEL` | `speed`(default) / `none` / `speed_and_size` |

### core modules (auto-load)

起動時に `core/` 配下を再帰 integrate して `math::sin(x)` 等を `import`
行なしで呼べるようにする。解決順:

1. `--core-modules` フラグ
2. `TOYLANG_CORE_MODULES` 環境変数
3. 実行ファイル相対探索 (`<exe>/core/` → `<exe>/../share/toylang/core/`
   → `<exe>/../../core/`)

dev tree から `target/debug/compiler` を直接実行する場合は最後の
fallback (`<repo>/core/`) で見つかる。

### インクリメンタルコンパイル cache

`interpreter::check_typing_with_core_modules` 経由で frontend を呼ぶため、
Phase 4 で入った Full AST cache がそのまま効く。各 core module の `File` +
module-local interner を `<cache_dir>/<hash_prefix>/<source_hash>.full` に
保存し、二回目以降は parse を skip。schema mismatch / corrupt / missing
は silent fall-back。設計詳細は
[`design-docs/INCREMENTAL_COMPILATION.md`](../design-docs/INCREMENTAL_COMPILATION.md)。

## サポート状況

interpreter と (`dict` と `dyn Trait` JIT 経路を除き) ほぼ同等の言語機能を
カバー。型 / 式 / 文 / struct / tuple / enum / match / trait / generics /
配列 / `extern fn` / DbC / allocator / `str` / closure などすべて動作する。
言語仕様は [`docs/language.md`](../docs/language.md)、Phase 別の実装履歴は
[`design-docs/todo.md`](../design-docs/todo.md) を参照。

明確なエラーで reject される主な制約:

- struct / tuple binding 全体の再代入は不可 (leaf field への代入は可)
- compound を返す関数 / メソッド呼び出しを式位置で直接使えない場合がある —
  `val` で受ければ動く
- dict はインタープリタ専用 (3-way テストから除外)
- `dyn Trait` の interpreter JIT は silent fallback (AOT は MVP-A〜F 対応)
- `f64` の `%` (mod) — cranelift に native fmod がない
- bool / Unit との `as` キャスト、narrow int ↔ f64 cast
- 文字列 / 複雑な式の `const` 初期化 (リテラル / 単純算術 fold のみ)

## 設計

パイプラインは **AST → IR → Cranelift IR → object bytes** の 3 段。中間
IR を挟むことで AST に直接バックエンドの都合を持ち込まず、定数伝搬・
インライン化・devirtualize などを IR レイヤで完結できる構造。

| ファイル | 役割 |
|---|---|
| `src/main.rs` | CLI |
| `src/lib.rs` | `compile_file()` / `resolve_core_modules_dir()` |
| `src/options.rs` | `CompilerOptions` / `EmitKind` |
| `src/ir.rs` | 中間 IR 定義 (`Module` / `Function` / `Type` / `Linkage`) |
| `src/lower/` | AST → IR (24 ファイル分割、`program.rs` が top-level) |
| `src/codegen.rs` | IR → Cranelift IR + `.o` 出力 |
| `src/driver.rs` | `cc` 経由のリンク |
| `build.rs` | `runtime/toylang_rt.c` を pre-build して `.o` を同梱 |

frontend / type checker は `compiler_core::CompilerSession` と
`interpreter::check_typing_with_core_modules` を再利用するため、
interpreter / JIT と同じフロントエンド検査を経由する。

`str` runtime layout (NUL 終端 + 末尾の `u64 len` を指す opaque pointer
表現) や Phase 別の codegen 詳細は `src/lower/*.rs` と
`design-docs/todo.md` の各 Phase エントリを参照。

### `--emit=ir` 出力例

```
local function toy_fib(@l0: u64) -> u64 {
  bb0:
    %v0: u64 = load @l0
    %v1: u64 = const 1u64
    %v2: bool = le %v0, %v1
    br %v2, bb2, bb3
  ...
}

export function main() -> u64 {
  bb0:
    %v0: u64 = const 8u64
    %v1: u64 = call fn#0(%v0)
    ret %v1
}
```

## テスト

| ファイル | 件数 | 内容 |
|---|---:|---|
| `tests/e2e.rs` | 34 | source → compile → spawn → exit code 比較 |
| `tests/e2e_batched.rs` | 12 | 小サンプルを 1 spawn にまとめる省 spawn 版 |
| `tests/consistency.rs` | 178 | interpreter / AOT / JIT の 3 経路一致 |
| `tests/jit_smoke.rs` | 15 | compiler 側 cranelift JIT の in-process テスト |

```bash
cargo nextest run -p compiler                      # 並列実行 (推奨)
cargo nextest run -p compiler -E 'test(name)'      # 単一テスト
COMPILER_E2E=skip cargo nextest run -p compiler    # cc が無い環境で skip
```

テスト時は `.config/nextest.toml` で `TOYLANG_CRANELIFT_OPT_LEVEL=none` を
注入して codegen を約 3 倍高速化している。CLI から直接呼ぶ本番経路は
`speed` がデフォルト。

並列 wall-clock の支配項は macOS の Mach-O コード署名検証 (新規バイナリ
ごとに 150〜300ms)。`build.rs` で `toylang_rt.c` を 1 度だけ pre-build
して各テストの `cc` 起動コストを削っている。
