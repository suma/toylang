# TODO

## 完了済み ✅

> **この節は直近 (ここ 3〜4 週) の 1 行サマリだけを持つ。** それより前は
> [`todo_done.md`](todo_done.md) に移す (常時は読まない)。 実装の経緯・測定値・ファイルパス・
> テスト数は git log のコミットメッセージにある。フェーズ設計は
> [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md) /
> [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md) /
> [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md) /
> [`FEATURE_NOTES.md`](FEATURE_NOTES.md) を参照。
> ここを段落で埋めると、常時読まれるファイルが changelog になる。

### 2026-10-12

- **RUN-ENTRY-DISPLAY — `toy build` / `run` が入口ファイルをパッケージ相対で名乗る (全レーンと build したバイナリ。打ったパスのままだった)。実パスは `RunOptions::entry_path` / `check_typing_diagnostics_at` で別に運ぶ**
- **TEST-FAILURE-SHAPE — テストの backtrace の底は `test "名前"` (VM レーンは `__test_N`、AOT は driver の `main` だった)、AOT ドライバの位置もパッケージ相対。`--release` の `__builtin_backtrace()` が入口フレームを返す (空文字列だった)**
- **TEST-NAME-COLLISION — モジュールの `test` が自分の本体を走らせる (各ファイルが `__test_0` から数えるので、エントリの同番のテストの本体を走らせ、その合否を報告していた)**
- **COMPOUND-CALL-EXPR-POS — compiled レーンで compound の結果を連鎖・配列要素・フィールド・タプル要素・捨てる文・`match` の scrutinee に置ける。配列要素の compound なフィールド (`hs[i].e`) と enum を持つタプル (`(1u64, E::A(7i64))`) も**
- **COMPOUND-TEMP-RECEIVER — 所有しない compound の一時値へのメソッド (`f().to_str()`) と `println(f())` が compiled レーンで通る**
- **TRAIT-DEFAULT-BODY-T — generic trait の default body が `T` を書ける (書いた本体は impl ごとに複製して型を言い直す)。`T::default()` も**
- **ITER-BOUND-INFERENCE — `fn f<T, I: Iterator<T>>(it: I)` の `T` を `I` の impl から推論する**
- **GENERIC-REF-PARAM-PRIM — generic な method の `&T` 引数は、`T` がスカラーなら番地で渡す (VM は誤答・AOT は落ちていた)**
- **CALLEE-DROP-GENERIC の裸の `T` — `min` / `max` / `clamp` が返さなかった `String` を解放する (漏れていた)**
- **GENERIC-INFER-STRUCT-LITERAL-ARG — compiled レーンが構造体リテラルの引数から型引数を推論する (`min(K { .. }, K { .. })`)**
- **PRIM-RECEIVER-ENUM-TAIL — プリミティブのレシーバが compound を返す呼び出し (`a.cmp(b)`) を関数末尾 / 枝の値 / compound の slot に置ける**
- **STDLIB-ITER-TERMINAL — `Iterator<T>` の default body に `count` / `last` / `nth` / `fold` / `any` / `all` / `position` / `find`、opt-in の `OrdIterator` (`min` / `max`) と `SumIterator` (`sum`、`core/std/ops.t` の `Add`)、`Option::unwrap_or_default`**
- **STDLIB-ORD-CMP — `enum Ordering`、`Ord` の default `cmp` / `le` / `gt` / `ge` (struct に `<=` / `>` / `>=` が付く)、汎用 `min` / `max` / `clamp`、`Vec::binary_search`**
- **generic trait の default body — 継承したシグネチャを impl の型引数で言い直す / trait メソッド自身の型引数 (`fold<Acc>`) を捨てずに解析 / 空の impl の bound (`impl<T: Ord> X<T> {}`) を継承した本体に効かせる (`Stmt::ImplBlock` が自分の generics を持つ)**
- **トレイト実装の登録を signature の事前パスへ — ユーザの impl の本体検査から stdlib の `u64: Default` が見えなかった**
- **IR VM: compound の戻り値をアドレスを取られたローカルの heap cell に書く (`var best = it.next()` の後の `x.lt(b)` が古い値を読んでいた)**

### 2026-10-11

- **RUNTIME-REFS-LAZY — ランタイム関数の import を呼ぶ分だけに。宣言は `codegen/runtime_syms.rs` の表 1 つ**
- **COMPOUND-CALL-EXPR-POS (関数の結果) — compound を返す呼び出しを関数の末尾と `return` に置ける** (残りは未実装節)
- **COMPOUND-CALL-EXPR-POS (読み出しの根) — `mk().n` / `triple(1).2` / `mk().get()` / `v.get(i).len()` がどの位置でも compiled レーンで動く**
- **STDLIB-CHECK-FIXED の一部 — 全体走査 pass の clone と行番号の数え直しを削り、型検査の固定費 8.0 → 6.7 ms** (残りは未実装節)
- **OWNING-TEMP-DROP — `String::from_str("a").len()` / `mk().n` の一時値をスカラーの読み出しの後で drop する** (残りは未実装節)
- **OWNING-TEMP-DROP (所有値の読み出し) — `val r = mk().renamed()` / `String::from_str("a").concat(b)` の一時値を文の前に束縛して drop する (compiled レーンの拒否も消えた)**
- **OWNING-TEMP-DROP (スカラーだけの compound) — `val i = mk().inner` の一時値も drop する**
- **PARTIAL-MOVE — 所有フィールドを 1 つ渡しても束縛の残りのフィールドは読めて、その場で drop する (`val` / 引数 / 消費する method の `self`)**
- **CONSUMING-SELF-NO-DROP (同名の `&self` がある名前) — 全呼び出しのレシーバの型が分かる名前なら `self: Self` の method がレシーバを drop する**
- **CONSUME-THROUGH-BORROW — `&T` 引数 / `&self` の `self` / `&dyn` / generic `&T` / 要素の読み出しに `self: Self` の method を呼ぶと二重 drop していたのを `[E0027]` で拒否。trait impl は `dyn` で使われていてもレシーバを drop する**
- **OWNING-TEMP-DROP (式の途中) — `take(mk().renamed())` / `println(s.concat(..))` / `if peek(&mk())` / `return` / 前にスカラーを評価する引数も束縛する。値が窓として文の外へ出る形は対象外 (aec212f0 の穴を塞いだ)**
- **COMPOUND-BLOCK-DROP-TIMING (呼び出しの中だけで触れる末尾) — `{ val t = mk()  t.renamed() }` の `t` もブロックの終わりで drop、`val r: H = { .. }` のこの形も compiled レーンで通る**
- **OWNING-TEMP-DROP (引数) — `s.concat(String::from_str("x"))` / `peek(&mk())` / `v.push(mk())` の引数の一時値を文の前に束縛する**
- **WINDOW-ESCAPE-UNWRAP — `??` / match の腕 / `if val` で `Option` から出した窓も `[E0026]` で止める**
- **CONSUMING-SELF-NO-DROP (trait impl) — `dyn` で使われない trait の `self: Self` method もレシーバを drop する**
- **NEXT-ITEM-ALIAS — `val m = it.next()` / `match it.next()` / for-in の item に所有者を付けない (二重解放と tree-walker のずれを解消)**

### 2026-10-08

- **CONSUMING-SELF-NO-DROP — `self: Self` の method が受け手を drop する** (残りは未実装節)
- **FIELD-MOVE-DOUBLE-DROP — 所有するフィールドを渡すと根を渡したことにする**
- **LLVM-FRAME-POINTER — LLVM の関数にフレームポインタを残す (`"frame-pointer"="non-leaf"`)。`--profile=compile` が LLVM の 4 段とバックエンド名を出す**
- **AOT-LLVM — AOT の 2 つ目のバックエンドとして LLVM (L0〜L5)**

### 2026-10-06

- **POC に spawn を当てた — `poc/logsearch` のセグメント書き出しがイベントループの外で走る**
- **CONCURRENCY B3 — `Task::as_fd()` で完了を `Poller` に載せられる**
- **CONCURRENCY B2 — `spawn` の本文が compiled レーンでスレッドで走る**
- **CONCURRENCY B1 — `spawn { body }` と `Task<T>`**
- **PAR-HEAP-SHARED — `parallel for` のワーカーが spawner の heap を共有する**

### 2026-10-02

- **BUILD-TOOL D6 — `toy build` / `toy run --backend aot` は入力が変わっていなければビルドを飛ばす**
- **POST-CHECKS-COST — effect 表と region 検査が AST を clone せず借用で読む**

### 2026-10-01

