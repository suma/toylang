# NETWORK_IO.md — socket ラッパーとプラットフォーム切り替えの設計

> **状態: 未着手 (設計のみ)**。イベント通知 (epoll / kqueue) の設計は
> 分量が大きいので [`EVENT_POLLING.md`](EVENT_POLLING.md) に分けた。
> 本文書は **socket の呼び出しインターフェース**と、その下の
> **コンパイル時プラットフォーム切り替えの機構**を扱う。
> 項目の状態管理は [`todo.md`](todo.md) が正本。

## Status snapshot

| Phase | Scope | Status |
|---|---|---|
| **N0** | `sys` 切り替え機構 + ABI probe テスト + `backend_name()` | 未着手 |
| **N0.5** | CONV-SPAN + 確保しないための stdlib (`Span::slice` / `Vec::with_capacity` / `set_size`) | ✅ 完了 (2026-08-31) |
| **N1** | ~~EXTERN-BUF~~ ✅ (2026-08-31) + TCP client (`socket`/`connect`/`send`/`recv`/`close`) + `NetError` + blocking 切り替え | 部分完了 |
| **N2** | TCP server (`bind`/`listen`/`accept`) + nonblocking | 未着手 |
| **N3** | イベント通知 ([`EVENT_POLLING.md`](EVENT_POLLING.md)) | 未着手 |
| **N4** | UDP + socket option (timeout / nodelay) + `local_addr` / `peer_addr` | 未着手 |
| **N5** | 名前解決 (`getaddrinfo`) | 未着手 |

## なぜ今これを設計するか

`RUNTIME_LIBRARY.md` の非目標節は「ネットワーク / async runtime は需要が
未確認」としていた。設計を起こす理由は 1 つで、**これは並行性 (todo の
CONCURRENCY) を待たずに書ける**からである。

- 並行性は「共有可変性を move / Drop モデルにどう載せるか」が本体で、
  `Send` 相当の判定を決めるまで着手できない (RUNTIME_LIBRARY P3)。
- 一方 **nonblocking socket + イベント通知は単一スレッドで完結する**。
  スレッドも channel も要らず、`while` ループと `match` だけでサーバが
  書ける。toylang に今ある道具でちょうど書ける形をしている。

つまり本設計は並行性の前借りではなく、**並行性なしで実プログラム
(エコーサーバ / HTTP クライアント) を書けるようにする**ためのもの。
逆に言えば、本設計は blocking API を主役に据えない (下の非目標)。

## 既存の決定から引く制約

新しく決めることを減らすため、まず動かせない前提を並べる。

1. **`toylang_rt` は `no_std` で依存ゼロ** — libc は `unsafe extern "C"`
   でその都度手宣言している (`lib.rs` 冒頭)。`libc` crate は入れない。
   したがって **socket / epoll / kqueue の構造体と定数はすべて手で書く**
   ことになる。これが本設計で最も壊れやすい部分なので、ABI probe テスト
   (下記) で固定する。
2. **extern 境界はスカラと str handle しか運べない** — `core/std/io.t`
   の冒頭が明記しているとおり、`extern fn` の境界は C ポインタを
   deref できない (interpreter の `ptr` はホスト番地ではなく
   `HeapManager` のバイト列への index)。`time(t: ptr)` が引数を捨てて
   いるのはこの制約の現れ。**buffer をやり取りする API はここを最初に
   解く必要がある** (§4)。
3. **`Result` を返す extern は status ペア方式** — payload を運ぶ extern が
   失敗コードを runtime 側に記録し、直後にペアの `__extern_*_status` が
   読む (RUNTIME-IO)。境界はスカラのままで `Result` が作れる。
   ネットワークもこの方式に乗せる。
4. **IR VM は extern を実行できない** — `compiler_vm::eligibility` が
   到達可能な body-less 関数を見つけた時点で不適格を返す。つまり
   **net を使うプログラムは interpreter では必ず tree-walker で走る**。
   実装が要るレーンは **tree-walker / compiler JIT / AOT の 3 つ**で、
   IR VM と interpreter 側 JIT には何も要らない (後者は silent fallback)。
5. **extern は追跡不能なので全エフェクトを持つ** (EFFECT_SYSTEM) —
   ただし宣言で 1 つ取り戻せる。net の extern は toylang ヒープを
   一切触らないので**全部 `never_allocates extern fn` で宣言する**。
   これで「確保しないイベントループ」が `never_allocates fn` として
   書けるようになる (この設計のちょっとした報酬)。
6. **`from "toylang_rt"` は FFI の型制約の適用外** — シンボルが内部で
   マーシャリングする前提 (FFI_PLAN P1 メモ)。net もこの経路を使う。

## 1. 層構造

```
core/std/net.t          TcpListener / TcpStream / UdpSocket / NetError   ← ユーザが見る層
core/std/poll.t         Poller / Event / Interest                        (EVENT_POLLING.md)
        │  extern fn __extern_net_* from "toylang_rt" as "toy_net_*"
        ▼
toylang_rt/src/net.rs   toy_net_*  — str/scalar 境界、errno → status 変換
toylang_rt/src/poll.rs  toy_poll_* — イベントの staging と読み出し
        │  sys::* (単一の名前集合)
        ▼
toylang_rt/src/sys_epoll.rs   |   toylang_rt/src/sys_kqueue.rs
   Linux: epoll + accept4 +   |   macOS/BSD: kqueue + accept + fcntl +
   SOCK_NONBLOCK + MSG_NOSIGNAL|   SO_NOSIGPIPE + sin_len
```

`net.rs` / `poll.rs` には `#[cfg]` を**一切書かない**。OS 差はすべて
`sys_*.rs` の内側に閉じる。これが守れているかは「`net.rs` に `cfg` が
出てきたら設計が漏れている」という 1 行の grep で確認できる。

## 2. コンパイル時切り替えの機構

### 検討した 3 案

