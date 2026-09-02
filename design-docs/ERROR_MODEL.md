# ERROR_MODEL — stdlib のエラー型の統一規約

> **状態: E0〜E5 landing 済み (2026-09-03)。** 以下は決定の記録として残す。
> 先送りにした項目は末尾の「先送り」節にある。

> 対象: `core/std/io.t` / `parse.t` / `net.t` / `poll.t` / `result.t` /
> `option.t` と、その裏の `toylang_rt` / `extern_io.rs` / `extern_net.rs`
> 状態の正本: [`todo.md`](todo.md) の **STDLIB-ERROR-MODEL**
> 俯瞰と優先順位: [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md)
> 実測: 2026-09-02 (この文書の観測はすべてこの日に取った)

## Status snapshot

| 失敗の運び方 | 今どうなっているか |
|---|---|
| `IoError` (`io.t:63`) | 7 variant。`Display` あり。status コードを `io_error_from_status` で復号 |
| `ParseError` (`parse.t:43`) | 3 variant。`Display` あり |
| `NetError` (`net.t:113`) | 17 variant。`Display` あり。うち 3 つは**失敗ではない** (`WouldBlock` / `InProgress` / `Interrupted`) |
| `Poller` (`poll.t`) | 独自の型を持たず `NetError` を借りている |
| `panic` / trap | 捕捉不可。`main` の外へ出て exit 1 + backtrace |
| 確保の失敗 | **null `ptr` が返るだけ**。誰も検査していない (→ D5 で検査 + panic、`try_reserve` を追加) |
| 共通の `Error` trait | 無い。`Display` (`to_str`) が事実上の共通面 |
| `From` 連鎖 | stdlib 全体で `impl From<str> for String` の **1 つだけ** |
| discard の検査 | `[E0025]` (2026-09-02 landing) |

## なぜ今これを決めるか

**4 つ目のエラー型を足す前だから**、ではない。**3 つで既に壊れているから**。
「規約が無い」は将来の負債の話に聞こえるが、実際に測ったら**今日の実行結果
が間違っている**箇所が 3 つ出てきた (F1 / F3 / F6)。とくに F1 は
「アプリが自分のエラー型を持つ」という、この分野でいちばん普通の書き方が
**型検査を通らない**という話で、規約以前に機構が足りていない。

## 測ったこと (2026-09-02)

以下はすべて `--all-backends` か `--effects` で実際に叩いた結果。

### F1. 1 つの型は `From` impl を 1 つしか持てない ★★

```rust
enum E { A(u64), B(bool) }
impl From<u64> for E { fn from(value: u64) -> Self { E::A(value) } }
impl From<bool> for E { fn from(value: bool) -> Self { E::B(value) } }

val a: E = E::from(5u64)
#          ^ [E0001] Type mismatch: expected bool, but got u64
#            (in argument of associated function 'E::from')
```

**後に書いた impl が前を上書きする。** 機構は
`context.rs:564` の `register_struct_method` で、method spec のキーが
`(型, method 名, **impl 対象の型引数**)` — `impl From<u64> for E` と
`impl From<bool> for E` はどちらも対象型引数が `[]` なので同じ枠に入り、
`impl_block.rs:31` の登録で 2 つ目が 1 つ目を置き換える。
**trait 側の型引数 (`From<u64>` の `u64`) はキーに入っていない。**

これがそのままアプリの集約エラー型を殺す:

```rust
enum AppError { Io(IoError), Parse(ParseError), Config(str) }
impl From<IoError> for AppError { ... }
impl From<ParseError> for AppError { ... }   # ← 前者が消える

fn load(path: str) -> Result<u64, AppError> {
    val text: str = io::read_file(path)?     # [E0001] expected ParseError, but got IoError
    val n: u64 = parse::to_u64(text)?
    Result::Ok(n)
}
```

`?` のクロス変換 (TRY-ERR-RETYPE) の機構自体は正しく、
`type_implements_from` (`utility.rs:473`) は**複数 impl を正しく走査する**。
落ちるのはその後の `E::from(...)` 呼び出しの解決。つまり**「From を張るか」
を議論する以前に、張れない**。stdlib に `From` impl が 1 つしか無いのは
方針ではなく、**2 つ目を書いた人がまだ居なかっただけ**。

