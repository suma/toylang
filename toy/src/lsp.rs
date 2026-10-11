//! `toy lsp` (CLAUDE-CODE T4): a Language Server Protocol server on
//! stdin / stdout.
//!
//! It answers from the same two things the command line does: the
//! diagnostics `toy check` prints and the `interpreter::query::Index`
//! `toy query` reads. Nothing here is a second analysis; the server is
//! the protocol layer, the document store and the bookkeeping of which
//! package a file belongs to.
//!
//! * **What is checked.** A document is checked through the package
//!   `toy hook` would check it through (`package::find_checking`): a
//!   module under `src/` through the package's entry, anything else as
//!   the entry it is, and a file in no package on its own against the
//!   stdlib. One analysis per entry, kept while a document needs it.
//! * **Unsaved text.** An open document is read from the editor, not
//!   the disk: the entry directly, a module through
//!   `module_integration::set_module_source_overlay`.
//! * **When.** Changes mark analyses dirty; they are re-checked once
//!   the messages already queued have been handled, so a burst of
//!   keystrokes costs one check. A request re-checks its own analysis
//!   first, so it never answers from text older than it was asked about.
//! * **Positions.** UTF-16 code units, the protocol's default; the
//!   index works in byte offsets of the text it checked.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use frontend::diagnostic::{Applicability, Diagnostic, Severity};
use interpreter::query::{CallableItem, Index, Place, Symbol, SymbolKind};
use serde_json::{Value, json};

use crate::package::{self, Package};

pub fn run() -> i32 {
    let (tx, rx) = mpsc::channel::<Option<Value>>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        loop {
            let message = read_message(&mut input);
            let end = message.is_none();
            if tx.send(message).is_err() || end {
                return;
            }
        }
    });
    let mut server = Server::new();
    loop {
        // Block for one message, then take whatever else is queued
        // before checking: a burst of edits is one check.
        let Ok(first) = rx.recv() else { return 1 };
        let mut batch = vec![first];
        while let Ok(more) = rx.try_recv() {
            batch.push(more);
        }
        for message in batch {
            let Some(message) = message else { return if server.shut_down { 0 } else { 1 } };
            if let Some(code) = server.handle(message) {
                return code;
            }
        }
        server.refresh();
    }
}

/// One `Content-Length`-framed message, or `None` at the end of input.
fn read_message(input: &mut impl BufRead) -> Option<Value> {
    loop {
        let mut length: Option<usize> = None;
        loop {
            let mut line = String::new();
            if input.read_line(&mut line).ok()? == 0 {
                return None;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().ok();
            }
        }
        let Some(length) = length else { continue };
        let mut body = vec![0u8; length];
        input.read_exact(&mut body).ok()?;
        // A body that is not JSON is skipped, not fatal.
        if let Ok(value) = serde_json::from_slice(&body) {
            return Some(value);
        }
    }
}

fn send(message: &Value) {
    let body = message.to_string();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = out.flush();
}

fn notify(method: &str, params: Value) {
    send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
}

/// One checked program: the package, what it was checked from, and
/// what the checker said.
struct Analysis {
    pkg: Package,
    session: compiler_core::CompilerSession,
    /// The last program that parsed. A parse error keeps the previous
    /// one, so navigation still works while a line is half written.
    program: Option<frontend::ast::File>,
    types: rustc_hash::FxHashMap<frontend::ast::ExprRef, frontend::type_decl::TypeDecl>,
    /// What the program's files are called inside it (the entry as
    /// given, a module as `src/greet.t`) to where they are.
    paths: HashMap<String, PathBuf>,
    diagnostics: Vec<(PathBuf, Diagnostic)>,
}

