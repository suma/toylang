# compiler

AOT コンパイラ。toylang のソースから native の実行可能バイナリを生成する。

## ステータス: MVP

現状サポートしている機能はとても限定的で、interpreter / JIT で実行できるプログラムのうち **数値計算と制御フローのみ** からなるものが対象。

サポート:

- 型: `i64`, `u64`, `f64`, `bool`, `Unit`、scalar フィールドのみの struct、scalar 要素のみの tuple
- 式: リテラル、算術 (`+ - * / %`)、比較 (`== != < <= > >=`)、短絡論理 (`&& ||`)、ビット演算 (`& | ^ ~ << >>`)、unary (`- ! ~`)
- 文: `val` / `var`（型注釈あり）、代入、`if`/`elif`/`else`、`while`、`for ... in start..end`、`break` / `continue`、`return`
- 同一プログラム内の関数呼び出し（`main` のみ C ABI でエクスポート、それ以外は `toy_<name>` プレフィックス）
- **`as` キャスト**: `i64 ↔ u64`（identity）、`{i64, u64} ↔ f64`（cranelift の `fcvt_*_sat` で truncating saturation）。bool との cast や Unit との cast は不可
- **`f64`**: 算術（`+ - * /`）、比較、unary `-`。`%` (mod) は cranelift に native fmod が無いため reject。print 用ヘルパー (`toy_print_f64` / `toy_println_f64`) は `%g` か `%.1f` で出力
- **`panic("literal")` / `assert(cond, "literal")`**: メッセージは文字列リテラル限定。`puts` + `exit(1)` で実装
- **`print(x)` / `println(x)`**: `i64` / `u64` / `f64` / `bool` / 文字列リテラル / **struct binding / tuple binding** を受け取る。compound 値は interpreter と同じ表記 (`Point { x: 3, y: 4 }`、`(3, 4)`、1-tuple は `(x,)`) に展開され、struct のフィールドはアルファベット順にソートされる（interpreter `Object::to_display_string` と一致）。ネストした struct も再帰的に整形。実体は `compiler/runtime/toylang_rt.c` の `toy_print_*` / `toy_println_*` ヘルパー経由で stdout に出力（driver が `cc` で同時にコンパイル＋リンク）。**制約**: struct / tuple は識別子（`val` / `var` 由来の binding）のみ。struct リテラル直接や struct-returning call の結果を直接 print することはできず、いったん `val` で受ける必要がある
- **struct**: `struct Name { field: Type, ... }` 宣言、`Name { field: value, ... }` リテラル、`obj.field` 読み取り、`obj.field = value` 書き込み、**関数引数として struct 値を渡せる**、**関数戻り値として struct 値を返せる**（codegen が境界で per-field cranelift param / multi-return に展開）。**制約**: フィールドは scalar のみ、struct binding 全体の再代入は不可、`struct.struct.field` のような chain 構造は未対応、struct-returning call を式位置で使えない（必ず `val` で受ける）
- **tuple**: `(a, b, c)` リテラル、`t.0` / `t.1` 要素アクセス、`t.N = value` 要素書き込み、`val (a, b) = (x, y)` 分解（パーサが desugar）、**関数引数 / 戻り値として tuple 値を渡せる**（codegen が境界で per-element cranelift param / multi-return に展開）、`val (a, b) = make_pair()` 形式の tuple-returning call も動作。**制約**: scalar 要素のみ、ネストした tuple は未対応、tuple-returning call は式位置で使えない（必ず `val` で受ける）
- **トップレベル `const`**: `const NAME: Type = expr` を定義、起動時の値（リテラル / 既存 const 参照 / 単純な算術 fold）として利用可能。複雑な初期化式や文字列定数は未対応
- **DbC (`requires` / `ensures`)**: 関数の事前 / 事後条件を実行時にチェック。違反時は `panic: requires violation` / `panic: ensures violation` で停止。`ensures` 内の `result` は scalar 戻り値にのみ bind される（struct 戻り値は最初の field を bind）。`--release` フラグで全 contract チェックを skip
- **ネストした struct**: struct のフィールドが別の struct でも可。`a.b.c` のような chain access、`outer.inner.x = v` のような chain assignment、`Outer { inner: Inner { x: 1 } }` の入れ子リテラルがすべて動作。関数引数として渡せば codegen が leaf scalar まで再帰展開