### F2. 集約エラー型そのものは (From が 1 つなら) 3 レーンで動く

`enum AppError { Io(IoError), ... }` — **enum の payload に別の enum**、
`impl Display for AppError`、`?` による変換、`println(e)` の
`Display` dispatch。**すべて interpreter / JIT / AOT で一致**した。
`SysError::Errno(i32)` / `Ctx(str)` のような payload つき variant も同様。
つまり F1 を直せば、この分野の設計は**言語機能としては揃っている**。

### F3. 同じ失敗がバックエンドごとに別の名前を名乗る ★★

`/etc/hosts/nope.txt` (`/etc/hosts` はディレクトリではないので `ENOTDIR`)
への書き込みと読み込み:

```text
interpreter: "write error\nread error\n"
jit:         "read error\nread error\n"     ← 不一致
aot:         "read error\nread error\n"     ← 不一致
```

原因は**同じ語彙が 3 箇所に独立に書かれている**こと:

| 書き込みの経路 | 未知 errno で失敗したとき |
|---|---|
| interpreter `io_write_file` (str) | `IO_READ_ERROR` を `IO_WRITE_ERROR` に読み替える (`extern_io.rs:641`) |
| interpreter `io_write_file_bytes` (Span) | errno があれば `status_from_io_error` のまま → **ReadError** (`extern_io.rs:340`) |
| `toylang_rt::toy_io_write_file` (str) | `io_status_from_errno` のまま → **ReadError** (`lib.rs:3721`) |
| `toylang_rt::toy_io_write_file_bytes` (Span) | 同上 → **ReadError** (`lib.rs:2768`) |

**書き込み 1 つに経路が 4 本あり、一致しているのは 3 本だけ** —
しかも同じインタプリタの中の 2 本が食い違っている。
`io_status_from_errno` (`lib.rs:329`) は未知の errno をすべて
`IO_READ_ERROR` に畳むので、`ENOSPC` (ディスク満杯) で書き込みが失敗しても
`read error` と出る。

**`net` はこの問題を既に解いている。** `extern_net.rs:4` は
「**These forward to `toylang_rt` rather than reimplementing**」と書いて
あるとおり、interpreter が runtime の関数をそのまま呼ぶ。だから
`NetError` の語彙は原理的に割れない。**`io` と `parse` だけが再実装
している**。E0 はこの形を移すだけで、新しい発明は要らない。

### F4. `&dyn Error` は書けない。`<T: Trait>` は書ける

```rust
fn describe(e: &dyn Error) -> str { e.message() }
describe(&e)   # [E0001] expected &dyn Error, but got &MyErr
```

`dyn Trait` は struct レシーバのみで、**enum は載らない**。一方
`fn describe<T: Error>(e: T) -> str` は enum で 3 レーンとも通る。
「エラーを型消去して 1 本の経路で報告する」は今の言語では書けない。

ついでに: generic 関数の単相化は**束縛から型を取る**。
`report(E1::X)` は compiled lane で
``cannot infer type arguments for generic function `report` `` になり、
`val err = E1::X` に束縛してから `report(err)` なら通る。

### F5. `expect(msg)` が `msg` を捨てている

```rust
o.expect("the config must carry a port")
# → panic: Option::expect on None
```

`option.t:50` / `result.t:49` はどちらも引数を使わず固定文字列で panic する。
`docs/language.md:4478` は「**panics with the literal message**」と書いて
あるので、**文書と実装が食い違っている**。`expect` はユーザが文脈を付ける
唯一の口なので、これは規約以前の欠陥。

### F6. 確保の失敗は誰も見ていない ★

```rust
val fb = FixedBuffer::new(32u64)
val p: ptr = fb.alloc(64u64)     # quota 超過
__builtin_ptr_is_null(p)         # true
```

null が返る。そして `core/std/collections/vec.t` / `string.t` / `box.t` の
**どこにも `is_null` 検査は無い** — `Vec::push` が伸ばせなかったら
null に書きに行く。さらに `with allocator = fb { v.push(...) }` は
wrapper の quota を通らない (CLAUDE.md 既知) ので、64 要素を 32 バイトの
FixedBuffer に押し込んでも**成功して 64 と表示される**。
**「メモリが尽きた」が観測できない。**

### F7. `--effects` は失敗について何も言わない

