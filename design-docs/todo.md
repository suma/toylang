# TODO - Interpreter Improvements

## 完了済み ✅

> **この節は 1 行サマリだけを持つ。** 実装の経緯・測定値・ファイルパス・
> テスト数は git log のコミットメッセージにある。フェーズ設計は
> [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md) /
> [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md) /
> [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md) /
> [`FEATURE_NOTES.md`](FEATURE_NOTES.md) を参照。
> ここを段落で埋めると、常時読まれるファイルが changelog になる。

### 2026-09-25

- **MOVE-REINIT: 丸ごとの代入で所有し直す** — `x = e` は旧値を (まだ
  持っていれば) `e` の評価後に drop して新しい値を持つ。移動の後も
  読め、`kept = keep(p, kept)` はループ内でも通る。以前は代入した旧値を
  誰も解放せず (compiled)、tree-walker は旧値の代わりに新しい値を
  漏らしていた。compiled レーンは struct の丸ごと代入に対応した
  (一時領域に作って写す)。tree-walker の drop glue は Vec / tuple /
  配列 / enum payload を先頭から drop するようにした (compiled と同順)。

- **RETURN-DROP: enum を返す関数のローカルが compiled レーンで drop されて
  いなかった** — 本体を戻り値の storage へ直接 lower する経路が drop
  スコープを開いていなかった (`Result` / `Option` を返す関数の中の
  `File` も閉じていなかった)。返り値として出ていく束縛 (本体の末尾・
  `return x`・その分岐の末尾) は移動として扱う (分岐なら flag)。以前は
  `return h` が h を drop してから返し、呼び出し側でもう一度 drop して
  いた。compound 値を作るブロックの束縛は外側のスコープに登録される
  ので flag を付け、作らなかった経路でゼロの値を drop しない。

- **MOVE-CONDITIONAL: 分岐の中の移動を実行時 drop flag で追う** — 移動を
  含む最も内側の文 (か腕の本体) の直前で flag を落とし、drop は flag を
  見る (compiled レーンは Bool ローカル、tree-walker は drop エントリを
  外す)。各腕は分岐前の状態から始め、抜ける腕の移動は後ろに残さない。
  ループ本体の外側の束縛は、直後に `return` か 1 重の `break` があるとき
  だけ移動できる (`break s` もこれで通る)。closure の中は従来どおり拒否。

- **LEND-MUTATING-CALLEE: 何も所有しない場所だけを書き換える受け手も貸し出し** —
  `b.n = ..` / それしかしない `&mut self` メソッド / 受け手内の
  `var c = b` 経由の受け手に渡した値を、呼び出し側が drop する (以前は
  誰も解放しなかった)。自前の `impl Drop` を持つ型のフィールドへの書き
  込みは従来どおり移る。受け手内の仮引数の別名を tree-walker だけが
  drop していたのも揃えた。

- **TEST-PARALLEL P6 / TEST-TOOL T4: `toy test --backend all`** — aot と
  vm の両方で走らせ、テストごとの合否の食い違いを報告する (両方で落ちた
  テストも報告し、どちらもあれば終了コード 1。AOT で先の失敗のせいで
  走らなかったテストは比較しない)。レーンは 1 つのジョブ列に混ぜず順に
  走らせる — 混ぜると同じテストの 2 つの写しが決まったパスを取り合った。
  `poc/logsearch` は 152 件すべて一致。

- **NARROW-UNSIGNED-SUB: 符号なし減算は全幅で trap** — `u8` / `u16` /
  `u32` のアンダーフローも `u64` と同じく trap する (以前は wrap、4 レーン
  一致で `5u8 - 10u8 == 251u8`)。メッセージは幅を名指す。guard の省略
  (契約・制御フロー) は全幅に効く。`poc/logsearch` の Apache 時刻の
  分・秒が数字かを確かめずに `b - '0'` していた箇所が見つかった (秒が
  1 桁のテスト入力でゴミの時刻を作っていた)。

- **CONST-ARRAY: struct / tuple の表** — `const RS: [R; N]` を compiled
  レーンでも読める (以前は「only scalars are supported」)。要素は leaf の
  定数として評価し、読み出し時に `Vec<T>` と同じ詰めた配置で `.rodata` に
  置く。`val r = RS[i]` と `RS[i].w` (入れ子・タプル添字も) が 3 レーン
  一致。リテラルのフィールド順は自由。

- **SHARED-BORROW-WRITE: 共有の借用を通した書き込みは型エラー** —
  `&T` 引数 / `&self` の下へのフィールド・要素・添字の代入 (複合代入を
  含む) と、`&mut self` メソッドの呼び出しを拒否する。以前は全レーンで
  写しに書かれて黙って失われていた。`jit_fixed_buffer_allocator.t` が
  これを踏んでいた (`&FixedBuffer` 越しの `alloc` が割り当て量を
  数えていなかった)。

- **CONST-ARRAY: 表を名前で渡せる** — `&[T; N]` / `&mut [T; N]` (スカラー
  要素) の引数が compiled レーンで番地 1 つとして通る (`const` は
  `.rodata`、スタック配列は先頭要素の番地)。境界検査は所有する配列と同じ。
  共有の借用を通した書き込みは型エラー。続けて値渡しの配列引数 (呼び
  出し側の番地を受けて入口で自分のスロットへコピー) と配列リテラルの
  引数も通した (`array_type_only.t` が AOT_UNSUPPORTED から外れた)。

- **RANGE-TYPE-ANNOTATION: 範囲が関数の境界を越える** — パーサが
  型引数 1 つの `Range<T>` を組み込みの範囲型に読む (以前は generic
  struct として読まれ、`expected Range<u64>, but got Range<u64>`)。
  compiled レーンは範囲を `(start, end)` の 2 要素タプルとして受け渡す
  (`&dyn` と同じ平坦化)。引数・戻り値 (末尾 / if の両腕 / `return`)・
  メソッド・generic `Range<T>` が 3 レーン一致。本体内の分岐が作る範囲と
  タプル / フィールドの中の範囲は compiled レーンでは従来どおり不可。

- **IRVM-SELF-WRITEBACK-REALLOC: IR VM の `value not defined` は無効な
  読み出しだった** — 値渡しの `self: Self` は写しなので、`push` は
  呼び出し元を伸ばさず、`get` が長さ 0 の確保 (番地 0) を読んでいた
  (BY-VALUE-SELF-ALIAS 修正後はプログラム側の誤り)。IR VM は
  `PtrRead` が何も読めないと値を未定義のまま進め、次の使用で Rust の
  panic になっていた。今は toylang の実行時エラー (`invalid memory
  access ...`、backtrace つき) で、tree-walker の `Invalid memory access`
  と揃う。AOT は検査しない (未定義動作のまま)。

- **`toy new` / `toy init` / `run --backend all` / `test --check`** —
  規約どおりの雛形 (上書きしない)、パッケージ単位の 3 レーン突き合わせ、
  契約のプロパティテスト ([`BUILD_TOOL.md`](BUILD_TOOL.md) D3)。途中で
  2 つ直した: 並列 `toy test` の初回で出ていた「module cache の保存に
  失敗」の警告 (同一プロセスのスレッドが同じ一時ファイル名を使っていた。
  リンクキャッシュも同じ)、`--check` が generic 型の関連関数
  (`Vec::with_capacity`) を型引数なしで走らせて誤って FAILED にしていた件
  (`String` を使う全プログラムで出ていた)。

- **MEMORY-ACCESS M5: `Vec` / `String` / `Dict` / `Box` が生 builtin を
  直接叩かない** — 要素の読み書きと借用は `Ptr<T>` (`get` / `set` /
  `borrow` は全レーンで intrinsic、呼び出しにならない)、範囲操作は
  `Span<T>`。`Vec` / `VecIter` / `String` の `elem_size`、`Dict` の
  `sizes`、`ZipIter` の `elems` を撤去し、Vec は 3 leaf に。stdlib の
  `unsafe fn` 宣言は 102 本に (コメント行を除いた数)、`poc/logsearch` は出力一致のまま `__text`
  −8.2%・archive ~6% 速い。途中で tree-walker の型引数の穴を 3 つ直した
  (struct リテラルのフィールド型、`Ptr` 注釈と呼び出し元の `T`、
  enum 値の型引数)。残りは未実装節の UNSAFE-REST / SPAN-RANGE-INTRINSIC /
  WINDOW-ESCAPE-UNWRAP。

- **UNSAFE-REST 前半: コレクションから `unsafe fn` を外した** — `Deque` /
  `Set` / `PriorityQueue` / `SoaVec` と String の SIMD 走査が `Ptr<T>` /
  新設の `SoaPtr<T>` (列分割の窓、intrinsic) / `Span<u8>` の範囲演算・
  `load16` / `store16` を通る。`Deque` / `Set` の `elem_size` も撤去。
  impl メソッドがベクトル型を受け渡せるようにした。続けて `hex` /
  `base64` も `Ptr<u8>::load16` / `store16` (intrinsic) で書き直した。
  stdlib の `unsafe fn` は 102 → 59、`poc/logsearch` と codec の速度は不変。

- **TREE-WALKER-GENERIC-SCOPE: closure の型がメソッドの型引数を決める** —
  `map<U>(&self, f: fn (T) -> U)` の `U` を tree-walker が束縛して
  いなかった (引数の値から型引数を拾う `collect_generic_bindings` が
  closure のシグネチャを見ていなかった)。そのため `MapIter<T, U>` の
  `U` が名前のまま残り、そこから作った `Vec<U>` の中の
  `__builtin_sizeof::<T>()` が「unbound generic parameter」で落ちていた。
  compiled レーンは monomorph で置き換えるので元から通る。`Vec` が
  `elem_size` を最初の `push` で学ぶ回避策は、これで不要になった
  (撤去は MEMORY-ACCESS M5 と一緒に行う)。

- **AOT 実行ファイルの非再現性** — 「run ごとに変わる」のではなく
  **出力先のディレクトリで変わる**のだった。ld64 が runtime の各 object に
  `N_OSO` の stab を書き、その archive を絶対パスで名指す — archive は
  出力の隣の一時ファイル `.toy_compile_<名前>.rt.a` (リンク後に消える)。
  macOS のリンクに `-Wl,-S` を足し、同じ名前ならどのディレクトリでも
  バイト一致 (`reproducible_build.rs` が pin)。**ファイル名は残る** —
  ad-hoc 署名がそれを識別子にする (変えるには `codesign` の再実行が要る
  ので見送り)。確認したのは macOS のみ。

- **MEMORY-ACCESS: 旧形 `__builtin_ptr_read(p, off)` を削除** — 型引数の無い
  読み出しはパースエラーで `::<T>` 形を案内する (警告期間は置かなかった —
  stdlib は M2 で移行済み、残る使用者はテストと例 1 本だった)。旧形専用の
  分岐 (lowering の `val` / 代入の特例、tree-walker の注釈読み、
  interpreter JIT の `ptr_read_hints` 一式) を削除。`allocator_list.t` は
  新形に移って AOT で動くようになり、`AOT_UNSUPPORTED` から外れた。
  `pending_annotation` は `__builtin_soa_read` が使うので残る。

- **NUM-W-ENUMERATION: 「全スカラー」を名指すリストを述語に寄せた** — IR
  `Type` に `is_scalar` / `is_unsigned` / `is_narrow_int` /
  `scalar_byte_size` を置き、幅の表 6 コピーを 1 つに、`matches!` で
  集合を書き下していた 12 か所を述語にした。**寄せる途中で列挙漏れの
  バグを 2 件直した** (通算 8 件目・9 件目): タプルの型引数が
  `i64` / `u64` / `f64` / `bool` しか受けず `Pair<(u8, u64)>` が注釈を
  要求した、`&dyn` の leaf 幅の表に `f32` が無く `f32` フィールドの
  struct を trait object にできなかった。残る幅ごとの `match` の腕は
  幅で処理が違う正当なもの。`PRIMITIVE_IMPL_TARGETS` の各型が述語と
  幅の表に載っていることを `compiler_lower` の単体テストが見張る。

- **CODE-SIZE-DIAG-STRINGS: 診断文の共有部分を 1 度だけ持つ** — panic
  サイトごとの描画済み blob (`poc/logsearch` で 551 個・64 KB) を 1 つの
  `toy_diag_pool` にまとめ、サイトは見出し・ファイル名・メッセージを
  自分からの相対 offset で指す記録にした。runtime (`write_diag_fd`) が
  連結すると以前と同じバイト列。`poc/logsearch` のバイナリは
  570,096 → 548,504 B。[`CODE_SIZE.md`](CODE_SIZE.md)。

- **121-Phase-B の残り: allocator を決められる確保は runtime に聞かない** —
  `AllocPush` が 1 つも無いプログラムでは、lowering の後
  (`alloc_devirt`) に全 `Heap*` の binding を `Static(0)` にし、codegen は
  `toy_alloc_current()` を呼ばずに定数を渡す (`with` が 1 つでもあれば
  全部 `Ambient` のまま — 関数は呼び出し側の `with` を見られない)。
  確保と解放 300 万回のループで 0.06 → 0.05 s。
  **`__builtin_default_allocator()` を `u64` にする案は採らない**:
  `Allocator` の同一性は interpreter では `Rc::ptr_eq`、compiled レーンでは
  handle の数で、数として見せるとレーンごとに違う表現が漏れる。比較は
  `==` / `!=` で足りている。

- **NUM-W-AOT-pack Phase 3: compound 要素の AoS 配列を pack** — 要素内の
  leaf を実幅・自然アラインメントで置き、slot はバイト単位で添字を取る
  (stride 1、`要素番号 × size + offsets[j]`)。codegen / IR VM は無変更、
  lowering の添字の組み立て 7 か所を `interleaved_units` に寄せた。
  `[Mixed; 3]` (`bool` / `u8` / `f32` / `u64`) の frame は 96 → 48 バイト。

- **REF-Stage-2 の残り: ポインタで渡す引数がすべての呼び出しの形に届いた** —
  compound の `&T` / `&mut T` を番地で渡す ABI (CODE-SIZE-SELF-ABI) が
  3 つの形で抜けていて、leaf 9 個以上の struct では**ビルドが止まって
  いた**: struct / enum を返すメソッドへの参照引数 (`val v = w.plus(&o)`)、
  一時値の参照引数 (`w.plus(&mk(7u64))`)、幅の広い iterator の `for`
  (`match it.next()` が receiver の leaf を手で並べていた)。
  `ReceiverReload` が複数の slot を持てるようにし、一時値は
  `temporary_address` で slot に書いて渡す。閾値 8 の既存プログラムの
  コードは変わらない (`poc/logsearch` の出力はバイト一致)。

- **BREAK-WITH-VALUE: `break <value>` で `loop` を値にする** — パーサが
  `var __loop_value_N = None` + `while true` + 取り出しの `match` に
  desugar し、型検査器が最初に型を名指す `break` から var の注釈
  `Option<T>` を書き戻す (バックエンド無変更)。`val x = loop {..}` /
  関数末尾 / `@label: loop` からの `break @label v`。`while` / `for` は
  値を持たない、値つきと値なしの `break` の混在は不可、値は `break` と
  同じ行。ループの外の所有束縛を `break s` で出すのは 2026-09-25 の
  MOVE-CONDITIONAL で通るようになった。
- **STRUCT-SUGAR-GAP: struct の省略形と分割束縛** — `P { x, y }` と
  `val P { x, y: b, .. } = e` (入れ子・tuple の中・`var` 可)。分割束縛は
  一時束縛 + 1 腕の `match` による検査 + フィールド読みに desugar する
  ので、型名とフィールドの過不足は match の腕と同じ診断になる。

### 2026-09-24

- **MATCH-STRING-LITERAL: `String` をリテラル腕で match** — `"a" =>` の
  腕を型検査器が `_ if <scrutinee>.eq_str("a")` に書き換える (確保なし、
  バックエンド無変更)。scrutinee は名前かフィールドパスに限り、計算式は
  `val` を案内するエラー。**ネストした位置** (`Option::Some("a")` on
  `Option<String>`) は未対応。同時に compiled レーンが struct / tuple の
  **フィールド** (`match o.p { P { .. } => }`) を scrutinee にできるよう
  にした (以前は scalar 扱いで拒否)。
- **CONST-UNSUFFIXED-INIT: const の宣言型が初期化子の型を名指す** —
  `const J: u64 = 4` が実行時に `Expr::Number` で落ち、`const M: u8 = 'a'`
  は型エラーだった。初期化子を宣言型のヒントつきで検査する。同時に、
  const 表 (`const T: [u32; 3]`) がある program で計算式の const が
  compiled レーンに畳まれずに届く既存バグを修正 (fold の lowering が
  表を scalar で仮置きしていた)。

- **MATCH-MOVE-OUT-DOUBLE-DROP (+ MOVE-ALIAS-GAP): 別名を渡すと根も渡す** —
  `val b = a` / `match a` の腕の payload 名 / `val x = match a { Ok(c) => c, .. }`
  は `a` の値の別名で、別名を渡しても `a` が drop していた (fd なら二重
  close。`poc/logsearch` の VM レーンが時々 abort した原因)。`move_check`
  が別名の根を記録し、別名の移動を根の移動として扱う (根は drop しない・
  以後の読みは E0014)。`val x = match a {..}` の `x` 自身は drop しない。
  腕の中で自分の scrutinee の payload を渡すのは、他の variant が何も
  所有しないときに限り許す。バックエンドは無変更。
- **BY-VALUE-PARAM-NO-DROP: 読むだけの受け手への値渡しは貸し出し** —
  値渡しの引数は呼び出し側が手放し、受け手も drop しないので、読む
  だけの受け手に渡した値は誰も解放しなかった。`move_check` が各関数の
  値渡し引数を「読むだけか」で不動点判定し (`compute_lend`)、読むだけ
  なら呼び出し側が drop を持ち続ける。言語上は移動のまま (E0014)。
  残る穴は「`&mut self` などで変えるが解放しない受け手」で、これは
  従来どおり誰も解放しない。
- **`if val` の `else` 無しで本体が `()`** (compiled レーンが拒否) と
  **モジュール修飾の const の lowering** を修正。`poc/logsearch` の
  数を返す関数 74 本を `const` 42 本と enum 9 つに移した。

### 2026-09-23

- **ENUM-STRUCT-VARIANT: enum の struct variant** — `A { x: u64 }` を
  宣言でき、構築 `E::A { x: .. }` と pattern `E::A { x, .. }` を
  型検査器が tuple variant の形に書き換える (バックエンド無変更、
  AST キャッシュは v53)。`println` の位置表示は下の未実装項目。
- **ENUM-DISCRIMINANT: unit variant の番号と `as`** — `Apache = 10`、
  未指定は直前 +1。`e as T` は全 variant が unit で全番号が `T` に
  収まるときだけ許し、型検査器が `match` (素のパスは literal) に
  書き換える。layout と tag は変えていない (AST キャッシュは v52)。
- **CHAR-LITERAL-MATCH: 全整数幅を match の scrutinee に** — `u8` / `u16` /
  `u32` / `i8` / `i16` / `i32` を match でき、腕は suffix つき narrow
  literal・char リテラル (scrutinee の幅に narrow)・範囲・`|` で書ける。
  網羅性は値の区間で数えるので、範囲で埋めた `u8` は `_` が要らない。
  バックエンドは無変更 (interpreter JIT は narrow の literal を断って
  fallback)。
- **MATCH-CONST-PATTERN: pattern の const 名は値と比較する** — 以前は
  黙って新しい名前の束縛になり、腕が全値に当たって後続の `_` が
  unreachable と言われるだけだった。型検査器がリテラルパターンに
  書き換える (ネスト位置・const の連鎖・モジュールの `pub const` も)。
  初期化子が型検査時点で literal でない const と型の不一致はエラー。
- **STR-ESCAPE-HATCH: `\"` と raw 文字列リテラル** — 通常リテラルで
  `\"` が書け (閉じない)、`r"..."` / `r#"..."#` (`#` は任意個) は
  エスケープも補間もしない。どちらも改行をまたげる。lexer は開き
  引用符だけを rule で受けて閉じを手で走査する形になった。
  `"""..."""` は作らない — 通常リテラルが既に改行をまたげ、`"` を
  含む複数行は raw が受ける。

### 2026-09-22

- **COMPILE-PROFILE: `compiler` / `toy build` の `--profile=compile`** —
  AOT コンパイルのフェーズの木・読んだファイルごとの量と AST キャッシュ・
  各段の処理量・重い関数 (typecheck / lower / codegen) を stderr に出す。
  記録器は `frontend::compile_profile`。項目と初回の計測結果は
  [`COMPILE_PROFILE.md`](COMPILE_PROFILE.md)。
- **出力の形を選ぶ flag を `--format=text|json` 1 つに統合** —
  `compiler` / `interpreter` / `toy` の `--diagnostics` と
  `--profile-format` を廃止 (渡すと `--format` を案内するエラー)。
  `json` で結果 (stdout)・診断・`--profile=mem` レポート (stderr) が
  まとめて JSON になる。`toy run --format=json` は拒否をやめ、
  プログラムの stdout はそのままで診断だけ JSON にする。
- **`f32` を知らない型の列挙が 5 つあった** — リファクタリングの
  棚卸しで見つけた。`SIMD-F32` が `f32` をスカラーとして足したとき、
  **手書きの型リスト**が付いてこなかった:
  * `&f32` / `&mut f32` — 「どの引数を番地で渡すか」を決めるリストと
    「受け側でどう束縛するか」のリストが、この型 1 つで食い違って
    いた。**両者は同じ答えでなければならないと doc に書いてある**。
    しかも症状は診断ではなく **cranelift の verifier が panic**
    (`declared type of variable var0 doesn't match type of value v0`)
  * `[f32; N]` — 要素型として拒否されていたのに、`elem_stride_bytes`
    は最初から `f32` に 4 バイト stride を与えていた

  * クロージャの**捕捉**と**引数** — 「primitive scalar しか捕捉
    できない」と言いながら `f32` を断っていた。捕捉リストの
    コメント自身が「不透明型と compound だけを拒む」と書いている

  同じ意味のものを 1 つの定義 (`is_scalar_pointee`) に寄せた。
  **リストが手書きである限り次の型でまた起きる**ので、
  新しいスカラーを足すときはこの定義から辿ること。
- **`println(ps[i].f)` が compiled lane で断られていた** — `val` に
  束縛すれば通るのに print に直接書くと
  「field-access chains rooted at a bare identifier」。print の経路が
  `resolve_field_chain` の拒否を `?` で伝播していたためで、値の経路に
  落ちれば 1 回の leaf load で済む (DATA-ORIENTED)。

### 2026-09-21

- **AOT-MATCH-STR-ARM-BLOCK — 名前は型を指していたが、原因は形
  だった** — 末尾の `match` の arm が**そのブロック自身が束縛した名前
  で終わる** (`{ val a = f()  a }`) と、結果の局所を決める覗き見が
  `Unit` を返し、「falls through without producing a value」で断られて
  いた。`str` で見つかったので名前に `STR` が入っているが、**`u64`
  でも同じ**だった (`val` に束縛する形は別の文面 `val/var rhs produced
  no value` で落ちる)。覗き見が `val` の注釈か右辺から型を読むように
  した。
- **NEVER-ALLOCATES-METHOD-STACK — impl の中で修飾子を重ねられる
  ようになった** — 修飾子の並びは「今見ている語の**直後**が `fn` か」で
  判定していたので、`unsafe fn` と `never_allocates fn` は通り
  `never_allocates unsafe fn` は通らなかった (自由関数側は 3 つとも
  通る)。並び全体を先に見てから消費する。`unsafe` を名前に使う形は
  従来どおり。`Vec::get` が `never_allocates` を名乗る。
- **TEST-PARALLEL P5 — `test "..." serial { }`** — 共有資源を触る
  テストが**最後に 1 本ずつ**走る。既定が並列になった以上、逃げ道が
  `-j1` (スイート全体) しかないのは粗すぎた。ジョブは 1 本のリストの
  ままで `serial_from` から後ろが直列なので、ジョブ id もエラー
  スロットもワーカーのカーソルも変わらない。AOT では `serial` な
  テストを**自分だけの driver** にする (ファイルの他のテストと共有
  すると、そちらまで直列になる — `serial` は隣について何も言って
  いない)。`panics` と順不同。`--list` が印を出す (「このスイートが
  なぜ速くならないか」の答えだから)。`poc/logsearch` の
  `admin/repair` は共有マウントに**書く**唯一のテストなので付けた。
- **統合が運んでいなかった 3 つ目と 4 つ目 — `call_paths` と
  `parallel_loops`** — プールの脇にある表 (プール参照がキー) を
  統合が写していなかったので、**モジュールの中では**パーサが記録した
  事実が丸ごと無かった。`parallel_loops` が効く方で、
  **モジュールの中の `parallel for` はただの `for` だった** —
  本文が `println` できて (E0029 が走らない)、lowering も切り出さない。
  逐次の答えを先に固定したはずの構文が、その構文ではなかった。
  `call_paths` は P3 の沈黙の残り半分 (`zzz::tbl::f()` をモジュールの
  中に書くと通る)。`const` と合わせて**同じ一家の 4 件目**。
- **MODULE-CONST-PATH — `const` の修飾子が検査されるようになった
  (`[E0030]`)** — 呼び出しは P3 で検査されるのに、定数は修飾子を
  捨てていたので `zzz::BASE` が通っていた。書かれたパスは元から
  木に在る (パーサは修飾付き識別子の全セグメントを残す) ので、
  足したのは読む側だけ。**選ぶのではなく検査する** — 名前空間は
  平らなので修飾子は選択に使えないが、間違っていることは言える。
- **モジュールの診断がそのモジュールのソースを出すようになった** —
  span は正しいファイル (`FileId`) を持ち JSON も正しかったが、
  テキスト整形器にソースマップが渡っていない経路があり、
  **正しい行番号を間違ったファイルから読んで** `<line not available>`
  を出していた。`compiler` と `run_source` / `prepare_tests` /
  `effects_from_source` の 5 か所。
- **DBC-RESULT-FIELD — compound を返す関数が `result` の中身を
  契約で言えるようになった** — `result` は**先頭の戻り値 1 本**に
  スカラーとして束縛されていたが、compound の戻りは leaf ごとに 1 本
  なので、`ensures result.cap == n` は `field access on a non-struct
  value` で拒否されていた (構築子が自分の作ったものについて何も
  言えない)。戻り型の形どおりに leaf を束縛する。method 呼び出し
  (`ensures result.size() == 32u64`) と tuple (`result.0`) も同じ理由で
  直った。**`sha256::sum` が出力長を、`Vec::with_capacity` が容量と
  長さを契約で言う**ようになった。
  副産物として **`val` の右辺の位置が backtrace に入るようになった** —
  compound を返す呼び出しは `lower_expr` を通らずに lower されるので
  現在位置が更新されず、`make (called at line 14)` のように**ファイルの
  末尾**を指していた (tree-walker は正しく 10 と言う)。
  `interpreter/example/contracts.t` が AOT の skip リストから外れた。
- **STDLIB-FN-SHADOWED-BY-USER-FN — モジュールの body は自分の
  モジュールを先に見る** — 素の名前は**必ずユーザ側の表を先に**
  引いていたので、`core/std/time.t` が自分の `pad2_field` を呼ぶと
  ユーザの同名関数に解決していた。大きい方の症状は stdlib の body が
  誤ったシグネチャで検査され、**ユーザのファイルの存在しない行**を
  指してエラーが出ること。**静かな方**はシグネチャがたまたま合った
  場合で、全レーンが黙って違う関数を呼ぶ (`lib2::report(2)` が
  20 ではなく 2000 を返す)。`import` はモジュールの名前を
  プログラムに見せる仕組みであって、その逆ではない — モジュールは
  何にインポートされるか知らずに書かれている。
  `Function` / `MethodFunction` / `TraitMethodSignature` が
  **自分のモジュールを持つ**ようになり (統合が刻む)、3 つの解決器
  (型検査器 / tree-walker / lowering) が同じ規則を読む。
  `.toycache` の schema は 50 に。
- **TRAIT-CONTRACT-EXPRREF — モジュールの trait が契約を持てるように
  なった** — 署名の `requires` / `ensures` は**モジュールのプール**を
  指す `ExprRef` なのに、統合がそれを写さずそのまま運んでいた。
  結果、各節は本体側プールの同じ添字にたまたま在る節点を指し、
  `[E0010] requires clause must be of type bool, got Unknown` が
  **無関係なファイルの無関係な行**を指して出ていた (カレットが
  コメント行に載る)。関数の節と同じ `map_expr` を通すだけ。
  `trait Digest` が約束を散文で書いていた理由で、DBC-LISKOV の
  `[E0023]` が出していた**「trait 側に移せ」が従える指示になった**。
  `trait Digest` の `output_size` / `block_size` に
  `ensures result > 0u64` を入れてある。