impl Analysis {
    fn index(&self) -> Option<Index<'_>> {
        let program = self.program.as_ref()?;
        Some(Index::new(program, self.session.string_interner(), &self.types))
    }

    /// Where a file the program names is.
    fn path_of(&self, name: &str) -> Option<PathBuf> {
        if let Some(p) = self.paths.get(name) {
            return Some(p.clone());
        }
        let p = Path::new(name);
        if p.is_absolute() {
            return Some(p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
        }
        // A display path is relative to its root's parent (`src/x.t`
        // under the package, `core/std/x.t` under the stdlib's).
        self.pkg
            .module_roots
            .iter()
            .rev()
            .filter_map(|root| root.parent().map(|d| d.join(p)))
            .chain(std::iter::once(self.pkg.root.join(p)))
            .find(|c| c.is_file())
            .map(|c| c.canonicalize().unwrap_or(c))
    }

    /// The name the program gives `path`, when it holds the file.
    fn name_of(&self, path: &Path) -> Option<String> {
        self.paths.iter().find(|(_, p)| p.as_path() == path).map(|(n, _)| n.clone())
    }

    /// The checked text of a file the program holds.
    fn source(&self, name: &str) -> Option<&str> {
        let program = self.program.as_ref()?;
        let (id, _) = program.source_map.iter().find(|(_, f)| f.path == name)?;
        program.source_map.source(id)
    }
}

struct Document {
    uri: String,
    text: String,
}

struct Server {
    stdlib: Vec<PathBuf>,
    workspace: Option<PathBuf>,
    documents: HashMap<PathBuf, Document>,
    /// By entry.
    analyses: HashMap<PathBuf, Analysis>,
    dirty: HashSet<PathBuf>,
    /// What each document was last told, so unchanged sets are not resent.
    published: HashMap<PathBuf, Value>,
    shut_down: bool,
}

impl Server {
    fn new() -> Self {
        Server {
            stdlib: compiler::resolve_core_modules_dirs(Vec::new()),
            workspace: None,
            documents: HashMap::new(),
            analyses: HashMap::new(),
            dirty: HashSet::new(),
            published: HashMap::new(),
            shut_down: false,
        }
    }

    /// Handle one message; `Some(code)` ends the process.
    fn handle(&mut self, message: Value) -> Option<i32> {
        let method = message["method"].as_str().unwrap_or("").to_string();
        let params = message["params"].clone();
        let id = message.get("id").cloned();
        let Some(id) = id else {
            // A notification.
            match method.as_str() {
                "exit" => return Some(if self.shut_down { 0 } else { 1 }),
                "textDocument/didOpen" => {
                    let doc = &params["textDocument"];
                    self.opened(doc["uri"].as_str().unwrap_or(""), doc["text"].as_str().unwrap_or(""));
                }
                "textDocument/didChange" => {
                    // Full sync: the last change is the whole text.
                    if let Some(text) = params["contentChanges"].as_array().and_then(|c| c.last()).and_then(|c| c["text"].as_str()) {
                        self.opened(params["textDocument"]["uri"].as_str().unwrap_or(""), text);
                    }
                }
                "textDocument/didClose" => self.closed(params["textDocument"]["uri"].as_str().unwrap_or("")),
                "textDocument/didSave" | "workspace/didChangeWatchedFiles" => {
                    interpreter::module_integration::forget_discovered_modules();
                    self.dirty.extend(self.analyses.keys().cloned());
                }
                _ => {}
            }
            return None;
        };
        if method.is_empty() {
            return None; // A response to a request this server never sends.
        }
        let reply = match method.as_str() {
            "initialize" => Ok(self.initialize(&params)),
            "shutdown" => {
                self.shut_down = true;
                Ok(Value::Null)
            }
            _ if self.shut_down => Err((-32600, "the server is shutting down".to_string())),
            "textDocument/hover" => Ok(self.hover(&params)),
            "textDocument/definition" => Ok(self.definition(&params)),
            "textDocument/references" => Ok(self.references(&params)),
            "textDocument/implementation" => Ok(self.implementation(&params)),
            "textDocument/documentSymbol" => Ok(self.document_symbols(&params)),
            "workspace/symbol" => Ok(self.workspace_symbols(&params)),
            "textDocument/prepareCallHierarchy" => Ok(self.prepare_call_hierarchy(&params)),
            "callHierarchy/incomingCalls" => Ok(self.calls(&params, true)),
            "callHierarchy/outgoingCalls" => Ok(self.calls(&params, false)),
            "textDocument/codeAction" => Ok(self.code_actions(&params)),
            other => Err((-32601, format!("`{other}` is not supported"))),
        };
        let response = match reply {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        };
        send(&response);
        None
    }