| 案 | 形 | 判定 |
|---|---|---|
| (a) 関数ごとの `#[cfg]` | 既存の `current_errno` と同じ手口を 40 関数に展開 | **却下**。切り替え点が 40 箇所に散り、「片方の OS だけ実装が古い」を誰も検出できない |
| (b) `trait Sys` + `type Active = ...` | 契約が型で書ける | **却下**。単一ホストでは impl が 1 つしかコンパイルされないので、trait でも「両方実装した」ことは強制できない。得られるのは可読性だけで、`no_std` に vtable / generic 層を持ち込む対価に見合わない |
| (c) **`#[cfg_attr(..., path = ...)] mod sys;`** | 切り替え点 1 箇所、実装 1 OS 1 ファイル | **採用** |

### 採用形

```rust
// toylang_rt/src/lib.rs
//
// The platform switch. Exactly one `sys` module is compiled; the file
// *is* the porting contract — anything `net.rs` / `poll.rs` calls has
// to exist in every `sys_*.rs`, or that target fails to build. Keep
// `#[cfg]` out of `net.rs` and `poll.rs`: OS differences live here.
#[cfg_attr(target_os = "linux", path = "sys_epoll.rs")]
#[cfg_attr(
    any(target_os = "macos", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd"),
    path = "sys_kqueue.rs"
)]
mod sys;

#[cfg(not(any(
    target_os = "linux", target_os = "macos",
    target_os = "freebsd", target_os = "netbsd", target_os = "openbsd"
)))]
compile_error!(
    "toylang_rt: no event-notification backend for this target \
     (expected epoll on Linux or kqueue on a BSD); see design-docs/NETWORK_IO.md"
);
```

**この形を選ぶ理由**は 3 つ。

1. **切り替え点が 1 箇所**。「どちらが有効か」を知るのに読む行が 5 行で
   済む。関数ごとの `#[cfg]` はこの性質を持たない。
2. **ファイルが移植の契約になる**。`net.rs` が呼ぶ名前が
   `sys_kqueue.rs` に無ければ macOS ビルドが落ちる。片肺の実装が
   黙って残ることがない (これが (a) を却下した理由そのもの)。
3. **未対応 OS が親切に落ちる**。`compile_error!` が無いと、Windows で
   ビルドしたときに「未定義シンボル数十個」というリンクエラーになる。

### `sys` が提供する名前 (移植の契約)

socket 側。イベント通知側は [`EVENT_POLLING.md`](EVENT_POLLING.md) の
同名節にある。

```rust
// Constants whose *values* differ per OS. Everything above this layer
// uses these names, never the numbers.
pub const SOL_SOCKET: i32;      // Linux 1        / macOS 0xffff
pub const SO_REUSEADDR: i32;    // Linux 2        / macOS 0x0004
pub const O_NONBLOCK: i32;      // Linux 0o4000   / macOS 0x0004

// Operations whose *shape* differs per OS.
pub fn socket_stream(family: i32) -> i32;    // Linux: SOCK_NONBLOCK|SOCK_CLOEXEC in one call
                                             // BSD:   socket() + fcntl() + SO_NOSIGPIPE
pub fn accept_nonblocking(listen_fd: i32) -> i32;  // Linux: accept4() / BSD: accept()+fcntl()
pub fn send_nosignal(fd: i32, buf: *const u8, len: usize) -> isize;
                                             // Linux: send(MSG_NOSIGNAL) / BSD: plain send()
// Address handling. Text in, sockaddr out — the *caller* never sees a
// sockaddr, and `family` is threaded through from day one so adding
// AF_INET6 later changes no signature (論点 4). BSD writes sin_len,
// Linux does not; that difference lives here.
pub const AF_INET: i32;
pub fn sockaddr_from_str(text: &[u8], port: u16, family: i32, out: *mut u8) -> i32;
pub fn sockaddr_to_str(sa: *const u8, out: &mut [u8]) -> usize;   // local_addr / peer_addr

// Blocking mode and its timeouts (論点 1). `SO_RCVTIMEO` differs in
// value *and* in payload: `struct timeval`'s tv_usec is i64 on Linux
// and i32 on macOS, so the whole set/get goes through here.
pub const SO_RCVTIMEO: i32;                  // Linux 20 / macOS 0x1006
pub const SO_SNDTIMEO: i32;                  // Linux 21 / macOS 0x1005
pub fn set_blocking(fd: i32, on: bool) -> i32;
pub fn set_timeout(fd: i32, which: i32, ms: i64) -> i32;

pub fn status_from_errno(err: i32) -> u64;   // the errno *values* differ (see §6)
pub const BACKEND_NAME: &str;                // "epoll" / "kqueue"
```

`sockaddr_in` を `sys` に閉じ込めているのが要点で、**toylang 側は
sockaddr を一度も見ない**。macOS の `sin_len`(先頭 u8) と Linux の
`sin_family`(先頭 u16) という表現差が上へ漏れないのはこのため。
`connect` / `bind` は**アドレスを `str` で、family を i32 で**渡す —
u32 スカラにしないのは IPv6 を後から足せるようにするため (論点 4)。

### 定数を手書きすることの安全策 — ABI probe テスト

依存ゼロの代償として、`SOL_SOCKET` や `struct epoll_event` の大きさを
間違えても**コンパイルは通り、実行時に静かに壊れる**。これを防ぐため、
**C 側に真の値を印字させて Rust 側の定義と突き合わせるテスト**を置く
(FFI の `compiler/tests/fixtures/ffi/libtoytest.c` を `cc` でビルドして
いる前例がある)。

