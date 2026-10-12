//! `toy lsp` (CLAUDE-CODE T4) over its real transport: the binary on
//! stdin / stdout, framed messages in, framed messages out.
//!
//! The answers themselves are `interpreter::query`'s and the
//! diagnostics `toy check`'s; what is pinned here is the protocol
//! layer — positions in UTF-16 lines and columns, the package a
//! document is checked through, unsaved text winning over the disk,
//! and the lifecycle.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct Pkg(PathBuf);

impl Drop for Pkg {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(stem: &str) -> Pkg {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let p = std::env::temp_dir().join(format!("toy_lsp_{stem}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(p.join("src")).expect("create package");
    Pkg(p.canonicalize().expect("canonical"))
}

fn write(pkg: &Pkg, rel: &str, text: &str) {
    std::fs::write(pkg.0.join(rel), text).expect("write");
}

const GREET: &str = "\
pub fn twice(n: u64) -> u64 {
    n * 2u64
}
";

const MAIN: &str = "\
# The entry.
fn main() -> u64 {
    val a: u64 = greet::twice(4u64)
    val b: u64 = a + 1u64
    b
}
";

struct Server {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
    /// Notifications that arrived while waiting for a response.
    notes: Vec<Value>,
}

impl Server {
    fn start(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_toy"))
            .arg("lsp")
            .env("TOYLANG_CORE_MODULES", concat!(env!("CARGO_MANIFEST_DIR"), "/../core"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn toy lsp");
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut s = Server { child, input, output, next_id: 0, notes: Vec::new() };
        let init = s.request("initialize", json!({ "rootUri": uri(root), "capabilities": {} }));
        assert_eq!(init["capabilities"]["definitionProvider"], json!(true), "{init}");
        s.notify("initialized", json!({}));
        s
    }

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        write!(self.input, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.input.flush().unwrap();
    }

    fn receive(&mut self) -> Value {
        let mut length = 0;
        loop {
            let mut line = String::new();
            assert!(self.output.read_line(&mut line).unwrap() > 0, "the server closed its output");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                length = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        self.output.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let m = self.receive();
            if m["id"] == json!(id) {
                return m.get("result").cloned().unwrap_or_else(|| m["error"].clone());
            }
            self.notes.push(m);
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn open(&mut self, path: &Path, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri(path), "languageId": "toy", "version": 1, "text": text } }),
        );
    }

    fn change(&mut self, path: &Path, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri(path), "version": 2 }, "contentChanges": [{ "text": text }] }),
        );
    }

    /// The latest diagnostics published for `path`, once a request has
    /// flushed the queue (the server publishes before it answers).
    fn diagnostics(&mut self, path: &Path) -> Vec<Value> {
        self.request("workspace/symbol", json!({ "query": "\u{0}" }));
        self.notes
            .iter()
            .rev()
            .find(|n| n["method"] == "textDocument/publishDiagnostics" && n["params"]["uri"] == json!(uri(path)))
            .map(|n| n["params"]["diagnostics"].as_array().cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    fn at(&mut self, method: &str, path: &Path, line: u32, character: u32) -> Value {
        self.request(
            method,
            json!({
                "textDocument": { "uri": uri(path) },
                "position": { "line": line, "character": character },
                "context": { "includeDeclaration": true },
            }),
        )
    }

    fn stop(mut self) -> i32 {
        assert_eq!(self.request("shutdown", Value::Null), Value::Null);
        self.notify("exit", Value::Null);
        self.child.wait().unwrap().code().unwrap_or(-1)
    }
}

fn uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn package() -> Pkg {
    let pkg = scratch("basic");
    write(&pkg, "main.t", MAIN);
    write(&pkg, "src/greet.t", GREET);
    pkg
}

#[test]
fn hover_definition_and_references_answer_from_the_checked_program() {
    let pkg = package();
    let main = pkg.0.join("main.t");
    let mut s = Server::start(&pkg.0);
    s.open(&main, MAIN);

    // `twice` in `greet::twice(4u64)`: line 2, after `val a: u64 = greet::`.
    let hover = s.at("textDocument/hover", &main, 2, 24);
    let text = hover["contents"]["value"].as_str().unwrap_or("");
    assert!(text.contains("fn twice(n: u64) -> u64") && text.contains("greet::twice"), "{hover}");

    let def = s.at("textDocument/definition", &main, 2, 24);
    assert_eq!(def["uri"], json!(uri(&pkg.0.join("src/greet.t"))), "{def}");
    assert_eq!(def["range"]["start"], json!({ "line": 0, "character": 7 }), "{def}");

    // `a`: declared on line 2, used on line 3.
    let refs = s.at("textDocument/references", &main, 3, 17);
    let lines: Vec<u64> = refs.as_array().unwrap().iter().map(|r| r["range"]["start"]["line"].as_u64().unwrap()).collect();
    assert_eq!(lines, vec![2, 3], "{refs}");

    let hover = s.at("textDocument/hover", &main, 3, 17);
    assert!(hover["contents"]["value"].as_str().unwrap_or("").contains("`u64`"), "{hover}");

    let symbols = s.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri(&main) } }));
    assert_eq!(symbols[0]["name"], json!("main"), "{symbols}");

    assert_eq!(s.stop(), 0);
}

