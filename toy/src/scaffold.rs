//! `toy new` / `toy init` — lay down a package that already builds.
//!
//! There is no manifest (BUILD_TOOL.md D2), so the convention is the
//! whole interface: a `main.t` is the entry, `src/*.t` are its modules
//! (`src/greet.t` is `greet::`), `tests/*.t` are programs of `test`
//! blocks. Nothing in a directory listing says that, so the first
//! package a person makes is where they learn it -- and the files this
//! writes are that lesson, each one exercising the part of the
//! convention it is named for. `toy run`, `toy test` and `toy check`
//! all pass on the result as written.
//!
//! **It never overwrites.** `new` wants a directory that does not
//! exist yet; `init` fills an existing one but refuses a directory
//! that is already a package, and neither writes over a file that is
//! already there.

use std::path::{Path, PathBuf};

pub struct Options {
    /// `init`: the directory may exist (it is filled in); `new`: it
    /// must not.
    pub in_place: bool,
    pub verbose: bool,
    pub json: bool,
}

const MAIN: &str = r#"# The entry: `toy run` compiles this file, `toy build` makes it an
# executable under build/. The modules in src/ are visible by name --
# `src/greet.t` is `greet::`.
fn main() -> u64 {
    val line: String = greet::greeting("world")
    println(line)
    val n: u64 = greet::double(21u64)
    println("double(21) = {n}")
    0u64
}
"#;

const MODULE: &str = r#"# A module. The file name is the module name, so everything here is
# reached as `greet::...` -- from main.t, from tests/, and from the
# `test` block below.

pub fn greeting(name: str) -> String {
    var out: String = String::from_str("hello, ")
    out.push_str(name)
    out
}

# `requires` / `ensures` are checked at run time (and compiled out by
# `--release`); `toy test --check` also uses them to generate inputs.
pub fn double(n: u64) -> u64
    requires n < 9223372036854775808u64
    ensures result == n + n
{
    n * 2u64
}

# A test can live next to the code it tests; `toy test` finds it.
test "double adds a number to itself" {
    assert_eq(greet::double(4u64), 8u64)
}
"#;

const TEST: &str = r#"# Each tests/*.t is a program of `test` blocks -- no `main` needed.
# It sees the package's modules just as main.t does.

test "the greeting names who it greets" {
    val g: String = greet::greeting("toylang")
    assert(g.len() == 14u64, "unexpected greeting length")
}
"#;

pub fn run(dir: &Path, opts: &Options) -> Result<(), String> {
    if opts.in_place {
        if dir.exists() && !dir.is_dir() {
            return Err(format!("`{}` exists and is not a directory", dir.display()));
        }
        if dir.join("main.t").exists() || dir.join("src").exists() {
            return Err(format!(
                "`{}` is already a package (it has a main.t or a src/); nothing written",
                dir.display()
            ));
        }
    } else if dir.exists() {
        return Err(format!(
            "`{}` already exists; `toy init {}` fills in an existing directory",
            dir.display(),
            dir.display()
        ));
    }

    let files: [(&str, &str); 3] =
        [("main.t", MAIN), ("src/greet.t", MODULE), ("tests/basic.t", TEST)];
    // Checked before anything is written, so a refusal leaves the
    // directory exactly as it was.
    for (rel, _) in &files {
        let path = dir.join(rel);
        if path.exists() {
            return Err(format!("`{}` already exists; nothing written", path.display()));
        }
    }

    let mut created: Vec<PathBuf> = Vec::new();
    for (rel, text) in &files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create `{}`: {e}", parent.display()))?;
        }
        std::fs::write(&path, text)
            .map_err(|e| format!("cannot write `{}`: {e}", path.display()))?;
        if opts.verbose {
            eprintln!("toy: wrote {}", path.display());
        }
        created.push(path);
    }

    if opts.json {
        let created: Vec<String> = created.iter().map(|p| p.display().to_string()).collect();
        let doc = serde_json::json!({ "root": dir.display().to_string(), "created": created });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return Ok(());
    }
    println!("created package `{}`", package_name(dir));
    for path in &created {
        println!("  {}", path.display());
    }
    println!();
    println!("next:");
    println!("  toy run {}", dir.display());
    println!("  toy test {}", dir.display());
    Ok(())
}

/// The name `toy build` will give the executable: the directory's own
/// name, as `Package::name` has it.
fn package_name(dir: &Path) -> String {
    let resolved = if dir.as_os_str().is_empty() || dir == Path::new(".") {
        std::env::current_dir().unwrap_or_else(|_| dir.to_path_buf())
    } else {
        dir.to_path_buf()
    };
    resolved
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("main")
        .to_string()
}