```c
/* compiler/tests/fixtures/net/abi_probe.c — prints the ground truth */
printf("SOL_SOCKET %d\n", SOL_SOCKET);
printf("SO_REUSEADDR %d\n", SO_REUSEADDR);
printf("O_NONBLOCK %d\n", O_NONBLOCK);
printf("EAGAIN %d\n", EAGAIN);
printf("SO_RCVTIMEO %d\n", SO_RCVTIMEO);
printf("sizeof_timeval %zu\n", sizeof(struct timeval));
printf("sizeof_tv_usec %zu\n", sizeof(((struct timeval *)0)->tv_usec));  /* i64 vs i32 */
printf("sizeof_sockaddr_in %zu\n", sizeof(struct sockaddr_in));
printf("offset_sin_port %zu\n", offsetof(struct sockaddr_in, sin_port));
/* ... epoll_event / kevent は EVENT_POLLING.md 側 ... */
```

テストは probe をビルド・実行し、行ごとに `toylang_rt` の `pub const` と
比較する。**N0 の受け入れ基準はこのテストが green になること**で、
socket が 1 本も繋がらなくても価値がある (以降のすべての土台なので)。

### build.rs の罠 — 先に直す必要がある

`compiler/build.rs` は AOT 用 staticlib を `rustc --crate-type staticlib`
で直接ビルドし、再ビルド条件を

```
cargo:rerun-if-changed=runtime/toylang_rt/src/lib.rs
```

の**1 ファイルだけ**で宣言している。`sys_epoll.rs` を足すと、
**そのファイルを編集しても AOT の staticlib が再ビルドされない** —
JIT (cargo がビルドする rlib) だけが新しくなり、AOT だけ古い挙動を
続ける。バックエンド不一致として現れるので原因が遠い。

**モジュール分割と同じコミットで**、ディレクトリ監視に変える:

```
cargo:rerun-if-changed=runtime/toylang_rt/src
```

## 3. toylang から見える API (`core/std/net.t`)

「適宜リッチでよい」という方針なので、fd を裸で扱わせず型で包む。

```rust
# A listening TCP socket. Owns its fd: `impl Drop` closes it, so the
# move checker (E0014) stops it from being used after it is handed
# away. `close()` is idempotent (it parks fd = -1) — see 論点 2.
struct TcpListener { fd: i32, closed: bool }

impl TcpListener {
    # Bind to `addr:port` and start listening. `port = 0` asks the OS
    # for an ephemeral port; read it back with `local_port()`, which
    # is what makes tests deterministic without hardcoding a port.
    fn bind(addr: str, port: u16) -> Result<TcpListener, NetError>
    fn local_port(&self) -> Result<u16, NetError>
    # `Err(NetError::WouldBlock)` when no connection is pending — the
    # normal answer in an event loop, not a failure.
    fn accept(&self) -> Result<TcpStream, NetError>
    fn as_fd(&self) -> i32          # to register with a Poller
    fn local_addr(&self) -> Result<str, NetError>
    # Blocking is a property of the fd, not of the call (論点 1): the
    # same `accept` blocks or returns WouldBlock depending on this.
    # New sockets start non-blocking.
    fn set_blocking(&self, on: bool) -> Result<(), NetError>
    fn close(&mut self) -> Result<(), NetError>
}

struct TcpStream { fd: i32, closed: bool }

impl TcpStream {
    # Non-blocking connect: `Ok(stream)` when it completed immediately
    # (the loopback case), `Err(NetError::InProgress)` otherwise — the
    # caller then waits for writability and calls `take_error()`.
    fn connect(addr: str, port: u16) -> Result<TcpStream, NetError>
    fn read(&self, buf: Span<u8>) -> Result<u64, NetError>    # 0 = peer closed
    fn write(&self, buf: Span<u8>) -> Result<u64, NetError>   # may be short
    fn take_error(&self) -> Result<(), NetError>              # SO_ERROR, for connect
    fn shutdown_write(&self) -> Result<(), NetError>
    fn set_nodelay(&self, on: bool) -> Result<(), NetError>
    fn set_blocking(&self, on: bool) -> Result<(), NetError>
    # Only meaningful while blocking; `Err(NetError::TimedOut)` on
    # expiry. Keeps a blocking client from hanging the whole program,
    # which is the whole risk of blocking mode in a single-threaded
    # language.
    fn set_read_timeout(&self, ms: u64) -> Result<(), NetError>
    fn set_write_timeout(&self, ms: u64) -> Result<(), NetError>
    fn peer_addr(&self) -> Result<str, NetError>
    fn as_fd(&self) -> i32
    fn close(&mut self) -> Result<(), NetError>
}
```

`read` / `write` が **`Span<u8>` を受け取ってバイト数を返す**のは
POINTER P4 の決定 (`&[T]` はライブラリ側で回収済み) に従うと同時に、
**API が確保しないための形**でもある (§5)。バッファの所有者は常に
呼び出し側で、stdlib も runtime も確保しない。

### `NetError`

`IoError` と同じ設計 (網羅 match できる enum + `Display`)。**別の enum に
する**のは、`WouldBlock` / `InProgress` がネットワークでは正常系であり、
ファイル IO の語彙に混ぜると `io::read_file` の match が意味のない腕を
持つことになるため。相互変換が要るなら `From` で足す。

```rust
pub enum NetError {
    WouldBlock,          # EAGAIN / EWOULDBLOCK — 正常系。再度待つ
    InProgress,          # EINPROGRESS — nonblocking connect の進行中
    Interrupted,         # EINTR — シグナルで中断。呼び直してよい
    ConnectionRefused, ConnectionReset, ConnectionAborted,
    BrokenPipe,          # EPIPE
    NotConnected,        # ENOTCONN
    AddrInUse, AddrNotAvailable,
    NetworkUnreachable, HostUnreachable,
    TimedOut,
    TooManyOpenFiles,    # EMFILE / ENFILE
    InvalidInput,        # EINVAL / EAFNOSUPPORT / 呼び出し側の誤り
    Unknown,
}
```

## 4. バイト列をどう運ぶか (CONV-SPAN + EXTERN-BUF)

