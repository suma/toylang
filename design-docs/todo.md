# TODO - Interpreter Improvements

## 完了済み ✅
> **この節は 1 行サマリだけを持つ。** 実装の経緯・測定値・ファイルパス・
> テスト数は git log のコミットメッセージにある。フェーズ設計は
> [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md) /
> [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md) /
> [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md) /
> [`FEATURE_NOTES.md`](FEATURE_NOTES.md) を参照。
> ここを段落で埋めると、常時読まれるファイルが changelog になる。

### 2026-09-03
- **SIMD-INTRINSIC-3 — `__simd_bitmask` / `__simd_swizzle` /
  `__simd_bitcast` (intrinsic 13 → 16)** — 「どの lane か」を聞く手段が
  抜けていたので `__simd_any` で当たった後は 1 バイトずつ舐め直していた。
  `String::contains` / `Split` を bitmask 版に置換して**密ケース 2.0x**
  (1.26s → 0.62s)、疎ケースは変化なし。設計と実測は [`SIMD.md`](SIMD.md)
  の「Phase 3 の追補」。
- **STDLIB-SERIALIZE S1/S3/S4/S5 — JSON (`core/std/json.t`)** —
  設計は [`STDLIB_SERIALIZE.md`](STDLIB_SERIALIZE.md)。`JsonWriter`
  (木を作らない writer) / 平坦な `Json` の木 / RFC 8259 の部分集合の
  reader。**設計から 3 点ずらした**: (1) 木は `enum Json` ではなく
  `Vec<JsonNode>` の pre-order 平坦表現 — enum 版は tree-walker では
  動くが compiled lane では関数に渡せない (`cannot lower parameter`)、
  (2) `parse(s) -> Result<Json, JsonError>` ではなく
  `doc.read(s) -> Option<u64>` + `doc.error()` — レジスタ予算
  (RESULT-COMPOUND-WRITEBACK)、(3) 深さ上限は 128 ではなく 32 —
  ホストの stack が 40〜60 で尽きるので、それ以上は発火しない上限。
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
- **MODULE-CONST: モジュールの top-level `const` がどこからも見えない** ★★ —
  `pub const X: u32 = 1u32` を `core/std/poll.t` に書いても、他モジュール
  からは修飾しても `import` しても見えず、**同じモジュールの関数本体
  からも見えない**。統合が module の const を運んでいない
  (`interpreter/src/lib.rs` の const 登録はユーザプログラムの
  `program.consts` だけを見る)。2026-09-01 に NET N3 で踏み、
  `poll.t` は `pub fn interest_read() -> u32 { 1u32 }` の形で回避した。
  stdlib に定数を置く自然な方法が無いので、次に定数が要る機能でまた踏む。

- **NUM-W-SHIFT: narrow int の `<<` / `>>` が型検査で拒否される** ★ —
  `u8 << u8` も `u8 << u64` も「incompatible types u8 and u64」。
  `&` / `|` / `^` は全幅で動く (2026-09-01 に tree-walker 側を修正) ので
  shift だけが取り残されている。2026-09-01 に NET N3 のテストで踏んだ。

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
- **CLAUDE.md の `--message-format=short` 案内を `--diagnostics=json`
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
  `core/std/ord.t` に `trait Ord { fn lt(self: Self, other: Self) -> bool }`、
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
- **`Display` trait — 型が自分の見せ方を決める** — `core/std/display.t`。
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
## 未実装 📋

- **MODULE-FN-REF-ARG: module の自由関数が `&compound` を取り scalar を
  返すと lowering が落ちる** — `hex::probe(v: &Vec<u8>) -> u64` を
  `hex::probe(&v)` で呼ぶと `call argument produced no value`
  (JIT / AOT)。**同じシグネチャでも戻りが compound なら通る**
  (`hex::encode(&Vec<u8>) -> String` は動く) し、**同一ファイルの
  自由関数**や **method** なら scalar 戻りでも通る。拒否なので誤答は
  出ない。`core/std/json.t` の `skip_ws` / `byte_at` / `hex4` /
  `word_at` はこれを避けて method にしてある。
