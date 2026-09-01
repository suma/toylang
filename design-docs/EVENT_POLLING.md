# EVENT_POLLING.md — epoll / kqueue の統一形

> **状態: 未着手 (設計のみ)**。[`NETWORK_IO.md`](NETWORK_IO.md) の
> **Phase N3**。socket 側の設計・プラットフォーム切り替えの機構
> (`#[cfg_attr(path)] mod sys;`)・extern 境界の制約は同文書にあり、
> 本文書はそれを**前提として引く**。分けたのは、2 つの API の意味論差が
> socket 本体より大きく、決定を 1 箇所にまとめておきたいため。

## Status

**実装済み (2026-09-01)。** `core/std/poll.t` に `Poller` / `Event`、
runtime に `poll_create` / `poll_ctl` / `poll_wait` +
`poll_event_{token,flags,error}`、`sys_kqueue.rs` / `sys_epoll.rs` に
各バックエンドの写像。決定 1〜6 はすべてそのまま実装されている。
`compiler/tests/consistency/net.rs` が **4 レーンで** 3 件 pin
(イベントループ一巡、read+write が 1 イベントにマージされること、
peer が閉じても bytes が残っていること)。すべて 1 プロセスで自己完結する。

設計から変えた点が 1 つ: **interest フラグは `pub const` ではなく
`pub fn`**。モジュールの top-level `const` は他モジュールから見えず、
**自モジュールの関数本体からも見えない** (todo: MODULE-CONST)。

## 1. なぜ統一が難しいか

epoll と kqueue は「fd を待つ」という目的が同じだけで、**登録の単位が
違う**。ここが全体を決める。

| 観点 | epoll (Linux) | kqueue (BSD/macOS) |
|---|---|---|
| 登録の単位 | **fd 1 つに 1 エントリ**、interest は bitmask | **(ident, filter) のペア**。read と write は**別々の kevent** |
| 追加 / 変更 / 削除 | `epoll_ctl(ADD / MOD / DEL)` を 1 回ずつ | `kevent()` の changelist に `EV_ADD` / `EV_DELETE` を積む |
| 待機 | `epoll_wait(epfd, events, max, timeout_ms)` | `kevent()` が changelist と eventlist を**同時に**扱う |
| ユーザデータ | `epoll_data` の `u64` | `udata` (`void*`) |
| edge trigger | `EPOLLET` | `EV_CLEAR` |
| oneshot | `EPOLLONESHOT` | `EV_ONESHOT` |
| 切断 | `EPOLLHUP` / `EPOLLRDHUP` | `EV_EOF` (read/write どちらの filter にも立つ) |
| エラー | `EPOLLERR` (詳細は `SO_ERROR` で取る) | `EV_ERROR` + `data` に **errno そのもの** |
| 追加時の失敗 | `epoll_ctl` の戻り値で即座に分かる | **eventlist に `EV_ERROR` として返る** (非同期に見える) |
| timeout | `i32` のミリ秒、`-1` で無限 | `timespec*`、`NULL` で無限 |
| 1 回の wait で同じ fd | mask を合成した **1 イベント** | read と write が **2 イベント**として返りうる |
| fork | epoll fd は継承される | **kqueue fd は継承されない** |
| 待てるもの | fd (+ eventfd / timerfd / signalfd) | fd + タイマ + シグナル + プロセス + vnode |

**最後から 3 行目 (1 fd = 1 イベント か 2 イベントか) が本設計最大の
分岐点**で、これを決めないと同じ toylang プログラムが OS で違う回数
ループする。

## 2. 統一形の決定

### 決定 1: **1 回の `wait` で 1 fd につき 1 イベント。フラグは合成する**

kqueue 側が同一 ident の read/write を別々に返すので、**runtime が
マージする**。mio は逆の選択 (マージしない) をしているが、それは
mio の利用者がライブラリ作者だからで、toylang のユーザが
`while` ループを直接書くことを考えると、

- **macOS で書いたループが Linux で同じ回数だけ回る**ことのほうが価値が
  高い、
- マージしないと「同じ fd のイベントを 2 回受け取り、1 回目の処理で
  close したのに 2 回目が来る」という use-after-close をユーザが自分で
  防ぐ必要がある、

の 2 点で**マージを選ぶ**。代償は wait ごとの線形走査 (返ってきた
イベント数 n に対し、staging 配列を舐めて同一 ident を畳む O(n) —
n は 1 回の wait の返却数なので実質無視できる)。