    fn initialize(&mut self, params: &Value) -> Value {
        self.workspace = params["rootUri"]
            .as_str()
            .or_else(|| params["workspaceFolders"][0]["uri"].as_str())
            .and_then(uri_to_path);
        let mut capabilities = json!({
            "positionEncoding": "utf-16",
            "textDocumentSync": { "openClose": true, "change": 1, "save": { "includeText": false } },
            "hoverProvider": true,
            "definitionProvider": true,
            "referencesProvider": true,
            "implementationProvider": true,
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": true,
            "callHierarchyProvider": true,
            "codeActionProvider": { "codeActionKinds": ["quickfix"] },
        });
        if let Some(problem) = compiler::stdlib_problem(&self.stdlib) {
            // Without the stdlib every check fails on `String`; say why
            // once rather than as a type error in every file.
            notify("window/showMessage", json!({ "type": 1, "message": format!("toy lsp: {problem}") }));
            capabilities["experimental"] = json!({ "stdlibMissing": true });
        }
        json!({
            "capabilities": capabilities,
            "serverInfo": { "name": "toy lsp", "version": env!("CARGO_PKG_VERSION") },
        })
    }

    // ---- documents --------------------------------------------------

    fn opened(&mut self, uri: &str, text: &str) {
        let Some(path) = uri_to_path(uri) else { return };
        interpreter::module_integration::set_module_source_overlay(&path, Some(text.to_string()));
        self.documents.insert(path.clone(), Document { uri: uri.to_string(), text: text.to_string() });
        self.touch(&path);
    }

    fn closed(&mut self, uri: &str) {
        let Some(path) = uri_to_path(uri) else { return };
        interpreter::module_integration::set_module_source_overlay(&path, None);
        self.documents.remove(&path);
        // Analyses no open document needs any more go, and with them
        // what they published.
        let needed: HashSet<PathBuf> =
            self.documents.keys().filter_map(|p| self.package_for(p).map(|pkg| pkg.entry)).collect();
        self.analyses.retain(|entry, _| needed.contains(entry));
        self.dirty.retain(|entry| needed.contains(entry));
        self.touch_holders(&path);
        if !self.documents.contains_key(&path) && !self.analyses.values().any(|a| a.name_of(&path).is_some()) {
            self.publish(&path, Vec::new());
        }
    }

    /// `path` changed: its own analysis, and every analysis holding it,
    /// need checking again.
    fn touch(&mut self, path: &Path) {
        if let Some(pkg) = self.package_for(path) {
            self.dirty.insert(pkg.entry.clone());
            self.analyses.entry(pkg.entry.clone()).or_insert_with(|| Analysis {
                pkg,
                session: compiler_core::CompilerSession::new(),
                program: None,
                types: Default::default(),
                paths: HashMap::new(),
                diagnostics: Vec::new(),
            });
        }
        self.touch_holders(path);
    }

    fn touch_holders(&mut self, path: &Path) {
        for (entry, a) in &self.analyses {
            if a.name_of(path).is_some() {
                self.dirty.insert(entry.clone());
            }
        }
    }