- **RESULT-COMPOUND-WRITEBACK: `&mut self` の method が
  `Result<u64, E>` を返せない** — `Vec` を 1 つ持つ struct (4 leaf) の
  `&mut self` method が `Result<u64, JsonError>` を返すと
  `Too many return values to fit in registers`。writeback の leaf と
  戻りの leaf が同じ予算を食う。**`Option<u64>` なら通る** (6 leaf 側の
  struct でも通った)。`Result<Struct, E>` を返す自由関数も同じ壁
  (`json::parse(s) -> Result<Json, JsonError>` が書けない理由)。
  cranelift の `StructReturn` を使えば外せるはずの制限。
- **AOT-MATCH-STR-ARM-BLOCK: `str` を返す match の arm がブロックだと
  AOT が拒否する** — 最小再現:
  ```
  fn pick(o: Option<u64>) -> str {
      match o {
          Option::Some(v) => { val a: String = String::from_str("one")
                               val s: str = a.to_str()
                               s }
          Option::None => { val b: String = String::from_str("none")
                            val t: str = b.to_str()
                            t }
      }
  }
  ```
  → `function falls through without producing a value of the declared
  return type`。**片方の arm がリテラル (`"err"`) なら通る**ので、
  両 arm が実行時に組み立てた `str` を返す形が落ちる。
  **拒否であって誤答ではない** (コンパイル時に止まる)。回避は
  arm で `println` する / `String` を返して呼び出し側で `to_str`。
  `core/std/hex.t` / `base64.t` のテストはこの形を避けている。
- **STDLIB-FN-SHADOWED-BY-USER-FN: user の自由関数が stdlib module の
  同名関数を内側から置き換える** — `fn pad2_field(n: u64) -> u64` を
  書いたプログラムが `println(dt)` で落ちる
  (`[E0001] expected u64, but got u32 ... 'pad2_field'`、**行は
  `core/std/time.t` の中**を指す)。stdlib の body の裸の呼び出しが
  user の関数に解決されるため。**黙って壊れる形もある**: `fn at(...)`
  を書くと `log::at` の body が誤った引数型で検査され、
  `Display for Level` の書き換えが起きず `INFO` の代わりに
  `Level::Info` が出た (診断は出ない)。回避は module 側が自分の名前を
  `log::` で修飾すること (それでも body の検査は直らないので、
  `log.t` は Display 依存も外した)。直す場所は名前解決 —
  module の body から見える自由関数は、その module のものを先に
  探すべき。FREE-FN-VS-ASSOC-COLLISION (型の method vs 自由関数、
  解決済み) と同じ族の残り。

> 完了した項目はここに残さない (完了済み節と二重になる)。優先度は
> ★ = あると良い / ★★ = 効果が見えている / ★★★ = ロードマップ級。

### バックエンドのカバレッジ

- **FN-NAME-AS-VALUE: トップレベル関数の名前を `fn` 値として渡せない**
  ★ — `fn twice(x: u64) -> u64` があっても `apply(twice, 21u64)` は
  `[E0001] expected fn (u64) -> u64, but got u64` (名前が値の位置で
  u64 と型付けされている)。closure literal を `val` に束縛すれば通るので
  回避はできるが、`Vec::sort_by(cmp)` のような comparator API は毎回
  これを踏む。2026-09-03 に COLLECTIONS C3 で発見。


- **compound 要素の drop glue が `f32` leaf で落ちる** ★ —
  `Vec<S>` / `SoaVec<S>` の `S` に `f32` フィールドがあると
  `drop glue: unsupported leaf type F32` で compiled レーンが拒否する
  (`drop_glue.rs::drop_glue_signature`)。`f64` / narrow int は通るので
  抜けているのは f32 だけ。SIMD-F32 が後から入った順序の名残
  (2026-08-30 に DOD Phase 2 の作業中に発見)
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

- **REF-REBORROW: `&mut` 引数を再帰呼び出しにそのまま渡せない** ★ —
  `fn insert(arena: &mut Vec<Node>, ..)` の中で `insert(arena, ..)` は
  `expected &mut Vec<Node>, but got Vec<Node>`。`insert(&mut arena, ..)`
  と書き直せば 3 レーンで通る。木やグラフを書き換える関数は必ずこの形に
  なるので毎回踏む。値渡しに逃げると今度は `[E0014] 分岐の中では move
  できない` (MOVE-CONDITIONAL) に当たるので、回避は再借用一択
