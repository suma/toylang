# EFFECT SYSTEM — 「その関数は何をしうるか」を 1 か所で答える

> 実装: [`frontend/src/type_checker/effects.rs`](../frontend/src/type_checker/effects.rs)
> 利用側: `alloc_check.rs` / `const_fn_check.rs` / `contract_purity.rs`
> 問い合わせ: `cargo run -q -p interpreter -- --effects <file>`

## なぜ入れたか

到達可能性で「その宣言を認めてよいか」を決める検査が 3 つあった。

| 検査 | 拒否するもの | 診断 |
|---|---|---|
| `never_allocates` | アロケータに届く | `E0016` |
| `const fn` | コンパイル中にできないことに届く | `E0017` |
| 契約の純粋性 | 述語が質問以上のことをする | `E0018` (警告) |

3 つは**歩き方**を `reachability.rs` で共有していたが、**知識**は共有して
いなかった。それぞれが「どの builtin が禁止か」の手書きテーブルを持って
いたので、`BuiltinFunction` に variant を足したら 3 つのテーブルを訪ね直す
必要があり、忘れてもコンパイルは通る (レビューでしか捕まらない)。

統合後は「builtin が何をするか」の表が 1 つ ([`builtin_effect`]) あり、
各検査は結果集合に対する**マスク**でしかない:

```text
never_allocates   ALLOC
const fn          ALLOC | FREE | RAW_READ | RAW_WRITE | ALLOC_CTX | IO
契約の純粋性       ALLOC | FREE | RAW_WRITE | IO
```

**新しい検査はマスク 1 行、新しい builtin は表の 1 行**になった。

## 格子

7 つ。「概念として別」ではなく「どれかの検査が隣と区別する必要がある」
ものだけを variant にした。

| Effect | 何 | 由来する builtin |
|---|---|---|
| `Alloc` | 現在の allocator にメモリを要求 | `heap_alloc` / `heap_realloc` |
| `Free` | 返却 | `heap_free` / `heap_realloc` |
| `RawRead` | 生ポインタを読む・問い合わせる | `ptr_read` / `ptr_eq` / `null_ptr` / `ptr_offset` / `str_to_ptr` / `str_from_bytes` / `ptr_is_null` |
| `RawWrite` | 生ポインタ・ランタイム状態に書く | `ptr_write` / `mem_copy` / `mem_move` / `mem_set` / `record_allocator_layout` |
| `AllocCtx` | 実行中のプログラムについて訊く | `current_allocator` / `default_allocator` / アロケーションカウンタ / `backtrace` |
| `Io` | 出力する | `print` / `println` |
| `Panic` | 実行を中断しうる | `panic` / `assert` |

`Panic` はどの検査もマスクしていない。`const fn` が**意図的に許して
いる**(fold 中に到達したらコンパイルエラーにするのが狙い) という決定を、
コメントではなく型に置くために variant にしてある。後述の `never_panics`
の土台でもある。

`str` は `Alloc` ではない。カウンタが数えないもの (ランタイムが
`str` を保持するために使うメモリ) はここでも数えない —
MEM-COUNTER-INTERP-DRIFT で固定した定義をそのまま使う。

### 意図的に変えた 1 点

`__builtin_backtrace()` は旧 `const fn` の sink 表に**入っていなかった**
ので、`const fn` から呼べていた。fold 中に呼べばコンパイラ自身のスタックが
返る (誰も望まない答え) ので `AllocCtx` に入れた。契約の純粋性は
`AllocCtx` をマスクしないので、`ensures` からの backtrace 読みは従来どおり
通る。

## 追えないもの

closure 値・`dyn` レシーバ・`extern fn` への呼び出しは追跡できない。
そういうノードには**全エフェクトを与える**。これが安全側で、
「追えない呼び出しは綺麗だと仮定する」ことだけが検査全体を無意味にする
唯一の間違いだから。

`extern fn` は宣言でエフェクトを 1 つ取り戻せる:

```rust
never_allocates extern fn getchar() -> i32 from "c"   # ALLOC だけ落ちる
```

`const fn` の側から見るとこれは救いにならない (`Free` 以下が残るので
依然として拒否される)。作者の「純粋だ」という言葉は、コンパイラが
その関数を**呼べる**理由にはならないので、これは正しい非対称性。

## 到達可能性であって伝播属性ではない

D の `@nogc` は callee 全部に属性を要求する。toylang の stdlib は toylang
で書かれているので、それは method 単位の注釈行脚を意味する。呼び出し
グラフを歩けば注釈は要らない — `Vec::push` が allocating なのは
`__builtin_heap_realloc` に届くからで、誰かが印を付けたからではない。
代償は「診断が行ではなく**経路**を出す必要がある」ことで、
各エフェクトが [`Witness`] (sink か opaque か + 経路) を持つのはそのため。

## 計算の性質

- **遅延**: root が訊くまで何も計算しない。`never_allocates` も `const fn`
  も契約も無いプログラムは 1 歩も歩かない。
- **メモ化**: ノード単位。経路はノードからの相対なので**呼び出し元に
  依存せず**、呼び出しグラフのダイヤモンドは 1 回で済む
  (旧実装は「clean な答えだけ」しかキャッシュできなかった)。
- **再帰**: 進行中のノードに再入したら空集合を返す (循環は新しい到達コードを
  足さない)。その最中に計算された結果は不完全なのでメモ化しない
  (`cycles` カウンタで検出)。
- **method の同名衝突**: 呼び出し側が所有型を知っていればその型の body
  だけ、知らなければ同名の body 全部を畳む。旧実装は「名前が clean」を
  キャッシュしていたので、別の型の同名 dirty method を取りこぼしえた。

## `--effects`

```console
$ cargo run -q -p interpreter -- --effects prog.t
add             pure
double          pure
shout           io
grow            alloc
main            alloc, io
Counter::bump   pure
Counter::shout  io
```

`--api` が「何が呼べて、どういう形か」を答えるのに対し、これは
「計算以外に何をするか」を**本文を読まずに**答える。`--api` と違って
**型検査を通す** — 答えが型に依存するから (レシーバが `str` なら `concat`
はランタイム実装、`String` なら stdlib のコードでアロケートする)。
そのため引数は「実行できるプログラム」であって任意のモジュールではない。

一覧は**エントリファイルの宣言だけ**。stdlib は同じプールに統合される
ので、全部出すと訊いた答えが埋まる (free function は統合前の
`user_func_count`、method は `SourceLocation.file == FileId::ENTRY` で
切り分ける)。

## この先

- **`never_panics`** — `Panic` マスクの検査 1 つ。CONTRACT-ELISION が
  `requires` から trap guard を落とせた関数は「panic しない」と言えるので、
  両者が噛み合う。
- **並行性の `Send` 相当** — todo.md の CONCURRENCY が「これを決めるまで
  着手できない」と言っている判定。エフェクト + リージョン (下記) で
  「region を跨がない値 = 送れる値」と定義するのが本命。
- **リージョン型** — `with allocator = arena { ... }` から出るポインタを
  静的に止める。`Alloc` / `Free` がどの region に対するものかを持てば、
  エフェクトの側から支えられる。
