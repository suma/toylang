# Path syntax (STDLIB-FS-PATH F0).
#
# **Nothing here calls a syscall.** Every function is a string
# operation, so the answers do not depend on what exists on disk --
# which is what lets the tests below be values rather than fixtures,
# and what keeps `normalize` from quietly following a symlink.
#
# **There is no `Path` type**, for the reason there is no `Duration`:
# a new type brings `Display`, `eq`, `Ord`, `Hash`, conversions both
# ways and `Vec<Path>` with it, and buys only "you cannot mix a path
# up with a string" -- while `io::read_file` takes a `str`, so every
# call would convert at the boundary anyway.

# Whether this string can name a file at all.
#
# A NUL byte cannot: the C boundary would silently truncate the path
# there and operate on a different file. The functions below panic on
# one rather than returning a failure, the way an out-of-range index
# does -- this is the way to ask first.
pub unsafe fn is_valid(p: str) -> bool {
    val b: String = String::from_str(p)
    var i: u64 = 0u64
    while i < b.size() {
        if b.get(i) == 0u8 { return false }
        i = i + 1u64
    }
    true
}

unsafe fn require_valid(p: str, who: str) {
    if is_valid(p) == false {
        panic("path::{who}: a path cannot contain a NUL byte")
    }
}

pub fn is_absolute(p: str) -> bool { p.starts_with("/") }

# Join two path pieces with exactly one separator.
#
#     join("a", "b")   == "a/b"
#     join("a/", "b")  == "a/b"      # separators are not doubled
#     join("a", "/b")  == "/b"       # an absolute right side wins
#     join("", "b")    == "b"
#     join("a", "")    == "a"        # no trailing separator is added
pub unsafe fn join(a: str, b: str) -> String {
    require_valid(a, "join")
    require_valid(b, "join")
    if b.len() == 0u64 {
        val left: String = String::from_str(a)
        return left
    }
    if is_absolute(b) {
        val right: String = String::from_str(b)
        return right
    }
    if a.len() == 0u64 {
        val only: String = String::from_str(b)
        return only
    }
    var out: String = String::from_str(a)
    if out.get(out.size() - 1u64) != '/' { out.push_str("/") }
    out.push_str(b)
    out
}

# Everything before the last separator.
#
#     dirname("/a/b")  == "/a"
#     dirname("a")     == "."        # as POSIX dirname(1)
#     dirname("/")     == "/"
#     dirname("a/b/")  == "a"        # a trailing separator is dropped first
pub unsafe fn dirname(p: str) -> String {
    require_valid(p, "dirname")
    val b: String = trim_trailing_slashes(p)
    val n: u64 = b.size()
    if n == 0u64 { val lit: String = String::from_str("/")
        return lit }
    var i: u64 = n
    while i > 0u64 {
        if b.get(i - 1u64) == '/' {
            # Keep the root's own separator; drop any other.
            if i == 1u64 { val lit: String = String::from_str("/")
        return lit }
            val head: String = b.substring(0u64, i - 1u64)
            return head
        }
        i = i - 1u64
    }
    val here: String = String::from_str(".")
    here
}

# Everything after the last separator.
#
#     basename("/a/b")  == "b"
#     basename("/a/b/") == "b"
#     basename("/")     == "/"
pub unsafe fn basename(p: str) -> String {
    require_valid(p, "basename")
    val b: String = trim_trailing_slashes(p)
    val n: u64 = b.size()
    if n == 0u64 { val lit: String = String::from_str("/")
        return lit }
    # The root is its own name: trimming leaves it as `/`, and the
    # loop below would answer with the empty string after it.
    if b.eq_str("/") { return b }
    var i: u64 = n
    while i > 0u64 {
        if b.get(i - 1u64) == '/' {
            val tail: String = b.substring(i, n)
            return tail
        }
        i = i - 1u64
    }
    b
}