```text
r_str      alloc, free, raw_read, raw_write, alloc_ctx, io, panic
r_enum     alloc, free, raw_read, raw_write, alloc_ctx, io, panic
o_plain    alloc, free, raw_read, raw_write, alloc_ctx, io, panic
unchecked  pure        # fn unchecked(a: u64, b: u64) -> u64 { a / b }
```

2 つ問題がある。

1. **payload つきの `Option` / `Result` を作る関数は effect 行が top に
   なる。** `Option::None` を返すだけの関数は `pure` なのに、
   `Option::Some(1u64)` を作った瞬間に全部載る。原因は未確認だが、
   generic enum の impl block にある closure を取る method
   (`map` / `unwrap_or_else`) が「追えない呼び出し」として top に飛ばして
   いる線が濃い。**エラーを返す関数はすべてこの形**なので、
   `--effects` は失敗を扱うコードについては現状ほぼ無情報。
2. **trap は `Panic` effect に入っていない。** `a / b` は 0 除算で
   panic しうるが `pure` と報告される。
   [`EFFECT_SYSTEM.md`](EFFECT_SYSTEM.md) が構想する `never_panics` は、
   この状態では「明示的な `panic` / `assert` を書いていない」しか
   保証しない。

### F8. `?` は `Option` と `Result` を跨がない

`Result<u64, str>` を返す関数の中で `find(x)?` (`find` は `Option<u64>`)
は `[E0001] expected Option<u64>, but got Result<u64, str>`。
`?` は**囲む関数の戻り型の家族**で desugar される。

### F9. 文の位置の `?` は desugar されない ★★

```rust
fn m1() -> Result<u64, E> {
    unit_fail(1u64)?        # 値を束縛しない `?`
    Result::Ok(7u64)
}
# 型検査は通る。実行すると:
# Internal error: evaluate: unexpected expr: Try { inner: ExprRef(13), ... }
```

**`val v = f()?` は動くが、`f()?` 単独は動かない。** 型検査器の書き換えは
「値として訪問された式」でしか起きず、文の位置の `Try` 節点が生のまま
バックエンドへ流れる。`Result<(), E>` でも `Result<T, E>` でも同じ。
**型検査は通るので、その行が実行されるまで分からない。**

`Result<(), E>` にはもう 1 つ穴がある:

```rust
val _ = unit_fail(1u64)?
# Internal error: Invalid cast from Tuple([]) to Unit
```

`?` の success arm が挿入する `as T` のキャストを `()` が通れない。
**つまり今日、`Result<(), E>` に `?` を書く方法は 1 つも無い。**

現に踏まれていないのは、既存の `?` の使用箇所 (example と stdlib の
doc コメント) が**すべて束縛形**だから。`Poller::register(fd, ...)?` は
書けそうに見えて書けない。**D5 の復帰の口 `v.try_reserve(n)?` が
まさにこの形** (文の位置 × `Result<(), E>`) なので、これは E5 の前提条件。

### F10. (周辺) `push_str("literal")` が型検査を通って実行時に壊れる

`push_str(&mut self, other: &String)` (`string.t:180`) に `str` リテラルを
渡すと型検査を通り、実行時に
`Internal error: Cannot access field on non-struct object: ConstString` で
落ちる。エラーメッセージを組み立てようとすると最初に踏む形なので
ここに記録するが、担当分野は **STDLIB-TEXT**。

## 既存の決定から引く制約

1. **例外は入れない** (`docs/language.md`)。失敗は戻り値で運ぶか、
   `panic` で止まるかの 2 択しかない。`try` / `catch` / `throw` は
   parser が受理しない。
2. **`Result` の discard は警告** (`[E0025]`)。「戻り値で運ぶ」の
   運用は既に見張られている。抜け道は `val _ignored = ...`。
3. **エラー enum は stack 上の tagged union**で allocator を持たない
   (`result.t` の冒頭)。エラー値を作るのに heap は要らない、が既定。
4. **compiled lane は式位置の compound 呼び出しを拒否する** —
   `match io::write_file(...)` は書けず `val w = ...` を挟む
   (F3 の再現でも踏んだ)。エラーを返す API の使用例はすべてこの形で書く。