- **UNIT-STRUCT-FIELD: struct のフィールドに `()` を書けない** ★ —
  `struct S { u: () }` が `[E0004] Unsupported operation 'field type in
  struct 'S'' for type ()`。`()` は戻り型 / `val` 注釈 / 引数 / 型引数
  (`Result<(), E>` / `Vec<()>`) / リテラル (`val x = ()`) では**すべて
  書ける**ので、フィールドだけが穴。門番は `struct_literal.rs` の
  フィールド型検査 1 箇所で、layout 側は `flatten_compound_leaf_types`
  が `Type::Unit` に leaf 0 個を与える扱いを既に持っている
  (UNIT-TYPE-ARG がそれで通った)。実用途 (phantom フィールド、
  `T = ()` の実体化) を踏んでから
- **COMPOUND-BLOCK-RHS の残: method call の枝** ★ —
  `val p = if c { x.twin() } else { .. }` は
  `detect_struct_result` が method の戻り型を安く引けないので検出されず、
  従来どおり「compound-returning method を式の位置で使えない、`val` で
  束縛せよ」というエラーになる。誘導が具体的なので実害は小さい。
  (2026-09-01 に **match の arm 束縛** と **`return` する arm** は解消。
  残っているのは method call の枝だけ。)

- **COMPOUND-GENERIC-INSTANCE: `match` から generic struct を取り出すと
  注釈が要る** ★ — `val out: Span<u8> = match w { Option::Some(s) => s, .. }`。
  `BranchShape::Produces` が base name (`Span`) しか運ばないので、
  型引数は注釈から取るしかない。`struct_of_arm_binding` は payload の
  **具体的な `StructId` を既に持っている**ので、`BranchShape` を
  そこまで運べるようにすれば注釈は要らなくなる。既存の
  `val v: Vec<u8> = Vec::new()` と同じ規則なので実害は小さい。
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

- **SIMD-F32 の残** ★ — (a) **format spec 未対応**: `{x:.2}` の
  formattable 集合に f32 を入れるには `toy_format_f32` が要る
  (promote して f64 で整形すると最下位桁が変わるので専用ヘルパ)。
  (b) **f32 の math intrinsics** (`math::sqrt_f32` 等) は未提供 —
  `x as f64 → math::sqrt → as f32` の橋渡しで代替できるが、
  cranelift の `sqrt` は F32 を受けるので unary op 経路の supplied helper
  を増やせば direct にできる。(c) **`f32` の `min` / `max` 演算子**は
  f64 同様 AOT 未対応 (cranelift の fmin / fmax で入れられる)。
- **NUM-W-AOT-pack Phase 3** ★ — compound element 配列の tighter layout (`[PackedRgba; N]` が 4 バイト相当のところ 32 バイト消費)。メモリ効率のみで機能差はない。
- **195b. `extern fn` の monomorph 化** ★ — generic extern は現状 interpreter の type-erased registry でのみ動く。JIT / AOT には mangled symbol の emit と Rust 側実装の登録が要る。実需要なし。
- **185残. 3+ part qualified call** ★ — `std::math::abs(x)`。現状は `import std.math` 経由のみ (parser が last 名だけを採る)。auto-load があるので実害は限定的。
- **121-Phase-B-rest-leftover** ★ — `AllocatorBinding::Generic/Local/Ambient` の lower 配線 (perf のみ、観察可能な振る舞い変化なし)、`__builtin_default_allocator()` の戻り型を `u64` にして生比較を許すかの API 判断。
- **REF-Stage-2 (residual)** ★ — compound `&mut T` の真の pointer-passing、`&T` compound の RefScalar 経路活用。どちらも copy 削減で機能差はない。

### 標準ライブラリ・実行環境 (STDLIB-RUNTIME)

