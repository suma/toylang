# HTTP_API.md §2 / §4 — 経路と、実際のソケットの上での 1 往復。
#
# 経路の検査はソケット抜きで行う。`route` は「相手が loopback か」だけを
# 受け取り、接続そのものは受け取らないので、応答の形は接続を作らずに
# 固定できる。**管理系が loopback 以外に答えない**ことがここで最も
# 大事な 1 本で、認証機構が無い以上それが唯一の線だからである。
#
# 最後の 1 本だけは本物のソケットを使う。同じプロセスにクライアントを
# 置くのは `compiler/tests/consistency/net.rs` の流儀で、相手を待つ
# テストは決定的にならないため。

import std.fs
import std.io
import std.net
import std.poll
import http
import server
import store
import ui

fn span_of(s: &String) -> Span<u8> {
    val w = s.as_span()
    match w {
        Option::Some(sp) => sp,
        Option::None => { panic("span_of: empty string") }
    }
}

fn rendered(w: &ByteWriter) -> String {
    var text = String::new()
    var i: u64 = 0u64
    while i < w.len() {
        text.push(w.byte_at(i))
        i = i + 1u64
    }
    text
}

fn contains(hay: &String, needle: str) -> bool {
    val n = String::from_str(needle)
    val at = hay.find(&n)
    match at {
        Option::Some(i) => { true }
        Option::None => { false }
    }
}

# 1 要求を通して、応答の本文を返す。
fn answer_for(spec: str, raw: str, local: bool) -> String {
    val req_text = String::from_str(raw)
    val b = span_of(&req_text)
    val r = http::parse_request(b, req_text.len())
    var st = Stats::new()
    var out = ByteWriter::with_capacity(4096u64)
    # The ingest state a real server keeps for its lifetime. A fresh
    # one per request is wrong for a server and right for a test:
    # each case starts from nothing.
    var w = ArchiveWriter::new()
    var ms = MountSet::new()
    var gens: Vec<u64> = Vec::new()
    server::route(spec, b, &r, local, &mut st, &mut w, &mut ms, &gens, &mut out)
    val text = rendered(&out)
    text
}

# 既定のマウント。**`admin/repair` がここにカタログを書く**ので、
# 「存在しないディレクトリ」を見たいテストは別の名前を使うこと
# (最初に書いたとき、repair のテストがこのディレクトリを作ってしまい、
# 後ろの 503 のテストが 200 を受け取った)。
fn answer(raw: str, local: bool) -> String {
    val text = answer_for("build/server-spec", raw, local)
    text
}

# ---------------------------------------------------------------------

test "healthz answers without looking at anything" {
    val got = answer("GET /healthz HTTP/1.1\r\n\r\n", true)
    assert(contains(&got, "HTTP/1.1 200 OK"), "healthz is 200")
    assert(contains(&got, "content-type: text/plain"), "healthz is text")
    assert(contains(&got, "\r\n\r\nok\n"), "healthz says ok")
    assert(contains(&got, "connection: keep-alive"), "healthz keeps the connection")
}

test "a path nobody serves is 404 and not a guess" {
    val got = answer("GET /nope HTTP/1.1\r\n\r\n", true)
    assert(contains(&got, "HTTP/1.1 404 Not Found"), "unknown paths are 404")
    assert(contains(&got, "no such endpoint"), "the body says which kind of no")
}

# **認証が無いので、これが唯一の防御である。**
test "administration does not answer anyone but the loopback" {
    val far = answer("POST /v1/admin/repair HTTP/1.1\r\n\r\n", false)
    assert(contains(&far, "HTTP/1.1 403"), "a remote peer gets 403")
    assert(contains(&far, "loopback"), "and is told why")

    val near = answer("POST /v1/admin/repair HTTP/1.1\r\n\r\n", true)
    assert(contains(&near, "HTTP/1.1 200 OK"), "the loopback is served")
    assert(contains(&near, "segments"), "repair reports what it found")
}

# 管理系は POST である。GET で同じ経路を叩いて動いてしまうと、
# リンクを踏んだだけでカタログが作り直される。
test "an administrative path does nothing over GET" {
    val got = answer("GET /v1/admin/gc HTTP/1.1\r\n\r\n", true)
    assert(contains(&got, "HTTP/1.1 404"), "GET is not a way to run admin")
}