`send` / `recv` はバイト列を運ぶが、制約 2 のとおり extern 境界は
ポインタを deref できない。ここは**独立した 2 つの問題**が重なっていて、
分けないと解けない。

- **(1) toylang 側に「バイト列の窓」を作る口が無い** (CONV-SPAN)
- **(2) interpreter の extern がメモリを読み書きできない** (EXTERN-BUF)

### (1) CONV-SPAN — `str` / `String` / `Vec<T>` から窓を作る

`Span<T>` は既にあるが、**`Span::from_parts` は `Ptr<T>` を要求し、
`Ptr<T>` は `Ptr::alloc` でしか作れない** (`core/std/ptr.t` の公開関数は
`alloc` / `get` / `set` / `offset` / `as_raw` / `__getitem__` /
`__setitem__` の 7 つで、**生 `ptr` から持ち上げる口が無い**)。
一方 `String::as_ptr()` も `Vec::as_ptr()` も生 `ptr` を返す。つまり

```
String ──as_ptr()──> ptr ──?──> Ptr<u8> ──from_parts()──> Span<u8>
                                  ↑ ここが繋がっていない
```

**これを繋ぐのが本項の作業**で、`Span<u8>` を受け口にする本設計の API
(§3) はこれが無いと 1 行も書けない。追加するのは 4 つ:

**CONV-SPAN は 2026-08-31 に landing 済み。** `Option<Ptr<T>>` という
戻り型自体がコンパイラの 3 層で通らなかったので、そこを先に直している
(todo の GENERIC-IN-ENUM-PAYLOAD / SELF-IN-TYPE-ARG)。入ったのは:

```rust
# core/std/ptr.t — lift a raw address into a typed window. Checked
# rather than unsafe-by-fiat: `Ptr<T>` is non-null by construction
# (POINTER P5), and a null here would break that invariant silently.
fn try_from_raw(p: ptr) -> Option<Self>

# core/std/span.t — the two steps above in one call, plus the
# sub-window that makes application-side zero-copy real.
fn try_from_raw_parts(p: ptr, len: u64) -> Option<Self>
fn slice(&self, offset: u64, len: u64) -> Span<T>    # panics out of range

# core/std/string.t, collections/vec.t — the everyday forms.
fn as_span(&self) -> Option<Span<u8>>        # String: len() bytes
fn as_span(&self) -> Option<Span<T>>         # Vec<T>: size() live elements
fn capacity_span(&self) -> Option<Span<T>>   # Vec<T>: the whole allocation
fn with_capacity(n: u64) -> Self             # reserve once
fn set_size(&mut self, n: u64)               # declare how much is live
```

**2 つの規則で統一した**: 生の番地が型に入るところは `Option`
(`Ptr<T>` の非 null 不変が構成で保たれる)、範囲外の添字は panic
(`Span::get` / `Vec::get` の既定)。空の `Vec` / `String` が `None` なのは
**確保が無い**からで、「要素が無い」からではない。受信は
`capacity_span()` に書いて `set_size(n)` で live を宣言し、`as_span()`
がその n 要素を見る (§5)。

これで `stream.write(msg.as_span())` / `stream.read(buf.as_span())` が
書ける。**CONV-SPAN は net 専用ではない** — `__simd_load` の受け口
(`Span::as_raw`)、`Vec` と `String` の相互運用、将来のバッファ系 API が
全部ここに乗る。POINTER.md の `Span<T>` が「`&[T]` のライブラリ側の
答え」であることを考えると、**構築の口が 1 つしか無かったのは元から穴**
だった。

### (2) EXTERN-BUF — それでも残る問題

CONV-SPAN は **toylang 側**の話で、extern 境界は解かない。`Span<u8>` を
渡すとき境界を越えるのは `(ptr, len)` のスカラ 2 つだが、
**interpreter の `ptr` はホスト番地ではなく `HeapManager` のバイト列への
index** なので、registry の実装が

```rust
pub type ExternFn = fn(&[Value]) -> Result<Value, InterpreterError>;
```

とヒープを持たない以上、`Object::Pointer(addr)` を受け取っても
**ただの整数**でしかない。`__builtin_str_to_ptr` が既に `ptr` を返せる
ことからも分かるとおり、**詰まっているのは「ポインタを作れないこと」
ではなく「extern がメモリを読めないこと」**。

検討した案:

| 案 | 内容 | 判定 |
|---|---|---|
| (a) `str` で運ぶ | `recv(fd, max) -> str` | **却下**。interpreter の `str` は Rust の `String` = UTF-8 必須。compiled レーンの `[bytes][NUL][u64 len]` は元からバイナリ安全なので、**この案はレーン間で答えが割れる** (`read_file` の非 UTF-8 が interpreter でだけ read error になるのと同じ穴を、ソケットで日常的に踏む) |
| (b) 1 バイトずつの extern | `recv_byte(i) -> u8` | **却下**。正しいが 64KB で 65,536 回。tree-walker では実用にならない |
| (c) **registry にコンテキストを渡す** | `ExternBufFn = fn(&mut EvaluationContext, &[Value])` を 2 本目の registry として足す | **採用** (2026-08-31 landing) |

`dispatch_extern_fn` は既に `&mut self` を持ち、`ConstString` を
`Object::String` へ正規化してから registry を呼んでいる。同じ場所で
2 本目の表を引くだけなので、変更は 1 箇所に収まる。compiled レーンは
**既に `ptr` 引数の extern を通す** (FFI P1 の型制約で許可済み、
`ffi_tests.rs` が 3 レーンで pin) ので、作業は tree-walker だけ。

**CONV-SPAN のおかげで EXTERN-BUF の表面は 2 つに縮む** — 「範囲を
読みで借りる」と「書きで借りる」だけ。**コピーする API にしない**のが
要点で (§5)、`HeapManager` の
`get_memory_slice` / `get_memory_slice_mut` を `pub(crate)` にすれば
そのまま使える (どちらも囲む allocation に対して境界検査済み)。