### 決定 2: **token は `u64`**

`epoll_data.u64` と `udata` (64bit ポインタ幅) の両方が u64 を運べる。
fd をそのまま token にしてもよいし、ユーザ側の配列 index を入れても
よい。**runtime は token を解釈しない**。

### 決定 3: **level-triggered を既定にする**

edge-trigger は「readable が来たら EAGAIN が返るまで読み切る」という
規律をユーザに強制する。守り損ねると**接続が静かにハングする**
(バグの中でも最も追いにくい部類)。level なら 1 回に読める分だけ読んで
戻ってよい。`Interest::EDGE` を明示したときだけ edge にする。

### 決定 4: **`EINTR` は隠さず `Interrupted` を返す**

内部でリトライすると、シグナルでループを抜ける手段がユーザから消える。
`IoError` / `NetError` が「起きたことをそのまま名前で返す」設計なので、
それに揃える。

### 決定 5: **イベント配列は runtime 側に置き、index で読み出す**

`NETWORK_IO.md` の制約どおり extern 境界はポインタを deref できない。
epoll / kqueue はイベント**配列**を返すので、そのままでは渡せない。

RUNTIME-IO の status ペア方式をそのまま拡張して解く:

```
1. toy_poll_wait(pfd, timeout_ms) -> i64     # 返り値 = イベント数 (負なら失敗)
2. toy_poll_event_token(i) -> u64            # i 番目の token
3. toy_poll_event_flags(i) -> u32            # i 番目のフラグ合成
4. toy_poll_event_error(i) -> u64            # i 番目の errno (無ければ 0)
```

`wait` の直後に読み出す限り、toylang から見て**この列はアトミック**
(間に別の toylang コードが走らない) — `read_file` + `read_file_status`
と同じ理屈。**この設計の利点は、EXTERN-BUF (extern がヒープに触れる
拡張) を待たずに Poller が実装できる**ことで、N3 を N1 と独立に
進められる。

コストは「イベント 1 つあたり extern 3 回」。1 回の wait で 100 イベント
返っても 300 回で、`epoll_wait` 自体のコストに対して無視できる。
staging バッファは per-thread state に置く (出力シンクと同じ場所)。

### 決定 6: **`add` は失敗を即座に返す**

kqueue は changelist のエラーを eventlist に `EV_ERROR` で返す。
これを toylang に伝えると「`add` は成功したように見えたが後で
エラーイベントが来る」という epoll と違う世界になる。したがって
**`sys::poll_ctl` は changelist を積んだ直後に `nevents=1` の
`kevent()` を呼んで `EV_ERROR` を回収し、その場で失敗として返す**
(epoll の `epoll_ctl` と同じ同期的な失敗になる)。

## 3. `sys` が提供する名前 (移植の契約)

`NETWORK_IO.md` §2 の socket 側と合わせて、これが `sys_epoll.rs` /
`sys_kqueue.rs` の全表面。

```rust
// Interest / event flags. Platform-neutral values defined *here*, not
// the raw EPOLL* / EV* numbers — the mapping is each backend's job.
pub const INTEREST_READ:    u32 = 1 << 0;
pub const INTEREST_WRITE:   u32 = 1 << 1;
pub const INTEREST_EDGE:    u32 = 1 << 2;
pub const INTEREST_ONESHOT: u32 = 1 << 3;

pub const EVENT_READ:  u32 = 1 << 0;
pub const EVENT_WRITE: u32 = 1 << 1;
pub const EVENT_HUP:   u32 = 1 << 2;   // peer closed / EV_EOF / EPOLLHUP
pub const EVENT_ERROR: u32 = 1 << 3;   // EPOLLERR / EV_ERROR

/// Create the poller fd (close-on-exec). Negative on failure.
pub fn poll_create() -> i32;

/// Register / modify / unregister `fd`. `interest == 0` unregisters.
/// Returns 0 or a negative errno. Synchronous failure on both
/// backends (決定 6).
pub fn poll_ctl(pfd: i32, fd: i32, token: u64, interest: u32) -> i32;

/// Wait, writing at most `cap` merged events into `out`. Returns the
/// event count, or a negative errno. Merging (決定 1) happens here so
/// `poll.rs` above never sees the per-filter shape.
pub fn poll_wait(pfd: i32, out: &mut [RawEvent], cap: usize, timeout_ms: i64) -> isize;

/// The merged, platform-neutral event the staging buffer holds.
pub struct RawEvent { pub token: u64, pub flags: u32, pub error: u32 }
```