test "stats reports the process and the disks in one object" {
    val got = answer("GET /v1/stats HTTP/1.1\r\n\r\n", true)
    assert(contains(&got, "HTTP/1.1 200 OK"), "stats is 200")
    assert(contains(&got, "content-type: application/json"), "stats is JSON")
    assert(contains(&got, "uptime_s"), "stats says how long it has been up")
    assert(contains(&got, "mounts"), "stats lists the mounts")
    # 確保カウンタをそのまま出す。**この数が増え続けないこと**が
    # このサーバの健康の定義なので (MEMORY.md §5)、出せない数を
    # 健康の定義にはできない。
    assert(contains(&got, "live_bytes"), "stats says what it is holding")
    assert(contains(&got, "cumulative_bytes"), "and what it has ever held")
}

# 解釈できなかった要求は**接続を閉じる**。どこから次の要求が始まるか
# 分からないまま keep-alive を続けると、次の応答が前の要求の途中に
# 割り込むことになる。
test "a request that could not be parsed closes the connection" {
    val got = answer("PUT /a HTTP/1.1\r\n\r\n", true)
    assert(contains(&got, "HTTP/1.1 405"), "the method is refused")
    assert(contains(&got, "connection: close"), "and the connection ends there")
}

test "a refused request is counted apart from a served one" {
    val req_text = String::from_str("PUT /a HTTP/1.1\r\n\r\n")
    val b = span_of(&req_text)
    val r = http::parse_request(b, req_text.len())
    var st = Stats::new()
    var out = ByteWriter::with_capacity(1024u64)
    var w = ArchiveWriter::new()
    var ms = MountSet::new()
    var gens: Vec<u64> = Vec::new()
    server::route("build/server-spec", b, &r, true, &mut st, &mut w, &mut ms, &gens, &mut out)
    assert_eq(st.refused, 1u64)
    assert_eq(st.requests, 0u64)
}

# ---------------------------------------------------------------------

# 本物のソケット 1 本。クライアントを同じプロセスに置くので、相手を
# 待たずに決定的に終わる。
test "the server answers over a real socket" {
    val bound = TcpListener::bind("127.0.0.1", 0u64)
    var listener = match bound {
        Result::Ok(l) => l,
        Result::Err(e) => { panic("bind: {e}") }
    }
    val got_port = listener.local_port()
    val port = match got_port {
        Result::Ok(n) => n,
        Result::Err(e) => { panic("local_port: {e}") }
    }

    val dialled = TcpStream::connect("127.0.0.1", port)
    var client = match dialled {
        Result::Ok(c) => c,
        Result::Err(e) => { panic("connect: {e}") }
    }

    # `connection: close` を付けるのは、keep-alive だとサーバが次の
    # 要求を 60 秒待つからである。閉じると言ったので閉じる。
    val request = String::from_str("GET /healthz HTTP/1.1\r\nconnection: close\r\n\r\n")
    val sent = client.write(span_of(&request))
    match sent {
        Result::Ok(n) => { assert_eq(n, request.len()) }
        Result::Err(e) => { panic("write: {e}") }
    }

    val taken = listener.accept()
    var conn = match taken {
        Result::Ok(c) => c,
        Result::Err(e) => { panic("accept: {e}") }
    }

    val made = Poller::new()
    var poller = match made {
        Result::Ok(p) => p,
        Result::Err(e) => { panic("poller: {e}") }
    }

    var st = Stats::new()
    var inbox = ByteWriter::with_capacity(4096u64)
    var outbox = ByteWriter::with_capacity(4096u64)
    var w = ArchiveWriter::new()
    var ms = MountSet::new()
    var gens: Vec<u64> = Vec::new()
    val keep = server::serve_connection(&poller, &conn, "build/server-spec",
                                        &mut st, &mut w, &mut ms, &gens,
                                        &mut inbox, &mut outbox)
    assert(keep, "one healthz does not stop the server")
    assert_eq(st.requests, 1u64)
    assert_eq(st.connections, 1u64)

    # 返ってきたバイトを読む。非ブロッキングなので、来るまで回す。
    var reply: Vec<u8> = Vec::with_capacity(4096u64)
    var total: u64 = 0u64
    var spins: u64 = 0u64
    while total == 0u64 && spins < 500u64 {
        val room = reply.capacity_span()
        match room {
            Option::Some(win) => {
                val got = client.read(win)
                match got {
                    Result::Ok(n) => { total = n }
                    Result::Err(e) => { }
                }
            }
            Option::None => { }
        }
        spins = spins + 1u64
    }
    assert(total > 0u64, "the response should arrive")
    reply.set_size(total)

    var text = String::new()
    var i: u64 = 0u64
    while i < reply.size() {
        text.push(reply.get(i))
        i = i + 1u64
    }
    assert(contains(&text, "HTTP/1.1 200 OK"), "the socket carried the status line")
    assert(contains(&text, "\r\n\r\nok\n"), "and the body")
}

