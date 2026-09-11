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

import std.io
import std.net
import std.poll
import http
import server

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
fn answer(raw: str, local: bool) -> String {
    val req_text = String::from_str(raw)
    val b = span_of(&req_text)
    val r = http::parse_request(b, req_text.len())
    var st = Stats::new()
    var out = ByteWriter::with_capacity(4096u64)
    server::route("build/server-spec", b, &r, local, &mut st, &mut out)
    val text = rendered(&out)
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
    server::route("build/server-spec", b, &r, true, &mut st, &mut out)
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
    val keep = server::serve_connection(&poller, &conn, "build/server-spec",
                                        &mut st, &mut inbox, &mut outbox)
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
