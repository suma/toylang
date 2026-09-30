//! `toy fix` (LLM-TOOLING #1): apply the compiler's machine-applicable
//! suggestions and check again, until none are left.
//!
//! A suggestion is offered only when applying it resolves its
//! diagnostic (`docs` of `frontend::diagnostic::Applicability`), and
//! `interpreter/tests/diagnostics_json_tests.rs` holds every kind to
//! that by applying it and re-checking. What this adds is the loop: a
//! fix can uncover a mistake the first one hid — a parse error stops
//! type checking — so one round is not always enough.
//!
//! Edits land only in the package's own files. A suggestion whose
//! edits reach elsewhere (the stdlib) is reported and left alone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use frontend::diagnostic::{Applicability, Diagnostic};

use crate::package::Package;

/// More rounds than any fix chain needs; a bound so that a suggestion
/// which does not resolve its diagnostic cannot loop forever.
const MAX_ROUNDS: usize = 8;

/// One edit that was (or, with `--dry-run`, would be) made.
pub struct Applied {
    pub file: PathBuf,
    pub line: u32,
    pub column: u32,
    pub written: String,
    pub replacement: String,
    pub code: &'static str,
    pub message: String,
}

pub struct Outcome {
    pub applied: Vec<Applied>,
    pub rounds: usize,
    /// Diagnostics still standing after the last round.
    pub remaining: Vec<Diagnostic>,
    /// Suggestions not applied because they edit outside the package.
    pub outside: Vec<String>,
}

/// Check the package, the way `toy check --backend vm` does: parse
/// errors if there are any, otherwise type errors and warnings.
pub fn diagnose(pkg: &Package) -> Result<Vec<Diagnostic>, String> {
    let source = std::fs::read_to_string(&pkg.entry)
        .map_err(|e| format!("cannot read {}: {e}", pkg.entry.display()))?;
    let name = pkg.entry.to_string_lossy().into_owned();
    let mut session = compiler_core::CompilerSession::new();
    let mut program = match session.parse_program_all_errors(&source, &name) {
        Ok(program) => program,
        // Parse errors and the type errors behind them: one round can
        // then fix both.
        Err(_) => return Ok(interpreter::diagnose_parse_failure(&source, &name, &pkg.module_roots)),
    };
    let result = interpreter::check_typing_diagnostics(
        &mut program,
        session.string_interner_mut(),
        Some(&source),
        Some(&name),
        &pkg.module_roots,
    );
    Ok(match result {
        Ok(warnings) => warnings,
        Err(errors) => errors,
    })
}

pub fn run(pkg: &Package, dry_run: bool) -> Result<Outcome, String> {
    if dry_run {
        return run_on_a_copy(pkg);
    }
    run_in_place(pkg)
}

/// `--dry-run`: every round, on a copy of the package, reported against
/// the real one. A dry run that wrote nothing could only show the first
/// round — the second is what the checker says once the first is in.
fn run_on_a_copy(pkg: &Package) -> Result<Outcome, String> {
    let root = pkg.root.canonicalize().unwrap_or_else(|_| pkg.root.clone());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let scratch = std::env::temp_dir().join(format!("toy-fix-dry-run-{}-{nanos}", std::process::id()));
    copy_package(&root, &scratch)
        .map_err(|e| format!("cannot copy the package for a dry run: {e}"))?;
    let scratch = scratch.canonicalize().unwrap_or(scratch);
    let moved = |p: &Path| match p.canonicalize().ok().as_deref().and_then(|c| c.strip_prefix(&root).ok()) {
        Some(rel) => scratch.join(rel),
        None => p.to_path_buf(),
    };
    let copy = Package {
        root: scratch.clone(),
        entry: moved(&pkg.entry),
        module_roots: pkg.module_roots.iter().map(|r| moved(r)).collect(),
        build_dir: pkg.build_dir.clone(),
    };
    let result = run_in_place(&copy);
    interpreter::module_integration::forget_discovered_modules();
    let _ = std::fs::remove_dir_all(&scratch);
    let mut outcome = result?;
    // Name the package's own files, not the copy's.
    let back = |p: &Path| match p.strip_prefix(&scratch) {
        Ok(rel) => root.join(rel),
        Err(_) => p.to_path_buf(),
    };
    for a in &mut outcome.applied {
        a.file = back(&a.file);
    }
    let scratch_str = scratch.to_string_lossy().into_owned();
    let root_str = root.to_string_lossy().into_owned();
    let rename = |f: &mut String| {
        if let Some(rest) = f.strip_prefix(&scratch_str) {
            *f = format!("{root_str}{rest}");
        }
    };
    for d in &mut outcome.remaining {
        rename(&mut d.file);
        for e in d.suggestions.iter_mut().flat_map(|s| s.edits.iter_mut()) {
            if let Some(f) = e.file.as_mut() {
                rename(f);
            }
        }
        for r in &mut d.related {
            if let Some(f) = r.file.as_mut() {
                rename(f);
            }
        }
    }
    Ok(outcome)
}