# ---------------------------------------------------------------------
# `/v1/query`
#
# パラメータは**セグメントを 1 本も開く前に**検査する。300 ms 歩いた
# あとで同じことを言っても答えは変わらないが、遅い。

test "a query without a query string says which parameter is missing" {
    val got = answer("GET /v1/query HTTP/1.1\r\n\r\n", true)
    assert(contains(&got, "HTTP/1.1 400"), "a missing q is the client's mistake")
    assert(contains(&got, "q: missing"), "and it is named")
}

# 上限は好みではなく**応答の予算**である。本文は 1 バイト書く前に
# メモリ上で組み上がるので、上限の無い limit はサーバの寿命に効く。
test "a limit outside the budget is refused before anything is read" {
    val big = answer("GET /v1/query?q=a&limit=5000 HTTP/1.1\r\n\r\n", true)
    assert(contains(&big, "HTTP/1.1 400"), "1000 is the cap")
    assert(contains(&big, "1 to 1000"), "and the range is stated")

    val zero = answer("GET /v1/query?q=a&limit=0 HTTP/1.1\r\n\r\n", true)
    assert(contains(&zero, "HTTP/1.1 400"), "zero records is not a query")

    val words = answer("GET /v1/query?q=a&limit=soon HTTP/1.1\r\n\r\n", true)
    assert(contains(&words, "HTTP/1.1 400"), "a limit that is not a number is refused")
}

test "a format nobody renders is refused rather than guessed at" {
    val got = answer("GET /v1/query?q=a&format=xml HTTP/1.1\r\n\r\n", true)
    assert(contains(&got, "HTTP/1.1 400"), "xml is not one of the three")
    assert(contains(&got, "json, ndjson or text"), "and the three are named")
}

# `top=<field>` は分布を求める言葉だが、その経路はまだコマンドライン
# 側にしか無い。放っておくと **`parse_query` に落とされて全件が返る** —
# 機能が無いのではなく答えが出たように見えるので、断る方を取る。
test "a distribution asked for over HTTP is refused, not quietly dropped" {
    val got = answer("GET /v1/query?q=top%3Dstatus HTTP/1.1\r\n\r\n", true)
    assert(contains(&got, "HTTP/1.1 400"), "top= is not served here")
    assert(contains(&got, "command-line only"), "and the answer says where it is")

    # 条件と混ざっていても見つける。
    val mixed = answer("GET /v1/query?q=status%3D404+top%3Dpath HTTP/1.1\r\n\r\n", true)
    assert(contains(&mixed, "HTTP/1.1 400"), "even next to a filter")
}

# ただの前方一致で誤爆させない。`topic=x` を断ると、`top=` と何の
# 関係も無いクエリが通らなくなる。
test "a token that merely begins with top is left alone" {
    val topic = answer("GET /v1/query?q=topic%3Dx HTTP/1.1\r\n\r\n", true)
    assert(!contains(&topic, "command-line only"), "topic= is not top=")
    val inside = answer("GET /v1/query?q=path%7Etop%3D HTTP/1.1\r\n\r\n", true)
    assert(!contains(&inside, "command-line only"), "a needle containing it is not it")
}

# 読めるマウントが 1 つも無いのは 400 ではない。要求は正しく、
# 答えられないのはこちらの側である。
test "nothing readable is 503 and not a bad request" {
    # 存在しないディレクトリは打ち間違いであって、空のアーカイブでは
    # ない。**この名前はどのテストも作らない** (作ると 200 になる)。
    val got = answer_for("build/server-absent", "GET /v1/query?q=a HTTP/1.1\r\n\r\n", true)
    assert(contains(&got, "HTTP/1.1 503"), "a spec that is not there cannot be served")
    assert(contains(&got, "no readable mount"), "and says so")
}