- **LOWER-BINDING-CLONE — lowering の束縛表を undo ログで巻き戻す**
- **CODEGEN-THREADS — codegen のスレッド数を仕事量で決める**
- **LINK-CACHE-FLAP — 同じソースからオブジェクトが毎回違っていた**
- **TYPECHECK-HASH-TYPEDECL — frontend 全体を FxHash に、`TypeDecl` の子を `Rc` で共有**
- **AST-BORROW — AST を clone せずに借用で読む**
- **LEND-SKIP-PRIMITIVE — 読むだけ判定がスカラー引数の本体を歩かない**
- **CLAUDE-CODE T4 — `toy lsp` (stdio の LSP サーバ) と Claude Code プラグイン**
- **COMPILE-CONST-FACTORS — コンパイラの定数倍 5 件**
- **CTFE-LOWER-ROOTS / CRANELIFT-VERIFIER — コンパイル時間の 2 件**
- **MATCH-SCRUTINEE-CALL-AOT — 呼び出しの enum を scrutinee にすると compiled レーンが位置なしで 落ちる**
- **MATCH-TEMP-EXIT-LEAK — 呼び出しを直接 scrutinee にした `match` の payload**
- **FRONTEND-DEAD-CODE — frontend の死にコード ~1050 行を削除**
- **LLM-TOOLING-QUERY-TEXT — `toy query` がパターンと関数値を木から引く**
- **INTERP-STRING-LITERAL — 補間の中の文字列リテラル**
- **CLAUDE-CODE T2 / T3 — 編集直後の自動検査**

### 2026-09-30

- **CLAUDE-CODE C1 / C2 — 権限とスキル**
- **CLAUDE-CODE T1 — stdlib が見つからないことを言う**
- **LLM-TOOLING-PARSE-RECOVERY-HEURISTIC — 壊れた宣言の範囲をパーサが記録**
- **LLM-TOOLING-QUERY-REST — `toy query` の残り**
- **NEWLINE-BINARY-AMP — 行頭の `&` は新しい式**
- **REBORROW-CHECK-GAPS (1) — 値渡しの引数への参照を返す関数を拒否**
- **EXAMPLE-TEMP-PATH-RACE — example の一時ファイル競合**
- **LLM-TOOLING-FIX-REST** — `toy fix --dry-run` はパッケージの複製で全巡を回して報告する (元のファイルは触らない)。
- **PARSER-DECL-END — 宣言ノードの終わり**
- **LLM-TOOLING-PARSE-RECOVERY-REST**
- **LLM-TOOLING-DID-YOU-MEAN-TRANSPOSE**
- **LLM-TOOLING-MAYBE-INCORRECT — 推測の提案**
- **LLM-TOOLING-QUERY-SCOPE — `toy query` の精度**
- **LLM-TOOLING-PATHS — `toy` の JSON 診断をパッケージ相対に**
- **LLM-TOOLING-SCRUTINEE-OPTION — 調査済み**
- **LLM-TOOLING-CASCADE-BY-KIND — 連鎖の判定を印で**
- **LLM-TOOLING-RELATED-REST — 関連箇所の残り**
- **LLM-TOOLING-NO-SPAN — 位置のない型エラー**
- **LLM-TOOLING-E0010-REST — 汎用エラーの分類**
- **LLM-TOOLING #7 — 速度** — 再計測で 1 往復を支配するのは `cargo run` と debug ビルドと確認 (release バイナリ直叩きで 8 倍)。
- **LLM-TOOLING #6 — 回復** — パースエラーがあっても、壊れていない関数は型検査して 同じ回に報告する。
- **LLM-TOOLING #5 — 診断の順序の正規化**
- **LLM-TOOLING #4 — `toy query`**
- **LLM-TOOLING #3 — 関連箇所と主スパンの欠落**
- **LLM-TOOLING #2 — エラーコードの分割**

### 2026-09-29

- **LLM-TOOLING #1 — 機械適用できる修正提案と `toy fix`**
- **LLM-TOOLING L0 — 同一ファイル内の重複宣言を `[E0031]` に**

### 2026-09-26

- **HEAP-CHECK — `toy` の `--heap-check`**
- **HEAP-CHECK H5 — 二重 free をエラーに、example を poison で常時検査**
- **HEAP-CHECK H4 — redzone と `__builtin_heap_poison`**
- **HEAP-CHECK H3 — `--heap-check=reuse`**
- **HEAP-CHECK H2 — compiled レーンの `--heap-check=poison`**
- **HEAP-CHECK H1 — interpreter レーンの `--heap-check=poison`**
- **HEAP-CHECK H0 — 二重 free の棚卸し (`--heap-check=report`)**
- **DOUBLE-DROP-LANE-DIVERGENCE — HEAP-CHECK が見つけた二重 drop を全部潰した**
- **`self: Self` の method がレシーバを消費する**
- **`val v = f()?` の payload を 1 回だけ drop する**
- **tree-walker の列の窓が元を drop しない**
- **tree-walker が `Ptr::get` から束縛した要素を drop しない**
- **借用を match した腕が payload を drop しない**
- **HEAP-CHECK H0b — 二重 free を起こした関数を名指す**
- **BARE-NAME-COLLISION の残り — `pub` の実効化と警告の文言**
- **E0014-WRONG-FILE — モジュールの中の所有権エラーがそのファイルと行を 指す**
- **COMPOUND-FIELD-ARG — compound なフィールドを引数に渡せる**
- **TYPE-NAME-COLLISION — 同じ root の 2 モジュールの同名の型がエラーに なる**
- **QUALIFIER-BARE-FALLBACK — 修飾付き呼び出しが別モジュールの関数に 落ちない**
- **COMPOUND-BLOCK-DROP-TIMING — compound を作るブロックの束縛がブロックと 一緒に死ぬ**
- **CALLEE-DROP-GENERIC — 手渡すだけの generic な受け手は引数を drop する**
- **COMPOUND-BLOCK-RHS の残り — 枝が struct を返す method 呼び出しで 終わる形**
- **USER-TYPE-SHADOWS-GENERIC-PARAM — ユーザ型の名前が stdlib の型引数名と 同じでも stdlib が壊れない**
- **OP-OVERLOAD-ENUM — enum の比較演算子を `eq` / `lt` 等で定義できる**
- **TREE-WALKER-CONCRETE-IMPL — tree-walker も concrete impl の associated function を注釈で選ぶ**
- **JIT-INTERP-COVERAGE (b) の一部 — interpreter JIT が範囲 / 名前 / `@` パターンをコンパイルする**
- **SIMD-F32 の残りは解消済みだった** — format spec (`toy_format_f32`)、 f32 の libm 群 (`math::sqrt_f32` 等)、`math::min_f32` / `max…
- **TYPECHECK-LIES 残 (`str.substring` / `str.split` の compiled 対応) は 不要になっていた**
- **tuple 要素の `Vec` と `enumerate` / `zip` の `collect`**
- **ENUM-TUPLE-SUBPATTERN-AOT — enum variant の中の tuple / struct パターンが compiled レーンで動く**
- **SPAN-RANGE-INTRINSIC — `Span` の範囲演算を呼び出しにしない**
- **ZIP-ITER-GENERIC-SCOPE — method 自身の型引数を turbofish に書ける**
- **FN-NAME-AS-VALUE / HOF-RETURN-UNKNOWN: 関数名を値として渡せる**
- **TRY-OPERAND-GAP: `?` を演算子の項・条件・引数に置ける**

### 2026-09-25

- **LEND-FREEING-CALLEE: 受け手が受け取った引数を drop する**
- **MOVE-REINIT: 丸ごとの代入で所有し直す**
- **RETURN-DROP: enum を返す関数のローカルが compiled レーンで drop されて いなかった**
- **MOVE-CONDITIONAL: 分岐の中の移動を実行時 drop flag で追う**
- **LEND-MUTATING-CALLEE: 何も所有しない場所だけを書き換える受け手も貸し出し**
- **TEST-PARALLEL P6 / TEST-TOOL T4: `toy test --backend all`**
- **NARROW-UNSIGNED-SUB: 符号なし減算は全幅で trap**
- **CONST-ARRAY: struct / tuple の表**
- **SHARED-BORROW-WRITE: 共有の借用を通した書き込みは型エラー**
- **CONST-ARRAY: 表を名前で渡せる** — `&[T; N]` / `&mut [T; N]` (スカラー 要素) の引数が compiled レーンで番地 1 つとして通る (`const` は `.rodata`、ス…
- **RANGE-TYPE-ANNOTATION: 範囲が関数の境界を越える**
- **IRVM-SELF-WRITEBACK-REALLOC: IR VM の `value not defined` は無効な 読み出しだった**
- **`toy new` / `toy init` / `run --backend all` / `test --check`**
- **MEMORY-ACCESS M5: `Vec` / `String` / `Dict` / `Box` が生 builtin を 直接叩かない**
- **UNSAFE-REST 前半: コレクションから `unsafe fn` を外した**
- **TREE-WALKER-GENERIC-SCOPE: closure の型がメソッドの型引数を決める**
- **AOT 実行ファイルの非再現性** — 「run ごとに変わる」のではなく **出力先のディレクトリで変わる**のだった。
- **MEMORY-ACCESS: 旧形 `__builtin_ptr_read(p, off)` を削除**
- **NUM-W-ENUMERATION: 「全スカラー」を名指すリストを述語に寄せた**
- **CODE-SIZE-DIAG-STRINGS: 診断文の共有部分を 1 度だけ持つ**
- **121-Phase-B の残り: allocator を決められる確保は runtime に聞かない**
- **NUM-W-AOT-pack Phase 3: compound 要素の AoS 配列を pack**
- **REF-Stage-2 の残り: ポインタで渡す引数がすべての呼び出しの形に届いた**
- **BREAK-WITH-VALUE: `break <value>` で `loop` を値にする**
- **STRUCT-SUGAR-GAP: struct の省略形と分割束縛**

### 2026-09-24

- **MATCH-STRING-LITERAL: `String` をリテラル腕で match**
- **CONST-UNSUFFIXED-INIT: const の宣言型が初期化子の型を名指す**
- **MATCH-MOVE-OUT-DOUBLE-DROP (+ MOVE-ALIAS-GAP): 別名を渡すと根も渡す**
- **BY-VALUE-PARAM-NO-DROP: 読むだけの受け手への値渡しは貸し出し**
- **`if val` の `else` 無しで本体が `()`**

### 2026-09-23

- **ENUM-STRUCT-VARIANT: enum の struct variant**
- **ENUM-DISCRIMINANT: unit variant の番号と `as`**
- **CHAR-LITERAL-MATCH: 全整数幅を match の scrutinee に**
- **MATCH-CONST-PATTERN: pattern の const 名は値と比較する**
- **STR-ESCAPE-HATCH: `\"` と raw 文字列リテラル**

