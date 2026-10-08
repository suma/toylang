# TODO - Interpreter Improvements

## 完了済み ✅

> **この節は 1 行サマリだけを持つ。** 実装の経緯・測定値・ファイルパス・
> テスト数は git log のコミットメッセージにある。フェーズ設計は
> [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md) /
> [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md) /
> [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md) /
> [`FEATURE_NOTES.md`](FEATURE_NOTES.md) を参照。
> ここを段落で埋めると、常時読まれるファイルが changelog になる。

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

### 2026-09-19

- **MATCH-PAYLOAD-COPY — `match` の腕は payload を名指すのであって 複製しない**

### 2026-09-18

- **STR-PTR-UNCOUNTED — `str::as_ptr` が確保カウンタを動かさなくなった**

### 2026-09-17
- **RANGE-FOR — `for i in r` が範囲値で動き、範囲値が 3 レーンに 入った**
- **MODULE-EXPR-REMAP — モジュールの body に配列の添字を書くと integration が落ちていた**

### 2026-09-11
- **TEST-PARALLEL P0〜P3 — `toy test` が並列に走る**

### 2026-09-06
- **TRY-COMPOUND / COMPOUND-BLOCK-RHS / COMPOUND-GENERIC-INSTANCE — `?` が compound を運べるようになった**

### 2026-09-10
- **REF-REBORROW の残り 2 経路 — module 呼び出しと generic 推論**