# **中身が無いアーカイブは壊れたアーカイブではない。** 開けたが
# セグメントを 1 本も持っていないマウントは 200 と空の結果を返し、
# `segments_considered: 0` がその事実を言う。ここを 503 と一緒に
# すると、何も archive していないだけの人がディスクの権限を
# 疑いに行くことになる — 引数なしの `serve` が既定で指す
# `/tmp/logarchive` がまさにその状態になる。
test "an archive with nothing in it answers, it does not fail" {
    val dir = "build/server-empty"
    val made = fs::mkdir_all(dir)
    match made {
        Result::Ok(u) => { }
        Result::Err(e) => { panic("mkdir {dir}: {e}") }
    }
    val got = answer_for(dir, "GET /v1/query?q=a&format=json HTTP/1.1\r\n\r\n", true)
    assert(contains(&got, "HTTP/1.1 200 OK"), "an empty archive is still an answer")
    assert(contains(&got, "\u{22}records\u{22}:[]"), "with no records")
    assert(contains(&got, "\u{22}segments_considered\u{22}:0"), "and nothing to have considered")
}

# ---------------------------------------------------------------------
# Web UI

fn ui_text() -> String {
    var out = ByteWriter::with_capacity(16384u64)
    ui::page(&mut out)
    val text = rendered(&out)
    text
}

test "the root is one page and nothing else is fetched" {
    val got = answer("GET / HTTP/1.1\r\n\r\n", true)
    assert(contains(&got, "HTTP/1.1 200 OK"), "the UI is served")
    assert(contains(&got, "content-type: text/html"), "as HTML")
    assert(contains(&got, "<!doctype html>"), "and it is a document")

    val page = ui_text()
    # 外から何も取らない。閉じたネットワークで動かないサーバに
    # なるので、CDN も画像も外部 CSS も無い (HTTP_API.md §3)。
    assert(!contains(&page, "http://"), "the page fetches nothing over http")
    assert(!contains(&page, "https://"), "nor over https")
    assert(!contains(&page, "<img"), "and there are no images")
    # 話す相手は 1 つだけ。
    assert(contains(&page, "/v1/query?format=json"), "it asks /v1/query for JSON")
}

# **二重化した波括弧が本文に漏れていないこと。** 足りなければ
# コンパイルが補間エラーで落ちるので気づくが、多すぎても落ちない —
# CSS が黙って壊れるだけである。ここが唯一その差を見る場所になる。
test "the page comes out with the braces it was written with" {
    val page = ui_text()
    assert(contains(&page, "body {{ margin: 0;"), "a CSS rule opens with one brace")
    assert(!contains(&page, "{{{{"), "no brace was doubled twice")
    assert(!contains(&page, "}}}}"), "nor a closing one")
    # `"` は toylang の文字列リテラルに書けないので、ページは
    # 単引用符だけで書かれている。混ざると属性が壊れる。
    assert(!contains(&page, "\u{22}"), "the page holds no double quote")
}

# ---------------------------------------------------------------------
# `POST /v1/ingest`

# A whole ingest request with the right `content-length`.
fn ingest_request(body: &String) -> String {
    val n = body.len()
    val head = "POST /v1/ingest HTTP/1.1\r\ncontent-length: {n}\r\n\r\n"
    var out = String::from_str(head)
    out.push_str(body.to_str())
    out
}

# Each ingest test gets its own directory. Two tests sharing one
# fails four runs in five under `-j4`, which is how the identity
# tests in `tests/mount.t` taught this file the rule.
fn writable(dir: str, st: &mut Stats, ms: &mut MountSet,
            gens: &mut Vec<u64>) -> bool {
    val made = fs::mkdir_all(dir)
    match made {
        Result::Ok(u) => { }
        Result::Err(e) => { panic("mkdir {dir}: {e}") }
    }
    val crc = Crc32::new()
    val ok = store::open_for_write(dir, ms, gens, &crc, false, true)
    if ok { st.segid = store::next_segid(ms, &crc) }
    st.ready = ok
    ok
}