**注意**: `panic` / `print` / `println` は stdout に出力する（interpreter / JIT は `panic` を stderr に出力する点が既知の挙動差）

未対応（明確なエラーで reject される）:

- 任意の文字列値（リテラルのみ可）、配列、dict
- enum、match、trait
- allocator
- generics（型パラメータを持つ関数 / struct）
- struct / tuple リテラルや関数戻り値を直接 `print` / `println` する（`val` で受ければ可）
- struct / tuple binding 全体の再代入
- ネストした tuple 要素 (`((a, b), c)`)、tuple-of-struct, struct-of-tuple
- 文字列 const、複雑な const 初期化式（リテラル / 単純算術 fold のみ）
- `ensures` 内で struct field を個別に参照する
- ネストしたフィールド全体への代入（`p.inner = Inner { ... }` 不可、leaf scalar への代入は可）
- `f64` の `%` (mod) — cranelift に native fmod が無い
- bool との `as` キャスト、Unit との `as` キャスト
- heap / pointer builtins

## 使い方

```bash
# 実行ファイルを生成
cargo run -p compiler -- input.t -o output

# DbC チェックを無効化（`INTERPRETER_CONTRACTS=off` 相当）
cargo run -p compiler -- input.t --release -o output

# .o だけ生成
cargo run -p compiler -- input.t --emit=obj -o input.o

# Cranelift IR をテキストで dump
cargo run -p compiler -- input.t --emit=ir -o input.clif

# 進行ログ
cargo run -p compiler -- input.t -v -o output
```

`main` の戻り値（`u64` または `i64`）はプロセス終了コードになる。POSIX
シェルは下位 8 bit に切り詰める点に注意。

例:

```bash
cargo run -p compiler -- compiler/example/fib.t -o /tmp/fib
/tmp/fib; echo $?    # 21 (= fib(8))
```

## 設計

パイプラインは **AST → IR → Cranelift IR → object bytes** の3段。
中間 IR を挟むことで、AST に直接バックエンドの都合を持ち込まず、
将来の `AllocatorBinding` 配線・定数伝搬・devirtualize 等の解析を
IR レイヤで完結できる構成にしてある。

- `src/main.rs` — CLI
- `src/lib.rs` — `compile_file()` パブリック API
- `src/options.rs` — `CompilerOptions` / `EmitKind`
- `src/ir.rs` — 中間 IR 定義（`Module` / `Function` / `Block` / `Instruction` / `Terminator` / `Type` / `ValueId` / `LocalId` / `BlockId` / `FuncId`）と `Display` 実装
- `src/lower.rs` — AST → IR の lowering pass
- `src/codegen.rs` — IR → Cranelift IR の codegen pass + `.o` 出力
- `src/driver.rs` — `cc` を呼んで `.o` を実行ファイルにリンク

IR の値モデルは「型付きローカルスロット + 関数ローカルな SSA 値」の
組み合わせで、`val` / `var` / 関数引数は `LocalId` 経由で `LoadLocal` /
`StoreLocal` する。SSA 構築は Cranelift の `FunctionBuilder` に任せる。

frontend と type_checker は既存の `compiler_core::CompilerSession` と
`interpreter::check_typing` を再利用しており、interpreter / JIT と同じ
フロントエンド検査を経由する。

### `--emit=ir` の出力例

```bash
cargo run -p compiler -- compiler/example/fib.t --emit=ir -o /tmp/fib.ir
```

```
local function toy_fib(@l0: u64) -> u64 {
  locals:
    @l1: u64
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

`--emit=clif` は IR lowering 後の Cranelift IR を、`--emit=obj` は
リンク前の `.o`、`--emit=exe`（default）は最終バイナリを出力する。

## 次のフェーズ（todo.md #183 参照）

- Phase A: `toy_ir` の新設、`AllocatorBinding` 配線、AST → IR lowering
- Phase B: 拡張（文字列、struct、tuple、enum、trait のサポート）
- Phase C: 呼び出し規約（隠し allocator パラメータ）、ランタイムを C ABI `.o` で提供
- Phase D: interpreter / JIT / コンパイラの 3 経路一致テスト
- Phase E: 定数伝搬・インライン化・devirtualize による最適化
