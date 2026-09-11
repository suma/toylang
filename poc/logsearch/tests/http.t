# HTTP_API.md §1 — 話すと決めた部分集合の境界。
#
# ここで固めたいのは「何を受け入れるか」ではなく**何を受け入れないか**
# である。受け入れる形は 1 つ増えるたびに壊れる面が 1 つ増えるので、
# 折り返しヘッダ・chunked・GET のボディは**通らないことをテストする**。
#
# もう 1 つは「まだ届いていない」と「間違っている」を取り違えないこと。
# 途中まで届いた要求に 400 を返すサーバは、遅いクライアントを全部
# 切ることになる。

import std.io
import http

fn span_of(s: &String) -> Span<u8> {
    val w = s.as_span()
    match w {
        Option::Some(sp) => sp,
        Option::None => { panic("span_of: empty string") }
    }
}

fn text_at(s: &String, at: u64, len: u64) -> String {
    val out = s.substring(at, at + len)
    out
}

fn check(name: str, got: &String, want: str) {
    val w = String::from_str(want)
    if !got.eq(&w) {
        panic("{name}: got `{got}`, want `{want}`")
    }
}

# ---------------------------------------------------------------------

test "a whole GET is taken apart into its pieces" {
    val raw = String::from_str("GET /v1/query?q=timeout&limit=5 HTTP/1.1\r\nhost: localhost\r\nuser-agent: curl/8.0\r\n\r\n")
    val r = http::parse_request(span_of(&raw), raw.len())

    assert(r.complete, "the request is all there")
    assert_eq(r.status, 0u64)
    assert(r.is_get(), "the method is GET")
    assert(r.keep_alive, "HTTP/1.1 keeps the connection by default")
    assert(r.has_query(), "there is a query string")
    check("path", &text_at(&raw, r.path_at, r.path_len), "/v1/query")
    check("query", &text_at(&raw, r.query_at, r.query_len), "q=timeout&limit=5")
    assert_eq(r.body_len, 0u64)
    assert_eq(r.total_len, raw.len())
}

test "a path with no query string has no query string" {
    val raw = String::from_str("GET /healthz HTTP/1.1\r\n\r\n")
    val r = http::parse_request(span_of(&raw), raw.len())
    assert(r.complete, "the request is all there")
    assert(!r.has_query(), "there is nothing after the path")
    check("path", &text_at(&raw, r.path_at, r.path_len), "/healthz")
}

# 行末が `\n` だけでも読む。`nc` で手で叩けることに 2 行ぶんの価値がある。
test "a bare newline ends a line as well as CRLF does" {
    val raw = String::from_str("GET /healthz HTTP/1.1\nhost: x\n\n")
    val r = http::parse_request(span_of(&raw), raw.len())
    assert(r.complete, "a bare-LF request is still a request")
    check("path", &text_at(&raw, r.path_at, r.path_len), "/healthz")
}

# **届いていないのは誤りではない。** ここを 400 にすると、遅い
# クライアントが全部切られる。
test "a request that has not finished arriving is not an error" {
    val head = String::from_str("GET /healthz HTTP/1.1\r\nhost: local")
    val a = http::parse_request(span_of(&head), head.len())
    assert(!a.complete, "the head is not finished")
    assert_eq(a.status, 0u64)

    # ヘッダは揃ったがボディが途中。
    val partial = String::from_str("POST /v1/ingest HTTP/1.1\r\ncontent-length: 10\r\n\r\nabc")
    val b = http::parse_request(span_of(&partial), partial.len())
    assert(!b.complete, "the body is not finished")
    assert_eq(b.status, 0u64)
    assert_eq(b.body_len, 10u64)
}

test "a POST hands over where its body starts and how long it is" {
    val raw = String::from_str("POST /v1/ingest?host=web01 HTTP/1.1\r\ncontent-length: 11\r\n\r\nhello world")
    val r = http::parse_request(span_of(&raw), raw.len())
    assert(r.complete, "the body is all there")
    assert(r.is_post(), "the method is POST")
    check("body", &text_at(&raw, r.body_at, r.body_len), "hello world")
    check("query", &text_at(&raw, r.query_at, r.query_len), "host=web01")
    assert_eq(r.total_len, raw.len())
}

