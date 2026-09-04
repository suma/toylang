//! `core/std/json.t` — the writer, the tree and the reader
//! (STDLIB-SERIALIZE S1/S3/S4/S5).

use super::harness::{assert_renders, compiled_run_output, compiled_run_streams, core_modules_dir};
use interpreter::RunOptions;

/// Read a document, print it back, or print why it would not read.
const SHOW: &str = r#"
    fn show(s: str) {
        var d: Json = Json::new()
        val r = d.read(s)
        match r {
            Result::Ok(root) => {
                val out: String = d.to_string()
                println(out)
            }
            Result::Err(e) => { println(e) }
        }
    }
"#;

#[test]
fn the_writer_places_every_separator() {
    // Commas, colons and the escapes, in one document -- the state a
    // writer with no tree has to keep is exactly this.
    let src = r#"
        fn main() -> u64 {
            var w: JsonWriter = JsonWriter::new()
            w.begin_object()
            w.key("name")
            w.str_value("toy \u{22}lang\u{22}\n")
            w.key("n")
            w.u64_value(42u64)
            w.key("half")
            w.f64_value(0.5f64)
            w.key("ok")
            w.bool_value(true)
            w.key("nothing")
            w.null_value()
            w.key("list")
            w.begin_array()
            w.i64_value(-1i64)
            w.begin_object()
            w.key("deep")
            w.bool_value(false)
            w.end_object()
            w.end_array()
            w.end_object()
            val doc: String = w.finish()
            println(doc)
            0u64
        }
    "#;
    assert_renders(
        src,
        "json_writer",
        "{\"name\":\"toy \\\"lang\\\"\\n\",\"n\":42,\"half\":0.5,\"ok\":true,\"nothing\":null,\"list\":[-1,{\"deep\":false}]}\n",
    );
}

#[test]
fn the_writer_refuses_a_number_json_cannot_spell() {
    // Writing `null` instead would put a different value in the
    // document than the one that was passed, and writing `NaN` would
    // produce something no other reader accepts. Both lanes stop.
    let src = r#"
        fn main() -> u64 {
            var w: JsonWriter = JsonWriter::new()
            w.begin_array()
            val zero: f64 = 0f64
            w.f64_value(zero / zero)
            0u64
        }
    "#;
    let mut opts = RunOptions::default();
    let core = core_modules_dir();
    opts.core_modules_dir = Some(core.as_path());
    let err = interpreter::run_source(src, "json_nan.t", &opts)
        .expect_err("NaN must not be written");
    assert!(
        err.to_string().contains("JSON has no NaN"),
        "interpreter said: {err}"
    );

    if let Some((code, stderr)) = compiled_run_output(src, "json_nan") {
        assert_ne!(code, 0, "compiled binary should exit non-zero");
        assert!(stderr.contains("JSON has no NaN"), "compiled said: {stderr}");
    }
}

#[test]
fn a_document_reads_back_as_what_was_written() {
    // The round trip is the test the tree and the writer share: the
    // tree is printed through `JsonWriter`, so a disagreement between
    // the two spellings cannot hide.
    let src = format!(
        r#"
        {SHOW}
        fn main() -> u64 {{
            show("{{{{\u{{22}}a\u{{22}}: 1, \u{{22}}b\u{{22}}: [true, null, 2.5, \u{{22}}x\u{{22}}]}}}}")
            show("[]")
            show("{{{{}}}}")
            show("  7  ")
            show("\u{{22}}\u{{22}}")
            0u64
        }}
    "#
    );
    assert_renders(
        &src,
        "json_roundtrip",
        "{\"a\":1,\"b\":[true,null,2.5,\"x\"]}\n[]\n{}\n7\n\"\"\n",
    );
}

#[test]
fn an_integer_stays_an_integer() {
    // The reason `Int` and `Num` are separate kinds: an identifier
    // put through an f64 stops being itself above 2^53.
    let src = format!(
        r#"
        {SHOW}
        fn main() -> u64 {{
            show("1234567890123456789")
            show("-9007199254740993")
            # Anything with a `.` or an exponent is a Num, and so is
            # an integer too wide for an i64 -- that one loses
            # precision, which is the documented edge.
            show("2.0")
            show("1e2")
            show("123456789012345678901234")
            0u64
        }}
    "#
    );
    assert_renders(
        &src,
        "json_numbers",
        "1234567890123456789\n-9007199254740993\n2.0\n100.0\n123456789012345685803008.0\n",
    );
}