- **CONST-ARRAY — `const K: [u32; 64] = [...]` が全レーンで読めるように
  なった** — 定数配列は `.rodata` のバイト列になり、添字は
  `InstKind::ConstBytesAddr` + `PtrRead` 1 回。境界検査はスタック配列と
  同じ経路を通るので、範囲外は同じ文面と位置で panic する。
  **`Sha256` の K 表が `Vec` から `const` に移り、`compress` が
  `never_allocates` を名乗れるようになった** — 64 回の `push` と
  ハッシャごとの 256 バイトが消えた。narrow なリテラル
  (`99u32`) も定数評価器が読めるようになっている (`Expr::UInt32` 等
  6 種と char リテラルを知らず、`const S: u32 = 99u32` が
  「評価できない」と言われていた)。
- **MODULE-CONST — モジュールの `const` が見えるようになった** —
  統合が module の `const` を 1 つも運んでいなかったので、
  **そのモジュール自身の関数本体からも**見えなかった
  (`[E0003] Identifier 'K' not found` が、それを宣言している
  ファイルの行を指す)。型検査器の const 登録が**統合の前**の
  スナップショットを見ていたのも同じ穴の片側。名前空間は関数と同じく
  平らで、先に定義したものが勝つ。`core/std/poll.t` が
  `pub fn interest_read()` と書いているのはこれが理由だった
  (stdlib 側の書き換えは別途)。
- **CONCURRENCY A2-b-2 — `parallel for` が本当に並列に走るように
  なった** — lowering が本文を関数に切り出し、`InstKind::ParFor` が
  AOT / JIT では `toy_par_for` に、IR VM では「1 区間として 1 回
  呼ぶ」になる (逐次も合法な分割)。実測は 64 反復の重い本文が
  **23 ms → 3 ms (8 スレッド)**。捕捉は呼び出し元フレームのスロットに
  置くので**確保ゼロ** (`ensures allocates(0)` の中に置ける)。
  渡すのは添字ではなく**個数**なので `-3i64..3i64` も特別扱い無しで
  通る。**捕捉への書き込みは断る** — env に入るのは写しで、書いても
  届かず、届いていたら競合している。検査は生成後の IR に対して、
  **writeback の枝刈りの後**に行う (`v.set(i, x)` は ABI 上 `v` の
  全 leaf を返すが 1 つも書かないので、枝刈り前に訊くと並列本文で
  いちばん普通の行が断られる)。A1 の consistency テストは
  アキュムレータの 2 本以外**1 つも変わっていない** — 逐次の答えを
  先に固定した狙いどおり。
- **E0029 が `break` / `return` と外側の名前への代入も断るように
  なった** — どれも順序が観測できる側で、lowering の都合ではないので
  型検査器で断る (でないと tree-walker だけがアキュムレータを平然と
  回す)。本文の中で宣言した `var` は反復ごとなので自由に書ける。
- **効果解析が `Option::Some(x)` を「本体の無い呼び出し」と見なさなく
  なった** — enum の variant 構築は呼び出しの形に parse されるが本体が
  無いので、`enter` が opaque (= 全効果) を与えていた。結果
  **`Option` を作る関数を呼ぶ `parallel for` は軒並み「prints」で
  断られて**いた (`the loop body -> pick -> Some`)。値を作るだけなので
  効果は引数のぶんだけ。`never_allocates` / `const fn` も同じ表を
  読むので、そちらの偽陽性も消えた。
- **IR VM がフレームスロットを確保として数えなくなった** — `&dyn
  Trait` の coercion スロットを `alloc_at` で取っていたので、
  compiled レーンが cranelift のフレームに置く同じものが、この
  レーンだけ `__builtin_live_bytes()` に出ていた。`alloc_internal`
  (address-taken local のために用意された、カウンタに届かない確保) に
  寄せた。
- **CONCURRENCY A2-b-1 — ランタイムが範囲を並列に回せるようになった**
  — `toy_par_for(from, until, env, body)` を `toylang_rt` に追加。
  範囲を割ってスレッドに配り、**呼び出し元も 1 区間を担当する**ので
  1 スレッドなら spawn は 0。スレッド数は `TOY_PAR_THREADS` か
  `sysconf`、上限 64。ジョブは呼び出し元のフレームに置くから
  **確保ゼロ**。`no_std` なので pthread を直接呼ぶ (既存の
  pthread_key と同じ流儀)。**切り出し (A2-b-2) はまだ** — 置き場所は
  フロントエンドではなく lowering だと分かったので、CONCURRENCY.md の
  該当節を書き直した (フロントエンドは宣言を新造できないが、
  lowering は単相化と drop glue で日常的にやっている)。
- **予約語を名前に書いたときの診断が、どの語かを言うようになった** —
  3 か所が 3 通りに壊れていた: `fn f(to: u64)` は `ParenClose`
  (本当のエラーを引数ループの回復が握り潰していた)、`val to = ...` は
  「reserved keyword 'keyword'」(手書き match の catch-all)、struct
  フィールドは「expected field name」で理由を言わなかった。
  `Kind::keyword_spelling` / `keyword_hint` の 1 つの表に寄せ、
  `to` のように紛れやすい語には行き先も添える。ブロックの回復が
  内側のエラーを `{:?}` で包んでいたのも直した (構造体ダンプの中に
  文面が埋まっていた)。テストヘルパも `Display` を使う。
- **CONCURRENCY A2-b の設計を訂正した** — env struct に
  `&mut Vec<u64>` を持たせる案は**書けなかった**
  (`struct S { r: &u64 }` は REF-Stage-2 (e) が拒否する)。持てるのは
  **スカラーと窓** (`Span<T>` / `Column<T>`) だけで、窓なら `set` で
  元のバッファに書ける — 添字で分かれた書き込みという需要にはそれで
  足りる。3 レーンで動く形を consistency に置いた。捕捉できるものが
  窓とスカラーに限られることは、§5-3 の「disjoint は規約」を書きやすく
  もする (書き込み先が窓の添字しかない)。
- **ARRAY-REPEAT-LITERAL — `[0u8; 64]` が書けるようになった** —
  parse エラーだったので、固定長の作業領域は要素を全部並べるしか
  なかった。パーサが展開するので**バックエンドは砂糖を見ない**。
  繰り返せるのは**リテラルだけ**で、呼び出しを書くと断る — 任意の式は
  要素ごとに評価されることになり、それはこの形の意味ではない
  (Rust が `Copy` を要求するのと同じ理由)。
- **MODULE-SYSTEM P3 (`mod.t`) — ディレクトリ自身の名前になった** —
  auto-load の walker がファイル名をそのまま段にしていたので
  `<root>/geo/mod.t` は `mod::` で呼ぶしかなく、`import geo` の解決
  (`candidate_module_paths` は `geo/mod.t` を探す) と食い違い、
  ツリー中のすべての `mod.t` が同じ名前で衝突していた。`mod.t` は
  段を足さずディレクトリの段を名乗る。ルート直下の `mod.t` は
  名乗るものが無いので読み飛ばす (名前を発明するより黙る)。
  **これで P3 は全部埋まった。**
- **E0026〜E0030 の `origin_module` は `None` のままでよい** — この
  項目は「全体パスの診断がどのモジュールか言わない」だったが、
  `Diagnostic::anchor_in` が `file` を span の実ファイルにしたので、
  この欄が守っていた不変条件 (「span は `file` に無い」) は成立して
  いない。意味を「出どころのヒント」に書き直した。
- **compound 要素の drop glue が `f32` leaf を通るようになった** —
  glue のシグネチャを組む leaf 型の一覧が `f32` 以前に書かれたもので、
  誰も足していなかった。`Vec<S>` の `S` に `f32` フィールドがあるだけで
  compiled レーンが `drop glue: unsupported leaf type f32` で拒否して
  いた。leaf は何も所有しない (`f64` と同じ) ので、足りなかったのは
  名前だけ。
- **MODULE-SYSTEM P3 (後半) — 余分なセグメントが解決に参加するように
  なった** — `a::dup::f()` と `b::dup::f()` が**別の関数として解決
  する**。2 つの解決器 (`context::lookup_fn_detailed` と
  `compiler_ir::lookup_function`) は元から多セグメントの修飾子を
  末尾一致で受けていて、呼び出し側が 1 セグメントしか渡していな
  かっただけだった。型検査は `visit_expr` と `check_expr_located`
  (文・末尾式の経路) で `File::call_paths` を拾い、lowering は
  `written_qualifier_at` で同じものを引く — **両者が同じ slice を
  使う**ことが、型検査した呼び先と lowering した呼び先が一致する
  理由である。P2 の曖昧エラーの文言も「ファイル名を変えろ」から
  「セグメントを増やせ」に変わった。
- **MODULE-SYSTEM P3 (前半) — 書いたモジュールパスが検査されるように
  なった (`[E0030]`)** — パーサが 3 セグメント以上のパスを**全部
  捨てて**いたので、`zzz::math::min_i64(..)` は `min_i64(..)` として
  解決し、誰も辿れないパスが黙って通っていた。全パスを
  `File::call_paths` に脇に記録し (`parallel_loops` と同じ手口)、
  専用パスが「書いたパスが実在するか」を見る。**解決規則は P2 のまま**
  (最寄りのセグメントで引く) で、余分なセグメントは検証されるだけ。
  最寄りのセグメント自体が無いときは黙る — 既存の診断が同じ位置で
  同じことを言うので、1 つの間違いに 1 つの診断。
- **CONCURRENCY A2-a — shadow stack が per-thread になった** —
  `toy_shadow_stack` / `toy_shadow_depth` の 2 つのグローバルを、
  `toy_shadow_ctx()` が返すスレッドごとの `{ depth, slots }` に
  置き換えた (既存の pthread_key TLS の上)。backtrace は
  スレッドごとのもので、2 つのスレッドが 1 つの depth を共有したら
  どちらの backtrace も正しくない。codegen は prologue で 1 回
  呼ぶだけ (番地は活性化の間は定数なので hoisting の前提は不変)、
  `--release` は frame を記録しないので 0 コスト。実測は呼び出し
  しかしないマイクロベンチで 0.13 → 0.30 秒、実仕事で +4%。
- **`toy test` の driver が 1 件も走らずに落ちたとき、理由を捨てて
  いた** — 「not run: an earlier test ended the process」が全件に
  付くだけで、**その「earlier test」は存在しない**。driver の
  stderr と終了コードを報告するようにした。

### 2026-09-20

- **MODULE-FN-REF-ARG は既に直っていた** — 「module の自由関数が
  `&compound` を取り scalar を返すと lowering が落ちる」は、
  `random::shuffle(&mut v)` を直したとき (module-call の経路が呼び先を
  渡すようになり、`&T` パラメータが leaf ではなく番地を受け取る) に
  一緒に消えていた。**誰も pin していなかった**ので todo に残り続けて
  いた。`time::format(&DateTime, str) -> str` が stdlib にあるその形
  なので、consistency に置いた。
- **CONCURRENCY A1 — `parallel for` の意味論が入った (実行はまだ逐次)**
  — §5 の論点 1〜4 を決めて (構文にする / 並列度はコア数・意味論では
  ない / disjoint は規約 / 逐次レーンは完全逐次)、構文と検査を landing
  した。ループは普通の `Stmt::For` で、`parallel` が付いたことは
  `File::parallel_loops` に**脇に記録する** (`transferred_bindings` と
  同じ手口) ので、気にしないパスは 1 行も変わらない。本文は出力
  (`Io`) と `with allocator` を禁じる (`[E0029]`)。**4 レーンとも
  逐次で実行する** — 答えを先に固定してから並列化を足すと、並列化は
  答えを変えられない最適化になる。残りは A2 (pthread、shadow stack の
  per-thread 化、本文の切り出し)。
- **BORROW-MATCH-DROP — 借用越しの `match` が payload を解放しなくなった**
  — `match v.borrow(i) { Some(s) => .. }` の腕が payload に drop を
  付けていた。tree-walker は**無条件に**、compiled レーンは「同じ
  local を覆う生きた drop 対象が無いとき」— 借用は何も所有しないので
  対象が無く、まさにこの条件に当たる。**スロットを読むだけで容器の
  持ち物が解放されていた**。腕が drop を取るのは「他に誰も持って
  いないとき」だけ、という元の規則はそのままで、判定に (a) 借用 /
  転送された束縛の local (compiled)、(b) 場所式かどうか (tree-walker)
  を足した。`poc/logsearch` の接続表が踏んだ — 表を走査した瞬間に
  ソケットが閉じた。
- **ENUM-ARG-NESTED-LOWER — `Vec<Option<T>>` が lower されるように
  なった** — 型引数の置換が enum を `Enum(..)` の綴りでしか認識せず、
  パーサが渡す `Struct("Option", [T])` を取りこぼしていた
  (フィールドの位置には同じ arm が既にあった)。構造体が丸ごと
  lower 不能になり、診断は 1 つ上の「cannot lower parameter
  `t: &mut Table`」として出ていた。
- **ASSOC-FN-REF-ARG — associated function が `&compound` を取れるように
  なった** — `String::join(&parts, &sep)` が JIT / AOT で
  `call argument produced no value` だった。`Type::f(args)` の 3 つの
  形 (struct 戻り / enum 戻り / scalar 戻り) だけが**引数を 1 つずつ、
  呼び先を知らずに** lowering していたので、`&`-compound の
  パラメータが番地を欲しがることを誰も知らなかった。自由関数と同じ
  `lower_call_arg_items(args, Some(target))` に揃えた。
- **MODULE-DIAG-POSITION — モジュール内の診断がファイルと行を言うように
  なった** — 行と列を**エントリのファイル**の本文から計算し直していたので、
  モジュールの 5 行目のエラーが「3 行目」になり、しかも**エントリを
  編集すると数字が動いた**。位置は自分のファイルで解決するようにし、
  text はそのファイルからスニペットを描き、JSON の `file` も
  (`Diagnostic::anchor_in`) そのパスを載せる。E0028 の移行で 42 件が
  全部 `main.t` の行末を越えた行番号を名乗ったのが発見のきっかけ。
- **ELEMENT-BORROW / CONTAINER-ELEM-DROP — 容器は要素を貸せるようになり、
  値で取り出すのは拒否されるようになった** — `val e: T = v.get(i)` は
  要素の**別名**なのに所有者として扱われ、`T` が所有型なら容器がまだ
  指している資源を解放していた (fd なら EBADF)。`&T` を返す `borrow` を
  `Vec` / `Box` / `Span` / `Column` に、`Option<&V>` を `Dict` に新設し
  (E1〜E3)、参照の返却 (引数の再借用のみ) とローカル束縛を許して
  `[E0026]` / `[E0027]` で囲った。stdlib 自身の 16 か所が**実バグ**
  だった (`Vec::clone` / `Box::clone` / `json` 12 か所 / `String::join`)。
  最後に **`[E0028]`** で「所有型を返す `get` から束縛し、その値を誰にも
  渡さない」形をエラーにした (E5)。`SoaVec` に `borrow` は無い — 列に
  散った要素に貸せる番地が無いので、**所有型を `soa Vec` に入れない**。
  設計は [`ELEMENT_BORROW.md`](ELEMENT_BORROW.md)。
- **VEC-REPLACE — スロットの中身を入れ替えて、元を受け取れるようになった**
  — `Vec::replace(index, value) -> T`。固定スロットの表 (`Vec<Option<T>>`)
  から所有型を**取り出す**手段が無かった: `remove` は詰め、`swap_remove`
  は最後を持ってくるので、どちらも添字が指すものを変えてしまう
  (poller の token のように**添字が名前**のときに使えない)。`set` は
  上書きするだけで、載っていたものは誰にも解放されない。
  `poc/logsearch` の `tests/server.t` が `Vec<Option<TcpStream>>` で
  「持つ・貸す・空ける」を通し、空けたときに**相手が EOF を見る**ことで
  所有が戻っていることを確かめている。
- **MOVE-CHECK-QUALIFIER — move 検査が呼び先を修飾子で引くようになった**
  — 関数表が名前 + arity だけだったので、利用者の `fn sum(c: Cell)` と
  `sha256::sum(&Vec<u8>)` が 1 つの曖昧な項目に潰れ、`sum(x)` が借用
  扱いになっていた (= 転送が記録されず、渡した先とローカルの両方が
  解放する)。`File::function_module_paths` が「どのモジュール由来か」を
  持っているので、(a) エントリ由来は名前 + arity、(b) モジュール由来は
  **モジュールの末尾セグメント** + 名前 + arity で引く形に分けた。
  bare 名はエントリ由来が勝ち、`sha256::sum(..)` は修飾子で一意に解決
  する — 実行時の解決規則と同じ。
- **MOVE-CHECK-OVERLOAD — move 検査が同名の別署名を取り違えなくなった**
  — 関数表が名前だけを鍵にしていたので、`Box::set(value)` と
  `Vec::set(i, value)` が衝突して**すべての `set` が借用扱い**になり、
  `self.nodes.set(id, n)` の転送が記録されず json が読んだノードの文字列を
  解放していた。利用者の `fn sum(l: List)` を `sha256::sum(&Vec<u8>)` が
  上書きする形も同じ穴。名前 + arity を鍵にし、食い違えば答えないようにした。

### 2026-09-19

- **MATCH-PAYLOAD-COPY — `match` の腕は payload を名指すのであって
  複製しない** — 腕の `Name` 束縛は compiled レーンで payload の**複製**
  を作り、自分の drop glue も持っていた。scrutinee 側も持っているので
  **1 つの資源に所有者が 2 人**。heap ブロックでは見えない (アドレスを
  再利用しないヒープで `free` は冪等) が、OS が 1 度しか返さないもの —
  fd — では致命的で、`match listener.accept() { Result::Ok(c) => ... }`
  が**受け取った瞬間に接続を閉じていた**。tree-walker は別名なので
  閉じず、4 レーンで意味が割れていた。腕は別名を張るようにし、drop は
  「まだ誰も持っていないとき」(一時値やパラメータを match したとき) だけ
  腕に付ける。`poc/logsearch` の接続表が踏んだ。

### 2026-09-18

- **STR-PTR-UNCOUNTED — `str::as_ptr` が確保カウンタを動かさなくなった**
  — tree-walker だけが `__builtin_str_to_ptr` の受け皿 (`len + 1` バイト)
  を**プログラムの確保として数え、しかも誰も解放しない**ので、
  `String::from_str` を呼ぶたびに live バイトが増えていた。IR VM は
  最初から `alloc_uncounted` で、compiled レーンは `.rodata` なので
  0 — カウンタの意味がレーンで割れていた
  (MEM-COUNTER-INTERP-DRIFT が固定した定義に戻した)。tree-walker は
  `--check` と consistency harness のオラクルなので、**メモリの約束を
  オラクル側で検査できるようになった**。
  `poc/logsearch` の定常性テストが見つけた (当初は
  「`Vec<String>` の要素が解放されない」と読んでいたが、要素ではなく
  文字列リテラルの受け皿だった)。

### 2026-09-17
- **RANGE-FOR — `for i in r` が範囲値で動き、範囲値が 3 レーンに
  入った** — パーサは `in` の後の名前をイテレータとして desugar するので
  `r.next()` が無くて落ちていた。型検査器が受け手の型が `Range<T>` の
  ループを `for i in r.start..r.end` に書き戻す (消費しない。境界は
  ループ開始時に 1 回読む)。`r.start` / `r.end` を足し、compiled lane は
  `Binding::Range` (境界 2 local) で束縛・再代入・表示・補間に対応。
  tuple 要素と分岐からの生成は compiled lane が拒否する (tree-walker は通る)。
  範囲値が `i64` / `u64` 限定で `u8..u8` を「型不一致」と言っていたのも
  直した。`start` / `end` は `BuiltinFunctionSymbols::new` で seed
  (cache v46)。
- **MODULE-EXPR-REMAP — モジュールの body に配列の添字を書くと
  integration が落ちていた** — `remap_expression` に `SliceAccess` /
  `SliceAssign` / `DictLiteral` / `Range` / `Closure` / `StructUpdate`
  の腕が無く、`Unsupported expression type for remapping`。入口ファイルは
  remap を通らないので、同じ関数を `src/` に移したときだけ落ちた
  (`poc/logsearch`)。catch-all を消して `Expr` を網羅したので、
  次に variant を足すと remap の書き忘れはコンパイルエラーになる。

### 2026-09-11
- **TEST-PARALLEL P0〜P3 — `toy test` が並列に走る** — `-j N` (既定は
  コア数、`-j1` は従来の逐次経路、`--bless` は暗黙に `-j1`)。plan も
  実行も同じカーソルで配る。VM レーンはテスト 1 本がジョブ、AOT は
  driver 1 本がジョブ。`poc/logsearch` の VM が 4.83 → 0.80 s。
  前提バグ 5 件 (テストバイナリ名の衝突 / `.toycache` の非アトミック
  書き込み / VM の filter が走らせてから捨てていた / 破棄ごとの
  グローバル Mutex / `TOY_BLESS` の `set_var` 位置) も同時に。
  **P4 (所要時間の記録と longest-first) は同日に取り下げた** —
  ビルドディレクトリに run をまたぐ状態を増やさない判断。効いていたのは
  ワーカーを絞ったときだけ (偏ったスイートの `-j4` で 1.30 → 0.94 s、
  既定のコア数では 0.02〜0.09 s)。測定は
  [`TEST_PARALLEL.md`](TEST_PARALLEL.md) の「P4 を取り下げた」に残した。

### 2026-09-06
- **TRY-COMPOUND / COMPOUND-BLOCK-RHS / COMPOUND-GENERIC-INSTANCE —
  `?` が compound を運べるようになった** — `val f = File::open(p)?`
  と、struct / tuple / enum / generic instance のどれでも通る。3 つの
  修正が要った: (a) desugar の `__try_v as T` は **scalar のときだけ**
  吐く (`as` は全レーンで scalar 変換なので `Point as Point` は
  no-op ではなく拒否だった。`??` も同じ)、(b) `val x = { .. match .. }`
  の**検出が block 自身の先頭束縛を見る**ようになった (検出は lowering
  前の peek なので `val t = mk()` がまだ `bindings` に居らず、arm 束縛の
  型を引けなかった。注釈か素の call の戻り型から引く)、(c) 検出が
  base name だけでなく**具体の instance を運ぶ**ようになった
  (`Result<Vec<u64>, E>` の payload は既に実体化済み。注釈の無い
  generic を初めて解決できる — COMPOUND-GENERIC-INSTANCE がこれで消えた)。
  加えて tree-walker の block 退出時の drop 免除を **identity から
  包含関係に**広げた: compound は alias なので、block が渡した値が
  block 内束縛の payload だと、その束縛の drop が渡した先を壊していた
  (`File` は fd が閉じるので見える。`Vec` はバッファが読めてしまい隠れる)。
  `poc/logsearch` の `File::create` / `File::open` の手書き `match` 2 か所を
  `?` にして出力一致を確認。例: `interpreter/example/try_compound.t`。

### 2026-09-10
- **REF-REBORROW の残り 2 経路 — module 呼び出しと generic 推論** — 型検査の引数検査は 4 か所あり、最初の landing で自由関数と method の 2 つしか直していなかった。`query::resolve_indexed(traw, tlen, &q, out)` (非 generic module) と `random::shuffle(v)` (generic module) が今も借用を要求していた。後者は制約解決の**前**に書き換える必要がある — 内側の型は推論中なので、両辺が `&mut` であることだけを見る。

### 2026-09-05
- **toylang 側のリファクタリング (stdlib / poc)** — 言語に入った機能で
  手書きの定型を畳んで **-198 行**。`checked_pow` 8 本の
  `match { Some(v) => .., None => return None }` を `?` に
  (96 → 16 行)、`net` / `poll` の「status を `Result` に変える」
  23 + 4 か所を `net_unit_result` / `net_u64_result` / `net_str_result`
  に集約、iterator アダプタ 4 本の drain ループを `for v in it` に、
  `Vec::with_capacity` / `grow_to` の容量オーバーフロー検査を `??` に。
  `poc/logsearch` は `arg_u64` / `top=` / `parse_time` / `verify` /
  `Reader::load` の 5 か所。**`net_result<T>` の 1 本で済ませられない**
  のは、generic 関数が compiled レーンの enum を産む tail 位置に立てない
  ため (`unknown function .. in enum-producing position`)。
- **REF-REBORROW — `&mut` 引数の転送に借用を書かなくてよくなった** — `fn insert(arena: &mut Vec<Node>, ..)` の中の `insert(arena, ..)` が通る。**再借用だけ**を暗黙にした — 所有値から `&mut` を取るのは今も明示が要る (呼び先が値を変えてよいかの決定だから)。転送は何も決めない (呼び出し側は既に可変アクセスを与えられており、渡しても増えない)。型検査器が明示形に書き換えるので**下流は略記を見ない** — writeback の帳簿もその形を見ている。
- **BY-VALUE-SELF-ALIAS — 値渡しの引数は callee 自身のコピーになった** — tree-walker が `Rc` を共有していたので、`self: Self` / 値渡しの compound 引数への書き込みが呼び出し側に漏れていた (compiled レーンは leaf をコピーするので漏れない)。`BACKEND.md` の規定どおり tree-walker を直した。コピーは**構造的で深くない** — compound の背骨を作り直し scalar で止まるので、`Vec` の heap buffer は共有のまま。この挙動に依存していた 2 か所 (`interpreter/example/allocator_list.t` と struct の `__setitem__` テスト) は、どちらも `&mut self` のつもりで書かれていたので直した。
- **CODE-SIZE-SELF-ABI — 演算子オーバーロードと method の参照引数も番地で渡す** — 被演算子を作る前に呼び先は解決済みなので、埋める引数枠を渡すだけで除外リストが消えた。`fn add(&self, o: &Self)` は 13 → 2 引数。ついでに「ポインタ引数は writeback slot を持たない」を呼び出し側の数え方と body 側の導出にも通した (前者は数が合わず黙って素の `Call` に落ちていた)。`poc/logsearch` の `__text` は 191,080 → 153,056 B (一連の作業で −19.9%)。[`CODE_SIZE.md`](CODE_SIZE.md)。
- **CODE-SIZE-SELF-ABI S3b — 幅の広いローカル束縛を stack slot に常駐** — 鎖の根が呼び出しごとに slot を作り直すのをやめ、生涯 slot に住まわせる。`AddressOf` も囲っている記憶域を返すようにして「leaf の家は 1 つ」を守った (`&mut wide.field` がここで壊れていた)。`cmd_archive` 2,706 → 1,211 命令、`poc/logsearch` の `__text` は 191,080 → 156,252 B (一連の作業で −18.2%)。[`CODE_SIZE.md`](CODE_SIZE.md)。
- **CODE-SIZE-SELF-ABI S3a — `&T` / `&mut T` の compound 引数もポインタで渡す** — receiver と同じ扱いを参照引数に広げ、受け取った pointer param はどれでも転送元になる。`flush_segment` 1,384 → 455 命令、`poc/logsearch` の `__text` は 191,080 → 162,912 B (一連の作業で −14.7%)。[`CODE_SIZE.md`](CODE_SIZE.md)。
- **CODE-SIZE-SELF-ABI S1+S2 — 幅の広い by-reference receiver をポインタで渡す** — leaf 8 個超の `&self` / `&mut self` は 1 本の番地で渡り、codegen が leaf の `LoadLocal` / `StoreLocal` をポインタ経由の load/store に読み替える。鎖を下るときは番地をそのまま転送する。`poc/logsearch` で `__text` 191,080 → 167,736 B (WB-PRUNE と合わせて −12.2%)、実行も ~4% 速い。設計は [`CODE_SIZE.md`](CODE_SIZE.md)。
- **CODE-SIZE-WB-PRUNE — `&mut self` が書かない leaf を返さなくした** — lowering 後に writeback slot を落とす pass。不動点まで回すので callee → caller と連鎖する (`write_seg` の戻り 53 → 1)。`poc/logsearch` で `__text` −10.3%。設計は [`CODE_SIZE.md`](CODE_SIZE.md)。
- **リファクタリング (frontend / compiler / interpreter)** — 重複と
  手書きの冗長データを 7 か所落として **-516 行**。compiler:
  `CodegenSession::new` の 577 行の signature 組み立てを
  `SymbolImporter` + `abi` / `sext` / `uext` で 1 宣言 1 行に
  (declare 順は 81 個とも不変)、`define_function` /
  `lower_function` を `prepare_function_context` に寄せて 130 行の
  三重複製を削除、`CallIndirectFn{Struct,Tuple,Enum}` の同一 3 arm を
  1 メソッドに、JIT の 159 シンボル登録を
  `stringify!` マクロに (名前とポインタの取り違えが書けなくなる)。
  frontend: `BuiltinFunctionSignature::arg_count` は**どこからも
  読まれず** 36 行で手書き保守されていたので削除、表は `sig(...)` 1 行に。
  interpreter: property trial の const 初期化 45 行の重複と、
  `*_from_source` 4 本の parse / 型エラー報告の前置きを共通化。
  併せて `gen_block` の doc コメントが toylang を Rust doctest として
  コンパイルしていた既存の失敗を修正 (`cargo test --doc --workspace`)。