/// The package's sources, without what building left behind.
fn copy_package(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "build" || name == ".toycache" {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            copy_package(&path, &to.join(&name))?;
        } else {
            std::fs::copy(&path, to.join(&name))?;
        }
    }
    Ok(())
}

fn run_in_place(pkg: &Package) -> Result<Outcome, String> {
    let root = pkg.root.canonicalize().unwrap_or_else(|_| pkg.root.clone());
    let mut outcome = Outcome { applied: Vec::new(), rounds: 0, remaining: Vec::new(), outside: Vec::new() };
    for _ in 0..MAX_ROUNDS {
        let diagnostics = diagnose(pkg)?;
        let round = plan(pkg, &root, &diagnostics, &mut outcome.outside);
        if round.is_empty() {
            outcome.remaining = diagnostics;
            return Ok(outcome);
        }
        outcome.rounds += 1;
        for (file, edits) in &round {
            let mut text = std::fs::read_to_string(file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            // Back to front, so earlier offsets stay valid.
            for edit in edits.iter().rev() {
                outcome.applied.push(Applied {
                    file: file.clone(),
                    line: edit.line,
                    column: edit.column,
                    written: text.get(edit.start..edit.end).unwrap_or("").to_string(),
                    replacement: edit.replacement.clone(),
                    code: edit.code,
                    message: edit.message.clone(),
                });
                text.replace_range(edit.start..edit.end, &edit.replacement);
            }
            std::fs::write(file, text)
                .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        }
        // The module files just changed under a process that read
        // them once and remembered the text.
        interpreter::module_integration::forget_discovered_modules();
    }
    outcome.remaining = diagnose(pkg)?;
    Ok(outcome)
}

impl Outcome {
    /// In source order, the way a reader goes through a file (they
    /// are applied back to front).
    pub fn sort(&mut self) {
        self.applied.sort_by(|a, b| (&a.file, a.line, a.column).cmp(&(&b.file, b.line, b.column)));
    }
}

struct PlannedEdit {
    start: usize,
    end: usize,
    line: u32,
    column: u32,
    replacement: String,
    code: &'static str,
    message: String,
}

/// This round's edits, per file, sorted and non-overlapping. A
/// suggestion is taken whole or not at all: one whose edits collide
/// with an earlier suggestion's waits for the next round, when the
/// diagnostic is re-reported against the edited text.
fn plan(
    pkg: &Package,
    root: &Path,
    diagnostics: &[Diagnostic],
    outside: &mut Vec<String>,
) -> BTreeMap<PathBuf, Vec<PlannedEdit>> {
    let mut by_file: BTreeMap<PathBuf, Vec<PlannedEdit>> = BTreeMap::new();
    for d in diagnostics {
        for s in &d.suggestions {
            if s.applicability != Applicability::MachineApplicable {
                continue;
            }
            let mut edits = Vec::new();
            let mut in_package = true;
            for e in &s.edits {
                let (Some(file), Some(span)) = (&e.file, e.span) else {
                    in_package = false;
                    break;
                };
                let path = resolve(pkg, file);
                if !path.canonicalize().is_ok_and(|p| p.starts_with(root)) {
                    in_package = false;
                    outside.push(format!("{file}:{}: {}", span.line, s.message));
                    break;
                }
                edits.push((
                    path,
                    PlannedEdit {
                        start: span.offset as usize,
                        end: span.end_offset as usize,
                        line: span.line,
                        column: span.column,
                        replacement: e.replacement.clone(),
                        code: d.code,
                        message: s.message.clone(),
                    },
                ));
            }
            if !in_package {
                continue;
            }
            let collides = edits.iter().any(|(path, edit)| {
                by_file.get(path).is_some_and(|taken| {
                    taken.iter().any(|t| edit.start < t.end.max(t.start + 1) && t.start < edit.end.max(edit.start + 1))
                })
            });
            if collides {
                continue;
            }
            for (path, edit) in edits {
                by_file.entry(path).or_default().push(edit);
            }
        }
    }
    for edits in by_file.values_mut() {
        edits.sort_by_key(|e| (e.start, e.end));
    }
    by_file
}

/// A diagnostic names its file as the compiler was told it (the entry
/// as given) or as the module root displays it (relative to the
/// package).
fn resolve(pkg: &Package, file: &str) -> PathBuf {
    let path = Path::new(file);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let in_root = pkg.root.join(path);
    if in_root.exists() { in_root } else { path.to_path_buf() }
}
