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

# ---------------------------------------------------------------------
# Open files (F4).
#
# Everything above addresses a file **by path** and touches the whole
# of it. `File` is the other half: open it once, and read or write a
# range of it.
#
# That distinction is not a performance detail. `poc/logsearch` split
# its index into a second file and capped a segment at 8 MiB purely
# because reading a footer meant reading the file
# (`RUNTIME_GAPS.md` G2 / R2, "the single largest remaining
# assumption"). `read_at` is the call that removes the reason.
#
# **The handle owns the descriptor.** `Drop` closes it at scope exit,
# `close()` closes it early, and the field is parked at `-1` after
# either -- so a second close cannot shut down whatever unrelated file
# has since been handed that number. This is `TcpStream`'s rule, for
# the same reason.
#
# **`Ok(0)` from `read` is end of file**, not a failure, and a short
# read or write is the count rather than an error: the file was
# shorter than the buffer, or the device is full. Compare the count
# with `buf.len()` -- the same reading `net::TcpStream` asks for.

extern fn __extern_file_open(path: str, mode: u64) -> i32 from "toylang_rt" as "toy_file_open"
extern fn __extern_file_close(fd: i32) -> u64 from "toylang_rt" as "toy_file_close"
extern fn __extern_file_read(fd: i32, buf: ptr, len: u64) -> u64 from "toylang_rt" as "toy_file_read"
extern fn __extern_file_write(fd: i32, buf: ptr, len: u64) -> u64 from "toylang_rt" as "toy_file_write"
extern fn __extern_file_read_at(fd: i32, buf: ptr, len: u64, offset: u64) -> u64 from "toylang_rt" as "toy_file_read_at"
extern fn __extern_file_write_at(fd: i32, buf: ptr, len: u64, offset: u64) -> u64 from "toylang_rt" as "toy_file_write_at"
extern fn __extern_file_seek(fd: i32, offset: i64, whence: u64) -> u64 from "toylang_rt" as "toy_file_seek"
extern fn __extern_file_size(fd: i32) -> u64 from "toylang_rt" as "toy_file_size"
extern fn __extern_file_sync(fd: i32) -> u64 from "toylang_rt" as "toy_file_sync"
extern fn __extern_file_truncate(fd: i32, len: u64) -> u64 from "toylang_rt" as "toy_file_truncate"
extern fn __extern_file_status() -> u64 from "toylang_rt" as "toy_file_status"

# An open file. One descriptor, closed by `Drop`.
pub struct File {
    fd: i32,
}

impl Drop for File {
    fn drop(&mut self) {
        if self.fd >= 0i32 {
            val ignored: u64 = __extern_file_close(self.fd)
            self.fd = -1i32
        }
    }
}

impl File {
    # Open an existing file for reading. `Err(IoError::NotFound)` when
    # it is not there -- this constructor never creates one.
    pub fn open(path: str) -> Result<File, IoError> {
        val f: Result<File, IoError> = open_mode(path, 0u64)
        f
    }

    # Create `path` for writing, **discarding what was there**, or
    # make it when it does not exist.
    pub fn create(path: str) -> Result<File, IoError> {
        val f: Result<File, IoError> = open_mode(path, 1u64)
        f
    }

    # Open `path` for writing at the end, creating it when missing.
    # Every write lands after what is already there, whatever the
    # cursor says -- which is why `write_at` does not belong with this
    # mode (POSIX leaves `pwrite` on an appending descriptor
    # unspecified, and Linux appends regardless of the offset).
    pub fn append(path: str) -> Result<File, IoError> {
        val f: Result<File, IoError> = open_mode(path, 2u64)
        f
    }

    # Open `path` for reading *and* writing, creating it when missing
    # and **keeping** what is there. The mode for updating a record in
    # place: it is the one `read_at` / `write_at` are meant for.
    pub fn open_rw(path: str) -> Result<File, IoError> {
        val f: Result<File, IoError> = open_mode(path, 3u64)
        f
    }

    # The descriptor, for handing to `Poller::register`.
    pub fn as_fd(&self) -> i32 {
        self.fd
    }

    # Whether this handle still owns a descriptor. False after
    # `close()`.
    pub fn is_open(&self) -> bool {
        self.fd >= 0i32
    }

    # Fill `buf` from the cursor, answering how many bytes landed in
    # it and advancing the cursor by that much.
    #
    # **`Ok(0)` is end of file.** A count below `buf.len()` is not a
    # failure either -- ask again to find out whether more follows.
    pub fn read(&self, buf: Span<u8>) -> Result<u64, IoError> {
        val n: u64 = __extern_file_read(self.fd, buf.as_raw(), buf.len())
        val r: Result<u64, IoError> = count_or_error(n)
        r
    }

    # Write `buf` at the cursor, answering how many bytes went out and
    # advancing the cursor by that much.
    #
    # **A short write is a count, not an `Err`** -- that is `write(2)`,
    # and reporting a full disk as an `Ok` with a small number is what
    # lets the caller notice it. Compare with `buf.len()`.
    pub fn write(&self, buf: Span<u8>) -> Result<u64, IoError> {
        val n: u64 = __extern_file_write(self.fd, buf.as_raw(), buf.len())
        val r: Result<u64, IoError> = count_or_error(n)
        r
    }

