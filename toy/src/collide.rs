//! B4: say which bare names two module roots both define, before the
//! compiler has to (BUILD_TOOL.md D5).
//!
//! A bare call now resolves — the later root wins, so a package's own
//! `fn parse` shadows `std::json::parse` rather than colliding with
//! it. Shadowing is still worth saying out loud: it is usually
//! deliberate, and when it is not, the symptom is a stdlib function
//! quietly not being the one that ran.
//!
//! The roots are known before anything is parsed, so this can be said
//! at the point they are assembled — whereas the compiler reaches the
//! question only when a *call* is resolved, which may be in a branch
//! nobody ran today.
//!
//! The scan is deliberately shallow: a top-level `fn` / `pub fn` name
//! at the start of a line. It is a warning, so a false positive costs
//! a line of output; a parse would cost a second front end.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One name defined in more than one place.
pub struct Collision {
    pub name: String,
    pub files: Vec<String>,
}

/// Every bare function name that two of `roots` both define **and
/// that the package has a hand in**.
///
/// A duplicate that lives entirely inside the first root — the stdlib
/// — is filtered out. It is real, and unlike a package's shadow it is
/// **not** resolvable: `encode` is in both `std::base64` and
/// `std::hex` at the same rank, so a bare `encode(...)` is ambiguous
/// and the compiler says so at the call. It is not this package's to
/// fix, and a warning that appears on every command and cannot be
/// acted on is a warning people learn to scroll past. The stdlib's
/// own duplicates belong in the language's ledger, not in each
/// build.
///
/// Names are reported once, with every file that declares them, in
/// name order — a report that moves between runs is one nobody reads
/// twice.
pub fn scan(roots: &[PathBuf]) -> Vec<Collision> {
    let mut seen: HashMap<String, Vec<String>> = HashMap::new();
    for root in roots {
        let mut files = Vec::new();
        collect(root, &mut files);
        for file in files {
            let Ok(source) = std::fs::read_to_string(&file) else {
                continue;
            };
            let label = file.to_string_lossy().into_owned();
            // Only `pub` names can meet: a module's other functions are
            // its own (BARE-NAME-COLLISION), so two private `helper`s
            // never compete for a call.
            for name in top_level_pub_fn_names(&source) {
                let entry = seen.entry(name).or_default();
                if !entry.contains(&label) {
                    entry.push(label.clone());
                }
            }
        }
    }
    let first_root = roots.first().map(|r| r.to_string_lossy().into_owned());
    let mut out: Vec<Collision> = seen
        .into_iter()
        .filter(|(_, files)| files.len() > 1)
        .filter(|(_, files)| match &first_root {
            // Keep it only if something outside the stdlib declares it.
            Some(stdlib) => files.iter().any(|f| !f.starts_with(stdlib.as_str())),
            None => true,
        })
        .map(|(name, files)| Collision { name, files })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Render the warnings the way the design writes them: what collides,
/// where, and what to do about it.
pub fn render(collisions: &[Collision]) -> String {
    let mut out = String::new();
    for c in collisions {
        out.push_str(&format!(
            "warning: `{}` is defined in {}\n  a bare call from one of these files takes its own; from anywhere else, the last one -- qualify it to be explicit\n",
            c.name,
            c.files.join(" and ")
        ));
    }
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("t") {
            out.push(path);
        }
    }
}

/// Top-level `pub fn` names in `source`.
///
/// Only column zero: a `fn` indented inside an `impl` block is a
/// method, which lives in its type's namespace and does not take part
/// in the bare-name collision this looks for.
fn top_level_pub_fn_names(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        // No leading whitespace -- that is what makes it top level.
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = line
            .strip_prefix("pub fn ")
            .or_else(|| line.strip_prefix("pub unsafe fn "));
        let Some(rest) = rest else { continue };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::top_level_pub_fn_names;

    #[test]
    fn only_column_zero_pub_functions_count() {
        let src = "\
pub fn alpha(x: u64) -> u64 { x }
fn beta() -> u64 { 0u64 }
pub unsafe fn gamma() -> u64 { 0u64 }
impl Thing {
    fn delta(&self) -> u64 { 0u64 }
}
";
        let names = top_level_pub_fn_names(src);
        assert_eq!(names, vec!["alpha", "gamma"]);
    }
}
