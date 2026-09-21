# STDLIB FS / PATH — path の文法とディレクトリの列挙

> 対象: 新設する `core/std/path.t` と `core/std/fs.t`、および
> `core/std/io.t` の既存のファイル API
> 状態の正本: [`todo.md`](todo.md) の **STDLIB-FS-PATH**
> 俯瞰と優先順位: [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md)
> 実測: 2026-09-03

## Status snapshot

| 項目 | 状態 |
|---|---|
| ファイルの読み書き | `read_file` / `read_file_into` / `write_file` / `append_file` / `write_file_bytes` |
| 存在確認 | `file_exists(path)` — 実体は `access(F_OK)` なので**ディレクトリにも true** |
| path の結合・分解 | **無い** (`join` / `dirname` / `basename` / `extension`) |
| ディレクトリ列挙 | **無い** (`ls` 相当が書けない) |
| metadata | **無い** (サイズも更新時刻も種別も読めない) |
| 作成・削除・改名 | **無い** (`mkdir` / `remove` / `rename`) |
| カレントディレクトリ | **無い** |
| 開いたファイル (handle) | **無い** (`open` / `seek` / `pread` / `fsync` / `truncate`) — §10 で入った |

## なぜ今これを設計するか

**ファイルを 1 つ名指しで読み書きすることしかできない。** 「このディレクトリ
の `.t` を全部読む」も「出力先の親ディレクトリを作る」も「拡張子を
差し替える」も書けない。実プログラム — この repo 自身のようなツール —
は、たいていそこから始まる。

分野として設計が要るのは、**経路が 2 つに割れる**から。path の結合と
分解は**文字列の操作**で純 toylang だが、ディレクトリの列挙と metadata は
**syscall** で extern。混ぜて設計すると、片方の都合がもう片方の API に
漏れる。

## 測ったこと (2026-09-03)

1. **`file_exists` は「ファイルがある」ではない。**
   `toylang_rt::toy_io_file_exists` は `access(path, F_OK) == 0`
   (`lib.rs:3858`)。**ディレクトリでも true**、シンボリックリンクは
   辿った先を見る。名前が実装より狭いことを言っている。

2. **extern の実装を 2 つ持つと割れる — この repo が既にやっている。**
   `interpreter/src/evaluation/extern_net.rs` の冒頭は、net の extern を
   `toylang_rt` へ**転送**した理由をこう書いている:

   > 代わりに `std::net` で 2 つ目の実装を書くと、errno → `NetError` の
   > 対応表が 2 つになりコメントで同期を取ることになる。既存の
   > RUNTIME-IO の extern は 2 実装を持っていて「同じ数を出す」と
   > 注記しているが、それをもう一度やる理由は無い。

   そして**実際に割れている** — ERROR_MODEL の E0 (同じ書き込み失敗が
   interpreter で `write error`、AOT/JIT で `read error`) がその 2 実装
   の帰結。**fs の新しい extern は net の流儀 (転送) にする。**

3. **配列を返せない extern の先例がある。**
   `Poller` は「`wait` が個数を返し、`event(i)` が i 番目を読む。次の
   `wait` まで有効 — 検査しない文書上の寿命で、`Span<T>` が自分の寿命を
   検査しないのと同じ」(`core/std/poll.t`)。**ディレクトリ列挙は同じ形**
   になる。

4. **module の top-level `const` は届かなかった** (MODULE-CONST、2026-09-21 に解消)。区切り文字も
   `pub fn separator() -> u8` の形になる。

5. **path に NUL を入れるとレーンで割れる** (コード上の帰結、未計測)。
   `str` は AOT では `[bytes][NUL][u64 len]` で `len()` は `strlen`
   (`core/std/str.t`)、tree-walker では `s.bytes().len()`。途中に NUL を
   持つ `str` は**compiled lane で切り詰められる**。path は外から来る
   文字列なので、**toylang 側で検査して弾く**。

## 既存の決定から引く制約