# 2020-01-01T00:00:00Z. Safely in the past, so a record that took the
# arrival time is always later than one that kept its own.
fn y2020() -> i64 { 1577836800i64 }

test "a batch of lines is taken, and the sequence runs on" {
    var st = Stats::new()
    var ms = MountSet::new()
    var gens: Vec<u64> = Vec::new()
    var w = ArchiveWriter::new()
    assert(writable("build/server-ingest-batch", &st, &mut ms, &mut gens),
           "the mount should be writable")

    val body = String::from_str("2020-01-01T00:00:00Z first\n2020-01-01T00:00:02Z second\n")
    val raw = ingest_request(&body)
    val b = span_of(&raw)
    val r = http::parse_request(b, raw.len())
    assert(r.complete, "the request should be whole")

    var out = ByteWriter::with_capacity(1024u64)
    server::route("build/server-ingest-batch", b, &r, true, &mut st, &mut w,
                  &mut ms, &gens, &mut out)
    val got = rendered(&out)
    assert(contains(&got, "HTTP/1.1 200 OK"), "the batch is accepted")
    assert(contains(&got, "\u{22}accepted\u{22}:2"), "both lines went in")
    assert(contains(&got, "\u{22}rejected\u{22}:0"), "neither was refused")
    assert(contains(&got, "\u{22}seq_first\u{22}:1"), "numbering starts at 1")
    assert(contains(&got, "\u{22}seq_last\u{22}:2"), "and runs to the last")
    assert_eq(w.count(), 2u64)

    # 2 通目は続きの番号から。送り手が自分の行を数え直せる。
    var out2 = ByteWriter::with_capacity(1024u64)
    val body2 = String::from_str("2020-01-01T00:00:03Z third\n")
    val raw2 = ingest_request(&body2)
    val b2 = span_of(&raw2)
    val r2 = http::parse_request(b2, raw2.len())
    server::route("build/server-ingest-batch", b2, &r2, true, &mut st, &mut w,
                  &mut ms, &gens, &mut out2)
    val got2 = rendered(&out2)
    assert(contains(&got2, "\u{22}seq_first\u{22}:3"), "the sequence continues")
    assert_eq(w.count(), 3u64)
}

# 行が時刻を持たなければ**受信時刻**を付ける。付けないと、時刻で
# 絞るどのクエリからも見えなくなる。
test "a line with no time of its own takes the time it arrived" {
    var st = Stats::new()
    var ms = MountSet::new()
    var gens: Vec<u64> = Vec::new()
    var w = ArchiveWriter::new()
    assert(writable("build/server-ingest-time", &st, &mut ms, &mut gens),
           "the mount should be writable")

    val body = String::from_str("2020-01-01T00:00:00Z dated\nlevel=info undated\n")
    val raw = ingest_request(&body)
    val b = span_of(&raw)
    val r = http::parse_request(b, raw.len())
    var out = ByteWriter::with_capacity(1024u64)
    server::route("build/server-ingest-time", b, &r, true, &mut st, &mut w,
                  &mut ms, &gens, &mut out)

    assert_eq(w.count(), 2u64)
    assert_eq(w.ts_min(), y2020())
    assert(w.ts_max() > y2020(), "the undated line landed at the arrival time")
}

# **部分成功。** 100 行のうち 3 行が壊れていても 97 行は取り込む —
# 全体を失敗にすると、送り手は同じ 100 行を再送し続ける。
test "a line that cannot be taken does not take the batch with it" {
    var st = Stats::new()
    var ms = MountSet::new()
    var gens: Vec<u64> = Vec::new()
    var w = ArchiveWriter::new()
    assert(writable("build/server-ingest-partial", &st, &mut ms, &mut gens),
           "the mount should be writable")

    var body = String::new()
    body.push_str("2020-01-01T00:00:00Z fine one\n")
    var i: u64 = 0u64
    while i < 70000u64 {
        body.push('x')
        i = i + 1u64
    }
    body.push_str("\n2020-01-01T00:00:01Z fine two\n")

    val raw = ingest_request(&body)
    val b = span_of(&raw)
    val r = http::parse_request(b, raw.len())
    assert(r.complete, "a 70 KB body is under the request limit")
    var out = ByteWriter::with_capacity(1024u64)
    server::route("build/server-ingest-partial", b, &r, true, &mut st, &mut w,
                  &mut ms, &gens, &mut out)
    val got = rendered(&out)
    assert(contains(&got, "HTTP/1.1 200 OK"), "the batch still succeeds")
    assert(contains(&got, "\u{22}accepted\u{22}:2"), "the good lines went in")
    assert(contains(&got, "\u{22}rejected\u{22}:1"), "the long one is reported")
    assert_eq(w.count(), 2u64)
}

