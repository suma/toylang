# The file system (STDLIB-FS-PATH).
#
# `path.t` is the syntax and touches nothing; this is the half that
# calls the kernel. The split is deliberate: it keeps `normalize`
# from following a symlink, and it keeps most of the area's tests
# free of fixtures.
#
# Every extern here **forwards to `toylang_rt`** rather than being
# implemented twice. That is the shape `net.t` uses, and the reason is
# on the record: `io.t` did carry two implementations, and they drifted
# until the same failure reported two different names (ERROR_MODEL E0).
#
# **What is missing, and why.** There is no `mtime` and no `mode`.
# Both live in `struct stat`, whose layout differs between macOS and
# Linux, and a mis-transcribed field offset compiles cleanly and
# returns a wrong number -- the failure NETWORK_IO's per-platform
# module and its header cross-check exist to prevent. Size comes from
# seeking to the end of the file and kind from whether the path opens
# as a directory, neither of which needs a layout. When `mtime` is
# actually wanted, it should arrive with the same care the socket
# constants got.

extern fn __extern_fs_dir_open(path: str) -> u64 from "toylang_rt" as "toy_fs_dir_open"
extern fn __extern_fs_dir_name(i: u64) -> str from "toylang_rt" as "toy_fs_dir_name"
extern fn __extern_fs_status() -> u64 from "toylang_rt" as "toy_fs_status"
extern fn __extern_fs_is_dir(path: str) -> bool from "toylang_rt" as "toy_fs_is_dir"
extern fn __extern_fs_file_size(path: str) -> u64 from "toylang_rt" as "toy_fs_file_size"
extern fn __extern_fs_mkdir(path: str) -> u64 from "toylang_rt" as "toy_fs_mkdir"
extern fn __extern_fs_remove_file(path: str) -> u64 from "toylang_rt" as "toy_fs_remove_file"
extern fn __extern_fs_remove_dir(path: str) -> u64 from "toylang_rt" as "toy_fs_remove_dir"
extern fn __extern_fs_rename(from_path: str, to_path: str) -> u64 from "toylang_rt" as "toy_fs_rename"
extern fn __extern_fs_realpath(path: str) -> str from "toylang_rt" as "toy_fs_realpath"
extern fn __extern_fs_current_dir() -> str from "toylang_rt" as "toy_fs_current_dir"

# The status vocabulary `io.t` decodes, plus the three a file system
# adds. Decoded here rather than in `io.t` because these three only
# arise from file-system calls, and `io_error_from_status` is a
# `pub`-less helper there.
fn fs_error_from_status(status: u64) -> IoError {
    if status == 7u64 { IoError::AlreadyExists }
    elif status == 8u64 { IoError::NotADirectory }
    elif status == 9u64 { IoError::NotEmpty }
    elif status == 1u64 { IoError::NotFound }
    elif status == 2u64 { IoError::PermissionDenied }
    elif status == 3u64 { IoError::IsADirectory }
    elif status == 4u64 { IoError::ReadError }
    elif status == 5u64 { IoError::WriteError }
    else { IoError::Unknown }
}

# ---------------------------------------------------------------------
# Listing (F1).

# What a directory entry is. Only the two kinds a caller acts on
# differently -- recurse, or read.
pub enum FileKind {
    File,
    Dir,
}

impl Display for FileKind {
    fn to_str(&self) -> str {
        match self {
            FileKind::File => "file",
            FileKind::Dir => "dir",
        }
    }
}

# The whole listing, copied.
#
# The alternative -- hand back an index and let the caller read
# entries one at a time, the way `Poller` does -- is wrong here.
# Walking a tree means recursing *while* iterating, and the recursive
# open would overwrite the buffer being read. `Poller` reads in a
# tight loop and never recurses, which is why it can afford the other
# shape.
#
# **`.` and `..` are not included.** Returning them makes every
# tree-walking program an infinite loop the first time it is written.
#
# **The order is unspecified.** It is whatever the file system hands
# over, and it differs between systems for the same directory. Sort
# before comparing.
pub unsafe fn list_dir(path: str) -> Result<Vec<String>, IoError> {
    val n: u64 = __extern_fs_dir_open(path)
    val status: u64 = __extern_fs_status()
    if status != 0u64 {
        return Result::Err(fs_error_from_status(status))
    }
    var out: Vec<String> = Vec::new()
    var i: u64 = 0u64
    while i < n {
        val name: str = __extern_fs_dir_name(i)
        val owned: String = String::from_str(name)
        out.push(owned)
        i = i + 1u64
    }
    Result::Ok(out)
}

# ---------------------------------------------------------------------
# Asking about one path (F2).

# Whether `path` names a directory. False for anything else,
# including a path that does not exist -- this is a question, not a
# failure.
pub fn is_dir(path: str) -> bool { __extern_fs_is_dir(path) }

# Whether `path` names something that is not a directory.
#
# `io::file_exists` is `access(F_OK)`, which is true for a directory
# too -- its name is narrower than what it does. This is the narrower
# question.
pub fn is_file(path: str) -> bool {
    if is_dir(path) { return false }
    io::file_exists(path)
}

