# compiler

AOT コンパイラ。toylang のソースから native の実行可能バイナリを生成する。

## ステータス: MVP

現状サポートしている機能はとても限定的で、interpreter / JIT で実行できるプログラムのうち **数値計算と制御フローのみ** からなるものが対象。

サポート:

- 型: `i64`, `u64`, `bool`, `Unit`、scalar フィールドのみの struct
- 式: リテラル、算術 (`+ - * / %`)、比較 (`== != < <= > >=`)、短絡論理 (`&& ||`)、ビット演算 (`& | ^ ~ << >>`)、unary (`- ! ~`)
- 文: `val` / `var`（型注釈あり）、代入、`if`/`elif`/`else`、`while`、`for ... in start..end`、`break` / `continue`、`return`
- 同一プログラム内の関数呼び出し（`main` のみ C ABI でエクスポート、それ以外は `toy_<name>` プレフィックス）
- **`panic("literal")` / `assert(cond, "literal")`**: メッセージは文字列リテラル限定。`puts` + `exit(1)` で実装
- **`print(x)` / `println(x)`**: `i64` / `u64` / `bool` / 文字列リテラルを受け取る。`compiler/runtime/toylang_rt.c` の `toy_print_*` / `toy_println_*` ヘルパー経由で stdout に出力（driver が `cc` で同時にコンパイル＋リンク）
- **struct**: `struct Name { field: Type, ... }` 宣言、`Name { field: value, ... }` リテラル、`obj.field` 読み取り、`obj.field = value` 書き込み。**制約**: フィールドは scalar のみ、関数引数 / 戻り値として struct 値は渡せない（field を個別に渡す必要あり）、`struct.struct.field` のような chain 構造は未対応

**注意**: `panic` / `print` / `println` は stdout に出力する（interpreter / JIT は `panic` を stderr に出力する点が既知の挙動差）

未対応（明確なエラーで reject される）:

- 任意の文字列値（リテラルのみ可）、tuple、配列、dict
- enum、match、trait
- allocator、contracts (`requires` / `ensures`)
- generics（型パラメータを持つ関数 / struct）
- struct を関数引数 / 戻り値として渡す
- ネストしたフィールドアクセス（`a.b.c`）
- heap / pointer builtins
- `i64` ↔ `u64` 以外の cast

## 使い方

```bash
# 実行ファイルを生成
cargo run -p compiler -- input.t -o output

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