5. **`Drop` を持つ値の move は検査される** (`[E0014]`)。エラー payload に
   `String` を入れると所有権が移るので、`Err` を返す経路で drop flag の
   要る分岐 (ループ内 move 等) が書けなくなる。
6. **status コードは OS 非依存の語彙**であることが `net` の設計の核
   (`lib.rs:261` のコメント)。errno はプラットフォーム側の表で畳む。

## 決めたこと

### D1. 失敗を 3 つに分ける。運び方は分類で決まり、モジュールで決まらない

| 分類 | 何 | 運び方 |
|---|---|---|
| **A: 呼び出し側のバグ** | 範囲外添字、空の `pop`、`unwrap` の失敗、契約違反、算術 trap | **`panic`**。捕捉不可 |
| **B: 世界の事情** | ファイルが無い、接続が切れた、入力が数でない | **`Result<T, E>`** |
| **C: 予算の枯渇** | 確保が失敗した | 既定は **`panic`**。復帰したい呼び出し側は `try_reserve` で**先に訊く** (D5) |

境界の判定は**「正しいプログラムでも起きうるか」**の一点。
`v.get(i)` の範囲外は呼び出し側が `i < v.size()` を確かめれば消せるので A。
`read_file` の `NotFound` は誰にも消せないので B。
既存の stdlib はこの線を既にほぼ守っている (`panic` は 22 箇所、すべて
A に該当) ので、これは**発見の追認であって新しい制約ではない**。

分類 A を `Result` に格上げしない理由: `Vec::get` が `Result` を返すと、
**正しいプログラムのすべての添字アクセスに `?` が生える**。B を `panic`
に格下げしない理由: 呼び出し側が方針を持ちうる失敗を勝手に殺すから。

**C だけが両側に足を掛ける** (D5)。既定は A と同じ「止まる」— `push` が
要素ごとに `Result` を返すと、正しいプログラムの全ループに分岐と `?` が
生えるから。一方この言語では予算つき allocator が設計の道具なので、
**復帰の口は要る** — それを要素ごとの `Result` ではなく
「確保する前にまとめて訊く」形 (`try_reserve`) で置く。

### D2. 共通の `Error` trait は置かない。`Display` を必須にする

エラー型に課す規約は 1 つだけ:

> **`Result` の `E` に置く型は `impl Display` を持つ。**

`Error` trait を作らない理由は F4 —— **enum は `dyn` に載らないので、
trait を作っても型消去には使えない**。使えるのは `<T: Error>` の
bound だけで、それは `<T: Display>` で足りる。`code()` や `kind()` を
足したくなったら、それは**その enum の inherent method**でよい
(`NetError::is_retryable` が D4 でまさにそれ)。

再検討の条件を先に書いておく: **`dyn Trait` が enum を受けるようになったら**
`trait Error { fn to_str(&self) -> str }` を置き直す価値が出る。
それまでは trait 1 つ分の間接を払う相手が居ない。

### D3. 語彙は 1 箇所に定義し、変種は「直し方」を名乗る

- **定義の場所は `toylang_rt` の const 群 1 箇所。** interpreter は
  `extern_net.rs` と同じく**そこへ forward する** (再実装しない)。
  `.t` 側の `*_from_status` は復号だけを担う。
- **変種は「読んだ人が次に何をするか」で切る。** `NotFound` と
  `PermissionDenied` は別 (作るか、権限を直すか)。`NameNotFound` と
  `HostUnreachable` が別なのも同じ理由 (`net.t:133` に明記されている)。
  errno 1 個 1 変種にはしない。
- **未知の errno は `Unknown`。隣の変種に化けさせない。** F3 の
  `read error` はこの規則の違反で、「ディスクが満杯」を「読み取り失敗」
  と報告している。**間違った具体名は `Unknown` より悪い** — 読んだ人が
  違う場所を直しに行く。
- **raw errno は語彙に載せない。** OS 非依存であることが語彙の存在理由
  (制約 6)。診断のために生の値が要るなら、`Unknown(i32)` に payload を
  足すのではなく `io::last_os_code() -> i32` を別に置く
  (payload 化は 3 レーンで動く (F2) が、`match` の全 arm に影響が出る
  うえ OS 依存が語彙の内側に入る)。**今は要らない**ので置かない。

### D4. 「失敗でないもの」は述語で見分けられるようにする

