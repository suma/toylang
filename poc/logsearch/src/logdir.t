# Finding the files worth reading under a log directory.
#
# The directory tree is walked recursively: `/var/log`-shaped trees
# put the interesting files one level down (`apache2/access_ssl.log`),
# so stopping at the top would miss most of them.
#
# Two things are deliberately *not* here. There is no watching (the
# service re-scans on demand), and there is no decompression: a
# rotated `.gz` is skipped rather than half-read, because this
# language has no gzip and a binary file read as text would parse
# into garbage records (RUNTIME_GAPS.md G7).
#
# Discovery allocates -- `fs::list_dir` hands back a `Vec<String>`
# and every path is a fresh `String`. That is why it belongs to the
# start-up side of the program and is never called per record
# (MEMORY.md D4b).

import std.fs
import std.path

# How deep to recurse. `/var/log` is two levels in practice
# (`apache2/`, `nginx/`, ...); the bound keeps a symlink loop from
# turning into an unbounded walk.
pub const MAX_SCAN_DEPTH: u64 = 4u64

# Whether `name` looks like a log file this reader can parse.
#
# Accepted: `foo.log`, and the rotated `foo.log.1` that logrotate
# leaves behind uncompressed. Rejected: everything compressed, and
# anything without `.log` in it at all.
pub fn is_log_name(name: &String) -> bool {
    val gz = String::from_str(".gz")
    if name.ends_with(gz) { return false }
    val xz = String::from_str(".xz")
    if name.ends_with(xz) { return false }
    val zst = String::from_str(".zst")
    if name.ends_with(zst) { return false }
    val bz2 = String::from_str(".bz2")
    if name.ends_with(bz2) { return false }

    val dotlog = String::from_str(".log")
    if name.ends_with(dotlog) { return true }

    # `access_ssl.log.1`: `.log.` followed by digits only.
    val marker = String::from_str(".log.")
    val at = name.rfind(marker)
    match at {
        Option::Some(pos) => {
            var i = pos + 5u64
            val n = name.len()
            if i >= n { return false }
            while i < n {
                val c: u8 = name.get(i)
                if c < '0' || c > '9' { return false }
                i = i + 1u64
            }
            true
        }
        Option::None => { false }
    }
}

# Every file under `dir` that `keep` accepts, in a stable order.
#
# **Iterative, with an explicit work list.** The obvious shape is
# recursion, and for a while it did not compile: passing `&mut out`
# on to the recursive call was read as a conditional move. That is
# fixed (2026-09-05, verified with the call nested in both a loop and
# a branch), so the work list is now a choice rather than a
# workaround -- it is what keeps a symlink loop from becoming an
# unbounded walk, with one counter instead of a depth argument
# threaded through every level.
#
# The two `full.clone()` calls are not decoration. A `String` owns
# its bytes, so pushing one into a container moves it, and a move
# that only happens on some paths is `[E0014]` (there are no drop
# flags). Copying the path costs one allocation per directory entry,
# which a scan of a few thousand files does not notice.
fn collect(dir: str, suffix: String, use_suffix: bool, out: &mut Vec<String>) {
    var todo: Vec<String> = Vec::new()
    todo.push(String::from_str(dir))
    var visited: u64 = 0u64
    while todo.size() > 0u64 {
        val here: String = todo.pop()
        val here_str = here.to_str()
        visited = visited + 1u64
        if visited > 4096u64 {
            # A guard rather than a depth limit: the list cannot grow
            # for ever, whatever the tree does.
            todo.clear()
        } else {
            val listing = fs::list_dir(here_str)
            match listing {
                Result::Ok(names) => {
                    var i: u64 = 0u64
                    while i < names.size() {
                        val name: &String = names.borrow(i)
                        val name_str = name.to_str()
                        val full = path::join(here_str, name_str)
                        val full_str = full.to_str()
                        if fs::is_dir(full_str) {
                            todo.push(full.clone())
                        } else {
                            var take = false
                            if use_suffix {
                                if name.ends_with(suffix) { take = true }
                            } else {
                                if is_log_name(&name) { take = true }
                            }
                            if take { out.push(full.clone()) }
                        }
                        i = i + 1u64
                    }
                }
                Result::Err(e) => { }
            }
        }
    }
}

# Every file under `dir` whose name ends in `suffix`, in a stable
# order. Used for archives (`.dat`), where the naming rules are the
# writer's own rather than logrotate's.
pub fn scan_suffix(dir: str, suffix: str) -> Vec<String> {
    var out: Vec<String> = Vec::new()
    val sfx = String::from_str(suffix)
    collect(dir, sfx, true, &mut out)
    out.sort()
    out
}

# Every log file under `dir`, in a stable order.
#
# The order matters: `fs::list_dir` returns whatever the file system
# hands over, which differs between machines for the same directory,
# and a report that changes order between runs cannot be diffed.
pub fn scan(dir: str) -> Vec<String> {
    var out: Vec<String> = Vec::new()
    val unused = String::from_str("")
    collect(dir, unused, false, &mut out)
    out.sort()
    out
}