- **MODULE-IMPORTS D1 — `import a.b as h` が効くようになった**
  ([`MODULE_IMPORTS.md`](MODULE_IMPORTS.md))。alias はパーサが受理して
  `visit_import` が捨てていたので `h::f()` は `Struct 'h' not found`
  だった。**パーサが alias を末尾セグメントに置換する**ので、qualifier を
  末尾一致で解決する既存経路 (MODULE-SYSTEM P2) がそのまま効き、
  型検査器 / tree-walker / IR lowerer は alias を知らない —
  **同じ解決規則を 4 か所目に増やさない**のが要点 (rank を 1 か所落として
  型検査と実行が食い違った件の再発防止)。alias はファイル局所なので、
  1 ファイルしか見ないパーサが唯一完全に解決できる場所でもある。
  併せて `X::f(...)` の未解決 qualifier を
  `Type or module 'X' not found` に (以前は `Struct 'X' not found` で、
  module の綴り間違いを struct 探しに送っていた)。
  `FULL_AST_CACHE_SCHEMA_VERSION` 45
- **`toy version`** — toy / compiler / interpreter / stdlib の
  version + git revision + **パス**を 1 行ずつ。**パスが無い行は
  同じ行に色つきで警告**する (`NO_COLOR` / `TOY_COLOR` 対応、
  端末でないときは自動で消える)。効く問いは「どのリリースか」ではなく
  **「今動いているのは今ビルドした物か、どの stdlib に対してか」**。
  revision はビルド時に build script が埋めるが、
  **stdlib だけは実行時に `git -C <root>`** で引く — stdlib はデータで、
  バイナリを作った checkout と別のところから来うるので、
  その食い違いこそこのコマンドが可視化したいもの
- **TEST-TOOL T3 — `core/std/testing.t`** — `assert_close` /
  `assert_str_eq` / `assert_bytes_eq` / `assert_some` / `assert_ok` /
  `assert_err` / `assert_in_range(_u64)` と、確保の区間検査
  (`heap_mark` / `assert_no_growth` / `assert_growth_at_most`)。
  **失敗が位置と両辺を言う**のが要点で、`assert_bytes_eq` は
  最初に違うオフセットを出す (4 MiB が 1 バイト違うときに
  「違います」だけでは使えない)。走査は失敗時にしか走らない
  (`Span::bytes_eq` が先に yes/no を答えるので、通った側の費用は
  そのまま)
- **TEST-TOOL T4 — `test "..." panics { }`** (`panics "text"` で
  メッセージも照合)。VEC-CONTRACTS が `Vec` の境界を `requires` に
  したのに、**その契約が破れることを確かめる術が無かった**。
  AOT は panic がプロセスを終わらせるので**テスト 1 本 = バイナリ 1 本**
  (driver の filter を名前の集合にした — 1 本を除くとは他の全部を
  名指すこと)。`--backend all` (レーン間の食い違い報告) は未着手
- **TEST-TOOL T5 — golden ファイルと `toy test --bless`** —
  `testing::assert_golden(path, bytes)`。**無いときは失敗**で初回の
  自動記録はしない (一度も見られていないテストが緑になるため)。
  差分は T3 の関数が出すので「何バイト目から違う」が出る。
  テストは**パッケージ根から走る**ので、テストに書いた golden の
  パスが書いたとおりの意味になる
- **TEST-TOOL T1 — compiled レーンで `test` が走るようになった** —
  `assert` が literal メッセージしか受けなかったので、
  2 値からメッセージを組む `assert_eq` を含む `test` は
  **そもそもコンパイルできなかった**。`panic` は既に `PanicStr` で
  非リテラルを受けていた (ERROR_MODEL E3) ので、同じ落とし方に揃えた。
  メッセージは **fail ブロックの中で** lower する (false のときだけ
  評価する規約 + 通ったテストに報告の費用を払わせない)。
  entry は `compiler_lower::install_test_driver` が合成する —
  各テストの前に stderr へマーカーを出して呼ぶ `main` で、
  ユーザの `main` は `toy_program_main` に改名して残す。
  **最初の失敗で止まる** (panic がプロセスを終わらせるため)。
  `compiler --test` / `toy test` (既定 AOT) から使う
- **TEST-TOOL T2 — `main` の無いファイルの AOT が無関係なエラーを出す件** —
  `test` ブロックを entry として数えるようにした。以前は
  `main` だけが entry で、無ければ「全部 lower する」フォールバックに
  落ち、`log::level_from_rank is neither a variant ...` のような
  無関係な stdlib の body で死んでいた
- **`--test` が IR VM で全テストを黙って pass させていた** ★★★ —
  `execute_entry` は entry 関数を受け取るのに、IR VM / JIT の fast path は
  **`main` を走らせていた**。`assert_eq` が lower できなかったおかげで
  そういうプログラムは ineligible になり tree-walker に落ちていたので
  露見せず、T1 で lower できるようにした瞬間に**全テストが緑になった**。
  fast path を「entry が本当に `main` のときだけ」に絞った。
  literal `assert` を含む test は**以前から**黙って通っていた
- **`toy clean`** — `build/{debug,release}/` を消す。
  **リンクキャッシュは残す** (捨てると次のビルドが 30ms → 90ms に戻り、
  content-addressed なので古くなりようがない)。`--all` は `build/` ごと。
  削除する前に全パスを `Package::is_build_output` で照合する —
  「出力を消す」と「パッケージを消す」の差はパス計算 1 つなので、
  正しいはずのものを検査する
- **`toy` の出力レイアウトを決めた** — `build/{debug,release}/` で
  profile を分ける (`--release` は契約を消すので**別のプログラム**であり、
  同じパスだとディスク上のファイルがどちらか言わなくなる)。
  `toy run` は `build/{profile}/.run/` に出す (build の成果物を
  上書きしない)。テストは `build/{profile}/tests/`。
  `build/.gitignore` を初回に自動生成。リンクキャッシュは
  content-addressed なので profile 共通
- **`toy build` / `check` / `test` の既定を AOT にした** —
  `check` は型検査に加えて lowering まで走らせるので、
  「型は通るが AOT が拒否する」形をここで捕まえる
- **BARE-NAME-COLLISION — 後の module root が bare 名を勝ち取るようにした** —
  B0 は module **パス**の解決に「後の root が勝つ」を入れたが、
  bare 名 (qualifier 無しの呼び出し) は素通しで、パッケージ自身の
  `fn parse` が `std::json::parse` に対して**単なる 3 つ目の候補**
  だった。つまり stdlib が同名の関数を生やした日にユーザの private
  helper が壊れる (BUILD_TOOL §1 穴 2)。同じ規則を 1 段下に適用した。
  **修飾付きの呼び出しは対象外** — `hex::encode` は module を名指して
  いるので、後の root に在るからと別のものを返すのは誤答。
  rank は `File::function_module_ranks` として 3 か所
  (型検査器 / interpreter の `QualifiedFunction` / IR の
  `FunctionEntry`) に届ける — **1 つでも落とすと型検査した関数と
  実行される関数が食い違う** (実際 interpreter 側を落として踏んだ)。
  4 レーン一致をテストで固定。`FULL_AST_CACHE_SCHEMA_VERSION` を 44 に
- **`ambiguous` の診断が用途で分かれた** — bare 呼び出しに
  「Two modules cannot share a file name — rename one of them」と
  言っていたが、`std::base64` と `std::hex` はファイル名が違う。
  修飾付き (`dup::f`) は従来の文言、bare は
  「どちらの module か書くか、後に来る root に自分の定義を置け」に
- **TEST-TOOL T0 — module の中に `test` ブロックを書けるようにした** —
  `assert_eq` はパーサが文字列連結に desugar するマクロで、統合の
  remapper に `Expr::BuiltinMethodCall` の腕が無かった。つまり
  **`test` を持つモジュールは統合できず**、テストを書ける場所は
  entry (この言語で唯一モジュールでないファイル) だけだった —
  `poc/logsearch` が 5,000 行でテスト 0 件だった一番の理由。
  併せて **`TestCase` がモジュールから運ばれるようにした** (0 引数
  関数の方は元から写っていたが、それを名指すエントリが無かったので
  「死んだ関数」になっていた)。名前は `mathx::triple works` のように
  module 修飾し、`TestCase` に `file` を足して失敗が自分のファイルを
  引くようにした
- **BUILD-TOOL B2 — `toy test`** — `tests/*.t` と entry を走らせ、
  module のテストは統合経由で拾う。名前の部分一致フィルタ /
  `--list` / `--format=json`。同じ module のテストは
  `(file, line, name)` で畳むので、取り込んだプログラムの数だけ
  重複しない。1 プロセスで全部走る
- **BUILD-TOOL B4 — bare 名の衝突を事前に報告する** — root の並びは
  parse 前に分かるので、コンパイラが**呼び出しに到達したとき**に
  出す `[E0010] ambiguous module path` より早く言える。
  **stdlib 内だけで閉じた重複は報告しない** (パッケージ側に打つ手が
  無く、毎回出る警告は読み飛ばしを教えるだけ)。`--no-warn-collisions` で切れる
- **BARE-NAME-COLLISION の実例が 3 件見つかった** (上の B4 が最初に
  出したもの): **stdlib 自身の `encode` / `decode` が
  `std::base64` と `std::hex` に重複**していて、bare な
  `encode(...)` は既に曖昧。`poc/logsearch` 側は `decode`
  (`src/lsz.t`) と `parse` (`src/record.t` vs `std::json`)。
  どれも呼び出しに到達するまでコンパイラは黙っている
- **BUILD-TOOL B0 — `--core-modules` を繰り返し指定できるようにした** —
  これまでは**置き換え**だったので、自分のモジュールを指した瞬間に
  stdlib が消えた。root は指定順に探索し、**後の root が勝つ**ので
  `--core-modules core --core-modules mypkg/src` が
  「stdlib + 自分、自分が勝つ」になる。`poc/logsearch` の
  `refresh.sh` (symlink 農場) を書く理由が消えた。
  併せて **ENTRY-IN-MODULE-ROOT を解消** — auto-load の walker が
  コンパイル対象と同じ正規化パスのファイルを飛ばすので、
  `src/main.t` を普通に書ける (以前は二重取り込みで複製が自分の
  `const` を失い `[E0003]` になった)。
  代償は `RunOptions.core_modules_dir: Option<&Path>` →
  `core_modules_dirs: &[PathBuf]` の波及で 40 ファイル。
  **EFFECTS-CORE-MODULES も解消**: `--effects` はクエリなので main の
  引数解析より前に走り root を落としていた。argv から拾うようにした
- **BUILD-TOOL B1/B3 — `toy` コマンドを追加した** (`toy/`) —
  `build` / `run` / `check` / `api` / `effects` / `explain`。
  パッケージは「`main.t` か `src/` を持つ最寄りの祖先」で、
  **マニフェストは無い** (宣言することがまだ無い)。root は
  stdlib → `src/` → `--core-modules` の順に積む。リンクキャッシュは
  `build/.link/` を既定にした (90ms → 30ms が既定になる)。
  `-v` は等価な `compiler` / `interpreter` 呼び出しを 1 行で出す —
  道具は引数を組み立てるだけで意味論を持たない、という非目標の担保。
  **`--backend tree` は名前だけ取ってあり、今は `vm` と同じ経路**
  (tree-walker を名指しする library API が無い)
- **`Bits` / `Checked` も借用にした** — 受け手は `&self`、`Checked` の
  `other` は `&Self`。`Ord` / `Hash` で直した 4 つに加えて、
  **`&T` 引数を渡していない経路があと 2 つあった**:
  compound を返す method の引数ループ (struct/enum レシーバ) と、
  primitive レシーバ + compound 戻りの経路 (`let_lowering`)。
  `Checked` は `Option<Self>` を返すので後者に当たり、
  `250u8.checked_add(10u8)` が `other` を 0 と読んで `Some(250)` を
  返していた。auto-borrow を持つ経路が**全部で 4 つ**あることになる。
  併せて **IR VM が address-taken local の裏当てセルを
  確保カウンタに数えていた**のを uncounted にした
  (`VmHost::alloc_internal`)。AOT / JIT は stack slot なので、
  `f(&x)` にするだけで `alloc_count` がレーン間で食い違っていた
  (`Vec::grow_to` の `checked_mul` で実際に踏んだ)。
  ambient allocator ではなく global heap から取るので、
  `with allocator = arena` の中で `&x` しても `arena.bytes_used()` は動かない
- **`Ord` / `Hash` と container の読み取りメソッドを借用にした** —
  `trait Ord { fn lt(&self, other: &Self) }` / `trait Hash { fn hash(&self) }`、
  `Dict::get` / `get_or` / `get_or_default` / `contains_key` / `size` と
  `Set::contains` / `size` / `is_empty`。**破壊的変更** —
  `impl Ord for T { fn lt(self: Self, other: Self) }` は
  conformance を満たさなくなる (受け手の種類と引数型は trait の契約)。
  `&self` は `<` 演算子オーバーロードが元から文書化していた形でもある。
  **primitive だけを相手にする trait は `self: Self` のまま**
  (`Bits` / `Checked` / `AsciiClass` / `Abs` / `Sqrt` / `str.t` の
  `Length` / `AsPtr` / `StrSearch`)、`collect(self: Self)` /
  `Into::into` / `JsonWriter::finish` も**意図的な by-value** なので不変。
  これを通すのに処理系側で 4 つ直した:
  - `resolve_self` が `&Self` の内側を解決していなかった (conformance が
    `&Self` の impl を落としていた)
  - `TypeDecl::is_equivalent` に `Ref` の腕が無く、導出 PartialEq に
    落ちていたので `Identifier` と `Struct` で綴られた同じ型が
    「expected &String, found &String」で不一致になっていた
  - `resolve_self_type` が `&Self` を解決せず、body の中で `other` が
    `Self` のままだった
  - **`&Self` 引数が primitive レシーバで渡っていなかった** ★ —
    `populate_method_writeback_types` が `Self` を置換せずに
    `param_ref_pointee` を作るので `None` になり、かつ
    `try_lower_primitive_method_call` に auto-borrow が無かった。
    呼び出し側が**値**を渡し callee が `LoadRef` するので、
    `3u64.lt(5u64)` が番地 5 を読んで**黙って false** を返していた
    (crash しないのが最悪)
- **STRING-NO-DROP — `String` が自分のバッファを解放するようにした** —
  `Vec<T>` は最初から持っていた `impl Drop` を `String` は持たず、
  **プログラムが作った `String` は 1 つ残らず漏れていた**
  (`--profile=mem` の `leaks` に `from_str` / `push` が並ぶ)。
  本体は 4 行。影響範囲を測れという注記どおり測った結果:
  - **`Vec::sort` / `sort_by` が use-after-free になった**。
    `val key: T = self.get(i)` のように **compound を返す method 呼び出し**
    から束縛したローカルには drop glue が付くが、その値は要素の
    **別名**であって所有ではない。ソートが自分の要素を解放し、次の読みが
    死んだ確保に当たって IR VM が `value not defined` で落ちる。
    `contains` / `index_of` が元からそうしていたように
    `__builtin_ptr_read::<T>` で直接読む形に直した (これには glue が
    付かない)。**`Vec<T>` の他の method は元から安全**
    (`contains` / `index_of` / `clone` / iterator は temporary か
    ptr_read 経由)
  - JIT の `impl Drop` allowlist に `String` を足す必要があった。
    `File` と同じ穴で、忘れると**文字列に触る全プログラムが
    tree-walker に落ちる**
  - `parse.t` / `time.t` のヘルパが `String` を値で取っていたのを
    `&String` に変えた (値渡しは move になるので、同じ束縛を 2 回
    渡せなくなる)
  - **残る `Ord::lt` の形**: `fn lt(self: Self, other: Self)` は
    値渡しだが alias なので move にならない。`&self` にすべきかは
    RUNTIME_LIBRARY「関数粒度の空白」B の `Ordering` 論点と一緒に決める
- **IR VM の `value not defined` が場所を言うようになった** —
  関数名・ブロック・命令位置を出す。上の use-after-free を切り分けるのに
  丸ごと 1 往復かかったので。`PtrRead` が死んだ確保に当たると
  値を作らないため、この panic は**実際には解放済みメモリの読み**を
  意味することが多い
- **STDLIB-FS-HANDLE — `fs::File` (開いたファイル) を入れた** —
  `fs.t` は全部パス指定の全体操作で、ハンドルを持つ型が 1 つも
  無かった。`poc/logsearch` が「残る前提のうち最大 (R2)」と書いた項目。
  `File::open` / `create` / `append` / `open_rw` / `read` / `write` /
  `read_at` / `write_at` / `seek_to` / `seek_by` / `seek_end` / `tell` /
  `size` / `sync` / `truncate` / `close` / `as_fd` / `is_open`。
  buffer は `Span<u8>` (EXTERN-BUF、確保もコピーも無し)、失敗は既存の
  `IoError`、`Drop` が fd を閉じる。設計は
  [`STDLIB_FS_PATH.md`](STDLIB_FS_PATH.md) §10、例は
  `interpreter/example/fs_file.t`。R5 (`fsync`) も同じハンドルで解消。
- **`Result<(), E>` を `val` に束縛すると lowering が panic した** —
  `val r: Result<(), IoError> = f()` が
  `compound_storage.rs` の `payload slot shape mismatch` で落ちていた。
  `()` payload の `(Unit, Unit)` に copy の arm が無かっただけで、
  `()` は写す leaf を持たないので空実装。上の `File` を書いていて踏んだ。
- **METHOD-ARG-UNCHECKED — method 呼び出しの引数を型検査するようにした**
  — 個数も型も見ていなかったので、`w.two(1u64)` (2 引数の宣言) や
  `h.fill(out)` (`&mut Sink` を要求する位置に値) が通り、tree-walker は
  値を出し、compiled レーンは cranelift の verifier が SSA 値の名前で
  落ちていた。とくに `&mut` は**黙って値渡し (コピー) に化ける**ので、
  callee の書き込みがどこにも残らない (`poc/logsearch` が壊れた
  アーカイブを「成功」と報告した原因)。自由関数と同じ
  `[E0001] ... (in argument N of method 'f')` を出す。宣言が読める
  場合だけ検査する — レシーバが struct / enum で、その引数位置の
  宣言型が型パラメータを含まないとき (generic の束縛は後段の仕事)。
  associated function (`P::make(...)`) は元から検査されていた。
- **METHOD-MUT-PARAM-REBORROW — method が自分の `&mut` パラメータを
  再借用できるようになった** — `setup_method_parameter_context` が
  全パラメータを `set_var` で登録していたので、method の body の
  `&mut out` だけが `cannot borrow \`out\` as mutable: binding is not
  declared \`var\`` で蹴られていた (自由関数側の `visitor.rs` には
  `&mut T` の分岐がある)。bare 渡しは METHOD-ARG-UNCHECKED で
  黙って値渡しになっていたため (同日 METHOD-ARG-UNCHECKED で解消)、
  **`&mut` パラメータを method から転送する綴りが 1 つも無い**状態
  だった (`poc/logsearch` が踏んだ「書き込みが消える」の片割れ)。`&mut self` レシーバも同じ理由で
  可変にしたので `bump(&mut self.count)` が書ける (compound
  フィールドの借用は COMPOUND-FIELD-ARG のまま AOT が拒否する —
  自由関数からの `&mut w.s` と同じ)。receiver そのものへの代入
  (`self = ...`) は、これまで同じ不変性が兼ねていた拒否を専用の
  規則に移した (以前は tree-walker の実行時エラーに落ちていた)。
- **MEMORY-ACCESS M3 — `Span<T>` の範囲演算** — `__builtin_mem_eq` /
  `mem_find` / `mem_find_seq` (実装は `toylang_rt` 1 か所、libc では
  ない) と、それを包む `copy_from` / `move_from` / `bytes_eq` /
  `find` / `find_seq` / `fill`。`String::eq` の「SIMD ループ + 端数
  ループ」と `String::find_from` の二重ループが**それぞれ 1 呼び出し**
  になった。
- **MEMORY-ACCESS M2 — stdlib の読み出し 130 箇所を `::<T>` 形へ移行**
  — `core/**.t` に旧形は 0 箇所。`Vec::elem_size` は撤去できず
  (未実装節の TREE-WALKER-GENERIC-SCOPE)。
- **MEMORY-ACCESS M0 — `mem_move` / `mem_set` が全バックエンドで動く
  ようになった** — compiled レーンに lowering が無く
  (`cannot lower builtin yet: MemMove`)、`mem_set` の fill value は
  doc が `u8`・型検査表が `u64`・tree-walker が `u64` で三者バラバラ
  だった。`u8` に統一し、**3 つの `mem_*` は引数を型検査するように
  した** (`visit_builtin_call` は署名表から戻り型を返すだけで引数を
  訪問しておらず、`arg_types` は飾りだった)。診断は
  [`MEMORY_ACCESS.md`](MEMORY_ACCESS.md)。
- **MEMORY-ACCESS M1 — `__builtin_ptr_read::<T>(p, off)`** — 読み出し
  幅を呼び出しに書けるようにした。旧形 (注釈から取る形) は残す
  (stdlib の 213 箇所を移行する M2 まで警告は出さない)。これに伴い
  IR VM の scalar 読みが typed-slot map より byte を優先するようになり、
  同じ IR を走らせる AOT / JIT との不一致が消えた。
- **IRVM-BOOL-STRIDE — IR VM が bool 配列を 8 バイト間隔で書いていた**
  — 配列アドレス計算が stride を型から引き直す表を持っており、その表
  だけ `bool` を 8 バイトとしていた (lowering は 1 バイトで確保)。
  `soa [T; N]` の bool 列が隣の列を踏む。typed-slot map が値を返して
  いたので見えていなかった。frame が確保時の stride を持つようにして
  表を削除。

### 2026-09-04
- **JSON-RESULT-READER — json の reader が `Result` を返すようになった
  (`json::parse` が設計どおりの入口になった)** — WIDE-RETURN が
  landing したので、`doc.read(s) -> Option<u64>` + `doc.error()` と
  いう迂回 (`Json` が失敗をフィールドに溜める形) をやめ、reader の
  全段を `Result<u64, JsonError>` にして `?` で伝播させた。設計の
  `pub fn parse(s: str) -> Result<Json, JsonError>` も入った
  (`Ok` は `Vec` を積んだ木、`Err` は payload つき variant —
  両方 wide なので以前は署名ごと拒否されていた)。副産物として
  `Json` から 2 フィールド、`fail` / `error()` / 4 つの err コードが
  消え、`read_array` / `read_object` / `read_text` の `failed` フラグ
  も消えた。これを書く途中で下の 2 つが出た。
- **MODULE-TRY-REMAP — stdlib の body に `?` を書くと integration が
  落ちていた** — `module_integration.rs` の remap に `Expr::Try` の
  腕が無く、`Unsupported expression type for remapping: Try { ... }`。
  `NullCoalesce` (`??`) の腕はあったので `?` だけが穴で、**stdlib は
  今まで `?` を 1 つも使っていなかった**ため誰も踏んでいなかった。
- **TRY-STDLIB-ALIAS — user が `Result` / `Option` を影にすると
  stdlib の `?` / `??` が壊れる** — 影があると stdlib 側の enum は
  `__std_Result` に再 intern される (DICT-CROSS-MODULE-OPTION) のに、
  `?` / `??` の desugar は**書かれた綴りで分類**していたので
  ``[E0010] `?` requires Result or Option, got enum `__std_Result` ``。
  影のあるプログラムでだけ落ちるので、stdlib 全体が `?` に依存できな
  かった。分類だけ prefix を剥がす (patterns は別名のままで正しい)。
- **STDLIB-CRYPTO C0/C1 — SHA-256 / SHA-224 (`core/std/crypto/`)** —
  設計は [`STDLIB_CRYPTO.md`](STDLIB_CRYPTO.md)。`digest.t` が
  `trait Digest` (streaming) + `struct Sum` (出力値) + `ct_eq`
  (定数時間**を意図した**比較。バリアが無いので保証はしないと明記)、
  `sha256.t` が FIPS 180-4 の SHA-256 / SHA-224。純 toylang
  (経路の判断は §4、**64 KB で interpreter 6.2s / AOT 7ms**)。
  出力は裸の `Vec<u8>` ではなく `Sum` — 入力も出力もバイト列なので、
  型が無いと二重ハッシュを検査器が見逃す。`to_hex` は `hex::encode`
  に委譲 (16 進の綴りを 2 つ持たない)。契約は shift 量 / 出力長 /
  ブロック長 / 添字に置いた (trait 側に置けなかった理由は
  TRAIT-CONTRACT-EXPRREF。2026-09-21 に解消)。3 レーン一致 +
  公開ベクタで pin。
  例: `interpreter/example/crypto_sha256.t`。C2〜C4 は未着手。
- **WIDE-RETURN — 戻り値の leaf が返却レジスタを超える compound を
  compiled lane が返せるようになった** — struct / tuple / enum の戻りは
  leaf ごとに 1 つの cranelift 戻りスロットへ展開されるので、返却
  レジスタ (aarch64 で 8、x86-64 はさらに少ない) を超えると cranelift が
  `Too many return values to fit in registers` で署名ごと拒否していた。
  3 つの cranelift 設定に `enable_multi_ret_implicit_sret` を足し、
  溢れた分を cranelift 自身が導入する return-area ポインタ経由で渡す
  ようにした (lowering 側は無変更 — 呼び出し形ごとの sret 実装が要らない)。
  AOT / compiler JIT / interpreter JIT の 3 つに効く。これで
  **RESULT-COMPOUND-WRITEBACK も解けた** — `&mut self` の method が
  `Result<u64, E>` を返せる (writeback の leaf と戻りの leaf が同じ予算を
  食っていた) し、`Result<Struct, E>` を返す自由関数も書ける
  (`json::parse(s) -> Result<Json, JsonError>` を阻んでいた壁)。