**注意点が 1 つある**: `HeapManager` は生バイト列と `typed_slots` の
二重表現を持ち、読み出しは typed slot を優先する。extern が生バイトを
書き込んだ範囲に**古い typed slot が残っていると、書いたはずのバイトが
読めない**。したがって書き込みヘルパは

1. `write_bytes_raw` で生バイトを stamp し、
2. **同じ範囲の typed slot を無効化する**

の 2 段でなければならない。`copy_memory` が typed slot を範囲コピー
しているのと対になる操作で、この 1 点を落とすと
「`__builtin_str_from_bytes` が 5 個の NUL を返した」のと同型のバグに
なる (heap.rs のコメントに前例が記録されている)。

### 決定: バイト列を `str` に住まわせない

(a) を却下した理由の裏返しとして、**API の規約**を 1 つ置く:

> **受信したバイト列は `Span<u8>` / `Vec<u8>` に入る。`str` への変換は
> ユーザが「これはテキストだ」と知っているときに明示的に行う**
> (`__builtin_str_from_bytes`、`unsafe fn` を要求する既存の builtin)。

`recv` が `str` を返す API にしなかったのはこのため。この規約を守る限り、
**interpreter の `str` が UTF-8 必須であることは観測されない**。


## 5. 確保しない・コピーしない recv / send

**要件**: (a) 呼ばれた側 (runtime) が呼び出しごとに確保しない、
(b) 呼び出し元 (app) が確保しない、(c) **呼び出し元と呼び出し先の間で
バイト列をコピーしない**、(d) アプリ実装が zero-copy を選べる。

**結論から言うと 4 つとも満たせる** — しかも **4 実行レーンすべてで**
(下の「まとめ」の表)。鍵は 2 つで、**API がバッファを呼び出し側に
持たせること**と、**extern 境界を「コピー」ではなく「借用」で
設計すること**。

### 決定: 呼び出し側が持つバッファを埋める

```rust
fn read(&self, buf: Span<u8>) -> Result<u64, NetError>    # 返すのはバイト数だけ
```

バッファの所有者は常にアプリで、runtime も stdlib も**確保しない**。
検討して却下した対案:

| 案 | なぜ却下したか |
|---|---|
| `recv() -> Vec<u8>` | 呼ぶたびに確保する。素直な API なので、**却下したことを明記しておく** |
| `recv_borrowed() -> Span<u8>` (runtime 所有バッファへの窓、次の `recv` まで有効) | io_uring の provided buffer 相当。**interpreter で表現できない** — 窓の番地が runtime のホスト番地になり、tree-walker の `ptr` (`HeapManager` の index) と互換が無い。レーンで答えが割れる。**コピー回数では採用案と並ぶ (どちらも 0) ので、失うものが無い** — 「バッファを誰が持つか」だけの違いで、呼び出し側所有のほうが寿命が明確 |

### 確保が起きうる 4 箇所と、それぞれの潰し方

| 箇所 | 素朴な実装だとどうなるか | 決定 |
|---|---|---|
| runtime の受信バッファ (compiled レーン) | recv ごとに malloc して staging に受け、呼び出し側へコピー | **確保もコピーもしない** — `(ptr, len)` が実番地なので `recv(2)` が**呼び出し側のバッファへ直接書く** |
| runtime の受信バッファ (tree-walker) | 同上 | **こちらも確保もコピーもしない** — 下の「借用」で compiled と同じ形にする |
| アプリのバッファ | ループ内で `Vec::new()` して push | `Vec::with_capacity(n)` で**一度だけ**確保し、ループを跨いで使い回す |
| アプリのメッセージ切り出し | 部分列を新しい `Vec` にコピー | `Span::slice(offset, len)` で**窓を作るだけ** (コピーなし) |

### tree-walker も 0 コピーにする — 「コピー」ではなく「借用」

初稿はここを「`HeapManager` にホスト番地から書けないのでコピー 1 回は
残る」としていたが、**それは EXTERN-BUF を「範囲を読む / 書く」という
コピー API として設計していたから**であって、制約ではなかった。

`HeapManager::memory` は**連続した実体の `Vec<u8>`** で、toylang の
`ptr` からホスト番地への写像は `memory_offset = addr - 1` の 1 行。
つまり `(addr, len)` から**実際のバイト列への `&mut [u8]` が取れる**。
しかも**その関数は既にある**:

```rust
// interpreter/src/heap.rs — already bounds-checks against the
// *enclosing allocation* (resolve_block), not just the byte vec, so a
// bad (ptr, len) is a toylang-level error instead of clobbering the
// neighbouring block. Currently private; EXTERN-BUF exposes it.
fn get_memory_slice(&self, addr: usize, size: usize) -> Option<&[u8]>
fn get_memory_slice_mut(&mut self, addr: usize, size: usize) -> Option<&mut [u8]>
```

したがって tree-walker の `recv` は

```rust
let buf = heap.get_memory_slice_mut(addr, len)?;   // borrow the toylang buffer
sys::recv(fd, buf.as_mut_ptr(), buf.len())         // the OS writes into it directly
```

と書ける — **compiled レーンと文字どおり同じ呼び出し**になる。
staging バッファは net には存在しない。

**借用が満たすべき条件** (すべて自然に満たされる):

- **借用を呼び出しより長く持たない** — `memory` は toylang の確保で
  伸びうる `Vec` なので、跨いで保持すると dangling になる。
  `recv(2)` はインタプリタに再入しないので、借用は syscall の間だけ。
  Rust のライフタイムがそのまま制約になる
- **範囲は囲む allocation の内側** — `resolve_block` 経由なので、
  隣のブロックへはみ出す `(ptr, len)` は `None` になり
  toylang のエラーとして返せる