# 1 接続に 2 要求が続けて届いても、最初のぶんだけを切り出せる。
# これが無いと keep-alive はバッファの取り違えになる。
test "two requests in one buffer do not run together" {
    val raw = String::from_str("GET /a HTTP/1.1\r\n\r\nGET /b HTTP/1.1\r\n\r\n")
    val r = http::parse_request(span_of(&raw), raw.len())
    assert(r.complete, "the first request is all there")
    check("path", &text_at(&raw, r.path_at, r.path_len), "/a")
    # 2 本目は残りとして手つかずで残る。
    assert(r.total_len < raw.len(), "the second request is still in the buffer")

    val rest = raw.substring(r.total_len, raw.len())
    val second = http::parse_request(span_of(&rest), rest.len())
    assert(second.complete, "the second request parses on its own")
    check("path 2", &text_at(&rest, second.path_at, second.path_len), "/b")
}

# ---------------------------------------------------------------------
# 受け入れないもの

test "a folded header line is refused rather than guessed at" {
    val raw = String::from_str("GET /a HTTP/1.1\r\nx-long: one\r\n  two\r\n\r\n")
    val r = http::parse_request(span_of(&raw), raw.len())
    assert_eq(r.status, 400u64)
}

test "a chunked body asks for a length instead" {
    val raw = String::from_str("POST /v1/ingest HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n")
    val r = http::parse_request(span_of(&raw), raw.len())
    assert_eq(r.status, 411u64)
}

test "a method this server does not serve is 405" {
    val raw = String::from_str("PUT /a HTTP/1.1\r\n\r\n")
    val r = http::parse_request(span_of(&raw), raw.len())
    assert_eq(r.status, 405u64)

    val nonsense = String::from_str("BREW /a HTTP/1.1\r\n\r\n")
    val n = http::parse_request(span_of(&nonsense), nonsense.len())
    assert_eq(n.status, 405u64)
}

test "a header line longer than the limit is refused" {
    var raw = String::new()
    raw.push_str("GET /a HTTP/1.1\r\nx-big: ")
    var i: u64 = 0u64
    while i < 1100u64 {
        raw.push('x')
        i = i + 1u64
    }
    raw.push_str("\r\n\r\n")
    val r = http::parse_request(span_of(&raw), raw.len())
    assert_eq(r.status, 431u64)
}

test "a content-length that is not a number is a bad request" {
    val raw = String::from_str("POST /a HTTP/1.1\r\ncontent-length: soon\r\n\r\n")
    val r = http::parse_request(span_of(&raw), raw.len())
    assert_eq(r.status, 400u64)
}

# GET にボディを付ける相手は居るが、この API に読み方が無い。当てずっぽう
# に読むより断る方がよい。
test "a GET with a body is refused" {
    val raw = String::from_str("GET /a HTTP/1.1\r\ncontent-length: 3\r\n\r\nabc")
    val r = http::parse_request(span_of(&raw), raw.len())
    assert_eq(r.status, 400u64)
}

test "a request line without a target is a bad request" {
    val raw = String::from_str("GET\r\n\r\n")
    val r = http::parse_request(span_of(&raw), raw.len())
    assert_eq(r.status, 400u64)
}

# ---------------------------------------------------------------------
# keep-alive

test "the connection is kept unless someone says otherwise" {
    val plain = String::from_str("GET /a HTTP/1.1\r\n\r\n")
    val a = http::parse_request(span_of(&plain), plain.len())
    assert(a.keep_alive, "HTTP/1.1 defaults to keeping the connection")

    val closing = String::from_str("GET /a HTTP/1.1\r\nconnection: close\r\n\r\n")
    val b = http::parse_request(span_of(&closing), closing.len())
    assert(!b.keep_alive, "`connection: close` closes it")

    # 大文字小文字はヘッダ名にも値にも効かない。
    val shouty = String::from_str("GET /a HTTP/1.1\r\nConnection: CLOSE\r\n\r\n")
    val c = http::parse_request(span_of(&shouty), shouty.len())
    assert(!c.keep_alive, "header names and values fold case")

    val old = String::from_str("GET /a HTTP/1.0\r\n\r\n")
    val d = http::parse_request(span_of(&old), old.len())
    assert(!d.keep_alive, "HTTP/1.0 defaults to closing")

    val old_kept = String::from_str("GET /a HTTP/1.0\r\nconnection: keep-alive\r\n\r\n")
    val e = http::parse_request(span_of(&old_kept), old_kept.len())
    assert(e.keep_alive, "HTTP/1.0 can still ask to be kept")
}