1. **POSIX だけ。** この言語は epoll / kqueue のどちらかを前提に
   ビルドし、未対応 OS は `compile_error!` で落ちる (NET N0)。
   Windows の path 文法 (`\`、ドライブレター、UNC) は**規約にすら
   書かない** — 書けば「そのうち対応する」という意味になる。
2. **新しい文字列を作る API は `String` を返す** (STDLIB_TEXT §1)。
   `str` は NUL 終端なので**部分文字列を借用で表せない** (実測 5) —
   `basename` が確保するのは避けようがない。
3. **失敗は module ごとの enum、共通 trait は置かない** (ERROR_MODEL)。
   fs は `IoError` を**共有する** (別の enum を作らない) —
   `read_file` と `list_dir` の失敗が別の型なら、呼び出し側は 2 つの
   `match` を書くことになる。
4. **payload + status のペア** (RUNTIME-IO)。境界は scalar のまま。
5. **受け入れは 4 レーン一致**。fs はホストの状態を触るので、
   テストは tempdir fixture の上で行う (`write_file` のテストが既に
   その形)。

## 1. 経路を分ける

| 層 | 実装 | 中身 |
|---|---|---|
| `core/std/path.t` | **純 toylang** | 結合・分解・正規化・判定。**syscall を 1 つも呼ばない** |
| `core/std/fs.t` | **extern (`toylang_rt` へ転送)** | 列挙・metadata・作成・削除・改名 |

**`path` が syscall を呼ばないことを不変にする。** そうすると:

- `path` の全関数が **`never_allocates` ではない**が **`pure` に近い**
  (確保はするが io エフェクトを持たない)。`--effects` でそれが見える。
- **テストがホストに依存しない** — `join("/a", "b") == "/a/b"` は
  ファイルシステムの状態と無関係に 4 レーンで pin できる。
  この分野のテストの大半がここに乗る。
- 「存在するか」を混ぜない。`normalize` が `..` を畳むのは**語彙的**で、
  symlink を辿らない (§4)。

## 2. `Path` 型は作らない

`str` / `String` の上の free function にする。TIME で `Duration` を
作らなかったのと同じ判断:

- `Path` を作ると `Display` / `eq` / `Ord` / `Hash` / `String` との
  相互変換 / `Vec<Path>` が付いてくる。
- 得るのは「path と普通の文字列を取り違えない」ことだけ。
- そのうえ **`io::read_file(path: str)` が `str` を取る**ので、
  境界で毎回変換することになる。

## 3. path の文法

```
pub fn join(a: str, b: str) -> String
pub fn dirname(p: str) -> String
pub fn basename(p: str) -> String
pub fn extension(p: str) -> String
pub fn stem(p: str) -> String
pub fn is_absolute(p: str) -> bool
pub fn normalize(p: str) -> String
pub fn with_extension(p: str, ext: str) -> String
```

**端の場合を全部決めて表にする** (`parse::to_u64` が受理集合を表にした
のと同じ。決めないと 4 レーンで割れるのではなく、**人の期待と割れる**):

| 式 | 結果 | 理由 |
|---|---|---|
| `join("a", "b")` | `a/b` | |
| `join("a/", "b")` | `a/b` | 区切りを重ねない |
| `join("a", "/b")` | `/b` | 右が絶対なら右が勝つ |
| `join("", "b")` | `b` | |
| `join("a", "")` | `a` | 末尾に区切りを足さない |
| `dirname("/a/b")` | `/a` | |
| `dirname("a")` | `.` | POSIX `dirname(1)` と同じ |
| `dirname("/")` | `/` | |
| `dirname("a/b/")` | `a` | 末尾の区切りは先に落とす |
| `basename("/a/b")` | `b` | |
| `basename("/a/b/")` | `b` | 同上 |
| `basename("/")` | `/` | |
| `extension("a.tar.gz")` | `gz` | **最後の `.` の後ろ** |
| `extension(".bashrc")` | `` | 先頭の `.` は拡張子ではない |
| `extension("a.")` | `` | |
| `extension("a")` | `` | |
| `stem("a.tar.gz")` | `a.tar` | `extension` の補集合 |
| `is_absolute("/a")` | `true` | 先頭が `/` かどうかだけ |

**NUL を含む path は関数の入口で弾く** (実測 5) — `path` は失敗を
返さない設計なので、**panic** する (`Vec` の範囲外と同じ扱い。
壊れた入力を黙って切り詰めない)。外から来る path を検査したい側には
`pub fn is_valid(p: str) -> bool` を置く (STDLIB_TEXT の
「先に訊く」規律)。

## 4. `normalize` は語彙的

`a/b/../c` → `a/c`、`./a` → `a`、`a//b` → `a/b`、末尾の `/` を落とす。
**symlink を辿らない**ので、`b` が symlink なら `a/b/../c` は
実際には `a/c` ではない。

- doc comment に**その 1 文を書く**。
- 本物が要る側には `fs::realpath(p) -> Result<String, IoError>`
  (extern、`realpath(3)`)。**存在しない path は失敗する**ので、
  「path をきれいにする」用途には使えない — だから 2 つある。

## 5. ディレクトリの列挙

extern は poller の形 (実測 3):

```
extern fn __extern_fs_dir_open(path: str) -> u64      # 個数。失敗は status で
extern fn __extern_fs_dir_name(i: u64) -> str
extern fn __extern_fs_dir_kind(i: u64) -> u32         # 0=file 1=dir 2=symlink 3=other
extern fn __extern_fs_status() -> u64
```

**toylang 側の正面は「全部コピーする」方**にする:

```
pub fn list_dir(path: str) -> Result<Vec<DirEntry>, IoError>
struct DirEntry { name: String, kind: FileKind }
```

poller が index 読みを**正面**にしているのは、イベントループが 1 ms ごとに
読む場所だから。**ディレクトリの列挙はそうではない** — そして
「列挙しながら各エントリで再帰する」(ディレクトリの木を歩く) が
いちばん普通の使い方で、それは**runtime 側のバッファが次の
`dir_open` で上書きされる**形と正面から衝突する。コピーを既定にする。

- **`.` と `..` は返さない** (`readdir` は返すので runtime 側で落とす)。
  返すと、木を歩くコードが 100% 無限ループを書く。
- **順序は未規定。** ファイルシステム次第で、同じディレクトリでも
  OS をまたぐと変わる。**テストは sort してから比較する**と決めておく
  (COLLECTIONS が反復順を「決めて書く」ことにしたのの裏返し —
  ここは**決められない**ので、決められないと書く)。
- `kind` は `readdir` の `d_type` から取り、**`DT_UNKNOWN` を返す
  ファイルシステムがある**ので、その場合だけ runtime 側で `lstat` に
  落とす。呼び出し側に「種別が分からないことがある」を漏らさない。

## 6. metadata

`stat` は複数のスカラーを返すので、RUNTIME-IO の「payload + 直後に
status」を**フィールド数だけ広げた**形にする (poller と同じ read-back):

```
extern fn __extern_fs_stat(path: str, follow: bool) -> u64   # status
extern fn __extern_fs_stat_size() -> u64
extern fn __extern_fs_stat_kind() -> u32
extern fn __extern_fs_stat_mtime() -> i64                    # Unix 秒
extern fn __extern_fs_stat_mode() -> u32
```

```
pub fn metadata(path: str) -> Result<FileInfo, IoError>
struct FileInfo { size: u64, kind: FileKind, mtime_secs: i64, mode: u32 }
impl FileInfo { fn is_dir(&self) -> bool; fn is_file(&self) -> bool }
```

- **`mtime_secs` は Unix 秒**なので、`time::DateTime::from_unix` に
  そのまま渡る (STDLIB_TIME §4)。fs 側で日付を組み立てない。
- **`follow` 引数で `stat` / `lstat` を選ぶ**。`metadata` は辿り、
  `symlink_metadata` は辿らない。2 つの extern にしない。
- `mode` は生の st_mode 下位 12 bit。**権限の型は作らない** (§非目標)。
- **`file_exists` は残すが、doc comment を実装に合わせる** (実測 1)。
  `is_file(p)` / `is_dir(p)` を `metadata` の上に純 toylang で置く。

## 7. 変更系

```
pub fn mkdir(path: str) -> Result<(), IoError>
pub fn mkdir_all(path: str) -> Result<(), IoError>       # 純 toylang (mkdir の反復)
pub fn remove_file(path: str) -> Result<(), IoError>
pub fn remove_dir(path: str) -> Result<(), IoError>      # 空のみ
pub fn rename(from: str, to: str) -> Result<(), IoError>
pub fn copy_file(from: str, to: str) -> Result<u64, IoError>   # 純 toylang
```

- **`remove_dir_all` は置かない。** 再帰的な削除は、この言語で最初に
  書かれる「取り返しのつかない道具」になる。`list_dir` + `remove_file`
  で 10 行で書けるので、**書く人が自分で書く**。
- `mkdir_all` と `copy_file` は**純 toylang** (`mkdir` / `read_file_into` /
  `write_file_bytes` の上)。extern を増やさない。
- `copy_file` は **binary safe** — `read_file` (str) ではなく
  `read_file_into` + `Span<u8>` を使う (EXTERN-BUF の経路)。

## 8. 失敗

**`IoError` を共有し、3 つ variant を足す:**

```
AlreadyExists      # mkdir / rename の衝突 (EEXIST)
NotADirectory      # ENOTDIR
NotEmpty           # ENOTEMPTY (remove_dir)
```

- **これは破壊的変更** — `IoError` を網羅 match している既存コードが
  落ちる。`K: Hash` bound を足したときと同じ扱いで、todo に明記する。
- ERROR_MODEL の規約どおり、**errno は畳んで未知は `Unknown`**。
  隣の variant に化けさせない。
- 対応表は **`toylang_rt` に 1 つだけ**置く (実測 2)。

## 9. テスト

- **`path` は 4 レーンで値ごと pin** — §3 の表がそのままテストになる。
  ホストに触らないので fixture も要らない。**この分野のテストの
  大半がここ。**
- **`fs` は tempdir fixture の上で 4 レーン**。`write_file` の
  consistency テストが既にその形を持っている。
- **列挙の順序は pin しない** — sort してから比較する (§5)。
- **`stat` の値は pin しない** — サイズだけ pin し、mtime は
  「書いた直後の `now_unix()` との差が小さい」で見る。

## Phase 分割

| Phase | 内容 | 受け入れ |
|---|---|---|
| **F0** | `path.t` (§3・§4 の `normalize`) | §3 の表を 4 レーンで pin。NUL の panic も |
| **F1** | `list_dir` + `DirEntry` / `FileKind` (§5) | tempdir に作った木の列挙が sort 後一致 |
| **F2** | `metadata` / `is_file` / `is_dir` + `file_exists` の doc 修正 (§6) | サイズ pin、`mtime` は `time::` と繋がること |
| **F3** | 変更系 (§7) + `IoError` の 3 variant (§8) | 失敗の文言が 4 レーン一致 (E0 の再発防止) |
| **F4** | `realpath` / `current_dir` / `temp_dir` | `temp_dir` は `TMPDIR` から純 toylang |
| **F5** | `mkdir_all` / `copy_file` (純 toylang) | binary safe を非 UTF-8 ファイルで pin |
| **F6** | `File` (§10) | `read_at` が cursor を動かさないこと・append・idempotent な `close` を 3 レーンで pin |

**F0 が先頭**なのは、syscall を 1 つも呼ばずに分野の半分が landing
できるから。F3 の `IoError` 拡張は ERROR_MODEL の E0 (語彙が 4 経路に
別々に書かれている) が直った**後**にやる — 先にやると割れた語彙を
3 つ増やすことになる。

## 10. 開いたファイル (F6)

§1〜§9 はすべて**パスを指定した全体操作**で、これは意図的な線引き
だった (「ハンドルは需要が出てから」)。需要は
[`poc/logsearch`](../poc/logsearch/design-docs/RUNTIME_GAPS.md) が出した
— 索引を別ファイルに割ったのも、セグメントを 8 MiB で止めたのも、
**footer を読むにはファイルを読むしかなかった**ためで、あの文書は
これを「残る前提のうち最大 (R2)」と書いている。

### 形

`net.t` の `TcpStream` と同じ: `struct File { fd: i32 }`、`Drop` が
閉じ、`close()` は早く閉じて field を `-1` に置く (二度目の close が
無関係なファイルを閉じないため)。

| 種類 | API |
|---|---|
| 開く | `File::open` (読み、作らない) / `create` (書き、消す) / `append` (末尾に書く) / `open_rw` (読み書き、**残す**) |
| 逐次 | `read(buf)` / `write(buf)` — cursor が進む |
| 範囲 | `read_at(offset, buf)` / `write_at(offset, buf)` — **cursor を動かさない** |
| cursor | `seek_to(u64)` / `seek_by(i64)` / `seek_end(i64)` / `tell()` |
| その他 | `size()` (cursor を動かさない) / `sync()` / `truncate(len)` / `close()` / `as_fd()` / `is_open()` |

`buf` は `Span<u8>`。確保もコピーもせず、EXTERN-BUF の経路で
**toylang のメモリに直接**読み書きする (`net::TcpStream` の read/write と
同じ受け口なので、`Vec::with_capacity` + `capacity_span` + `set_size`
という規約も同じ)。

### 決めたこと

1. **`whence` を露出しない。** `lseek(2)` の 0/1/2 は `seek_to` /
   `seek_by` / `seek_end` の 3 メソッドに分けた。書いた当時は module
   top-level の `const` がどこからも見えず (MODULE-CONST、2026-09-21 に
   解消) `Poller` の `interest_read()` 方式しか無かったが、この 3 つは
   どちらにせよ名前にした方が短い。
2. **開き方は 4 つの named constructor。** `open(2)` の
   `O_CREAT` / `O_TRUNC` / `O_APPEND` は**値がホストで違う**ので、
   flag を toylang 側に出すと `sys_epoll` / `sys_kqueue` を分けた
   のと同じ事故 (値を写し間違えても開けてしまう) を招く。runtime が
   4 つの番号を platform の flag に写す。
3. **短い read / write は `Ok(n)` であって `Err` ではない。**
   `Ok(0)` は EOF。これは `net::TcpStream` の規約と同じで、
   count と status を**必ず一緒に読む** (RUNTIME-IO のペア方式)。
   ディスクが一杯なのを `Ok` の小さい数で報せるのは `write(2)` の形。
4. **`FILE*` ではなく生の fd。** `pread` / `fsync` / `ftruncate` は
   fd の操作で、buffered stream に載せると「毎回手で flush する」
   規律が要る。`as_fd()` があるので `Poller::register` にも渡せる。
5. **`write_at` は `append` と組み合わせない。** POSIX は
   `O_APPEND` の fd への `pwrite` を未規定とし、Linux は offset を
   無視して末尾に足す。`open_rw` と使う。

失敗は §8 の `IoError` をそのまま使う (status の表は `io_status` 1 つ、
`fs_error_from_status` が decode する — 3 つ目の表は作らない)。

### 残した穴

`openat` / `dup` / `flock` / `mmap` / 非同期 I/O は入れていない。
`mtime` が無いのは §6 のまま (`struct stat` の layout 問題は handle が
入っても変わらない — `fstat` でも同じ layout を読む)。

## 非目標

- **Windows の path 文法** — 制約 1。
- **`set_current_dir`** — プロセス全体の可変状態で、相対 path の意味が
  実行中に変わる。絶対 path を `join` で組む方に倒す。
- **権限の型 / `chmod` / `chown`** — `mode: u32` を生で返すところで
  止める。型を作ると POSIX の権限モデル全部が付いてくる。
- **symlink の作成 (`symlink` / `link`)** — 読む側 (`kind` /
  `symlink_metadata` / `realpath`) だけ置く。作る需要が出てから。
- **`remove_dir_all`** — §7。
- **ファイルロック / mmap** — どちらも「いつ書かれたか」の規約を
  持ち込むので、必要になった時点で別に設計する。
  **`fsync` はここから外した (F6)** — handle が入ると `fsync` は
  「その fd の書き込みを落とす」以上の意味を持たず、規約は
  「呼んだら落ちる」の 1 行で足りる。持ち込むと思っていた複雑さは
  path 指定の全体操作しか無かった頃の見立てだった。
- **ディレクトリ監視 (inotify / kqueue の vnode)** — EVENT_POLLING の
  統一形に vnode を足す話で、この分野ではない。
- **glob / パターンマッチ** — `list_dir` と STDLIB_TEXT の
  `starts_with` / `find` があれば純 toylang で書ける。stdlib に
  入れるかは、書いてみてから。

## 関連

- [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) — 分野の俯瞰
- [`STDLIB_TEXT.md`](STDLIB_TEXT.md) — path は文字列の走査。`str` が
  借用で部分文字列を表せない件 (制約 2) と NUL (実測 5)
- [`STDLIB_TIME.md`](STDLIB_TIME.md) — `mtime_secs` の受け口
- [`ERROR_MODEL.md`](ERROR_MODEL.md) — `IoError` の拡張と E0
- [`NETWORK_IO.md`](NETWORK_IO.md) — extern を `toylang_rt` へ転送する
  流儀 (実測 2)
- [`EVENT_POLLING.md`](EVENT_POLLING.md) — index で読み戻す形 (実測 3)
