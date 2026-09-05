//! Finding the package and building its module roots (BUILD_TOOL.md D2).
//!
//! There is no manifest. `toy` walks **up** from the path it was given
//! until it finds a directory holding a `main.t` or a `src/`, and that
//! is the package root. The design records why: a manifest would have
//! nothing to declare yet — the name is the directory, there is no
//! version because there are no dependencies, and the three build
//! flags live on the command line. A manifest arrives with
//! dependencies, not before.

use std::path::{Path, PathBuf};

/// A located package: where it is, what to compile, and the module
/// roots to compile it against.
#[derive(Debug, Clone)]
pub struct Package {
    /// The directory holding `main.t` and/or `src/`.
    pub root: PathBuf,
    /// The file to compile.
    pub entry: PathBuf,
    /// Module roots in search order. The stdlib comes first and the
    /// package's own `src/` after it, because a later root wins
    /// (BUILD-TOOL B0) — a package may shadow a stdlib module, not
    /// the other way round.
    pub module_roots: Vec<PathBuf>,
    /// Where build artefacts go. Not created until something is
    /// written.
    pub build_dir: PathBuf,
}

/// Which build a path belongs to. `--release` compiles contracts out,
/// so the two are different programs and must not share a name.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Debug,
    Release,
}

impl Profile {
    pub fn of(release: bool) -> Self {
        if release { Profile::Release } else { Profile::Debug }
    }

    fn dir_name(self) -> &'static str {
        match self {
            Profile::Debug => "debug",
            Profile::Release => "release",
        }
    }
}

impl Package {
    /// The name used for the default output binary: the root
    /// directory's own name.
    pub fn name(&self) -> String {
        self.root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("main")
            .to_string()
    }

    /// `build/.link/`, the link cache this package uses. BUILD-TOOL D4
    /// makes it the default rather than an environment variable only
    /// the initiated set: it is the difference between a 90 ms and a
    /// 30 ms rebuild, and there is no reason for that to be opt-in.
    ///
    /// Shared across profiles on purpose: the cache is
    /// content-addressed on the object bytes, so a debug and a release
    /// build of the same program simply do not collide.
    pub fn link_cache_dir(&self) -> PathBuf {
        self.build_dir.join(".link")
    }

    /// Where a build of `profile` puts things. `--release` compiles
    /// contracts out, so a release binary is a different program from
    /// a debug one; sharing a path would mean the file on disk does
    /// not say which it is, and the answer would change under you.
    pub fn profile_dir(&self, profile: Profile) -> PathBuf {
        self.build_dir.join(profile.dir_name())
    }

    /// The product binary: what `toy build` leaves behind for you to
    /// keep, copy or ship.
    pub fn exe_path(&self, profile: Profile) -> PathBuf {
        self.profile_dir(profile).join(self.name())
    }

    /// Where `toy run` builds. Separate from [`exe_path`] so running
    /// does not silently replace a binary you built and handed to
    /// someone: `run`'s output is scratch, `build`'s is a result.
    pub fn run_exe_path(&self, profile: Profile) -> PathBuf {
        self.profile_dir(profile).join(".run").join(self.name())
    }

    /// Test binaries, one per test file, kept away from the product
    /// binary so a directory listing of the build output is the thing
    /// you meant to build.
    pub fn test_exe_path(&self, profile: Profile, stem: &str) -> PathBuf {
        self.profile_dir(profile).join("tests").join(stem)
    }

    /// The profile directories that exist, in a stable order.
    pub fn existing_profile_dirs(&self) -> Vec<PathBuf> {
        [Profile::Debug, Profile::Release]
            .into_iter()
            .map(|p| self.profile_dir(p))
            .filter(|d| d.is_dir())
            .collect()
    }

    /// Refuse to delete anything that is not build output.
    ///
    /// `toy clean` removes directories, and the only thing standing
    /// between "removes the build output" and "removes the package" is
    /// that the path was computed correctly. Check it rather than
    /// trust it: the path must be inside this package's `build/`, or
    /// be that directory itself.
    pub fn is_build_output(&self, path: &Path) -> bool {
        let Ok(build) = self.build_dir.canonicalize() else {
            return false;
        };
        let Ok(target) = path.canonicalize() else {
            return false;
        };
        target == build || target.starts_with(&build)
    }

    /// Create `dir`, and on the first use of `build/` drop a
    /// `.gitignore` in it.
    ///
    /// Build output is not source, and every package would otherwise
    /// have to be told the same thing by hand. Written once and never
    /// overwritten, so a package that wants to keep something under
    /// `build/` can say so and be believed.
    pub fn ensure_dir(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create `{}`: {e}", dir.display()))?;
        let marker = self.build_dir.join(".gitignore");
        if self.build_dir.is_dir() && !marker.exists() {
            let _ = std::fs::write(&marker, "*\n");
        }
        Ok(())
    }
}

/// Locate the package containing `start`.
///
/// `start` may be a `.t` file (then that file is the entry) or a
/// directory (then `main.t` is, wherever the walk finds it). The walk
/// stops at the filesystem root; failing to find anything is an error
/// naming what was looked for, because the alternative — silently
/// treating the current directory as a package — produces a confusing
/// "no entry point" later.
pub fn find(start: &Path, stdlib: Vec<PathBuf>) -> Result<Package, String> {
    let start = if start.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        start.to_path_buf()
    };
    let abs = std::fs::canonicalize(&start)
        .map_err(|e| format!("cannot read `{}`: {e}", start.display()))?;

    // An explicit `.t` file names its own entry; the package is
    // whichever ancestor looks like one, so `toy run src/tool.t` works
    // from anywhere.
    let (explicit_entry, mut dir) = if abs.is_file() {
        (Some(abs.clone()), abs.parent().unwrap_or(&abs).to_path_buf())
    } else {
        (None, abs.clone())
    };

    let root = loop {
        if dir.join("main.t").is_file() || dir.join("src").is_dir() {
            break dir;
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => {
                return Err(format!(
                    "no package found at or above `{}`\n  \
                     a package is a directory holding `main.t` or `src/` \
                     (see design-docs/BUILD_TOOL.md)",
                    start.display()
                ));
            }
        }
    };

    let entry = match explicit_entry {
        Some(e) => e,
        None => {
            let candidate = root.join("main.t");
            if candidate.is_file() {
                candidate
            } else {
                let src_main = root.join("src").join("main.t");
                if src_main.is_file() {
                    src_main
                } else {
                    return Err(format!(
                        "package `{}` has no entry point\n  \
                         expected `main.t` or `src/main.t`",
                        root.display()
                    ));
                }
            }
        }
    };

    // The stdlib first, the package's `src/` after it. Order is the
    // whole point: B0 gives a later root the win, so a package can
    // shadow a stdlib module deliberately.
    let mut module_roots = stdlib;
    let src = root.join("src");
    if src.is_dir() {
        module_roots.push(src);
    }

    let build_dir = root.join("build");
    Ok(Package { root, entry, module_roots, build_dir })
}