> 2026-08-16 に「言語機能として何が残っているか」を実際に叩いて洗い出した結果。
> RUNTIME-IO (Result を返す IO) は 2026-08-29 に landing 済み (完了済み節)。
> 実プログラムを書けなくしている残りは下記。
> 俯瞰と優先順位 ([`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md)、2026-08-30 実測)。

> 2026-09-02 に `core/std/` 全 28 ファイルを**分野で**棚卸しした。以下は
> 分野ごとの空白で、`RUNTIME_LIBRARY.md` の P1〜P4 に対応する
> (下の 2 件も分野で言えば TEXT と IO に属する)。

- **HOF-RETURN-UNKNOWN: 関数を値として渡す形が使えない** ★ —
  (a) **名前つき関数を値として渡せない** — `fn run(f: fn () -> ())` に
  `run(work)` と書くと `[E0001] expected fn () -> (), but got ()`
  (名前が関数の**戻り型**に解決される)。(b) closure リテラルを渡すと
  通るが、**その呼び出しの戻り型が `Unknown` になる**ので
  `val b: Bench = bench(3u64, fn() -> () { })` の `b.iters` が
  `field access for type Unknown`。2026-09-03 の STDLIB-TIME で
  `bench(iters, f)` を書こうとして踏み、**`bench` を入れずに
  `Stopwatch` だけにした**
- **STRING-NO-DROP: `String` に `impl Drop` が無い** ★ — `Vec<T>` は
  持っているのに `String` は持たないので、**すべての `String` が
  バッファを漏らす** (`--profile=mem` の `leaks` に `string.t` の
  `from_str` / `push` が並ぶ)。2026-09-03 に `Clone` の leak 検査で
  気づいた。足すのは 4 行だが、`Drop` を持つ型は container に入れると
  **move する** (`[E0014]`) ので、既存の `String` を受け渡すコードが
  move 検査に引っかかりうる。影響範囲を測ってから
- **並行性 (CONCURRENCY)** は分野としては stdlib だが、本体が move /
  Drop モデルとの接合なので「検討中の機能」節に置いてある (★★★)。
  RUNTIME_LIBRARY P3 も「設計文書を別に取ってから着手」と同じ判断

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

- **CHAR-LITERAL-MATCH: narrow int の match scrutinee** ★ —
  `match byte { 'h' => ... }` は書けない (`match scrutinee must be an
  enum, struct, primitive (bool / i64 / u64 / str), or tuple, got
  UInt8`)。pattern 側は char リテラルを受けるようになったので、残るのは
  scrutinee の型リスト + 網羅性 + 4 バックエンドの lowering。
  byte 走査を書いていて実際に困ってから。

- **NUM-W-ENUMERATION の残り** ★ — 整数型の列挙が
  **42 ファイル 625 箇所**に散っている。型を 1 つ足すコストがそのまま
  42 ファイル。`TypeDecl::is_numeric` / `is_integer` /
  `is_signed_integer` と `ScalarTy` の同名メソッドが「再列挙しない」
  入口なので、残りの match arm もそこへ寄せられる。
  **「primitive レシーバ → 対象型名」の 6 コピーは 2026-09-01 に決着**
  (`TypeDecl::PRIMITIVE_IMPL_TARGETS` が正本、各層は射影)。
  残っているのは (a) 演算・キャスト・codegen 側の幅ごとの match arm、
  (b) `TypeDecl` と `ScalarTy` と IR `Type` の相互変換 3 組。
  **この列挙が産んだバグは通算 6 件** — 単項 `-` / `~` が narrow を
  拒否 (2026-08-25)、レシーバ表の narrow 欠落と `f32` 欠落
  (2026-08-31)、tree-walker のビット演算 (2026-09-01)、
  interpreter JIT の符号判定 3 箇所と `MIN / -1` guard の即値
  (2026-09-01)。次に踏んだら (a) から着手する

- **NARROW-UNSIGNED-SUB: `u8` / `u16` / `u32` の減算は
  アンダーフローで trap せず wrap する** ★ — RUNTIME-TRAP-NARROW の
  作業中に実測 (2026-08-31、4 レーン一致で `5u8 - 10u8` == `251u8`)。
  trap するのは `u64` だけで、0 除算と `MIN / -1` は全幅で効いている。
  `docs/language.md` の Runtime traps 表は元から `u64` としか書いて
  いないので**嘘ではない**が、幅で意味論が割れているのが意図なのかは
  決まっていない。揃えるなら (a) narrow unsigned も trap させる
  (guard が 3 幅分増える、RUNTIME-TRAP の「wrap した答えが誤解を招く」
  基準は narrow でも同じ) か、(b) 現状を明示的な決定として書く。
  **今は (b) の書き方にしてある** — 「トラップでないもの」の一覧に
  narrow unsigned の減算を足し、`core/std/checked.t` の doc comment が
  「narrow 幅では `checked_sub` だけが報告する」と説明する

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
> 実行時の失敗は `--diagnostics=json` にも載り、無限再帰と stdlib の
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

- **MOVE-CONDITIONAL: 分岐 / ループからの移動** ★ — 現状は E0014 で拒否。
  許すには実行時 drop flag (Rust と同じ) が要る。実プログラムで踏んだら着手。
- **MOVE-ALIAS-GAP: `val b = a` 後の `a`** ★ — alias なので `b` を移動しても
  `a` の読みは検出されない。DROP-GLUE の冪等 free + never-reuse ヒープが
  二重 drop を無害化しているので、これは診断の網羅性の問題 (読み放題)。
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

- **raw / multi-line string literal** ★ — `r"..."` と `"""..."""`。
  lexer 拡張のみで AST / 型検査 / バックエンドは無変更。設計メモ:
  (a) `r"..."` は**生文字列** — エスケープ処理も文字列補間もしない
  (正規表現・パス向け。`{{` / `}}` の二重化も不要になる)、
  改行を含めてよい。内容に `"` を含めるには Rust の `r#"..."#` 形が
  理想だが MVP は省略可 (現行の通常文字列も `\"` を越えられないので
  制限は同じ)。(b) `"""..."""` は**複数行** — エスケープ処理は通常
  文字列と同じで、閉じは `"""` のみ。改行をそのまま値に含めるので、
  lexer が `line_count` を数えることを忘れないこと (現行の単一行
  ルール `"[^"]*"` は改行を含まない)。both とも `Kind::String` に落とす
  ので parser 以降は何も変わらない。
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
- **ENUM-DISCRIMINANT: enum の明示 discriminant + `as u64`** ★ —
  `enum Color { Red = 1u64, Green = 4u64 }` (unit variant のみに `= <整数
  リテラル>` を許す。data-carrying variant は不可、未指定は Rust 規約
  (先頭 0、以降 +1) で自動採番、重複は型エラー)。**layout / match
  dispatch は現状のまま** (tag = variant index) — discriminant は
  `as u64` の射影としてだけ存在する。`as u64` は**型検査器が match に
  書き換える** (`E::A as u64` ならリテラルへ直接畳み、式なら
  `match e { E::A => 1u64, ... }` の網羅 match — バックエンドは砂糖を
  見ない)。`as` は全 variant が unit の enum に限る (Rust と同じ)。
  [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) の P4 (FFI P2 で C に
  enum を渡す / bitflags) の前提になる。
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

### リファクタリングの残り (2026-08-25 の一巡で見送った分)

> 巨大関数と重複の一巡は済んでいる。以下は「踏んでから」で保留した分。

- **`remap_statement` 249 行** ★ — `Stmt` の variant ごとにフィールドを
  1 つずつ写す構造コピーで、分岐ロジックではない。コレクションの
  remap ヘルパ化は済み。これ以上分けても行が移るだけ。

- **`execute_builtin_method` の引数チェック 4 箇所** ★ —
  `evaluate_builtin_call` は `expect_args` に寄せたが、str メソッド側は
  文言が別系統 (`"concat(str) takes exactly one string argument"`)。
  揃えるとユーザ向けメッセージが変わるので手を付けていない。

- **AOT 実行ファイルの非再現性** ★ — オブジェクトと CLIF は再現的
  (`reproducible_build.rs` が両方 pin)。実行ファイルだけ run ごとに
  変わる。macOS リンカの LC_UUID あたりと踏んでいるが未調査。
  リンクキャッシュは content-addressed なので実害は出ていない。

> リファクタ時の等価性の確かめ方は
> [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md) の D8 にある。

## 検討中の機能

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
* 並行性 (CONCURRENCY) ★★★ — **言語コアで唯一の完全な空白** (2026-08-20 まで
  仕様にもこの todo にも項目が無かった)。最小形は `spawn(fn () -> ())` + join ハンドル +
  チャネル。ランタイム側の下地は一部ある (`toylang_rt` の出力シンクは
  `pthread_key` TLS で per-thread 化済み)。ただし本体は「共有可変性を現行の
  move / Drop モデルにどう載せるか」で、`Send` 相当の判定を決めるまで
  着手できない。設計フェーズを別に取る前提。
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
  `__simd_swizzle` / `__simd_bitcast` も入った (2026-09-03)。残りは
  (a0) **`__simd_shuffle`** — 唯一の未実装 intrinsic。定数マスクを
  「配列リテラルを型検査器が畳んで synthetic な `u64` 2 語にする」形で
  受ける設計まで書いてある。hex / base64 の SIMD 化は `swizzle` と
  `shuffle` が**対**で要るので、着手するならセット、
  (a) **stdlib の残り kernel**
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

**直った項目をこの節に段落で残さないこと** — 常時読まれるファイルが
changelog になる。過去にここへ挙がった 3 件 (f64 の print が 3 バックエンドで
食い違う / `if` の条件が型検査されない / MATCH-STRUCT-ARM) はいずれも解消し、
経緯は git log と完了済み節にある。`__getitem__` の 2 件
(`&self` 受理 / generic 戻り型置換) も 2026-08-30 に解消
(POINTER P2、完了済み節)。

- **METHOD-ARG-UNCHECKED: method 呼び出しの引数が型検査されない** ★★★ —
  自由関数は正しく落ちる (`fn take(s: &String)` に `take("ab")` で
  `[E0001] Type mismatch: expected &String, but got str`) のに、
  **method は型も個数も検査していない**。2026-09-03 実測、いずれも
  interpreter が値を出して完走する:

  | 書いたもの | 宣言 | 出た値 |
  |---|---|---|
  | `w.take_u64(true)` | `fn take_u64(&self, n: u64)` | `1` |
  | `w.take_str(42u64)` | `fn take_str(&self, s: str)` | `0` |
  | `w.eat(7u64)` | `fn eat(&mut self, other: &W)` | `7` (body の `other.n` が通る) |
  | `w.two(1u64)` | `fn two(&self, a: u64, b: u64)` | `1` (引数不足が通る) |

  stdlib でも同じで、**`s.push_str("ab")` が通る** (`push_str` は
  `&String` を取るので `String::from_str("ab")` が正しい)。結果は
  レーンで割れる: interpreter は**黙って何もしない** (`len()` は 0 のまま)、
  AOT は cranelift の verifier がクラッシュする
  (`mismatched argument count ...: got 5, expected 8` /
  `arg 1 (v14) has type i8, expected i64`)。`Vec<u8>::push_str` も同型。
  **String に文字列を足すという最初に書く形が黙って壊れ**、compiled
  レーンの診断は internal error の文言なので原因に辿り着けない。
  直し方は自由関数側の検査 (`[E0001]` を出している経路) を method 呼び出しにも
  通すこと。材料は揃っている — `declared_method_param_types` は既にあり、
  CHAR-LITERAL-GENERIC-ARG がレシーバの型引数で置換してから hint に使っている。
  2026-09-03 に `logsearch/` の設計 (ログ検索サービスのモデルケース) で発見。

### パーサーの既知制限事項
- bare `self` 非対応 — `self: Self` / `&self` / `&mut self` のいずれかを書く。
- `else if` 非対応 — `elif` を使う。
- `val` はキーワードなのでパラメータ名に使えない。
- 関数のネスト定義 (`fn` の中の `fn`) は不可 — closure (`fn(x: T) -> R { ... }`) を使う。
- デフォルト引数 / 名前付き引数は不可 (`f(a: u64, b: u64 = 1u64)` / `f(a: 1u64)`)。導入予定も無い。
- `extern fn` の generic params は parser では受理されるが、JIT / AOT が per-instance シンボル名を持たないため interpreter でのみ動く (`#195b`)。
- `package` 宣言 / `import` path のセグメントに primitive type キーワード (`i64` / `f64` / ...) は使えない (`core/std/i64.t` が `package` 宣言を省いているのはこのため)。
- 関数名に primitive type キーワードは使えない (`fn f64(...)` は `expected function name`)。
- 3-part qualified call (`std::math::abs(x)`) は parser が **last 名だけを採る**。名前が一意なら結果的に解決するが、意図した経路ではない (`#185残`)。