# The characters after the **last** dot in the file name.
#
#     extension("a.tar.gz") == "gz"
#     extension(".bashrc")  == ""    # a leading dot is not an extension
#     extension("a.")       == ""
#     extension("a")        == ""
pub unsafe fn extension(p: str) -> String {
    val name: String = basename(p)
    val n: u64 = name.size()
    var i: u64 = n
    while i > 0u64 {
        if name.get(i - 1u64) == '.' {
            # Position 1 means the dot is the first character.
            if i == 1u64 {
                val empty: String = String::new()
                return empty
            }
            if i == n {
                val none: String = String::new()
                return none
            }
            val ext: String = name.substring(i, n)
            return ext
        }
        i = i - 1u64
    }
    val nothing: String = String::new()
    nothing
}

# The file name without its extension -- `extension`'s complement.
#
#     stem("a.tar.gz") == "a.tar"
pub unsafe fn stem(p: str) -> String {
    val name: String = basename(p)
    val ext: String = extension(p)
    if ext.size() == 0u64 { return name }
    # Drop the extension and the dot before it.
    val out: String = name.substring(0u64, name.size() - ext.size() - 1u64)
    out
}

# The path with a different extension. An empty `ext` removes it.
pub unsafe fn with_extension(p: str, ext: str) -> String {
    val dir: String = dirname(p)
    val base: String = stem(p)
    var name: String = base
    if ext.len() > 0u64 {
        name.push_str(".")
        name.push_str(ext)
    }
    val d: str = dir.to_str()
    if d == "." && p.starts_with("./") == false {
        return name
    }
    val out: String = join(d, name.to_str())
    out
}

# Collapse `.`, `..` and repeated separators, **without looking at the
# file system**.
#
#     normalize("a/b/../c") == "a/c"
#     normalize("./a")      == "a"
#     normalize("a//b")     == "a/b"
#
# **This does not follow symbolic links.** If `b` is a link, the real
# `a/b/../c` is not `a/c`. When that matters, `fs::realpath` asks the
# kernel -- and fails if the path does not exist, which is why both
# exist.
pub unsafe fn normalize(p: str) -> String {
    require_valid(p, "normalize")
    val absolute: bool = is_absolute(p)
    val b: String = String::from_str(p)
    val slash: String = String::from_str("/")
    val parts: Vec<String> = b.split(slash)
    var out: Vec<String> = Vec::new()
    var i: u64 = 0u64
    while i < parts.size() {
        val part: String = parts.get(i)
        val s: str = part.to_str()
        if s == "" || s == "." {
            i = i + 1u64
            continue
        }
        if s == ".." {
            if out.size() > 0u64 {
                val last: String = out.get(out.size() - 1u64)
                if last.eq_str("..") == false {
                    val _dropped: String = out.pop()
                    i = i + 1u64
                    continue
                }
            }
            # A `..` above the root has nowhere to go; above a relative
            # path it has to stay.
            if absolute {
                i = i + 1u64
                continue
            }
        }
        out.push(part)
        i = i + 1u64
    }
    var result: String = String::new()
    if absolute { result.push_str("/") }
    var k: u64 = 0u64
    while k < out.size() {
        if k > 0u64 { result.push_str("/") }
        val piece: String = out.get(k)
        result.push_string(piece)
        k = k + 1u64
    }
    if result.size() == 0u64 {
        if absolute { val lit: String = String::from_str("/")
        return lit }
        val lit: String = String::from_str(".")
        return lit
    }
    result
}

# The path with any trailing separators removed. The root keeps its
# one separator, which is why `basename("/")` is `/`.
unsafe fn trim_trailing_slashes(p: str) -> String {
    val b: String = String::from_str(p)
    var n: u64 = b.size()
    while n > 1u64 && b.get(n - 1u64) == '/' {
        n = n - 1u64
    }
    if n == 1u64 && b.get(0u64) == '/' { val lit: String = String::from_str("/")
        return lit }
    val out: String = b.substring(0u64, n)
    out
}