`NetError` の `WouldBlock` / `InProgress` / `Interrupted` は失敗ではない
(`net.t:114` / `lib.rs:270`)。にもかかわらず `Err` で返るので、
**`?` で上に投げると平常状態がアプリのエラーとして報告される。**

```rust
impl NetError {
    # Not a failure: the operation should simply be attempted again.
    pub fn is_retryable(&self) -> bool { ... }   # WouldBlock / Interrupted
    # Not a failure: a non-blocking connect is still under way.
    pub fn is_pending(&self) -> bool { ... }     # InProgress
}
```

規約: **`is_retryable()` が真の値を `?` で伝播させない。** 分岐して
`Poller` に戻る。これは型では強制できないので、
`interpreter/example/net_echo_server.t` を正しい形の見本として名指しする。

### D5. 確保の失敗は既定で panic する。復帰は「先に訊く」で書く

今 (F6) は null が黙って伝播し、書き込みで壊れる。**まず「失敗した」と
気づくこと**が要る。そのうえで運び方を決める。

決め:

- **確保しうる操作は失敗したら `panic` する。署名は変えない。**
  `push` / `push_str` / `push_char` / `extend_bytes` / `Box::new` /
  `with_capacity` / `Ptr::alloc`。メッセージは D7 の形で**要求バイト数を
  出す** (`"Vec::push: allocation failed (1024 bytes)"`) —— 予算を
  直すのに要る情報がそれ。
- **`push` は再確保する** (Rust の `Vec::push` と同じ、償却 O(1) の
  幾何級数成長 — `vec.t:127`)。だから失敗する確保の主な形は fresh alloc
  ではなく **realloc** で、実装規則が 2 つ出る:
  1. **検査は `self.data` に代入する前。** 今の `vec.t:131` /
     `string.t:86` は `self.data = __builtin_heap_realloc(...)` と
     直に代入しているので、失敗すると**元のポインタごと失う**
     (漏れ + 以後の null write)。realloc は失敗しても元のブロックを
     保つ — `FixedBuffer::realloc` は quota 超過を**下位の realloc を
     呼ぶ前に** null で返す (`allocator.t:154`)。**panic する時点で
     vector はまだ壊れていない**、が正しい形。
  2. **allocator 側の会計も同じ。** `Arena::realloc` は結果を無検査で
     追跡表に書き `bytes_used` を更新する (`allocator.t:279`) ので、
     失敗すると古い addr を忘れたまま使用量だけ増える。E5 はここも直す。
- **再確保はアドレスを動かす。** `as_span()` / `ps.field` の `Column` /
  `Ptr` の窓は `push` の後では無効 (`vec.t:121` に既出)。エラーモデルの
  外の話だが、ループの前に `try_reserve` を呼ぶ理由がもう 1 つある。
- **復帰したい呼び出し側は、確保する前に訊く。** 各コンテナに
  **`try_reserve(n) -> Result<(), AllocError>`** を 1 つ置く。予算に
  収まるかは**要素ごとにではなく、まとめて 1 回**分かる:

  ```rust
  with allocator = fb {
      var v: Vec<u64> = Vec::new()
      v.try_reserve(items.size())?      # 予算を訊く。失敗しうるのはここだけ
      for x in items { v.push(x) }      # 容量の範囲なので再確保しない = panic しない
  }
  ```

  保証は**容量の範囲まで**: 「今の `len` に加えて n 要素は再確保しない」
  であって、それを超えた `push` はまた再確保し、また panic しうる。
- **構築の時点で確保する形**には `try_with_capacity(n) -> Result<Self,
  AllocError>` を置く (まだ値が無いので `try_reserve` を呼べない)。
- **`try_push` は置かない。** 要素ごとに訊く形は `try_reserve` の劣化版で、
  消したかった per-element の分岐をそのまま戻す。後から足せるので、
  必要になってから (先送り)。
- **null 検査は必須** (F6)。panic するにもまず気づく必要がある。
  今は `Vec` / `String` / `Box` のどこにも `is_null` が無く、
  失敗すると null へ書きに行く。
- **`heap_alloc(0)` は全バックエンドで null を返す** — これは失敗では
  なく契約 (`ptr.t:32` / `ptr.t:65`)。素朴に `is_null(p)` を失敗と読むと
  **空の `Vec::new()` がすべて「確保失敗」になる**。検査は
  「要求バイト数 > 0 かつ null」でだけ失敗と読む。
