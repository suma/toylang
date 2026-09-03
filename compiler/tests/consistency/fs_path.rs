//! STDLIB-FS-PATH: path syntax.
//!
//! `path.t` calls no syscall, so every answer here is a value rather
//! than something that depends on what is on disk -- which is what
//! makes this the bulk of the area's tests and why none of them need
//! a fixture.

use super::harness::*;

#[test]
fn the_path_table_holds() {
    // The design's table of edge cases, verbatim. These are the
    // answers that surprise people rather than the ones that
    // surprise a backend, so they are written down rather than
    // derived.
    let src = r#"
        fn main() -> u64 {
            val a1 = path::join("a", "b")
            println(a1)
            val a2 = path::join("a/", "b")
            println(a2)
            # An absolute right side wins outright.
            val a3 = path::join("a", "/b")
            println(a3)
            val a4 = path::join("", "b")
            println(a4)
            # No trailing separator is invented.
            val a5 = path::join("a", "")
            println(a5)

            val b1 = path::dirname("/a/b")
            println(b1)
            # POSIX dirname(1) answers `.` for a bare name.
            val b2 = path::dirname("a")
            println(b2)
            val b3 = path::dirname("/")
            println(b3)
            val b4 = path::dirname("a/b/")
            println(b4)

            val c1 = path::basename("/a/b")
            println(c1)
            val c2 = path::basename("/a/b/")
            println(c2)
            # The root is its own name.
            val c3 = path::basename("/")
            println(c3)

            # The last dot wins, a leading dot is not an extension, and
            # a trailing dot leaves nothing after it.
            val d1 = path::extension("a.tar.gz")
            println(d1)
            val d2 = path::extension(".bashrc")
            println(d2)
            val d3 = path::extension("a.")
            println(d3)
            val d4 = path::extension("a")
            println(d4)

            val e1 = path::stem("a.tar.gz")
            println(e1)
            val e2 = path::stem("a")
            println(e2)
            val e3 = path::stem(".bashrc")
            println(e3)

            println(path::is_absolute("/a"))
            println(path::is_absolute("a"))
            println(path::is_absolute(""))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "path_table");
}

#[test]
fn normalize_is_lexical() {
    // It collapses `.`, `..` and repeated separators **without
    // looking at the file system**, so a `..` through a symlink lands
    // somewhere the kernel would not. `fs::realpath` is the one that
    // asks -- and it fails on a path that does not exist, which is
    // why both have to exist.
    let src = r#"
        fn main() -> u64 {
            val a = path::normalize("a/b/../c")
            println(a)
            val b = path::normalize("./a")
            println(b)
            val c = path::normalize("a//b")
            println(c)
            val d = path::normalize("a/b/")
            println(d)
            # A `..` above the root has nowhere to go; above a relative
            # path it has to stay.
            val e = path::normalize("/a/../../b")
            println(e)
            val f = path::normalize("../a")
            println(f)
            val g = path::normalize("../../a")
            println(g)
            # Everything cancelling leaves the shortest legal answer.
            val h = path::normalize("a/..")
            println(h)
            val i = path::normalize("/a/..")
            println(i)
            val j = path::normalize("/")
            println(j)
            val k = path::normalize(".")
            println(k)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "path_normalize");
}

#[test]
fn with_extension_replaces_and_removes() {
    let src = r#"
        fn main() -> u64 {
            val a = path::with_extension("dir/a.txt", "md")
            println(a)
            val b = path::with_extension("a.txt", "md")
            println(b)
            val c = path::with_extension("a", "md")
            println(c)
            val d = path::with_extension("dir/a.tar.gz", "")
            println(d)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "path_with_extension");
}

#[test]
fn a_nul_byte_is_not_a_path() {
    // The C boundary would truncate there and act on a different
    // file, so this panics like an out-of-range index rather than
    // returning a failure. `is_valid` is the way to ask first.
    let src = r#"
        unsafe fn main() -> u64 {
            var bad = String::new()
            bad.push_str("a")
            bad.push(0u8)
            bad.push_str("b")
            val s: str = bad.to_str()
            println(path::is_valid(s))
            println(path::is_valid("a/b"))
            println(path::is_valid(""))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "path_nul_check");
}

// The half that calls the kernel. Everything here runs against a
// directory the test makes and removes, so it is the only part of
// this area that needs a fixture.

