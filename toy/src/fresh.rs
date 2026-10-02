//! Skipping a build whose inputs have not changed (BUILD-TOOL D6).
//!
//! `toy build` used to compile from scratch on every call: 80 ms for
//! `poc/logsearch` even when nothing had changed, which is the whole
//! cost of an edit-free "build, then run" loop. A build now leaves a
//! **stamp** next to its output — a plain-text list of everything the
//! output was made from — and the next build with the same stamp and an
//! untouched output does nothing.
//!
//! What the stamp names, and why each is enough:
//!
//! - **the `toy` executable itself** (path, size, mtime). The compiler,
//!   the prelude and the runtime archive are all linked into it, so a
//!   rebuilt toolchain is a different file.
//! - **the flags that change the output** (`--release`, heap-check
//!   mode, the output path) and the **environment variables the
//!   compiler reads** (`TOY*`, `INTERPRETER_*`, `CC`).
//! - **the entry file, and every file and directory under every module
//!   root**, by size and mtime. The whole tree rather than the files a
//!   compile happened to read: a *new* file can change what a module
//!   path resolves to (a later root shadows the stdlib), and a
//!   directory's mtime moves when an entry is added or removed.
//! - **the output** (size, mtime), so a binary someone replaced or
//!   deleted is rebuilt.
//!
//! Size + mtime is what `make` and cargo trust; the walk is about a
//! millisecond for `poc/logsearch` (two roots, ~70 files).
//!
//! The stamp is text on purpose: when a build is skipped that should
//! not have been, `diff` on two stamps says why.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const HEADER: &str = "toy build stamp v1\n";

/// The inputs side of a stamp: everything but the output. Taken
/// **before** compiling, so a file edited mid-build leaves a stamp
/// that no longer matches and the next build runs.
pub struct Inputs {
    text: String,
}

impl Inputs {
    /// `flags` is whatever the caller wants to count as part of the
    /// build's identity, one per line.
    pub fn collect(entry: &Path, module_roots: &[PathBuf], flags: &[String]) -> Inputs {
        let mut text = String::from(HEADER);
        match std::env::current_exe() {
            Ok(exe) => text.push_str(&format!("exe {}\n", describe(&exe))),
            // Without knowing which compiler built the output, no
            // stamp can be trusted; one that never matches does that.
            Err(_) => text.push_str("exe ?\n"),
        }
        for flag in flags {
            text.push_str(&format!("flag {flag}\n"));
        }
        let mut env: Vec<(String, String)> = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.to_string_lossy().into_owned())))
            .filter(|(k, _)| k.starts_with("TOY") || k.starts_with("INTERPRETER_") || k == "CC")
            .collect();
        env.sort();
        for (k, v) in env {
            text.push_str(&format!("env {k}={v}\n"));
        }
        text.push_str(&format!("in {}\n", describe(entry)));
        for root in module_roots {
            text.push_str(&format!("root {}\n", describe(root)));
            walk(root, &mut text);
        }
        Inputs { text }
    }

    /// Whether `output` was built from exactly these inputs and has not
    /// been touched since.
    pub fn is_fresh(&self, stamp: &Path, output: &Path) -> bool {
        match fs::read_to_string(stamp) {
            Ok(saved) => saved == self.with_output(output),
            Err(_) => false,
        }
    }

    /// Record that `output` is now built from these inputs. Best
    /// effort: a stamp that cannot be written only costs the next build
    /// its skip.
    pub fn record(&self, stamp: &Path, output: &Path) {
        if let Some(dir) = stamp.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(stamp, self.with_output(output));
    }

    fn with_output(&self, output: &Path) -> String {
        format!("{}out {}\n", self.text, describe(output))
    }
}

/// Forget the stamp before a build starts, so a build that fails part
/// way cannot leave an old stamp describing a half-written output.
pub fn invalidate(stamp: &Path) {
    let _ = fs::remove_file(stamp);
}

/// `path size mtime_ns`, or `path -` when it does not exist.
fn describe(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("{} {} {}", path.display(), meta.len(), mtime)
        }
        Err(_) => format!("{} -", path.display()),
    }
}

/// Every entry under `dir`, sorted, one line each. Symlinks are
/// described (through the link) but not followed into, so a loop
/// cannot hang the walk.
fn walk(dir: &Path, text: &mut String) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        text.push_str(&format!("  {}\n", describe(&path)));
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&path, text);
        }
    }
}