    /// The package a document is checked through.
    fn package_for(&self, path: &Path) -> Option<Package> {
        if !path.is_file() {
            return None;
        }
        match package::find_checking(path, self.stdlib.clone()) {
            Ok(pkg) => Some(pkg),
            // In no package: on its own, against the stdlib.
            Err(_) => Some(Package {
                root: path.parent()?.to_path_buf(),
                entry: path.to_path_buf(),
                module_roots: self.stdlib.clone(),
                build_dir: path.parent()?.join("build"),
            }),
        }
    }

    // ---- checking ---------------------------------------------------

    /// Check every dirty analysis and publish what changed.
    fn refresh(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        let dirty: Vec<PathBuf> = self.dirty.drain().collect();
        let mut files: HashSet<PathBuf> = self.published.keys().cloned().collect();
        for entry in dirty {
            if let Some(a) = self.analyses.get_mut(&entry) {
                check(a, &self.documents);
                files.extend(a.diagnostics.iter().map(|(p, _)| p.clone()));
            }
        }
        files.extend(self.documents.keys().cloned());
        for file in files {
            let diagnostics = self.diagnostics_for(&file);
            self.publish(&file, diagnostics);
        }
    }

    /// The analysis a request about `path` reads, checked first if it
    /// is stale: the file's own package, else any analysis holding it
    /// (a stdlib file opened from a definition).
    fn analysis_for(&mut self, path: &Path) -> Option<&Analysis> {
        let own = self.package_for(path).map(|p| p.entry);
        let entry = match own.filter(|e| self.analyses.contains_key(e)) {
            Some(e) => e,
            None => match self.analyses.iter().find(|(_, a)| a.name_of(path).is_some()) {
                Some((e, _)) => e.clone(),
                None => {
                    self.touch(path);
                    self.package_for(path)?.entry
                }
            },
        };
        if self.dirty.contains(&entry) {
            self.refresh();
        }
        self.analyses.get(&entry)
    }

    /// Every analysis's diagnostics for `file`, once each.
    fn diagnostics_for(&self, file: &Path) -> Vec<Value> {
        let text = self.text_of(file);
        let mut out: Vec<Value> = Vec::new();
        for a in self.analyses.values() {
            for (path, d) in &a.diagnostics {
                if path != file {
                    continue;
                }
                let lsp = to_lsp_diagnostic(a, d, &text);
                if !out.contains(&lsp) {
                    out.push(lsp);
                }
            }
        }
        out
    }

    fn publish(&mut self, file: &Path, diagnostics: Vec<Value>) {
        let value = Value::Array(diagnostics);
        if self.published.get(file) == Some(&value) {
            return;
        }
        if value.as_array().is_some_and(Vec::is_empty) && !self.published.contains_key(file) {
            return;
        }
        let uri = self.uri_of(file);
        notify("textDocument/publishDiagnostics", json!({ "uri": uri, "diagnostics": value }));
        if value.as_array().is_some_and(Vec::is_empty) {
            self.published.remove(file);
        } else {
            self.published.insert(file.to_path_buf(), value);
        }
    }

    /// The text the editor has, else the disk's.
    fn text_of(&self, path: &Path) -> String {
        match self.documents.get(path) {
            Some(d) => d.text.clone(),
            None => std::fs::read_to_string(path).unwrap_or_default(),
        }
    }

    /// The URI the client used for an open document (it may differ from
    /// the canonical path, `/tmp` against `/private/tmp`), else one made.
    fn uri_of(&self, path: &Path) -> String {
        self.documents.get(path).map(|d| d.uri.clone()).unwrap_or_else(|| path_to_uri(path))
    }

    // ---- requests ---------------------------------------------------

    /// The analysis, its index and the byte offset a position names.
    fn at<R>(&mut self, params: &Value, answer: impl FnOnce(&Analysis, &Index, frontend::source_map::FileId, usize) -> R) -> Option<R> {
        let path = uri_to_path(params["textDocument"]["uri"].as_str()?)?;
        let line = params["position"]["line"].as_u64()? as usize;
        let character = params["position"]["character"].as_u64()? as usize;
        let a = self.analysis_for(&path)?;
        let name = a.name_of(&path)?;
        let index = a.index()?;
        let file = index.file_named(&name)?;
        let offset = position_to_offset(a.source(&name)?, line, character);
        Some(answer(a, &index, file, offset))
    }