#[test]
fn the_reader_takes_json_and_not_its_neighbours() {
    let src = format!(
        r#"
        {SHOW}
        fn main() -> u64 {{
            show("  ")
            show("[1,]")
            show("{{{{\u{{22}}a\u{{22}}: 1,}}}}")
            show("// a comment")
            show("NaN")
            show("Infinity")
            show("01")
            show("'a'")
            show("[1] and more")
            show(".5")
            show("+1")
            show("[1")
            0u64
        }}
    "#
    );
    assert_renders(
        &src,
        "json_refusals",
        "empty input\ninvalid JSON at byte 3\ninvalid JSON at byte 8\ninvalid JSON at byte 0\n\
         invalid JSON at byte 0\ninvalid JSON at byte 0\ninvalid JSON at byte 1\ninvalid JSON at byte 0\n\
         trailing content at byte 4\ninvalid JSON at byte 0\ninvalid JSON at byte 0\ninvalid JSON at byte 2\n",
    );
}

#[test]
fn escapes_come_back_as_the_characters_they_name() {
    let src = format!(
        r#"
        {SHOW}
        fn main() -> u64 {{
            # A surrogate pair is one codepoint, not two halves.
            show("\u{{22}}\\u00e9 \\ud83d\\ude00\u{{22}}")
            # Half a pair is not a character.
            show("\u{{22}}\\ud83d\u{{22}}")
            show("\u{{22}}\\udc00\u{{22}}")
            # The short escapes, and one that is not an escape.
            show("\u{{22}}a\\tb\\\\c\\/d\u{{22}}")
            show("\u{{22}}\\q\u{{22}}")
            # A raw control character has to be written as an escape.
            show("\u{{22}}a\nb\u{{22}}")
            0u64
        }}
    "#
    );
    assert_renders(
        &src,
        "json_escapes",
        "\"\u{e9} \u{1f600}\"\ninvalid JSON at byte 1\ninvalid JSON at byte 1\n\"a\\tb\\\\c/d\"\n\
         invalid JSON at byte 1\ninvalid JSON at byte 2\n",
    );
}

#[test]
fn a_document_too_deep_is_reported_rather_than_fatal() {
    // The whole reason the limit exists: without it a nested input
    // is `fatal runtime error: stack overflow` from inside a
    // function whose job is to answer instead of crashing.
    //
    // Run as a spawned binary rather than through `assert_renders`,
    // because the in-process lanes run on a test thread whose stack
    // gives out at about ten levels of nesting -- far below any
    // limit a JSON reader could usefully set. The limit protects the
    // program a user actually runs; nothing in this library can
    // protect a 2 MiB thread from a recursive walk.
    let src = r#"
        fn nested(n: u64) -> String {
            var s: String = String::new()
            var i: u64 = 0u64
            while i < n { s.push(91u8)  i = i + 1u64 }
            s.push_str("1")
            i = 0u64
            while i < n { s.push(93u8)  i = i + 1u64 }
            s
        }

        fn try_at(n: u64) {
            val doc: String = nested(n)
            val text: str = doc.to_str()
            var d: Json = Json::new()
            val r = d.read(text)
            match r {
                Result::Ok(root) => { println(d.size()) }
                Result::Err(e) => { println(e) }
            }
        }

        fn main() -> u64 {
            try_at(32u64)
            try_at(33u64)
            try_at(500u64)
            0u64
        }
    "#;
    let Some((code, stdout, stderr)) = compiled_run_streams(src, "json_depth") else {
        return;
    };
    assert_eq!(code, 0, "the deep document must not crash:\n{stderr}");
    assert_eq!(
        stdout,
        "33\nnested too deeply at byte 33\nnested too deeply at byte 33\n"
    );
}

