# TODO - Interpreter Improvements

## 完了済み ✅

147. JIT skip 理由の詳細化: `analyze` が `Result<EligibleSet, String>` を返すように変更、各 reject 点で `note(reason, ...)` で具体的な理由 (関数名 + 構文要素 / unsupported builtin / ptr_read の type-hint 欠落 等) を記録。`-v` で `JIT: skipped (function `main`: uses unsupported expression array literal)` 形式で出力 (2026-04-26)
146. JIT Phase 2c-2 (ptr_read/ptr_write 対応): 8 helper を追加 (read/write × i64/u64/bool/ptr)。eligibility が val/var/assign の左辺型から `__builtin_ptr_read` の期待型を pre-pass で収集し `ptr_read_hints: HashMap<ExprRef, ScalarTy>` に格納。codegen は hint で helper を選択。callback は `HeapManager::typed_read/typed_write` を経由し interpreter と互換 (2026-04-26)
145. JIT 統合テスト追加: `interpreter/tests/jit_integration.rs` で `INTERPRETER_JIT=1` ON/OFF のバイナリ実行を比較。fib/jit_cast/jit_print/jit_heap で exit code + stdout 往復一致、fallback プログラム (配列使用) の挙動、verbose ログ (`JIT compiled:` / `JIT: skipped`) の存在を検証。8 テスト追加 (--no-default-features では 5 テスト) (2026-04-26)
144. JIT Phase 2c (heap builtins): `heap_alloc`/`heap_free`/`heap_realloc`/`ptr_is_null`/`mem_copy`/`mem_move`/`mem_set` を JIT で扱う。`ScalarTy::Ptr` を追加 (cranelift I64 マップ)、callback は thread_local の `JIT_HEAP` で `HeapManager` を共有、`PtrIsNull` は `icmp_imm` でインライン展開。`ptr_read`/`ptr_write` は typed-slot 仕様の都合で次回 (2026-04-26)
143. JIT Phase 2b (print/println callback): `BuiltinCall(Print/Println, scalar_arg)` を JIT で扱う。`extern "C"` Rust callback (jit_print_i64/u64/bool + println 各種) を `JITBuilder.symbol()` で登録、`Linkage::Import` で declare、codegen は引数型から helper を選んで call。eligibility は arg=1, type∈{i64,u64,bool} を許可、return type は Unit (2026-04-26)
142. JIT Phase 2a (Cast 対応): `Expr::Cast` を eligibility/codegen に追加。i64 ↔ u64 (identity 含む) のみ対応。両者ともクランリフトの I64 にマップされるため codegen は no-op (2026-04-26)
141. main の数値戻り値を process exit code に: `Object::Int64`/`UInt64` のときに `process::exit` で値を返す。fib なら `cargo run example/fib.t` の終了コードが 8 になる (2026-04-26)
140. cranelift-based JIT (Phase 1): `INTERPRETER_JIT=1` env var で opt-in、cargo feature `jit` (default on)。i64/u64/bool/Unit のみ使う関数 (`main` から transitively reachable) を一括コンパイル。リテラル/算術/比較/論理 (短絡)/ビット/シフト/単項/val/var/代入/if-elif-else/while/for-range/break/continue/return/関数呼び出しに対応。サポート外は silent fallback (`-v` で skip 理由表示)。設計は `~/.claude/plans/mutable-wobbling-kettle.md` (2026-04-26)
139. `__builtin_sizeof` の struct / enum / tuple / array 対応: struct はフィールド合計、enum は 1-byte タグ + payload 合計（variant 依存）、tuple / array は要素合計。`List<Option<i64>>` のようなケースで stride 計算に利用可能 (2026-04-22)
138. 任意型 T に対応した `ptr_write` / `ptr_read`: HeapManager に typed-slot map を追加、write は任意型の RcObject を保存、read は型ヒント（`val v: T = ...`）に従って返す。`List<i64>` / `List<bool>` / `List<T>` の実用的な動作 (2026-04-22)
137. Allocator を型パラメータに取る struct: `struct List<T, A: Allocator>` 形式。struct 生成時に型注釈をヒントとしてフィールドに現れない T を推論、メソッド内の `Self` 再構築に return type ヒントを伝播、struct-level bound を impl body へマージ、block レベルの型ヒント上書きを修正 (2026-04-22)
136. `__builtin_sizeof(value)` builtin: 引数の型のバイトサイズを u64 で返す。generic `T` の実体サイズを取得するジェネリックコレクションの土台。現状 primitive（u64/i64/bool/ptr/unit）のみ対応、struct/enum/str は未対応 (2026-04-22)
135. match の文字列リテラルパターン: `"hello" => ...` で分岐可能。scrutinee 型に `str` を追加、重複リテラルは unreachable エラー、wildcard 必須 (2026-04-22)
134. match のネストパターン: タプル variant のサブパターンに再帰的なパターンを書ける（`Option::Some(Option::Some(v))`、`Box::Put(Color::Red)`、`Some(42i64)`）。`Pattern` を再帰構造に統合し `PatternBinding` を削除、型ヒントをネスト構築に伝播、irrefutable 判定で不要な unreachable を避ける (2026-04-22)
133. match のリテラルパターン: primitive scrutinee（`bool`/`i64`/`u64`）に対して `0i64 =>`、`true =>` のようなリテラルで分岐可能。bool は両値網羅、整数は wildcard 必須、重複リテラルは unreachable エラー (2026-04-22)
132. Enum + match（Phase 3）: ジェネリック `enum Option<T> { None, Some(T) }`。タプル variant 引数からの型パラメータ推論、ユニット variant の型注釈ヒント補完、match パターンバインディングでの型パラメータ置換 (2026-04-21)
131. match の到達性チェック: `_` 以降の arm / 同一 variant の重複 arm を型チェックエラーとして検出 (2026-04-20)
130. Range literal を式として利用可能に: `0u64..10u64` を式位置で使える。`for i in 0..n` と `val r = 0..n` の両方が動作、`to` 形式も互換維持。`Object::Range`、`TypeDecl::Range(Box<T>)` 追加 (2026-04-19)
129. Enum + match（Phase 2c）: 網羅性チェック。wildcard なしで variant 欠落の場合に型チェックエラー。欠けている variant 名をエラーに明示 (2026-04-19)
128. Enum + match（Phase 2）: タプル variant `Shape::Circle(i64)`, `Rect(i64, i64)` のコンストラクタ、バインディングパターン `Circle(r)` と `_` discard、型チェックの payload 型検証 (2026-04-19)
127. Enum + match（Phase 1）: `enum Name { A, B, C }` unit variant、`Color::Red` バリアント参照、`match scrutinee { pat => body, _ => body }` による分岐。型チェックは全 arm の型一致と variant 存在を検証 (2026-04-19)
126. 非ジェネリック struct の associated function 対応: `List::new()` 形式が generic struct なしで動作、メソッドチェーンの return type 正規化 (2026-04-19)
125. struct field 代入 `obj.field = x` サポート: interpreter の handle_assignment に FieldAccess LHS 追加、Counter.inc() 等の imperative スタイルが書けるように (2026-04-19)
124. Allocator システム実装（Phase 1a/1b/1c/2a/2b + Phase 3 部分）: `with allocator = expr { ... }` 構文、`TypeDecl::Allocator`、`Object::Allocator(Rc<dyn Allocator>)`、`Allocator` trait + Global/Arena/FixedBuffer、`<A: Allocator>` bound（関数・struct・impl）、bound 連鎖、`ambient` 糖衣、自動 ambient 挿入、ユーザ空間 List<u64> 対応。設計・進捗は `ALLOCATOR_PLAN.md`、使用例は `interpreter/example/allocator_*.t` (2026-04-19)
123. ヒープメモリ管理の完全実装: heap_alloc/free/realloc、ptr_read/write/is_null、mem_copy/move/set を allocator stack 経由でルーティング (2026-04-19)
122. 動的配列（List 型）ユーザ空間対応: `struct List { data: ptr, len: u64, cap: u64 }` + impl + heap builtin で push/get/imperative な growth を記述可能 (2026-04-19)
120. interpreter/evaluation.rs (2632行) を evaluation/ モジュール7ファイルに分割: operators/expression/statement/call/slice/builtin/mod に責務分離 (2026-04-19)
119. parser/core.rs (1038行) を core/types/declarations/program_parser に4分割: パース責務ごとに独立 (2026-04-17)
118. type_checker.rs (1000行) を visitor/visitor_impl/module_access に3分割: Acceptable/ProgramVisitor/AstVisitor実装を切り離し (2026-04-17)
117. ast.rs (1647行) を ast/{expr,pool,program,builder}.rs に分割: 責務別5ファイル構成、re-exportで後方互換維持 (2026-04-17)
116. type_checkerコード重複削減リファクタ: シンボル解決/エラー位置付加ヘルパーの統一、__getitem__アクセスロジック統合で正味52行削減 (2026-04-17)
115. CLAUDE.mdにlexer定義のキーワード・演算子を追記 (2026-02-28)
114. テストスイート大規模改善・統合: frontend 26→16, interpreter 41→11ファイル、99テスト追加、合計787テスト (2026-02-28)
113. ジェネリック構造体高度テスト7件の失敗修正: bare `self`、`val`キーワード競合、`else if`バグ回避 (2026-02-27)
112. ネスト配列型推論と改行対応パース修正: `[[u64;2];3]`の型推論正常動作 (2026-02-27)
111. C++11スタイル`>>`トークン分割: `Container<Container<T>>`のネストジェネリック型パース対応 (2025-12-10)
110. パーサーでのジェネリック型引数サポート: `Container<T>`パースと関連関数戻り値型の完全な型置換 (2025-12-10)
109. ジェネリック構造体フィールドアクセス型パラメータ置換: `Container<u64>.value`が正しく`u64`を返す (2025-12-09)
108. 単一型パラメータGenericsの基本実装: 関数・構造体でのジェネリクス構文パース (2025-09-07)
107. 負数インデックス推論修正: `a[-1]`が自動的にi64として推論 (2025-09-06)