    fn hover(&mut self, params: &Value) -> Value {
        let uris = self.uris();
        self.at(params, |a, index, file, offset| {
            let (text, range) = index.hover_at(file, offset)?;
            let mut out = json!({ "contents": { "kind": "markdown", "value": text } });
            if let Some(r) = range.and_then(|r| location(a, &r, &uris)) {
                out["range"] = r["range"].clone();
            }
            Some(out)
        })
        .flatten()
        .unwrap_or(Value::Null)
    }

    fn definition(&mut self, params: &Value) -> Value {
        let uris = self.uris();
        self.at(params, |a, index, file, offset| index.definition_at(file, offset).and_then(|p| location(a, &p, &uris)))
            .flatten()
            .unwrap_or(Value::Null)
    }

    fn references(&mut self, params: &Value) -> Value {
        let uris = self.uris();
        let include = params["context"]["includeDeclaration"].as_bool().unwrap_or(true);
        let found = self.at(params, |a, index, file, offset| {
            index.references_at(file, offset, include).iter().filter_map(|p| location(a, p, &uris)).collect::<Vec<_>>()
        });
        Value::Array(found.unwrap_or_default())
    }

    fn implementation(&mut self, params: &Value) -> Value {
        let uris = self.uris();
        let found = self.at(params, |a, index, file, offset| {
            index.implementations_at(file, offset).iter().filter_map(|p| location(a, p, &uris)).collect::<Vec<_>>()
        });
        Value::Array(found.unwrap_or_default())
    }

    fn document_symbols(&mut self, params: &Value) -> Value {
        let Some(path) = params["textDocument"]["uri"].as_str().and_then(uri_to_path) else { return Value::Null };
        let Some(a) = self.analysis_for(&path) else { return Value::Null };
        let Some(name) = a.name_of(&path) else { return json!([]) };
        let Some(index) = a.index() else { return json!([]) };
        let Some(file) = index.file_named(&name) else { return json!([]) };
        let Some(source) = a.source(&name) else { return json!([]) };
        Value::Array(index.document_symbols(file).iter().map(|s| document_symbol(s, source)).collect())
    }

    fn workspace_symbols(&mut self, params: &Value) -> Value {
        let query = params["query"].as_str().unwrap_or("");
        if self.analyses.is_empty() {
            // Nothing open yet: the workspace's own package.
            if let Some(entry) = self
                .workspace
                .clone()
                .and_then(|w| package::find(&w, self.stdlib.clone()).ok())
                .map(|pkg| pkg.entry)
            {
                self.touch(&entry);
            }
        }
        self.refresh();
        let uris = self.uris();
        let mut out: Vec<Value> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for a in self.analyses.values() {
            let Some(index) = a.index() else { continue };
            for s in index.workspace_symbols(query) {
                let Some(loc) = location(a, &s.selection, &uris) else { continue };
                if !seen.insert(format!("{}{}", s.name, loc)) {
                    continue;
                }
                let mut info = json!({ "name": s.name, "kind": symbol_kind(s.kind), "location": loc });
                if let Some(c) = &s.container {
                    info["containerName"] = json!(c);
                }
                out.push(info);
                if out.len() >= 500 {
                    return Value::Array(out);
                }
            }
        }
        Value::Array(out)
    }

    fn prepare_call_hierarchy(&mut self, params: &Value) -> Value {
        let uris = self.uris();
        self.at(params, |a, index, file, offset| {
            let item = index.callable_at(file, offset)?;
            Some(json!([call_item(a, &item, &uris)?]))
        })
        .flatten()
        .unwrap_or(Value::Null)
    }