# 空行は数えない。末尾の改行が「1 行」になると、送った数と受けた数が
# 合わなくなる。
test "a blank line is neither taken nor rejected" {
    var st = Stats::new()
    var ms = MountSet::new()
    var gens: Vec<u64> = Vec::new()
    var w = ArchiveWriter::new()
    assert(writable("build/server-ingest-blank", &st, &mut ms, &mut gens),
           "the mount should be writable")

    val body = String::from_str("2020-01-01T00:00:00Z one\n\n\n2020-01-01T00:00:01Z two\n")
    val raw = ingest_request(&body)
    val b = span_of(&raw)
    val r = http::parse_request(b, raw.len())
    var out = ByteWriter::with_capacity(1024u64)
    server::route("build/server-ingest-blank", b, &r, true, &mut st, &mut w,
                  &mut ms, &gens, &mut out)
    val got = rendered(&out)
    assert(contains(&got, "\u{22}accepted\u{22}:2"), "two records, not four")
    assert(contains(&got, "\u{22}rejected\u{22}:0"), "and nothing was refused")
}

# 書ける先が無ければ 503。要求は正しいので 400 ではない。
test "ingest without a writable mount says so and keeps nothing" {
    var st = Stats::new()
    var ms = MountSet::new()
    var gens: Vec<u64> = Vec::new()
    var w = ArchiveWriter::new()
    # `ready` は false のまま。サーバはマウントを自分で作らない。
    val body = String::from_str("2020-01-01T00:00:00Z nowhere to go\n")
    val raw = ingest_request(&body)
    val b = span_of(&raw)
    val r = http::parse_request(b, raw.len())
    var out = ByteWriter::with_capacity(1024u64)
    server::route("build/server-ingest-absent", b, &r, true, &mut st, &mut w,
                  &mut ms, &gens, &mut out)
    val got = rendered(&out)
    assert(contains(&got, "HTTP/1.1 503"), "there is nowhere to put it")
    assert(contains(&got, "no writable mount"), "and the answer says so")
    assert_eq(w.count(), 0u64)
}

# `flush` は持っているものを書き出し、いくつ出したかを言う。
test "flush writes what is held and reports it" {
    var st = Stats::new()
    var ms = MountSet::new()
    var gens: Vec<u64> = Vec::new()
    var w = ArchiveWriter::new()
    val dir = "build/server-ingest-flush"
    assert(writable(dir, &st, &mut ms, &mut gens), "the mount should be writable")

    val body = String::from_str("2020-01-01T00:00:00Z one\n2020-01-01T00:00:01Z two\n")
    val raw = ingest_request(&body)
    val b = span_of(&raw)
    val r = http::parse_request(b, raw.len())
    var out = ByteWriter::with_capacity(1024u64)
    server::route(dir, b, &r, true, &mut st, &mut w, &mut ms, &gens, &mut out)
    assert_eq(w.count(), 2u64)

    val fraw = String::from_str("POST /v1/admin/flush HTTP/1.1\r\n\r\n")
    val fb = span_of(&fraw)
    val fr = http::parse_request(fb, fraw.len())
    var fout = ByteWriter::with_capacity(1024u64)
    server::route(dir, fb, &fr, true, &mut st, &mut w, &mut ms, &gens, &mut fout)
    val fgot = rendered(&fout)
    assert(contains(&fgot, "HTTP/1.1 200 OK"), "flush answers")
    assert(contains(&fgot, "\u{22}records\u{22}:2"), "it says what it held")
    # 書き出したので writer は空になり、セグメントが 1 本増える。
    assert(w.is_empty(), "the active segment starts over")
    assert_eq(st.segments, 1u64)
}