- **書いた範囲の typed slot を無効化する** (§4 の注意点と同じ)。
  OS が書いたバイトは typed slot を通っていないので、古い slot が
  残っていると読み出しがそちらを優先してしまう

### そのために足りていない stdlib (N0.5 で landing 済み)

CONV-SPAN と同じく **net と独立に価値がある**もので、2026-08-31 に入った:

```rust
# core/std/collections/vec.t — `Vec::new()` しか無く、事前確保できない。
# 受信ループでは「一度確保して使い回す」が基本形なので必須。
fn with_capacity(n: u64) -> Self

# recv が埋めたバイトを Vec に知らせる口。これが無いと、バッファは
# 埋まっているのに `size()` が 0 のままになる (push を通っていないため)。
unsafe fn set_size(&mut self, n: u64)

# core/std/span.t — 部分窓。アプリ側 zero-copy の本体。
fn slice(&self, offset: u64, len: u64) -> Span<T>   # 範囲外は panic

# 受信の宛先は「live 要素」ではなく「確保済みの空き」なので、窓は 2 つ。
fn capacity_span(&self) -> Option<Span<T>>          # cap 要素ぶん
```

`Span::slice` が無いと、「1 回の recv で届いた 2 つのメッセージを
別々に処理する」が**必ずコピーになる**。逆にこれがあれば、パーサに
`Span<u8>` を渡す限りアプリ側の確保は 0 回で書ける。

### 「確保しない」をコメントではなく検査にする

net の extern はすべて **`never_allocates extern fn` で宣言する**
(toylang ヒープを一切触らないので正当)。stdlib のラッパも
`never_allocates fn` で書く。NEVER-ALLOCATES (`E0016`) は
**到達可能性で検査する**ので、これが通れば

```rust
never_allocates fn serve(poller: &Poller, buf: Span<u8>) -> u64 { ... }
```

と書いたユーザのイベントループ全体が、**確保が紛れ込んだ瞬間に
コンパイルエラーになる**。テストでは `ensures allocations(0)` と
`--profile=mem` で実測を pin する。本設計が偶然得た最良の性質で、
docs の例はこの形で書く。

### バッファは動かない — 渡すのは窓だけ

受信ループでバッファを使い回すとき、**`Vec<u8>` は所有したまま、
関数に渡すのは `Span<u8>`** にする。`Vec<T>` は `impl Drop` を持つので
値渡しすると**所有権が移り (`E0014`)、ループの次の周回で使えなくなる**。
`Span` は窓なので移動が起きない。

```rust
never_allocates fn serve(stream: &TcpStream, buf: Span<u8>) -> Result<u64, NetError> {
    val n = stream.read(buf)?          # the OS writes straight into the caller's bytes
    stream.write(buf.slice(0u64, n))   # and reads straight back out of them
}

fn main() -> u64 {
    var buf: Vec<u8> = Vec::with_capacity(65536u64)   # allocated once
    # ... loop: serve(&stream, buf.as_span()) ...     # nothing allocated per message
    0u64
}
```

### 分散書き込み (writev / readv) — 後回しにするが、道は塞がない

「ヘッダと本体を連結せずに 1 回で送る」は send 側の zero-copy として
最も効く形で、`writev(2)` / `readv(2)` の `struct iovec` がそれにあたる。
**`iovec` は `{ void *iov_base; size_t iov_len }` で、compiled レーンの
`Span<u8>` の leaf 並び (ptr, u64) と同じ**なので、`Span<Span<u8>>` を
そのまま iovec 配列として渡せる可能性がある。

ただし **tree-walker では `Span<u8>` は `Object::Struct` であって
メモリ上の (ptr, len) 対ではない**ので、外側 span を歩いて iovec 配列を
組む必要がある (借用と違い、ここは小さな確保が要る)。レーンで確保回数が
変わるため、`never_allocates` の検査結果まで割れる。

**決定: MVP には入れない。**「連結を避けたい」場面が実プログラムで
出てから、`write_vectored` として設計する。ここで記録しておくのは、
**現在の `Span` 中心の API がその形をそのまま受けられる**ことと、
tree-walker が唯一の障害であることの 2 点。

### 送信側と `str` の注意

送信も同じ形 (`write(buf: Span<u8>)`) で、compiled レーンは番地を
そのまま渡すのでコピーが無い。ただし **`__builtin_str_to_ptr` は
interpreter で len+1 バイトを確保してコピーする** (`builtin.rs` の
実装コメントのとおり)。`str` を送るときは
**`String` に持ってから `as_span()`** を使うこと — `String::as_ptr()` は
自分のバッファを指すのでコピーが無い。

なお tree-walker では `Span<u8>` の構築そのものが `Object::Struct` の
確保になるが、これは**インタプリタの帳簿であって toylang のカウンタには
出ない** (MEM-COUNTER-INTERP-DRIFT で固定した定義)。

### まとめ — バイト列がコピーされる回数

| 経路 | AOT / compiler JIT | tree-walker |
|---|---|---|
| `read` (OS → アプリのバッファ) | **0** | **0** (借用) |
| `write` (アプリのバッファ → OS) | **0** | **0** (借用) |
| メッセージの切り出し (`Span::slice`) | **0** | **0** |
| `str` の送信 (`__builtin_str_to_ptr` 経由) | 0 | 1 (`String::as_span()` を使えば 0) |
| `local_addr` / `peer_addr` | 1 (str を作る) | 1 (同左、ホットパスではない) |

**受信ループの定常状態でバイト列のコピーは全レーンで 0 回**、確保も
`Vec::with_capacity` の 1 回きり。これが `never_allocates` +
`ensures allocations(0)` で検査・実測できる (上)。

## 6. errno の OS 差 — 表を cfg で分ける

既存の `io_status_from_errno` は「ENOENT 2 / EPERM 1 / EACCES 13 /
EISDIR 21 は macOS と Linux で一致する」ことに依存している。
**ネットワーク系は一致しない。**

