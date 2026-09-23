# ONTOLOGY §3 — 抽出器が行からフィールドを取り出せているか。
#
# ここまで §3 の表は「エンジンの出した分布が厳密オラクルと全一致した」
# という**その場の突き合わせ**でしか確かめていない (ROADMAP #1)。
# 分布は corpus に依存するので回帰テストにできないが、**分布をそう
# させている個々の判断**は行 1 本ずつ書き下せる。ここはそれを固める。
#
# 入力は合成である。実ログから取ると (a) corpus に依存し、(b) 実在の
# アドレスがリポジトリに入る (CLAUDE.md の規約)。アドレスはすべて
# プライベートにしてある。

fn span_of(s: &String) -> Span<u8> {
    val w = s.as_span()
    match w {
        Option::Some(sp) => sp,
        Option::None => { panic("span_of: empty string") }
    }
}

# フィールドの中身を取り出して比べる。
fn field_str(w: Span<u8>, f: u64) -> String {
    if !extract::has_field(f) {
        val empty = String::from_str("")
        return empty
    }
    val out = query::text_of(w, extract::field_start(f), extract::field_len(f))
    out
}

fn check(line: str, name: str, got: &String, want: str) {
    val w = String::from_str(want)
    if !got.eq(&w) {
        panic("{line}: {name} = `{got}`, want `{want}`")
    }
}

# vhost の無い形。第 1 フィールドが client。
test "an access line without a vhost gives up its fields" {
    val s = String::from_str(r#"10.0.0.1 - - [03/Sep/2026:12:00:01 +0000] "GET /index.html HTTP/1.1" 200 11103 "-" "curl/8.0""#)
    val w = span_of(&s)
    val f = extract::http(w, 0u64, s.len())

    assert(f.ok, "no-vhost line should parse")
    check("no-vhost", "vhost", &field_str(w, f.vhost), "")
    check("no-vhost", "client", &field_str(w, f.client), "10.0.0.1")
    check("no-vhost", "method", &field_str(w, f.method), "GET")
    check("no-vhost", "path", &field_str(w, f.path), "/index.html")
    check("no-vhost", "status", &field_str(w, f.status), "200")
    check("no-vhost", "bytes", &field_str(w, f.bytes), "11103")
    check("no-vhost", "ua", &field_str(w, f.ua), "curl/8.0")
}

# vhost のある形。§3 の「第 1 フィールドを client と決め打つと 6,682 件が
# ホスト名という名前の IP になる」がこれ。
test "an access line with a vhost does not mistake it for the client" {
    val s = String::from_str(r#"blog.example:80 10.0.0.2 - - [03/Sep/2026:12:00:02 +0000] "GET /robots.txt HTTP/1.1" 301 612 "-" "MJ12bot/1.4""#)
    val w = span_of(&s)
    val f = extract::http(w, 0u64, s.len())

    assert(f.ok, "vhost line should parse")
    check("vhost", "vhost", &field_str(w, f.vhost), "blog.example:80")
    check("vhost", "client", &field_str(w, f.client), "10.0.0.2")
    check("vhost", "path", &field_str(w, f.path), "/robots.txt")
    check("vhost", "status", &field_str(w, f.status), "301")
}

# §8: apache はリクエストと UA の中の `"` を `\"` と書く。最初の `"` で
# 引用を閉じる実装は 19 行でフィールドがずれ、payload が「ステータス」
# として数えられた。distinct な status が 30 → 16 に落ちたのがその効果。
test "an escaped quote does not close the field" {
    val s = String::from_str(r#"10.0.0.3 - - [03/Sep/2026:12:00:03 +0000] "GET /a HTTP/1.1" 404 209 "-" "Mozilla/5.0 (\"weird\") Gecko""#)
    val w = span_of(&s)
    val f = extract::http(w, 0u64, s.len())

    assert(f.ok, "escaped-quote line should parse")
    # ずれていれば status は payload の断片になる。
    check("escaped-quote", "status", &field_str(w, f.status), "404")
    check("escaped-quote", "bytes", &field_str(w, f.bytes), "209")
    check("escaped-quote", "ua", &field_str(w, f.ua), r#"Mozilla/5.0 (\"weird\") Gecko"#)
}

# §3: `method` の 4 位は TLS ハンドシェイク — HTTPS を平文ポートに
# 投げたクライアント。部分一致では決して見えない。
test "a TLS handshake shows up as the method it is" {
    val s = String::from_str("10.0.0.4 - - [03/Sep/2026:12:00:04 +0000] \"\u{16}\u{03}\u{01}\u{02}\u{00}\" 400 0 \"-\" \"-\"")
    val w = span_of(&s)
    val f = extract::http(w, 0u64, s.len())

    check("tls", "method", &field_str(w, f.method), "\u{16}\u{03}\u{01}\u{02}\u{00}")
    check("tls", "status", &field_str(w, f.status), "400")
}

# §3 の「フィールドを持たない行 1,672」— オラクルの「パース不能」と
# 同数。access log でない行は素通りしなければならない。
test "a line that is not an access log yields no fields" {
    val s = String::from_str("Sep  3 12:00:05 web01 CRON[1234]: (root) CMD (/usr/bin/true)")
    val w = span_of(&s)
    val f = extract::http(w, 0u64, s.len())

    assert(!f.ok, "a syslog line must not parse as an access line")
}