    fn calls(&mut self, params: &Value, incoming: bool) -> Value {
        let item = &params["item"];
        let Some(path) = item["uri"].as_str().and_then(uri_to_path) else { return Value::Null };
        let (Some(name), Some(file), Some(offset)) =
            (item["data"]["name"].as_str(), item["data"]["file"].as_str(), item["data"]["offset"].as_u64())
        else {
            return Value::Null;
        };
        let uris = self.uris();
        let Some(a) = self.analysis_for(&path) else { return Value::Null };
        let Some(index) = a.index() else { return Value::Null };
        let at = Place { file: file.to_string(), line: 0, column: 0, offset: offset as u32, end_offset: offset as u32 };
        let wanted = CallableItem { name: name.to_string(), method: false, range: at.clone(), selection: at };
        let edges = if incoming { index.incoming_calls(&wanted) } else { index.outgoing_calls(&wanted) };
        let out: Vec<Value> = edges
            .iter()
            .filter_map(|(other, sites)| {
                let other = call_item(a, other, &uris)?;
                // The call sites are in the caller: the other end for
                // incoming, `item` for outgoing.
                let ranges: Vec<Value> =
                    sites.iter().filter_map(|p| location(a, p, &uris)).map(|l| l["range"].clone()).collect();
                Some(if incoming { json!({ "from": other, "fromRanges": ranges }) } else { json!({ "to": other, "fromRanges": ranges }) })
            })
            .collect();
        Value::Array(out)
    }

    /// The suggestions of the diagnostics under the range, as edits.
    fn code_actions(&mut self, params: &Value) -> Value {
        let Some(path) = params["textDocument"]["uri"].as_str().and_then(uri_to_path) else { return json!([]) };
        if self.analysis_for(&path).is_none() {
            return json!([]);
        }
        let text = self.text_of(&path);
        let start = position_to_offset(&text, line_of(&params["range"]["start"]), char_of(&params["range"]["start"]));
        let end = position_to_offset(&text, line_of(&params["range"]["end"]), char_of(&params["range"]["end"]));
        let mut out: Vec<Value> = Vec::new();
        for a in self.analyses.values() {
            for (file, d) in &a.diagnostics {
                if file != &path {
                    continue;
                }
                let Some(span) = d.span else { continue };
                let (s, e) = (span.offset as usize, span.end_offset as usize);
                if !(s <= end && start <= e.max(s + 1)) {
                    continue;
                }
                for suggestion in &d.suggestions {
                    let Some(edit) = self.workspace_edit(a, suggestion) else { continue };
                    let action = json!({
                        "title": suggestion.message,
                        "kind": "quickfix",
                        "diagnostics": [to_lsp_diagnostic(a, d, &text)],
                        "isPreferred": suggestion.applicability == Applicability::MachineApplicable,
                        "edit": edit,
                    });
                    if !out.contains(&action) {
                        out.push(action);
                    }
                }
            }
        }
        Value::Array(out)
    }

    fn workspace_edit(&self, a: &Analysis, suggestion: &frontend::diagnostic::Suggestion) -> Option<Value> {
        let mut changes: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for edit in &suggestion.edits {
            let (Some(file), Some(span)) = (&edit.file, edit.span) else { return None };
            let path = a.path_of(file)?;
            let text = self.text_of(&path);
            changes.entry(self.uri_of(&path)).or_default().push(json!({
                "range": range(&text, span.offset as usize, span.end_offset as usize),
                "newText": edit.replacement,
            }));
        }
        Some(json!({ "changes": changes }))
    }

    /// Open documents' URIs by path, for answers that land in them.
    fn uris(&self) -> HashMap<PathBuf, String> {
        self.documents.iter().map(|(p, d)| (p.clone(), d.uri.clone())).collect()
    }
}