### 2026-09-22

- **COMPILE-PROFILE: `compiler` / `toy build` の `--profile=compile`**
- **出力の形を選ぶ flag を `--format=text|json` 1 つに統合**
- **`f32` を知らない型の列挙が 5 つあった** — `is_scalar_pointee` 1 つに寄せた (スカラー型を足すときはここから辿る)
- **`println(ps[i].f)` が compiled lane で断られていた**

### 2026-09-21

- **AOT-MATCH-STR-ARM-BLOCK — 名前は型を指していたが、原因は形 だった**
- **NEVER-ALLOCATES-METHOD-STACK — impl の中で修飾子を重ねられる ようになった**
- **TEST-PARALLEL P5 — `test "..." serial { }`**
- **統合が運んでいなかった 3 つ目と 4 つ目 — `call_paths` と `parallel_loops`**
- **MODULE-CONST-PATH — `const` の修飾子が検査されるようになった (`[E0030]`)**
- **モジュールの診断がそのモジュールのソースを出すようになった**
- **DBC-RESULT-FIELD — compound を返す関数が `result` の中身を 契約で言えるようになった**
- **STDLIB-FN-SHADOWED-BY-USER-FN — モジュールの body は自分の モジュールを先に見る**
- **TRAIT-CONTRACT-EXPRREF — モジュールの trait が契約を持てるように なった**
- **CONST-ARRAY — `const K: [u32; 64] = [...]` が全レーンで読めるように なった**
- **MODULE-CONST — モジュールの `const` が見えるようになった**
- **CONCURRENCY A2-b-2 — `parallel for` が本当に並列に走るように なった**
- **E0029 が `break` / `return` と外側の名前への代入も断るように なった**
- **効果解析が `Option::Some(x)` を「本体の無い呼び出し」と見なさなく なった**
- **IR VM がフレームスロットを確保として数えなくなった**
- **CONCURRENCY A2-b-1 — ランタイムが範囲を並列に回せるようになった**
- **予約語を名前に書いたときの診断が、どの語かを言うようになった**
- **CONCURRENCY A2-b の設計を訂正した**
- **ARRAY-REPEAT-LITERAL — `[0u8; 64]` が書けるようになった**
- **MODULE-SYSTEM P3 (`mod.t`) — ディレクトリ自身の名前になった**
- **E0026〜E0030 の `origin_module` は `None` のままでよい**
- **compound 要素の drop glue が `f32` leaf を通るようになった**
- **MODULE-SYSTEM P3 (後半) — 余分なセグメントが解決に参加するように なった**
- **MODULE-SYSTEM P3 (前半) — 書いたモジュールパスが検査されるように なった (`[E0030]`)**
- **CONCURRENCY A2-a — shadow stack が per-thread になった**
- **`toy test` の driver が 1 件も走らずに落ちたとき、理由を捨てて いた**

### 2026-09-20

- **MODULE-FN-REF-ARG は既に直っていた**
- **CONCURRENCY A1 — `parallel for` の意味論が入った (実行はまだ逐次)**
- **BORROW-MATCH-DROP — 借用越しの `match` が payload を解放しなくなった**
- **ENUM-ARG-NESTED-LOWER — `Vec<Option<T>>` が lower されるように なった**
- **ASSOC-FN-REF-ARG — associated function が `&compound` を取れるように なった**
- **MODULE-DIAG-POSITION — モジュール内の診断がファイルと行を言うように なった**
- **ELEMENT-BORROW / CONTAINER-ELEM-DROP — 容器は要素を貸せるようになり、 値で取り出すのは拒否されるようになった**
- **VEC-REPLACE — スロットの中身を入れ替えて、元を受け取れるようになった**
- **MOVE-CHECK-QUALIFIER — move 検査が呼び先を修飾子で引くようになった**
- **MOVE-CHECK-OVERLOAD — move 検査が同名の別署名を取り違えなくなった**

### 2026-09-19 以前

- [`todo_done.md`](todo_done.md) に移した (1 行サマリのまま)。

## 未実装 📋

### 型・所有権 (NEW-TYPE-SYSTEM)

- **MAP-OWNED-ITEM-LEAK: `map` が作った所有値を for-in の item で受けると漏れる** —
  NEXT-ITEM-ALIAS が item を要素の別名として drop しないため、新しい
  `String` を作る `map` は 1 周に 1 個漏れる (`--profile=mem` は
  `core/std/string.t` の行だけを指す)。束縛し直せば解放される。

- **COMPOUND-BLOCK-DROP-TIMING の残り: 末尾が束縛に触れる形** — compound を
  作るブロックの束縛は、末尾の式がそれに触れない (数値フィールドの読みは
  除く) ときブロックの終わりで drop するようになった (2026-09-26)。束縛に
  触れるのがスカラーか新しい所有値を返す呼び出しの中だけ (`t.renamed()` /
  `Some(t.clone())` / `u.name.len()`) の形も同じ (2026-10-11、
  `File::independent_calls`)。残る形 (`match t { .. }` = `?` の desugar /
  ポインタや compound のフィールドの読み / 要素の読み出し) は、別名の可能性があるので compiled レーンでは
  従来どおり外側のスコープの終わりまで生き、tree-walker と順序が割れうる。
  直すなら「ブロックの末尾から外へ出る束縛」を move_check が移動として扱う。

- **CALLEE-DROP-GENERIC の残り: 要素に触れる generic な受け手** ★ —
  generic な受け手が値渡しの引数を drop するのは、本体での出現がすべて
  「丸ごと手渡す / 貸す / `&self` の method でスカラーを読む」ときだけ
  (2026-09-26、`probe_generic_params`)。`get` / `borrow` / `iter` /
  フィールドで要素に触れる本体は、生メモリ経由で要素を写し出したかも
  しれないので従来どおり drop せず漏れる (`Vec::extend(other)` 等)。
  「どの要素を写し出したか」を型で言えるようになるまで (所有の移動を
  `ptr_read` 側に表す) 残る。

- **CONSUMING-SELF-NO-DROP の残り** ★ — `self: Self` の method が `self` を drop する
  ようになった (2026-10-08、trait impl も 2026-10-11。借用越しの消費呼び出しは
  `[E0027]` で拒否) が、対象はその名前の method が全部 `self: Self` のときだけ。
  残り: (b) 同名の
  method を `&self` で持つ型があり、かつその名前を generic / `dyn` のレシーバで呼ぶ
  箇所がある名前 (その呼び出しはどの impl に届くか分からず移動にしないため。型の
  分かる呼び出しだけなら 2026-10-12 に対応)、(c) generic impl で `self` を `var me = self` と別名にしてから要素に触れる形
  (CALLEE-DROP-GENERIC の probe が断る)。どれも二重解放ではなく漏れ。

- **PARTIAL-MOVE の残り** ★ — `val` / 引数 (消費する method の `self` を含む) の
  所有フィールドを無条件に渡すと、そのフィールドだけが移り残りはその場で drop
  する (2026-10-12)。残りは (a) `var` (丸ごとの代入 `w = ..` が旧値を drop するので、
  移ったフィールドを経路で持つ必要がある)、(b) 自前の `impl Drop` を持つ struct
  (Rust と同じく部分移動を禁止するか、今の丸ごと移動のままか)、(c) 分岐の中の
  フィールド移動 (drop flag をフィールド単位に)、(d) 入れ子 (`w.a.b`) と tuple
  (`t.0`)、(e) 別名のある束縛。どれも従来どおり束縛まるごとの移動で、漏れる側