### 2026-09-05
- **toylang 側のリファクタリング (stdlib / poc)**
- **REF-REBORROW — `&mut` 引数の転送に借用を書かなくてよくなった**
- **BY-VALUE-SELF-ALIAS — 値渡しの引数は callee 自身のコピーになった**
- **CODE-SIZE-SELF-ABI — 演算子オーバーロードと method の参照引数も番地で渡す**
- **CODE-SIZE-SELF-ABI S3b — 幅の広いローカル束縛を stack slot に常駐**
- **CODE-SIZE-SELF-ABI S3a — `&T` / `&mut T` の compound 引数もポインタで渡す**
- **CODE-SIZE-SELF-ABI S1+S2 — 幅の広い by-reference receiver をポインタで渡す**
- **CODE-SIZE-WB-PRUNE — `&mut self` が書かない leaf を返さなくした**
- **リファクタリング (frontend / compiler / interpreter)**
- **MODULE-IMPORTS D1 — `import a.b as h` が効くようになった**
- **`toy version`** — toy / compiler / interpreter / stdlib の version + git revision + **パス**を 1 行ずつ。
- **TEST-TOOL T3 — `core/std/testing.t`**
- **TEST-TOOL T4 — `test "..." panics { }`**
- **TEST-TOOL T5 — golden ファイルと `toy test --bless`**
- **TEST-TOOL T1 — compiled レーンで `test` が走るようになった**
- **TEST-TOOL T2 — `main` の無いファイルの AOT が無関係なエラーを出す件**
- **`--test` が IR VM で全テストを黙って pass させていた**
- **`toy clean`** — `build/{debug,release}/` を消す。
- **`toy` の出力レイアウトを決めた** — `build/{debug,release}/` で profile を分ける (`--release` は契約を消すので**別のプログラム**であり、 同じパスだとディスク上…
- **`toy build` / `check` / `test` の既定を AOT にした**
- **BARE-NAME-COLLISION — 後の module root が bare 名を勝ち取るようにした**
- **`ambiguous` の診断が用途で分かれた** — bare 呼び出しに 「Two modules cannot share a file name — rename one of them」と 言っていたが、`std::bas…
- **TEST-TOOL T0 — module の中に `test` ブロックを書けるようにした**
- **BUILD-TOOL B2 — `toy test`**
- **BUILD-TOOL B4 — bare 名の衝突を事前に報告する**
- **BARE-NAME-COLLISION の実例が 3 件見つかった**
- **BUILD-TOOL B0 — `--core-modules` を繰り返し指定できるようにした**
- **BUILD-TOOL B1/B3 — `toy` コマンドを追加した**
- **`Bits` / `Checked` も借用にした**
- **`Ord` / `Hash` と container の読み取りメソッドを借用にした**
- **STRING-NO-DROP — `String` が自分のバッファを解放するようにした**
- **IR VM の `value not defined` が場所を言うようになった**
- **STDLIB-FS-HANDLE — `fs::File` (開いたファイル) を入れた**
- **`Result<(), E>` を `val` に束縛すると lowering が panic した**
- **METHOD-ARG-UNCHECKED — method 呼び出しの引数を型検査するようにした**
- **METHOD-MUT-PARAM-REBORROW — method が自分の `&mut` パラメータを 再借用できるようになった**
- **MEMORY-ACCESS M3 — `Span<T>` の範囲演算**
- **MEMORY-ACCESS M2 — stdlib の読み出し 130 箇所を `::<T>` 形へ移行**
- **MEMORY-ACCESS M0 — `mem_move` / `mem_set` が全バックエンドで動く ようになった**
- **MEMORY-ACCESS M1 — `__builtin_ptr_read::<T>(p, off)`**
- **IRVM-BOOL-STRIDE — IR VM が bool 配列を 8 バイト間隔で書いていた**

### 2026-09-04
- **JSON-RESULT-READER — json の reader が `Result` を返すようになった (`json::parse` が設計どおりの入口になった)**
- **MODULE-TRY-REMAP — stdlib の body に `?` を書くと integration が 落ちていた**
- **TRY-STDLIB-ALIAS — user が `Result` / `Option` を影にすると stdlib の `?` / `??` が壊れる**
- **STDLIB-CRYPTO C0/C1 — SHA-256 / SHA-224 (`core/std/crypto/`)**
- **WIDE-RETURN — 戻り値の leaf が返却レジスタを超える compound を compiled lane が返せるようになった**
- **VEC-CONTRACTS #1〜#3 — `Vec<T>` の境界を `requires` にした**

### 2026-09-03
- **STDLIB-NUMERIC 完了 (N0〜N6) — 残っていた `shuffle` / `checked_pow` / `clamp_f32`**
- **UNBOUNDED-GENERIC-PARAM — bound の無い `<T>` が「宣言されていない」 扱いだった**
- **QUALIFIED-GENERIC-CALL-SCOPE — module 修飾の generic 呼び出しが 呼び出し側の束縛を消していた**
- **QUALIFIED-GENERIC-CALL-LOWER — 同じ呼び出しが lowering でも 落ちていた**
- **TYPECHECK-BODY-KEY — 同名の module 関数の body が丸ごと未検査だった**
- **CODEC-SIMD — hex / base64 の 4 カーネルを SIMD 化**
- **SIMD-INTRINSIC-4 — `__simd_bitmask` / `__simd_swizzle` / `__simd_bitcast` / `__simd_shuffle` (intrinsic 13 → 17、**
- **STDLIB-SERIALIZE S1/S3/S4/S5 — JSON (`core/std/json.t`)**
- **STDLIB-SERIALIZE S0/S2 — hex / base64 (`core/std/hex.t` / `base64.t` / `codec.t`)**
- **STDLIB-LOG — レベル付きログ (`core/std/log.t`)**
- **STDLIB-FS-PATH — path 操作とファイルシステム (`core/std/path.t` / `fs.t`)**
- **STDLIB-TIME — 単調時計・sleep・CPU 時間・日付 (`core/std/time.t`)**
- **GENERIC-SCALAR-REF 解消 — `&T` が primitive でも通る**
- **RETURN-LOCAL-DROP (tree-walker) 解消 + `Vec` / `Box` の `Clone`**
- **PTR-READ-ASSIGN 解消 + `str` の比較演算子 (STDLIB-ORD)**
- **STDLIB-TRAIT-BASE B5 — 戻り位置からの型引数推論と `Default`。 これで B0〜B5 すべて landing**
- **STDLIB-TRAIT-BASE B0 / B2 — stdlib の反復子が trait を名乗るように なった**
- **STDLIB-TRAIT-BASE B1 / B3 / B4 — bound を書いた先で何かできるように なった**
- **STDLIB-TEXT T3〜T5 — テキストの分野が完了**
- **STDLIB-TEXT T0〜T2 + STDLIB-ORD (`str`) — `str` / `String` の境界が 決着**
- **STDLIB-ERROR-MODEL E0〜E5 — 失敗の運び方が決着**
- **COLLECTIONS C5 — `PriorityQueue<T: Ord>` (`core/std/collections/priority_queue.t`)**
- **COLLECTIONS C4 — `Deque<T>` (`core/std/collections/deque.t`)**
- **COLLECTIONS C3 — `Vec` の `insert` / `remove` / `swap_remove` / `contains` / `index_of` / `reverse` / `sort_by`**
- **COLLECTIONS C2 — `Set<T>` (`core/std/collections/set.t`)**

### 2026-09-02
- **BUMP-CHUNK-OVERSIZE — 1 MiB を超える確保がチャンクをはみ出していた**
- **COLLECTIONS C1 — `Dict` が hash 表になった**
- **TREE-WALKER-SIZEOF-STR — `__builtin_sizeof` が `str` 値に答えるようになった**
- **COLLECTIONS C0 (a) — generic な `==` の相手に `eq` が無いと型エラー (`E0010`)**
- **COLLECTIONS C0 (b)(c) — hash の土台**
- **WINDOW-ESCAPE — 窓がバッファより長生きできなくなった (`[E0026]`)**
- **MUST-USE — 捨てられた `Result` を警告するようにした (`[E0025]`)**
- **OP-OVERLOAD-CHAIN — overload の結果が普通の値になった**
- **TREE-WALKER-SELF-TYPE-ARG — 宣言戻り型が `Self` の型引数を名指すようにした**
- **ENUM-ASSOC-FN-PRODUCER — enum を返す associated function を enum 生成位置に書けるようにした**
- **SUBDIR-ASSOC-FN は解消済みだった** — サブディレクトリの `core/std/collections/vec.t` から `Span::from_parts` / `Ptr::try_from_raw` が呼べない…
- **NUM-W-FOR-RANGE — narrow int の `for` 範囲が 4 レーンで一致した**
- **COMPOUND-ARG-CALL — compound を返す呼び出しを引数位置に書けるようにした**
- **CHAR-LITERAL-GENERIC-ARG — レシーバが決めた型引数を パラメータ型に流すようにした**
- **DIAG-SYMBOL-NAME-LOWER — lowering の診断も名前を綴るようにした**

### 2026-09-01
- **ENUM-ARG-NEST — enum を返す呼び出しを引数と payload に書けるようにした**
- **ENUM-VARIANT-ARG — enum の構築を引数位置に書けるようにした**
- **代入は `Unit` — ブロックの末尾に置いても値を持たない**
- **NUM-W-ENUMERATION — レシーバ表の 6 コピーを 1 つにし、隠れていた JIT の符号バグを 3 件出した**
- **DIAG-SYMBOL-NAME — 診断が名前を綴るようにした**
- **NET N5 — 名前解決。これで NETWORK_IO.md の N0〜N5 が全部埋まった**
- **NET N4 — UDP / アドレス / socket option**
- **NET N3 — Poller (epoll / kqueue の統一形)**
- **narrow int のビット演算が tree-walker で落ちていた**
- **NET N2 — TCP server (`TcpListener`)**
- **COMPOUND-BLOCK-RHS — `match` から compound を取り出せる**
- **UNIT-TYPE-ARG — `Result<(), E>` / `Option<()>` が 4 レーンで動く**
- **NET N1 — TCP client (tree-walker のみ)**
- **EXTERN-BUF の借用が typed slot を見ていなかった**
- **impl メソッドの move が解析されていなかった**
- **IMPL-BLOCK-VISIBILITY — impl メソッドから stdlib が見えるようになった**

### 2026-08-31
- **RUNTIME-TRAP-NARROW — `checked_*` / `saturating_*` が全 8 幅で使える**
- **NET N0 — プラットフォーム切り替えの足場** — `#[cfg_attr(path)] mod sys;` 1 箇所で epoll / kqueue を選び、未対応 OS は `compile_error!` で落ちる。
- **EXTERN-BUF — `extern fn` が toylang のメモリに届く**
- **CONV-SPAN — 既にあるバッファを `Span<T>` として見られるようになった**
- **primitive レシーバの method call が全幅で動く**
- **GENERIC-IN-ENUM-PAYLOAD / SELF-IN-TYPE-ARG — `Option<Ptr<T>>` が 書けるようになった**
- **DOD Phase 3 — 配列要素としての enum + tag 列**
- **DOD Phase 1 — 列の窓 `ps.mass`**
- **DOD Phase 0.5 — 列の tight pack**

### 2026-08-30
- **DOD Phase 0 — `soa [T; N]` landing**
- **DOD Phase 2 — `soa Vec<T>` landing**
- **STDLIB-FREE-FN-UNCHECKED 解消 — stdlib の free function body も型検査する**
- **CHAR-LITERAL-NUM — char リテラルが位置の整数型を取る**
- **`fn f() -> ()` が書けるようになった**
- **RUNTIME-LIB P0-B — `core/std/parse.t` (`parse::to_u64` / `to_i64` / `to_f64` / `to_bool`)**
- **RUNTIME-LIB P0-A — io 書き込み系 (`write_file` / `append_file` / `eprint` / `eprintln` / `io::exit`)**
- **POINTER P6 — `unsafe fn` の宣言と強制 (`[E0024]`)**
- **POINTER P5 — `Ptr<T>` の non-null 不変 + `Option<Ptr<T>>`**
- **POINTER P4 — `core/std/span.t` の `Span<T>`**
- **POINTER P3 — `core/std/ptr.t` の `Ptr<T>`**
- **POINTER P2 — `__getitem__` / `__setitem__` の 2 バグ + compiled レーン dispatch**
- **POINTER P1 — `__builtin_sizeof::<T>()` 型引数形**
- **SIMD-VM-SLOT — 測って払うと決めた** — IR VM の `RawSlot` 8 → 16 バイト化の代償を 4 ワークロードで実測: call 中心 +2.0% / ループ +5.3% / struct −0.5%…
- **SIMD Phase 3 (戦略 B) — stdlib kernel を SIMD 化**
- **AOT の ISA を baseline 固定に** — `make_object_module()` が `cranelift_native::builder()` (ビルドマシンの CPU 機能を検出) から `isa::look…
- **SIMD Phase 2 — 128bit vector を型にし、演算子を lane-wise に効かせた**
- **SIMD-F32 の残 (一部)** — IR VM の `to_string_value` に `F32` の行が無く、`"{x}"` が生ビット (`1069547520`) を出していた。
- **SIMD-F32 — `f32` を primitive 型として追加 (SIMD.md 論点 1 解決)**
- **NULL-COALESCE — `a ?? b` 演算子**
- **FROM-INTO-ENUM-ERR — `?` の cross-error 変換が enum エラー型でも 3 バックエンドで動く**
- **RUNTIME-IO — `read_file` / `env_var` / `read_line` が `Result<_, IoError>` を返す**
- **TRY-ERR-RETYPE — `?` が success 型の変更を跨げる + `return` の型検査**

### 2026-08-29
- **ENUM-EQ-ESCAPES-TYPECHECK**
- **METHOD-ARG-AUTOBORROW** — frontend が認める `T` → `&T` の auto-borrow を lowering が実体化していなかったので、値がポインタのスロットに入り、**IR VM は…
- **注釈の有無で operator overload の到達可否が変わっていたのを修正**
- **`&Self` / `&mut Self` を compiled レーンが lower できるようにした**
- **COMPOUND-ASSIGN-BITWISE** — ビット系の複合代入 5 種 (`&=` / `|=` / `^=` / `<<=` / `>>=`) を足した。
- **戻り型を書かない `main` の panic を修正**
- **DBC-LISKOV (E0023)** — trait method の `impl` が自分の `requires` を足すのを型検査で拒否するようにした。
- **CONTRACT-ELISION 拡張 (制御フロー)**
- **REGION Phase 1 (E0022)** — スコープ付き allocator (`with allocator = arena { ... }`) から確保したメモリが arena より長生きする形を型検査で拒否する。
- **EFFECT-SYSTEM** — 到達可能性で判定する 3 つの検査 (`never_allocates` / `const fn` / 契約の純粋性) が各自持っていた「禁止 builtin」テーブルを 1…

### 2026-08-28
- **CLOSURE-CAPTURE E0〜E3 / E5 (診断) / E6**

### 2026-08-27
- **DEBUG-OBS D3 の残: `HeapAlloc` の `SiteId` 移行**
- **DEBUG-OBS: tree-walker への replay を落とした**
- **DEBUG-OBS: 値を持つ文言を全実行系に** — D0 の目標表が決めていた 3 件 (`u64` underflow の `1 - 5` / 配列 OOB の `index 5, length 3` / 契約違反の `(wi…
- **DEBUG-OBS D6: 再帰深度と stdlib の境界**
- **DEBUG-OBS D5: ユーザ API と機械可読出力**
- **DEBUG-OBS D4: shadow stack**
- **DEBUG-OBS D3: IR の `SiteId`**
- **DEBUG-OBS D2: `FileId` と `SourceMap`**
- **DEBUG-OBS D1: interpreter の backtrace の穴埋め**
- **DEBUG-OBS D0: 診断の比較レーン** — 実行時の失敗の**文言**を突き合わせるレーンを `compiler/tests/consistency/diagnostics.rs` に作り、目標文言を `DEBUG_OB…

### 2026-08-26
- **COMPILE-TIME-EVAL C5: 配列長に `const` / `const fn` / 式**
- **COMPILE-TIME-EVAL C4: 契約との接続 + warning 機構**
- **COMPILE-TIME-EVAL C2: IR の定数畳み込み**
- **COMPILE-TIME-EVAL C6: 評価器の一本化**
- **COMPILE-TIME-EVAL C0/C1/C3: `const fn`**
- **COMPOUND-BLOCK-RHS: struct / tuple を産む composite を `val` の右辺に**

### 2026-08-25
- **MATCH-STRUCT-ARM: composite tail の struct / tuple 戻り値がゼロになる**
- **STRUCT-UPDATE: `P { x: 5i64, ..base }`**
- **リファクタリング一巡** — 巨大関数 6 本を機能別に分割 (`lower_program` / `lower_builtin_call` / `lower_instruction` / `evaluat…

### 2026-08-24
- **NUMBER-HINT: 既定は `u64` で確定 + 位置の網羅**
- **NUMBER-HINT: suffix なし整数リテラルの位置ベース解決**
- **FOR-RANGE-END: `for i in 0u64 to n {` が parse エラーだった件**
- **NEWTYPE: tuple struct (`struct Meters(i64)`)**
- **CHECK-NONTERMINATION: `--check` の trial にステップ予算**
- **DBC-CHECK-METHODS: `--check` がメソッドも掃く**
- **CONTRACT-ELISION の残 3 件** — 符号付き添字 / `!=` 由来の 0 除算 / `for` 範囲からの境界 guard を落とせるようになった。

### 2026-08-23
- **AOT-GENERIC-THROUGH-STRUCT: struct 引数越しの generic 型引数推論 (AOT / compiler JIT)**
- **TEST-PERF-CHECK-TRIALS: `--check` の trial ごとの registry 再構築を共有化**
- **TYPE-NAME-SPELLING: 名前型の 3 つの綴りを統一**
- **JIT-enum-1: struct のフィールドに enum を置けるように (3 backend)**
- **159: interpreter JIT の generic struct 対応**
- **CALL-ARG-COMPOUND-LITERAL: compound literal を call 引数に直接渡せるように**
- **STRUCT-FIELD-GENERIC-ENUM: struct のフィールドに enum を書けるようにした**
- **PATTERN-OR-NESTED: sub-pattern 位置の or**
- **PATTERN-RANGE: 範囲を実 pattern 形にし、被覆判定を入れた**
- **PATTERN-AT-BINDING: `@` を実 pattern に (`Pattern::Binding`)**
- **PATTERN-COMPOUND-LOWER: struct / tuple パターンを lowering 対応**
- **PATTERN-STRUCT: struct パターン**
- **CONTRACT-ELISION: 添字境界の guard も消す**
- **NEVER-ALLOCATES の残件 2 つを解消**
- **DBC-CHECK-CASES: `--check` が「どれだけ試したか」を出す**

### 2026-08-21
- **NEVER-ALLOCATES: 静的な「確保しない」**
- **MEM-COUNTER-INTERP-DRIFT: アロケーションカウンタの定義を固定**
- **ALLOC-CONTRACT-SUGAR: `ensures allocates(N)` / `retains(N)` / `allocations(N)`**
- **DBC-TRAIT-INHERIT: trait の契約を impl に継承**
- **CONTRACT-ELISION: 契約が RUNTIME-TRAP の guard を消す**
- **ALLOC-CONTRACT: `ensures` の `old(expr)`**

### 2026-08-20
- **TYPECHECK-LIES: `null` を型検査で拒否 (E0015)**
- **RUNTIME-TRAP: 算術 / 添字の実行時トラップを 4 バックエンドで統一**
- **TEST-PERF: with-core フロントエンドパスを全レーンで共有 (4 レーン → 1 パス)**
- **BUILD-PERF 運用: cargo-sweep 導入 + CLAUDE.md に定期 GC コマンドを明記**
- **FRONTEND-PERF: frontend の O(n²) を 3 点で解消 (★★★)**

### 2026-08-19
- **TEST-PERF: AOT の demand-driven lowering + codegen 刈り込み (★★★)**

### 2026-08-18
- **tree-walker の関数再帰ガード (call-depth)**
- **`null` / `is_null()` の扱いを確定 (docs の仕様に実装を追従)**
- **型不一致診断の user 型を source 綴りに (interner 経由)**
- **`str.substring` / `str.split` の dispatch を接続**
- **`str + str` を型検査で拒否 (E0002)**
- **CLAUDE.md の `--message-format=short` 案内を `--format=json` に誘導**
- **INCR-INTEGRATE: 統合パスを placeholder 2 パス + HashMap から 1 パス + オフセット演算に**
- **PATTERN-EXTEND: or / 範囲 / `@` パターン (3 バックエンド)**
- **INTERP-DIAG-SPAN: 補間内の診断が実際の位置を指すように**
- **STR-INTERP-FMT: 補間の format spec (`"{x:.2}"`, 3 バックエンド)**
- **DOC-DRIFT 解消** — `docs/language.md` の *Generics and bounds* が「bound は parse されるが強制されない」と書いていたのを実際の挙動 (cal…
- **STDLIB-ORD-BOUND: impl block の generic bound を call site で強制**
- **STDLIB-ORD: `Ord` trait + `Vec::sort` (3 バックエンド)**
- **RUNTIME-IO 拡張: 乱数シード / 時刻フォーマット / 環境変数一覧 (3 バックエンド)**
- **TRAIT-BOUND: generic trait の bound を call-site で強制**
- **FROM-INTO: `.into()` と `?` の cross-error 変換**

### 2026-08-17
- **STDLIB-ITER-ADAPT: `VecIter` に `map` / `filter` / `enumerate` / `zip` / `collect` (3 バックエンド)**
- **STDLIB-ITER-ADAPT (Dict / String 版)**

### 2026-08-16
- **RUNTIME-PORT R0+R1: ランタイムを C から Rust に移植**
- **RUNTIME-PORT R2 + FFI_PLAN P1: `extern fn ... from "lib" [as "sym"]`**
- **RUNTIME-PORT R3: toylang 化の計測と判断 (移動は中止)**
- **RUNTIME-PORT R4: f64 整形の toylang 化は計測で却下、byte 一致は固定**
- **RUNTIME-IO: 最小 I/O セット (3 バックエンド)**
- **STDLIB-ITER: `Vec` / `Dict` / `String` に `iter()` (3 バックエンド)**
- **DROP-GLUE: 移動先が再帰的に解放される (3 バックエンド + IR VM)**
- **BOX-T Phase E+F: stdlib `Box<T>` (`core/std/box.t`)**
- **BOX-T Phase D: 移動された束縛は drop しない (3 バックエンド)**
- **BOX-T Phase C: 所有権の移動と use-after-move チェック (E0014)**
- **BOX-T Phase A+B: 型引数経由の再帰を通す**
- **PTR-READ-ENUM: enum の byte layout を関数境界の flatten に統一**
- **`__builtin_ptr_read` が user 定義型名の注釈を受けるように**
- **RECURSIVE-TYPES step 1: 再帰型を診断で拒否 (E0013)**
- **parser: 改行前 `(` は method call に継続しない**
- **CONCRETE-IMPL-Phase-2c (generic-wildcard 完遂)**
- **STR-INTERP-COMPOUND-EXTEND-ENUM**
- **MATCH-LET-RHS-PAYLOAD-INFER 完遂**

### 2026-08-15
- **`--check` が満たせないサイズの確保で Rust panic していたのを修正**
- **lexer エラーを診断として報告 (E0012)**
- **struct field / enum payload の初期化形を 4 形に揃えた (AOT/JIT)**
- **compound 値の読み出し側を AOT/JIT で**
- **receiver を読まない method の AOT panic を修正**
- **非 ASCII のソースリテラルの化けを修正** — UTF-8 scalar 単位で写す。
- **テストスイートを 5.15s → 4.4s に (-15%、CPU 90s → 82s)**

### 2026-08-13
- **`str == str` を内容比較に統一** — interpreter (tree-walker) だけが内容比較で、他 4 実装は runtime handle の整数比較だった (**型は通るが答えが違う** diver…
- **`Display` trait — 型が自分の見せ方を決める**
- **str リテラルの数え差 (interpreter だけ +1 確保) を解消**
- **Drop 内で `&mut self` フィールドを free すると use-after-free するバグを修正**
- **ポインタ演算 builtin (`__builtin_ptr_offset`)**
- **allocator レジストリ: `layout_report` を `--profile=mem` に自動で載せる**
- **エンジン fallback 時の副作用重複実行を修正**

### 2026-08-10
- **MEMORY-PROFILING M0〜M5 完了**
- **暗黙の impl 型パラメータ (`impl Container<T>`)**
- **stdlib の generic enum HOF を全 backend で**
- **MATCH-LET-RHS-PAYLOAD-INFER (第一段)**
- **AOT-MATCH-SCRUTINEE-EXPAND**
- **INCREMENTAL-COMPILATION Phase 5**
- **LLM-LOOP-FIX: 式の span を full extent に**
- **LLM-LOOP-FIX: 壊れた `as` キャスト提案 / 引数型不一致 E0010 → E0001 / JIT 列の空洞化**
- **LLM-LOOP P7: 補助 CLI** — 型ホール `val x: _ = expr` (専用コード E0011)、`--api <file>` (シグネチャ一覧)、`--explain [<CODE>]`。
- **DEV-LOOP D6: `--all-backends` + stdin 入力**
- **DEV-LOOP D5: `CODE_MAP.md` 新設 + `CLAUDE.md` から履歴を分離**
- **LLM-LOOP P6-3: u64 アンダーフローの trap**

### 2026-08-09
- **LLM-LOOP P0〜P6** — 設計は [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md)。
- **DEV-LOOP D1〜D4, D7** — 設計は [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md)。
- **D7 sweep が検出した潜在バグ 5 件** — AOT の代入式が値を produce していなかった / f64 `%` の明示エラーが cranelift assertion に化けていた / struct field…
- **`else if` の拒否 + パースエラーの握り潰し解消**
- **`var` の型注釈チェック追加** — `var w: bool = 1u64` が通り `println(w)` が `1` を出していた (`val` は正しく拒否)。

### 2026-05-31
- **Interpreter IR VM 化 Phase 0〜4**
- **意味論の決着 3 件** — git だけでは追いにくいので残す):
- **負数 array index を compiler 側にも実装**
- **clippy fixes across workspace**

### 2026-05-23
- **core/std 並列パース (Phase 1)** — module integration を parallel pre-parse + sequential integrate の 2 段に分割、二重パースを除去。
- **Cranelift 関数 codegen 並列化 (Phase 2)**
- **Incremental compilation Full AST cache (Phase 4)**

### 2026-05-19
- **`dyn Trait` Phase 2 (A5-P2-MVP-A〜F)**
- **Bare-name imported function calls (Phase 1)**
- **`Program` → `File` rename (Phase 4)**

### 2026-05-18
- **`dyn Trait` Phase 1 (A5-P1)**
- **Trait 多重 bound `<T: A + B>` (A2)**
- **Trait デフォルトメソッド本体 (A1)** — AST mutation pre-pass で impl に synthesize。

### 2026-05-17
- **`?` (Try) early-return operator**
- **`loop {}` + comparison chain**
- **GENERIC-ENUM-MATCH-HOF** — `Option::map` / `Result::map` / `map_err` を stdlib に追加。

### 2026-05-10
- **DEBUG-BUILTINS Phase A+B+C**

### 2026-05-09
- **IF-VAL (`if val` / `while val`)**

### 2026-05-08
- **LABEL (labelled break / continue)**
- **OP-OVERLOAD 完全コレクション** — 同型 struct ペアの全 binary + unary operator を user method に dispatch。
- **STRING-NOMINAL + STR-INTERP-COMPOUND**
- **AOT lower 系の汎用拡張** — `__builtin_sizeof` の compound 対応、`__builtin_ptr_write/read` の compound 対応。

### 2026-05-07
- **ITER-PROTOCOL-TRAIT** — generic trait 宣言 `trait Foo<T, U>` と `impl Foo<i64> for Counter`。
- **ITER-PROTOCOL-AOT** — `for x in EXPR` を AOT でも動作。
- **STR-INTERP Phase 2 (AOT + cranelift JIT)**

### 2026-05-06
- **STR-INTERP Phase 1 (interpreter)**
- **ITER-PROTOCOL Phase 1 (interpreter + JIT)**

### 2026-05-05
- **NUM-LIT-SEPARATORS** — `1_000_000u64` 等。
- **CLOSURES Phase 1〜8** — frontend / 型検査 / interpreter / AOT (direct / indirect / capturing / narrow int / return…
- **DOCS-2026-05-05** — / **NUM-W-JIT** / **ZERO-MEMCOPY-FIX** (`size==0` の libc parity) / **TYPE-ALIAS 周辺整備**。

### 2026-05-04
- **エスケープシーケンス** — `\u{HEX}` / `\xHH` / char literal `'a'` (u32)。
- **TYPE-ALIAS / GENERIC-TYPE-ALIAS**
- **GENERIC-RAII** — user struct の `impl Drop` を scope-bound auto-call (interpreter + AOT)。
- **ALLOCATOR Phase 5** — `trait Drop` + temporary-form の auto-cleanup。
- **REF-Stage-2** — `&T` / `&mut T` の borrow + writeback、escape rule の構文 reject。
- **121-Phase-B-rest** — arena / fixed_buffer の native runtime、`with` body 早期 exit の cleanup。
- **TEST-PERF-lazy-core** — / **STRING stdlib** / **CONCRETE-IMPL Phase 1〜2b**。
- **NUM-W (Phase 1〜6 + AOT + AOT-pack + signed-hash)**
- **DICT 系まとめ** — `Dict::new()` の AssociatedFunctionCall 経路、per-monomorph generic substitution。

### 2026-05-03
- **VEC-collection** — user-space `Vec<T>` (`core/std/collections/vec.t`)。
- **STR-LEN-O1 / STR-PTR-LEN** — AOT で `__builtin_str_len` を O(1) 化、`.rodata` layout を `[bytes][NUL][u64 len LE]` に。
- **121-Phase-B-min / Phase-A**
- **MUT-SELF-Stage-1** — `&mut self` receiver。
- **96残-前半** — match の deep exhaustiveness check。

### 2026-05-02 以前 (大きめのマイルストーン)
- **#183 コンパイラ MVP** — IR / cranelift-object backend で実行ファイル生成。
- **per-module function namespacing (#193 / #202)**
- **コア・モジュール auto-load (#193)**
- **Extension trait 全 backend 対応 (#191、Step A〜F)**
- **Math externalisation (#190、Phase 1〜4)**
- **Option / Result stdlib (#203)**
- **Value/Reference 分離 Phase 1〜5**
- **panic / assert / DbC (#166〜#175)**
- **言語仕様拡充 (#161〜#165)** — f64、`%`、複合代入、タプル JIT、ネスト分解、match arm guard。
- **#184 Trait + impl** — / **#170 top-level const** / **#169 `docs/language.md` 新設**。
- **MODULE-SYSTEM P1 — stdlib の配置と名前**
- **MODULE-SYSTEM P2 — qualifier がモジュールのフルパスになった**
## 未実装 📋

### 型・所有権 (NEW-TYPE-SYSTEM)

- **COMPOUND-BLOCK-DROP-TIMING の残り: 末尾が束縛に触れる形** — compound を
  作るブロックの束縛は、末尾の式がそれに触れない (数値フィールドの読みは
  除く) ときブロックの終わりで drop するようになった (2026-09-26)。触れる形
  (`Some(t.clone())` / `match t { .. }` = `?` の desugar / ポインタや
  compound のフィールドの読み) は、別名の可能性があるので compiled レーンでは
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
  ようになった (2026-10-08) が、対象は inherent impl で、その名前の method が全部
  `self: Self` のときだけ。残り: (a) **trait impl** (`&dyn` から呼ばれると受け手が
  借用なので drop できない — `&dyn` 経由の呼び出しを区別すれば足りる)、(b) 同名の
  method を `&self` で持つ型がある名前 (呼び出し側が型を知らないと移動にしないため)、
  (c) generic impl で `self` を `var me = self` と別名にしてから要素に触れる形
  (CALLEE-DROP-GENERIC の probe が断る)。どれも二重解放ではなく漏れ。

- **PARTIAL-MOVE: フィールドを 1 つ渡すと束縛まるごと移動になる** ★ — 所有する
  フィールドを渡す (`keep.push(w.h)`) と根の `w` を渡したことになり (FIELD-MOVE-DOUBLE-DROP、
  2026-10-08)、(a) 以後は非所有フィールド `w.n` の読みも `[E0014]`、(b) 残りの所有
  フィールドは漏れる (`fn first(self: Self) -> String { self.s }` の `self.t`)。Rust の
  部分移動のように、移ったフィールドを経路で持てば両方消える (drop glue がフィールド
  単位で飛ばせることが前提)。

- **NEXT-ITEM-ALIAS — iterator の item は別名なのに所有者が付く形がある** —
  `VecIter<T>::next` は要素を浅いコピーで返す (`get` と同じ、ELEMENT-BORROW)。
  for-in / `while val` は腕が必ず `continue` で抜けるので item を drop しないが、
  `val m = it.next()` と束縛する形、腕の末尾まで走る `match it.next() { .. }` は
  compiled レーンで**コンテナの要素を解放する** (`Vec<String>` で `--heap-check=poison`
  が止める)。逆に **tree-walker は for-in の item も解放する** (live バイトが減る、
  compiled レーンと割れる)。E0028 が `get` にしか効かないのと同根で、直すなら
  `next` の戻りを借用として扱う (`&T` を返す iterator) か、名前で E0028 と同じ扱いにする。
  MATCH-TEMP-EXIT-LEAK で呼び出しの scrutinee を束縛したとき、`next()` だけはこのため
  除外した (2026-10-01)。

- **WINDOW-ESCAPE-UNWRAP: `Option` から出した窓の脱出を見逃す** —
  `[E0026]` は `v.as_span()` (`Option<Span<T>>`) をそのまま返すのは
  拒否するが、`val s: Span<u64> = w ?? panic(..)` や match の payload で
  取り出してから返すと通る (taint が unwrap で途切れる)。M5 の
  `Ptr::borrow` を足すときに見つけた (2026-09-25)。`??` / match の
  payload 束縛が scrutinee の taint を引き継げばよい。

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
  - **A5 残作業** — `&dyn Trait` の return / struct field 位置 (REF-Stage-2 の escape rule が阻む)、`dyn A + B`、`dyn Iterator<T>`、generic trait の default body 内での `T` 参照。

- **CLOSURE-CAPTURE の残: E4 / E5** ★ — 設計は
  [`CLOSURE_CAPTURE.md`](CLOSURE_CAPTURE.md)。**E0〜E3 + E6 は landing 済み**
  (escape しない closure は捕捉した束縛を共有し、escape するものはコピーを
  持って書き込みが `E0021`)。残りは (a) **E4: HOF / escape 越しの可変捕捉** —
  寿命の判断が要るので「実プログラムで踏んでから」、(b) **E5: compiled
  レーンの compound capture** — 診断は直した (capture の話だと分かる文言に
  なった) が、env に compound を載せるのは未着手。interpreter は動く。

- **const generics** ★ — `struct Array<T, const N: usize>`。大規模。

### 構文糖衣 (NEW-FEATURES)

- **f64 リテラルのサフィックス必須を緩和** ★ — 現状 `1.5` 単体は
  **parse エラーですらない** (実測 2026-08-30): lexer が `1` / `.` /
  `5` に分割し、parser が tuple access `1.5` (literal `1` への index 5)
  と解釈して `[E0010] Cannot access index 5 on non-tuple type Number`
  になる。緩和の本体は lexer に `-?[0-9][0-9_]*"."[0-9][0-9_]*`
  ルールを足すことだが、**`a.0.1` (tuple access 連鎖) との曖昧性**を
  Rust と同じ手口で解く必要がある: 直前のトークンが `.` のときだけ
  小数部を読まない (rflex は `zz_marked_pos` を巻き戻せる —
  `skip_current_char` と同じ機構で、整数部だけ返して `.` から
  再走査する)。`0..10` は `.` の後が数字でないので浮動小数ルールに
  入らない (既存の曖昧性なし)。`Kind::Float64` に落とせば
  suffix 付き `1.5f64` と同じ AST になり、f64 は唯一の float 型なので
  NUMBER-HINT の型確定機構は不要。

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

- **COMPOUND-CALL-EXPR-POS: compiled レーンは compound を返す呼び出しを式の位置に置けない** ★★ —
  `String::from_str(..)` / `parse::to_u64(..)` / `mk()` のような compound (struct /
  tuple / enum) を返す呼び出しは、`val x = f()` の右辺以外 (関数の末尾、ブロックの末尾、
  引数、`match` の scrutinee) に置くと compiled レーンが「compound-returning ... in
  expression position; bind the result with `val`」で断る (interpreter は通す)。
  `fn mk() -> String { String::from_str("x") }` すら書けない。POC と spawn で最も多く
  当たった制限 (2026-10-06〜07)。spawn の本文だけは切り出しが末尾を `val` に束縛して
  回避している。演算子 overload の let-rhs 以外 (OP-OVERLOAD-CHAIN) も同根

- **CONST-ARRAY の残り: struct / tuple の表の渡し方** — 2026-09-25 に
  struct / tuple 要素の表が compiled レーンで読めるようになった
  (`val r = RS[i]` / `RS[i].w`、完了済み節)。残り: その表を `&[R; N]` で
  渡すこと (`scalar_array_ref` がスカラー要素だけを番地で渡す)、値の
  位置に直接置く `f(RS[i])` (束縛を案内するエラー)、enum 要素の表。

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

- **STDLIB-CHECK-FIXED: stdlib の型検査 ~11 ms を毎回払う** ★★ — post_checks が
  stdlib 本体について出す事実 (lend 表・エフェクト等) を stdlib のハッシュで
  再利用する軽い版と、PARALLEL_FRONTEND.md §5c のスナップショットに型検査の
  状態まで含める本命。後者は型検査器が AST を書き換えるので大仕事。

- **RUNTIME-REFS-LAZY: ランタイム関数 ~90 個の import を全関数に入れている** ★ —
  `declare_runtime_refs`。関数の import を呼ぶ分だけにしたのと同じ形 (各 signature
  に ABI 計算が走る)。使う命令から引くか遅延宣言に。未測定。

- **LEND-WORKLIST: 読むだけ判定の固定点を worklist に** ★ — 今は変化が無くなる
  まで全本体を歩き直す (logsearch で 4 周)。変化した関数の呼び出し元だけ再訪する。
  1〜2 ms の見込み。

- **CTFE-DECLARE-SHARE: CTFE 用 lowering の `declare` ~1.2 ms** ★ — 本番の
  lowering の宣言段と共有すれば消える。

- **PERF-DOC-DRIFT: 文書の数字が実測と合わない** ★ — CLAUDE.md の
  「`TOYLANG_CRANELIFT_OPT_LEVEL` は codegen を ~20x 変える」は logsearch では
  ~10%。下の TEST-PERF の「1999 テスト ~6.5s」は現在 3278 テスト ~23s。

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

2026-08-23: `--check` (property) の trial が毎回 program 全体の registry を
  再構築していたのを共有化 (TEST-PERF-CHECK-TRIALS、完了済み節)。最長の
  単一テストだった `a_pass_records_how_much_was_actually_tried` が
  4.85s → 0.40s、`builtin_test_and_check_tests` が 7.0s → 1.6s (CPU)。
  以降の数値は 2026-08-19 の測定。**負荷が低い状態での再計測待ち**。
  クレート別 CPU:
  | | CPU | テスト数 | 平均 |
  |---|---|---|---|
  | compiler | 78.3s (52%) | 408 | 192ms |
  | interpreter | 55.5s (37%) | 984 | 56ms |
  | frontend | 15.6s (10%) | 583 | 27ms |
  - **`compiler::consistency` + `example_consistency` が suite CPU の 45%** ★★★ — 46.5s (317 テスト) + 22.0s (14 shard) = 68.5s。`consistency` の分布は二峰性で、**lite パスで完結するテストと full core にフォールスルーするテストで 1 桁違う**。**2026-08-20 のフロントエンドパス共有で 46.5s → 34s / 22.0s → 16s 相当 (下記)** まで下がったが、残りは AOT の codegen + link + spawn (cache warm でも ~50ms/テスト) と JIT の native compile が本質的なので、ここから先はバックエンド実行そのものの削減になる。
    **「lite → full 二重パス」は 2026-08-18 に潰したが、それ自体はコストではなかった**と分かったので記録しておく: `assert_consistent` の let-chain は**最も安いレーン (no-core の tree-walker) で短絡する**ので、stdlib を使うソースが捨てられる AOT codegen / link / spawn まで到達することは元から無かった。捨てていたのは parse + no-core 型検査 ~2ms だけ。実際に効いたのは同時に入れた**フロントエンドパスの共有**の方 (下記)。
  - **core module のロードが 1 プロセスあたり 27ms** ★★★ — trivial プログラムを空 core dir と比べた実測 (2026-08-18、debug ビルド): **33.5ms → 6.1ms**。nextest は 1 テスト 1 プロセスなので、interpreter の 984 テストはそれぞれこれを払う = ~26s CPU ≈ wall 1.3s。内訳は 2026-08-15 時点の計測 (integrate ~43% 削減が landing する前) で `integrate_modules` 11.3ms / `execute_entry` の context 構築 5.2ms / stdlib 40 impl block の型検査 2.5ms / その他の型検査 1.1ms。
    **測って分かった否定的な結果を 3 つ記録しておく**: (1) 「stdlib 本体を型検査しない」は採らない (測定時点では free function の body を `take(user_func_count)` で user 分しか検査しておらず、削れるのは impl block の 2.5ms だけだった。2026-08-30 にその `take` も外し、今は stdlib も全部検査する)。**型検査器は body を書き換える** (`?` の desugar、`Display` の `to_str` 挿入) ので、stdlib の body を検査しないと**書き換え前の AST がバックエンドに流れる** — 今の stdlib は `?` も補間も使っていないので通ってしまい、使った日に壊れる罠になる。(2) `remap_symbol` の memo 化 (module symbol → main symbol を Vec でキャッシュ) は**効果ゼロ**だった。integrate の時間は文字列ハッシュではなく AST を pool に複製する作業そのもの。(3) **「型検査済み core をプロセス内で使い回す」は unit テストには効かない** — nextest は 1 テスト 1 プロセスなので、そもそもプロセス内に 2 回目の呼び出しが無い。
    したがって残る手は (a) stdlib を使わないテストを `test_program_no_core` に寄せる (実測: `test_program` を no-core にすると interpreter の 879 テスト中 **797 が通り**、その binary は 2.3s → 1.3s。ただし stdlib 同居時の回帰を見なくなる = coverage を実際に落とす)、(b) **プロセスを跨いで**型検査済み core を再利用する (INCREMENTAL-COMPILATION 側の仕事。`File` が `Rc` を持つので素朴な in-memory memo 化はできない — 別スレッドから clone すると refcount が壊れる)、(c) 1 プロセスで core を複数回ロードしている `consistency` を直す — **解消 (2026-08-20)**: 4 レーンが 1 フロントエンドパスを共有するようになり、AOT / JIT レーンが毎回 core をロードし直す重複が無くなった (consistency -27% CPU)。
  - **プロセス起動が ~5ms × 1999 ≈ 10s CPU (約 7%)** ★ — 起動フロアの実測は空 core dir の trivial 実行 6.1ms。nextest は 1 テスト 1 プロセス。テストを機能別に束ねれば減るが、失敗の切り分けと引き換え。
  - ~~`serial_test` (`oop_tests.rs`) の並列化~~ — **効果ゼロと分かったので却下 (2026-08-18)**。`#[serial]` が付いているのは 8 テストで合計 **0.193s CPU (suite の 0.12%)**、1 本 18〜34ms と既に起動フロア。しかも `serial_test` のロックはプロセスローカルなので、**nextest では各テストが別プロセスに散る = 元から直列化していない**。

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

- **BUILD-PERF** — **ビルドはテスト実行より桁で高い**。クリーンな target で、`interpreter/src/lib.rs` を 1 行触ってからの再ビルドが **2.35s**、テストファイル 1 個なら **1.08s**、クリーンからのフルビルド (テストターゲット全部) が **23.7s** — 対して全 1999 テストの実行が 7.5s (2026-08-19 実測、20 コア)。ここは 2026-08-19 に一度片付けたので、**残っているのは運用の話**:
  - **target を肥大させないこと — 実測 49x で、他のどの施策より大きい** ★★★ — 同じ「lib を触って再ビルド」が、**95GB / 1,248,912 ファイル**まで育った target の上では **1m55s**、`cargo clean` 直後の 1.7GB / 7,729 ファイルでは **2.35s**。消えた 125 万ファイルの 99% は過去のビルドの残骸。理由は cargo が rustc に `-L dependency=target/debug/deps` を渡すことで、**リンカが毎回 125 万エントリのディレクトリを走査する**。「user 45s に対し sys 6分」という異常な比率の正体がこれで、リンカが遅いのではなくディレクトリが大きすぎた。**古い成果物を定期的に GC すること** — **運用セットアップ済み (2026-08-20)**: `cargo-sweep` を導入し、`cargo sweep --time 1` (世代 GC) を CLAUDE.md に明記。この状態に戻ると下の施策は全部誤差に埋もれる。
  - **テストバイナリは 1 クレート 1 本** (2026-08-19 landing、73 → 12) — cargo は `tests/*.rs` を **1 ファイル 1 バイナリ**でリンクするので、64 ファイルは ~27MB の実行ファイルを 64 回リンクすることを意味していた (各々が frontend / interpreter / cranelift を静的に抱える)。`autotests = false` + `[[test]]` 1 個 + `#[path]` でモジュール取り込み。ファイルは 1 つも移動していない。**クリーン比較で フルビルド 37.5s → 23.7s / CPU 8m45s → 3m13s、lib 変更ループ 4.33s → 2.35s**。代償はテストファイル 1 個の編集が 0.88s → 1.08s (クレートの suite 全体が再コンパイルされる) と、テスト名にファイル名が前置されること。
  - **third-party の opt-level は 0、ただし cranelift だけ 2** (2026-08-19) — 全 deps を 3 で焼くのはビルド時間の払い損だった。**テスト実行が速さを感じる dep は cranelift だけ** (各テストが小さなプログラムを JIT / AOT する) なので、そこだけ残した。**テスト実行は劣化していない** (7.5s)。綴りに 2 つ罠があり、**どちらも間違えても cargo はエラーを出さない**: キーは `overrides` ではなく **`package`** (`overrides` は 1.41 以前の名前で、`unused manifest key` として黙って無視される)、そして **`cranelift` 単体は umbrella crate にしか当たらない** (実体は `cranelift-codegen` 以下 12 crate なので個別に列挙する。一致しない package spec は警告なしで「オーバーライド無し」になる)。
  - **測って外れた仮説を 2 つ記録しておく**: (1) **デバッグ情報の削減は効かない** — `[profile.dev]` / `[profile.test]` に `debug = "line-tables-only"` を入れて 2m06s (対照 1m55s)、改善ゼロ。27MB の中身はデバッグ情報ではなく cranelift のコード。(2) **リンカ差し替え (lld) と Spotlight 除外は、上を片付けた後では測る意味がない** — 絶対値が 1〜2 秒台まで落ちているので削り代が残っていない。target が肥大していた頃の「リンクが遅い」という観察は、リンカの速度ではなくディレクトリ規模の問題だった。

- **TEST-PERF** — ワークスペース全体で **~6.5s** (2026-08-19 実測、20 コア、warm、`cargo nextest run`、1999 テスト。AOT demand-driven lowering で 7.8s → ~6.5s)。**この suite は wall ではなく CPU 律速**: テスト時間の総和 (149.4s CPU は demand-driven lowering 前の値) / 20 コア が下限で、実測 wall はそこに張り付いている。**ビルド時間は別問題で、そちらの方が大きい — BUILD-PERF を見ること**。最長の単一テストも 2.11s (`example_consistency` shard_8) なので critical path 律速でもない。したがって**並列度を上げる策は効かず、効くのは CPU そのものを減らす策だけ。換算レートは 20:1** (CPU を 20s 削って wall 1s)。

- **65. frontend リファクタリング** — (a)〜(g) と死にコードの削除 (2026-10-01) は完了。残: doc コメント拡充、プロパティベーステスト追加。

- **property test の generator が仕様と drift しないか** — `valid_identifier()` は lexer に問い合わせる形にした (2026-08-10)。他の generator (リテラル / 演算子) はまだ手書きなので、同種の drift が起きうる。

- **26. ドキュメント整備** — 残: API リファレンス、advanced topics。

- **AOT-LLVM の残り** ★ — LLVM バックエンドは完了 (完了済み節、[`AOT_LLVM.md`](AOT_LLVM.md))。
  残りは運用: LLVM レーンのテストは `--features llvm` のときだけ走る (既定の
  `cargo nextest run` では走らない) ので、定期的に回す場所を決める

- **LLVM-COMPILE-TIME: `--release` のコンパイルが cranelift の ~24 倍** ★ — `poc/logsearch` で
  1.94 s (cranelift 80 ms)。`-O2` 1.09 s + 機械語 0.77 s で、**1 スレッド**で走る
  (cranelift は関数ごとに全コア)。特定の pass が飛び抜けてはいない。案: `-O2` の後で
  モジュールを分け、別々の `Context` / `TargetMachine` で object を並列に作る
  (インライン展開は失わない)。[`AOT_LLVM.md`](AOT_LLVM.md) §2.8

- **RT-ALLOC-TABLE-COST: 常時引く確保表が確保の多いプログラムで効く** ★ — `free` を冪等に
  するための番地 → サイズの表 (`prof_put` / `prof_take`) が `poc/logsearch archive` の 6%
  (確保 1.54M + realloc 2.30M + free 1.50M、~26 ns/回、表がキャッシュに乗らない)。
  `String::push` の伸長が確保の 1.5 倍の realloc を出しているのも一因。
  [`AOT_LLVM.md`](AOT_LLVM.md) §2.8

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

* **Claude Code 連携 (CLAUDE-CODE-INTEGRATION) の残り** — T1 / C1 / C2 / T2 / T3 は
  2026-09-30〜10-01 に landing (完了済み節)。残りは T4 `toy lsp` (Claude Code の
  組み込み `LSP` ツールの操作が `toy query` とほぼ一対一)、T5 プラグイン化、
  T6 `toy mcp` (任意)。[`CLAUDE_CODE_INTEGRATION.md`](CLAUDE_CODE_INTEGRATION.md)。
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
  — `Vec` の `sum` / `min` / `max` (**API 自体が無い**ので追加から)、
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
* LSP 対応 — 補完 / go-to-definition / hover / 診断 / フォーマット。frontend の AST・型チェッカ・`SourceLocation` を再利用できる。ただし**エージェントは LSP より CLI クエリを使いやすい**ので、LLM ループの観点では `--api` / 型ホール (P7 で landing 済み) の方が先だった

## 現状の把握

> 言語仕様は [`../docs/language.md`](../docs/language.md) が正本、日常的に踏む
> 要点は [`CLAUDE.md`](../CLAUDE.md)、実装の場所は
> [`CODE_MAP.md`](CODE_MAP.md) にある。
> **機能一覧をここに再掲しない** — 3 重管理になり、実際に食い違った
> (この節は `String` を「`Vec<u8>` alias」と書き続けていたが、
> 2026-05-08 に nominal struct へ変わっていた)。

### テスト状況
- 合計 **3299 テスト** (100% 成功、2026-10-08 時点、`cargo nextest run`)。
  `--features llvm` では compiler / toy の 1182 本が、AOT レーンを LLVM でも
  作って cranelift と突き合わせる (AOT_LLVM.md)。
- 内訳: interpreter unit + integration、frontend unit、compiler e2e + consistency。後者は
  tree-walker / IR VM / JIT / AOT の一致を保証する。
- ワークスペース全体で **~20s** (2026-10-08 実測、warm、nextest の既定 profile 出力)。
  内訳と削り代は TEST-PERF、ビルド時間は BUILD-PERF (どちらも数字は 2026-08 時点の
  もので古い — PERF-DOC-DRIFT)。
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
  `u8 << u8` も `u8 << u64` も「incompatible types u8 and u64」。
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

- **USER-TYPE-NAMED-T: ユーザの型名 `T` が stdlib の型パラメータ `T` と衝突する** ★ —
  `struct T<X> { .. }` を宣言すると、無関係な stdlib の本体 (`vec.t` の
  `val e: T = p.get(i)` など) が `[E0028]` / `[E0014]` で落ちる。stdlib の generic
  本体の `T` がユーザの struct `T` に解決されている (2026-10-08、
  CONSUMING-SELF-NO-DROP の再現を書いていて踏んだ)。型パラメータは宣言より
  優先して解決されるべき。

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
- 3-part qualified call (`std::math::abs(x)`) は **MODULE-SYSTEM P3 で解決済み** (2026-09-21)。パーサが全セグメントを記録し、型検査と lowering が同じものを修飾子として使う。実在しないパスは `[E0030]`。
