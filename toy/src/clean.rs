//! `toy clean` — remove what a build left behind.
//!
//! Only the outputs, by default: the profile directories
//! (`build/debug`, `build/release`) and the scratch and test binaries
//! inside them. The **link cache stays**, because throwing it away
//! turns the next build from 30 ms back into 90 ms and it holds
//! nothing that can go stale — it is keyed on the bytes of the object
//! it links (BUILD_TOOL.md D4). `--all` removes it too, for when the
//! question is disk space rather than a fresh build.
//!
//! Every path this deletes is checked against the package's `build/`
//! first (`Package::is_build_output`). The path is computed, so it
//! should already be right; a command that removes directories should
//! not rest on "should".

use std::path::PathBuf;

use crate::package::Package;

pub struct Options {
    /// Also remove the link cache and the `build/` directory itself.
    pub all: bool,
    pub verbose: bool,
}

pub fn run(pkg: &Package, opts: &Options) -> Result<(), String> {
    if !pkg.build_dir.is_dir() {
        println!("nothing to clean in {}", pkg.build_dir.display());
        return Ok(());
    }
    let targets: Vec<PathBuf> = if opts.all {
        vec![pkg.build_dir.clone()]
    } else {
        pkg.existing_profile_dirs()
    };
    if targets.is_empty() {
        println!("nothing to clean in {}", pkg.build_dir.display());
        return Ok(());
    }

    let mut removed = 0usize;
    let mut bytes = 0u64;
    for target in &targets {
        if !pkg.is_build_output(target) {
            return Err(format!(
                "refusing to remove `{}`: it is not inside `{}`",
                target.display(),
                pkg.build_dir.display()
            ));
        }
        bytes += dir_size(target);
        if opts.verbose {
            eprintln!("toy: rm -r {}", target.display());
        }
        std::fs::remove_dir_all(target)
            .map_err(|e| format!("cannot remove `{}`: {e}", target.display()))?;
        removed += 1;
    }
    println!(
        "removed {removed} director{} ({})",
        if removed == 1 { "y" } else { "ies" },
        human_bytes(bytes)
    );
    Ok(())
}

/// Bytes under `dir`, for the one line the command prints. A file it
/// cannot stat contributes nothing rather than failing the clean —
/// the number is a courtesy, not the point.
fn dir_size(dir: &std::path::Path) -> u64 {
    let Ok(read) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += dir_size(&path);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