# The file's size in bytes.
pub fn file_size(path: str) -> Result<u64, IoError> {
    val n: u64 = __extern_fs_file_size(path)
    val status: u64 = __extern_fs_status()
    if status != 0u64 {
        return Result::Err(fs_error_from_status(status))
    }
    Result::Ok(n)
}

# ---------------------------------------------------------------------
# Changing things (F3).

pub fn mkdir(path: str) -> Result<(), IoError> {
    val status: u64 = __extern_fs_mkdir(path)
    if status != 0u64 { return Result::Err(fs_error_from_status(status)) }
    Result::Ok(())
}

pub fn remove_file(path: str) -> Result<(), IoError> {
    val status: u64 = __extern_fs_remove_file(path)
    if status != 0u64 { return Result::Err(fs_error_from_status(status)) }
    Result::Ok(())
}

# Removes an **empty** directory.
#
# There is deliberately no `remove_dir_all`. A recursive delete would
# be the first irreversible tool in this language, and it is ten lines
# on top of `list_dir` and `remove_file` -- so whoever needs it writes
# it, and owns it.
pub fn remove_dir(path: str) -> Result<(), IoError> {
    val status: u64 = __extern_fs_remove_dir(path)
    if status != 0u64 { return Result::Err(fs_error_from_status(status)) }
    Result::Ok(())
}

pub fn rename(from_path: str, to_path: str) -> Result<(), IoError> {
    val status: u64 = __extern_fs_rename(from_path, to_path)
    if status != 0u64 { return Result::Err(fs_error_from_status(status)) }
    Result::Ok(())
}

# ---------------------------------------------------------------------
# Where things are (F4).

# The absolute path with every symlink resolved.
#
# **This fails for a path that does not exist**, which is why
# `path::normalize` is separate: cleaning up a path someone typed and
# asking the kernel what a path really is are different jobs.
pub fn realpath(path: str) -> Result<String, IoError> {
    val resolved: str = __extern_fs_realpath(path)
    val status: u64 = __extern_fs_status()
    if status != 0u64 { return Result::Err(fs_error_from_status(status)) }
    val owned: String = String::from_str(resolved)
    Result::Ok(owned)
}

pub fn current_dir() -> Result<String, IoError> {
    val cwd: str = __extern_fs_current_dir()
    val status: u64 = __extern_fs_status()
    if status != 0u64 { return Result::Err(fs_error_from_status(status)) }
    val owned: String = String::from_str(cwd)
    Result::Ok(owned)
}

# The directory for temporary files: `TMPDIR` if it is set, `/tmp`
# otherwise. Read here rather than in the runtime so the rule is
# visible and testable.
pub fn temp_dir() -> String {
    val v = io::env_var("TMPDIR")
    match v {
        Result::Ok(dir) => {
            if dir.len() == 0u64 {
                val fallback: String = String::from_str("/tmp")
                fallback
            } else {
                val owned: String = String::from_str(dir)
                owned
            }
        }
        Result::Err(_) => {
            val fallback: String = String::from_str("/tmp")
            fallback
        }
    }
}

# ---------------------------------------------------------------------
# Built on the above (F5).

# Create `path` and any missing parent, like `mkdir -p`. Succeeds when
# the directory already exists.
pub unsafe fn mkdir_all(path: str) -> Result<(), IoError> {
    if is_dir(path) { return Result::Ok(()) }
    val parent: String = path::dirname(path)
    val p: str = parent.to_str()
    if p != path && p != "." && p != "/" {
        val up = mkdir_all(p)
        match up {
            Result::Ok(_) => { }
            Result::Err(e) => { return Result::Err(e) }
        }
    }
    val made = mkdir(path)
    match made {
        Result::Ok(_) => { Result::Ok(()) }
        Result::Err(e) => {
            # A racing creator is not a failure for `mkdir -p`.
            match e {
                IoError::AlreadyExists => { Result::Ok(()) }
                _ => { Result::Err(e) }
            }
        }
    }
}

# Copy a file's bytes. Returns how many were written.
#
# **Binary safe**: it goes through `read_file_into` and a `Span<u8>`
# rather than `read_file`, which would refuse anything that is not
# UTF-8 -- `str` holds text (STDLIB-TEXT §2), and a file need not.
pub unsafe fn copy_file(from_path: str, to_path: str) -> Result<u64, IoError> {
    val sized = file_size(from_path)
    val n: u64 = match sized {
        Result::Ok(v) => v,
        Result::Err(e) => { return Result::Err(e) }
    }
    var buf: Vec<u8> = Vec::with_capacity(n)
    val span = buf.capacity_span()
    val window: Span<u8> = match span {
        Option::Some(s) => s,
        Option::None => { return Result::Err(IoError::ReadError) }
    }
    val got = io::read_file_into(from_path, window)
    val read: u64 = match got {
        Result::Ok(v) => v,
        Result::Err(e) => { return Result::Err(e) }
    }
    buf.set_size(read)
    val out = buf.as_span()
    val payload: Span<u8> = match out {
        Option::Some(s) => s,
        Option::None => { return Result::Err(IoError::WriteError) }
    }
    val written = io::write_file_bytes(to_path, payload)
    written
}