#[test]
fn a_read_document_can_be_walked_by_name() {
    let src = r#"
        fn kind_name(k: JsonKind) -> str {
            match k {
                JsonKind::Null => "null",
                JsonKind::Bool => "bool",
                JsonKind::Int => "int",
                JsonKind::Num => "num",
                JsonKind::Text => "text",
                JsonKind::Array => "array",
                JsonKind::Object => "object",
            }
        }

        fn main() -> u64 {
            var d: Json = Json::new()
            val r = d.read("{{\u{22}name\u{22}: \u{22}toy\u{22}, \u{22}n\u{22}: 42, \u{22}xs\u{22}: [1, 2, 3]}}")
            match r {
                Result::Ok(root) => {
                    println(d.len(root))
                    var i: u64 = 0u64
                    while i < d.len(root) {
                        val k: str = d.key_at(root, i)
                        val v: u64 = d.value_at(root, i)
                        val kind: JsonKind = d.kind(v)
                        val name: str = kind_name(kind)
                        println("{k}: {name}")
                        i = i + 1u64
                    }
                    val n = d.get(root, "n")
                    match n {
                        Option::Some(v) => { println(d.as_int(v)) }
                        Option::None => { println(-1i64) }
                    }
                    # A name that is not there is `None`, not a panic.
                    val missing = d.get(root, "nope")
                    match missing {
                        Option::Some(v) => { println(true) }
                        Option::None => { println(false) }
                    }
                    val xs = d.get(root, "xs")
                    match xs {
                        Option::Some(v) => {
                            println(d.len(v))
                            val third: u64 = d.child(v, 2u64)
                            # An Int read as a number is still the
                            # same value, just spelled as one.
                            println(d.as_num(third))
                        }
                        Option::None => { println(0u64) }
                    }
                }
                Result::Err(e) => { println(e) }
            }
            0u64
        }
    "#;
    assert_renders(
        src,
        "json_walk",
        "3\nname: text\nn: int\nxs: array\n42\nfalse\n3\n3.0\n",
    );
}

#[test]
fn a_repeated_name_keeps_the_last_one() {
    // Not an error: the reader stores both members and `get` answers
    // with the later, which is what a map would have done.
    let src = format!(
        r#"
        {SHOW}
        fn main() -> u64 {{
            show("{{{{\u{{22}}a\u{{22}}: 1, \u{{22}}a\u{{22}}: 2}}}}")
            var d: Json = Json::new()
            val r = d.read("{{{{\u{{22}}a\u{{22}}: 1, \u{{22}}a\u{{22}}: 2}}}}")
            match r {{
                Result::Ok(root) => {{
                    val a = d.get(root, "a")
                    match a {{
                        Option::Some(v) => {{ println(d.as_int(v)) }}
                        Option::None => {{ println(-1i64) }}
                    }}
                }}
                Result::Err(e) => {{ println(-2i64) }}
            }}
            0u64
        }}
    "#
    );
    assert_renders(&src, "json_dup_key", "{\"a\":1,\"a\":2}\n2\n");
}

#[test]
fn parse_hands_back_the_document_or_the_reason() {
    // `json::parse` is the entrance §2 of the design asked for: a
    // `Result<Json, JsonError>`. Both halves are wide -- the `Ok`
    // carries a `Vec` of nodes, the `Err` a variant with a payload --
    // so until WIDE-RETURN this signature was rejected outright by
    // cranelift ("Too many return values to fit in registers") and the
    // reader had to keep its failure in a field instead.
    let src = r#"
        fn main() -> u64 {
            val good = json::parse("{{\u{22}n\u{22}: 42, \u{22}xs\u{22}: [1, 2]}}")
            match good {
                Result::Ok(doc) => {
                    val root: u64 = doc.root()
                    println(doc.len(root))
                    val n = doc.get(root, "n")
                    match n {
                        Option::Some(v) => { println(doc.as_int(v)) }
                        Option::None => { println(-1i64) }
                    }
                    val back: String = doc.to_string()
                    println(back)
                }
                Result::Err(e) => { println(e) }
            }

            val empty = json::parse("   ")
            match empty {
                Result::Ok(doc) => { println("read") }
                Result::Err(e) => { println(e) }
            }

            val trailing = json::parse("1 2")
            match trailing {
                Result::Ok(doc) => { println("read") }
                Result::Err(e) => { println(e) }
            }
            0u64
        }
    "#;
    assert_renders(
        src,
        "json_parse",
        "2\n42\n{\"n\":42,\"xs\":[1,2]}\nempty input\ntrailing content at byte 2\n",
    );
}
