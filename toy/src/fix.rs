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
        Err(errors) => {
            return Ok(errors
                .iter()
                .map(|e| {
                    let mut d = Diagnostic::from_parser_error(e, &name);
                    d.resolve_edits();
                    d
                })
                .collect());
        }
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
            if !dry_run {
                std::fs::write(file, text)
                    .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
            }
        }
        // The module files just changed under a process that read
        // them once and remembered the text.
        interpreter::module_integration::forget_discovered_modules();
        if dry_run {
            // Nothing was written, so a second round would see the
            // same program. Report what the first would do.
            outcome.remaining = diagnostics;
            return Ok(outcome);
        }
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