`poll.rs` はこの 4 つしか呼ばない。`EPOLLIN` も `EVFILT_READ` も
`poll.rs` には出てこない。

## 4. 各バックエンドの写像

### epoll (`sys_epoll.rs`)

```rust
// struct epoll_event is **packed on x86_64 only** (glibc's EPOLL_PACKED):
//   x86_64  : { u32 events; u64 data; }  packed  -> 12 bytes
//   aarch64 : { u32 events; u64 data; }  natural -> 16 bytes
// Getting this wrong shifts every `data` field by 4 bytes and the
// tokens come back as garbage — the ABI probe test pins the size.
#[cfg(target_arch = "x86_64")] #[repr(C, packed)] struct EpollEvent { events: u32, data: u64 }
#[cfg(not(target_arch = "x86_64"))] #[repr(C)]    struct EpollEvent { events: u32, data: u64 }
```

| 統一形 | epoll での実装 |
|---|---|
| `poll_create` | `epoll_create1(EPOLL_CLOEXEC)` |
| `poll_ctl` (新規) | `epoll_ctl(EPOLL_CTL_ADD, ...)`。既に居れば `EEXIST` → `MOD` で再試行 |
| `poll_ctl` (interest 0) | `epoll_ctl(EPOLL_CTL_DEL, ...)` |
| `INTEREST_READ` | `EPOLLIN` (0x001) |
| `INTEREST_WRITE` | `EPOLLOUT` (0x004) |
| `INTEREST_EDGE` | `EPOLLET` (1<<31) |
| `INTEREST_ONESHOT` | `EPOLLONESHOT` (1<<30) |
| `poll_wait` | `epoll_wait(pfd, buf, cap, timeout_ms as i32)`。**マージ不要** (元から 1 fd 1 イベント) |
| `EVENT_HUP` | `EPOLLHUP` (0x010) または `EPOLLRDHUP` (0x2000) |
| `EVENT_ERROR` | `EPOLLERR` (0x008)。errno は `getsockopt(SO_ERROR)` で取る |

epoll は登録が「fd 単位の 1 エントリ」なので、統一形とほぼ 1 対 1。
唯一の作業は `ADD` と `MOD` の使い分けを吸収すること
(統一形の `poll_ctl` は「この fd の interest をこれにする」という
冪等な意味なので、`EEXIST` を見て `MOD` に落とす)。

### kqueue (`sys_kqueue.rs`)

```rust
// macOS / FreeBSD (11 以前の互換構造体) の kevent。
//   { ident: usize, filter: i16, flags: u16, fflags: u32, data: isize, udata: *mut c_void }
// = 32 bytes on LP64. FreeBSD 12+ の `struct kevent` は末尾に ext[4] を
// 持つので、その OS を足すときはここを cfg で分ける (MVP は macOS)。
```

| 統一形 | kqueue での実装 |
|---|---|
| `poll_create` | `kqueue()` + `fcntl(FD_CLOEXEC)` (`kqueue()` に CLOEXEC 版が無い) |
| `poll_ctl` | **最大 2 件**の changelist を組む。READ が要れば `(fd, EVFILT_READ, EV_ADD)`、要らなければ `EV_DELETE`。WRITE も同様。`udata = token` |
| `poll_ctl` (interest 0) | 両 filter に `EV_DELETE` (既に無ければ `ENOENT` は握り潰す) |
| `INTEREST_EDGE` | `EV_CLEAR` を両 filter の flags に |
| `INTEREST_ONESHOT` | `EV_ONESHOT` |
| `poll_wait` | `kevent(pfd, NULL, 0, buf, cap, &timespec)`。**この後にマージ** (決定 1) |
| `EVENT_HUP` | `flags & EV_EOF` (0x8000) |
| `EVENT_ERROR` | `flags & EV_ERROR` (0x4000)。**errno は `data` にそのまま入っている** — epoll と違い `SO_ERROR` を引かなくてよい |
| 失敗の同期化 | changelist 投入時に `nevents=1` で `EV_ERROR` を回収 (決定 6) |

**マージの実装** — `kevent()` の返却は最大 `cap` 件で、同一 ident が
2 件来るのは同じ fd の read と write が同時に立ったときだけ。返却順は
登録順に近いが保証は無いので、staging に詰めながら**直近に見た token を
線形に探す** (返却数は通常 1〜数十なので、HashMap は割に合わない)。