## 未実装 📋

147. **JIT Phase 2 拡張** — Phase 1 / 2a (Cast) / 2b (print/println) / 2c (heap builtins) / 2c-2 (ptr_read/write) は完了。残: (d) struct field/method、(e) allocator stack 連携、(f) generic 関数の monomorphize
96. **Enum/match 拡張** — Phase 1/2/2c/3 + リテラル + ネスト + 文字列リテラルパターン完了。標準 Option/Result ライブラリ、深い網羅性解析は未実装
29. **Option<T> を標準的に提供** — ジェネリック enum は動作中。ユーザ空間で書ける（`enum Option<T> { None, Some(T) }`）。標準ライブラリとして組み込むかは別議論
30. **組み込み関数システム** — 型変換（u64 ↔ i64 は既に `as` で可能）、数学関数（`abs`, `min`, `max`, `pow`, `sqrt`）
65. **frontendの改善課題** — docコメント拡充、プロパティベーステスト追加、コード重複削減
26. **ドキュメント整備** — 言語仕様 / API ドキュメント
121. **Allocator システム残作業** — `__builtin_sizeof`（primitive/struct/enum/tuple/array）、`struct List<T, A: Allocator>`、任意型 T 対応の `ptr_write`/`ptr_read` 実装済み。残り: IR レベルの `AllocatorBinding`、Phase 4 以降の native codegen（詳細は `ALLOCATOR_PLAN.md`）

