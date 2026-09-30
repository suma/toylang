//! `toy hook` (CLAUDE-CODE T2): check a `.t` file right after Claude
//! Code edits it, and hand the errors back.
//!
//! Run as a `PostToolUse` hook on `Edit|Write`. The hook input arrives
//! on stdin as JSON; the edited path is `tool_input.file_path`. The
//! exit code is the answer Claude Code acts on:
//!
//! * `0`, nothing printed — not a `.t` file, not in a package, or it
//!   checks clean. A clean check says nothing, so a successful edit
//!   costs no context.
//! * `0`, a JSON `additionalContext` on stdout — only warnings.
//! * `2`, the errors on stderr, one per line (`--format=short`) — Claude
//!   Code feeds stderr of a blocking hook back to the model, which fixes
//!   them next.
//!
//! What is checked: a module under the package's `src/` is part of the
//! package, so the package's entry is checked (checked as an entry of
//! its own, a module's references to itself by name — `greet::double`
//! inside `greet.t` — would not resolve). Anything else (`main.t`,
//! `tests/*.t`) is checked as the entry it is. Type checking only, no
//! lowering: the `--backend vm` question, answered in ~20 ms.
//!
//! It never edits the file. Applying a fix behind Claude's back would
//! make its next `Edit` miss the text it expects; the fixes are named,
//! and `toy fix` applies them when Claude chooses to.

use std::io::Read;
use std::path::{Path, PathBuf};

use frontend::diagnostic::{Applicability, Diagnostic, Severity};

use crate::package;

/// At most this many errors go back per edit: enough to fix in one go,
/// few enough not to crowd the context. The rest are counted.
const MAX_ERRORS: usize = 10;

pub fn run() -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    let Some(file) = edited_file(&input) else { return 0 };
    if file.extension().and_then(|e| e.to_str()) != Some("t") || !file.is_file() {
        return 0;
    }

    let stdlib = compiler::resolve_core_modules_dirs(Vec::new());
    if let Some(problem) = compiler::stdlib_problem(&stdlib) {
        eprintln!("toy hook: {problem}");
        return 2;
    }
    // Not in a package: `toy` has nothing to check it against.
    let Ok(pkg) = package::find(&file, stdlib.clone()) else { return 0 };
    let pkg = match module_of_package(&pkg, &file) {
        true => package::find(&pkg.root, stdlib).unwrap_or(pkg),
        false => pkg,
    };
    interpreter::set_diagnostic_root(Some(pkg.root.clone()));

    let diagnostics = match crate::fix::diagnose(&pkg) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("toy hook: {e}");
            return 2;
        }
    };
    let (errors, warnings): (Vec<Diagnostic>, Vec<Diagnostic>) =
        diagnostics.into_iter().partition(|d| d.severity == Severity::Error);

    if !errors.is_empty() {
        eprint!("{}", report(&pkg.root, &pkg.entry, &errors));
        return 2;
    }
    if !warnings.is_empty() {
        let context = interpreter::diagnostics_short(&warnings);
        let out = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": format!("toy check: warnings after this edit\n{context}"),
            }
        });
        println!("{out}");
    }
    0
}

/// The path the hook input names: the edited file.
fn edited_file(input: &str) -> Option<PathBuf> {
    let v: serde_json::Value = serde_json::from_str(input).ok()?;
    let path = v["tool_input"]["file_path"]
        .as_str()
        .or_else(|| v["tool_response"]["filePath"].as_str())?;
    Some(PathBuf::from(path))
}

/// Whether `file` is a module of `pkg` (under its `src/`, other than an
/// entry `src/main.t`).
fn module_of_package(pkg: &package::Package, file: &Path) -> bool {
    let src = pkg.root.join("src");
    let (Ok(src), Ok(file)) = (src.canonicalize(), file.canonicalize()) else {
        return false;
    };
    file.starts_with(&src) && file != src.join("main.t")
}

/// The errors, one per line, with how to see the rest and how to apply
/// the fixes.
fn report(root: &Path, entry: &Path, errors: &[Diagnostic]) -> String {
    let shown = &errors[..errors.len().min(MAX_ERRORS)];
    let mut out = format!("toy check: {} error(s) after this edit\n", errors.len());
    out.push_str(&interpreter::diagnostics_short(shown));
    let rel = |p: &Path| {
        p.canonicalize()
            .ok()
            .and_then(|c| root.canonicalize().ok().and_then(|r| c.strip_prefix(r).ok().map(Path::to_path_buf)))
            .map(|r| r.display().to_string())
            .unwrap_or_else(|| p.display().to_string())
    };
    if errors.len() > shown.len() {
        out.push_str(&format!(
            "... and {} more: `toy check {} --format=json`\n",
            errors.len() - shown.len(),
            rel(entry)
        ));
    }
    let fixable = errors
        .iter()
        .any(|d| d.suggestions.iter().any(|s| s.applicability == Applicability::MachineApplicable));
    if fixable {
        out.push_str(&format!(
            "`toy fix {}` applies the ones marked (fix: ...)\n",
            root.display()
        ));
    }
    out
}