    # `read` from an absolute offset, **without moving the cursor**.
    #
    # This is the one that makes an index readable on its own: seek
    # nothing, read the 32 bytes of a footer, and leave the sequential
    # reader where it was. Same end-of-file and short-read rules as
    # `read`.
    pub fn read_at(&self, offset: u64, buf: Span<u8>) -> Result<u64, IoError> {
        val n: u64 = __extern_file_read_at(self.fd, buf.as_raw(), buf.len(), offset)
        val r: Result<u64, IoError> = count_or_error(n)
        r
    }

    # `write` at an absolute offset, without moving the cursor. Use it
    # with `open_rw`; on an appending descriptor the offset is ignored
    # (see `append`).
    pub fn write_at(&self, offset: u64, buf: Span<u8>) -> Result<u64, IoError> {
        val n: u64 = __extern_file_write_at(self.fd, buf.as_raw(), buf.len(), offset)
        val r: Result<u64, IoError> = count_or_error(n)
        r
    }

    # Move the cursor to `offset` from the start, answering its new
    # position.
    pub fn seek_to(&self, offset: u64) -> Result<u64, IoError> {
        val r: Result<u64, IoError> = seek_whence(self.fd, offset as i64, 0u64)
        r
    }

    # Move the cursor by `delta` from where it is (negative goes
    # back), answering its new position.
    pub fn seek_by(&self, delta: i64) -> Result<u64, IoError> {
        val r: Result<u64, IoError> = seek_whence(self.fd, delta, 1u64)
        r
    }

    # Move the cursor to `delta` from the end (`0i64` is the end
    # itself), answering its new position. Seeking past the end is
    # allowed; writing there leaves a hole of zeros.
    pub fn seek_end(&self, delta: i64) -> Result<u64, IoError> {
        val r: Result<u64, IoError> = seek_whence(self.fd, delta, 2u64)
        r
    }

    # Where the cursor is, without moving it.
    pub fn tell(&self) -> Result<u64, IoError> {
        val r: Result<u64, IoError> = seek_whence(self.fd, 0i64, 1u64)
        r
    }

    # How many bytes the file holds, **without moving the cursor**.
    # `fs::file_size(path)` answers the same question without a
    # handle.
    pub fn size(&self) -> Result<u64, IoError> {
        val n: u64 = __extern_file_size(self.fd)
        val r: Result<u64, IoError> = count_or_error(n)
        r
    }

    # Push this file's writes down to the storage device.
    #
    # The call that makes a write survive **losing power**, not just
    # losing the process. Without it a rename-based commit is ordered
    # but not durable: the OS may still be holding the bytes.
    pub fn sync(&self) -> Result<(), IoError> {
        val status: u64 = __extern_file_sync(self.fd)
        val r: Result<(), IoError> = unit_or_error(status)
        r
    }

    # Cut the file to `len` bytes, or extend it with zeros when it is
    # shorter. The cursor does not move, so this can leave it past the
    # end.
    pub fn truncate(&self, len: u64) -> Result<(), IoError> {
        val status: u64 = __extern_file_truncate(self.fd, len)
        val r: Result<(), IoError> = unit_or_error(status)
        r
    }

    # Close now rather than at scope exit. Idempotent: the field is
    # parked at `-1`, so the `Drop` that follows does nothing.
    #
    # Worth calling explicitly when the file was written: `close` is
    # where a delayed write can still fail, and `Drop` has nowhere to
    # report that.
    pub fn close(&mut self) -> Result<(), IoError> {
        if self.fd < 0i32 {
            return Result::Ok(())
        }
        val status: u64 = __extern_file_close(self.fd)
        self.fd = -1i32
        val r: Result<(), IoError> = unit_or_error(status)
        r
    }
}

# The four constructors' shared body. The mode is a number rather than
# a flag set because the `open(2)` flags differ between hosts; the
# runtime maps these four to the platform's.
fn open_mode(path: str, mode: u64) -> Result<File, IoError> {
    val fd: i32 = __extern_file_open(path, mode)
    if fd >= 0i32 {
        Result::Ok(File { fd: fd })
    } else {
        val err: IoError = fs_error_from_status(__extern_file_status())
        Result::Err(err)
    }
}

# The count-and-status pair, read together (RUNTIME-IO). The count
# alone cannot report the failure: `0` is a legitimate answer from
# every one of these calls.
fn count_or_error(n: u64) -> Result<u64, IoError> {
    val status: u64 = __extern_file_status()
    if status == 0u64 {
        Result::Ok(n)
    } else {
        val err: IoError = fs_error_from_status(status)
        Result::Err(err)
    }
}

# The same for the calls whose only answer is whether they worked.
fn unit_or_error(status: u64) -> Result<(), IoError> {
    if status == 0u64 {
        Result::Ok(())
    } else {
        val err: IoError = fs_error_from_status(status)
        Result::Err(err)
    }
}

fn seek_whence(fd: i32, offset: i64, whence: u64) -> Result<u64, IoError> {
    val pos: u64 = __extern_file_seek(fd, offset, whence)
    val r: Result<u64, IoError> = count_or_error(pos)
    r
}