- **UNIT-STRUCT-FIELD: struct のフィールドに `()` を書けない** ★ —
  `struct S { u: () }` が `[E0004] Unsupported operation 'field type in
  struct 'S'' for type ()`。`()` は戻り型 / `val` 注釈 / 引数 / 型引数
  (`Result<(), E>` / `Vec<()>`) / リテラル (`val x = ()`) では**すべて
  書ける**ので、フィールドだけが穴。
  **「門番 1 箇所」ではなかった** (2026-09-21 に着手して戻した):
  型検査の門 (`struct_literal.rs`) と lowering の門
  (`templates.rs`) を開けると、次は `FieldShape` に**局所を持たない
  `Unit` 枝**が要る — enum 側には同じ理由で `PayloadSlot::Unit` が
  既に在り、「leaf 0 個なので値の並びがずれない」という同じコメントが
  付いている。`FieldShape` の分類箇所は **74**。半分だけ開けると
  tree-walker が通して compiled lane が断る形になるので、やるなら
  通しで。実用途 (phantom フィールド、`T = ()` の実体化) を踏んでから

- **UNSAFE-REST: 残る `unsafe fn` (58 本)** — 2026-09-25 にコレクション
  (`Deque` / `Set` / `PriorityQueue` / `SoaVec`)、String の SIMD・`to_str`、
  `hex` / `base64` の encode・decode (`Ptr<u8>::load16` / `store16` を
  intrinsic にして速度は不変)、`path` / `fs` / `time` / `testing` / `io`
  の名残を外し、102 → 59、2026-09-26 に `String::eq` を外して 58。
  **生 builtin の置き場** (`allocator.t` 30 / `span.t` 14 / `ptr.t` 9 /
  `column.t` 3) が 56 で、これは残る側。それ以外は
  `Vec<u8>::extend_bytes` / `String::extend_bytes` (生の `ptr` を受ける —
  呼ぶ側に義務がある API なので `unsafe` が正しい)。`unsafe` の意味を
  「呼ぶ側に義務がある」に変える案 (呼び出しに `unsafe { }` を要求) は
  ユーザ判断待ち。

- **Trait 拡張** ★★★ (大規模、ロードマップ)
  - **A3: trait inheritance (`trait B: A`)** — 中。super trait 経由で `A` の method を `B` impl からも要求。
  - **A4: associated types (`trait Iterator { type Item }`)** — 中〜大。
  - **A5-P3-interp: interpreter 側 JIT の `dyn Trait`** ★ — `ScalarTy::from_type_decl` が `TypeDecl::Dyn` で `None` を返し silent fallback。correctness 問題はなく、compiler 側 JIT が実用的な高速化を担うので優先度は低い。
  - **A5-P4: `Box<dyn Trait>`** — owned trait object + `Vec<Box<dyn Trait>>`。`Box<T>` 自体は `core/std/box.t` にある (`Box<dyn G>` への coercion が無い)。
  - **A5 残作業** — `&dyn Trait` の return / struct field 位置 (REF-Stage-2 の escape rule が阻む)、`dyn A + B`、`dyn Iterator<T>`。

- **CLOSURE-CAPTURE の残: E4 / E5** ★ — 設計は
  [`CLOSURE_CAPTURE.md`](CLOSURE_CAPTURE.md)。**E0〜E3 + E6 は landing 済み**
  (escape しない closure は捕捉した束縛を共有し、escape するものはコピーを
  持って書き込みが `E0021`)。残りは (a) **E4: HOF / escape 越しの可変捕捉** —
  寿命の判断が要るので「実プログラムで踏んでから」、(b) **E5: compiled
  レーンの compound capture** — 診断は直した (capture の話だと分かる文言に
  なった) が、env に compound を載せるのは未着手。interpreter は動く。

- **const generics** ★ — `struct Array<T, const N: usize>`。大規模。

### 構文糖衣 (NEW-FEATURES)

- **小数リテラルのサフィックス必須を緩和** ★ — `1.5` は今
  `[E0034] a float literal needs a type suffix` で `1.5f64` と書くよう案内する
  (lexer が `1` / `.` / `5` に分け、tuple access と区別できないため)。
  緩和するなら lexer に小数のルールを足し、**`a.0.1` (tuple access の連鎖)
  との曖昧性**を Rust と同じく「直前のトークンが `.` なら小数部を読まない」で
  解く (rflex は `zz_marked_pos` を巻き戻せる)。`0..10` は `.` の後が数字で
  ないので影響しない。`f32` が入ったので、サフィックスの無い小数は整数
  リテラルと同じく型を名指す位置から `f32` / `f64` を決め、届かなければ
  `f64` に倒す形になる。

- **STR-INTERP-FMT の残** ★ — (a) user 型に spec を渡す API
  (`Display` の `to_str(&self)` は引数を取らない規約なので、
  `fn to_str(&self, spec: str)` にするかは未決)、(b) fill 文字 / `+` /
  `#` / `$`-parameterised width、(c) interpreter JIT の
  `jit_format_<ty>` helper。いずれも踏んでから。

- **ENUM-STRUCT-VARIANT-PRINT: struct variant の表示にフィールド名を**
  ★ — `println(R::A { x: 1u64, y: 2u64 })` は位置の形 `R::A(1, 2)` で
  出る (struct variant は型検査器が tuple variant に書き換えるため、
  表示器は名前を知らない)。NEWTYPE が「書いた形で出す」のと同じく
  名前つきで出すには、NEWTYPE が手を入れた 2 つの表示器
  (`interpreter/src/object.rs::to_display_string` /
  `compiler_lower/src/print.rs`) に `field_names` を渡す必要がある
  (`--api` の宣言の描画は対応済み)。踏んでから。

- **MATCH-STRING-LITERAL-NESTED: 入れ子の位置の `String` リテラル** ★ —
  MATCH-STRING-LITERAL (2026-09-24) は一番外の腕だけを
  `_ if s.eq_str("a")` に書き換える。`Option<String>` に
  `Option::Some("a") => ..` は `[E0010] literal pattern is only valid
  where a primitive value is expected, got String`。入れ子の位置では
  guard が名指す値が無いので、sub-pattern を束縛 (`Option::Some(__s)`)
  に変えて guard に `__s.eq_str("a")` を足す形になる (or / 複数の
  リテラルが同じ腕にあるときは guard の合成が要る)。

- **COLLECTION-LITERAL: compiled レーンで使えるコレクションリテラル** ★ —
  `dict{...}` は interpreter 限定 (`compiler MVP cannot lower a dict
  literal yet`)、`Vec` のリテラルは無いので、表は `push` の列になる。
  配列リテラル `[a, b]` は動くが**イテレートできない** (`for v in
  [a, b]` は `Method 'next' error for type [u64; 2]`)。`Vec::from`
  相当か、配列の `IntoIterator` のどちらかが入れば「小さな表を 1 行で
  書く」が成立する。

- **CHAR-LITERAL-RETURN / -SIBLING: 戻り位置と `if` の兄弟 arm** ★ —
  `fn f() -> u8 { '+' }` は `expected u8, but got u32` になる。
  CHAR-LITERAL-NUM が型を取る位置は注釈 / 引数 / 比較 / 演算相手までで、
  **宣言戻り型と `if` の兄弟 arm は入っていない** (サフィックス無し数値
  リテラルは両方から取れる — 戻り位置は `visitor.rs` の
  `last == TypeDecl::Number` gate、arm は `propagate_number_subtree`)。
  直すなら (a) その gate を char リテラルにも広げる、
  (b) `propagate_number_subtree` に `Expr::CharLiteral` の leaf を足す
  (if / match の arm を降りる再帰は既にある) の 2 箇所。
  2026-09-03 に `core/std/base64.t` の `symbol()` で踏んで、
  `'+' as u8` / `'/' as u8` で回避した (隣の 3 arm が元から `as u8`
  なので実害は小さい)。
  2026-09-26 の棚卸しで、`if b { '+' } else { 65u8 }` の文言が
  `expected u8, but got ()` と**実際と違う型を言う**ことも分かった
  (`match` の arm 版は `arm 0 is u32, arm 1 is u8` と正しい)。

### compiled レーンの穴

> interpreter (tree-walker / IR VM) では動くが、compiled レーン (AOT / JIT) が断るか
> 遅い形。interpreter 側 JIT の silent fallback もここ。

- **MAP-OWNED-U-INFER: `String` を返す closure を `VecIter::map` に渡せない** —
  `v.iter().map(fn(x: u64) -> String { .. })` は compiled レーンで
  `could not infer method-only generic param U for VecIter::map` になる
  (`val` に束縛しても同じ。`u64` を返す closure は束縛すれば通る)。
  interpreter は動く。2026-10-13、`docs/debugging.md` 6 節の例を書いていて発見。

