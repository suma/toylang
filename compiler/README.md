# compiler

AOT コンパイラ。toylang のソースから native の実行可能バイナリを生成する。

## ステータス

MVP として始まったが、Phase A〜Z の段階的拡張で interpreter とほぼ同等の表面をカバーするまで成長している。下記サポート一覧は実装順 (Phase A → 最近のもの) に並んでいる。`compiler/tests/e2e.rs` (191 件) と `compiler/tests/consistency.rs` (23 件、interpreter / JIT / AOT 3 経路一致) が緑のものはすべて使える。

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
- **compound-returning method を val/var rhs に (Phase W)**: `val q = p.swap()` のように struct / tuple / enum を返す method 呼び出しを val/var rhs として直接受理。`lower_let` に MethodCall + compound return パスを追加し、新ヘルパ `resolve_method_target` で receiver と method (inherent / generic 両方) を解決、`CallStruct` / `CallTuple` / `CallEnum` で multi-result call を emit して binding に入れる。これで Phase R / R3 で残っていた compound 戻り値の制限が val rhs では解消
- **compound-returning call の直接 print (Phase U)**: `println(make_point())` / `println(p.doubled())` のように、struct / tuple / enum を返す関数 / メソッド呼び出しを print 引数として直接書ける。print path は callee の戻り型に応じて scratch binding を allocate し、`CallStruct` / `CallTuple` / `CallEnum` で受けてから既存の `emit_print_*` ヘルパに routed
- **str 値 (Phase T)**: `str` を val/var、関数引数 / 戻り値、struct field、tuple element に持てる。runtime 表現は `.rodata` 上の null-terminated バイト列のポインタ (`Type::Str` = i64 ポインタ)。リテラルは `InstKind::ConstStr` で `symbol_value` 経由で address を materialize、関数 boundary も同じく i64 1 個分の slot で渡す。`println(s)` は `value_ty == Type::Str` で `toy_println_str` ヘルパに dispatch。**制約**: 文字列同士の連結 / 比較 / `.len()` などのメソッドは未対応 (interpreter 側のみ)
- **配列要素に tuple (Phase Y3)**: `[(1, 2), (3, 4)]` 形式の tuple 要素も struct と同じ leaf-index addressing で動作。`val t: (i64, i64) = arr[i]` は新しい `Binding::Tuple` を allocate して各 leaf を ArrayLoad で読む。runtime index も対応
- **配列の compound 要素 + range slicing (Phase Y2)**: 要素に struct (`[Point { ... }, Point { ... }]`) を許可。スライドは leaf-index addressing で扱い、各要素は `leaf_count` 個の連続する 8 バイトスロットを占有。`val p: Point = arr[i]` は新しい `Binding::Struct` を allocate し、各 leaf を ArrayLoad で読んで対応する local に store。`arr[start..end]` (両端 const) は新規 ArraySlot を確保して各 leaf を ArrayLoad+ArrayStore でコピー。**制約**: tuple / enum 要素は未対応、range slicing は const bound のみ
- **配列 (Phase S + Y)**: `[a, b, c]` リテラルと `arr[idx]` の read / write をサポート。`Binding::Array` が `ArraySlotId` を保持し、IR の `ArrayLoad` / `ArrayStore` で `(slot, index, elem_ty)` を渡す。codegen は per-IR-slot で cranelift `StackSlot` (length × stride バイト、現状 stride は 8 バイト固定) を確保、index は `iadd(stack_addr, idx * stride)` + `load`/`store`。const index も runtime index も同一の IR 命令で扱われる (cranelift の最適化で const index は折りたたまれる)。print 出力は `[1, 2, 3]` 形式 (interpreter 一致)。**制約**: 要素は scalar (i64/u64/f64/bool) のみ、range slicing は未対応
- **method-only generic params (Phase X)**: `impl Box { fn pick<U>(self, a: U, b: U) -> U }` のように impl の generic params とは独立した method 自身の generic params を許可。frontend parser が `<U>` を実際に parse、type-checker が arg type から U を substitute、compiler は `instantiate_generic_method_with_args` で receiver の type_args (impl-level) と call args の型 (method-only) の両方から subst を組み立ててモノモル化
- **method dispatch (Phase R)**: `impl <Type> { ... }` の inherent method、`impl <Trait> for <Type>` の trait conformance method、`fn f<T: Trait>(x: T) { x.method() }` の bound 経由 generic 呼び出しすべて対応。impl ブロックを pre-scan して `(target_struct_symbol, method_name) → MethodFunction` の registry を構築、各メソッドを mangled name `toy_<Type>__<method>` で declare。`Self` は impl 対象に substitute。call site は receiver 識別子を struct/enum binding に解決し、`(target, method)` で `FuncId` を引いて receiver の leaf scalar 列を call args の先頭に prepend。Phase L (generic monomorphisation) と組み合わせることで trait dispatch も静的に解決される (vtable 不要)。**Phase R3** で `impl<T> Cell<T> { fn get(self: Self) -> T }` のような generic method も lazy monomorphisation 対応 (call site で receiver の type_args から impl の generic param を bind して fresh `FuncId` を declare、queue で body lowering)。**制約**: dynamic `dyn Trait` は未対応
- **extension trait over primitives (Step A〜F)**: `impl <Trait> for i64 / f64 / u64 / bool / str / ptr` をユーザが書ける。`primitive_type_decl_for_target_sym` ヘルパで `Self` を対応する primitive `TypeDecl` に解決、`lower_method_call` の冒頭に `value_scalar` driven の primitive-receiver dispatch arm を追加 (struct path より先に走るので chained call `x.abs().abs()` も lower 可能)。impl method は `toy_<TypeName>__<method>` (例: `toy_i64__neg`、`toy_f64__abs`) として declare。stdlib の `i64.abs()` / `f64.abs()` / `f64.sqrt()` も `core/std/{i64,f64}.t` の extension trait impl として配信、`BuiltinMethod::{I64Abs, F64Abs, F64Sqrt}` の hardcoded fast path は削除済み
- **`extern fn` 宣言 (Math externalisation Phase 1〜4)**: `extern fn name(params) -> ret` で signature だけ宣言、body は backend が提供。Compiler は `lower/program.rs::libm_import_name_for` で `__extern_sin_f64` → `sin` 等を libm symbol 名にマップし、IR の `Linkage::Import` で declare、`build_object_module` / `emit_clif_text` は body 定義を skip。リンカが libm から解決。math intrinsic (sin/cos/tan/log/log2/exp/floor/ceil/sqrt/abs/pow) はすべてこの仕組み経由で interpreter / JIT と対称。`extern fn name<T>(x: T) -> T` のように generic params も parser で受理されるが、AOT 側は per-instance シンボル名を持たないため未対応 (interpreter のみ動作)
- **core modules auto-load**: `<repo>/core/` 配下を起動時に再帰 integrate (詳細は上記 *core modules*)。`compile_file` が `interpreter::check_typing_with_core_modules` 経由で frontend に core dir を forward、AOT 経路でも `math::sin(x)` 等が import 行なしで呼べる
- **stdlib Option / Result (#96)**: `core/std/option.t` の `enum Option<T> { None, Some(T) }` + `impl<T> Option<T>` (is_some / is_none / unwrap_or / expect)、`core/std/result.t` の `enum Result<T, E> { Ok(T), Err(E) }` + `impl<T, E> Result<T, E>` (is_ok / is_err / unwrap_or / expect) が auto-load 経由で利用可能。enum receiver method は `instantiate_generic_method_with_self_type` + `peek_method_return_type_with_self` で struct receiver と同じ monomorph パイプラインを通る。ユーザが同名の enum / struct を inline 宣言した場合は module integration が silent skip するので衝突しない (ユーザ版が優先)
- **per-module function namespacing (#193 / #193b)**: IR の `function_index` を `(Option<DefaultSymbol> qualifier, DefaultSymbol name)` キー化。qualifier は originating module の dotted path の **last segment** (`Some("math")` for `core/std/math.t`) または `None` (user-authored)。`Module::lookup_function(qualifier, name)` がバレ呼び (None 優先 + 一意な (Some(_), name) fallback) と qualified call (Some(m), name 直接) を統一処理。`declare_function_with_module` は collision を panic で表面化 (silent overwrite を不可能に)。同一プログラム内で複数モジュールが同名 `pub fn` を持っても安全に共存し、export name も `toy_<qualifier>__<name>` で mangle されるので cranelift の declare 衝突も発生しない
- **struct field / tuple element に compound 型 (Phase Q1 + Q2)**: `struct Outer { inner: (i64, i64) }` の struct-of-tuple、`((a, b), c)` の nested tuple、`(Point, i64)` の tuple-of-struct がすべて動作。`FieldShape::Tuple` と `TupleElementShape::{Scalar, Struct, Tuple}` が再帰的な shape を表現し、`outer.inner.0` / `t.0.1` / `t.0.x` などの chain access が `resolve_field_chain` と `resolve_tuple_chain_elements` で walk される。print 出力は `Outer { inner: (3, 7) }` / `((3, 4), 5)` / `(Point { x: 1, y: 2 }, 3)` のように再帰整形。関数 param / return も `flatten_tuple_element_locals` 経由で leaf scalar まで再帰展開し boundary 通過可
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
  - **str payload**: `Result<u64, str>::Err("boom")` のように `str` 値を payload に取れる。Phase T の opaque-pointer 表現 (`Type::Str = i64-sized`) をそのまま再利用、`flatten_struct_to_cranelift_tys` の enum アームと `allocate_payload_slot` の default scalar branch がカバー。`is_supported_enum_payload` の allow-list に `Type::Str` を追加するだけで全機能 (val/var binding, match, method dispatch) が通る
  - **制約**: tuple 要素にネストした compound (struct / 別 tuple / enum) は不可
  - **スコープ**: 全 arm の body は同じ scalar 型を返す必要あり

**注意**: `panic` / `print` / `println` は stdout に出力する（interpreter / JIT は `panic` を stderr に出力する点が既知の挙動差）

未対応（明確なエラーで reject される）:

- (廃止) 任意の文字列値 — **Phase T 以降**: `str` 型を val/var、関数引数 / 戻り値、struct field に渡せる。文字列操作 (`s.len()` / 連結) はランタイムヘルパ未対応
- dict
- 配列要素に enum、range slicing で variable bound — リテラル `[a, b, c]`、const/runtime index、struct/tuple 要素、const-bound range slicing は **Phase S/Y/Y2/Y3 以降対応**
- (廃止) trait — **Phase R 以降**: inherent method, `impl <Trait> for <Type>` 経由のメソッド呼び出し、`<T: Greet>` bound 経由の generic method 呼び出しすべて対応 (monomorphisation 経由)。`dyn Trait` の動的 dispatch は対象外
- allocator
- (廃止) generics（→ struct / enum / 関数とも対応済）
- (廃止) 関数戻り値 / メソッド戻り値の compound 値を直接 `print` / `println` する — **Phase U 以降**: `println(make_point())` / `println(p.doubled())` のように直接呼べる
- struct / tuple binding 全体の再代入
- (廃止) ネストした tuple 要素 / tuple-of-struct / struct-of-tuple — **Phase Q 以降すべて対応**
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
cargo run -p compiler -- input.t --emit=clif -o input.clif

# 中間 IR をテキストで dump
cargo run -p compiler -- input.t --emit=ir -o input.ir

# core modules ディレクトリを指定
cargo run -p compiler -- input.t --core-modules /path/to/my-core -o output

# 進行ログ
cargo run -p compiler -- input.t -v -o output
```

### CLI フラグ

| フラグ | 意味 |
|---|---|
| `<file>` | 入力ソース。必須。 |
| `-o <path>` | 出力パス。`--emit=exe` のときは実行ファイル、それ以外は対応する中間生成物。 |
| `--emit <kind>` (`--emit=<kind>` も可) | `exe`(default) / `obj` / `ir` / `clif` を選択。 |
| `--release` | 全 DbC (`requires` / `ensures`) チェックを skip。`INTERPRETER_CONTRACTS=off` 相当。 |
| `-v` / `--verbose` | コンパイル進行と core modules dir 解決結果を stderr に出す。 |
| `--core-modules <DIR>` (`--core-modules=<DIR>` も可) | core modules ディレクトリを上書き。下記参照。 |

### core modules (auto-load)

interpreter と同じく compiler も起動時に `core/` 配下を再帰的に
auto-load し、`math::sin(x)` 等を `import` 行なしで呼べるように
する。解決順:

1. `--core-modules <DIR>` フラグ
2. `TOYLANG_CORE_MODULES` 環境変数 (空文字で opt-out)
3. 実行ファイル相対探索 (`<exe>/core/` →
   `<exe>/../share/toylang/core/` → `<exe>/../../core/`)

dev tree から `target/debug/compiler` を直接実行する場合は最後の
fallback (`<repo>/core/`) で見つかる。`-v` で実際に拾った path が
出る。

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
- `src/lib.rs` — `compile_file()` パブリック API + `resolve_core_modules_dir()`
- `src/options.rs` — `CompilerOptions` (`core_modules_dir` / `release` / `emit` / `verbose`) / `EmitKind`
- `src/ir.rs` — 中間 IR 定義（`Module` / `Function` / `Block` / `Instruction` / `Terminator` / `Type` / `ValueId` / `LocalId` / `BlockId` / `FuncId`、`Linkage::{Export, Local, Import}`）と `Display` 実装
- `src/lower/` — AST → IR の lowering pass。Phase Z refactor で 24 ファイル / mod.rs 265 行に分割: `consts` / `array_layout` / `types` / `method_registry` / `templates` / `bindings` / `type_inference` / `method_call` / `print` / `array_access` / `compound_storage` / `call` / `match_lowering` / `field_access` / `compound_literal` / `expr_ops` / `type_resolution` / `assign` / `let_lowering` / `loops` / `stmt` / `expr` / `program` (top-level driver、`extern fn` を `Linkage::Import` で declare、`libm_import_name_for` で `__extern_*_f64` → libm symbol を解決)
- `src/codegen.rs` — IR → Cranelift IR の codegen pass + `.o` 出力
- `src/driver.rs` — `cc` を呼んで `.o` を実行ファイルにリンク。runtime の `toylang_rt.c` は `build.rs` で 1 度だけ pre-build され、driver は `include_bytes!` した `.o` を `.rt.o` として書き出すだけ
- `build.rs` — `cc -c -O2 -fPIC runtime/toylang_rt.c -o $OUT_DIR/toylang_rt.o` を実行 (各テストの `cc` 起動コストを削るため)

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

## テスト

### 構成

`compiler/tests/` 配下に 2 ファイル：

- **`e2e.rs`** (191 テスト) — toylang ソース文字列から `compile_and_run`
  ヘルパで実行可能ファイルを生成 → spawn → exit code を assert する
  end-to-end テスト。各機能フラグ（`val` / `if` / `for` / 関数呼び出し
  / struct / tuple / enum / match / trait method / generics / `print`
  / `panic` / `requires` / `ensures` / `as` cast / array / `extern fn`
  / extension trait など）ごとに最小再現プログラムが並んでいる。サンプルは
  ほぼすべて `fn main() -> u64 { ... }` で値を return し、終了コードを
  突き合わせる方式。
- **`consistency.rs`** (23 テスト) — 同じソースを **interpreter (lib API)
  / AOT compiler (compile + spawn) / JIT (`INTERPRETER_JIT=1` で
  interpreter binary を spawn)** の 3 経路に流し、`main` の戻り値が
  3 経路で一致することを確認する横並びテスト。仕様の解釈差を早期に
  検知するセーフティネット。interpreter binary は OnceLock で 1 回だけ
  `cargo build` する。

両ファイルとも `COMPILER_E2E=skip` を環境変数に渡すと early return
してスキップする（`cc` が無いサンドボックス環境向けの opt-out）。

### 実行

```bash
# nextest（推奨、並列実行）
cargo nextest run -p compiler

# cargo test（1プロセスにまとめる）
cargo test -p compiler

# 単一テスト
cargo nextest run -p compiler -E 'test(returns_literal_exit_code)'
```

テスト全体の wall-clock は 20 コアの macOS で約 60〜70 秒。

### パフォーマンス

各テストが `compile_file(... emit=Executable)` → 生成された binary の
spawn を行う構造のため、

- compile 部分（parse + type-check + IR lowering + Cranelift codegen
  + リンク）：1テストあたり ≈ 50ms（debug build）
- 生成バイナリの新規 exec：macOS では新規 Mach-O ごとにコード署名検証
  が走り、≈ 150〜300ms

の二段構成。**並列 wall-clock の支配項は後者** で、署名検証は path /
content / `cc` / `ld` / `dlopen` / 事前 `codesign --sign -` いずれの
工夫でも回避できないことを実測済み（同じパスで内容を上書きしても
再検証される）。

すでに入っている最適化：

- **`compiler/build.rs`** が `cc -c -O2 -fPIC runtime/toylang_rt.c
  -o $OUT_DIR/toylang_rt.o` をビルド時に 1 度だけ実行し、driver は
  その `.o` を `include_bytes!` で取り込む。これにより各テストの
  `cc` 呼び出しは「2 つの `.o` をリンクするだけ」となり、C
  コンパイル分（〜数百ms）を完全に削減。

並列 wall-clock を更に縮める余地としては、

- cranelift-jit を compiler crate に取り込み、テスト用 API
  `compile_to_jit_main(source) -> fn() -> u64` で **新規 Mach-O を
  ディスクに書かない経路** を提供する。これで macOS 検証を完全に
  回避できる（in-process で executable memory を確保するため）。
  codegen.rs を `Module` trait で generic 化する作業がそれなりに
  あるため別タスク扱い。
- `e2e.rs` の小さい `fn main() -> u64 { ... }` 系テストを「1 つの
  巨大プログラムにまとめてケース ID で dispatch」する形に再構成し、
  spawn 回数を減らす。テスト分離度が落ちるトレードオフがある。

## 次のフェーズ（todo.md #183 参照）

- Phase A: `toy_ir` の新設、`AllocatorBinding` 配線、AST → IR lowering
- Phase B: 拡張（文字列、struct、tuple、enum、trait のサポート）
- Phase C: 呼び出し規約（隠し allocator パラメータ）、ランタイムを C ABI `.o` で提供
- Phase D: interpreter / JIT / コンパイラの 3 経路一致テスト
- Phase E: 定数伝搬・インライン化・devirtualize による最適化