`cap` の意味が epoll と揃わない点に注意: kqueue は**マージ前**の数で
上限がかかるので、`cap` 件の統一イベントを保証するには
`kevent()` に `2 * cap` を渡す必要がある。staging バッファは
`2 * cap` で確保する。

## 5. toylang から見える API (`core/std/poll.t`)

```rust
# Platform-neutral interest flags. Plain u32 rather than a struct with
# an overloaded `|`: operator overloading only lowers in let-rhs
# position on the compiled lanes (todo OP-OVERLOAD-CHAIN), so
# `READABLE | WRITABLE | EDGE` would work in the interpreter and fail
# in AOT. See NETWORK_IO.md 論点 3.
pub const INTEREST_READ: u32 = 1u32
pub const INTEREST_WRITE: u32 = 2u32
pub const INTEREST_EDGE: u32 = 4u32
pub const INTEREST_ONESHOT: u32 = 8u32

# One ready file descriptor. `token` is whatever was handed to
# `register`; the poller never interprets it.
struct Event { token: u64, flags: u32, error: u32 }

impl Event {
    fn is_readable(&self) -> bool
    fn is_writable(&self) -> bool
    # The peer closed its end (EPOLLHUP / EPOLLRDHUP / EV_EOF). Still
    # readable: drain what is buffered before closing.
    fn is_hup(&self) -> bool
    fn is_error(&self) -> bool
    # The errno behind `is_error()`, as a NetError. Unknown when the
    # platform gave no code.
    fn error_reason(&self) -> NetError
}

struct Poller { fd: i32, closed: bool }

impl Poller {
    fn new() -> Result<Poller, NetError>
    # Idempotent: registering an already-registered fd replaces its
    # interest (epoll's ADD/MOD split is hidden). `interest == 0`
    # unregisters — or call `deregister`.
    fn register(&self, fd: i32, token: u64, interest: u32) -> Result<(), NetError>
    fn deregister(&self, fd: i32) -> Result<(), NetError>
    # Block until at least one fd is ready, `timeout_ms` elapses
    # (0 = poll, negative = forever), or a signal arrives
    # (`Err(NetError::Interrupted)`). Returns how many events are
    # ready; read them with `event(i)`.
    fn wait(&self, timeout_ms: i64) -> Result<u64, NetError>
    # The i-th event of the most recent `wait`. Valid until the next
    # `wait` on this thread.
    fn event(&self, i: u64) -> Event
    fn close(&mut self) -> Result<(), NetError>
}
```

`wait` → `event(i)` の 2 段は決定 5 の直接の帰結。「次の `wait` まで
有効」という寿命はドキュメント上の規約であって検査されない
(`Span<T>` の escape が検査されないのと同じ既定 — POINTER.md 選択肢 1)。

### 使う形 (エコーサーバ)

```rust
fn main() -> u64 {
    val listener = TcpListener::bind("127.0.0.1", 0u16)?
    val poller = Poller::new()?
    poller.register(listener.as_fd(), 0u64, INTEREST_READ)?

    var running: bool = true
    while running {
        val n = poller.wait(1000i64)?
        var i: u64 = 0u64
        while i < n {
            val ev = poller.event(i)
            if ev.token == 0u64 {
                val stream = listener.accept()?
                poller.register(stream.as_fd(), stream.as_fd() as u64, INTEREST_READ)?
            } else {
                # ... read, echo back, close on hup ...
            }
            i = i + 1u64
        }
    }
    0u64
}
```

スレッドも channel も出てこない。これが「並行性を待たずに書ける」の
実際の姿で、本設計の存在理由 (NETWORK_IO.md の冒頭)。

## 6. 意味論の落とし穴と、それぞれの決定

