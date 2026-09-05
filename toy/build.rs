//! Bake the git revision in, so `toy version` can say which build it
//! is (BUILD_TOOL).
//!
//! A version number alone answers nothing during development: every
//! build of this repo is `0.1.0`, and the question a `version`
//! command is asked — "is the tool I am running the one I just
//! built?" — is a question about the commit.
//!
//! `cargo:rerun-if-changed` on `.git/HEAD` and the packed refs keeps
//! it current without rebuilding on every `cargo build`. If git is not
//! available (a source tarball, a vendored build), the revision is
//! `unknown` rather than a build failure: not knowing the commit is a
//! worse `version` output, not a reason to refuse to compile.

use std::process::Command;

fn main() {
    let revision = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    // A dirty tree is worth saying: the binary is not any commit.
    let dirty = git(&["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let describe = if dirty {
        format!("{revision}-dirty")
    } else {
        revision
    };
    println!("cargo:rustc-env=TOY_GIT_REVISION={describe}");

    // Rebuild when the checked-out commit changes. `.git/HEAD` covers
    // a checkout, the ref file covers a commit on the current branch,
    // and `packed-refs` covers the case where the ref is packed away.
    for path in [".git/HEAD", ".git/packed-refs"] {
        let candidate = std::path::Path::new("..").join(path);
        if candidate.exists() {
            println!("cargo:rerun-if-changed={}", candidate.display());
        }
    }
    if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        let candidate = std::path::Path::new("../.git").join(head_ref.trim());
        if candidate.exists() {
            println!("cargo:rerun-if-changed={}", candidate.display());
        }
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