- **OWNING-TEMP-DROP の残り: 条件つきの位置 / 窓 / ポインタを含む compound** ★ —
  一時値は文の前に `val __temp_N` を出して (前に評価されるスカラーも順に束縛して)
  ブロックの終わりで drop する (2026-10-11、`type_checker/owning_temps.rs`)。
  残りは (a) 分岐の中・`&&` の右 (`if c { take(&mk()) }` の `take(..)` は分岐の
  ブロックの文なので対象だが、式の値として分岐する形)、(b) 一時値を指す窓・
  ポインタを文の外へ持ち出す形 (`val w = first(&mk())`) — 束縛の寿命を窓に
  揃える仕組みが要る、(c) ポインタを含む非所有の compound を読む形 (スカラー
  だけの compound は 2026-10-12 に対応)、generic な struct と enum の読み出し

- **ARRAY-REF-COMPOUND / CONST-ARRAY の残り: compound 要素の配列を渡せない** ★ —
  compiled レーンは `fn f(hs: &[H; 2])` を `cannot lower parameter` で断る
  (値渡しの `[H; 2]` も同じ)。借用で渡せるのはスカラー要素の配列だけ
  (`scalar_array_ref`)。`const` の struct / tuple の表も同じで、読めるのは
  `val r = RS[i]` / `RS[i].w` まで。値の位置に直接置く `f(RS[i])` は束縛を
  案内するエラー、enum 要素の表は未対応