## 検討中の機能

* FFI/拡張ライブラリ
* 文字列操作
* ラムダ式・クロージャ
* モジュール拡張（バージョニング、リモートパッケージ）
* 言語組み込みテスト機能
* 言語内からのAST取得・操作

## 実装済み機能サマリー

### コア言語機能
- 基本言語機能: if/else/elif、for、while、break/continue、return
- 変数: val（不変）/var（可変）、コンテキストベース型推論
- 固定配列: 型推論対応、インデックス型推論、境界チェック
- 配列スライス: `arr[start..end]`、`arr[..]`、負インデックス`arr[-1]`対応
- 辞書（Dict）型: `dict{key: value}`リテラル、Object型キーサポート
- 構造体: 宣言、implブロック、フィールドアクセス（read/write 両対応）、メソッド、非ジェネリック struct でも `Struct::new()` の associated function、`__getitem__`/`__setitem__`
- 文字列: ConstString/String二重システム、`str.len()`、`.concat()`、`.trim()`、`.to_upper()`、`.to_lower()`、`.split()`、`.substring()`、`.contains()`
- コメント: `#`（行）、`/* */`（ブロック）
- Allocator システム: `with allocator = expr { ... }`、`ambient` キーワード、`<A: Allocator>` bound、自動 ambient 挿入、Arena / FixedBuffer allocator
- Enum + match（Phase 1/2）: unit + tuple variant、`Enum::Variant` / `Enum::Variant(args)`、`match` arm は unit・tuple パターン（バインディング/`_` discard）+ ワイルドカード `_`

### 型システム
- 自動型変換・型推論（数値リテラルのサフィックス省略可）
- ジェネリック関数: `fn identity<T>(x: T) -> T`（パース→型推論→実行）
- ジェネリック構造体: `struct Container<T>`、constraint-based型推論
- ネストジェネリック: `Container<Container<T>>`（C++11スタイル`>>`分割）
- Self キーワード: implブロック内での構造体参照
- Trait bound: `<A: Allocator>` を関数・struct・impl に付与、呼び出し側で検証、bound 連鎖

### モジュール・その他
- Go-styleモジュールシステム: package/import/qualified name resolution
- 統合インデックスシステム: 配列・辞書・構造体で統一`x[key]`構文

### テスト状況
- 合計 894 テスト（100% 成功率、2026-04-22 時点）

### パーサーの既知制限事項
- bare `self` 構文非対応（`self: Self` が必要）
- `else if` 未サポート（`elif`を使用）
- `val` はキーワードのためパラメータ名に使用不可