- **VEC-CONTRACTS #1〜#3 — `Vec<T>` の境界を `requires` にした** — 設計と
  選択シートは [`VEC_CONTRACTS.md`](VEC_CONTRACTS.md)。`get` / `set` /
  `pop` / `insert` / `remove` / `swap_remove` / `set_size` / `grow_to` に
  `requires` を足し、`push_char` の `assert` 2 本を `requires` に置換
  (契約 11 行、body のロジックは無変更)。**panic は残す (A2)** — 契約は
  `--release` で消えるので、消すと `Vec` だけが release で unchecked な
  indexed read になる。checked ビルドでは契約が先に発火して**破った値**を
  出す (`(with index = 5)`)。コストは 4 億呼び出しで 0.74s vs 0.65s
  (~0.22ns/呼び出し、1.14x)。`ensures` 系 (#4〜#7) と `never_allocates`
  系 (#8〜#11) は未着手。

### 2026-09-03
- **STDLIB-NUMERIC 完了 (N0〜N6) — 残っていた `shuffle` / `checked_pow` /
  `clamp_f32`** — 設計は [`STDLIB_NUMERIC.md`](STDLIB_NUMERIC.md)。
  N0〜N5 と N6 の大半は先に landing していたが、宣言だけあって書かれて
  いなかった 3 つを入れた。`random::shuffle<T>(&mut Vec<T>)` は
  **stdlib で最初の generic 自由関数**で、そのせいで「bound の無い
  `<T>`」と「module 修飾の呼び出し」の組でしか出ない穴を 3 層ぶん
  掘り出した (下記 3 項)。`checked_pow` は `Checked` の 8 幅すべてに
  square-and-multiply で足した — 指数の**値**ではなく 32 bit で
  上限が決まるので `1i64.checked_pow(4000000000u32)` が即答する。
  `saturating_pow` は置かない (符号付きの飽和先が基数の符号で変わる)。
- **UNBOUNDED-GENERIC-PARAM — bound の無い `<T>` が「宣言されていない」
  扱いだった** — 型検査器は「この関数が宣言した型パラメータ」を
  `current_fn_generic_bounds` (bound を持つものだけの map) で判定して
  いたので、`fn shuffle<T>(v: &mut Vec<T>)` の body 中の `v.get(i)` が
  返す `T` が `[E0010] ... which is not bound here` になっていた。
  `current_fn_generic_params` を context に足して両方を見る。
- **QUALIFIED-GENERIC-CALL-SCOPE — module 修飾の generic 呼び出しが
  呼び出し側の束縛を消していた** — `visit_generic_call` は全出口で
  スコープを 1 つ pop する契約で、bare 呼び出し側は push していたが
  `dispatch_module_function_call_with_qualifier` は push していなかった。
  `random::shuffle(&mut v)` の**次の行から `v` が消え**、
  `[E0003] Identifier 'v' not found` になる。呼び出しは正しいので
  診断は現場を指さない。
- **QUALIFIED-GENERIC-CALL-LOWER — 同じ呼び出しが lowering でも
  落ちていた** — module 修飾の callee を関数索引から**名前で**引いて
  いたが、generic テンプレートはそこに居ない (呼び出しごとに実体化)。
  `resolve_call_target` を引数スライス版に分けて修飾側からも通し、
  併せて引数を `lower_call_arg_items` (= `&T` にアドレスを渡す経路) に
  乗せ、`&mut` compound の writeback も配線した。これ以前は module の
  関数に `&mut` の struct を渡すと `call argument produced no value` で
  落ちていた。
- **TYPECHECK-BODY-KEY — 同名の module 関数の body が丸ごと未検査だった**
  — `type_check_body` の「検査済み」メモが**関数名だけをキー**にしていた
  ため、`base64::encode` を検査したあと `hex::encode` は body を walk
  せずに返っていた。型検査器は body を**書き換える** (`?` の desugar /
  `Display` の `to_str` / CHAR-LITERAL-NUM の narrowing / SIMD の型
  焼き込み) ので、2 つ目の関数ではそれが全部黙って効かず、生の AST が
  バックエンドに流れていた。body の `StmtRef` をキーにする
  (`FunctionCheckingState::checked_bodies`)。2026-08-30 に free function
  を検査対象に入れた修正の**取りこぼし**で、症状は
  `hex::encode` の `__simd_load` が「型が焼き込まれていない」で落ちること。
- **CODEC-SIMD — hex / base64 の 4 カーネルを SIMD 化** — encode は
  **33x / 18x**、decode は **3.7x / 3.0x** (100 MiB、AOT)。
  `/usr/bin/base64` 比で encode 1.5x / decode 2.8x 速く、出力は
  `base64` / `xxd -p` とバイト一致。出力バッファ用に
  **`String::with_capacity` / `set_size`** を追加 (`Vec` の同名 API の
  String 版)。設計と実測は [`SIMD.md`](SIMD.md) の「codec の SIMD 化」。
- **SIMD-INTRINSIC-4 — `__simd_bitmask` / `__simd_swizzle` /
  `__simd_bitcast` / `__simd_shuffle` (intrinsic 13 → 17、**未実装の
  intrinsic は無くなった**)** — 「どの lane か」を聞く手段が
  抜けていたので `__simd_any` で当たった後は 1 バイトずつ舐め直していた。
  `String::contains` / `Split` を bitmask 版に置換して**密ケース 2.0x**
  (1.26s → 0.62s)、疎ケースは変化なし。`__simd_shuffle` の定数マスクは
  **パーサで**畳む (型検査器の rewrite 前置きは裸の式文 / binary operand /
  `if` 条件に届かず、ドライバも 3 つある)。設計と実測は
  [`SIMD.md`](SIMD.md) の「Phase 3 の追補」。
- **STDLIB-SERIALIZE S1/S3/S4/S5 — JSON (`core/std/json.t`)** —
  設計は [`STDLIB_SERIALIZE.md`](STDLIB_SERIALIZE.md)。`JsonWriter`
  (木を作らない writer) / 平坦な `Json` の木 / RFC 8259 の部分集合の
  reader。**設計から 2 点ずらした**: (1) 木は `enum Json` ではなく
  `Vec<JsonNode>` の pre-order 平坦表現 — enum 版は tree-walker では
  動くが compiled lane では関数に渡せない (`cannot lower parameter`)、
  (2) 深さ上限は 128 ではなく 32 —
  ホストの stack が 40〜60 で尽きるので、それ以上は発火しない上限。
  (もう 1 点あった error channel のずれは JSON-RESULT-READER で解消。)
- **STDLIB-SERIALIZE S0/S2 — hex / base64 (`core/std/hex.t` /
  `base64.t` / `codec.t`)** — RFC 4648 の test vector で pin。hex は
  出力小文字・入力両対応、base64 は標準アルファベット + padding 必須
  + 末尾の未使用ビットが 0 であること。失敗は `CodecError` 1 つ。
- **STDLIB-LOG — レベル付きログ (`core/std/log.t`)** — 設計は
  [`STDLIB_LOG.md`](STDLIB_LOG.md)。stderr 固定・純 toylang。レベルは
  runtime に 1 つ (初期値 `TOY_LOG`、不正値は警告して `info`)、
  熱いループ用に `enabled(level)`、`TOY_LOG_TIME=1` で
  `DateTime::to_str` の ISO 8601 を前置。`log(level, msg)` は
  `log::log` が読みにくいので `at` にした (衝突ではない)。
- **STDLIB-FS-PATH — path 操作とファイルシステム (`core/std/path.t` /
  `fs.t`)** — 設計は [`STDLIB_FS_PATH.md`](STDLIB_FS_PATH.md)。
  `path.t` は syscall を呼ばない。`mtime` / `mode` は `struct stat` の
  layout がプラットフォームで違うので置かない (誤った offset は
  黙って別の数を返す)。`remove_dir_all` も置かない。`IoError` に
  3 variant 追加 (破壊的)。FREE-FN-VS-ASSOC-COLLISION もここで解消。
- **STDLIB-TIME — 単調時計・sleep・CPU 時間・日付 (`core/std/time.t`)** —
  設計は [`STDLIB_TIME.md`](STDLIB_TIME.md)。`Stopwatch` /
  `DateTime` (`{ secs, nanos }` の 2 field — 7 field はレジスタ上限を
  超えた) / ISO 8601 の往復。`bench` は「関数を値として渡せない」ため
  落とし、HOF-RETURN-UNKNOWN として記録。
- **GENERIC-SCALAR-REF 解消 — `&T` が primitive でも通る** —
  パラメータの束縛が**置換前の** `&T` を見ていて
  `lower_scalar(Generic(T))` が None になり、compound 経路に落ちていた。
  一方でシグネチャと `param_ref_pointee` は既に「ポインタ」で合意して
  いたので、呼び出し側が番地を置いた所を callee が値として読み、
  3 レーンが 3 通りの誤答を返していた (2026-09-03 にいったん明示的な拒否に
  していたもの)。`lower_scalar_with_subst` にするだけ。
  併せて `val` の注釈から取る幅も**アクティブな置換込み**で lower する
  ようにした (body 内の `val c: T = ...` は空の置換では解決できない)。
  これで `interpreter/example/jit_panic_expr.t` が AOT 非対応リストから
  外れた。
- **RETURN-LOCAL-DROP (tree-walker) 解消 + `Vec` / `Box` の `Clone`** —
  `fn build() -> Vec<u64> { var out = Vec::new(); out.push(1u64); out }`
  が、**tree-walker では解放済みのバッファを返していた**。ブロック終端の
  drop glue が、そのブロック自身が渡す値まで解放していたため。次の
  `push` が `Invalid memory access in ptr_write` になり、原因の return を
  指すものは何も出なかった。所有型の `Clone` impl はすべてこの形なので、
  そこで露見した。**`--all-backends` の interpreter レーンは IR VM なので
  見えない** — tree-walker をオラクルにするのは consistency harness だけ
  (CLAUDE.md の取り違え注意そのもの)。
  これで `impl<T: Clone> Clone for Vec<T>` / `Box<T>` が入った。
  **VEC-CLONE-WITH-STRING-CLONE は誤診だった**: 実体は `String::clone` が
  `to_string()` を末尾位置で返していたことで、別のエラーを追う過程で
  既に直っており、コンテナの impl を再テストせずに落としていた。
- **PTR-READ-ASSIGN 解消 + `str` の比較演算子 (STDLIB-ORD)** —
  (1) `b = __builtin_ptr_read(p, i)` が書けるようになった。読み出し幅は
  `val` の注釈から取るので**代入には置き場所が無かった**が、書き込み先の
  束縛が既に幅を持っている。型検査器は黙って `u64` に倒しており、
  不一致は recovery が当てた別の文に、別の型を名指しして報告されていた
  (frontend は代入先の型をヒントとして渡し、lowering は束縛の IR 型で
  `PtrRead` を出す)。
  (2) `a < b` / `<=` / `>` / `>=` が `str` で動く。`impl Ord for str` は
  T0 で入っていたが、演算子オーバーロードは**struct レシーバ**にしか
  効かないので `a.lt(b)` と書くしかなかった。比較を `Ord` の呼び出しに
  書き換える (`Ord` は `lt` しか宣言しないので `a > b` は `b.lt(a)`、
  等号込みはその否定)。**post-pass** — 被演算子・条件・末尾式は検査器への
  到達経路が別々で、ノード自身の `ExprRef` を持つ経路と持たない経路が
  あるため。そのために `visit_binary` で両辺 `str` のときだけ型を記録する
  (被演算子は `accept_expr` 経由で型を記録しない)
- **STDLIB-TRAIT-BASE B5 — 戻り位置からの型引数推論と `Default`。
  これで B0〜B5 すべて landing** — `fn make<T: Default>() -> T {
  T::default() }` は 3 箇所で別々に落ちていたが、原因は 1 つ:
  **どの層も型引数を「引数」からしか読まない**ので、戻り型にしか現れない
  パラメータには材料が無かった。型検査器 (`T::assoc()` の解決 tier を
  method 側から写す) / 単相化 (束縛の注釈を hint として下ろす) /
  tree-walker (per-call generic scope の 3 番目の情報源に注釈を足す) の
  3 箇所に入れた。**注釈は最後の手段** — hint があれば常に戻り型と
  単一化する形にしたら既存テストが 15 本落ちた (call site の type hint は
  期待戻り型とは限らず、数値リテラルの文脈を引数へ運ぶ役目も持つ)。
  引数から解いて、それでも未束縛のときだけ注釈を見る。
  `core/std/default.t` に `Default` (primitive 全幅。`str` は
  **予約語で `str::default()` と綴れない**ので入れない — tree-walker
  だけが到達できる impl は無いより悪い)、利用者は `Vec::resize` と
  `Dict::get_or_default` (どちらも bound は**専用の impl block**に置く
  ので `Vec<T>` / `Dict<K, V>` は影響を受けない)
- **STDLIB-TRAIT-BASE B0 / B2 — stdlib の反復子が trait を名乗るように
  なった** — 16 個の `next` を inherent から `impl Iterator<Item> for X`
  へ**移した** (両方に書くと片方が黙って消えるため「足す」ではない)。
  これで `fn total<I: Iterator<u64>>(it: I)` が書ける — 以前は
  `Vec<u64>` は取れても `v.iter().filter(...)` の結果を受ける先が
  無かった。**呼び出し側は反復子を束縛してから渡す** (単相化は束縛から
  型引数を取る)。診断 3 件も B0 で修正: `trait B: A` が `BraceOpen`
  としか言わなかった件、trait 中の `type Item` が**次の行**を指して
  `BraceClose` と言った件、同じ method を inherent と trait impl の
  両方に書くと**実行時**に落ちた件 (型検査に移した — `impl Iterator<T>`
  を既存の `next` 持ちに足すと必ず踏む形なので)。
  `Iterator<T>` は associated type に移さない (16 impl と全利用箇所を
  書き換えて得るのは `<I: Iterator>` と書けることだけ、かつ明示の型引数は
  1 つの struct が複数名乗る余地を残す)。
  **残るのは B5** (`T::assoc()` + `Default`) — レシーバの値が無いので
  型引数の推論元が戻り位置しかなく、単相化に新しい能力が要る
- **STDLIB-TRAIT-BASE B1 / B3 / B4 — bound を書いた先で何かできるように
  なった** — 設計は [`STDLIB_TRAIT_BASE.md`](STDLIB_TRAIT_BASE.md)。
  `Ord` だけが bound 越しに使えていたのは偶然 (`lt` が `bool` を返し
  `&mut` を取らないため) で、穴は 2 つとも別の層にあった:
  (B1) `Self` 戻りの解決は正しく動いていて、その後の**ガードが関数の型
  パラメータを見ていなかった** (impl のものしか見ない) ため弾かれていた。
  診断の `DEBUG:` も消した。合わせて `val c: T = ...` の
  `Identifier(T)` と戻り型の `Generic(T)` を「同じ型を不一致と言う」形も
  修正。(B4) `unify_types` に**参照の行が無かった** — `&mut T` は
  `Cannot unify` で落ち、`&T` は静かに `Unknown` を返していた。
  さらに lowering 側も `&T` の型引数推論・置換後の lower・
  `param_ref_pointee` の置換後計算・`&mut T` の writeback 配線が
  すべて欠けていた (最後の 1 つは tree-walker だけが変異を報告し
  compiled lane が黙って捨てる誤答だった)。
  (B3) `core/std/clone.t` に `trait Clone`。primitive 全幅 + `str` +
  `String`。**コンテナの impl は入れていない** (→ VEC-CLONE-WITH-STRING-CLONE)。
  `Clone` は所有モデルの一部 — `val b = a` は alias で、container に
  入れると move する。
- **STDLIB-TEXT T3〜T5 — テキストの分野が完了** — `AsciiClass`
  (`core/std/char.t`、`u8` と `u32` の両方に impl。`digit_value(radix)` 込み。
  `parse.t` が手書きしていた `c < '0' || c > '9'` 4 箇所を置換) /
  `String::chars()` (`CharsIter`、不正バイトは U+FFFD で 1 バイト前進 —
  `None` は終端の意味なので失敗を乗せられない) / `find` / `find_from` /
  `rfind` / `starts_with` / `ends_with` / `eq_str` / `replace` / `repeat` /
  `lines` / `split_whitespace` / `String::join`。**`push_str` は `str` を、
  `push_string` は `String` を取る**ように分割し、ERROR_MODEL の F10
  (`s.push_str("literal")` が型検査を通って実行時に壊れる) が閉じた。
  **`from` と `to` は予約語**なので引数名に使えない (extern の library 名と
  `for ... to ...`)。
- **STDLIB-TEXT T0〜T2 + STDLIB-ORD (`str`) — `str` / `String` の境界が
  決着** — 設計は [`STDLIB_TEXT.md`](STDLIB_TEXT.md)。**`str` は所有しないので
  新しい文字列を作る API を持たない**: `substring` / `trim` / `to_upper` /
  `to_lower` / `split` を型検査器の表から外し (元から compiled lane には
  無く tree-walker だけで動いていた)、`String` 側に集約。読むだけの
  `find` / `find_from` / `contains` / `starts_with` / `ends_with` を
  `str` に新設 (`toy_str_find` extern 1 本の上)。**`str` は妥当な UTF-8 を
  不変とする** — `str_from_bytes` が検証して拒否し、tree-walker の
  `from_utf8_lossy` (同じプログラムが 6 と 2 に割れていた原因) が消えた。
  先に訊く口は `String::is_utf8()`。**`impl Ord for str`** (`toy_str_cmp`)
  で `Vec<str>::sort()` が動き、STDLIB-ORD の残項目が解消。
  `CaseConvert` は `to_ascii_upper` / `to_ascii_lower` に改名
  (ASCII しか畳まないことを名前で言う)。`docs/language.md` に役割表と
  Unicode の線引き (codepoint 止まり) を追加。残りは T3 (`AsciiClass`) /
  T4 (`chars()`) / T5 (足りない API)。
- **STDLIB-ERROR-MODEL E0〜E5 — 失敗の運び方が決着** — 設計は
  [`ERROR_MODEL.md`](ERROR_MODEL.md)。(E0) I/O の失敗語彙を
  `toylang_rt::io_status` の 1 箇所に集約し interpreter は forward する。
  未知の errno は `ReadError` ではなく `Unknown`。(E1) 1 つの型に
  `From` impl を複数書けるように (前処理が `from@IoError` に改名し、
  呼び出し側は型検査器が引数型で解決)。(E2) 文の位置の `?` と
  `Result<(), E>` の `?`。(E3) `expect(msg)` が message を出す
  (`panic` の非リテラルは既存の `Terminator::PanicStr` に載せた)。
  (E4) `docs/language.md` に「Error model」節 + `NetError::is_retryable`
  / `is_pending` + `interpreter/example/error_model.t`。(E5) 確保失敗の
  検査と `AllocError` / `try_reserve` / `try_with_capacity`。**署名は
  1 つも変えていない** — `push` は失敗したら panic するまま。
  E5 は前提条件を 2 つ掘り出した: インタプリタのヒープが確保失敗で
  **abort** していた (null を返すようにした。ただし memset が走るので
  1 TiB の上限を明示) のと、**両方の `realloc` が失敗時に元のブロックを
  壊していた** (interpreter は free + typed slot 破棄、両者とも得ていない
  成長を計上) 件。D5 がまさにその保証に依存している。
- **COLLECTIONS C5 — `PriorityQueue<T: Ord>`
  (`core/std/collections/priority_queue.t`)** — `Vec<T>` 上の binary
  min-heap (**最小が先**)。`pop` / `peek` は `Option<T>` なので空は答えで
  あって panic ではない。`Ord` は `lt` しか持たないので **max-heap は
  `lt` を反転した要素型**で取る (第 2 の型も comparator field も持たない)。
  `Vec` を内側に持つので成長・境界検査・`Drop` は借りられ、**自前の
  `impl Drop` を持たない** — JIT の allow-list を増やさずに済む形。
  **これで COLLECTIONS は C0〜C5 すべて landing。**
- **COLLECTIONS C4 — `Deque<T>` (`core/std/collections/deque.t`)** —
  ring buffer。`Vec` の上に載せなかったのは、`Vec` に安い前方削除が無く
  「`pop_front` が O(n) の queue」になるため。成長は倍化 + **巻き付いた
  前半だけを古い末尾の後ろへ動かす** ので `head` は動かさない (ここが
  静かに壊れる経路なので、巻き付いた状態での成長を pin した)。
  `push_front` は `head - 1` ではなく `head + cap - 1` — u64 の減算は
  wrap ではなく trap する。
  **`impl Drop` を持つ stdlib 型を足したら
  `jit/eligibility/analyze.rs` の allow-list にも足すこと** —
  interpreter JIT は「`impl Drop` が 1 つでもあれば全プログラムで諦める」
  検査なので、Deque を足した時点で JIT が言語全体で止まった
  (テスト 42 件が落ちて気づいた。allow-list は飾りではない)。
- **COLLECTIONS C3 — `Vec` の `insert` / `remove` / `swap_remove` /
  `contains` / `index_of` / `reverse` / `sort_by`** — `contains` /
  `index_of` に bound は要らない (C0(a) の性質)。`remove` は順序維持の
  O(n)、`swap_remove` は O(1) で順序を壊す — **名前で言う**
  (`Dict::remove` が黙って swap していた失敗の裏返し)。`sort_by` は
  comparator を取るので `Ord` の無い型も、逆順も書ける。
- **COLLECTIONS C2 — `Set<T>` (`core/std/collections/set.t`)** — `Dict` と
  同じ表 (probe / `hash_mix` / 7/8 成長) を持つ独立 struct。`Dict<T, ()>`
  は compiled レーンが unit 引数を拒否し、`Dict<T, bool>` は `insert` の
  戻り値が「新規かどうか」にならないので採らなかった。反復順は挿入順
  (削除後も)、`insert` は新規なら true、`clear` はバッファを保つ。
  **同じ算術が 2 箇所にある**ので、同じキー列を入れた `Set` と `Dict` の
  反復順が一致することを 3 レーンで pin して縛っている。

### 2026-09-02
- **BUMP-CHUNK-OVERSIZE — 1 MiB を超える確保がチャンクをはみ出していた** —
  `toylang_rt::bump_alloc_raw` は要求が残りに入らないとき**常に
  `BUMP_CHUNK_SIZE` (1 MiB) のチャンクを malloc して**先頭を返していたので、
  要求がチャンクより大きいと呼び出し側がチャンク外に書いた。
  `Vec<u64>` を 400,000 push すると Bus error、それ以下は**黙って**
  malloc の隣を壊す。直しは「大きい要求にはその大きさのチャンクを取る」。
  COLLECTIONS C1 の性能測定 (n=400,000) で発見。
- **COLLECTIONS C1 — `Dict` が hash 表になった** — probe する slot 表
  (power-of-two、u32 index) をエントリの脇に置く形。エントリは今までどおり
  keys/vals の並列配列に挿入順で並ぶので、**反復順が挿入順のまま**になり
  (削除後も。以前は swap-remove が壊していた)、`DictIter` と アダプタは
  **フィールドが 1 つも変わらない** — 8 レジスタ返し予算に余地が無いため
  これが layout の決め手。`K: Hash` は実 bound なので struct キーには
  `impl Hash` が要る (**破壊的変更**、`E0010`)。tombstone は**採らなかった**:
  liveness 列を反復子が読むと予算超過になるので、`remove` が後続を詰めて
  表を作り直す (O(n))。実測 (n 個入れて n 回引く): interpreter n=10,000 が
  **120s 超で終わらず → 4.7s**、AOT n=40,000 が **1.03s → 0.00s**。
  設計と逸脱の記録は [`COLLECTIONS.md`](COLLECTIONS.md)。
- **TREE-WALKER-SIZEOF-STR — `__builtin_sizeof` が `str` 値に答えるようになった**
  — tree-walker だけ `None` を返して internal error にしていた
  (compiled レーンは `Type::Str` に 8 を返す)。`Dict<str, V>` は最初の
  insert でキー幅を聞くので、**この 1 箇所のせいで tree-walker では
  動かなかった**。`object_byte_size` は「`compiler_lower` と一致すること」を
  自分のコメントで要求しているのに、その一致が取れていなかった。
- **COLLECTIONS C0 (a) — generic な `==` の相手に `eq` が無いと型エラー
  (`E0010`)** — `impl<T> Bag<T>` の `e == needle` は bound 無しで通り、
  `T` が `eq` を持つ struct ならそれに dispatch する (この性質は維持)。
  無い型を渡したときだけが穴で、**実行時**に
  `expected Struct(SymbolU32 { value: 60 }, []), found Struct(...同じ...)`
  という「同じ型を不一致と言う」診断で落ちていた。body が「この型引数を
  `==` で比べる」ことを記録し、**全 body と全呼び出しを見終えてから**
  突き合わせる (stdlib の body はユーザ文の後ろに integrate されるので、
  どちらか一方の時点では判断できない — `eq_requirement.rs`)。
  診断は呼び出し位置を指し、enum には「variant を match しろ」と言う。
  `Dict<P, V>` (P に `eq` 無し) もこれで落ちるようになった。
- **COLLECTIONS C0 (b)(c) — hash の土台** — `Hash for str` が 0 定数を
  やめて FNV-1a (`toylang_rt::toy_str_hash` + interpreter の
  `__extern_str_hash`)、`impl Hash for String` が同じ定数で toylang の
  バイト走査、`hash.t` に splitmix64 finalizer の `pub fn hash_mix()`。
  mixer を impl ではなく**表側**に置いたので user の `impl Hash` も
  同じ分散を得る。値は 3 レーンで pin (`consistency/collections.rs`)。
  設計は [`COLLECTIONS.md`](COLLECTIONS.md)。残りは C0 (a) と C1。
- **WINDOW-ESCAPE — 窓がバッファより長生きできなくなった (`[E0026]`)** —
  POINTER.md が P4 で先送りしていた検査。着手条件 (「実プログラムで
  dangling span が問題になる」) は満たされていた: ローカル `Vec` の窓を
  返す関数が**通り、解放済みメモリを読んで正しい答えを返していた**
  (never-reuse ヒープのおかげで当たるだけ)。`ref struct` マーカ
  (POINTER.md 選択肢 2) ではなく **REGIONS の規則を owner 違いで再利用** —
  「終わりが見えるものから派生した値はそれより長生きする場所へ行けない」を
  allocator とローカルバッファの 2 つに適用する (`RegionKind`)。
  マーカ構文が要らないのは、**パラメータを owner にしない**という
  REGIONS と同じ除外で `Vec::as_span(&self)` が自動的に合法になるため。
  `Span` (method call) と `Column` (field access) の両方が taint 源。
  未検査で残るのは closure が捕捉した窓と realloc を跨いだ窓。
  interpreter に 7 件 (stdlib が誤検知しないことの canary 込み)
- **MUST-USE — 捨てられた `Result` を警告するようにした (`[E0025]`)** —
  `?` は失敗が通る道を作ったが、通し忘れに気づく道が無かった。
  例外を持たない言語では**失敗は戻り値で運ばれるか消えるか**なので、
  `io::write_file(...)` を式文に置くだけでディスク一杯が成功として
  報告されていた。判定は**ブロックの末尾以外の式文**に限る — 末尾は
  ブロックの値で、要るかどうかは外側しか知らないので判断が要らない形
  だけを見る (誤検知ゼロ、逃げ道が 1 つで足りる)。逃げ道は `match` /
  `?` / **`val _ignored = ...`** で、**新しい構文は要らない**。
  **`Option` は対象外** (不在が答えである検索が多い)。
  E0018 と同じく警告 (既存コードがこう書かれている)。
  **todo の「警告の emit 経路が無い」は古かった** — 経路は
  `check_typing_diagnostics` に既にあり、E0018 が使っている。
  interpreter に 8 件 (stdlib が 1 件も踏まないことの canary 込み)
- **OP-OVERLOAD-CHAIN — overload の結果が普通の値になった** —
  interpreter は 6 形すべてを通し、compiled レーンは let-rhs 位置しか
  通していなかった (2026-08-29 の実測表)。**原因は 1 つ** — overload の
  結果は struct で、struct は leaf local に住むが、その local を確保する
  位置が `val` の rhs しか無かった。各サイトが別々の語彙で報告していた
  (`arith lhs must be a bare identifier` / `field-access chains rooted at
  a bare identifier` / `binary lhs produced no value`) が、どれも規則を
  名指していなかった。`emit_binary_overload` / `emit_unary_overload` を
  切り出し、(a) 引数位置、(b) field access の root、(c) `==` の operand
  から呼べるようにした。**operand 側も一般化** — 通常の compound 引数
  経路 (束縛 / literal / call / **別の overload**) を通すので chain が
  そのまま動く。docs の「Out of scope」から 5 形すべてが消えた。
  4 レーンに 4 件 pin
- **TREE-WALKER-SELF-TYPE-ARG — 宣言戻り型が `Self` の型引数を名指すようにした** —
  `fn window(&self) -> Option<Win<u64>> { Win::try_from_raw(self.data) }`
  で返ってきた `Win` が型引数を持たず、`__builtin_sizeof::<T>()` を使う
  method が `unbound generic parameter` で落ちていた (compiled 3 レーンは
  通る)。同じことを `val w: Option<Win<u64>> = ...` と書けば動いていた
  ので、**宣言戻り型を pending annotation にする**だけで揃う。2 面:
  (a) 本体の評価中は戻り型を annotation として見せる (tail も `return` も
  対象。注釈つき `val` は自分の注釈で上書きするので漏れない)、
  (b) 関数から出る値に戻り型の型引数を焼く — `Option::Some(Win { .. })`
  のように payload が裸の struct literal だと他に手がかりが無いので、
  `apply_annotation_type_args` を payload まで再帰させた (名前で対応付け、
  値から推論済みの具体型が勝つ)。**これで `Vec` / `String` の `as_span` /
  `capacity_span` が `Span::try_from_raw_parts` の 1 呼び出しになった**。
  4 レーンに 4 件 pin
- **ENUM-ASSOC-FN-PRODUCER — enum を返す associated function を
  enum 生成位置に書けるようにした** —
  `fn open(id) -> Option<Handle>` を `if` の枝 / `match` の arm /
  tail / 別 enum の payload に置くと
  `branch produces enum \`Span\` but the surrounding binding expects
  \`Option\`` (Span は struct で、返っているのは Option — **文言が
  全部間違っていた**)。`A::b(..)` を無条件に variant 構築とみなして
  いたのが原因で、variant でなければ associated function として解決し、
  戻り型がその enum なら `CallEnum` を撃つ形に。実体化は**対象 enum の
  payload が名指す struct instance**から取る (`Option<Span<u8>>` の
  スロットは `Span<u8>` を持っている — `-> Option<Self>` の `Self` を
  決められるのはこれだけ)。4 レーンに 3 件 pin
- **SUBDIR-ASSOC-FN は解消済みだった** — サブディレクトリの
  `core/std/collections/vec.t` から `Span::from_parts` /
  `Ptr::try_from_raw` が呼べないという項目は、2026-09-01 の
  `28d8c53` (impl block の登録を独立 pass にした) が副作用で直していた。
  原因はモジュール階層ではなく**登録が文の順序に従っていた**こと
  (「各 block は自分より上で宣言された block しか呼べない」)。
  `Vec::as_span` / `capacity_span` の回避コメントごと外して
  `Span::from_parts` に戻した
- **NUM-W-FOR-RANGE — narrow int の `for` 範囲が 4 レーンで一致した** —
  `for i in -3i32..2i32` が型検査を通ったうえで 3 通りに割れていた:
  IR VM は 5 回まわし、tree-walker は `For loop range must be UInt64 or
  Int64` で拒否し、AOT / JIT は cranelift の verifier で**クラッシュ**
  (`arg 1 (v18) has type i64, expected i32`)。原因は層ごとに別で、
  (a) lowering の step 定数が `I64` / `U64` の 2 択だったので 32bit の
  カウンタに 64bit の 1 を足していた (`Const::from_usize_in` で誘導変数の
  型に作る)、(b) tree-walker の dispatch が wide 2 種しか知らなかった
  (`Value` のペアで match して全 8 幅)。**`i8` は `From<u8>` を持たない**
  ので `execute_for_loop` の step は引数で渡す形に。8 幅 + `break` /
  `continue` / `to` 形 / ネストを 4 レーンに 4 件 pin
- **COMPOUND-ARG-CALL — compound を返す呼び出しを引数位置に書けるようにした** —
  `take(mk(3i64))` が `cannot use a struct-returning call in expression
  position; bind the result with \`val\``。ENUM-ARG-NEST で enum だけ
  通るようになっていた非対称を解消 (`Option::Some(mk())` は書けるのに
  `take(mk())` は書けない、理由は読み手から見えない)。引数スロットが
  自分の leaf local を確保して `CallStruct` / `CallTuple` / `CallEnum` の
  dest をそこに向ける。**3 つの call 形すべて** — 自由関数 /
  associated function (`take(P::origin())` / `count(Vec::new())` は
  スロットが実体化を決める) / method (`take(o.twin())`)。
  所有権は callee に移るので新しいストレージは drop を登録しない
  (`val` 束縛と free 回数が一致することをテストで pin)。
  **ネストした tuple 戻り (`((i64,i64), bool)`) は別の穴** で、
  lowering に戻り型が無い。4 レーンに 5 件 pin
- **CHAR-LITERAL-GENERIC-ARG — レシーバが決めた型引数を
  パラメータ型に流すようにした** — `Span<u8>::set(i, 'A')` が
  `arg 3 (v48) has type i32, expected i8` で **cranelift の verifier
  クラッシュ** (型検査は通してしまう)。CHAR-LITERAL-NUM の narrowing は
  「整数型を名指しする位置」で効くが、`fn set(&mut self, i: u64, v: T)`
  の `T` は名指しに見えず、hint が捨てられて 32bit のままだった。
  **具体型引数を持つレシーバは既に `T` を決めている**ので、
  `declared_method_param_types` で `substitute_generics` してから
  filter する形に。ついでに `v.push(65)` (サフィックス無し整数) も
  `u64` 既定ではなく要素型に落ちるようになった。**method 自身の
  generic (`fn pick<U>`) は据え置き** — レシーバは `U` を知らない。
  4 レーンに 2 件 + interpreter に 1 件 pin
- **DIAG-SYMBOL-NAME-LOWER — lowering の診断も名前を綴るようにした** —
  frontend は 2026-09-01 に決着していたが、監査は frontend だけだった。
  `compiler_lower` に 83 箇所の `{:?}` があり、`compiler MVP cannot lower
  expression yet: QualifiedIdentifier([SymbolU32 { value: 60 }, ...])` の
  ように**コンパイラの語彙を読み手に見せていた**。3 つの語彙それぞれに
  綴りを用意 (`spelling.rs`: IR `Type` / `TypeDecl` / AST)。**式は形を
  名乗る** — 「cannot lower a range yet」「got the call `mk(...)`」。
  再発は 2 本で止める: `frontend/tests/diagnostic_spelling_tests.rs` の
  source scan を `compiler_lower/src` にも広げ、
  `compiler/tests/lower_diagnostic_spelling.rs` が実際の refusal を読む。
  ついでに `compiler` の `parse error: {e:?}` を `Display` に
  (`ParserError { kind: ... }` が丸ごと出ていた)

### 2026-09-01
- **ENUM-ARG-NEST — enum を返す呼び出しを引数と payload に書けるようにした** —
  `node(leaf(), 1i64, leaf())` が `cannot use an enum-returning call in
  expression position`、`Option::Some(mk(2i64))` が
  `cannot lower ... as an enum-producing expression in this position`
  だった。どちらも「呼び出しの leaf を置く先が `val` しか無い」が理由なので、
  呼び出し先の dest をその場のスロットにして `CallEnum` を撃つ形に。
  method 版 (`Option::Some(it.next())`、writeback 込み) も同時に通した。
  **struct / tuple 戻りの引数位置は据え置き** (todo の COMPOUND-ARG-CALL)。
  例: `interpreter/example/box_binary_tree.t` (二分木が 1 式で組める)。
  4 レーンに 3 件 pin
- **ENUM-VARIANT-ARG — enum の構築を引数位置に書けるようにした** —
  `take(Color::Red)` / `area(Shape::Circle(3i64))` が compiled レーンで
  `cannot lower expression yet` だった。構築に `val` 以外の居場所が
  無かったのが理由で、struct / tuple リテラルの
  CALL-ARG-COMPOUND-LITERAL と同じ手口 (引数位置で `EnumStorage` を
  起こして leaf を渡す) で解消。**generic は callee のパラメータ型から
  実体化する** — `take(Option::None)` は `T` を他に知る手が無い。
  compound を返す method の引数経路にも同じ穴があったので同時に塞いだ。
  `if_val.t` が AOT_UNSUPPORTED から外れた。4 レーンに 5 件 pin
- **代入は `Unit` — ブロックの末尾に置いても値を持たない** — `visit_assign`
  が代入した値の型を返していたので、`match x { Some(v) => { acc = acc + v }
  None => {} }` が「arm 0 is i64, arm 1 is ()」で拒否され、
  `fn f() -> u64 { a = 5u64 }` が 5 を返して通っていた。4 経路のうち
  配列添字だけが既に `Unit` だったので、不整合の解消。仕様に明記。
  **これを直したことで interpreter JIT の shadowing バグが露出**して同時に修正
  (束縛マップが名前キーで巻き戻されず、内側ブロックの `var x` が外側を潰していた)
- **NUM-W-ENUMERATION — レシーバ表の 6 コピーを 1 つにし、隠れていた
  JIT の符号バグを 3 件出した** — 「primitive レシーバ → 対象型名」が
  4 crate に 6 コピーあって食い違っていたのを
  `TypeDecl::PRIMITIVE_IMPL_TARGETS` からの射影に統一。interpreter JIT
  のコピーは 5 件しか知らず narrow 幅と `str` を落としていて、埋めたら
  **その経路の符号判定が 3 箇所とも `== ScalarTy::I64`** だったことが
  露出した (`-1i16 < 0i16` が false、`-100i16 / -1i16` が 0)。
  `MIN / -1` guard の即値も `i64::MIN` 固定だった。3 crate に網羅テスト、
  4 レーンに 2 件 pin
- **DIAG-SYMBOL-NAME — 診断が名前を綴るようにした** — 型検査器が手で
  組み立てる 80 箇所の `format!` が `DefaultSymbol` / `TypeDecl` を
  `{:?}` で出していたのを interner 経由の綴りに統一
  (`SymbolU32 { value: 60 }` → `P`、`UInt64` → `u64`)。
  `spell_with` の interner あり経路も Debug に落ちなくなった
  (`spell_lossy`)。再発は source scan + 出力 corpus の 2 本で止める
- **NET N5 — 名前解決。これで NETWORK_IO.md の N0〜N5 が全部埋まった** —
  `net::resolve(host)` と、名前を受け付ける `TcpStream::connect`
  (**`connect_nonblocking` は数値のみ** — 「即座に返す」と約束している
  関数が DNS を引くわけにいかない)。`NetError::NameNotFound` を追加
  (存在するが届かない `HostUnreachable` とは直し方が違う)。
  **`struct addrinfo` は `ai_addr` と `ai_canonname` の順序が
  Linux と BSD で逆**なので struct 宣言ごと `sys` 側にある —
  間違えると sockaddr のはずの場所に名前のポインタが来て落ちる
  (間違った答えではなくクラッシュ)。テストが pin するのは
  `localhost` が **127/8 に入ること** (特定の番号ではない —
  machine の `/etc/hosts` を pin することになる) と、`.invalid`
  (RFC 2606 が「絶対に解決しない」と決めている唯一の名前) が
  `NameNotFound` になること。4 レーンで 2 件。
- **NET N4 — UDP / アドレス / socket option** — `UdpSocket`
  (`bind` / `send_to` / `recv_from` / `last_peer_addr` / `last_peer_port`)、
  `local_addr` / `peer_addr` / `local_port` / `peer_port`、
  `set_nodelay` / `set_read_timeout` / `set_write_timeout`。
  **アドレスと port は別々に返す** (port は数であって、コロンを探させる
  理由が無い)。`send_to` は 5 引数で **extern の 4 引数上限を超える**ので、
  宛先を直前の `set_dest` で置く 2 段にした (status ペアと同じ理屈で
  atomic)。`SO_RCVTIMEO` の `struct timeval` は `tv_usec` が macOS 32bit /
  Linux 64bit なので set 全体を `sys` に置いた。4 レーンで 3 件 pin。
- **NET N3 — Poller (epoll / kqueue の統一形)** — `core/std/poll.t` の
  `Poller` / `Event`。[`EVENT_POLLING.md`](EVENT_POLLING.md) の決定
  1〜6 をそのまま実装: **1 fd = 1 イベント** (kqueue の read/write を
  runtime がマージするので、macOS で書いたループが Linux で同じ回数
  回る)、**level-triggered が既定**、**`EINTR` は隠さない**、
  **イベント配列は runtime 側に置き index で読む** (extern 境界が
  ポインタを deref できないため)、**kqueue の `EV_ERROR` は
  changelist 投入時に回収して同期的な失敗にする**。4 レーンで 3 件 pin、
  すべて 1 プロセス自己完結。
- **narrow int のビット演算が tree-walker で落ちていた** — `& | ^` の
  Value 版 fast path に `u64` / `i64` の arm しか無く、`flags & 1u32` が
  **「expected UInt32, found UInt32」**で失敗していた (両辺が同じ型なのに
  型エラーを名乗るのは、比較していたのが operand 同士ではなかったから)。
  compiled レーンは通っていたのでレーン不一致。NUM-W-ENUMERATION の
  4 件目で、`poll.t` が narrow 幅でビット演算をした最初の stdlib コード
  だったために出た。
- **NET N2 — TCP server (`TcpListener`)** — `bind` / `local_port` /
  `accept` / `set_blocking` / `as_fd` / `close`。**テストは 1 プロセスで
  自己完結**する (同じプログラムが listener と client を持つ) ので
  thread も外部 peer も要らず完全に決定的。`bind` は port 0 を要求して
  `local_port` で読み戻す形なので番号を書かない。`SO_REUSEADDR` は
  bind の前に立てる (TIME_WAIT で再起動が `AddrInUse` になるのを防ぐ)。
  4 レーン一致で 3 件 pin。
- **COMPOUND-BLOCK-RHS — `match` から compound を取り出せる** —
  `val s = match r { Result::Ok(s) => s, Result::Err(e) => { return .. } }`
  が lower できるようになった。**`Result` を返すコンストラクタの
  使い方そのもの**で、これが通らないので NET N1 は compiled レーンで
  動かなかった。`detect_struct_result` を敗らせていたのは 2 つ:
  arm が束縛した名前は検出時点で `bindings` に無い (arm 束縛は
  lowering 中にしか存在しない) のと、`return` で抜ける arm が
  `panic` と違って発散扱いされていなかったこと。前者は
  **scrutinee の enum が payload の型を知っている**ので復元できる。
  `break` / `continue` は型検査器も発散扱いしないので載せていない。
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

- **UNIT-TYPE-ARG — `Result<(), E>` / `Option<()>` が 4 レーンで動く** —
  「成否だけを返す」API の自然な形が compiled レーンで書けなかった。
  拒んでいたのは 2 つの門番 (`lower_param_or_return_type` の Unit 型引数、
  `is_supported_enum_payload`) だけで、**layout 側は元から足りていた** —
  `flatten_compound_leaf_types` が `Type::Unit` に leaf を 0 個与えるのは
  unit variant と同じ扱い。local を持たない `PayloadSlot::Unit` を足して
  flat な値の並びが関数境界とずれないようにした。`Ok(())` は `Ok(())` と
  印字する (payload 無しの `Ok` と区別が付くように)。`core/std/net.t` は
  `Result<bool, NetError>` の回避をやめて本来の形に戻した。
  副産物: **注釈の型引数が要素ごとに反映されるようになった** —
  `Result::Ok(v)` は `T` しか埋めないので `[T, Unknown]` という
  混在ベクタになり、all-or-nothing の規則が触らずに `Unknown` を
  残していた (tree-walker だけが `Result<u64, Unknown>` と印字していた)。
- **NET N1 — TCP client (tree-walker のみ)** — `TcpStream` /
  `NetError` / `connect` / `read` / `write` / `close` /
  `shutdown_write` / `take_error` / `set_blocking`。runtime は
  `net_*` (Rust 入口) と `toy_net_*` (extern 薄皮) の二段で、
  **tree-walker は toylang_rt を直接呼ぶ**ので errno → `NetError` の
  表が 1 つしかない。バイト列は `Span<u8>` を借用して渡すので
  **全レーンでコピー 0**。`compiler/tests/consistency/net.rs` が
  `std::net` のエコーサーバをスレッドに立てて **4 レーンで** 6 件 pin
  (ポートはソース中のリテラルとして渡す — AOT バイナリに引数を渡す
  口が無いため)。当初 compiled レーンが動かなかった原因は socket と
  無関係の COMPOUND-BLOCK-RHS と UNIT-TYPE-ARG で、同日どちらも解消
  した。設計から変えた点は [`NETWORK_IO.md`](NETWORK_IO.md) の N1 節。
- **EXTERN-BUF の借用が typed slot を見ていなかった** — `push` で
  作った buffer (`String` / `Vec<u8>`) を `extern fn` に渡すと
  **正しい長さのゼロ**が渡っていた (tree-walker のみ、レーン不一致)。
- **impl メソッドの move が解析されていなかった** — `check_moves` が
  `program.function` しか歩かず、impl メソッドの局所は**常に** drop
  されていた。`Result` に包んで返した値は呼び出し側に届く前に
  `Drop` 済みだった。
- **IMPL-BLOCK-VISIBILITY — impl メソッドから stdlib が見えるようになった**
  — impl block の body 検査とメソッド登録が 1 パスで、しかも statement
  順だったため、**メソッドは自分より後ろの block からしか見えなかった**。
  `integrate_modules` は stdlib をユーザの statement の**後ろ**に足すので、
  結果としてユーザの `impl` から `Vec::new()` も `Span<u8>` の
  メソッドも呼べなかった (`Associated function 'new' not found`)。
  **同じコードが自由関数では動く** — 自由関数は後段のパスで検査され、
  その時点では全部登録済みだから。この非対称が発見を遅らせた: stdlib は
  ほぼ impl block だが、各 block は自分より上の block しか必要と
  しなかった。登録を独立したパス (pass 1) に分けた。
  NETWORK_IO N1 の `TcpStream::read(&self, buf: Span<u8>)` で踏んだ。

### 2026-08-31
- **RUNTIME-TRAP-NARROW — `checked_*` / `saturating_*` が全 8 幅で使える**
  — `CheckedU64` / `CheckedI64` の 2 trait を **`Checked` 1 本 (`Self` 上)**
  に統合し、`u8`〜`u64` / `i8`〜`i64` に impl。signed の
  `saturating_mul` は前身が無く新規 (積の符号で寄せる先を選ぶ)。
  **コンパイラ側の変更はゼロ** — 前提の 2 つ (`Option<Self>` を返す
  trait method、narrow レシーバの dispatch) が同日に landing していた。
  8 幅 × 15 ケースを Rust の `checked_*` / `saturating_*` と突き合わせて
  全一致を確認。**プロセス固定費が +3.7ms (44.5 → 48.2ms、debug、
  trivial プログラム 60 回の平均)** — stdlib はプロセス毎に全部読まれる
  ので、この増分は `checked` を一度も呼ばないプログラムも払う
  (TEST-PERF の「core module のロード 27ms/プロセス」参照)。
  無関係なテスト 172 本の A/B で 0.457s → 0.484s (+6%)。
  **stdlib を足すたびに全プログラムが払う**という一般的な性質で、
  横断的な解は INCREMENTAL-COMPILATION 側の「型検査済み core を
  プロセスを跨いで再利用する」にある。
  実測した副産物: **narrow unsigned の減算は trap せず wrap する**
  (未実装節の NARROW-UNSIGNED-SUB)。
- **NET N0 — プラットフォーム切り替えの足場** — `#[cfg_attr(path)] mod sys;`
  1 箇所で epoll / kqueue を選び、未対応 OS は `compile_error!` で落ちる。
  手書きの定数 39 個は **C の probe を `cc` でビルドして突き合わせる**
  (`compiler/tests/net_abi_tests.rs`) ので、転記ミスが実行時ではなく
  テストで出る。`net::backend_name()` が 3 レーンで一致することを pin。
  `compiler/build.rs` の `rerun-if-changed` をディレクトリ監視に変更
  (これを忘れると AOT の staticlib だけ古いまま残る)。
  **`sys_epoll.rs` はこのホストではコンパイルされない** — 未検証。
- **EXTERN-BUF — `extern fn` が toylang のメモリに届く** — registry を
  「値だけ」と「コンテキストも取る」の 2 本に分け、`(ptr, len)` を
  **コピーではなく借用**で渡す (`HeapManager::borrow_bytes{,_mut}`、
  クロージャ渡しで借用が呼び出しを越えないことを型で保証)。最初の利用者は
  `io::read_file_into` / `write_file_bytes` / `append_file_bytes` で、
  **バイナリ安全なファイル IO**でもある (`str` は tree-walker で
  UTF-8 必須なので `read_file` は非 UTF-8 をレーンごとに違う扱いにする)。
  併せて 3 件直した: module 修飾の呼び出しが compound 引数を取れなかった件、
  tree-walker の `ptr_read` に narrow 幅のバイト読み出しが無かった件、
  narrow の `ptr_write` がバイト列を更新していなかった件。
- **CONV-SPAN — 既にあるバッファを `Span<T>` として見られるようになった**
  — `Ptr::try_from_raw` / `Span::try_from_raw_parts` / `Span::slice` /
  `Vec::with_capacity` / `Vec::set_size` / `Vec::as_span` /
  `Vec::capacity_span` / `String::as_span`。生の番地が型に入る所は
  `Option`、範囲外は panic、という 2 つの規則で統一。tree-walker 側の
  generic scope も 2 件直した (phantom 型引数の自己参照と、注釈の型引数を
  呼び出し側のスコープで解決する件)。
- **primitive レシーバの method call が全幅で動く** — narrow int
  (`u8`〜`i32`) は lowering の dispatch 表から、`f32` は 4 表すべてから
  漏れていて、`impl <Trait> for u8` が到達不能だった。式レシーバ
  (`21u8.twice()` / `a.neg().neg()`) も型付けできるようになり、
  `extension_trait_chained.t` が AOT skip リストから外れた。
- **GENERIC-IN-ENUM-PAYLOAD / SELF-IN-TYPE-ARG — `Option<Ptr<T>>` が
  書けるようになった** — 「注釈の一番外側しか見ない」という同じ欠陥が
  型検査・lowering・tree-walker の 3 層にあり、`Option<Self>` は
  型検査で、`Option<Win<T>>` は monomorphize で落ちていた。
  `TypeDecl::substitute_self` / `nested_type_args` を frontend に置いて
  3 層で共有し、enum を返す associated call の束縛と、phantom 型引数
  (`Ptr<T>`) の構築時記録も直した。CONV-SPAN の `Ptr::try_from_raw` が
  landing。
- **DOD Phase 3 — 配列要素としての enum + tag 列** — compiled lane で
  一切動いていなかった enum 配列が動くようになり、`soa` では tag が
  独立した列になる。enum 要素だけは丸ごと書ける (`ss[i] = Shape::Point`)。
- **DOD Phase 1 — 列の窓 `ps.mass`** — 1 列を関数に渡せるようになった。
  言語の `&[T]` ではなく stdlib `Column<T>` (addr + len + stride) で回収し、
  stride を持つので AoS でも通る。stack 配列と `soa Vec<T>` の両対応。
- **DOD Phase 0.5 — 列の tight pack** — `soa [T; N]` の各列が leaf の実幅で
  stride するようになった (`allocate_array_storage` の 1 行)。narrow leaf の
  配列で AoS 96 バイトに対し SoA 42 バイト。

### 2026-08-30
- **DOD Phase 0 — `soa [T; N]` landing** — 前置修飾子で stack 配列の layout を
  選べる。same-type なので付け外して計測でき、`ps[i].f` の単列 shortcut 込み。
- **DOD Phase 2 — `soa Vec<T>` landing** — heap 側は stdlib `SoaVec<T>` への
  parser 砂糖。1 確保を leaf ごとの列に区切り、builtin 2 個が既存の
  `PtrRead` / `PtrWrite` に展開されるので IR / codegen 無変更。
- **STDLIB-FREE-FN-UNCHECKED 解消 — stdlib の free function body
  も型検査する** — `interpreter/src/lib.rs` の `take(user_func_count)`
  を外した。
- **CHAR-LITERAL-NUM — char リテラルが位置の整数型を取る** — `'a'` は
  **32bit (u32) で保持**したまま (`val c = 'a'` は
  u32)、**他の整数型を名指しする位置では値が収まればその型になる** (`val b: u8 = '0'`
  / `s.get(i) == 'h'` / `c - '0'`)。
- **`fn f() -> ()` が書けるようになった** — 型としての `()` を parser が空
  tuple にしていたので、Unit を返す body と突き合わせて必ず `expected (), but got ()`
  で落ちていた (省略形 `fn f()`
  は従来どおり動いていたので気づかれていなかった)。
- **RUNTIME-LIB P0-B — `core/std/parse.t` (`parse::to_u64` / `to_i64` /
  `to_f64` / `to_bool`)** — 文字列 → 数値。
- **RUNTIME-LIB P0-A — io 書き込み系 (`write_file` / `append_file` /
  `eprint` / `eprintln` / `io::exit`)** — 前 3 つは
  [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) の P0 一行目。
- **POINTER P6 — `unsafe fn` の宣言と強制 (`[E0024]`)** —
  生メモリを読み書きする body に宣言を要求する。
- **POINTER P5 — `Ptr<T>` の non-null 不変 + `Option<Ptr<T>>`** —
  `alloc(0)` を 1 バイトに丸める (全バックエンドが `heap_alloc(0)` に null
  を返すので非 null を保証)。
- **POINTER P4 — `core/std/span.t` の `Span<T>`** — `Ptr<T>` +
  長さの境界検査つきの窓 (`from_parts` / `get` / `set` / `s[i]` / `s[i] = v` /
  `len` / `is_empty` / `as_ptr` / `as_raw`)。
- **POINTER P3 — `core/std/ptr.t` の `Ptr<T>`** — 型付きポインタ窓
  (`alloc` / `get` / `set` / `p[i]` / `p[i] = v` / `offset` / `as_raw`)。
- **POINTER P2 — `__getitem__` / `__setitem__` の 2 バグ + compiled レーン
  dispatch** — arity 検査が `&self` 短縮形 (parameter スロットを占有しない)
  を数えておらず `self: Self` 形しか通らなかった件と、generic struct の戻り型
  `T` が置換されず `p[i]` だけ E0001 だった件。
- **POINTER P1 — `__builtin_sizeof::<T>()` 型引数形** — 値が要らないので
  `Ptr<T>::alloc(n)` が書ける (POINTER.md 実測 2 の解消)。
- **SIMD-VM-SLOT — 測って払うと決めた** — IR VM の `RawSlot` 8 → 16
  バイト化の代償を 4 ワークロードで実測: call 中心 +2.0% / ループ +5.3% /
  struct −0.5% / stdlib +2.9%。
- **SIMD Phase 3 (戦略 B) — stdlib kernel を SIMD 化** — `String::eq` /
  `Vec<u8>::eq` / `CaseConvert` / `Contains` / `Split`。
- **AOT の ISA を baseline 固定に** — `make_object_module()` が
  `cranelift_native::builder()` (ビルドマシンの CPU 機能を検出) から
  `isa::lookup(Triple::host())` に。
- **SIMD Phase 2 — 128bit vector を型にし、演算子を lane-wise に効かせた**
  (`SIMD.md`)。
- **SIMD-F32 の残 (一部)** — IR VM の `to_string_value` に `F32`
  の行が無く、`"{x}"` が生ビット (`1069547520`) を出していた。
- **SIMD-F32 — `f32` を primitive 型として追加 (SIMD.md 論点 1 解決)** —
  lexer (`1.5f32` リテラル + `f32` 型キーワード) / `TypeDecl::Float32` /
  `Expr::Float32` / IR `Type::F32` / cranelift `F32` まで直列に接続。
- **NULL-COALESCE — `a ?? b` 演算子** — `Option::Some` / `Result::Ok`
  なら中身、`None` / `Err` なら default。
- **FROM-INTO-ENUM-ERR — `?` の cross-error 変換が enum エラー型でも 3
  バックエンドで動く** — desugar が出す `val e: MyErr = MyErr::from(s)` を
  AOT/JIT が lower できなかった (「unknown enum variant `MyErr::from`」)。
- **RUNTIME-IO — `read_file` / `env_var` / `read_line` が `Result<_, IoError>`
  を返す** —失敗を `IoError` variant (`NotFound` / `PermissionDenied` /
  `IsADirectory` / `ReadError` / `EndOfInput` / `Unknown`) で返す。
- **TRY-ERR-RETYPE — `?` が success 型の変更を跨げる + `return` の型検査**
  — desugar の error arm が「scrutinee をそのまま return」だったため、内側
  `Result<T1, E>` を外側 `-> Result<T2, E>` で受ける `?` は checker
  を通ったまま compiled レーンだけ落ちる TYPECHECK-LIES だった。

### 2026-08-29
- **ENUM-EQ-ESCAPES-TYPECHECK** — 実際には enum 固有ではなかった。
- **METHOD-ARG-AUTOBORROW** — frontend が認める `T` → `&T` の auto-borrow
  を lowering が実体化していなかったので、値がポインタのスロットに入り、**IR
  VM は誤答・compiled は SIGSEGV** していた。
- **注釈の有無で operator overload の到達可否が変わっていたのを修正** —
  `val h: P = f + P { .. }` が `[E0001] Type mismatch: expected P, but got P`
  で落ちていた (`+=` の desugar 経由でも同じ)。
- **`&Self` / `&mut Self` を compiled レーンが lower できるようにした** —
  `docs/language.md` が overload の推奨形として書いている `fn op(&self, other: &Self) -> Self`
  を AOT / JIT が `cannot lower method parameter other: Ref { inner: Self_ }`
  で拒否していた。
- **COMPOUND-ASSIGN-BITWISE** — ビット系の複合代入 5 種 (`&=` / `|=` / `^=`
  / `<<=` / `>>=`) を足した。
- **戻り型を書かない `main` の panic を修正** — `fn main() { .. }` が 4
  実行系すべてで `unwrap` on `None` で落ちていた。
- **DBC-LISKOV (E0023)** — trait method の `impl` が自分の `requires`
  を足すのを型検査で拒否するようにした。
- **CONTRACT-ELISION 拡張 (制御フロー)** — RUNTIME-TRAP guard の消去が
  `requires` だけでなく **`if` の条件と `for`
  の範囲**からも事実を取るようになった。
- **REGION Phase 1 (E0022)** — スコープ付き allocator (`with allocator = arena { ... }`)
  から確保したメモリが arena より長生きする形を型検査で拒否する。
- **EFFECT-SYSTEM** — 到達可能性で判定する 3 つの検査 (`never_allocates` /
  `const fn` / 契約の純粋性) が各自持っていた「禁止 builtin」テーブルを 1
  つのエフェクト格子に統合し、各検査をマスク 1 行にした。

### 2026-08-28
- **CLOSURE-CAPTURE E0〜E3 / E5 (診断) / E6** — closure
  が捕捉した束縛をどう掴むかを決めて 5 実行系で揃えた。

### 2026-08-27
- **DEBUG-OBS D3 の残: `HeapAlloc` の `SiteId` 移行** — `--profile=mem`
  のリーク報告が `core/std/string.t:71:25`
  のようにファイル名を出すようになった (HeapAlloc / HeapRealloc の null
  リサイズ = stdlib コレクションの主要経路を呼び出し位置に帰属)。
- **DEBUG-OBS: tree-walker への replay を落とした** — IR VM が diverge
  したとき**プログラム全体を走らせ直していた**のをやめた (実測 2)。
- **DEBUG-OBS: 値を持つ文言を全実行系に** — D0 の目標表が決めていた 3 件
  (`u64` underflow の `1 - 5` / 配列 OOB の `index 5, length 3` / 契約違反の
  `(with n = 0)`) が全実行系で同じになり、**D0 の pin 4 件すべてが
  `assert_diagnostic_consistent`** になった (両方向 pin
  が「一致したので置き換えよ」と落ちて教えてくれた)。
- **DEBUG-OBS D6: 再帰深度と stdlib の境界** — 無限再帰が `panic: recursion limit exceeded (N frames deep)`
  + 折り畳んだ backtrace を出すようになった。
- **DEBUG-OBS D5: ユーザ API と機械可読出力** — `__builtin_function_name()`
  (パーサ置換、実行時コスト 0) と `__builtin_backtrace() -> str`
  (各エンジンが自前のスタックを読み、1 つの共有フォーマッタで描く)。
- **DEBUG-OBS D4: shadow stack** — AOT / compiler JIT / interpreter JIT が
  backtrace を出すようになり、**D0 のレーンが backtrace まで一致**した
  (`panic_three_calls_deep` / `panic_inside_the_stdlib` は 5
  レーン完全一致で、 pin が両方向検査で「一致したので
  `assert_diagnostic_consistent` に置き換えよ」と落ちた)。
- **DEBUG-OBS D3: IR の `SiteId`** — 4 実行系すべてが panic
  の位置を言うようになった (D0 のレーンが**位置まで**一致)。
- **DEBUG-OBS D2: `FileId` と `SourceMap`** —
  位置が「どのファイルか」を持つようになった (`SourceLocation.file`)。
- **DEBUG-OBS D1: interpreter の backtrace の穴埋め** — (a) `call_expr`
  が引数リストに `None` を積んでいたせいで `(called at line N)` が
  **到達しない死にコード**だったのを直し、(b) frame を `call_method` /
  `call_associated_method` という**user code に入る絞り**に置いて method /
  associated / closure / `dyn` / operator overload / drop glue をまとめて載せ
  (frame 名は receiver の実行時型で `S::boom` と修飾)、 (c) `main` を積み、(d)
  同一 (関数, 行) の連続フレームを `f (x7, called at line 3)` に畳み、(e)
  折り畳み後 10+5 行の上限と `... N frames elided` を入れた。
- **DEBUG-OBS D0: 診断の比較レーン** —
  実行時の失敗の**文言**を突き合わせるレーンを
  `compiler/tests/consistency/diagnostics.rs` に作り、目標文言を
  `DEBUG_OBSERVABILITY.md` に固定した。

### 2026-08-26
- **COMPILE-TIME-EVAL C5: 配列長に `const` / `const fn` / 式** — `const N: u64 = 3u64`
  → `val a: [i64; N]` に加えて、**計算が必要な長さ** (`[i64; double(2u64)]`、`[i64; N + 1u64]`)
  が動くようにした。
- **COMPILE-TIME-EVAL C4: 契約との接続 + warning 機構** — この言語に
  warning という出力が無かったので作った (`check_typing_diagnostics` の `Ok`
  が warnings を運ぶ)。
- **COMPILE-TIME-EVAL C2: IR の定数畳み込み** — `emit()` 直前に block
  ローカルで畳む。
- **COMPILE-TIME-EVAL C6: 評価器の一本化** — CTFE を tree-walker から **IR
  VM** に載せ替えた。
- **COMPILE-TIME-EVAL C0/C1/C3: `const fn`** —
  コンパイル時に走らせられる関数。
- **COMPOUND-BLOCK-RHS: struct / tuple を産む composite を `val` の右辺に**
  — `val p = if c { P { .. } } else { P { .. } }` / `match` / block が 3
  バックエンドで動く。

### 2026-08-25
- **MATCH-STRUCT-ARM: composite tail の struct / tuple 戻り値がゼロになる**
  — `fn f(n) -> P { if .. { P { .. } } else { P { .. } } }`
  が、走った枝と無関係にゼロ埋めの struct を返していた (match でも同じ)。
- **STRUCT-UPDATE: `P { x: 5i64, ..base }`** — 省略フィールドを base
  から埋める。
- **リファクタリング一巡** — 巨大関数 6 本を機能別に分割 (`lower_program` /
  `lower_builtin_call` / `lower_instruction` / `evaluate_builtin_call` /
  `gen_expr` + `check_expr` / `parse_program`)、 JIT eligibility に `Checker`
  導入 (18 関数 × 9〜11 引数 → 1〜4)、 interpreter JIT のランタイム 35
  シンボルを `toylang_rt` に統合、 `consistency.rs` 11k 行を機能別 14
  モジュールに分割、デッドコード (`ErrorHandling` trait 291 行) 削除。

### 2026-08-24
- **NUMBER-HINT: 既定は `u64` で確定 + 位置の網羅** — 型を名指しする位置を
  17 箇所に拡大 (代入 / 各種引数 / closure 引数・本体 / enum payload /
  struct・generic struct フィールド / 配列・tuple・dict 要素 / 兄弟要素 /
  `if`・`match` の分岐末尾)。
- **NUMBER-HINT: suffix なし整数リテラルの位置ベース解決** — 型注釈 /
  呼び出し引数 / 戻り値 (tail・`return`) が未解決リテラルの型を決める (narrow
  int 含む、範囲外は変換エラー)。
- **FOR-RANGE-END: `for i in 0u64 to n {` が parse エラーだった件** —
  範囲の **末尾**だけ `ParseContext::Condition` で括られておらず、`n { ... }`
  を struct literal の開始として読んでいた (`to` / `..` 両形)。
- **NEWTYPE: tuple struct (`struct Meters(i64)`)** — 宣言はパーサが位置名
  (`"0"`, `"1"`, ...) のフィールドを持つ struct に desugar、`Meters(v)` /
  `m.0` / `Meters(v)` パターンは型検査器が `StructLiteral` / `FieldAccess` /
  `Pattern::Struct` に書き換える。
- **CHECK-NONTERMINATION: `--check` の trial にステップ予算** —
  生成値に対して body が終わらない入力 (`alloc_contract.t` の `triangle` に `n = u64::MAX`)
  で `--check` 自体がハングしていた。
- **DBC-CHECK-METHODS: `--check` がメソッドも掃く** — 自由関数と同じ規則
  (contracts 必須 / `ensures` をオラクルに / `requires` はフィルタ) で impl
  block の契約付きメソッドを検査し、`StructType::method` の名前で報告。
- **CONTRACT-ELISION の残 3 件** — 符号付き添字 / `!=` 由来の 0 除算 /
  `for` 範囲からの境界 guard を落とせるようになった。

### 2026-08-23
- **AOT-GENERIC-THROUGH-STRUCT: struct 引数越しの generic 型引数推論 (AOT /
  compiler JIT)** — `fn peek<T>(c: Cell<T>) -> T` の呼び出しが "cannot infer
  type arguments for generic function" で落ちていた。
- **TEST-PERF-CHECK-TRIALS: `--check` の trial ごとの registry
  再構築を共有化** — `execute_function_with_values` (property の trial が 1
  回 1 呼び出し) が毎回 program 全体の function maps / method registry /
  enum・struct registry / drop 収集を 0 から作っていた。
- **TYPE-NAME-SPELLING: 名前型の 3 つの綴りを統一** — parser は位置によって
  `Identifier(N)` (裸) / `Struct(N, args)` (`N<args>` は struct でも enum
  でもこれ) / `Enum(N, args)` (型検査後) を出すのに、generic 推論の
  `unify_types` にそれらを突き合わせる arm が無かった。
- **JIT-enum-1: struct のフィールドに enum を置けるように (3 backend)** —
  `FieldShape` に `Enum(Box<EnumStorage>)` を追加。
- **159: interpreter JIT の generic struct 対応** — `struct_layouts`
  を宣言ごとの**テンプレート**にし、型引数は**値の側**が持つ形にした
  (`FieldRepr::Generic` / `StructLocalInfo` / `ParamTy::Struct { base_name, type_args }`)。
- **CALL-ARG-COMPOUND-LITERAL: compound literal を call
  引数に直接渡せるように** — `f(Point { x: 1i64, y: 2i64 })` / `f((1i64, 2i64))`
  / `g.shifted(Point { .. })` / `Grid::make(Point { .. })` が 3 backend
  で通る。
- **STRUCT-FIELD-GENERIC-ENUM: struct のフィールドに enum
  を書けるようにした** —原因は 2 つで、generic とは無関係だった: (1)
  フィールド型の検証が `struct_definitions` しか見ていなかった (enum
  は別表)、(2) struct は宣言前に一括登録されるのに enum
  はされておらず前方参照できなかった (auto-load 由来の `Option`
  はどう並べても救えない)。
- **PATTERN-OR-NESTED: sub-pattern 位置の or** — `Circle(1i64 | 2i64)` /
  `Point { x: 0i64 | 1i64, y }` / `(0i64 | 1i64, n)`。
- **PATTERN-RANGE: 範囲を実 pattern 形にし、被覆判定を入れた** —
  `Pattern::Range` (half-open)。
- **PATTERN-AT-BINDING: `@` を実 pattern に (`Pattern::Binding`)** — `x @ Color::Red`
  / `whole @ Point { x: 0i64, y }` / `Just(n @ 3i64)` が書ける (任意の深さ、3
  backend)。
- **PATTERN-COMPOUND-LOWER: struct / tuple パターンを lowering 対応** —
  `MatchScrutinee` に compound を足し、フィールド /
  要素ごとに比較・束縛・再帰する dispatch を実装。
- **PATTERN-STRUCT: struct パターン** — `match p { Point { x: 0i64, y } => ... }`。
- **CONTRACT-ELISION: 添字境界の guard も消す** — `requires i < 4u64` +
  `[T; 4]` で `emit_index_guard` が落ちる (符号なし添字のみ)。
- **NEVER-ALLOCATES の残件 2 つを解消** — (1) メソッドにも
  `never_allocates` を書けるように (違反は `Counter::bad` と owner
  付きで報告)、(2) メソッド解決を receiver の型で行うようにし、`Vec::new` と
  `Counter::new` の取り違えによる誤検出を解消
  (型が取れない場合は従来どおり同名 body を全部辿る)。
- **DBC-CHECK-CASES: `--check` が「どれだけ試したか」を出す** — `Passed` に
  `discarded` を持たせ、サマリに合計ケース数、`requires`
  が狭くて数ケースしか通らなかった関数には `THIN` 行を出す。

### 2026-08-21
- **NEVER-ALLOCATES: 静的な「確保しない」** — `never_allocates fn f()`
  をコンパイル時に検査 (`[E0016]`)。
- **MEM-COUNTER-INTERP-DRIFT: アロケーションカウンタの定義を固定**
  —文字列補間が IR VM で 24 バイト・AOT で 0 バイトと数えられていた
  (同じ契約が engine で通ったり落ちたりする状態)。
- **ALLOC-CONTRACT-SUGAR: `ensures allocates(N)` / `retains(N)` /
  `allocations(N)`** —アロケーション契約の専用節。
- **DBC-TRAIT-INHERIT: trait の契約を impl に継承** — trait の method
  シグネチャに書いた `requires` / `ensures` が、それを実装する impl の method
  に適用されるようにした (trait の節が先、impl の節が後で AND)。
- **CONTRACT-ELISION: 契約が RUNTIME-TRAP の guard を消す** — `requires b != 0`
  で 0 除算 guard、`requires a >= b` で u64 underflow guard を lowering
  から落とす (パラメータ限定、シャドウで失効、`--release`
  では契約が検査されないので guard を残す)。
- **ALLOC-CONTRACT: `ensures` の `old(expr)`** —
  関数入口時点の値を参照できるようにし (parser が `__old_N` に desugar、3
  backend
  が入口で評価)、アロケーションカウンタと組み合わせて**メモリ挙動を契約で縛れる**ようにした。

### 2026-08-20
- **TYPECHECK-LIES: `null` を型検査で拒否 (E0015)** —
  型が付いて実行だけ落ちる唯一の構文だった。
- **RUNTIME-TRAP: 算術 / 添字の実行時トラップを 4 バックエンドで統一** — 0
  除算 / 符号付き `MIN / -1` / 添字境界外を `panic` 経路に載せ、
  `core/std/checked.t` (`checked_*` / `saturating_*`) を追加。
- **TEST-PERF: with-core フロントエンドパスを全レーンで共有 (4 レーン → 1
  パス)** —残っていた 2 レーン (AOT の `compile_file` / JIT の `run_source`)
  が with-core テストごとに `core/std/*.t` の integrate +
  型検査を再実行していたのを、**「型検査済み program
  を受け取る」ライブラリ入口**を足して畳んだ。
- **BUILD-PERF 運用: cargo-sweep 導入 + CLAUDE.md に定期 GC コマンドを明記**
  —「target を肥大させない」の運用を具体化。
- **FRONTEND-PERF: frontend の O(n²) を 3 点で解消 (★★★)** —
  コンパイル時間は frontend が支配 (24k 行 99.97s が 0.14s、**~700x**)。

### 2026-08-19
- **TEST-PERF: AOT の demand-driven lowering + codegen 刈り込み (★★★)**
  — auto-load された stdlib の ~190 関数を**毎回全部 lower / codegen**
  していたのを、`main` + `test` ブロックから到達可能な transitive closure
  だけに。

### 2026-08-18
- **tree-walker の関数再帰ガード (call-depth)** — IR VM (既定エンジン)
  はヒープにフレームを積むので 100000 段でも通るが、IR VM が lower を諦めて
  **tree-walker に fallback したプログラム**は host stack を使い、debug
  ビルドの 2 MiB テストスレッドで ~40 フレーム、main スレッド (8 MiB) で ~200
  フレームで `fatal runtime error: stack overflow` (exit 134) になっていた。
- **`null` / `is_null()` の扱いを確定 (docs の仕様に実装を追従)** — 方針 2 択を
  「**予約・実行時停止**」に決めた (docs/language.md が 2026-08-18 の監査で
  既にそう確定していた)。
- **型不一致診断の user 型を source 綴りに (interner 経由)** —
  `TypeCheckError` の `Display` は interner を持てないので、診断変換経路に
  interner を渡した: (1) `TypeDecl::spell_with(interner)` を新設
  (`source_name` → 無ければ `display_name`)、(2)
  `TypeCheckError::message_with(interner)` を新設して `Display`
  は引数なしで委譲、(3) `Diagnostic::from_type_check_error(error, file, interner)`
  に interner を追加 — 呼び出し側 (interpreter の `check_typing_diagnostics`)
  は borrow 衝突を避けるため `tc.core.string_interner` (共有参照) 経由、(4)
  `type_name_for_error` の catch-all (`{:?}` を lowercase) を `spell_with`
  に置き換え — `Cannot convert 'u64' to 'identifier(symbolu32 { value: 41 })'`
  が `'Point'` になる。
- **`str.substring` / `str.split` の dispatch を接続** — 型検査器は
  `BuiltinMethod::StrSubstring` / `StrSplit` を登録するのに、interpreter の
  `Object::String` レシーバ分岐 (`evaluation/call.rs`) の arm が `trim` /
  `to_upper` / `to_lower` で止まっており、実行時に `Internal error: Method 'substring' not found for String type`
  で停止していた。
- **`str + str` を型検査で拒否 (E0002)** — `visit_binary` が `str + str`
  を明示的に受理する arm があったが、**どのバックエンドにも実装が無い**
  (interpreter はゴミハンドル、AOT は bus error / exit 138)。
- **CLAUDE.md の `--message-format=short` 案内を `--format=json`
  に誘導** — `--message-format=short` はどちらの CLI にも実装が無く、渡すと
  usage を出して終わるのに「診断を 1 行にする手段」として繰り返し勧めていた。
- **INCR-INTEGRATE: 統合パスを placeholder 2 パス + HashMap から 1 パス +
  オフセット演算に** — 16 個の core module のキャッシュ読み込み + 統合
  (~10ms) の削減。
- **PATTERN-EXTEND: or / 範囲 / `@` パターン (3 バックエンド)** — `1i64 | 2i64 => ...`
  / `0i64..5i64 => ...` / `n @ 2i64 => n`。
- **INTERP-DIAG-SPAN: 補間内の診断が実際の位置を指すように** —
  補間の中の型エラーが**ファイル先頭 (1:1)** を指していた (LLM-LOOP-FIX
  が潰した「無関係なコードを自信満々に指す」形が 1 箇所残っていた)。
- **STR-INTERP-FMT: 補間の format spec (`"{x:.2}"`, 3 バックエンド)** —
  `[align]['0'][width]['.'precision][type]` (`< > ^` / `x X b o`) の Rust
  サブセット。
- **DOC-DRIFT 解消** — `docs/language.md` の *Generics and bounds*
  が「bound は parse されるが強制されない」と書いていたのを実際の挙動 (call
  site で強制、pass-through / generic trait の型引数一致 / 多重 bound)
  に直し、*Known limitations* から解消済みの 2 件 (enum 補間 /
  MATCH-LET-RHS-PAYLOAD-INFER) を削除。
- **STDLIB-ORD-BOUND: impl block の generic bound を call site で強制** —
  `impl<T: Ord> Vec<T>` の method は receiver の型引数が bound を満たさないと
  `[E0010] Method 'sort' generic parameter 'T' bound violation`。
- **STDLIB-ORD: `Ord` trait + `Vec::sort` (3 バックエンド)** —
  `core/std/cmp.t` に `trait Ord { fn lt(self: Self, other: Self) -> bool }`、
  `core/std/collections/vec.t` に `impl<T: Ord> Vec<T>::sort()` (安定
  insertion sort)、`core/std/string.t` に `impl Ord for String` (byte-wise)。
- **RUNTIME-IO 拡張: 乱数シード / 時刻フォーマット / 環境変数一覧 (3
  バックエンド)** — `core/std/io.t` に `random_seed(seed)` / `strftime(fmt, secs)`
  / `env_count()` / `env_name(i)` / `env_value(i)` を追加。
- **TRAIT-BOUND: generic trait の bound を call-site で強制** — `fn first<I: Iter<i64>>(it: I)`
  が「型引数込みでその trait を実装している struct」だけを受け付けるように。
- **FROM-INTO: `.into()` と `?` の cross-error 変換** —
  `core/std/convert.t` に `trait From<T>` / `trait Into<T>`、`String` に `impl From<str>`。

### 2026-08-17
- **STDLIB-ITER-ADAPT: `VecIter` に `map` / `filter` / `enumerate` / `zip` /
  `collect` (3 バックエンド)** — `core/std/collections/vec.t`。
- **STDLIB-ITER-ADAPT (Dict / String 版)** — `DictIter<K, V>` に `map` /
  `filter` (`core/std/dict.t`)、`StringIter` に `map` / `filter` / `enumerate`
  / `collect` (`core/std/string.t`)。

### 2026-08-16
- **RUNTIME-PORT R0+R1: ランタイムを C から Rust に移植** — `toylang_rt`
  crate (`compiler/runtime/toylang_rt/`、`no_std` + alloc、依存 0) が
  `toylang_rt.c` の全 53 シンボルを継承し、**AOT と compiler 側 JIT
  が同一ソースを実行**するように (旧 jit.rs の ~880 行ミラーを削除)。
- **RUNTIME-PORT R2 + FFI_PLAN P1: `extern fn ... from "lib" [as "sym"]`**
  — extern 宣言がシンボルとライブラリを直接名指しできるようになり
  (`Function.extern_link`)、`core/std/io.t` が `getchar` / `time` を `from "c"`
  で宣言して **`read_line` / `now` が toylang 実装**に (RUNTIME-IO の
  `toy_io_read_line` / `toy_io_now` を toylang_rt から削除)。
- **RUNTIME-PORT R3: toylang 化の計測と判断 (移動は中止)** — `str_eq` /
  `str_concat` / `to_string_*` の toylang 実装をプロトタイプで書いて計測:
  既定エンジン (IR VM) で byte-walk パターンは **~20 倍遅い** (str==str 40000
  回: 0.2s → 3.9s) で doc の中止条件「interpreter
  が目に見えて遅くなる」に該当 → Layer 1 に残す (設計判断)。
- **RUNTIME-PORT R4: f64 整形の toylang 化は計測で却下、byte 一致は固定** —
  doc の手順「interpreter とバイト一致を先にテストで固定」どおりに進めた。
- **RUNTIME-IO: 最小 I/O セット (3 バックエンド)** — `core/std/io.t` に
  `read_line()` / `argc()` / `arg(i)` / `env_var(name)` / `read_file(path)` /
  `file_exists(path)` / `now()` / `random()`。
- **STDLIB-ITER: `Vec` / `Dict` / `String` に `iter()` (3 バックエンド)** —
  `for x in v.iter()` / `for kv in d.iter()` (キー順は挿入順、payload は `(K, V)`
  タプル) / `for b in s.iter()` (1 バイトずつ)。
- **DROP-GLUE: 移動先が再帰的に解放される (3 バックエンド + IR VM)** —
  `Box` を `Vec` / struct field / enum payload
  に移しても、コンテナの死とともに中の値が free される。
- **BOX-T Phase E+F: stdlib `Box<T>` (`core/std/box.t`)** — `enum List { Cons(i64, Box<List>), Nil }`
  が 3 バックエンドで動く。
- **BOX-T Phase D: 移動された束縛は drop しない (3 バックエンド)** —
  実測していた use-after-free が解消。
- **BOX-T Phase C: 所有権の移動と use-after-move チェック (E0014)** — `impl Drop`
  を持つ型の値を「今のスコープより長生きする場所」(値渡し引数 /
  struct・tuple・array の要素 / 代入右辺)
  に置くと所有権が移り、以後その名前を読むと E0014。
- **BOX-T Phase A+B: 型引数経由の再帰を通す** — `struct Tree { kids: Vec<Tree> }`
  が書けるようになった。
- **PTR-READ-ENUM: enum の byte layout を関数境界の flatten に統一** — enum
  に**サイズが 3 つ**あった (`__builtin_sizeof` の `1 + max(payload)` /
  tree-walker の「手元の variant 依存」/ 関数境界 flatten の `u64 tag + 全 variant 連結`)。
- **`__builtin_ptr_read` が user 定義型名の注釈を受けるように** — `val n: Node = __builtin_ptr_read(p, off)`
  が通る。
- **RECURSIVE-TYPES step 1: 再帰型を診断で拒否 (E0013)** —
  間接化なしで自分を含む struct / enum は有限な layout
  を持てないのに、型検査を素通りして lowering (`instantiate_struct` /
  `instantiate_enum` は memo 化の**前**にメンバを lower する) で host stack
  を食い潰し、**exit 134 / メッセージ無し**で abort していた。
- **parser: 改行前 `(` は method call に継続しない** — `b.v\n(x as i64)` が
  `b.v(...)` と parse
  され、ユーザが書いていない呼び出しについて型エラーが出ていた。
- **CONCRETE-IMPL-Phase-2c (generic-wildcard 完遂)** — 型チェッカの method
  registry を `Vec<MethodSpec>` 化し、3 層 (型検査 / interpreter / compiler)
  の dispatch を exact → wildcard → lone-spec に統一。
- **STR-INTERP-COMPOUND-EXTEND-ENUM** — enum 値の補間を AOT / compiler JIT
  で (tag brif chain + variant ごとの concat)。
- **MATCH-LET-RHS-PAYLOAD-INFER 完遂** — method call / field-access
  レシーバの method call を scrutinee に持つ val/var 右辺 match。

### 2026-08-15
- **`--check` が満たせないサイズの確保で Rust panic していたのを修正** —
  確保失敗は全バックエンドで null ポインタに統一。
- **lexer エラーを診断として報告 (E0012)** — 従来は `Ok(None)`
  に潰れて無関係な行の型エラーに化けていた。
- **struct field / enum payload の初期化形を 4 形に揃えた (AOT/JIT)** —
  literal / 既存束縛 / call / associated fn / method call を、struct 型・tuple
  型の両方で。
- **compound 値の読み出し側を AOT/JIT で** — `val inner: Inner = o.i` /
  `println(o.i)` / `"{o.i}"`。
- **receiver を読まない method の AOT panic を修正** — `self`
  を一度も書かないプログラムでは symbol が intern されず receiver が parameter
  列から落ちていた。
- **非 ASCII のソースリテラルの化けを修正** — UTF-8 scalar 単位で写す。
- **テストスイートを 5.15s → 4.4s に (-15%、CPU 90s → 82s)** — rayon の
  oversubscription / example sweep の二重パース / shard 4 → 12 / discard
  budget。

### 2026-08-13
- **`str == str` を内容比較に統一** — interpreter (tree-walker)
  だけが内容比較で、他 4 実装は runtime handle の整数比較だった
  (**型は通るが答えが違う** divergence)。
- **`Display` trait — 型が自分の見せ方を決める** — `core/std/fmt.t`。
- **str リテラルの数え差 (interpreter だけ +1 確保) を解消** — IR VM が str
  リテラルを counter-free に実体化 (コンパイル系の `.rodata` と同じ扱い)。
- **Drop 内で `&mut self` フィールドを free すると use-after-free
  するバグを修正** — **struct 束縛を return すると、そのローカルにも
  scope-exit の Drop が発火**して戻り値が dangling になっていた (AOT segfault
  / IR VM panic)。
- **ポインタ演算 builtin (`__builtin_ptr_offset`)** — interior pointer。
- **allocator レジストリ: `layout_report` を `--profile=mem`
  に自動で載せる** — `__builtin_record_allocator_layout` + `impl Drop`
  フック。
- **エンジン fallback 時の副作用重複実行を修正** — IR VM が出力後に diverge
  すると tree-walker の再実行で `println` が 2 回出ていた。

### 2026-08-10
- **MEMORY-PROFILING M0〜M5 完了** — 設計は
  [`MEMORY_PROFILING.md`](MEMORY_PROFILING.md)。
- **暗黙の impl 型パラメータ (`impl Container<T>`)** — `docs/language.md`
  が明記していたのに**未実装**で、リファレンス自身の例が型検査を通らなかった。
- **stdlib の generic enum HOF を全 backend で** (`Option::map` /
  `Result::map` / `map_err` / `unwrap_or_else`) — enum receiver の generic
  method target 解決 / 関数型パラメータ内にしか現れない method-only generic
  param の推論 / その monomorphisation の 3 つのギャップ。
- **MATCH-LET-RHS-PAYLOAD-INFER (第一段)** — 全 arm が payload 束縛の match
  を val/var 右辺に。
- **AOT-MATCH-SCRUTINEE-EXPAND** — enum を返す**関数呼び出し**を match
  scrutinee に許可 (`while val Some(x) = func(i)`)。
- **INCREMENTAL-COMPILATION Phase 5** — 実測して再スコープ。
- **LLM-LOOP-FIX: 式の span を full extent に** — postfix / unary / literal
  系が「自分を名付けるトークン」しか指しておらず、field access と `if` /
  `match` / `with` は**次の文の先頭**を指していた (P2
  が潰したはずの「無関係なコードを自信満々に指す」形)。
- **LLM-LOOP-FIX: 壊れた `as` キャスト提案 / 引数型不一致 E0010 → E0001 /
  JIT 列の空洞化** — machine-applicable 提案が `f(a) as i64`
  を生成して元のエラーを解決していなかった。
- **LLM-LOOP P7: 補助 CLI** — 型ホール `val x: _ = expr` (専用コード
  E0011)、`--api <file>` (シグネチャ一覧)、`--explain [<CODE>]`。
- **DEV-LOOP D6: `--all-backends` + stdin 入力** — `compiler f.t --all-backends`
  が 3 バックエンドを 1 コマンドで実行し一致なら 1 行。
- **DEV-LOOP D5: `CODE_MAP.md` 新設 + `CLAUDE.md` から履歴を分離** —
  常時ロードされる 48 KB の 30% が changelog だった。
- **LLM-LOOP P6-3: u64 アンダーフローの trap** — `0u64 - 1u64` の wrap
  を全バックエンドで trap 化。

### 2026-08-09
- **LLM-LOOP P0〜P6** — 設計は
  [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md)。
- **DEV-LOOP D1〜D4, D7** — 設計は
  [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md)。
- **D7 sweep が検出した潜在バグ 5 件** — AOT の代入式が値を produce
  していなかった / f64 `%` の明示エラーが cranelift assertion に化けていた /
  struct field 欠落がコンパイラクラッシュになっていた / JIT の `main`
  キャッシュが `File`
  のポインタ同一性をキーにしており別プログラムが前のコードを実行しえた
  (`File::id` 導入)。
- **`else if` の拒否 + パースエラーの握り潰し解消** — `else if`
  は「未サポートだが無害」ではなく 3 通りに壊れていた (偶然動く / 実行時 null
  エラー / ファイル残りを黙って破棄)。
- **`var` の型注釈チェック追加** — `var w: bool = 1u64` が通り `println(w)`
  が `1` を出していた (`val` は正しく拒否)。

### 2026-05-31
- **Interpreter IR VM 化 Phase 0〜4** — `compiler_ir` / `compiler_lower` を
  crate として切り出し (循環依存の解消)、`interpreter/src/ir_vm/` に VM
  を新設。
- **意味論の決着 3 件** (git だけでは追いにくいので残す):
- **負数 array index を compiler 側にも実装** — tree-walker
  のみ対応していた `a[-1]` を lowering に移し 3 バックエンド統一。
- **clippy fixes across workspace** — auto-fixable lint 適用 + 手動修正。

### 2026-05-23
- **core/std 並列パース (Phase 1)** — module integration を parallel
  pre-parse + sequential integrate の 2 段に分割、二重パースを除去。
- **Cranelift 関数 codegen 並列化 (Phase 2)**。
- **Incremental compilation Full AST cache (Phase 4)** — 一度 revert
  された後の再挑戦で成功。

### 2026-05-19
- **`dyn Trait` Phase 2 (A5-P2-MVP-A〜F)** — AOT の動的ディスパッチを empty
  struct → scalar field → nested struct + `&mut dyn` writeback → struct
  return → tuple / enum return → `&mut self` + compound return の順に
  landing。
- **Bare-name imported function calls (Phase 1)** —
  `enforce_import_namespace` を削除し import した `pub fn` を bare name
  で呼べるように。
- **`Program` → `File` rename (Phase 4)** と後方互換 alias の削除。

### 2026-05-18
- **`dyn Trait` Phase 1 (A5-P1)** — interpreter で `&dyn Trait` を
  landing。
- **Trait 多重 bound `<T: A + B>` (A2)** — call-site の bound check は
  AND、method dispatch は OR。
- **Trait デフォルトメソッド本体 (A1)** — AST mutation pre-pass で impl に
  synthesize。

### 2026-05-17
- **`?` (Try) early-return operator** — 型チェッカが `match` に in-place
  rewrite するのでバックエンドは Try を観測しない。
- **`loop {}` + comparison chain** — parser-level desugar。
- **GENERIC-ENUM-MATCH-HOF** — `Option::map` / `Result::map` / `map_err` を
  stdlib に追加。

### 2026-05-10
- **DEBUG-BUILTINS Phase A+B+C** — `__builtin_source_file/line/column` と
  `assert_eq` / `assert_ne` / `dbg` の parser-level macro 群。

### 2026-05-09
- **IF-VAL (`if val` / `while val`)** — pure parser desugar。

### 2026-05-08
- **LABEL (labelled break / continue)** — `@label:` 形式 (Rust 風 `'label:`
  は char literal と衝突するため)。
- **OP-OVERLOAD 完全コレクション** — 同型 struct ペアの全 binary + unary
  operator を user method に dispatch。
- **STRING-NOMINAL + STR-INTERP-COMPOUND** — `String` を `Vec<u8>` alias
  から**独立した nominal struct** に変更。
- **AOT lower 系の汎用拡張** — `__builtin_sizeof` の compound
  対応、`__builtin_ptr_write/read` の compound 対応。

### 2026-05-07
- **ITER-PROTOCOL-TRAIT** — generic trait 宣言 `trait Foo<T, U>` と `impl Foo<i64> for Counter`。
- **ITER-PROTOCOL-AOT** — `for x in EXPR` を AOT でも動作。
- **STR-INTERP Phase 2 (AOT + cranelift JIT)** と **STR-INTERP-INTERP-JIT**
  — interpreter 側 JIT では `str` を **function 境界 (param / return)
  で禁止** (Object lifecycle 整合性のため)。

### 2026-05-06
- **STR-INTERP Phase 1 (interpreter)** — `"hello {name}"` を lexer +
  parser-level desugar で。
- **ITER-PROTOCOL Phase 1 (interpreter + JIT)** — structural (duck-typed)
  で、generic trait `Iterator<T>` 自体は使わない。

### 2026-05-05
- **NUM-LIT-SEPARATORS** — `1_000_000u64` 等。
- **CLOSURES Phase 1〜8** — frontend / 型検査 / interpreter / AOT (direct /
  indirect / capturing / narrow int / return / struct field 格納)、`fn (T) -> R`
  関数型構文。
- **DOCS-2026-05-05** / **NUM-W-JIT** / **ZERO-MEMCOPY-FIX** (`size==0` の
  libc parity) / **TYPE-ALIAS 周辺整備**。

### 2026-05-04
- **エスケープシーケンス** — `\u{HEX}` / `\xHH` / char literal `'a'`
  (u32)。
- **TYPE-ALIAS / GENERIC-TYPE-ALIAS** — parse 時即時展開 + generic alias。
- **GENERIC-RAII** — user struct の `impl Drop` を scope-bound auto-call
  (interpreter + AOT)。
- **ALLOCATOR Phase 5** — `trait Drop` + temporary-form の auto-cleanup。
- **REF-Stage-2** — `&T` / `&mut T` の borrow + writeback、escape rule
  の構文 reject。
- **121-Phase-B-rest** — arena / fixed_buffer の native runtime、`with`
  body 早期 exit の cleanup。
- **TEST-PERF-lazy-core** / **STRING stdlib** / **CONCRETE-IMPL Phase
  1〜2b**。
- **NUM-W (Phase 1〜6 + AOT + AOT-pack + signed-hash)** — 狭い数値型
  (u8/u16/u32/i8/i16/i32) の interpreter + AOT 完全対応。
- **DICT 系まとめ** — `Dict::new()` の AssociatedFunctionCall
  経路、per-monomorph generic substitution。

### 2026-05-03
- **VEC-collection** — user-space `Vec<T>` (`core/std/collections/vec.t`)。
- **STR-LEN-O1 / STR-PTR-LEN** — AOT で `__builtin_str_len` を O(1)
  化、`.rodata` layout を `[bytes][NUL][u64 len LE]` に。
- **121-Phase-B-min / Phase-A** — Allocator builtin 群、heap / pointer
  builtins。
- **MUT-SELF-Stage-1** — `&mut self` receiver。
- **96残-前半** — match の deep exhaustiveness check。

### 2026-05-02 以前 (大きめのマイルストーン)
- **#183 コンパイラ MVP** — IR / cranelift-object backend
  で実行ファイル生成。
- **per-module function namespacing (#193 / #202)** — IR + 型検査 +
  実行時の関数テーブル分離。
- **コア・モジュール auto-load (#193)** — `<repo>/core/`
  を起動時に再帰的にロード。
- **Extension trait 全 backend 対応 (#191、Step A〜F)** — primitive 型への
  trait impl。
- **Math externalisation (#190、Phase 1〜4)** — `extern fn` 経由の f64 math
  intrinsic。
- **Option / Result stdlib (#203)** — `core/std/option.t` / `result.t` と
  AOT enum receiver dispatch。
- **Value/Reference 分離 Phase 1〜5** — fibonacci -8% / for_loop -12%。
- **panic / assert / DbC (#166〜#175)** — `requires` / `ensures` 全 backend
  対応。
- **言語仕様拡充 (#161〜#165)** — f64、`%`、複合代入、タプル
  JIT、ネスト分解、match arm guard。
- **#184 Trait + impl** / **#170 top-level const** / **#169
  `docs/language.md` 新設**。
- **MODULE-SYSTEM P1 — stdlib の配置と名前** — `ord.t` → `cmp.t` /
  `display.t` → `fmt.t` / `str_ops.t` を `str.t` に統合 /
  `i64.t` + `f64.t` → `num.t`。設計は
  [`MODULE_SYSTEM.md`](MODULE_SYSTEM.md)。
- **MODULE-SYSTEM P2 — qualifier がモジュールのフルパスになった** —
  3 つの関数表 (型検査 / IR / ランタイム) が候補ごとにパスを持ち、
  呼び出し側の qualifier は末尾一致で解決する。同名ファイル 2 つの
  `function_index collision` panic が、候補を名指しする型エラーに
  なった。`math::abs` の 1 セグメント形はそのまま。
## 未実装 📋

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

- **TEST-PARALLEL の残り: ジョブの配り方** — P6 (`--backend all`) は
  2026-09-25 に landing (完了済み節)。残るのは配り方だけ: 今は plan 順。
  P4 (`.testtimes` に所要時間を残して長い順に配る) は実装したが**状態を
  増やすので取り下げた**。状態を持たない候補は「自分が検査済みの
  ファイルから優先して取り、空いたら盗む」ワークキュー (VM レーンの
  粒度問題は解けるが、偏ったスイートの順序は解けない)。**効くのは
  `-j` を絞ったときだけ**なので優先度は低い。設計と測定は
  [`TEST_PARALLEL.md`](TEST_PARALLEL.md)。
- **MEMORY-ACCESS M4: `chunks::<N>()` と `read_uNN_le/be`** —
  設計は [`MEMORY_ACCESS.md`](MEMORY_ACCESS.md)。M3 で範囲を答える
  primitive は入ったが、**ブロック単位の反復と幅つきスカラー読みは
  まだ手書き**: `hex.t` / `base64.t` は「16 バイトずつ + 端数を
  スカラーで」を自分で書いており (同じアルゴリズムの 2 実装)、
  `base64` / `sha256` は `(b0 as u64) * 65536u64 + ...` で桁を
  組み立てている。`for c in s.chunks::<16>()` が端数を持つ形にすれば
  書き手は 1 回で済み、`s.read_u32_le(i)` は endianness を型の側に
  置ける。

- **UNSAFE-REST: 残る `unsafe fn` (59 本)** — 2026-09-25 にコレクション
  (`Deque` / `Set` / `PriorityQueue` / `SoaVec`)、String の SIMD・`to_str`、
  `hex` / `base64` の encode・decode (`Ptr<u8>::load16` / `store16` を
  intrinsic にして速度は不変)、`path` / `fs` / `time` / `testing` / `io`
  の名残を外し、102 → 59。**生 builtin の置き場** (`allocator.t` 30 /
  `span.t` 14 / `ptr.t` 9 / `column.t` 3) が 56 で、これは残る側。
  それ以外は `String::eq` (SPAN-RANGE-INTRINSIC) だけ、加えて
  `Vec<u8>::extend_bytes` / `String::extend_bytes` (生の `ptr` を受ける —
  呼ぶ側に義務がある API なので `unsafe` が正しい)。`unsafe` の意味を
  「呼ぶ側に義務がある」に変える案 (呼び出しに `unsafe { }` を要求) は
  ユーザ判断待ち。

- **SPAN-RANGE-INTRINSIC: `Span` の範囲演算を呼び出しにしない** —
  `String::eq` を `Span::bytes_eq` で書くと `poc/logsearch` の archive が
  ~9% 遅くなった (窓 2 つ + 呼び出し、`Dict<String, _>` のキー比較に
  乗る) ので、`eq` は生の `__builtin_mem_eq` のまま `unsafe fn` で残した。
  `Ptr` の `get` / `set` / `borrow` と同じく、stdlib の `Span` の
  `copy_from` / `bytes_eq` / `find_seq` を lowering と tree-walker で
  builtin に直結すれば戻せる。

- **WINDOW-ESCAPE-UNWRAP: `Option` から出した窓の脱出を見逃す** —
  `[E0026]` は `v.as_span()` (`Option<Span<T>>`) をそのまま返すのは
  拒否するが、`val s: Span<u64> = w ?? panic(..)` や match の payload で
  取り出してから返すと通る (taint が unwrap で途切れる)。M5 の
  `Ptr::borrow` を足すときに見つけた (2026-09-25)。`??` / match の
  payload 束縛が scrutinee の taint を引き継げばよい。

- **ZIP-ITER-GENERIC-SCOPE: method-level の型引数が turbofish から
  見えない** — `VecIter<T>::zip<U>(other: VecIter<U>)` の中で
  `__builtin_sizeof::<U>()` が `[E0010] unknown type \`U\`` になる
  (`check_sizeof_type_arg` は impl の generic params と型推論 scope しか
  見ない)。`ZipIter` が 2 つの stride を `elems` にパックして持って
  いたのはこれの回避だったが、M5 で `Ptr<A>` / `Ptr<B>` 経由の読みに
  なり、stride 自体を持たなくなった (2026-09-25)。

- **CONST-ARRAY の残り: struct / tuple の表の渡し方** — 2026-09-25 に
  struct / tuple 要素の表が compiled レーンで読めるようになった
  (`val r = RS[i]` / `RS[i].w`、完了済み節)。残り: その表を `&[R; N]` で
  渡すこと (`scalar_array_ref` がスカラー要素だけを番地で渡す)、値の
  位置に直接置く `f(RS[i])` (束縛を案内するエラー)、enum 要素の表。
- **STDLIB-CRYPTO C2〜C4: SHA-512 族 / HMAC / SHA-1・MD5** —
  設計と優先順位は [`STDLIB_CRYPTO.md`](STDLIB_CRYPTO.md)。C2 (SHA-512 /
  384 / 512-256) は C1 と同型で lane が u64 になるだけ、C3 (HMAC) は
  `trait Digest` が元を取る場所、C4 (SHA-1 / MD5) は相互運用専用で
  壊れていることを明示する。

- **NEVER-ALLOCATES-METHOD-STACK: method に `never_allocates` と `unsafe`
  を重ねられない** — `parse_method_modifiers` (`frontend/src/parser/stmt.rs`)
  が両方の修飾子に「次が `fn`」を要求するので、impl 内の
  `never_allocates unsafe fn` / `unsafe never_allocates fn` が parse
  エラー。自由関数側 (`program_parser.rs`) は「次がもう 1 つの修飾子」も
  通す。CLAUDE.md の「順不同」に実装が追いついていない。`Vec` の読み取り系
  (ほぼ全部 `unsafe fn`) に `never_allocates` を付けられない原因
  ([`VEC_CONTRACTS.md`](VEC_CONTRACTS.md) §5-1、2026-09-04)。

- **DBC-CHECK-SKIP-REPORT: `--check` が `ptr` レシーバの method を黙って
  飛ばす** — design_by_contract.md には明記があるが、`Vec` のように契約が
  増えるほど「検査されたつもり」が危険。最低限 `SKIPPED` 行を出す。
  本命は構築子 (`new()` + ランダムな `push` 列) 経由でレシーバを生成すること
  で、collection に `--check` を効かせる唯一の道
  ([`VEC_CONTRACTS.md`](VEC_CONTRACTS.md) §5-3)。

- **FN-NAME-AS-VALUE: トップレベル関数の名前を `fn` 値として渡せない**
  ★★ — `fn twice(x: u64) -> u64` があっても `apply(twice, 21u64)` は
  `[E0001] expected fn (u64) -> u64, but got u64`。closure literal を
  `val` に束縛すれば通るので回避はできるが、`Vec::sort_by(cmp)` の
  ような comparator API は毎回これを踏む。2026-09-03 に COLLECTIONS C3
  で発見。
  **型検査の 1 行ではない** (2026-09-21 に測った): 値の位置の関数名は
  `visit_identifier` が**戻り型**を返しているので、そこを
  `TypeDecl::Function(params, ret)` に変えるのは 1 箇所で、全テストも
  通る。**通らないのはその先**で、
  * tree-walker: 識別子が呼べる値に評価されない
    (`Undefined variable`)。`Object::Closure` の `body` は `ExprRef`
    なので、関数の `code` (`StmtRef`) から中の block を取り出せば
    作れそう
  * compiled lane: **`fn` 引数は env つきの closure ABI**
    (`CallIndirect` は callee を env とみなし、`env+0` の fn_ptr を
    読んで env を前置する)。生の `FuncAddr` はそのままでは渡せず、
    `(env, args...) -> R` の**thunk を新造**して `MakeClosure` で
    包む必要がある (dyn の `PendingThunkBody` と同じ手口)

  やるなら 3 レーン通しで。半分だけ入れると型検査がどのレーンでも
  走らないプログラムを受理する。


- **tuple 要素の `Vec` / `SoaVec` が AOT 不可** ★ —
  `Vec<(i64, u64)>` は `push` の `__builtin_sizeof(value)` が
  `could not infer arg type at AOT` になる。iterator アダプタの
  `enumerate` / `zip` の `collect` を提供していないのと同じ制限で、
  そちらは stdlib 側で避けている
- **DIAG-DEBUG-FMT の残: codegen 層** ★ — `compiler_lower` と
  `compiler/src` の parse error は 2026-09-02 に決着したが、
  `compiler/src/codegen/` には `{:?}` が **50 箇所以上**ある
  (`missing import for {target:?}` / `block {b:?} unterminated` /
  `invalid {:?} → f32 cast`)。**大半は internal error** (コンパイラの
  バグを報告する文言で、`FuncId` や `BlockId` こそが必要な情報) なので
  スキャナの対象に入れると偽陽性だらけになる。入れるなら
  「ユーザに見える refusal」と「internal error」を先に分ける必要がある

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
- **COMPOUND-BLOCK-RHS の残: method call の枝** ★ —
  `val p = if c { x.twin() } else { .. }` は
  `detect_struct_result` が method の戻り型を安く引けないので検出されず、
  従来どおり「compound-returning method を式の位置で使えない、`val` で
  束縛せよ」というエラーになる。誘導が具体的なので実害は小さい。
  (2026-09-01 に **match の arm 束縛** と **`return` する arm**、
  2026-09-06 に **block を挟んだ形**は解消。残っているのは method call
  の枝だけ。)
  同じ理由で、block の先頭束縛から enum を引く `pending_enum_id` も
  **注釈か素の call** しか読まない — `val t = Foo::open(p)` を注釈なしで
  書いて `match t { .. }` を tail に置くと従来どおり検出されない。
  `?` / `??` は自分で注釈を書くのでこの穴に落ちない。

- **TREE-WALKER-CONCRETE-IMPL** ★ — `impl C<u8>` と `impl C<i64>` の
  両方に同名の associated function があると tree-walker が spec を
  1 つしか持たず解決できない (`concrete_associated_hint` を
  `assert_consistent_without_tree_walker` で除外)。
  method 版と「concrete + generic」の組合せは動く。
  2026-08-25 に tree-walker レーンを本物にして初めて見えた
  (対だった TREE-WALKER-NUM-W は 2026-08-28 に解消)。

- **JIT-INTERP-COVERAGE (residual)** ★ — interpreter 側 JIT が silent
  fallback する残り: (a) impl block ではなく **method 固有の generic**
  (`fn map<U>(..)`) と **phantom 型パラメータ** (どのフィールドも触れない
  `T` は literal から復元できない、#159 の残)、(b) **範囲 / `@` / struct /
  tuple パターン** (`check_match_pattern` が literal / wildcard /
  enum variant しか受けない)、(c) **enum の payload 形** — 単一・一様
  スカラーのみなので `Option<Option<T>>` や struct / tuple payload は
  対象外 (compiled 側の同名の制限は JIT-enum-1 で解消済み。こちらは
  `EnumLayout` が別実装)、(d) **enum 型の struct field** (`StructLayout`
  は scalar フィールドのみ)。どれも correctness 問題ではない。
- **160. タプルの JIT 対応 (ネスト)** ★ — `((a,b),c)` と tuple-of-struct。`ParamTy::Tuple(Vec<ScalarTy>)` を tree 構造にする 100+ 箇所の refactor。(inline tuple literal を call 引数に渡す件は 2026-08-23 に CALL-ARG-COMPOUND-LITERAL で解消)
- **OP-OVERLOAD-ENUM: enum の operator overload** ★ — `impl SomeEnum` に
  `eq` を書いても効かない。型検査は 2026-08-29 に「宣言済み struct のみ」へ
  絞ったので通らないし、通したとしても interpreter の
  `overload_method_name` 経路が `(Object::Struct, Object::Struct)` しか
  見ず、AOT の `try_lower_struct_cmp` も struct 前提。enum 同士の比較は
  今のところ variant を match する (tuple scrutinee は AOT 非対応なので
  ネストするか scalar tag に落とす)。実プログラムで踏んでから。

- **SIMD-F32 の残** ★ — **2026-09-22 に `&f32` / `[f32; N]` /
  クロージャの `f32` が解消**(手書きの型リスト 5 つが `f32` を
  知らなかった)。残り: (a) **format spec 未対応**: `{x:.2}` の
  formattable 集合に f32 を入れるには `toy_format_f32` が要る
  (promote して f64 で整形すると最下位桁が変わるので専用ヘルパ)。
  (b) **f32 の math intrinsics** (`math::sqrt_f32` 等) は未提供 —
  `x as f64 → math::sqrt → as f32` の橋渡しで代替できるが、
  cranelift の `sqrt` は F32 を受けるので unary op 経路の supplied helper
  を増やせば direct にできる。(c) **`f32` の `min` / `max` 演算子**は
  f64 同様 AOT 未対応 (cranelift の fmin / fmax で入れられる)。
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

### 標準ライブラリ・実行環境 (STDLIB-RUNTIME)

> 2026-08-16 に「言語機能として何が残っているか」を実際に叩いて洗い出した結果。
> RUNTIME-IO (Result を返す IO) は 2026-08-29 に landing 済み (完了済み節)。
> 実プログラムを書けなくしている残りは下記。
> 俯瞰と優先順位 ([`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md)、2026-08-30 実測)。

> 2026-09-02 に `core/std/` 全 28 ファイルを**分野で**棚卸しした。以下は
> 分野ごとの空白で、`RUNTIME_LIBRARY.md` の P1〜P4 に対応する
> (下の 2 件も分野で言えば TEXT と IO に属する)。

> 2026-09-05 に **関数の粒度**でもう一度棚卸しした
> (`poc/logsearch` の `RUNTIME_GAPS.md` を突き合わせ相手にした)。
> 「モジュールは在るが標準的な関数が 1 本足りない」形の空白 8 分野を
> [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) の「関数粒度の空白」節に
> 置いた (**ここに表を再掲しない**)。効きの大きい順に
> **A 反復子の終端操作 (`fold`/`any`/`all`/`count`/`sum`) →
> B `Ordering` と汎用 `min`/`max`/`clamp` → F CRC-32**。
> どれも純 toylang か extern 1 行で書ける。処理系側が要るのは
> **ファイルハンドル (`open`/`seek`/`pread`/`fsync`)** と
> **シグナル捕捉**の 2 つで、これは同節の H に分けてある。

- **HOF-RETURN-UNKNOWN: 関数を値として渡す形が使えない** ★ —
  (a) **名前つき関数を値として渡せない** — `fn run(f: fn () -> ())` に
  `run(work)` と書くと `[E0001] expected fn () -> (), but got ()`
  (名前が関数の**戻り型**に解決される)。(b) closure リテラルを渡すと
  通るが、**その呼び出しの戻り型が `Unknown` になる**ので
  `val b: Bench = bench(3u64, fn() -> () { })` の `b.iters` が
  `field access for type Unknown`。2026-09-03 の STDLIB-TIME で
  `bench(iters, f)` を書こうとして踏み、**`bench` を入れずに
  `Stopwatch` だけにした**
- **並行性 (CONCURRENCY)** は分野としては stdlib だが、本体が move /
  Drop モデルとの接合なので「検討中の機能」節に置いてある (★★★)。
  RUNTIME_LIBRARY P3 も「設計文書を別に取ってから着手」と同じ判断。
  **設計文書は 2026-09-18 に取り、2026-09-20 に §5 を決めて A1 を
  landing した** → [`CONCURRENCY.md`](CONCURRENCY.md)

- **io.t の範囲外 `""` 既定の厳格化** ★ — `arg(i)` / `env_name(i)` /
  `env_value(i)` は範囲外で `""` を返す (ドキュメント化済みの既定)。
  `arg(i)` の `""` は「実際に空文字列の引数」と区別がつかない。
  `argc()` / `env_count()` で範囲チェックできるので設計上は許容だが、
  厳格化するなら `Result<_, IoError>` 化 (`IoError` に
  `OutOfRange` variant を足す) か `Option` 化。RUNTIME-IO と同じ
  ペア status extern の仕組みで境界変更なしにできる。実プログラムで
  困ってから。

### 実行時の意味論 (RUNTIME-TRAP)

> 2026-08-20 に算術 / 添字を 3 バックエンドで実際に叩いて洗い出した節。
> トラップ本体は同日 landing (完了済み節)。残りは下記。

- **CONTRACT-ELISION の残** ★ — (a) 片辺が literal の形
  (`requires a >= 5u64` で `a - 3u64` の guard を消す — `at_least` が
  param-param のみ)、(b) `x != MIN` の形 (lhs 側の `MIN / -1` 条件)。
  どちらも「実プログラムで書いていて guard がホットパスにある」を
  確認してから。

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

- **TYPECHECK-LIES 残: `str.substring` / `str.split` の AOT/JIT 対応** ★ —
  2026-08-20 に 3 件を実測したところ、**本物の嘘は `null` だけ**だった
  (E0015 で拒否、同日 landing)。`str + str` は元から型検査が拒否して
  おり (メッセージを E0004 + `concat` 誘導に改善)、`substring` / `split`
  は interpreter で**動く** — docs 側の Known limitations が古い記述を
  抱えていたのを実態に合わせた。残るのはバックエンドカバレッジで、
  compiled 側は `the method receiver must be a struct or enum binding`
  で受け付けない (`String` の同名 method には制限なし)。

### コンパイル時実行 (CTFE)

> 2026-08-26 に `const` / 契約 / IR を実際に叩いて洗い出した節。設計は
> [`COMPILE_TIME_EVAL.md`](COMPILE_TIME_EVAL.md) (現状調査 7 件 + 論点 6 +
> Phase C0〜C6)。**C0〜C6 はすべて landing 済み** (現状調査 5 件が残して
> いた課題は各 Phase で解消)。

- **CTFE の残りは無し** — 次にやるなら「コンパイル時のアロケーションと
  可変ヒープ」(`Const` の表現から作り直す、非目標) か、配列長での
  `const fn` 引数の非リテラル緩和 (`[i64; double(N)]`、fold の
  all-literal 規則を長さ式だけ緩める — 現状は理由つきエラー)。

### デバッグ・観測性 (DEBUG-OBS)

> 2026-08-26 に backtrace / 行番号 / ファイル名を 4 実行系で実際に叩いて
> 洗い出した節。設計は [`DEBUG_OBSERVABILITY.md`](DEBUG_OBSERVABILITY.md)
> (現状調査 9 件 + 論点 6 + Phase D0〜D6)。
> **D0〜D6 はすべて landing 済み** — 目標文言は D0 で固定され、現状は
> `compiler/tests/consistency/diagnostics.rs` に pin されている。
> backtrace の穴は塞がり、位置はどのファイルのものかを持ち、5 実行系
> すべてが panic の位置と backtrace を stderr に同じ書式で出し、
> 実行時の失敗は `--format=json` にも載り、無限再帰と stdlib の
> 範囲外はホストではなく toylang の言葉で落ちる。**5 レーンは pin した
> 全プログラムで完全一致**しており、両方向 pin の
> `assert_diagnostic_report` は現在どこからも呼ばれていない。

- **DEBUG-OBS D4 の残: panic に到達しえない関数のフレームを積まない** —
  **実測の結論: 作らない** (2026-08-27)。guard 除去が効くのは panic に
  到達しない関数だが、そこは debug/release 差 2% で、90% が出る fib の
  ような深い再帰は u64 減算 guard のせいで panic に到達し、かつ再帰な
  ので深さカウンタのために結局積む必要がある。効く場面が無いので
  条件 (「実際に効く場面を踏んでから」) が満たされないまま記録に留める。

### 型システム (NEW-TYPE-SYSTEM)

- **COMPOUND-BLOCK-DROP-TIMING: compound を作るブロックの束縛の drop 時期** —
  `val o: Option<u64> = if c { val t = H{..}  Some(1) } else { None }` の
  `t` を tree-walker はブロックの終わりで、compiled レーンは外側の
  スコープの終わりで drop する (回数は一致、`Drop` が出力すると順序が
  割れる)。ブロックで drop すると `val f = File::open(p)?` の desugar
  (`{ val t = ..  match t { Ok(v) => v, .. } }`) で持ち主 `t` がブロック
  より先に死ぬので、「ブロックの末尾から外へ出る束縛」を move_check が
  移動として扱う (`?` の別名規則と合わせる) のが先。
- **LEND-FREEING-CALLEE: 解放・再確保するがしまわない受け手に渡した値が漏れる** ★ —
  受け手は値渡しの仮引数を drop しないので、`fn grow(s: String) -> u64
  { s.push(100u8)  s.len() }` のように所有物を触る (realloc する) だけで
  しまいも返しもしない受け手に渡すと、誰も解放しない (3 レーンとも
  `leaks (1 sites, 6 bytes)`)。貸し出しにはできない (呼び出し側の持つ
  旧バッファは解放済み)。受け手の側で「移されなかった仮引数を出口で
  drop する」のが筋だが、`Box::new(v)` のように生メモリへ書いてしまう
  受け手は move_check 上は移動に見えないので、そのまま drop すると二重
  解放になる。その経路の扱いを決めてから。
- **Trait 拡張** ★★★ (大規模、ロードマップ)
  - **A3: trait inheritance (`trait B: A`)** — 中。super trait 経由で `A` の method を `B` impl からも要求。
  - **A4: associated types (`trait Iterator { type Item }`)** — 中〜大。
  - **A5-P3-interp: interpreter 側 JIT の `dyn Trait`** ★ — `ScalarTy::from_type_decl` が `TypeDecl::Dyn` で `None` を返し silent fallback。correctness 問題はなく、compiler 側 JIT が実用的な高速化を担うので優先度は低い。
  - **A5-P4: `Box<dyn Trait>`** — owned trait object + `Vec<Box<dyn Trait>>`。**前提**: `Box<T>` 自体が未実装。
  - **A5 残作業** — `&dyn Trait` の return / struct field 位置 (REF-Stage-2 の escape rule が阻む)、`dyn A + B`、`dyn Iterator<T>`、generic trait の default body 内での `T` 参照。
- **CLOSURE-CAPTURE の残: E4 / E5** ★ — 設計は
  [`CLOSURE_CAPTURE.md`](CLOSURE_CAPTURE.md)。**E0〜E3 + E6 は landing 済み**
  (escape しない closure は捕捉した束縛を共有し、escape するものはコピーを
  持って書き込みが `E0021`)。残りは (a) **E4: HOF / escape 越しの可変捕捉** —
  寿命の判断が要るので「実プログラムで踏んでから」、(b) **E5: compiled
  レーンの compound capture** — 診断は直した (capture の話だと分かる文言に
  なった) が、env に compound を載せるのは未着手。interpreter は動く。
- **const generics** ★ — `struct Array<T, const N: usize>`。大規模。

### 構文糖衣の候補 (NEW-FEATURES、未着手)

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
- **TRY-OPERAND-GAP: `?` が binary operand / 条件 / tail 位置で
  desugar されない** ★ — 2026-08-30 に `??` (NULL-COALESCE) の
  実装中に実測: `(r? == 1u64)` や `if r? { .. }` は `visit_try` が
  trait 既定 (`Unknown`) を返すため**型検査が黙って Unknown を返し**、
  個所によっては「expected bool, but got Unknown」のような本質でない
  エラーになる。val rhs は動く (`visit_expr` の intercept が効く)。
  `??` が取った解決策 — direct `accept_expr` dispatch の位置では
  型だけ付けて pool 書き換えを post-pass (`apply_null_coalesce_rewrites`)
  に回す — を `?` にもそのまま適用できる (`check_expr_located` と
  `visit_binary` の operand 経路に intercept を足す形)。

以下は **`poc/logsearch` (18,000 行) を書いて出てきた穴**で、
2026-09-23 に 3 レーン (`--all-backends`) で「本当に無い」ことを
確かめてから登録した。各項目の実測値と現物の引用は同 POC の
[`RUNTIME_GAPS.md`](../poc/logsearch/design-docs/RUNTIME_GAPS.md) §G19
にあり、ここには二重に置かない。

- **ENUM-STRUCT-VARIANT-PRINT: struct variant の表示にフィールド名を**
  ★ — `println(R::A { x: 1u64, y: 2u64 })` は位置の形 `R::A(1, 2)` で
  出る (struct variant は型検査器が tuple variant に書き換えるため、
  表示器は名前を知らない)。NEWTYPE が「書いた形で出す」のと同じく
  名前つきで出すには、NEWTYPE が手を入れた 2 つの表示器
  (`interpreter/src/object.rs::to_display_string` /
  `compiler_lower/src/print.rs`) に `field_names` を渡す必要がある
  (`--api` の宣言の描画は対応済み)。踏んでから。
- **ENUM-TUPLE-SUBPATTERN-AOT: enum variant の中の tuple パターン** ★ —
  `match o { Option::Some((a, b)) => .., Option::None => .. }` が AOT で
  ``compiler MVP only supports `Name`, `_`, literal, and nested
  `EnumVariant` sub-patterns inside enum variants, got a tuple pattern``
  (interpreter は通る)。tuple パターン自体は PATTERN-STRUCT のときに
  scrutinee 直下では lowering 対応したが、variant の payload の中は
  未対応。struct パターンも同じ位置で落ちるか要確認。2026-09-25 に
  README の例を書いていて踏んだ。
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

### インクリメンタルコンパイル

- **AOT の中間オブジェクト (分離コンパイル) は見送り** — 検討の記録は
  [`SEPARATE_COMPILATION.md`](SEPARATE_COMPILATION.md)。削れるのは stdlib の
  固定費 ~6-7ms/回で、同じプログラムの実 `cc` リンクが 55ms。着手条件
  (lowering が全体の 30% 超) と最小設計も同文書に記載。

- **INCREMENTAL-COMPILATION の残** — Phase 1〜5 は完了 (設計と実測は [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md))。統合パスの削減
  (placeholder 2 パス + HashMap → 1 パス + オフセット演算、シンボル翻訳キャッシュ)
  は **2026-08-18 に landing** (integrate 本体 ~43% 削減)。残るのは
  (a) **preparse の deserialize ~1.5ms** (16 ファイルの並列 read + bincode。
  bundle 化 = 1 ファイルにすると invalidation が全モジュール単位になるので
  見送り)、(b) per-module IR compilation + IR linker (warm 19ms のうち ~4ms
  しか狙えないので保留 — 着手するなら、大きめの実プログラムで lowering が
  支配的になることを**再測定してから**)。

### テスト・ドキュメント

- **BUILD-PERF** — **ビルドはテスト実行より桁で高い**。クリーンな target で、`interpreter/src/lib.rs` を 1 行触ってからの再ビルドが **2.35s**、テストファイル 1 個なら **1.08s**、クリーンからのフルビルド (テストターゲット全部) が **23.7s** — 対して全 1999 テストの実行が 7.5s (2026-08-19 実測、20 コア)。ここは 2026-08-19 に一度片付けたので、**残っているのは運用の話**:
  - **target を肥大させないこと — 実測 49x で、他のどの施策より大きい** ★★★ — 同じ「lib を触って再ビルド」が、**95GB / 1,248,912 ファイル**まで育った target の上では **1m55s**、`cargo clean` 直後の 1.7GB / 7,729 ファイルでは **2.35s**。消えた 125 万ファイルの 99% は過去のビルドの残骸。理由は cargo が rustc に `-L dependency=target/debug/deps` を渡すことで、**リンカが毎回 125 万エントリのディレクトリを走査する**。「user 45s に対し sys 6分」という異常な比率の正体がこれで、リンカが遅いのではなくディレクトリが大きすぎた。**古い成果物を定期的に GC すること** — **運用セットアップ済み (2026-08-20)**: `cargo-sweep` を導入し、`cargo sweep --time 30` (世代 GC) を CLAUDE.md に明記。この状態に戻ると下の施策は全部誤差に埋もれる。
  - **テストバイナリは 1 クレート 1 本** (2026-08-19 landing、73 → 12) — cargo は `tests/*.rs` を **1 ファイル 1 バイナリ**でリンクするので、64 ファイルは ~27MB の実行ファイルを 64 回リンクすることを意味していた (各々が frontend / interpreter / cranelift を静的に抱える)。`autotests = false` + `[[test]]` 1 個 + `#[path]` でモジュール取り込み。ファイルは 1 つも移動していない。**クリーン比較で フルビルド 37.5s → 23.7s / CPU 8m45s → 3m13s、lib 変更ループ 4.33s → 2.35s**。代償はテストファイル 1 個の編集が 0.88s → 1.08s (クレートの suite 全体が再コンパイルされる) と、テスト名にファイル名が前置されること。
  - **third-party の opt-level は 0、ただし cranelift だけ 2** (2026-08-19) — 全 deps を 3 で焼くのはビルド時間の払い損だった。**テスト実行が速さを感じる dep は cranelift だけ** (各テストが小さなプログラムを JIT / AOT する) なので、そこだけ残した。**テスト実行は劣化していない** (7.5s)。綴りに 2 つ罠があり、**どちらも間違えても cargo はエラーを出さない**: キーは `overrides` ではなく **`package`** (`overrides` は 1.41 以前の名前で、`unused manifest key` として黙って無視される)、そして **`cranelift` 単体は umbrella crate にしか当たらない** (実体は `cranelift-codegen` 以下 12 crate なので個別に列挙する。一致しない package spec は警告なしで「オーバーライド無し」になる)。
  - **測って外れた仮説を 2 つ記録しておく**: (1) **デバッグ情報の削減は効かない** — `[profile.dev]` / `[profile.test]` に `debug = "line-tables-only"` を入れて 2m06s (対照 1m55s)、改善ゼロ。27MB の中身はデバッグ情報ではなく cranelift のコード。(2) **リンカ差し替え (lld) と Spotlight 除外は、上を片付けた後では測る意味がない** — 絶対値が 1〜2 秒台まで落ちているので削り代が残っていない。target が肥大していた頃の「リンクが遅い」という観察は、リンカの速度ではなくディレクトリ規模の問題だった。

- **TEST-PERF** — ワークスペース全体で **~6.5s** (2026-08-19 実測、20 コア、warm、`cargo nextest run`、1999 テスト。AOT demand-driven lowering で 7.8s → ~6.5s)。**この suite は wall ではなく CPU 律速**: テスト時間の総和 (149.4s CPU は demand-driven lowering 前の値) / 20 コア が下限で、実測 wall はそこに張り付いている。**ビルド時間は別問題で、そちらの方が大きい — BUILD-PERF を見ること**。最長の単一テストも 2.11s (`example_consistency` shard_8) なので critical path 律速でもない。したがって**並列度を上げる策は効かず、効くのは CPU そのものを減らす策だけ。換算レートは 20:1** (CPU を 20s 削って wall 1s)。

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

    **測って分かった否定的な結果を 3 つ記録しておく**: (1) **free function の body は既に user 分しか検査していない** (`take(user_func_count)`) ので「stdlib 本体を型検査しない」で削れるのは impl block の 2.5ms だけ。しかも**型検査器は body を書き換える** (`?` の desugar、`Display` の `to_str` 挿入) ので、stdlib の body を検査しないと**書き換え前の AST がバックエンドに流れる** — 今の stdlib は `?` も補間も使っていないので通ってしまい、使った日に壊れる罠になる。(2) `remap_symbol` の memo 化 (module symbol → main symbol を Vec でキャッシュ) は**効果ゼロ**だった。integrate の時間は文字列ハッシュではなく AST を pool に複製する作業そのもの。(3) **「型検査済み core をプロセス内で使い回す」は unit テストには効かない** — nextest は 1 テスト 1 プロセスなので、そもそもプロセス内に 2 回目の呼び出しが無い。
    したがって残る手は (a) stdlib を使わないテストを `test_program_no_core` に寄せる (実測: `test_program` を no-core にすると interpreter の 879 テスト中 **797 が通り**、その binary は 2.3s → 1.3s。ただし stdlib 同居時の回帰を見なくなる = coverage を実際に落とす)、(b) **プロセスを跨いで**型検査済み core を再利用する (INCREMENTAL-COMPILATION 側の仕事。`File` が `Rc` を持つので素朴な in-memory memo 化はできない — 別スレッドから clone すると refcount が壊れる)、(c) 1 プロセスで core を複数回ロードしている `consistency` を直す — **解消 (2026-08-20)**: 4 レーンが 1 フロントエンドパスを共有するようになり、AOT / JIT レーンが毎回 core をロードし直す重複が無くなった (consistency -27% CPU)。
  - **プロセス起動が ~5ms × 1999 ≈ 10s CPU (約 7%)** ★ — 起動フロアの実測は空 core dir の trivial 実行 6.1ms。nextest は 1 テスト 1 プロセス。テストを機能別に束ねれば減るが、失敗の切り分けと引き換え。
  - ~~`serial_test` (`oop_tests.rs`) の並列化~~ — **効果ゼロと分かったので却下 (2026-08-18)**。`#[serial]` が付いているのは 8 テストで合計 **0.193s CPU (suite の 0.12%)**、1 本 18〜34ms と既に起動フロア。しかも `serial_test` のロックはプロセスローカルなので、**nextest では各テストが別プロセスに散る = 元から直列化していない**。
- **65. frontend リファクタリング** — (a)〜(g) は完了。残: doc コメント拡充、プロパティベーステスト追加。
- **property test の generator が仕様と drift しないか** — `valid_identifier()` は lexer に問い合わせる形にした (2026-08-10)。他の generator (リテラル / 演算子) はまだ手書きなので、同種の drift が起きうる。
- **26. ドキュメント整備** — 残: API リファレンス、advanced topics。

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

- **`remap_statement` 249 行** ★ — `Stmt` の variant ごとにフィールドを
  1 つずつ写す構造コピーで、分岐ロジックではない。コレクションの
  remap ヘルパ化は済み。これ以上分けても行が移るだけ。

- **`execute_builtin_method` の引数チェック 4 箇所** ★ —
  `evaluate_builtin_call` は `expect_args` に寄せたが、str メソッド側は
  文言が別系統 (`"concat(str) takes exactly one string argument"`)。
  揃えるとユーザ向けメッセージが変わるので手を付けていない。

> リファクタ時の等価性の確かめ方は
> [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md) の D8 にある。

## 検討中の機能

* **明示 import (MODULE-IMPORTS)** — stdlib も
  `import std.hex` を書かないと使えない形にする提案。
  [`MODULE_IMPORTS.md`](MODULE_IMPORTS.md)。**D1 の alias 束縛だけ
  2026-09-05 に landing** (`import a.b as h`)。残り (可視性の規則 P1 /
  遅延読み込み P2 / 型の名前空間化 P3) は未着手。BARE-NAME-COLLISION /
  TYPE-NAME-COLLISION を「規則」で消し (今の rank は同 root の衝突を
  消せない)、`pub` を実効化し、hello world の **145ms → 5.7ms**
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
  契約、#8〜#11 — #8 は §5-1 の parser の穴に塞がれている) /
  E (`is_sorted` helper、#12)。D (擬似 invariant) は B に吸収されるので
  不採用、要素値の契約は generic `T` に `eq` を要求するので不採用。
* FFI — P1 (静的 FFI、`from`/`as`) 完了 (2026-08-16、[`FFI_PLAN.md`](FFI_PLAN.md))。
  P2 (動的ロード / dlopen builtin) は未着手
* AOT ランタイムの Rust 化 — R0+R1 完了、R2 (extern 一般化 = FFI_PLAN P1)
  も完了 (2026-08-16、[`RUNTIME_PORT.md`](RUNTIME_PORT.md))。R3 (str 系の
  toylang 化) と R4 (f64 整形等) は計測で中止条件に該当 (interpreter
  ~20 倍〜~1000 倍遅延) し、Layer 1 に残すのが確定。R4 の byte 一致
  テスト固定のみ実施済み。
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
* 並行性 (CONCURRENCY) ★★★ — **設計は決まり、A1 は landing 済み**
  ([`CONCURRENCY.md`](CONCURRENCY.md) §5、2026-09-20)。入れたのは
  `parallel for` の**意味論**で、実行は 4 レーンとも逐次 — 答えを
  先に固定したので、並列化は答えを変えられない最適化になる。
  **残りは A2**: `toylang_rt` に pthread、shadow stack の per-thread
  化 (今は `static mut`)、AOT / JIT が本文を関数に切り出して分割実行、
  逐次との一致を consistency で縛る。`Send` 相当の判定は A の形では
  要らない (捕捉はスカラーと窓だけ) ので、`spawn` / チャネルに進む
  ときに初めて決める。
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
* ~~ネットワーク IO (NET)~~ — **N0〜N5 すべて完了 (2026-09-01)**。
  TCP client / server、poller、UDP、名前解決が 4 レーンで動く。
  以下は着手時の記録。**並行性を待たずにサーバが書ける形**。
  nonblocking socket + epoll/kqueue は単一スレッドで完結するので、
  CONCURRENCY (`Send` 相当の判定が要る) の前に landing できる。設計は
  [`NETWORK_IO.md`](NETWORK_IO.md) (socket ラッパー + `#[cfg_attr(path)]
  mod sys` によるコンパイル時切り替え + ABI probe テスト + errno の OS 差表)
  と [`EVENT_POLLING.md`](EVENT_POLLING.md) (epoll/kqueue の統一形、
  決定 6 件)。着手前に効く前提が 2 つある:
  * ~~**CONV-SPAN**~~ — **2026-08-31 に landing** (完了済み節)。
    `Span` を受け口にする API が書けるようになった
  * **EXTERN-BUF** — extern 境界がバッファを運べない (interpreter の
    registry `fn(&[Value])` がヒープに触れない)。`send` / `recv` に要る。
    compiled レーンは `ptr` 引数を既に通すので tree-walker だけの作業。
    **コピー API ではなく借用 API にする** — `HeapManager::memory` は
    連続した `Vec<u8>` で `get_memory_slice_mut` (囲む allocation に
    対して境界検査済み) が既にあるので、`recv(2)` に toylang の
    バッファを直接渡せる。**全レーンでバイト列のコピーが 0 回**になる
  * **確保しない受信ループのための stdlib** — `Span::slice(offset, len)`
    (部分窓、アプリ側 zero-copy の本体)、`Vec::with_capacity(n)`、
    `Vec::set_size(n)` (recv が埋めたバイト数を Vec に知らせる口)。
    いずれも現状の `core/std` に無く、無いとバッファの使い回しが
    書けない。net の extern を `never_allocates` で宣言すると、
    **確保しないことが E0016 で検査される**
  * **`compiler/build.rs` の `rerun-if-changed` が `lib.rs` 単体** —
    `toylang_rt` をモジュール分割すると AOT の staticlib だけ古いまま
    残る。分割と同じコミットでディレクトリ監視に変える

* **ビルドコマンド `toy`** — [`BUILD_TOOL.md`](BUILD_TOOL.md)。
  B0〜B4 は landing 済み (完了済み節)。残る B5 (マニフェストと依存) は
  依存が来るまで作らない。
* **テストの道具とライブラリ** — [`TEST_TOOL.md`](TEST_TOOL.md)。
  T0〜T5 は landing 済み (完了済み節)。残る T4 後半 (`--backend all`
  = レーン間の食い違い報告) は未実装節の TEST-PARALLEL P6 で追う。
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
- 合計 **2601 テスト** (100% 成功、2026-08-31 時点)。
- 内訳: interpreter unit + integration、frontend unit、compiler e2e + consistency。後者は interpreter / JIT / AOT の 3 経路一致を保証する。
- テスト実行はワークスペース全体で **~12s** (2026-08-31 実測、warm、
  nextest の既定 profile 出力)。2026-08-19 頃の ~6.5s からはテスト数の
  増加 (1999 → 2601) と stdlib の肥大 (整合性レーンの core ロード) 分。
  内訳と削り代は TEST-PERF、ビルド時間は BUILD-PERF。
  `compiler/build.rs` が `toylang_rt` を rustc で
  staticlib pre-build し、リンク結果は `TOY_LINK_CACHE_DIR` で
  content-addressed にキャッシュされる (キャッシュが効くにはコード生成が
  決定的である必要がある — `compiler/tests/reproducible_build.rs` が pin)。

### 既知の不具合

- **compound を返す method 呼び出しから束縛したローカルに drop glue が付く**
  ★★ — `val e: T = v.get(i)` の `e` は要素の**別名**なのに所有として
  扱われ、スコープを抜けるときにコンテナの持つバッファを解放する。
  `__builtin_ptr_read::<T>` から束縛すれば付かないので、stdlib の
  generic なコンテナ実装はそちらで書いている (`Vec::contains` /
  `sort` / `sort_by`)。**回避策は分かっているが、規律であって検査では
  ない** — ユーザが `Vec<String>` に対して同じ形を書けば同じことが
  起きる。根本は「別名を返す API と所有を返す API を型で区別できない」
  ことなので、`Vec::get` が `&T` を返せるようになる (借用の一般化) か、
  drop flag が入るまで残る。2026-09-05 の STRING-NO-DROP で踏んだ。

**直った項目をこの節に段落で残さないこと** — 常時読まれるファイルが
changelog になる。過去にここへ挙がった 3 件 (f64 の print が 3 バックエンドで
食い違う / `if` の条件が型検査されない / MATCH-STRUCT-ARM) はいずれも解消し、
経緯は git log と完了済み節にある。`__getitem__` の 2 件
(`&self` 受理 / generic 戻り型置換) も 2026-08-30 に解消
(POINTER P2、完了済み節)。

- **COMPOUND-FIELD-ARG: compound な *フィールド* を引数に渡せない** ★★ —
  束縛・リテラル・呼び出し結果は通る (COMPOUND-ARG-CALL、2026-09-02) が、
  フィールドパスだけが残っている。

  ```rust
  struct Holder { buf: Vec<u8> }
  fn count(b: &Vec<u8>) -> u64 { b.size() }
  impl Holder {
      fn via_field(&self) -> u64 { count(self.buf) }   # compile error
  }
  ```

  AOT が `call argument produced no value` (method 呼び出しなら
  `method argument produced no value`) で拒否する。**診断が規則を
  名指ししていない** ので、原因に辿り着くのに二分探索が要る。
  回避策は窓を渡すこと (`self.buf.as_span()` を `val` に束縛して
  `Span<u8>` で渡す) で、`poc/logsearch` の行分割はこの形にしてある。

- **BARE-NAME-COLLISION: auto-load される全モジュールが bare 名の
  1 つの名前空間を共有する** ★★ — `poc/logsearch` に
  `fn is_digit(b: u8) -> bool` (**`pub` でない**) を書いたら
  `[E0010] ambiguous module path \`is_digit\`: it matches
  logsearch::record::is_digit and std::json::is_digit` で落ちた。
  `max_depth` も `std::json::max_depth` と衝突した。診断は明快で
  「ファイル名を変えろ」と言うが、**stdlib が 1 つ関数を増やすたびに、
  ユーザのプライベート関数が壊れうる**ということでもある。

  **rank (BUILD-TOOL B0) が消したのはこの形の半分だけ** — 別 root の
  衝突は後の root が勝つが、**同じ root の 2 モジュール**は同 rank
  なので今も落ちる (`src/a.t` と `src/b.t` がそれぞれ private な
  `fn helper` を持ち、**各自が自分のを呼んでいる**だけで
  `ambiguous call` になる)。stdlib 内の `encode` / `decode`
  (base64 / hex、どちらも `pub`) も同じ形。

  **シンボル名のマングリングは解決にならない** (2026-09-05 検討)。
  定義側の一意化は既に済んでいて (`toy_std_math__add` /
  `function_index` の (path, rank) エントリ)、残っているのは
  **使用側の解決規則**だから。要るのは規則 2 つ:
  (1) **`pub` を実効化する** — 非 `pub` の module 関数は自分の module
  からの呼び出しにしか候補にならない (今は修飾付きでも呼べてしまう。
  `check_function_access` の `is_same_module_access` が
  `true` 固定)。stdlib の非 pub 40 本を他 module から呼んでいる箇所は
  **0 件**なので、この変更単体では stdlib は壊れない。
  (2) **呼び出し元 module を rank より先に優先する**。
  難所は「呼び出し元 module」を 3 レーンに届けること — 型検査器は
  `type_check(func)`、lowering は関数ごとのループで既に持っているが、
  **tree-walker は実行中の関数の module を持っていない**
  (`CallFrame` に足すか、型検査時に解決結果を AST に焼く)。

- **QUALIFIER-BARE-FALLBACK: 修飾付き呼び出しが別モジュールの関数に
  落ちる** ★★ — `hex::abs(-3i64)` が `std::math::abs` を呼んで `3` を
  返す。`m::f(...)` の `m` が既知のモジュールで `f` を輸出していないとき、
  `method_call.rs` の module 呼び出し経路が**bare 名で引き直す**
  (「module integration が修飾なしで入れていた頃の流れ」のための
  フォールバック)。**誤答であって拒否ではない** — 修飾は「このモジュールの」
  と言っているのに別のモジュールのものが返る。MODULE-IMPORTS D1 の
  alias (`import std.hex as math` → `math::abs` が `std.math::abs` に
  当たる) で表面化したが、alias 以前からある。消すと、修飾付きで書いて
  bare に救われていた既存コードが落ちうるので、影響範囲を測ってから。
  2026-09-05。

- **TYPE-NAME-COLLISION: struct / enum 名には曖昧性検査すら無い** ★★ —
  関数には (module path, rank) の候補集合があるが、型は
  `register_struct` / `enum_definitions` が**名前だけのマップ**なので、
  2 つの module が同じ `struct Item` を宣言すると**後勝ちで黙って
  上書き**される。壊れるのは負けた側の module で、しかも診断は
  そちらのフィールドを名指す:

  ```
  src/a.t: struct Item { v: u64 }        # 1 フィールド
  src/b.t: struct Item { v: u64, w: u64 }
  -> [E0010] Error in imported module `a`: Missing required field 'w'
     in struct 'Item'
  ```

  BARE-NAME-COLLISION より重い (あちらは曖昧だと**言う**)。
  2026-09-05、BARE-NAME-COLLISION の検討中に見つけた。

- **ENUM-CALL-VALUE-COUNT (internal error)** ★ —
  `internal error: enum call returned 19 value(s), expected 15`。
  自由関数が `&mut` の compound を 2 つと `&` を 1 つ取り `u64` を返す形で
  出た (`poc/logsearch` のアーカイブ書き出し)。**最小再現は取れていない** —
  7 フィールド struct を `Result` で返す形は通る。`&mut` を 1 つに
  減らしたら消えたので、writeback の leaf 数の数え方が疑わしい。
  internal error なのでユーザ側に直し方の手掛かりが無い。

- **E0014-WRONG-FILE: モジュールの中の `[E0014]` が入口ファイルを指す** ★★ —
  所有権検査の診断だけがモジュール帰属を持っていない。**ファイル名は
  入口のもの、行番号はモジュールのもの**という混ざり方をするので、
  入口が短ければ `<line not available>`、長ければ**無関係な行**を
  指すスニペットが出る。

  ```rust
  # probe.t (モジュール根の下)
  pub fn collect_names(flag: bool) -> Vec<String> {
      var out: Vec<String> = Vec::new()
      val s = String::from_str("hello")
      if flag { out.push(s) }        # ← ここが [E0014]
      out
  }
  # entry.t (9 行)  ->  Error at entry.t:10:1 / `<line not available>`
  ```

  同じ状況で `[E0010]` は
  `Error in imported module \`logsearch::query\` (line 680 of that
  module)` と正しく出るので、**帰属を持つ診断と持たない診断がある**。
  2026-09-05、`String` が所有型になった直後の `poc/logsearch` で
  5 件同時に出て、全部が入口 `main.t` の無関係な行を指した
  (実際の出所は `query.t` と `logdir.t`)。原因の特定が grep 頼みになる。

  **帰属を持つ診断も、行番号は合っていない。** 同じ日に
  `[E0001]` で踏んだ最小再現:

  ```rust
  # probe.t -- 1..9 行はコメント
  pub fn sink(out: &mut ByteWriter) { out.put_u8(1u8) }   # 10 行目
  # 11..13 もコメント
  pub fn hop(out: &mut ByteWriter) {                      # 14 行目
      sink(out)                                           # 15 行目 <- ここ
  }
  ```

  報告は `Error in imported module \`logsearch::probe\` (line 6 of
  that module)`。**6 行目はコメント**である。別の例では実際の 120 行目が
  「line 17」と出た (ずれ幅は一定ではない)。ファイル名は正しいので、
  モジュール内の位置を数える側が間違っている。

### パーサーの既知制限事項
- **行末の識別子と、次の行頭の `(` が改行を跨いで呼び出しになる** —
  セミコロンが無いので、

  ```rust
  fn f(mask: u64) -> u64 {
      val a: u64 = 1000000u64
      val b: u64 = 3u64
      val prod = a * b          # 行末が識別子
      (prod >> 17u64) & mask    # 行頭が `(`
  }
  ```

  は `b(prod >> 17u64)` と読まれ、`[E0003] Function 'b' not found`
  (指すのは **1 行上**) で落ちる。JavaScript の ASI と同じ罠。
  行末がリテラルなら起きず (`val b = a + 0u64` の次行に `(...)` は通る)、
  行頭を `val` にすれば起きない。`[` で始めた場合は繋がらず
  `parse error: BracketClose` になる。**規則としては妥当だが、診断が
  原因の行を指さず「関数が無い」と言う**ので、知らないと最小化に
  10 回近くかかる (2026-09-04、`poc/logsearch` の圧縮で実際にそうなった)。
  診断だけでも「行を跨いだ呼び出しとして解釈した」と言えれば実害は消える。
- bare `self` 非対応 — `self: Self` / `&self` / `&mut self` のいずれかを書く。
- `else if` 非対応 — `elif` を使う。
- `val` はキーワードなのでパラメータ名に使えない。
- 関数のネスト定義 (`fn` の中の `fn`) は不可 — closure (`fn(x: T) -> R { ... }`) を使う。
- デフォルト引数 / 名前付き引数は不可 (`f(a: u64, b: u64 = 1u64)` / `f(a: 1u64)`)。導入予定も無い。
- `extern fn` の generic params は parser では受理されるが、JIT / AOT が per-instance シンボル名を持たないため interpreter でのみ動く (`#195b`)。
- `package` 宣言 / `import` path のセグメントに primitive type キーワード (`i64` / `f64` / ...) は使えない (`core/std/str.t` が `package` 宣言を省いているのはこのため)。
- 関数名に primitive type キーワードは使えない (`fn f64(...)` は `expected function name`)。
- 3-part qualified call (`std::math::abs(x)`) は **MODULE-SYSTEM P3 で解決済み** (2026-09-21)。パーサが全セグメントを記録し、型検査と lowering が同じものを修飾子として使う。実在しないパスは `[E0030]`。