/// Check one analysis against the editor's text.
fn check(a: &mut Analysis, documents: &HashMap<PathBuf, Document>) {
    let entry = a.pkg.entry.clone();
    let source = match documents.get(&entry) {
        Some(d) => d.text.clone(),
        None => match std::fs::read_to_string(&entry) {
            Ok(s) => s,
            Err(_) => return,
        },
    };
    let name = entry.to_string_lossy().into_owned();
    let mut session = compiler_core::CompilerSession::new();
    let diagnostics = match session.parse_program_all_errors(&source, &name) {
        Ok(mut program) => {
            let mut types = rustc_hash::FxHashMap::default();
            let result = interpreter::check_typing_with_types(
                &mut program,
                session.string_interner_mut(),
                Some(&source),
                Some(&name),
                &a.pkg.module_roots,
                &mut types,
            );
            a.session = session;
            a.program = Some(program);
            a.types = types;
            a.paths = file_paths(a);
            match result {
                Ok(warnings) => warnings,
                Err(errors) => errors,
            }
        }
        Err(_) => interpreter::diagnose_parse_failure(&source, &name, &a.pkg.module_roots),
    };
    a.diagnostics = diagnostics
        .into_iter()
        .filter_map(|d| Some((a.path_of(&d.file).or_else(|| Some(entry.clone()))?, d)))
        .collect();
}

/// Where each file of the checked program is: the entry is named as
/// given, a module by its root's display path.
fn file_paths(a: &Analysis) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    let entry_name = a.pkg.entry.to_string_lossy().into_owned();
    out.insert(entry_name, a.pkg.entry.clone());
    if let Ok(modules) =
        interpreter::module_integration::discover_core_modules_multi(&a.pkg.module_roots, Some(&a.pkg.entry))
    {
        for m in modules {
            out.insert(m.display_path, m.path);
        }
    }
    if let Some(program) = &a.program {
        for (_, f) in program.source_map.iter() {
            if !out.contains_key(&f.path) {
                // An `import`ed file outside the walked roots.
                let p = Path::new(&f.path);
                if p.is_file() {
                    out.insert(f.path.clone(), p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
                }
            }
        }
    }
    out
}

fn to_lsp_diagnostic(a: &Analysis, d: &Diagnostic, text: &str) -> Value {
    let span_range = |file_text: &str, s: Option<frontend::diagnostic::Span>| match s {
        Some(s) => range(file_text, s.offset as usize, s.end_offset as usize),
        None => json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }),
    };
    let mut out = json!({
        "range": span_range(text, d.span),
        "severity": match d.severity { Severity::Error => 1, Severity::Warning => 2 },
        "code": d.code,
        "source": "toy",
        "message": d.message,
    });
    let related: Vec<Value> = d
        .related
        .iter()
        .filter_map(|r| {
            let path = a.path_of(r.file.as_deref().unwrap_or(&d.file))?;
            let file_text = std::fs::read_to_string(&path).ok()?;
            Some(json!({
                "location": { "uri": path_to_uri(&path), "range": span_range(&file_text, r.span) },
                "message": r.message,
            }))
        })
        .collect();
    if !related.is_empty() {
        out["relatedInformation"] = Value::Array(related);
    }
    out
}

/// A place in the program as an LSP `Location`.
fn location(a: &Analysis, p: &Place, uris: &HashMap<PathBuf, String>) -> Option<Value> {
    let path = a.path_of(&p.file)?;
    let source = a.source(&p.file)?;
    let uri = uris.get(&path).cloned().unwrap_or_else(|| path_to_uri(&path));
    Some(json!({ "uri": uri, "range": range(source, p.offset as usize, p.end_offset as usize) }))
}

fn call_item(a: &Analysis, item: &CallableItem, uris: &HashMap<PathBuf, String>) -> Option<Value> {
    let full = location(a, &item.range, uris)?;
    let selection = location(a, &item.selection, uris)?;
    Some(json!({
        "name": item.name,
        "kind": if item.method { 6 } else { 12 },
        "uri": full["uri"],
        "range": full["range"],
        "selectionRange": selection["range"],
        "data": { "name": item.name, "file": item.selection.file, "offset": item.selection.offset },
    }))
}