#[test]
fn unsaved_text_is_checked_and_a_quick_fix_is_offered() {
    let pkg = package();
    let main = pkg.0.join("main.t");
    let mut s = Server::start(&pkg.0);
    s.open(&main, MAIN);
    assert!(s.diagnostics(&main).is_empty());

    // Not saved: the disk still has the good text.
    let broken = MAIN.replace("    b\n", "    if b > 3u64 { 1u64 } else if b > 1u64 { 2u64 } else { 3u64 }\n");
    s.change(&main, &broken);
    let diags = s.diagnostics(&main);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["code"], json!("E0033"));
    assert_eq!(diags[0]["range"]["start"]["line"], json!(4));

    let actions = s.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri(&main) },
            "range": diags[0]["range"],
            "context": { "diagnostics": [] },
        }),
    );
    let edit = &actions[0]["edit"]["changes"][uri(&main)][0];
    assert_eq!(edit["newText"], json!("elif"), "{actions}");
    assert_eq!(edit["range"], diags[0]["range"], "{actions}");

    s.change(&main, MAIN);
    assert!(s.diagnostics(&main).is_empty());
    assert_eq!(s.stop(), 0);
}

#[test]
fn an_unsaved_module_is_what_the_entry_is_checked_against() {
    let pkg = package();
    let main = pkg.0.join("main.t");
    let greet = pkg.0.join("src/greet.t");
    let mut s = Server::start(&pkg.0);
    s.open(&main, MAIN);
    s.open(&greet, &GREET.replace("twice", "double"));
    let diags = s.diagnostics(&main);
    assert!(diags.iter().any(|d| d["code"] == json!("E0003")), "{diags:?}");

    // Closed without saving: the disk's module again.
    s.notify("textDocument/didClose", json!({ "textDocument": { "uri": uri(&greet) } }));
    assert!(s.diagnostics(&main).is_empty());
    assert_eq!(s.stop(), 0);
}

#[test]
fn a_file_that_does_not_parse_still_has_an_outline_and_types() {
    // Never parsed cleanly, so there is no earlier program to fall back
    // on: the outline is the recovered tree's, at the text's positions.
    let pkg = scratch("recover");
    let text = "\
fn helper(o: Option<u64>) -> u64 {
    match o {
        Option::Some(v) => v + 1u64,
        Option::None => 0u64,
    }
}

fn main() -> u64 {
    if 1u64 > 0u64 { 1u64 } else if 2u64 > 1u64 { 2u64 } else { 3u64 }
}
";
    write(&pkg, "main.t", text);
    let main = pkg.0.join("main.t");
    let mut s = Server::start(&pkg.0);
    s.open(&main, text);
    let diags = s.diagnostics(&main);
    assert!(diags.iter().any(|d| d["code"] == json!("E0033")), "{diags:?}");
    let symbols = s.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri(&main) } }));
    let names: Vec<(&str, u64)> = symbols
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["name"].as_str().unwrap(), x["selectionRange"]["start"]["line"].as_u64().unwrap()))
        .collect();
    assert_eq!(names, vec![("helper", 0), ("main", 7)], "{symbols}");

    // A match arm's payload, used only as an operand, has a type.
    let hover = s.at("textDocument/hover", &main, 2, 21);
    assert!(hover["contents"]["value"].as_str().unwrap_or("").contains("`u64`"), "{hover}");

    // An edit that shifts the lines: the outline follows the text, not
    // the program from before the edit.
    s.change(&main, &format!("# a comment\n\n{text}"));
    let symbols = s.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri(&main) } }));
    assert_eq!(symbols[1]["selectionRange"]["start"]["line"], json!(9), "{symbols}");
    assert_eq!(s.stop(), 0);
}

#[test]
fn columns_are_utf16_code_units() {
    let pkg = scratch("utf16");
    // The emoji is 4 bytes but 2 UTF-16 units, so the `n` after it is
    // at byte column 36 and UTF-16 column 34.
    let text = "fn main() -> u64 {\n    val n: u64 = 1u64\n    val m: u64 = if \"\u{1F600}\" == \"x\" { n } else { 2u64 }\n    m\n}\n";
    write(&pkg, "main.t", text);
    let main = pkg.0.join("main.t");
    let mut s = Server::start(&pkg.0);
    s.open(&main, text);
    let refs = s.at("textDocument/references", &main, 1, 8);
    let starts: Vec<&Value> = refs.as_array().unwrap().iter().map(|r| &r["range"]["start"]).collect();
    assert_eq!(starts, vec![&json!({ "line": 1, "character": 8 }), &json!({ "line": 2, "character": 34 })], "{refs}");
    // And a position given in UTF-16 units finds the same name.
    let def = s.at("textDocument/definition", &main, 2, 34);
    assert_eq!(def["range"]["start"], json!({ "line": 1, "character": 8 }), "{def}");
    assert_eq!(s.stop(), 0);
}

#[test]
fn exit_without_shutdown_is_an_error() {
    let pkg = package();
    let mut s = Server::start(&pkg.0);
    s.notify("exit", Value::Null);
    assert_eq!(s.child.wait().unwrap().code(), Some(1));
}
