# compiler

AOT コンパイラ。toylang のソースから native の実行可能バイナリを生成する。

## ステータス: MVP

現状サポートしている機能はとても限定的で、interpreter / JIT で実行できるプログラムのうち **数値計算と制御フローのみ** からなるものが対象。

サポート:

- 型: `i64`, `u64`, `f64`, `bool`, `Unit`、scalar フィールドのみの struct、scalar 要素のみの tuple
- 式: リテラル、算術 (`+ - * / %`)、比較 (`== != < <= > >=`)、短絡論理 (`&& ||`)、ビット演算 (`& | ^ ~ << >>`)、unary (`- ! ~`)
- 文: `val` / `var`（型注釈あり）、代入、`if`/`elif`/`else`、`while`、`for ... in start..end`、`break` / `continue`、`return`
- 同一プログラム内の関数呼び出し（`main` のみ C ABI でエクスポート、それ以外は `toy_<name>` プレフィックス）
- **ジェネリック関数 (Phase L)**: `fn id<T>(x: T) -> T { x }` を宣言可能。各呼び出しサイトで型引数を引数の型から推論し、`(template_name, type_args)` ごとに新しい IR Function を monomorphise。`fn unwrap_or<T>(o: Option<T>, default: T) -> T` のようにジェネリック enum / struct と組み合わせ可、ジェネリック関数からジェネリック関数を呼ぶチェーンも自動展開（pending work queue で処理）
- **`as` キャスト**: `i64 ↔ u64`（identity）、`{i64, u64} ↔ f64`（cranelift の `fcvt_*_sat` で truncating saturation）。bool との cast や Unit との cast は不可
- **`f64`**: 算術（`+ - * /`）、比較、unary `-`。`%` (mod) は cranelift に native fmod が無いため reject。print 用ヘルパー (`toy_print_f64` / `toy_println_f64`) は `%g` か `%.1f` で出力
- **`panic("literal")` / `assert(cond, "literal")`**: メッセージは文字列リテラル限定。`puts` + `exit(1)` で実装
- **`print(x)` / `println(x)`**: `i64` / `u64` / `f64` / `bool` / 文字列リテラル / **struct binding / tuple binding / enum binding** / **(Phase P 以降) struct / tuple / enum のリテラル直接** を受け取る。compound 値は表記 (`Point { x: 3, y: 4 }`、`(3, 4)`、1-tuple は `(x,)`、`Color::Red`、`Shape::Circle(5)`、`Shape::Rect(3, 7)`) に展開される。**ジェネリック instantiation は型引数も表示** (`Y<i64> { b: 2 }`、`Cell<u64> { data: 7 }`、`Option<i64>::Some(5)`、`Option<Option<i64>>::Some(Option<i64>::Some(7))`) — interpreter は型引数を落として表示するため、ジェネリック型では出力が異なる (interpreter は `Y { b: 2 }`)。struct のフィールドはアルファベット順にソート、enum はランタイムで tag を見て該当 variant の表示パスを brif chain で選ぶ（`n - 1` 個の比較、最後の variant は無条件 fallthrough）。ネストした struct も再帰的に整形。リテラル直接の場合は scratch binding を allocate してから既存の `emit_print_*` ヘルパに routed。実体は `compiler/runtime/toylang_rt.c` の `toy_print_*` / `toy_println_*` ヘルパー経由で stdout に出力（driver が `cc` で同時にコンパイル＋リンク）。**制約**: struct/tuple-returning call の結果や generic struct/enum リテラル（型引数推論手段が無い）は依然 `val` で受ける必要がある
- **struct**: `struct Name { field: Type, ... }` 宣言、`Name { field: value, ... }` リテラル、`obj.field` 読み取り、`obj.field = value` 書き込み、**関数引数として struct 値を渡せる**、**関数戻り値として struct 値を返せる**（codegen が境界で per-field cranelift param / multi-return に展開）、**ジェネリック struct (Phase K)** をサポート (`struct Cell<T> { data: T }`、`val c: Cell<u64> = ...`、`fn make() -> Cell<u64> { ... }`、`Cell<u64>` と `Cell<i64>` は別の `StructId` として独立)。**制約**: フィールドは scalar (i64/u64/f64/bool) または別 struct のみ、struct binding 全体の再代入は不可、struct-returning call を式位置で使えない（必ず `val` で受ける）、ジェネリック struct リテラルの単独構築 (`Cell { data: ... }`) は型注釈必須
- **tuple**: `(a, b, c)` リテラル、`t.0` / `t.1` 要素アクセス、`t.N = value` 要素書き込み、`val (a, b) = (x, y)` 分解（パーサが desugar）、**関数引数 / 戻り値として tuple 値を渡せる**（codegen が境界で per-element cranelift param / multi-return に展開）、`val (a, b) = make_pair()` 形式の tuple-returning call も動作。**制約**: scalar 要素のみ、ネストした tuple は未対応、tuple-returning call は式位置で使えない（必ず `val` で受ける）
- **トップレベル `const`**: `const NAME: Type = expr` を定義、起動時の値（リテラル / 既存 const 参照 / 単純な算術 fold）として利用可能。複雑な初期化式や文字列定数は未対応
- **DbC (`requires` / `ensures`)**: 関数の事前 / 事後条件を実行時にチェック。違反時は `panic: requires violation` / `panic: ensures violation` で停止。`ensures` 内の `result` は scalar 戻り値にのみ bind される（struct 戻り値は最初の field を bind）。`--release` フラグで全 contract チェックを skip
- **ネストした struct**: struct のフィールドが別の struct でも可。`a.b.c` のような chain access、`outer.inner.x = v` のような chain assignment、`Outer { inner: Inner { x: 1 } }` の入れ子リテラルがすべて動作。関数引数として渡せば codegen が leaf scalar まで再帰展開
- **struct field に tuple (Phase Q)**: `struct Outer { inner: (i64, i64) }` のように tuple を struct field に持てる。`FieldShape::Tuple { tuple_id, elements }` で per-element local を保持し、`outer.inner.0` のような chain access も `lower_tuple_access` の FieldAccess アームで動作。print 出力は `Outer { inner: (3, 7), ... }`、関数 param / return も `flatten_struct_locals` の Tuple アームで leaf scalar まで再帰展開し boundary 通過可。要素は scalar (i64/u64/f64/bool) のみ
- **enum + match (Phase A1 + A2)**: 非ジェネリックな `enum E { Unit, Tuple(i64, u64), ... }` 宣言、`E::Unit` / `E::Tuple(args)` 構築、`match` で variant 分岐。各 variant の payload は `i64` / `u64` / `f64` / `bool` / 別 enum / struct / tuple を受理。
  - **トップレベルパターン**: `Enum::Variant(...)` / `Wildcard (_)` / `Literal(...)`（scalar scrutinee に対してのみ）
  - **scrutinee**: enum binding に加え、scalar 値を返す任意の式（`match n { 0u64 => ..., _ => ... }` のように integer / bool 直接 match 可能）
  - **variant サブパターン**: `Name(sym)` で payload を fresh scalar local に bind、`_` で discard、`Literal` で payload にリテラル等価チェック追加（`Shape::Circle(0i64) => ...` のように）
  - **guard**: `Pat if cond => body` をサポート。bindings は guard 評価時にスコープ内
  - **関数境界 (Phase B + E)**: enum を関数引数 / 戻り値の双方で受け取り / 返せる（`fn area(s: Shape) -> i64`、`fn make() -> Shape { ... }`）。codegen が `[tag, variant0_payload..., variant1_payload..., ...]` の canonical 順で per-slot cranelift param / 多値 Return に展開し、caller / callee の per-variant payload locals が同順で allocate されるので boundary が一致。enum 戻り型の関数 body は tail が if-chain / match / 単一の `Enum::Variant(args)` 構築 / 既存 enum binding の identifier いずれでも OK（`lower_body` が target locals を pre-allocate して `lower_into_enum_target` 経由で書き込む）。frontend type-checker の return-type 比較も `Identifier <-> Enum(name, [])` および `Struct(name, args) <-> Enum(name, args)` を unify するよう拡張済み
  - **ジェネリック enum (Phase F + G)**: `enum Option<T> { None, Some(T) }` を宣言可能。各使用サイト（型注釈、関数引数 / 戻り値、`val x: Option<i64> = ...`）で型引数を取り出してモノモル化（`(base_name, type_args) → EnumId` の dedup）。型引数は (1) val/var の型注釈、(2) 関数 param / return 型から決定、(3) `Option::Some(42i64)` のように tuple variant の引数型から推論（型注釈なしのケース）。`Option<i64>` と `Option<u64>` は別の `EnumId` として管理されるので衝突しない。**ネストしたジェネリック** (`Option<Option<i64>>`) もサポート。**制約**: `f64` は引き続き payload 不可、ジェネリックパラメータは i64/u64/bool/別 enum に解決されるもののみ
  - **ネストした enum payload + サブパターン (Phase G)**: enum payload に enum を許容（`enum Box<T> { Put(T) }` で `Box<Box<u64>>` も可）。`val x: Option<Option<i64>> = Option::Some(Option::Some(42i64))` の構築、`match x { Option::Some(Option::Some(v)) => ... }` のネストパターン、`println(x)` の再帰的出力すべて動作。`EnumStorage` は `PayloadSlot::Scalar { local, ty }` または `PayloadSlot::Enum(Box<EnumStorage>)` を持つ recursive 構造で、function boundary flatten / load / copy / dispatch すべて再帰
  - **`print` / `println`**: enum binding（`val` / `var` 由来 または関数引数）を受け取って interpreter と同形式に出力（unit variant: `Color::Red`、tuple variant: `Shape::Circle(5)` / `Shape::Rect(3, 7)`）。runtime tag dispatch で variant ごとの分岐を brif chain で生成。enum リテラル直接（`println(Enum::Variant(args))`）は不可、`val` で受ける必要あり
  - **enum 構築を `if` / `match` 等の式位置で (Phase D)**: `val s = if cond { Pick::A(n) } elif ... { Pick::B } else { Pick::C(m) }` や `val s = match n { 0u64 => Pick::Zero, _ => Pick::Big(n) }` のように、複数分岐の各 tail で enum を構築するパターンを受理。`detect_enum_result` で全分岐が同じ enum を返すか静的に判定し、`lower_into_enum_target` 経由で各分岐が同じ tag/payload locals に書き込む（cranelift の `def_var` walk で merge 時に SSA 化）。ネストした if-chain、`match` arm の guard、blocks (`{ stmt; tail }`) も再帰で動作。tail 位置で既存の enum binding identifier を返すケースも copy 経路で動作
  - **enum 再代入 (Phase I)**: `var p = Pick::A(5u64); p = Pick::B; p = Pick::C(7u64)` のように enum binding 全体の再代入が可能（既存の tag/payload locals に書き込む、cranelift の def_var が再 binding 担当）
  - **tuple payload (Phase O)**: `enum Pair { Both((i64, i64)), None }` のように tuple 値を payload に取れる。`PayloadSlot::Tuple { tuple_id, elements }` で per-element local を保持し、`emit_print_tuple` 経由で `Pair::Both((3, 4))` のように出力。`Option<(i64, i64)>` のような generic 経由も `substitute_payload_type` の Tuple アームで処理。要素は scalar (i64/u64/f64/bool) のみ
  - **制約**: tuple 要素にネストした compound (struct / 別 tuple / enum) は不可
  - **スコープ**: 全 arm の body は同じ scalar 型を返す必要あり

**注意**: `panic` / `print` / `println` は stdout に出力する（interpreter / JIT は `panic` を stderr に出力する点が既知の挙動差）

未対応（明確なエラーで reject される）:

- 任意の文字列値（リテラルのみ可）、配列、dict
- trait
- allocator
- (廃止) generics（→ struct / enum / 関数とも対応済）
- 関数戻り値の compound 値を直接 `print` / `println` する（`val` で受ければ可。**Phase P 以降**: struct / tuple / enum リテラルは直接 `print` できる）
- struct / tuple binding 全体の再代入
- ネストした tuple 要素 (`((a, b), c)`)、tuple-of-struct（**Phase Q 以降**: struct-of-tuple は対応）
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