#[test]
fn a_directory_can_be_made_listed_and_removed() {
    // The listing's **order is unspecified** -- it is whatever the
    // file system hands over, and it differs between systems for the
    // same directory. Only the count is compared here for that
    // reason.
    //
    // `.` and `..` are never in it: returning them makes every
    // tree-walking program an infinite loop the first time it is
    // written, which is why the count below is 1 and not 3.
    let src = r#"
        unsafe fn main() -> u64 {
            val tmp: String = fs::temp_dir()
            val base: String = path::join(tmp.to_str(), "toylang_fs_consistency")
            val b: str = base.to_str()
            val f: String = path::join(b, "a.txt")
            # Leave nothing behind from an earlier run.
            val _c0 = fs::remove_file(f.to_str())
            val _c1 = fs::remove_dir(b)

            val made = fs::mkdir_all(b)
            match made {
                Result::Ok(_) => { println("made") }
                Result::Err(e) => { println(e) }
            }
            println(fs::is_dir(b))
            # A directory is not a file, though `io::file_exists` --
            # which is `access(F_OK)` -- says true for both.
            println(fs::is_file(b))
            println(io::file_exists(b))

            val w = io::write_file(f.to_str(), "hello")
            match w {
                Result::Ok(n) => { println(n) }
                Result::Err(e) => { println(e) }
            }
            val sz = fs::file_size(f.to_str())
            match sz {
                Result::Ok(n) => { println(n) }
                Result::Err(e) => { println(e) }
            }
            println(fs::is_file(f.to_str()))

            val listed = fs::list_dir(b)
            match listed {
                Result::Ok(v) => { println(v.size()) }
                Result::Err(e) => { println(e) }
            }

            # The three failures a file system adds to `IoError`.
            val again = fs::mkdir(b)
            match again {
                Result::Ok(_) => { println("unexpectedly re-made") }
                Result::Err(e) => { println(e) }
            }
            val busy = fs::remove_dir(b)
            match busy {
                Result::Ok(_) => { println("unexpectedly removed") }
                Result::Err(e) => { println(e) }
            }
            val missing: String = path::join(b, "nope")
            val gone = fs::list_dir(missing.to_str())
            match gone {
                Result::Ok(v) => { println(v.size()) }
                Result::Err(e) => { println(e) }
            }

            # `mkdir_all` succeeds on a directory that already exists.
            val twice = fs::mkdir_all(b)
            match twice {
                Result::Ok(_) => { println("again ok") }
                Result::Err(e) => { println(e) }
            }

            val _r0 = fs::remove_file(f.to_str())
            val _r1 = fs::remove_dir(b)
            println(fs::is_dir(b))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "fs_directory_lifecycle");
}

#[test]
fn copy_file_carries_bytes_that_are_not_text() {
    // It goes through `read_file_into` and a `Span<u8>` rather than
    // `read_file`, which refuses anything that is not UTF-8 -- `str`
    // holds text, and a file need not. The payload here would be
    // rejected by the str path.
    let src = r#"
        unsafe fn write_bytes(p: str, s: Span<u8>) -> u64 {
            val w = io::write_file_bytes(p, s)
            match w {
                Result::Ok(n) => n,
                Result::Err(_) => 999u64,
            }
        }

        unsafe fn main() -> u64 {
            val tmp: String = fs::temp_dir()
            val dir: String = path::join(tmp.to_str(), "toylang_fs_copy")
            val d: str = dir.to_str()
            val src_path: String = path::join(d, "src.bin")
            val dst_path: String = path::join(d, "dst.bin")
            val _c0 = fs::remove_file(src_path.to_str())
            val _c1 = fs::remove_file(dst_path.to_str())
            val _c2 = fs::remove_dir(d)
            val _m = fs::mkdir_all(d)

            var payload: Vec<u8> = Vec::new()
            payload.push(255u8)
            payload.push(0u8)
            payload.push(254u8)
            val span = payload.as_span()
            match span {
                Option::Some(s) => { println(write_bytes(src_path.to_str(), s)) }
                Option::None => { println("no span") }
            }

            val copied = fs::copy_file(src_path.to_str(), dst_path.to_str())
            match copied {
                Result::Ok(n) => { println(n) }
                Result::Err(e) => { println(e) }
            }
            val sz = fs::file_size(dst_path.to_str())
            match sz {
                Result::Ok(n) => { println(n) }
                Result::Err(e) => { println(e) }
            }

            val _r0 = fs::remove_file(src_path.to_str())
            val _r1 = fs::remove_file(dst_path.to_str())
            val _r2 = fs::remove_dir(d)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "fs_copy_binary");
}

#[test]
fn realpath_resolves_and_normalize_does_not() {
    // The two exist separately because they answer different
    // questions: `normalize` cleans up a path someone typed, and
    // fails at nothing; `realpath` asks the kernel, and fails when
    // the path is not there.
    let src = r#"
        unsafe fn main() -> u64 {
            val cleaned = path::normalize("/tmp/./a/../b")
            println(cleaned)
            val nope = fs::realpath("/no/such/path/anywhere")
            match nope {
                Result::Ok(p) => { println(p) }
                Result::Err(e) => { println(e) }
            }
            val here = fs::current_dir()
            match here {
                Result::Ok(p) => { println(path::is_absolute(p.to_str())) }
                Result::Err(e) => { println(e) }
            }
            0u64
        }
    "#;
    assert_stdout_consistent(src, "fs_realpath_vs_normalize");
}
