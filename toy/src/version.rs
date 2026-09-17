//! `toy version` — which build of what, and where it is.
//!
//! The question this answers during development is not "which
//! release" — every build of this repo is `0.1.0` — but **"is the
//! thing I am running the thing I just built, and against which
//! stdlib?"**. Three parts can disagree: the tool, the standalone
//! `compiler` / `interpreter` binaries beside it, and the stdlib
//! directory, which is data on disk and can come from an entirely
//! different checkout.
//!
//! So each row carries a path, and a path that is not there is called
//! out on its own line rather than left for the reader to notice. A
//! missing `compiler` beside `toy` is not fatal — `toy` holds it as a
//! crate — but it means `compiler ...` typed by hand will not work,
//! and that is worth one line.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

/// ANSI, and only when someone is looking. Piped into a file or a
/// grep, the escapes are noise that also breaks a `==` against
/// expected output — which is what the tests here do.
struct Style {
    on: bool,
}

impl Style {
    fn new() -> Self {
        // `NO_COLOR` is the convention (no-color.org); an explicit
        // `TOY_COLOR=1` forces it on for a caller that is capturing
        // output on purpose.
        let forced = std::env::var("TOY_COLOR").is_ok_and(|v| v != "0");
        let suppressed = std::env::var("NO_COLOR").is_ok();
        Style {
            on: forced || (!suppressed && std::io::stdout().is_terminal()),
        }
    }

    fn warn(&self, text: &str) -> String {
        if self.on {
            format!("\x1b[33m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn dim(&self, text: &str) -> String {
        if self.on {
            format!("\x1b[2m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

/// One thing `toy` depends on.
struct Row {
    name: &'static str,
    version: String,
    revision: String,
    path: Option<PathBuf>,
    /// Why the path matters when it is missing. `None` for rows whose
    /// absence is not a problem.
    missing_note: &'static str,
}

pub fn run(verbose: bool, json: bool) -> Result<(), String> {
    let style = Style::new();
    let exe = std::env::current_exe().ok();
    let bin_dir = exe.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf);
    let stdlib = compiler::resolve_core_modules_dirs(Vec::new());

    let rows = vec![
        Row {
            name: "toy",
            version: env!("CARGO_PKG_VERSION").to_string(),
            revision: env!("TOY_GIT_REVISION").to_string(),
            path: exe.clone(),
            missing_note: "",
        },
        Row {
            name: "compiler",
            version: compiler::version().to_string(),
            revision: env!("TOY_GIT_REVISION").to_string(),
            path: bin_dir.as_ref().map(|d| d.join(bin_name("compiler"))),
            missing_note: "the standalone CLI is not built here (cargo build -p compiler)",
        },
        Row {
            name: "interpreter",
            version: interpreter::version().to_string(),
            revision: env!("TOY_GIT_REVISION").to_string(),
            path: bin_dir.as_ref().map(|d| d.join(bin_name("interpreter"))),
            missing_note: "the standalone CLI is not built here (cargo build -p interpreter)",
        },
        Row {
            name: "stdlib",
            version: "-".to_string(),
            // Resolved from the directory, not baked in: the stdlib is
            // data, and it can come from a different checkout than the
            // binary reading it. That mismatch is exactly what this
            // command exists to make visible.
            revision: stdlib
                .first()
                .map(|d| git_revision(d))
                .unwrap_or_else(|| "unknown".to_string()),
            path: stdlib.first().cloned(),
            missing_note: "no stdlib here; set TOYLANG_CORE_MODULES or pass --core-modules",
        },
    ];

    if json {
        // The same rows; a missing path is `exists: false` plus the
        // note, rather than a coloured warning.
        let entries: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                let exists = row.path.as_ref().is_some_and(|p| p.exists());
                serde_json::json!({
                    "name": row.name,
                    "version": row.version,
                    "revision": row.revision,
                    "path": row.path.as_ref().map(|p| display_path(p)),
                    "exists": exists,
                    "note": if exists || row.missing_note.is_empty() { None } else { Some(row.missing_note) },
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Array(entries)).unwrap_or_default()
        );
        return Ok(());
    }

    let name_w = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let ver_w = rows.iter().map(|r| r.version.len()).max().unwrap_or(0);
    let rev_w = rows.iter().map(|r| r.revision.len()).max().unwrap_or(0);

    for row in &rows {
        let path_text = match &row.path {
            // The exe-relative search produces `.../target/debug/../../core`,
            // which is the right directory and an unreadable way to say
            // so. Canonicalise when the path exists; leave it as
            // computed when it does not, because that is the path the
            // reader has to go and create.
            Some(p) => display_path(p),
            None => "<unknown>".to_string(),
        };
        let exists = row.path.as_ref().is_some_and(|p| p.exists());
        let suffix = if exists {
            String::new()
        } else {
            let note = if row.missing_note.is_empty() {
                "not found".to_string()
            } else {
                format!("not found -- {}", row.missing_note)
            };
            format!("  {}", style.warn(&format!("! {note}")))
        };
        println!(
            "{:name_w$}  {:ver_w$}  {:rev_w$}  {path_text}{suffix}",
            row.name,
            row.version,
            row.revision,
            name_w = name_w,
            ver_w = ver_w,
            rev_w = rev_w,
        );
    }

    if verbose {
        println!();
        println!(
            "{}",
            style.dim(
                "`compiler` and `interpreter` are linked into `toy` as crates, so the\n\
                 revision shown for them is the code `toy` runs. The path is where the\n\
                 standalone CLI would be, which a stale build can leave behind."
            )
        );
    }
    Ok(())
}

/// Canonical when the path exists; as computed when it does not,
/// because that is the path the reader has to go and create.
fn display_path(p: &Path) -> String {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf()).display().to_string()
}

/// The revision of the checkout `dir` is in, or `unknown`.
///
/// Asked at run time on purpose. The stdlib is a directory, and the
/// one being used may not be from the tree this binary was built in —
/// which is the failure this command is for.
fn git_revision(dir: &Path) -> String {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(dir).args(["rev-parse", "--short", "HEAD"]);
    let Ok(out) = cmd.output() else {
        return "unknown".to_string();
    };
    if !out.status.success() {
        return "unknown".to_string();
    }
    let rev = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let dirty = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain", "--", "."])
        .output()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false);
    if dirty { format!("{rev}-dirty") } else { rev }
}

fn bin_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}