# ---------------------------------------------------------------------
# クエリ文字列

fn decoded(raw: str) -> String {
    val s = String::from_str(raw)
    var out = ByteWriter::with_capacity(64u64)
    http::percent_decode(span_of(&s), 0u64, s.len(), &mut out)
    var text = String::new()
    var i: u64 = 0u64
    while i < out.len() {
        text.push(out.byte_at(i))
        i = i + 1u64
    }
    text
}

test "a value comes back with its escapes undone" {
    check("space", &decoded("a%20b"), "a b")
    check("plus", &decoded("a+b"), "a b")
    check("mixed", &decoded("%2Fvar%2Flog+x"), "/var/log x")
    check("lower hex", &decoded("%2fvar"), "/var")
}

# **壊れたエスケープはそのまま残す。** 末尾の `%` は切れた
# エスケープよりも誰かのパスワードである方が多く、黙って食うと
# 違いが見えなくなる。
test "an escape that is not an escape is kept as written" {
    check("trailing", &decoded("abc%"), "abc%")
    check("short", &decoded("ab%4"), "ab%4")
    check("not hex", &decoded("a%ZZb"), "a%ZZb")
}

fn param(qs: str, name: str) -> String {
    val s = String::from_str(qs)
    var out = ByteWriter::with_capacity(64u64)
    val found = http::query_param(span_of(&s), 0u64, s.len(), name, &mut out)
    var text = String::new()
    if !found {
        text.push_str("<absent>")
        return text
    }
    var i: u64 = 0u64
    while i < out.len() {
        text.push(out.byte_at(i))
        i = i + 1u64
    }
    text
}

test "a parameter is found wherever it sits in the query string" {
    check("first", &param("q=timeout&limit=5", "q"), "timeout")
    check("last", &param("q=timeout&limit=5", "limit"), "5")
    check("only", &param("format=json", "format"), "json")
    check("decoded", &param("path=%2Fvar%2Flog", "path"), "/var/log")
}

# 「無い」と「空」は違う。`?limit=` はクライアントが何か言っている。
test "an absent parameter is not the same as an empty one" {
    check("absent", &param("q=x", "limit"), "<absent>")
    check("empty", &param("q=x&limit=", "limit"), "")
    check("bare", &param("q=x&verbose", "verbose"), "")
}

# 前方一致で拾ってはいけない。`limit` を探して `limited` に当たると、
# 上限が黙って別の値になる。
test "a parameter is not matched by a longer name that starts the same" {
    check("prefix", &param("limited=9&limit=5", "limit"), "5")
    check("suffix", &param("xlimit=9&limit=5", "limit"), "5")
    check("no match", &param("limited=9", "limit"), "<absent>")
}

# ---------------------------------------------------------------------
# 応答

fn rendered(w: &ByteWriter) -> String {
    var text = String::new()
    var i: u64 = 0u64
    while i < w.len() {
        text.push(w.byte_at(i))
        i = i + 1u64
    }
    text
}

test "a plain response carries its own length" {
    var out = ByteWriter::with_capacity(256u64)
    http::respond_text(&mut out, 200u64, "text/plain", "ok\n", true)
    val got = rendered(&out)
    val want = String::from_str("HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 3\r\nconnection: keep-alive\r\n\r\nok\n")
    assert(got.eq(&want), "the health response should be exactly this")
}

test "an error response says what went wrong in the documented shape" {
    var out = ByteWriter::with_capacity(256u64)
    http::respond_error(&mut out, 400u64, "bad parameter", "limit: not a number", false)
    val got = rendered(&out)
    val want = String::from_str("HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: 57\r\nconnection: close\r\n\r\n{{\u{22}error\u{22}:\u{22}bad parameter\u{22},\u{22}detail\u{22}:\u{22}limit: not a number\u{22}}}\n")
    assert(got.eq(&want), "the error response should be exactly this")
}