| errno | Linux | macOS/BSD | `NetError` |
|---|---|---|---|
| EINTR | 4 | 4 | `Interrupted` |
| EAGAIN / EWOULDBLOCK | 11 | 35 | `WouldBlock` |
| EINVAL | 22 | 22 | `InvalidInput` |
| EMFILE | 24 | 24 | `TooManyOpenFiles` |
| EPIPE | 32 | 32 | `BrokenPipe` |
| EADDRINUSE | 98 | 48 | `AddrInUse` |
| EADDRNOTAVAIL | 99 | 49 | `AddrNotAvailable` |
| ENETUNREACH | 101 | 51 | `NetworkUnreachable` |
| ECONNABORTED | 103 | 53 | `ConnectionAborted` |
| ECONNRESET | 104 | 54 | `ConnectionReset` |
| ENOTCONN | 107 | 57 | `NotConnected` |
| ETIMEDOUT | 110 | 60 | `TimedOut` |
| ECONNREFUSED | 111 | 61 | `ConnectionRefused` |
| EHOSTUNREACH | 113 | 65 | `HostUnreachable` |
| EINPROGRESS | 115 | 36 | `InProgress` |

つまり `status_from_errno` は **`sys` 側に置く** (§2 の契約に入っている
のはこのため)。表の値は ABI probe テストで固定する — 手書きの数値表を
テスト無しで信じないこと。

**status コードの語彙 (toylang 側に渡る u64) は OS 非依存**で、
`NET_OK = 0` から始まる連番。`core/std/net.t` の
`net_error_from_status` が `NetError` に写す。`io.t` の
`io_error_from_status` と同じ形。

## 7. 各レーンの分担

| レーン | 必要な作業 |
|---|---|
| tree-walker (interpreter) | `extern_net.rs` registry。**`toylang_rt` の `toy_net_*` を直接呼ぶ** — interpreter は既に `toylang_rt` に依存しているので、`io` のように std で書き直さない。fd 番号や errno 分類が実装ごとに割れるのを構造的に防げる (RUNTIME-IO は同じ語彙を 2 実装で維持していて、コメントで「同じ数字を出す」と約束している。net でそれをやる理由はない)。**バッファは `get_memory_slice_mut` で借用して渡す** (§5) ので、compiled レーンと同じ syscall 呼び出しになりコピーが無い |
| IR VM | **なし** — extern 到達で不適格になり tree-walker に落ちる |
| compiler JIT | `jit.rs` の symbol map に `toy_net_*` を登録する行を追加 |
| AOT | **なし** — staticlib に含まれるのでリンクで解決する |
| interpreter JIT | **なし** — silent fallback |

## 8. Phase 分割と受け入れ基準

| Phase | 内容 | 受け入れ基準 |
|---|---|---|
| **N0** | `mod sys` 切り替え + `sys_epoll.rs` / `sys_kqueue.rs` の骨、`build.rs` の rerun 修正、ABI probe テスト、`net::backend_name()` | probe テストが green。3 レーンが `backend_name()` に同じ答えを返す |
| **N0.5** ✅ | CONV-SPAN + `Span::slice` / `Vec::with_capacity` / `Vec::set_size` / `Vec::capacity_span` | **完了 (2026-08-31)**。net と独立に landing した。`compiler/tests/consistency/conv_span.rs` が 5 件 pin: slice が窓であって複製でないこと、null に窓が無いこと、確保済みの空きに書いてから `set_size` で live にする形、`Vec::new()` と「確保済みで空」の区別、`String` のバイト列の書き換えが元に通ること |
| **N1** | EXTERN-BUF + TCP client + `NetError` + `set_blocking` | テスト側が Rust の `std::net` でエコーサーバを立て、toylang が接続して往復。3 レーン一致。**バイト列が `str` を経由しない**ことを非 UTF-8 のペイロードで pin。**blocking と nonblocking の両方で同じ答え**になること。`ensures allocations(0)` で受信ループが確保しないことを pin (**コピー回数は観測できないので、確保 0 と「借用で書いた」ことをコードレビューで担保する**) |
| **N2** | `bind` / `listen` / `accept` / nonblocking | **1 プロセス内で自己完結**: 同じプログラムが listener と client を持ち、nonblocking で往復する。外部の peer が要らないので完全に決定的 |
| **N3** | Poller | [`EVENT_POLLING.md`](EVENT_POLLING.md) |
| **N4** | UDP / socket option / `local_addr` / `peer_addr` | 同上の自己完結形 |
| **N5** | `getaddrinfo` | `localhost` の解決のみ pin (DNS は非決定なのでテストしない) |

### テストの決定性

- **loopback (127.0.0.1) のみ**。外部への接続はテストしない。
- **ポートは必ず 0 (ephemeral) で bind し `local_port()` で読み戻す**。
  固定ポートは並列テスト同士で衝突する (`cargo nextest` は 1 テスト
  1 プロセスで並列に走る)。
- **タイムアウト値は pin しない**。pin するのは「何が起きたか」
  (受信バイト列、`NetError` の variant、接続の成否) だけ。
- `compiler/tests/consistency/net.rs` を新設し、`mod.rs` に登録する
  (`tests/suite.rs` への登録も忘れないこと — 忘れると**無音で実行され
  ない**)。

## 9. 論点と決定

> 5 件のうち **4 件は決定済み (2026-08-31)**。未決のまま残すのは
> **論点 3 (`Interest` の型) だけ**で、これは N3 の実装時に判断する。