fn document_symbol(s: &Symbol, source: &str) -> Value {
    // The protocol wants the selection inside the range.
    let start = s.range.offset.min(s.selection.offset) as usize;
    let end = s.range.end_offset.max(s.selection.end_offset) as usize;
    let mut out = json!({
        "name": s.name,
        "kind": symbol_kind(s.kind),
        "range": range(source, start, end),
        "selectionRange": range(source, s.selection.offset as usize, s.selection.end_offset as usize),
        "children": s.children.iter().map(|c| document_symbol(c, source)).collect::<Vec<_>>(),
    });
    if s.kind == SymbolKind::Test {
        out["detail"] = json!("test");
    }
    out
}

fn symbol_kind(kind: SymbolKind) -> u32 {
    match kind {
        SymbolKind::Function | SymbolKind::Test => 12,
        SymbolKind::Method => 6,
        SymbolKind::Struct => 23,
        SymbolKind::Enum => 10,
        SymbolKind::Trait => 11,
        SymbolKind::Impl => 3, // Namespace: a group of methods
        SymbolKind::Field => 8,
        SymbolKind::Variant => 22,
        SymbolKind::Const => 14,
    }
}

// ---- positions and URIs -------------------------------------------------

fn line_of(p: &Value) -> usize {
    p["line"].as_u64().unwrap_or(0) as usize
}

fn char_of(p: &Value) -> usize {
    p["character"].as_u64().unwrap_or(0) as usize
}

fn range(text: &str, start: usize, end: usize) -> Value {
    let (sl, sc) = offset_to_position(text, start);
    let (el, ec) = offset_to_position(text, end.max(start));
    json!({ "start": { "line": sl, "character": sc }, "end": { "line": el, "character": ec } })
}

/// A byte offset as (line, UTF-16 column), both 0-based.
pub fn offset_to_position(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..floor_char_boundary(text, offset)];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = before[line_start..].encode_utf16().count();
    (line, column)
}

/// (line, UTF-16 column) as a byte offset; past the end of a line is
/// its end, past the last line the end of the text.
pub fn position_to_offset(text: &str, line: usize, character: usize) -> usize {
    let mut start = 0;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let mut units = 0;
    for (i, c) in text[start..end].char_indices() {
        if units >= character {
            return start + i;
        }
        units += c.len_utf16();
    }
    end
}

fn floor_char_boundary(text: &str, mut i: usize) -> usize {
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `file:///a%20b/c.t` to `/a b/c.t`, canonicalised when it exists.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://localhost/...` names the same file.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = rest.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            decoded.push(b);
            i += 3;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    let path = PathBuf::from(String::from_utf8(decoded).ok()?);
    Some(path.canonicalize().unwrap_or(path))
}

pub fn path_to_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_count_utf16_units() {
        let text = "ab\n😀x\n";
        assert_eq!(offset_to_position(text, 0), (0, 0));
        assert_eq!(offset_to_position(text, 3), (1, 0));
        // The emoji is 4 bytes and 2 UTF-16 units.
        assert_eq!(offset_to_position(text, 7), (1, 2));
        assert_eq!(position_to_offset(text, 1, 2), 7);
        assert_eq!(position_to_offset(text, 1, 99), 8);
        assert_eq!(position_to_offset(text, 9, 0), text.len());
    }

    #[test]
    fn uris_round_trip_through_percent_encoding() {
        let p = Path::new("/no such dir/a b.t");
        let uri = path_to_uri(p);
        assert_eq!(uri, "file:///no%20such%20dir/a%20b.t");
        assert_eq!(uri_to_path(&uri).as_deref(), Some(p));
    }
}