| 落とし穴 | 決定 |
|---|---|
| **close する前に deregister するか** | epoll も kqueue も fd の最後の参照が閉じれば自動で外れるが、`dup` した fd があると epoll は残る。**規約: `close` の前に必ず `deregister`**。`TcpStream::close` が自動でやることはしない (Poller を知らないため) |
| **登録済み fd の close 後に token が来る** | 上の規約を守れば起きない。守らなかった場合の挙動は OS 依存で、**統一しない** (docs に「規約違反」と書く) |
| **`wait` 中に register できるか** | 単一スレッドなので `wait` 中に toylang コードは走らない。論点にならない (並行性が入ったら再考) |
| **timeout の精度** | 統一形は**ミリ秒**。kqueue は `timespec` (ns) だが精度を上げても OS のタイマ粒度に埋もれる。テストで時間を pin しないので実害なし |
| **`cap` (1 回に受け取る最大数)** | `Poller::new()` で固定 (既定 64)。可変にするとバッファ再確保が要り、`never_allocates` なイベントループが書けなくなる |
| **同一 fd を 2 つの Poller に登録** | 両 OS とも可能。**禁止しない**が、どちらに来るかは OS 依存なので docs で非推奨と書く |
| **`EVENT_HUP` と `EVENT_READ` の同時発生** | 両方立ちうる。**hup を見て即 close しない**こと (バッファに残りがある) を docs の例で示す |
| **kqueue fd が fork を跨がない** | fork を提供していないので今は問題にならない。RUNTIME_LIBRARY の非目標 (プロセス spawn) に紐づけて記録しておく |

## 7. テストで pin すること

`compiler/tests/consistency/poll.rs` を新設 (`mod.rs` と
`tests/suite.rs` への登録を忘れないこと — 忘れると**無音で実行されない**)。

**pin する (3 レーン一致)**:
- listener を登録して自己接続すると **readable が 1 回来る**
- 接続を受けてデータを送ると、**受信側に readable が来て同じバイト列が読める**
- peer が close すると **`is_hup()` が真になり、`read` が `Ok(0)` を返す**
- 誰も繋がない状態で `wait(0)` は **`Ok(0)`** (タイムアウトは失敗ではない)
- 存在しない fd の `register` は **`Err(NetError::InvalidInput)`**
- 未登録 fd の `deregister` は **`Ok(())`** (冪等)
- 決定 1 の核心: **read と write の両方が立つ状況で、イベント数が
  epoll と kqueue で一致する** (これが割れると設計が破れている)

**pin しない**: 待ち時間、イベントの順序、1 回の `wait` で返る数
(合成後の「fd ごと 1 件」以外)、`data` に入る受信可能バイト数。

**ABI probe (NETWORK_IO.md §2) に追加する行**:
`sizeof(struct epoll_event)` / `offsetof(epoll_event, data)` /
`sizeof(struct kevent)` / `EPOLLIN` / `EPOLLET` / `EVFILT_READ` /
`EV_EOF` / `EV_ERROR`。**`epoll_event` の packed 差 (§4) はここでしか
捕まらない**ので、この行が本テストの主目的と言ってよい。

## 8. 未決の論点

1. **タイマとシグナルを待てるようにするか** — kqueue は `EVFILT_TIMER` /
   `EVFILT_SIGNAL` を持ち、epoll は `timerfd` / `signalfd` という別の
   fd を作る。**統一形は「fd を待つ」だけに閉じており、タイマは
   `wait` の timeout で足りる**。周期タイマが要るなら、統一形は
   `Poller::add_timer(token, ms)` を足して両者を吸収する形になる
   (epoll 側は内部で timerfd を作って隠す)。**MVP には入れない**。
2. **`wait` のイベントを `Vec<Event>` で返すか** — 決定 5 の
   index 読み出しは EXTERN-BUF を待たずに済むのが利点。EXTERN-BUF が
   N1 で入るなら、`wait(&mut Vec<Event>)` 形に寄せる選択肢が生まれる。
   **判断を N1 完了後まで保留**。API の互換性を壊すので、寄せるなら
   N3 の landing 前に決めること。
3. **level 既定を edge にする日** — edge のほうが wake 回数は減るが、
   決定 3 のとおりユーザ側の規律を要求する。`--simd-report` と同じ
   「聞けば答える」tooling で「この fd は edge にできる」と教える形が
   あり得るが、実プログラムで wake 回数が問題になってから。
4. **`io_uring`** — Linux の新しい非同期 IO。epoll の置き換えではなく
   別のモデル (完了通知) なので、統一形に混ぜられない。**やらない**。
   将来やるなら `sys` の 3 つ目ではなく、別の toylang API になる。

## 関連

- [`NETWORK_IO.md`](NETWORK_IO.md) — socket 側、`mod sys` 切り替えの機構、
  extern 境界の制約、ABI probe テスト、errno 表
- [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) — P3 並行性 (本設計はその前に
  単一スレッドで完結する形を出す)
- [`POINTER.md`](POINTER.md) — `Span<T>` の escape 未検査という既定
- [`todo.md`](todo.md) — OP-OVERLOAD-CHAIN (`Interest` を struct に
  できない理由)