1. **blocking API を出すか** — **決定: 出す (2026-08-31)**。
   ただし**別の API 表面としては作らない** — blocking かどうかは
   **fd の属性**であって呼び出しの種類ではないので、
   `set_blocking(&self, on: bool)` (`fcntl(O_NONBLOCK)`) 1 つで切り替え、
   `read` / `write` / `accept` / `connect` は同じ関数のままにする。
   これで CLI クライアントは Poller を知らずに書け、サーバは
   nonblocking + Poller で書ける。付随する決定:
   * **既定は nonblocking** — `connect` / `bind` が返す fd は
     nonblocking。blocking が要る側が明示的に切り替える (逆にすると、
     切り替え忘れがサーバ全体のハングとして出る)
   * **無期限に止まるのを避ける口を同時に出す** —
     `set_read_timeout(ms)` / `set_write_timeout(ms)`
     (`SO_RCVTIMEO` / `SO_SNDTIMEO`)。超過は `Err(NetError::TimedOut)`。
     **定数の値も `struct timeval` の形も OS で違う**ので `sys` の
     契約に入れる (§2)
   * **blocking では `WouldBlock` / `InProgress` が返らない** —
     `NetError` の variant は共通のままで、現れない腕があるだけ。
     match が壊れない
   * **`EINTR` は隠さない** (EVENT_POLLING 決定 4 と同じ)。blocking では
     現れやすくなるので、docs の例に retry ループを載せる
   * **1 スレッドしかないことを明記する** — blocking `read` は
     **プログラム全体を止める**。サーバを書くなら nonblocking + Poller、
     という誘導を `core/std/net.t` のヘッダに書く
2. **fd の所有権と二重 close** — `impl Drop` で fd を閉じると move
   検査 (E0014) が効いて RAII になる。ただし heap と違い
   **close は冪等ではない** (閉じた番号は再利用され、他人の fd を
   閉じうる)。`val b = a` が alias である以上、drop glue が 2 回走る形を
   作れてしまう。**対策: `closed: bool` を持ち、`close` は 2 回目を
   no-op にする**。これで toylang 側からは冪等に見える。
   `io::exit` は Drop を走らせないが、プロセス終了で OS が閉じるので
   実害はない。
3. **`Interest` を struct + 演算子オーバーロードにするか** —
   `Interest::readable() | Interest::writable()` は魅力的だが、
   **compiled レーンでは let-rhs 位置の 1 演算しか通らない**
   (OP-OVERLOAD-CHAIN)。3 つ以上の `|` を書いた瞬間 interpreter では
   動いて AOT で落ちる。**決定: 未決のままとする (2026-08-31)**。MVP は素の
   `u32` + 名前つき `const` で進め、**struct 化するかは N3 の実装時に
   持ち越す** — それまでに OP-OVERLOAD-CHAIN が解ければ struct が素直
   だし、解けていなければ `u32` のままで困らない。どちらでも影響は
   `poll.t` の中に閉じる (`register` の引数型が変わるだけ) ので、
   ここで決め切る利益が無い。
4. **IPv6** — **決定: 実装は IPv4 のみ。ただし API を変えずに足せる形に
   今しておく (2026-08-31)**。当初案の「アドレスを u32 スカラで extern に
   渡す」は 128bit で破れるので、**設計を先に変える**:
   * **extern 境界はアドレスを `str` で運ぶ** (`"127.0.0.1"`、将来の
     `"::1"`)。文字列 → sockaddr の変換は `sys` の中
     (`sockaddr_from_str`)。`str` は既に境界を越えられる (io.t が
     やっている) ので追加の機構は要らない
   * **`family` を今からすべての経路に通す** — `socket_stream(family)` /
     `sockaddr_from_str(text, port, family)`。今は `AF_INET` 固定だが、
     **シグネチャが将来変わらないこと**が目的
   * `AF_INET6` を足す作業は「`sys_*.rs` に定数 1 つ + `sockaddr_in6` の
     構築 + probe テストの行追加」に閉じる。`net.t` も `NetError` も
     変わらない
   * `local_addr()` / `peer_addr()` は `str` を返す (確保が要るが
     ホットパスではない — §5 の規律の明示的な例外として書く)
5. **`--profile=mem` との関係** — socket バッファは Rust 側の malloc で、
   toylang の bump ヒープを通らないのでカウンタに出ない。これは
   「ランタイムが str を保持するために使うメモリは数えない」
   (MEM-COUNTER-INTERP-DRIFT) と同じ扱いで一貫している。docs に 1 行書く。

## 10. 非目標

- **async / await、Future、グリーンスレッド** — 言語に並行性が入る
  前にランタイムだけ先行させない。イベントループはユーザが `while` で
  書く。
- **TLS** — 本設計 (平文の socket とイベント通知) の範囲外。
  ここで方針を決め打たない — 既存ライブラリを FFI P2 (dlopen) で
  呼ぶ形も、toylang / `toylang_rt` 側に実装する形も、どちらも
  塞がっていない。着手するなら独立した設計文書を取る。
- **Windows (IOCP)** — `compile_error!` で明示的に落とす。完成度の
  見かけを上げるために動かない分岐を置かない。
- **プロセス spawn / fork** — bump ヒープ (never-reuse) と fork の
  相性を検討していない (RUNTIME_LIBRARY の非目標のまま)。
- **接続プール / HTTP パーサ等の上物** — stdlib ではなくユーザ空間。

## 関連

- [`EVENT_POLLING.md`](EVENT_POLLING.md) — epoll / kqueue の統一形 (本文書の N3)
- [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) — stdlib 全体の優先順位。本文書は
  そこの非目標「ネットワーク」に対する再検討
- [`FFI_PLAN.md`](FFI_PLAN.md) — `extern fn ... from "lib" as "sym"` の機構
- [`RUNTIME_PORT.md`](RUNTIME_PORT.md) — `toylang_rt` を 1 実装に集約した経緯
- [`BUILTIN_ARCHITECTURE.md`](BUILTIN_ARCHITECTURE.md) — builtin ではなく
  extern を選んだ理由の背景
- [`EFFECT_SYSTEM.md`](EFFECT_SYSTEM.md) — extern のエフェクト申告
- [`POINTER.md`](POINTER.md) — `Ptr<T>` / `Span<T>` (バッファの受け口)