- **型は `AllocError`** (`core/std/alloc.t`)。`try_*` が返す先。
  `Display` 必須 (D2)、`to_str` は `str` リテラル (D7)。変種は D3 の基準
  (直し方で切る) で 2 つ:

  ```rust
  pub enum AllocError {
      OutOfMemory,   # the allocator has no room left — raise the budget
      SizeOverflow,  # the requested byte count does not fit u64 — hold fewer
  }
  ```

- **既存の署名は 1 つも変わらない。** `.push` 系の 66 箇所は書き換え不要で、
  E5 は「`try_*` の追加 + null 検査」に縮む。

引き受けたトレードオフ:

- **`try_reserve` を呼び忘れた予算つきコードは落ちる。** 型はそれを
  強制しない。受け入れる理由は、`push` が `Result` を返す形の費用
  (全ループに分岐 + `?` + `Result` の構築) のほうが、実際に書くコードでは
  重いから。
- **復帰できないわけではない** — 復帰の口 (`try_reserve` /
  `try_with_capacity`) はあり、「復帰したいと言った人だけが書く」形にした。
  予算つき allocator が設計の道具である以上、この口は必須。

`with allocator = fb` が wrapper の quota を通らない件は allocator 側の
話なので [`ALLOCATOR_PLAN.md`](ALLOCATOR_PLAN.md) に属するが、
**F6 の「32 バイトの FixedBuffer に 64 要素が入ってしまう」は null 検査を
入れても直らない**ことはここに書いておく (そもそも null が返らないので
検査に掛からない)。

### D6. `From` はアプリが張る。stdlib は横に張らない

- **stdlib は `IoError` → `NetError` のような cross-module `From` を
  提供しない。** n 個のエラー型に n² 個の変換を置くことになり、しかも
  どの向きが正しいかはアプリの都合。
- **アプリの集約エラー型が `From` を張る**のが正規の形:

  ```rust
  enum AppError { Io(IoError), Parse(ParseError), Net(NetError) }
  impl From<IoError> for AppError { fn from(value: IoError) -> Self { AppError::Io(value) } }
  impl From<ParseError> for AppError { ... }
  impl From<NetError> for AppError { ... }
  ```

  これが **E1 (F1 の修正) が前提条件**である理由。今は 2 つ目の impl を
  書いた時点で 1 つ目が消える。
- stdlib が張ってよいのは**自分の型への `From`** だけ
  (`impl From<str> for String` が唯一の既存例)。

### D7. メッセージの規約

- **`Display::to_str` は文の断片**。小文字始まり、末尾にピリオドを打たない、
  `error:` の接頭辞を付けない (`"not found"` / `"would block"`)。
  呼び出し側が `"cannot open {path}: {e}"` の形に埋める。
- **`to_str` は `str` リテラルだけを返す** (確保しない)。既存の 3 型は
  すべてこの形。動的な文脈が要るなら **enum の payload に載せる**
  (F2 で 3 レーン動作を確認済み)。ただし `String` payload は制約 5 の
  move 検査に掛かるので、まず `str` で足りないかを疑う。
- **`panic` のメッセージは `Type::method <何が起きたか>`**
  (`"Vec::get index out of bounds"`)。既存 22 箇所がこの形。
- **`expect(msg)` は `msg` を使う** (E3)。

## フェーズ

小さい順。**おおむね独立に landing できる**が、依存が 2 つある —
D6 の書き方 (アプリの集約エラー型) が実際にできるのは E1 から、
D5 の復帰の書き方 (`v.try_reserve(n)?`) が実際にできるのは E2 から。

