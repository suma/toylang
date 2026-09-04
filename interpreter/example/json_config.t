# JSON (STDLIB-SERIALIZE).
#
# Two entrances: `JsonWriter` for writing, which never builds a tree,
# and `Json` for reading one.

# Writing: a struct goes out field by field. This is what to reach
# for first -- the only allocation is the string growing.
fn render(port: u64, host: str, debug: bool) -> String {
    var w: JsonWriter = JsonWriter::new()
    w.begin_object()
    w.key("host")
    w.str_value(host)
    w.key("port")
    w.u64_value(port)
    w.key("debug")
    w.bool_value(debug)
    w.key("retries")
    w.begin_array()
    w.u64_value(1u64)
    w.u64_value(2u64)
    w.u64_value(5u64)
    w.end_array()
    w.end_object()
    val doc: String = w.finish()
    doc
}

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
    val config: String = render(8080u64, "localhost", false)
    println(config)

    # Reading. `read` answers with the root index, or with why it
    # stopped.
    var d: Json = Json::new()
    val text: str = config.to_str()
    val r = d.read(text)
    match r {
        Result::Ok(root) => {
            var i: u64 = 0u64
            while i < d.len(root) {
                val key: str = d.key_at(root, i)
                val value: u64 = d.value_at(root, i)
                val kind: JsonKind = d.kind(value)
                val name: str = kind_name(kind)
                println("{key} is a {name}")
                i = i + 1u64
            }
            val port = d.get(root, "port")
            match port {
                Option::Some(v) => { println(d.as_int(v)) }
                Option::None => { println("no port") }
            }
            # The document prints back as the writer wrote it.
            val again: String = d.to_string()
            println(again.eq_str(text))
        }
        Result::Err(e) => { println(e) }
    }

    # A reader that takes no extensions says where it stopped.
    # `json::parse` is the same reader with the document made for you.
    val r2 = json::parse("{{\u{22}a\u{22}: 1,}}")
    match r2 {
        Result::Ok(doc) => { println("accepted") }
        Result::Err(e) => { println(e) }
    }
    0u64
}