- **JIT-INTERP-COVERAGE (residual)** ★ — interpreter 側 JIT が silent
  fallback する残り: (a) impl block ではなく **method 固有の generic**
  (`fn map<U>(..)`) と **phantom 型パラメータ** (どのフィールドも触れない
  `T` は literal から復元できない、#159 の残)、(b) **struct / tuple
  パターン**と、enum の scrutinee に名前 / `@` を当てる形 (範囲・
  トップレベルの名前・`@` はスカラーの scrutinee なら 2026-09-26 に対応)、(c) **enum の payload 形** — 単一・一様
  スカラーのみなので `Option<Option<T>>` や struct / tuple payload は
  対象外 (compiled 側の同名の制限は JIT-enum-1 で解消済み。こちらは
  `EnumLayout` が別実装)、(d) **enum 型の struct field** (`StructLayout`
  は scalar フィールドのみ)。どれも correctness 問題ではない。

- **160. タプルの JIT 対応 (ネスト)** ★ — `((a,b),c)` と tuple-of-struct。`ParamTy::Tuple(Vec<ScalarTy>)` を tree 構造にする 100+ 箇所の refactor。(inline tuple literal を call 引数に渡す件は 2026-08-23 に CALL-ARG-COMPOUND-LITERAL で解消)

- **195b. `extern fn` の monomorph 化** ★ — generic extern は現状 interpreter の type-erased registry でのみ動く。JIT / AOT には mangled symbol の emit と Rust 側実装の登録が要る。実需要なし。

- **PTR-ABI-LOW-THRESHOLD: 閾値を下げると lane 間で確保の集計が割れる** ★ —
  `PTR_SELF_LEAF_THRESHOLD` (既定 8) を下げて小さな struct もポインタで
  渡すと、閾値 4 では全テストが通り `poc/logsearch` の `__text` が
  −0.7%・`archive` が ~2% 速い (2026-09-25 実測)。だが閾値 2 では
  値は合ったまま**確保の集計が lane 間で割れる**: `SoaVec<Box<i64>>` の
  drop で interpreter レーンが box 3 個を残し JIT は解放する、
  `Vec<String>` の sort で peak が 140 / 145 に割れる、`Vec` の範囲外
  読みで IR VM がレーンから外れる。tree-walker は閾値に依らないので、
  変わったのは lowering を通るレーンの側 (どちらが正しいかは未確認)。
  **閾値 8 でも leaf 9 個以上の所有型で同じことが起きうる**ので、
  閾値を下げる前にこちらを詰める。[`CODE_SIZE.md`](CODE_SIZE.md)。

### 並行性 (CONCURRENCY)

> 設計は [`CONCURRENCY.md`](CONCURRENCY.md)。A (`parallel for`) と B (`spawn` + `Task<T>`、
> B1〜B3) は完了済み。

- **spawn の残り** ★ — (a) 本文の `random()` はスレッドごとの状態 (`ThreadState`) を
  読むので、逐次レーン (親と共有) と答えが割れうる、(b) 捕捉の検査の穴 2 つ: 窓を
  **フィールドに持つ** struct の捕捉と、スコープ付き allocator から来た値の捕捉は
  まだ断らない (CONCURRENCY.md §7)

- **C: チャネルとワーカープール** ★ — B の上。デッドロック、ブロッキング、poller との
  統合が一度に来る (CONCURRENCY.md §4-C)

### 実行時の意味論 (RUNTIME-TRAP)

- **CONTRACT-ELISION の残** ★ — (a) 片辺が literal の形
  (`requires a >= 5u64` で `a - 3u64` の guard を消す — `at_least` が
  param-param のみ)、(b) `x != MIN` の形 (lhs 側の `MIN / -1` 条件)。
  どちらも「実プログラムで書いていて guard がホットパスにある」を
  確認してから。

### 標準ライブラリ・実行環境 (STDLIB-RUNTIME)

> 分野ごとの空白と優先順位は [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md)。

- **STDLIB-GAPS: 関数粒度の空き (純 toylang で今日書けるもの)** ★★ — 一覧と優先順位は
  [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) の「関数粒度の空白」。A (反復子の終端操作) と
  B (`Ordering` と汎用 `min` / `max`) は 2026-10-12 に landing。残りは効きの順に
  F (CRC-32 / Adler-32)、E (`Vec` の `truncate` / `retain` / `dedup` / `swap` / `first` /
  `last` / `fill` / `extend`、`Dict` の `keys` / `values`、`Set` の集合演算、`Deque` の
  `front` / `back`)、C (`Option` / `Result` の `and_then` / `or_else` / `ok_or` ...)、
  D (`split_once` / `strip_prefix` / `Span<u8>` の `starts_with` ...)、G (数学関数の残りと
  f32 の公開関数)

- **MEMORY-ACCESS M4: `chunks::<N>()` と `read_uNN_le/be`** —
  設計は [`MEMORY_ACCESS.md`](MEMORY_ACCESS.md)。M3 で範囲を答える
  primitive は入ったが、**ブロック単位の反復と幅つきスカラー読みは
  まだ手書き**: `hex.t` / `base64.t` は「16 バイトずつ + 端数を
  スカラーで」を自分で書いており (同じアルゴリズムの 2 実装)、
  `base64` / `sha256` は `(b0 as u64) * 65536u64 + ...` で桁を
  組み立てている。`for c in s.chunks::<16>()` が端数を持つ形にすれば
  書き手は 1 回で済み、`s.read_u32_le(i)` は endianness を型の側に
  置ける。

- **STDLIB-CRYPTO C2〜C4: SHA-512 族 / HMAC / SHA-1・MD5** —
  設計と優先順位は [`STDLIB_CRYPTO.md`](STDLIB_CRYPTO.md)。C2 (SHA-512 /
  384 / 512-256) は C1 と同型で lane が u64 になるだけ、C3 (HMAC) は
  `trait Digest` が元を取る場所、C4 (SHA-1 / MD5) は相互運用専用で
  壊れていることを明示する。

- **io.t の範囲外 `""` 既定の厳格化** ★ — `arg(i)` / `env_name(i)` /
  `env_value(i)` は範囲外で `""` を返す (ドキュメント化済みの既定)。
  `arg(i)` の `""` は「実際に空文字列の引数」と区別がつかない。
  `argc()` / `env_count()` で範囲チェックできるので設計上は許容だが、
  厳格化するなら `Result<_, IoError>` 化 (`IoError` に
  `OutOfRange` variant を足す) か `Option` 化。RUNTIME-IO と同じ
  ペア status extern の仕組みで境界変更なしにできる。実プログラムで
  困ってから。

### デバッグ・観測性 (DEBUG-OBS)

> D0〜D6 は完了 ([`DEBUG_OBSERVABILITY.md`](DEBUG_OBSERVABILITY.md))。

- **DIAG-DEBUG-FMT の残: codegen 層** ★ — `compiler_lower` と
  `compiler/src` の parse error は 2026-09-02 に決着したが、
  `compiler/src/codegen/` には `{:?}` が **50 箇所以上**ある
  (`missing import for {target:?}` / `block {b:?} unterminated` /
  `invalid {:?} → f32 cast`)。**大半は internal error** (コンパイラの
  バグを報告する文言で、`FuncId` や `BlockId` こそが必要な情報) なので
  スキャナの対象に入れると偽陽性だらけになる。入れるなら
  「ユーザに見える refusal」と「internal error」を先に分ける必要がある

### コンパイル速度 (COMPILE-SPEED)

> 2026-10-02 時点、release の `toy build poc/logsearch` のリンク前は ~98 ms
> (型検査 29 / lowering 23 / codegen 34 (CPU 117、4 スレッド) / モジュール 6)、
> 実リンク 35 ms。fib.t はリンク前 ~17 ms のうち型検査 11 ms が stdlib の固定費。
> 測り方は `--profile=compile` (COMPILE_PROFILE.md) と、バイナリを交互に走らせる
> 中央値比較。**測って取り分が無かった案** (型キャッシュの sparse clear、lexer
> での intern、単相化検索の clone、型検査器本体の AST 借用化) は再提案しない。

- **LINK-COST: 実リンク ~35 ms (編集後ビルドの 1/3)** ★★ — `cc` ドライバを
  経由せず `ld` を直接呼ぶ / lld、ランタイムの `.rt.a` を毎リンク一時ファイルに
  書き出すのをやめる、`compiler` CLI でもリンクキャッシュを既定 on に。

- **STDLIB-CHECK-FIXED: stdlib の型検査 ~6.7 ms を毎回払う** ★★ — 2026-10-11 に
  全体を走査する小さな pass の無駄 (節点の clone、宣言ごとの行番号計算) を削って
  8.0 → 6.7 ms (`toy check` の空パッケージ、release)。残りの大半は impl block の
  method 本体の検査そのもの (~16%)、moves (~7%)、module の統合 (~7%)。post_checks が
  stdlib 本体について出す事実 (lend 表・エフェクト等) を stdlib のハッシュで
  再利用する軽い版と、PARALLEL_FRONTEND.md §5c のスナップショットに型検査の
  状態まで含める本命。後者は型検査器が AST を書き換えるので大仕事。

- **LEND-WORKLIST: 読むだけ判定の固定点を worklist に** ★ — 今は変化が無くなる
  まで全本体を歩き直す (logsearch で 4 周)。変化した関数の呼び出し元だけ再訪する。
  1〜2 ms の見込み。

- **CTFE-DECLARE-SHARE: CTFE 用 lowering の `declare` ~1.2 ms** ★ — 本番の
  lowering の宣言段と共有すれば消える。

- **PERF-DOC-DRIFT: 文書の数字が実測と合わない** ★ — CLAUDE.md の
  「`TOYLANG_CRANELIFT_OPT_LEVEL` は codegen を ~20x 変える」は logsearch では
  ~10%。測り直して書き換える (テスト時間の数字は 2026-10-12 に直した)。

### インクリメンタルコンパイル

- **AOT の中間オブジェクト (分離コンパイル) は見送り** — 検討の記録は
  [`SEPARATE_COMPILATION.md`](SEPARATE_COMPILATION.md)。削れるのは stdlib の
  固定費 ~6-7ms/回で、同じプログラムの実 `cc` リンクが 55ms。着手条件
  (lowering が全体の 30% 超) と最小設計も同文書に記載。

- **INCREMENTAL-COMPILATION の残** — Phase 1〜5 は完了 (設計と実測は [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md))。統合パスの削減
  (placeholder 2 パス + HashMap → 1 パス + オフセット演算、シンボル翻訳キャッシュ)
  は **2026-08-18 に landing** (integrate 本体 ~43% 削減)。残るのは
  (a) **preparse の deserialize ~1.5ms** (当時 16 ファイル、今の stdlib は 46 モジュール。並列 read + bincode。
  bundle 化 = 1 ファイルにすると invalidation が全モジュール単位になるので
  見送り)、(b) per-module IR compilation + IR linker (warm 19ms のうち ~4ms
  しか狙えないので保留 — 着手するなら、大きめの実プログラムで lowering が
  支配的になることを**再測定してから**)。

### テスト・ドキュメント

- **TEST-PERF / BUILD-PERF** — 2026-08 の測定と施策の記録 (テストバイナリを 1 クレート
  1 本に束ねた理由、`target/` 肥大で再ビルドが 49 倍遅くなった件、外れた仮説) は
  [`todo_done.md`](todo_done.md) に移した。残る削り代は (a) stdlib を使わない
  テストを `test_program_no_core` に寄せる (coverage と引き換え)、(b) 型検査済み
  stdlib のスナップショットをプロセスを跨いで使う (STDLIB-CHECK-FIXED と同じ仕事)。
  suite は wall ではなく CPU 律速なので、効くのは CPU を減らす策だけ

- **TEST-PARALLEL の残り: ジョブの配り方** — P6 (`--backend all`) は
  2026-09-25 に landing (完了済み節)。残るのは配り方だけ: 今は plan 順。
  P4 (`.testtimes` に所要時間を残して長い順に配る) は実装したが**状態を
  増やすので取り下げた**。状態を持たない候補は「自分が検査済みの
  ファイルから優先して取り、空いたら盗む」ワークキュー (VM レーンの
  粒度問題は解けるが、偏ったスイートの順序は解けない)。**効くのは
  `-j` を絞ったときだけ**なので優先度は低い。設計と測定は
  [`TEST_PARALLEL.md`](TEST_PARALLEL.md)。

- **DBC-CHECK-SKIP-REPORT: `--check` が `ptr` レシーバの method を黙って
  飛ばす** — design_by_contract.md には明記があるが、`Vec` のように契約が
  増えるほど「検査されたつもり」が危険。最低限 `SKIPPED` 行を出す。
  本命は構築子 (`new()` + ランダムな `push` 列) 経由でレシーバを生成すること
  で、collection に `--check` を効かせる唯一の道
  ([`VEC_CONTRACTS.md`](VEC_CONTRACTS.md) §5-3)。

- **65. frontend リファクタリング** — (a)〜(g) と死にコードの削除 (2026-10-01) は完了。残: doc コメント拡充、プロパティベーステスト追加。

- **property test の generator が仕様と drift しないか** — `valid_identifier()` は lexer に問い合わせる形にした (2026-08-10)。他の generator (リテラル / 演算子) はまだ手書きなので、同種の drift が起きうる。

- **26. ドキュメント整備** — 残: API リファレンス、advanced topics。

- **AOT-LLVM の残り** ★ — LLVM バックエンドは完了 (完了済み節、[`AOT_LLVM.md`](AOT_LLVM.md))。
  残りは運用: LLVM レーンのテストは `--features llvm` のときだけ走る (既定の
  `cargo nextest run` では走らない) ので、定期的に回す場所を決める

- **LLVM-COMPILE-TIME: `--release` のコンパイルが cranelift の ~24 倍** ★ — `poc/logsearch` で
  1.94 s (cranelift 80 ms)。`-O2` 1.09 s + 機械語 0.77 s で、**1 スレッド**で走る
  (cranelift は関数ごとに全コア)。特定の pass が飛び抜けてはいない。**試作して測った**
  ([`LLVM_PERF.md`](LLVM_PERF.md) §6〜§8): (B) `-O2` の後で分けて機械語だけ並列にすると
  1.4 s で実行時間は不変、(C) `-O2` の前に分けて、小さい関数を分割ごとの internal な写しに
  すると 0.60 s (8 分割) で実行時間 +0.1%、CPU 1.8 倍、RSS 2.4 倍。pipeline を軽くする案
  (`O1` / `Os` / codegen の段の opt を下げる) は効かないか実行時間を払う。段取りは B → C

- **RT-ALLOC-TABLE-COST: 常時引く確保表が確保の多いプログラムで効く** ★ — `free` を冪等に
  するための番地 → サイズの表 (`prof_put` / `prof_take`) が `poc/logsearch archive` の 6%
  (確保 1.54M + realloc 2.30M + free 1.50M、~26 ns/回、表がキャッシュに乗らない)。
  `String::push` の伸長が確保の 1.5 倍の realloc を出しているのも一因。
  [`LLVM_PERF.md`](LLVM_PERF.md) §4

### リファクタリングの残り

> 2026-09-22 に 2 巡目をやった。見つかったものと、やらなかった理由:
>
> - **IR VM のディスパッチをカテゴリ別に分けた** (628 行の match 1 つ →
>   ルータ + 8 関数)。AOT が同じ命令集合に対して既に同じ形をしていたので、
>   **語も揃えた** — 命令を 1 つ足すとき両レーンで同じ名前の関数を見る
> - **`FunctionLower::new` の 22 引数を `LowerCtx` に束ねた**。7 か所に
>   同じ 22 行が並んでいて、表を 1 つ足すと 7 か所 + シグネチャを直す
>   必要があった (実際に 2 回やった)
> - **「スカラーの借用は番地で渡す」の 100 行が 2 か所にあった**のを
>   1 つにした。正規化すると**完全に同一**で、片方だけ直ると静かに
>   食い違う形だった
> - **手書きの型リストが `f32` を知らない箇所が 5 つ**あり、うち 1 つは
>   **cranelift の verifier を panic させる**バグだった (下の完了済み節)
> - **巨大関数のうち網羅 match は触らない**。`writes_locals` /
>   `for_each_operand` / `inst_supported` などは「新しい命令を足したら
>   ここで build が落ちる」ことが目的で、分けると目的を失う。
>   実際 `validate_type_argument` をガードに変えようとして、
>   網羅性が消えるので戻した
>
> 以下は「踏んでから」で保留した分。
> リファクタ時の等価性の確かめ方は
> [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md) の D8 にある。

- **`remap_statement` 286 行** ★ — `Stmt` の variant ごとにフィールドを
  1 つずつ写す構造コピーで、分岐ロジックではない。コレクションの
  remap ヘルパ化は済み。これ以上分けても行が移るだけ。

- **`execute_builtin_method` の引数チェック 4 箇所** ★ —
  `evaluate_builtin_call` は `expect_args` に寄せたが、str メソッド側は
  文言が別系統 (`"concat(str) takes exactly one string argument"`)。
  揃えるとユーザ向けメッセージが変わるので手を付けていない。

## 検討中の機能

> 着手するかどうか、どの形にするかが決まっていないもの。決まって段取りが
> できたら未実装節へ、終わったら完了済み節へ移す。

* **Claude Code 連携 (CLAUDE-CODE-INTEGRATION) の残り** — T1 / C1 / C2 / T2 / T3 /
  T4 (`toy lsp`) は landing (完了済み節)。残りは T5 プラグイン化の仕上げ
  (T4 の LSP 登録だけ `claude-plugin/` に置いた。フック・スキルの同梱と、
  別リポジトリ向けの `toy` の解決が残り)、T6 `toy mcp` (任意)。LSP の
  残り (補完・rename・formatting・増分同期) は
  [`CLAUDE_CODE_INTEGRATION.md`](CLAUDE_CODE_INTEGRATION.md) §6。
* **ヒープ検査モード (HEAP-CHECK) の残り** — H0 (`--heap-check=report`、
  二重 free の棚卸し) と H0b (報告に二重 drop を起こした関数名) は 2026-09-26 に
  landing (完了済み節)、棚卸しで見つかった二重 drop も全部潰した
  (DOUBLE-DROP-LANE-DIVERGENCE)。H1 (`poison`、interpreter レーン)・H2
  (計装、compiled レーン)・H3 (`reuse`)・H4 (redzone / `__builtin_heap_poison`)・
  H5 (二重 free をエラーに)・`toy` の `--heap-check` も landing。残りは
  ブロック内から始まる末尾越えとヒープ外の範囲外 (§11 の残り)。
  [`HEAP_CHECK.md`](HEAP_CHECK.md) §5 / §13。
* **明示 import (MODULE-IMPORTS)** — stdlib も
  `import std.hex` を書かないと使えない形にする提案。
  [`MODULE_IMPORTS.md`](MODULE_IMPORTS.md)。**D1 の alias 束縛だけ
  2026-09-05 に landing** (`import a.b as h`)。残り (可視性の規則 P1 /
  遅延読み込み P2 / 型の名前空間化 P3) は未着手。BARE-NAME-COLLISION /
  TYPE-NAME-COLLISION を「規則」で消し (関数の衝突は呼び出し元優先で
  実害が消え、`pub` も 2026-09-26 に実効化した。型の衝突は同じ root なら
  エラーになったが、名前空間は無い)、hello world の **145ms → 5.7ms**
  (auto-load が 46 モジュール全部を読んでいる分) を取り戻す。
  計測: stdlib のモジュール間依存は 119 辺で**非循環**、prelude を引くと
  足す import は **30 行 / 21 ファイル**、example + poc 200 ファイル側は
  `mod::` を使う 21 ファイルだけ。構文は既に parse を通るので、変えるのは
  意味論。**[`MODULE_SYSTEM.md`](MODULE_SYSTEM.md) の D5 (「暗黙に入る
  集合は stdlib 全部」) を覆す**提案なので、採否は両方を読んで決める。
* `Vec<T>` への Design by Contract 適用 (VEC-CONTRACTS) の**残り** —
  `requires` の 3 行 (§4 の #1〜#3) は 2026-09-04 に landing 済み
  ([`VEC_CONTRACTS.md`](VEC_CONTRACTS.md))。未着手は B (長さ・容量の
  `ensures` + `old`、本命、#4〜#7) / C (`never_allocates` と allocation
  契約、#8〜#11 — #8 を塞いでいた §5-1 の parser の穴は
  NEVER-ALLOCATES-METHOD-STACK で解消し、`Vec::get` は `never_allocates`) /
  E (`is_sorted` helper、#12)。D (擬似 invariant) は B に吸収されるので
  不採用、要素値の契約は generic `T` に `eq` を要求するので不採用。
* **DEBUG-OBS D4 の残: panic に到達しえない関数のフレームを積まない** —
  実測の結論は「作らない」(2026-08-27)。効く場面を踏んだら再検討。
  経緯は [`DEBUG_OBSERVABILITY.md`](DEBUG_OBSERVABILITY.md)。
* FFI — P1 (静的 FFI、`from`/`as`) 完了 (2026-08-16、[`FFI_PLAN.md`](FFI_PLAN.md))。
  P2 (動的ロード / dlopen builtin) は未着手
* フロントエンドの並列化 (PARALLEL-FRONTEND) — **今は着手しない**。検討と
  実測の記録は [`PARALLEL_FRONTEND.md`](PARALLEL_FRONTEND.md)。stdlib の
  pre-parse (rayon) と AOT codegen は既に並列で、取り分は warm 0.2ms /
  cold 1.5ms。残る候補は 1 ファイル内の並列 parse (22k 行で 36ms → 9.2ms を
  実験で確認、ただしマージ ~0.5µs/行 を含まない) と関数単位の並列型検査
  (22k 行で 10.7ms、ただし型検査器が AST を書き換えるのをやめるのが前提)。
  着手条件は「単一ファイルが 5,000 行を超える実プログラム」+「1 実行
  あたりの固定費 5.4ms を先に片付けてあること」。同文書の §5 に
  **ビルドサーバ案の測定**もある — 常駐サーバで消せる 7.2ms/プロセスは
  **統合済みスナップショット 1 ファイル (cold で 0.95ms ロード、319KB)**
  でも落ちるので**サーバは作らない**。スナップショットの方は 1 テスト 1
  プロセスのテスト群にも効くので、INCREMENTAL-COMPILATION 側の候補
* データ指向の配列 layout (DOD) ★★ — Phase 0 (`soa [T; N]` + `ps[i].f`
  単列 shortcut) と Phase 2 (`soa Vec<T>` → `SoaVec<T>`) は 2026-08-30、
  Phase 0.5 (列 tight pack) / Phase 1 (列の窓 `Column<T>`) /
  Phase 3 (配列要素としての enum) は 2026-08-31 landing 済み (上) —
  **設計 doc の Phase は全部埋まった**。設計は
  [`DATA_ORIENTED.md`](DATA_ORIENTED.md)。残っているのは 1 つ:
  * **tag 専用 scan の読み手** (小〜中)。Phase 3 で `soa [Shape; N]` の
    tag は独立した列になったが、`val s = ss[i]` は全 leaf を読むので
    「どの variant か」だけを舐める形がまだ書けない。(a) payload を
    束縛しない match が tag だけを読む最適化、(b) tag 列に名前を与える
    (Phase 1 の `Column` を discriminant に向ける) のどちらか。
    どちらも設計を決めるのが先

  未決 (Phase に紐づかない): **要素まるごとの書き込み `ps[i] = p`** は
  leaf ごとに散った store になるので、要素単位更新が主のワークロードでは
  SoA が AoS より遅い (enum 要素だけは代替が無いので Phase 3 で許可した)。
  警告を出すかは未決で、`--simd-report` と同じ「聞けば答える」tooling 側に
  置くのが妥当 (DATA_ORIENTED.md の論点 1)
* SIMD Phase 3 の残 / Phase 4 ★★ — Phase 2 (型 + 演算子 + intrinsic) と
  戦略 B の主要 kernel は landing 済み。`__simd_bitmask` /
  `__simd_swizzle` / `__simd_bitcast` / `__simd_shuffle` も入った
  (2026-09-03、**intrinsic の穴は無し**)。hex / base64 の 4 カーネルも
  SIMD 化済み。残りは (a) **stdlib の残り kernel**
  — `v.iter().sum()` / `min()` / `max()` (STDLIB-ITER-TERMINAL で API は入った。中身は逐次)、
  `Vec<T>::sort` の小配列部分、
  (b) `--simd-report` (「なぜベクトル化されなかったか」を聞ける CLI)、
  (c) 限定自動ベクトル化、(d) **256bit + runtime dispatch** — baseline を
  超えるのはここが最初で、cranelift の ISA フラグはモジュール単位なので
  関数の multi-versioning をどう作るかが論点 (SIMD.md 論点 2)。設計は
  [`SIMD.md`](SIMD.md)
* **ビルドコマンド `toy`** — [`BUILD_TOOL.md`](BUILD_TOOL.md)。
  B0〜B4 は landing 済み (完了済み節)。残る B5 (マニフェストと依存) は
  依存が来るまで作らない。
* モジュール拡張 — バージョニング、リモートパッケージ
* 言語内からの AST 取得・操作

## 現状の把握

> 言語仕様は [`../docs/language.md`](../docs/language.md) が正本、日常的に踏む
> 要点は [`CLAUDE.md`](../CLAUDE.md)、実装の場所は
> [`CODE_MAP.md`](CODE_MAP.md) にある。
> **機能一覧をここに再掲しない** — 3 重管理になり、実際に食い違った
> (この節は `String` を「`Vec<u8>` alias」と書き続けていたが、
> 2026-05-08 に nominal struct へ変わっていた)。

### テスト状況
- 合計 **3363 テスト** (100% 成功、2026-10-12 時点、`cargo nextest run`)。
  `--features llvm` では compiler / toy の 1182 本が、AOT レーンを LLVM でも
  作って cranelift と突き合わせる (AOT_LLVM.md)。
- 内訳: interpreter unit + integration、frontend unit、compiler e2e + consistency。後者は
  tree-walker / IR VM / JIT / AOT の一致を保証する。
- ワークスペース全体で **~19s** (2026-10-12 実測、warm、nextest の既定 profile 出力)。
  2026-08 時点の内訳とビルド時間の記録は [`todo_done.md`](todo_done.md) の TEST-PERF /
  BUILD-PERF。
  `compiler/build.rs` が `toylang_rt` を rustc で staticlib pre-build し、リンク結果は
  `TOY_LINK_CACHE_DIR` で content-addressed にキャッシュされる (キャッシュが効くには
  コード生成が決定的である必要がある — `compiler/tests/reproducible_build.rs` が pin)。

### 既知の不具合

- **CACHE-DIR-RACE: schema を上げた直後の `toy test` が
  「failed to save module cache」を出す** ★ — `.toycache/<2桁>/` を
  `create_dir_all` で作ってから `rename` するのに、その間に
  ディレクトリが消えて `No such file or directory` になる
  (2026-09-21 に 2 回観測、どちらも schema bump の直後 = 全エントリが
  書き直される回)。`.toycache` は**カレントディレクトリ相対**なので、
  並列ジョブが別の cwd を掃除しているのが筋。無害 (キャッシュミスに
  落ちるだけ) だが、bump のたびに出る。直すなら ENOENT のとき
  ディレクトリを作り直して 1 回だけ再試行する — ただし**誰が消して
  いるか**を先に確かめること。

- **PARALLEL-CAPTURE-WRITE-LANE: 捕捉を変える呼び出しを断るのが
  compiled レーンだけ** ★ — `parallel for` の本文が外側の束縛に
  **書く method を呼ぶ** (`v.push(x)`) と lowering が断り、
  tree-walker は通す。`push` が `v` に書くことは callee の
  シグネチャを読まないと分からないので、型検査器に同じ検査を置くと
  近似の二重実装になる。「インタプリタで動いた形が AOT で落ちる」形の
  1 つ ([`CONCURRENCY.md`](CONCURRENCY.md) の A2-b-2 節)。

- **NUM-W-SHIFT: narrow int の `<<` / `>>` が型検査で拒否される** ★★ —
  `u8 << u8` も `u8 << u64` も拒否される (2026-10-12 に再確認、`x >> 3u8` は
  「incompatible types u64 and u8」)。
  `&` / `|` / `^` は全幅で動く (2026-09-01 に tree-walker 側を修正) ので
  shift だけが取り残されている。2026-09-01 に NET N3 のテストで踏んだ。
  **拒んでいるのは lhs の幅**で、`u32` でも同じ (`x >> 3u32` は
  「u64 and u32」、`x >> 3u64` は「u32 and u64」)。`docs/language.md:1249`
  の「rhs must be `u64`」は片方しか書いていない。`Bits` が
  `rotate_left` / `rotate_right` を全 8 幅に提供しているのと非対称。
  2026-09-04 に STDLIB-CRYPTO で再度踏んで ★ を上げた: 回避の
  `((x as u64) >> n) as u32` は**32 以上の shift が trap せず 0 になる**
  という別の意味論を持ち込むので、`core/std/crypto/sha256.t` の
  `shr32` / `shl32` は `requires n < 32u64` でそこを埋めている
  ([`STDLIB_CRYPTO.md`](STDLIB_CRYPTO.md) 実測 1)。

- **TREE-WALKER-DYNAMIC-GENERIC-SCOPE — 呼び出し先が呼び出し元の型引数を
  見る** — tree-walker の `merged_generic_scope` は**実行中の全呼び出し**の
  scope を合わせたもの (動的スコープ) なので、generic でない関数の中でも
  呼び出し元の `T` が見える。phantom パラメータ (`Ptr<T>` の `T`) は
  struct リテラルが scope から取るため、`Vec<String>::clone` →
  `String::push` の中の `Ptr { addr: .. }` が `Ptr<String>` になって
  いた (M5 の `String` 移行で発覚)。**`val` の注釈があれば注釈を優先する**
  ところまでは直した (2026-09-25)。注釈の無い phantom リテラルは
  まだ呼び出し元の `T` を拾う。直すなら関数・メソッドの呼び出しで
  scope を**積むのではなく差し替える** (closure の本体だけは書かれた
  関数の scope を持ち込む)。

- **TYPE-NAME-COLLISION の残り: 型の名前空間** ★ — 同じ root の 2 モジュールが
  同じ型名を宣言すると、黙って上書きせず `type \`Item\` is declared by more
  than one module: src/a.t, src/b.t` のエラーになった (2026-09-26)。残りは
  (a) **root をまたぐ衝突** (パッケージのモジュールと stdlib が同名の型を
  持つ形。entry の型だけは `__std_<name>` の別名で共存できる) と、
  (b) 衝突を許す本当の解決 = 型の名前空間化 (MODULE-IMPORTS P3)。
- **ALIAS-ROOT-BY-NAME: move 検査の別名の根が名前で引かれる** ★★ — `var poller = match made { .. }`
  の `poller` は `made` を根に持つ別名として記録されるが、根は**名前**で引かれる。同じ関数の
  内側のスコープで `val made = f()` / `if val Option::Some(base) = made` と書いて `base` を move
  すると、外側の `made`、つまり `poller` まで move 済みになり、無関係な行に E0014 が出る
  (2026-10-07、POC に spawn を当てて踏んだ。名前を変えて回避)。根を宣言 (`StmtRef`) で持てば直る。
- **OPTION-IS-SOME-CONSUMES: `Option::is_some` / `is_none` が `self: Self`** ★ — 所有型を入れた
  `Option` は `o.is_some()` と訊くだけで move され、以後の読みが E0014。`&self` にすべき
  (`Result` の同名も確認)。回避は `if val Option::Some(t) = o { .. }`。

- **USER-TYPE-NAMED-T の残り: ユーザの型名 `T` の struct リテラルを compiled レーンが推論できない** ★ —
  `struct T<X> { .. }` を宣言すると stdlib の generic 本体 (`vec.t` の `val e: T = ..`) が
  `[E0028]` / `[E0014]` で落ちていた (2026-10-08) のは、2026-10-12 の再確認では型検査を
  通る。残りは `val t = T { x: 3u64 }` を compiled レーンが「generic struct `T` には注釈が
  要る」と断ること (`T<u64>` と注釈すれば通り、同じ形を `W` と名付ければ注釈なしで通る)。
  型パラメータ `T` との取り違えが lowering 側に残っている

- **ENUM-CALL-VALUE-COUNT (internal error)** ★ —
  `internal error: enum call returned 19 value(s), expected 15`。
  自由関数が `&mut` の compound を 2 つと `&` を 1 つ取り `u64` を返す形で
  出た (`poc/logsearch` のアーカイブ書き出し)。**最小再現は取れていない** —
  7 フィールド struct を `Result` で返す形は通る。`&mut` を 1 つに
  減らしたら消えたので、writeback の leaf 数の数え方が疑わしい。
  internal error なのでユーザ側に直し方の手掛かりが無い。

- **行末の識別子と、次の行頭の `(` が改行を跨いで呼び出しになる** —
  セミコロンが無いので `val prod = a * b` の次行が `(prod >> 17u64) & mask`
  だと `b(prod >> 17u64)` と読まれる (JavaScript の ASI と同じ罠)。規則は
  そのまま。診断は 2026-09-26 に直した — 呼ばれた名前がスコープ内の値なら
  「`b` は値で関数ではない、行末の `b` と次行頭の `(` は 1 つの呼び出しに
  なる」と言う (以前は `Function 'b' not found` と別の関数への置換提案)。
- bare `self` 非対応 — `self: Self` / `&self` / `&mut self` のいずれかを書く。
- `else if` 非対応 — `elif` を使う。
- `val` はキーワードなのでパラメータ名に使えない。
- 関数のネスト定義 (`fn` の中の `fn`) は不可 — closure (`fn(x: T) -> R { ... }`) を使う。
- デフォルト引数 / 名前付き引数は不可 (`f(a: u64, b: u64 = 1u64)` / `f(a: 1u64)`)。導入予定も無い。
- `extern fn` の generic params は parser では受理されるが、JIT / AOT が per-instance シンボル名を持たないため interpreter でのみ動く (`#195b`)。
- `package` 宣言 / `import` path のセグメントに primitive type キーワード (`i64` / `f64` / ...) は使えない (`core/std/str.t` が `package` 宣言を省いているのはこのため)。
- 関数名に primitive type キーワードは使えない (`fn f64(...)` は `expected function name`)。