| # | 何 | なぜこの順 |
|---|---|---|
| **E0** | `io` / `parse` の status を `toylang_rt` へ forward し、語彙の定義を 1 箇所にする。未知 errno は `Unknown` に落とす (`ReadError` を止める)。`--all-backends` の consistency テストで固定 | **設計判断ゼロのバグ修正**。F3 は今日の実行結果が間違っている。`extern_net.rs` に手本がある |
| **E1** | `From` の複数 impl を通す。`register_struct_method` のキーに trait 側の型引数を含めるか、`from` の呼び出しを引数型で解決する | D6 の前提。**アプリが自分のエラー型を持てない**のがこの分野で一番大きな穴 |
| **E2** | 文の位置の `?` を desugar する + `Result<(), E>` の success arm から `as ()` のキャストを外す | **E5 の前提**。F9 は型検査を通って実行時に落ちるので、放置すると D5 の復帰の口 (`v.try_reserve(n)?` — 文の位置 × `Result<(), E>`) が書けない |
| **E3** | `expect(msg)` が `msg` を panic に渡す | 1 行。文書 (`language.md:4478`) が既にそう約束している |
| **E4** | 規約の明文化 (`docs/language.md` に「Error model」節) + `NetError::is_retryable` / `is_pending` + `interpreter/example/error_model.t` | D1〜D7 を読める形にする。example が 3 レーン consistency の検体になる |
| **E5** | コンテナの null 検査を**代入の前に**入れ (`heap_alloc(0)` は除外)、失敗したら要求バイト数つきで `panic`。allocator 側の会計も同じ。`AllocError` / `try_reserve` / `try_with_capacity` を足す | **既存の署名は変わらない** — 追加と検査だけ。F6 は今日メモリを壊しているので、大きさの割に効く。D5 |

**先送り** (別項目として立てる。この文書では決めない):

- `never_panics` と、**trap を `Panic` effect に入れるか** (F7-2)。
  入れないと「panic しない」の保証が薄い。EFFECT_SYSTEM 側の話。
- **`--effects` が `Option` / `Result` を返す関数で top になる件** (F7-1)。
  原因未確認。エラーを扱うコードの effect が読めないのは痛いが、
  エラーモデルの決めごとではなく effect 解析のバグ。
- `dyn Trait` が enum を受けるようにする (→ D2 の再検討)。
- `io::last_os_code()` (→ D3、実需要が出てから)。
- `try_push` (要素ごとに訊く形)。D5 は `try_reserve` (まとめて訊く形) だけを
  置いた。既存の署名を変えずに後から足せるので、`try_reserve` では
  足りない場面が出てから。

## テスト

- **E0 の語彙は `compiler/tests/consistency/` で pin する。** 検体は
  「未知の errno」を安定して作れる形にする — テストが自分で作った
  **ファイル**の下へ path を伸ばす (`<tmpfile>/nope.txt`) と `ENOTDIR` に
  なる (F3 の再現は `/etc/hosts` を使ったが、環境によっては存在せず
  `ENOENT` = `NotFound` に化ける)。**「4 レーンが同じ変種名を出す」を
  pin しないと同じ形で再発する** — 語彙が 4 経路に散っている限り、
  片方だけ直る。
- **`interpreter/example/error_model.t`** を置けば
  `compiler/tests/example_consistency.rs` が自動で 3 レーンに掛ける
  (CLAUDE.md の「example を追加すればカバレッジは自動で増える」)。
  集約エラー型 + `?` + `Display` + retryable の分岐を 1 本に入れる。
- **E1 は frontend のユニットテスト**で「同じ型に `From` を 2 つ」を
  直接置く。F1 の最小再現がそのまま検体。
- **E2 は「実行される `?`」を pin する。** F9 が今まで見つからなかったのは
  既存のテストと example が束縛形 (`val v = f()?`) しか書いていないから。
  文の位置 × (`Result<(), E>` / `Result<T, E>`) × (自由関数 / method) を
  4 レーンで走らせる。
- **E5 は `FixedBuffer` の quota 超過**を検体にする。3 つ見る:
  予算を跨いだ `push` が**要求バイト数つきで panic する**こと、
  `try_reserve` が同じ状況で `Err(OutOfMemory)` を返すこと、そして
  **空の `Vec` / `String` が panic しない**こと (最後が `heap_alloc(0)` の
  null 契約の回帰テスト — ここを間違えると全プログラムが即死する)。
  加えて **realloc の失敗で元のバッファを失わない**ことを
  `--profile=mem` の `leaks` で見る (`try_reserve` が `Err` を返した
  あとも vector が使えて、最後に漏れが 0)。

## 用語

- **語彙 (vocabulary)** — status コードの集合と、それに対応する enum
  variant の集合。`net` は「OS 非依存の語彙」を設計目標として明記している。
- **分類 A / B / C** — D1 の 3 分類。この文書の中でだけ使う略記。
