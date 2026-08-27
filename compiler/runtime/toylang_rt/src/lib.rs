//! Tiny runtime shipped alongside every compiled toylang executable.
//!
//! Rust port of the former `compiler/runtime/toylang_rt.c`. The
//! compiler's Cranelift codegen emits direct calls into the `toy_*`
//! helpers here when it lowers `print` / `println` / string
//! interpolation / the allocation builtins / the stdlib io externs.
//!
//! The crate is deliberately dependency-free and `no_std` so it can be
//! compiled twice from the same source: as an rlib for the compiler's
//! in-process JIT (which registers the function pointers in its symbol
//! table) and — via `compiler/build.rs` — as a staticlib for AOT
//! binaries. Both backends therefore execute literally the same code;
//! the former three-way mirror (C / JIT / interpreter) is down to one
//! native implementation plus the interpreter's own `heap.rs` /
//! `output` (see RUNTIME_PORT.md "やらないこと").
//!
//! ## Output sink
//!
//! All print output goes through a per-thread sink (`set_sink`). The
//! AOT binary never touches it and keeps the libc-`write` default; the
//! JIT swaps it out to capture stdout into a test-harness buffer
//! (`JitProgram::run_capturing_stdout`). This single indirection is
//! what lets the JIT share every print helper with the AOT path
//! instead of reimplementing them. Per-thread rather than global so
//! parallel `cargo test` workers can each run a capturing program
//! without sharing a buffer.
//!
//! ## Per-thread state
//!
//! `#[thread_local]` and the `thread_local!` macro are unavailable in
//! stable `no_std`, so the runtime keeps its state in one
//! heap-allocated `ThreadState` per OS thread, reached through a
//! `pthread_key_create` / `pthread_getspecific` key (POSIX-only, which
//! is all this host-only toolchain targets). Everything the C runtime
//! kept in file-scope statics lives there; the AOT binary is
//! single-threaded, and the JIT runs on whatever thread invoked
//! `main`, so per-thread state matches both.
//!
//! ## f64 display
//!
//! RUNTIME_PORT.md 論点4: the canonical f64 rendering is Rust's
//! `Display` (shortest round-trip), with the interpreter's rule that
//! integral values print with a trailing `.0` (`1f64` → `1.0`). The
//! C runtime's `%g` (6 significant digits) is gone; see
//! `docs/language.md` "Output".

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// libc surface. Declared by hand because the crate is dependency-free.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn malloc(size: usize) -> *mut u8;
    fn calloc(nmemb: usize, size: usize) -> *mut u8;
    // Only referenced by the `toylang_rt_standalone` global allocator.
    #[allow(dead_code)]
    fn realloc(ptr: *mut u8, size: usize) -> *mut u8;
    fn free(ptr: *mut u8);
    fn exit(code: i32) -> !;
    fn getenv(name: *const u8) -> *mut u8;
    fn atexit(f: extern "C" fn()) -> i32;
    fn time(t: *mut i64) -> i64;
    fn getpid() -> i32;
    fn access(path: *const u8, mode: i32) -> i32;
    // The process environment, an array of `name=value` C strings
    // terminated by a null pointer. Iterated by `toy_io_env_*`.
    static environ: *mut *mut u8;
    fn fopen(path: *const u8, mode: *const u8) -> *mut u8;
    fn fclose(f: *mut u8) -> i32;
    fn fseek(f: *mut u8, offset: i64, whence: i32) -> i32;
    fn ftell(f: *mut u8) -> i64;
    fn rewind(f: *mut u8);
    fn fread(dest: *mut u8, size: usize, count: usize, f: *mut u8) -> usize;
    fn strlen(s: *const u8) -> usize;
    fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn pthread_key_create(key: *mut usize, destructor: Option<unsafe extern "C" fn(*mut u8)>) -> i32;
    fn pthread_getspecific(key: usize) -> *mut u8;
    fn pthread_setspecific(key: usize, value: *mut u8) -> i32;
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn _NSGetArgc() -> *const i32;
    fn _NSGetArgv() -> *const *mut *mut u8;
}

const SEEK_END: i32 = 2;
const F_OK: i32 = 0;

fn write_fd(fd: i32, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    unsafe {
        write(fd, bytes.as_ptr(), bytes.len());
    }
}

fn err_write(s: &str) {
    write_fd(2, s.as_bytes());
}

fn fatal(msg: &str) -> ! {
    err_write(msg);
    unsafe { exit(1) };
}

// ---------------------------------------------------------------------------
// Per-thread state (pthread-key TLS; see the crate docs).
// ---------------------------------------------------------------------------

/// A print sink: `(bytes, len) -> ()`. Swapped by the JIT to capture
/// stdout; the AOT binary keeps [`default_sink`].
pub type SinkFn = extern "C" fn(*const u8, usize);

extern "C" fn default_sink(bytes: *const u8, len: usize) {
    if len > 0 && !bytes.is_null() {
        unsafe {
            write(1, bytes, len);
        }
    }
}

#[repr(C)]
struct BumpChunk {
    next: *mut BumpChunk,
    used: usize,
}

const BUMP_CHUNK_SIZE: usize = 1 << 20; // 1 MiB per chunk
const ALLOC_STACK_CAP: usize = 64;
const PROF_SITES_CAP: usize = 256;
const PROF_LAYOUT_CAP: usize = 64;

#[repr(C)]
#[derive(Clone, Copy)]
struct ProfSlot {
    key: *mut u8,
    size: u64,
    site: u64, // packed (line << 32) | column, MEMORY_PROFILING M2
    state: u8, // 0 empty, 1 occupied, 2 tombstone
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ProfSite {
    site: u64,
    alloc_count: u64,
    cumulative_bytes: u64,
    live_count: u64,
    live_bytes: u64,
}

const PROF_SITE_ZERO: ProfSite = ProfSite {
    site: 0,
    alloc_count: 0,
    cumulative_bytes: 0,
    live_count: 0,
    live_bytes: 0,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct ProfLayout {
    name: *const u8, // str value: points at the u64 len field
    managed: u64,
    live: u64,
    free_blocks: u64,
    largest_free: u64,
}

const PROF_LAYOUT_ZERO: ProfLayout = ProfLayout {
    name: core::ptr::null(),
    managed: 0,
    live: 0,
    free_blocks: 0,
    largest_free: 0,
};

/// The counters, field-for-field the same as
/// `interpreter::heap::MemoryStats` (see that doc comment for the
/// definitions). The JIT accessor in `compiler/src/jit.rs` converts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RtMemoryStats {
    pub alloc_count: u64,
    pub free_count: u64,
    pub realloc_count: u64,
    pub cumulative_bytes: u64,
    pub live_bytes: u64,
    pub peak_live_bytes: u64,
    pub peak_at_request: u64,
}

/// Per-site totals, mirroring `interpreter::heap::SiteStats`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RtSiteStats {
    pub alloc_count: u64,
    pub cumulative_bytes: u64,
    pub live_count: u64,
    pub live_bytes: u64,
}

/// One registered allocator layout, mirroring
/// `interpreter::heap::AllocatorLayoutReport` (name resolved to an
/// owned `String` for the accessor; the report itself re-walks the
/// raw pointer from the stored layout).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtLayout {
    pub name: String,
    pub managed: u64,
    pub live: u64,
    pub free_blocks: u64,
    pub largest_free: u64,
}

struct ThreadState {
    sink: SinkFn,
    // #121 Phase B-min: active-allocator stack. 64 nesting levels
    // covers any realistic `with allocator = ...` structure; overflow
    // aborts (the only way to hit it is a codegen bug).
    alloc_stack: [u64; ALLOC_STACK_CAP],
    alloc_stack_len: usize,
    // Bump region (DROP-GLUE). See the dispatcher section below.
    bump_head: *mut BumpChunk,
    // Profiler (MEMORY_PROFILING). State: -1 unresolved, 0 off, 1 on.
    prof_state: i8,
    prof_json: bool,
    prof_forced: bool,
    stats: RtMemoryStats,
    prof_tab: *mut ProfSlot,
    prof_tab_cap: u64,
    prof_tab_occupied: u64,
    prof_sites: [ProfSite; PROF_SITES_CAP],
    prof_site_len: usize,
    prof_layouts: [ProfLayout; PROF_LAYOUT_CAP],
    prof_layout_len: usize,
    // Program arguments for `toy_io_argc` / `toy_io_arg`. The AOT
    // binary reads the real process argv; the JIT injects these
    // (default: empty, matching a compiled binary with no arguments).
    io_args: *mut *const u8,
    io_args_len: u64,
    // RUNTIME-IO: the `toy_io_random` state. `random_seeded` is what
    // distinguishes "never seeded" (derive from clock + pid on first
    // use) from an explicit `random_seed(0)` (stay at 0), so seeding
    // with 0 is honoured rather than re-derived.
    random_state: u64,
    random_seeded: bool,
}

impl Default for ThreadState {
    fn default() -> Self {
        ThreadState {
            sink: default_sink,
            alloc_stack: [0; ALLOC_STACK_CAP],
            alloc_stack_len: 0,
            bump_head: core::ptr::null_mut(),
            prof_state: -1,
            prof_json: false,
            prof_forced: false,
            stats: RtMemoryStats::default(),
            prof_tab: core::ptr::null_mut(),
            prof_tab_cap: 0,
            prof_tab_occupied: 0,
            prof_sites: [PROF_SITE_ZERO; PROF_SITES_CAP],
            prof_site_len: 0,
            prof_layouts: [PROF_LAYOUT_ZERO; PROF_LAYOUT_CAP],
            prof_layout_len: 0,
            io_args: core::ptr::null_mut(),
            io_args_len: 0,
            random_state: 0,
            random_seeded: false,
        }
    }
}

/// Lazily created pthread key (u32 on Linux, usize on macOS; usize
/// covers both — see the libc declarations above).
static TLS_KEY: AtomicUsize = AtomicUsize::new(usize::MAX);

fn tls_key() -> usize {
    let k = TLS_KEY.load(Ordering::Relaxed);
    if k != usize::MAX {
        return k;
    }
    let mut key = 0usize;
    // Safe: `key` is writable for the call's duration and the
    // destructor is never used.
    let rc = unsafe { pthread_key_create(&mut key, None) };
    if rc != 0 {
        // Cannot happen on supported platforms; fall back to 0.
        key = 0;
    }
    TLS_KEY.store(key, Ordering::Relaxed);
    key
}

/// The calling thread's runtime state, created on first use and leaked
/// for the process lifetime (the JIT may hold it across tests).
fn thread_state() -> &'static mut ThreadState {
    let key = tls_key();
    let p = unsafe { pthread_getspecific(key) };
    if !p.is_null() {
        // Safe: the value is a boxed ThreadState we created below.
        return unsafe { &mut *(p as *mut ThreadState) };
    }
    let raw = Box::into_raw(Box::new(ThreadState::default()));
    unsafe {
        pthread_setspecific(key, raw as *mut u8);
    }
    // Safe: `raw` is uniquely owned by this thread now.
    unsafe { &mut *raw }
}

/// Replace the calling thread's output sink. `None` restores the
/// libc-`write` default used by AOT binaries.
pub fn set_sink(f: Option<SinkFn>) {
    thread_state().sink = f.unwrap_or(default_sink);
}

/// Emit program output through the current sink.
fn emit(bytes: &[u8]) {
    let sink = thread_state().sink;
    sink(bytes.as_ptr(), bytes.len());
}

// ---------------------------------------------------------------------------
// Formatting: a small stack buffer with a String fallback, so the hot
// print path allocates nothing while pathological cases (an integral
// f64 so large that `{:.1}` needs hundreds of digits) still work.
// ---------------------------------------------------------------------------

struct StackBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> StackBuf<N> {
    const fn new() -> Self {
        StackBuf { buf: [0; N], len: 0 }
    }
    /// The bytes as text. Everything written here is ASCII from
    /// `format_args!` on integers, so the conversion cannot fail.
    fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_slice()).unwrap_or("")
    }

    fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
    fn push(&mut self, b: u8) -> bool {
        if self.len < N {
            self.buf[self.len] = b;
            self.len += 1;
            true
        } else {
            false
        }
    }
}

impl<const N: usize> core::fmt::Write for StackBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if self.len + s.len() <= N {
            self.buf[self.len..self.len + s.len()].copy_from_slice(s.as_bytes());
            self.len += s.len();
            Ok(())
        } else {
            Err(core::fmt::Error)
        }
    }
}

/// Format `args` and emit it through the sink, appending `newline` if
/// requested.
fn emit_fmt(args: core::fmt::Arguments<'_>, newline: bool) {
    let mut buf = StackBuf::<64>::new();
    if core::fmt::write(&mut buf, args).is_ok() && (!newline || buf.push(b'\n')) {
        emit(buf.as_slice());
        return;
    }
    let mut s = String::new();
    let _ = s.write_fmt(args);
    if newline {
        s.push('\n');
    }
    emit(s.as_bytes());
}

// ---------------------------------------------------------------------------
// print / println helpers. One entry point per primitive width; the
// codegen call site names the actual type instead of routing through
// extensions. Formatting follows the interpreter's
// `Object::to_display_string` so all backends print byte-identical
// text.
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn toy_print_i64(v: i64) {
    emit_fmt(format_args!("{v}"), false);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_println_i64(v: i64) {
    emit_fmt(format_args!("{v}"), true);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_print_u64(v: u64) {
    emit_fmt(format_args!("{v}"), false);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_println_u64(v: u64) {
    emit_fmt(format_args!("{v}"), true);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_print_i8(v: i8) {
    emit_fmt(format_args!("{v}"), false);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_println_i8(v: i8) {
    emit_fmt(format_args!("{v}"), true);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_print_u8(v: u8) {
    emit_fmt(format_args!("{v}"), false);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_println_u8(v: u8) {
    emit_fmt(format_args!("{v}"), true);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_print_i16(v: i16) {
    emit_fmt(format_args!("{v}"), false);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_println_i16(v: i16) {
    emit_fmt(format_args!("{v}"), true);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_print_u16(v: u16) {
    emit_fmt(format_args!("{v}"), false);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_println_u16(v: u16) {
    emit_fmt(format_args!("{v}"), true);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_print_i32(v: i32) {
    emit_fmt(format_args!("{v}"), false);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_println_i32(v: i32) {
    emit_fmt(format_args!("{v}"), true);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_print_u32(v: u32) {
    emit_fmt(format_args!("{v}"), false);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_println_u32(v: u32) {
    emit_fmt(format_args!("{v}"), true);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_print_bool(v: u8) {
    emit_fmt(format_args!("{}", if v != 0 { "true" } else { "false" }), false);
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_println_bool(v: u8) {
    emit_fmt(format_args!("{}", if v != 0 { "true" } else { "false" }), true);
}

/// The canonical f64 display (RUNTIME_PORT.md 論点4): Rust `Display`,
/// with the interpreter's trailing `.0` for integral values.
#[unsafe(no_mangle)]
pub extern "C" fn toy_print_f64(v: f64) {
    if v.is_finite() && v % 1.0 == 0.0 {
        emit_fmt(format_args!("{v:.1}"), false);
    } else {
        emit_fmt(format_args!("{v}"), false);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_println_f64(v: f64) {
    if v.is_finite() && v % 1.0 == 0.0 {
        emit_fmt(format_args!("{v:.1}"), true);
    } else {
        emit_fmt(format_args!("{v}"), true);
    }
}

/// Print a str value.
///
/// Takes the handle -- the address of the length field -- not the
/// byte_start, and reads the length rather than scanning for the NUL.
/// It used to do the latter, which meant a str containing a NUL printed
/// only up to it: `"ab\u{0}cd"` came out as `ab` on every compiled
/// backend while the tree-walker printed all five bytes. The length is
/// right there in the layout; nothing had to be scanned for.
#[unsafe(no_mangle)]
pub extern "C" fn toy_print_str(s: *const u8) {
    if s.is_null() {
        return;
    }
    emit(str_bytes(s));
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_println_str(s: *const u8) {
    if !s.is_null() {
        emit(str_bytes(s));
    }
    emit(b"\n");
}

/// Recover the byte slice of a str value from its handle.
fn str_bytes<'a>(s: *const u8) -> &'a [u8] {
    let len = unsafe { (s as *const u64).read_unaligned() } as usize;
    unsafe { core::slice::from_raw_parts(s.sub(len + 1), len) }
}

// ---------------------------------------------------------------------------
// Active-allocator stack (#121 Phase B-min).
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn toy_alloc_push(handle: u64) {
    let st = thread_state();
    if st.alloc_stack_len >= ALLOC_STACK_CAP {
        fatal("toylang runtime: allocator stack overflow\n");
    }
    st.alloc_stack[st.alloc_stack_len] = handle;
    st.alloc_stack_len += 1;
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_alloc_pop() {
    let st = thread_state();
    if st.alloc_stack_len == 0 {
        fatal("toylang runtime: allocator stack underflow\n");
    }
    st.alloc_stack_len -= 1;
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_alloc_current() -> u64 {
    let st = thread_state();
    if st.alloc_stack_len == 0 {
        0 // default global allocator sentinel
    } else {
        st.alloc_stack[st.alloc_stack_len - 1]
    }
}

// ---------------------------------------------------------------------------
// Bump region (DROP-GLUE).
//
// The dispatcher hands memory out of a bump region that *never reuses
// a freed address*, exactly like the interpreter's `HeapManager`. A
// freed block keeps its contents, so a drop-glue walk that reaches the
// same boxed node twice (aliasing, a `get()` copy, a shared boxed list
// tail) reads the original values on the second visit and the free is
// an idempotent no-op. libc malloc is never called for the freed
// address again, which is also what keeps `toy_dispatched_free`
// idempotent without any reuse races.
// ---------------------------------------------------------------------------

fn bump_alloc_raw(size: usize) -> *mut u8 {
    let size = (size + 15) & !15usize; // 16-byte align
    let st = thread_state();
    let chunk = st.bump_head;
    if chunk.is_null() || unsafe { (*chunk).used } + size > BUMP_CHUNK_SIZE {
        let fresh = unsafe {
            malloc(core::mem::size_of::<BumpChunk>() + BUMP_CHUNK_SIZE) as *mut BumpChunk
        };
        if fresh.is_null() {
            return core::ptr::null_mut();
        }
        unsafe {
            (*fresh).next = chunk;
            (*fresh).used = 0;
        }
        st.bump_head = fresh;
        let p = unsafe { (fresh as *mut u8).add(core::mem::size_of::<BumpChunk>()) };
        unsafe { (*fresh).used += size };
        p
    } else {
        let p = unsafe {
            (chunk as *mut u8).add(core::mem::size_of::<BumpChunk>() + (*chunk).used)
        };
        unsafe { (*chunk).used += size };
        p
    }
}

// ---------------------------------------------------------------------------
// Profiler (MEMORY_PROFILING M1–M5).
//
// Field-for-field the same definitions as
// `interpreter/src/heap.rs::MemoryStats`, so `--all-backends
// --profile=mem` can compare the three backends verbatim. The size
// table is maintained even when no report was asked for — an
// always-on registry is what makes `toy_dispatched_free` idempotent
// (DROP-GLUE). The counters themselves are only updated when counting
// is enabled (`TOY_PROFILE_MEM`, `toy_prof_force_counting`, or the
// JIT's explicit `profiler_reset`).
// ---------------------------------------------------------------------------

fn prof_hash(p: *mut u8) -> u64 {
    let mut x = p as usize as u64;
    x >>= 4; // malloc alignment: the low bits carry no information
    x = x.wrapping_mul(0x9E3779B97F4A7C15);
    x ^ (x >> 29)
}

fn prof_enabled() -> bool {
    let st = thread_state();
    if st.prof_state < 0 {
        let p = unsafe { getenv(c"TOY_PROFILE_MEM".as_ptr().cast()) };
        let mut want_report = false;
        let mut json = false;
        if !p.is_null() {
            let v = unsafe { core::ffi::CStr::from_ptr(p as *const core::ffi::c_char) }.to_bytes();
            want_report = !v.is_empty() && v[0] != b'0';
            json = want_report && v == b"json";
        }
        st.prof_json = json;
        st.prof_state = (want_report || st.prof_forced) as i8;
        // Only an explicit request prints anything. A program that
        // asserts on `__builtin_live_bytes()` in a contract has not
        // asked for its stderr to grow a report.
        if want_report {
            unsafe {
                atexit(toy_prof_report);
            }
        }
    }
    st.prof_state != 0
}

fn prof_put(p: *mut u8, size: u64, site: u64) {
    let grow = {
        let st = thread_state();
        st.prof_tab_cap == 0 || (st.prof_tab_occupied + 1) * 4 >= st.prof_tab_cap * 3
    };
    if grow {
        prof_tab_grow();
    }
    let st = thread_state();
    let mask = st.prof_tab_cap - 1;
    let mut i = prof_hash(p) & mask;
    let tab = st.prof_tab;
    while unsafe { (*tab.add(i as usize)).state } == 1
        && unsafe { (*tab.add(i as usize)).key } != p
    {
        i = (i + 1) & mask;
    }
    if unsafe { (*tab.add(i as usize)).state } != 1 {
        st.prof_tab_occupied += 1;
    }
    unsafe {
        (*tab.add(i as usize)).key = p;
        (*tab.add(i as usize)).size = size;
        (*tab.add(i as usize)).site = site;
        (*tab.add(i as usize)).state = 1;
    }
}

fn prof_tab_grow() {
    let (old_cap, old) = {
        let st = thread_state();
        (st.prof_tab_cap, st.prof_tab)
    };
    let new_cap = if old_cap == 0 { 256 } else { old_cap * 2 };
    let fresh = unsafe { calloc(new_cap as usize, core::mem::size_of::<ProfSlot>()) } as *mut ProfSlot;
    if fresh.is_null() {
        return; // out of memory while profiling: keep running, lose accuracy
    }
    {
        let st = thread_state();
        st.prof_tab = fresh;
        st.prof_tab_cap = new_cap;
        st.prof_tab_occupied = 0;
    }
    for i in 0..old_cap as usize {
        if unsafe { (*old.add(i)).state } == 1 {
            let slot = unsafe { *old.add(i) };
            prof_put(slot.key, slot.size, slot.site);
        }
    }
    if !old.is_null() {
        unsafe { free(old as *mut u8) };
    }
}

/// Remove `p` and return the size it held, or 0 if it was not tracked
/// (a double free, or a pointer this runtime never handed out).
fn prof_take(p: *mut u8) -> (u64, u64) {
    let st = thread_state();
    if st.prof_tab_cap == 0 {
        return (0, 0);
    }
    let mask = st.prof_tab_cap - 1;
    let mut i = prof_hash(p) & mask;
    let tab = st.prof_tab;
    while unsafe { (*tab.add(i as usize)).state } != 0 {
        if unsafe { (*tab.add(i as usize)).state } == 1
            && unsafe { (*tab.add(i as usize)).key } == p
        {
            let slot = unsafe { *tab.add(i as usize) };
            unsafe { (*tab.add(i as usize)).state = 2 };
            st.prof_tab_occupied -= 1;
            return (slot.size, slot.site);
        }
        i = (i + 1) & mask;
    }
    (0, 0)
}

fn prof_site_for(st: &mut ThreadState, site: u64) -> *mut ProfSite {
    for i in 0..st.prof_site_len {
        if st.prof_sites[i].site == site {
            return &mut st.prof_sites[i];
        }
    }
    if st.prof_site_len >= PROF_SITES_CAP {
        return core::ptr::null_mut(); // beyond the cap the per-site view degrades; totals stay exact
    }
    let e = &mut st.prof_sites[st.prof_site_len];
    st.prof_site_len += 1;
    e.site = site;
    e
}

fn prof_obtained(st: &mut ThreadState, bytes: u64) {
    st.stats.cumulative_bytes += bytes;
    st.stats.live_bytes += bytes;
    if st.stats.live_bytes > st.stats.peak_live_bytes {
        st.stats.peak_live_bytes = st.stats.live_bytes;
        st.stats.peak_at_request = st.stats.alloc_count + st.stats.realloc_count;
    }
}

fn prof_released(st: &mut ThreadState, bytes: u64) {
    st.stats.live_bytes = st.stats.live_bytes.saturating_sub(bytes);
}

/// Emitted at the top of `main` when the program reads a counter, so
/// it lands before any allocation and the profiler state has not
/// resolved yet.
#[unsafe(no_mangle)]
pub extern "C" fn toy_prof_force_counting() {
    thread_state().prof_forced = true;
}

// ---------------------------------------------------------------------------
// DEBUG-OBS D4: the shadow stack.
//
// A compiled binary has no call stack it can read back — the machine
// one carries return addresses, and turning those into function names
// and lines needs a line table this project decided not to emit
// (`DEBUG_OBSERVABILITY.md` 論点 3). So the generated code keeps its
// own: one pointer per live call, pushed at the call site because that
// is where the *caller's* line is known.
//
// The globals are plain statics rather than thread-locals: toylang has
// no threads. If it ever grows them, this becomes a `#[thread_local]`
// and codegen's addressing changes with it.
// ---------------------------------------------------------------------------

/// Slots in the shadow stack. A power of two so the index is a mask
/// rather than a branch; deeper than this and the *oldest* frames
/// scroll out, since the innermost ones are what a reader wants.
pub const TOY_SHADOW_CAP: usize = 1024;

/// One frame, as codegen lays it in `.rodata`: the line the call was
/// written on (`0` = the entry function, which nothing called),
/// immediately followed by the NUL-terminated name.
///
/// The name is inline rather than a pointer so the blob needs no
/// relocation — one fewer thing for a linker to disagree about, and
/// the address codegen materialises is the whole record.
#[repr(C)]
pub struct ToyFrameInfo {
    pub line: u64,
}

/// Where the name starts inside a frame record.
const FRAME_NAME_OFFSET: usize = 8;

#[unsafe(no_mangle)]
pub static mut toy_shadow_stack: [*const ToyFrameInfo; TOY_SHADOW_CAP] =
    [core::ptr::null(); TOY_SHADOW_CAP];

/// Live call depth. Counts every push, including the ones past
/// `TOY_SHADOW_CAP` that had nowhere to go, so the report can say how
/// many frames it is not showing.
#[unsafe(no_mangle)]
pub static mut toy_shadow_depth: u64 = 0;

/// Write the backtrace for the current shadow stack, innermost first.
///
/// The folding and the head/tail budget are hand-copied from
/// `compiler_ir::render_backtrace` — this crate is deliberately
/// dependency-free, the same pairing `format_alloc_budget_violation`
/// already has. `compiler/tests/consistency/diagnostics.rs` pins the
/// two against each other by comparing stderr across engines.
/// Write the shadow stack's backtrace to stderr, or nothing when it is
/// empty.
///
/// Exported because the *interpreter's* JIT keeps the same shadow
/// stack — one runtime, one renderer — but panics through its own
/// helpers rather than `toy_panic_at`.
#[unsafe(no_mangle)]
pub extern "C" fn toy_write_backtrace() {
    write_backtrace();
}

fn write_backtrace() {
    render_backtrace_into(&mut ErrSink, true);
}

/// Render the shadow stack into `sink`, innermost first.
///
/// Folding and the head/tail budget are hand-copied from
/// `compiler_ir::render_backtrace` — this crate is deliberately
/// dependency-free, the same pairing `format_alloc_budget_violation`
/// already has. `compiler/tests/consistency/diagnostics.rs` pins the
/// two against each other by comparing stderr across engines.
fn render_backtrace_into(sink: &mut dyn BacktraceSink, leading_newline: bool) {
    let depth = unsafe { toy_shadow_depth } as usize;
    if depth == 0 {
        return;
    }
    let shown = if depth > TOY_SHADOW_CAP { TOY_SHADOW_CAP } else { depth };
    let lost = depth - shown;
    // The diagnostic path needs the blank separator; the `str` a
    // program asks for does not, and the interpreter's does not have
    // one either.
    sink.put(if leading_newline {
        "\n   = backtrace (innermost first):"
    } else {
        "   = backtrace (innermost first):"
    });

    // Two walks: the first counts the folded lines so the elision
    // count is known before anything is written, the second emits.
    let mut i = 0usize;
    let mut total = 0usize;
    while i < shown {
        let mut run = 1;
        while i + run < shown && frame_at(depth, i + run) == frame_at(depth, i) {
            run += 1;
        }
        total += 1;
        i += run;
    }
    let elided = total.saturating_sub(BACKTRACE_HEAD + BACKTRACE_TAIL);
    let mut folded_index = 0usize;
    i = 0;
    while i < shown {
        let mut run = 1;
        while i + run < shown && frame_at(depth, i + run) == frame_at(depth, i) {
            run += 1;
        }
        if elided > 0 && folded_index == BACKTRACE_HEAD {
            let mut buf = StackBuf::<64>::new();
            if core::fmt::write(&mut buf, format_args!("\n       ... {elided} frames elided")).is_ok()
            {
                sink.put(buf.as_str());
            }
        }
        if elided == 0 || folded_index < BACKTRACE_HEAD || folded_index >= BACKTRACE_HEAD + elided {
            write_frame_line(sink, frame_at(depth, i), run);
        }
        folded_index += 1;
        i += run;
    }
    if lost > 0 {
        let mut buf = StackBuf::<64>::new();
        if core::fmt::write(
            &mut buf,
            format_args!("\n       ... {lost} outermost frames not recorded"),
        )
        .is_ok()
        {
            sink.put(buf.as_str());
        }
    }
}

const BACKTRACE_HEAD: usize = 10;
const BACKTRACE_TAIL: usize = 5;

/// The `i`-th frame counting inwards from the top of the stack.
fn frame_at(depth: usize, i: usize) -> *const ToyFrameInfo {
    let slot = (depth - 1 - i) & (TOY_SHADOW_CAP - 1);
    unsafe { toy_shadow_stack[slot] }
}

fn write_frame_line(sink: &mut dyn BacktraceSink, frame: *const ToyFrameInfo, repeats: usize) {
    if frame.is_null() {
        return;
    }
    let line = unsafe { (*frame).line };
    let name = unsafe { (frame as *const u8).add(FRAME_NAME_OFFSET) };
    sink.put("\n       ");
    sink.put(unsafe { cstr_as_str(name) });
    let mut buf = StackBuf::<64>::new();
    let written = match (line, repeats) {
        (0, 1) => Ok(()),
        (0, n) => core::fmt::write(&mut buf, format_args!(" (x{n})")),
        (l, 1) => core::fmt::write(&mut buf, format_args!(" (called at line {l})")),
        (l, n) => core::fmt::write(&mut buf, format_args!(" (x{n}, called at line {l})")),
    };
    if written.is_ok() {
        sink.put(buf.as_str());
    }
}

/// # Safety
/// `p` must point at a NUL-terminated UTF-8 string that outlives the
/// borrow.
unsafe fn cstr_as_str<'a>(p: *const u8) -> &'a str {
    let mut len = 0usize;
    while len < 1 << 20 && unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(p, len)) }
}

/// DEBUG-OBS D5: the current backtrace as a toylang `str`.
///
/// `__builtin_backtrace()` lowers to a call here. The text is the same
/// one a panic prints, so a program that reports its own failures says
/// what the runtime would have.
///
/// The str layout is the one every compiled `str` uses —
/// `[bytes][NUL][u64 len]`, with the pointer at the length — so the
/// result flows through `print`, `concat` and the rest unchanged.
#[unsafe(no_mangle)]
pub extern "C" fn toy_backtrace_str() -> *const u8 {
    // Two passes: the first measures, the second fills. A single
    // generous allocation would be simpler and would truncate a deep
    // stack, which is the one thing a backtrace must not do quietly.
    let mut counter = ByteCounter { len: 0 };
    render_backtrace_into(&mut counter, false);
    let total = counter.len;
    let base = unsafe { malloc(total + 1 + 8) };
    if base.is_null() {
        fatal("toy_backtrace_str: out of memory\n");
    }
    let mut writer = BufWriter { base, offset: 0, cap: total };
    render_backtrace_into(&mut writer, false);
    unsafe {
        *base.add(total) = 0;
        (base.add(total + 1) as *mut u64).write_unaligned(total as u64);
    }
    unsafe { base.add(total + 1) }
}

/// Somewhere backtrace text can go: stderr, a length, or a buffer.
///
/// One renderer, three destinations — the alternative is three copies
/// of the folding rules, which is how the engines' diagnostics drifted
/// apart before D0.
trait BacktraceSink {
    fn put(&mut self, s: &str);
}

struct ErrSink;
impl BacktraceSink for ErrSink {
    fn put(&mut self, s: &str) {
        err_write(s);
    }
}

struct ByteCounter {
    len: usize,
}
impl BacktraceSink for ByteCounter {
    fn put(&mut self, s: &str) {
        self.len += s.len();
    }
}

struct BufWriter {
    base: *mut u8,
    offset: usize,
    cap: usize,
}
impl BacktraceSink for BufWriter {
    fn put(&mut self, s: &str) {
        let n = s.len().min(self.cap - self.offset);
        if n > 0 {
            unsafe { memcpy(self.base.add(self.offset), s.as_ptr(), n) };
            self.offset += n;
        }
    }
}

/// DEBUG-OBS D3: write a pre-rendered diagnostic to stderr and stop.
///
/// `text` is a NUL-terminated blob the compiler laid in `.rodata`,
/// holding the *entire* message a failing run prints — header,
/// `Error at file:line:column:`, the quoted source line, the caret and
/// the message. All of it is static (an interned literal at a fixed
/// position), so there is nothing to format here and nothing to look
/// up: this binary never reads the source it was built from.
///
/// It replaces a `puts` call, which put panics on **stdout** — in the
/// middle of whatever the program had already printed, and out of
/// reach of a `2>` redirect (`DEBUG_OBSERVABILITY.md` 実測 9).
///
/// # Safety
/// `text` must point at a NUL-terminated byte string that outlives the
/// call. Codegen only ever passes the address of a `.rodata` blob.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn toy_panic_at(text: *const u8) -> ! {
    unsafe { write_cstr_fd(2, text) };
    write_backtrace();
    err_write("\n");
    unsafe { exit(1) };
}

/// Write a NUL-terminated string to `fd`.
///
/// # Safety
/// `p` must point at a NUL-terminated byte string.
unsafe fn write_cstr_fd(fd: i32, p: *const u8) {
    if p.is_null() {
        return;
    }
    let mut len = 0usize;
    // Bounded so a blob that somehow lost its terminator cannot walk
    // the address space; no diagnostic this crate emits is near it.
    while len < 1 << 20 && unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    write_fd(fd, unsafe { core::slice::from_raw_parts(p, len) });
}

/// ALLOC-CONTRACT-SUGAR: report a violated allocation budget and stop.
///
/// `Terminator::Panic` can only carry a static message, so a compiled
/// binary could say "ensures violation" and nothing else; this takes
/// the readings and formats them the way the interpreter does.
///
/// The wording is duplicated from
/// `compiler_ir::format_alloc_budget_violation` — this crate is
/// deliberately dependency-free, so it cannot call it. The pairing is
/// the same one `Spec` already has with `frontend::format_spec`, and
/// `compiler/tests/consistency.rs` pins the two against each other by
/// comparing stderr across backends.
///
/// `stat` is `frontend::ast::MemStat::code()`.
/// # Safety
/// `prefix` and `suffix` must each be NUL-terminated or null. They are
/// the static halves of the diagnostic frame around the computed
/// message (DEBUG-OBS D3); codegen passes `.rodata` addresses.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn toy_panic_alloc_budget(
    stat: u64,
    entry: u64,
    current: u64,
    limit: u64,
    prefix: *const u8,
    suffix: *const u8,
) -> ! {
    let used = current.saturating_sub(entry);
    let budget = limit.saturating_sub(entry);
    unsafe { write_cstr_fd(2, prefix) };
    let mut buf = StackBuf::<128>::new();
    let written = match stat {
        // MemStat::CumulativeBytes
        3 => core::fmt::write(
            &mut buf,
            format_args!("panic: requested {used} bytes, budget {budget} bytes"),
        ),
        // MemStat::LiveBytes
        4 => core::fmt::write(
            &mut buf,
            format_args!("panic: retained {used} bytes, budget {budget} bytes"),
        ),
        // MemStat::AllocCount
        0 => core::fmt::write(
            &mut buf,
            format_args!("panic: made {used} allocations, budget {budget}"),
        ),
        _ => core::fmt::write(
            &mut buf,
            format_args!("panic: allocation budget exceeded: {used} over {budget}"),
        ),
    };
    if written.is_ok() {
        write_fd(2, buf.as_slice());
    } else {
        err_write("panic: allocation budget exceeded");
    }
    unsafe { write_cstr_fd(2, suffix) };
    write_backtrace();
    err_write("\n");
    unsafe { exit(1) };
}

/// One counter, selected by `frontend::ast::MemStat::code()`. The
/// numbering is shared with that enum; it is an ABI, not an
/// implementation detail.
#[unsafe(no_mangle)]
pub extern "C" fn toy_prof_stat(which: u64) -> u64 {
    prof_enabled();
    let st = thread_state();
    match which {
        0 => st.stats.alloc_count,
        1 => st.stats.free_count,
        2 => st.stats.realloc_count,
        3 => st.stats.cumulative_bytes,
        4 => st.stats.live_bytes,
        5 => st.stats.peak_live_bytes,
        _ => 0,
    }
}

/// MEMORY_PROFILING M3 residual. A region-owning allocator pushes its
/// final layout here (from its `Drop`) so the report can fold
/// fragmentation in without the runtime reaching back into a toylang
/// object. Entries are recorded even when no report was asked for —
/// the cost is one push per dropped allocator, not a hot path.
#[unsafe(no_mangle)]
pub extern "C" fn toy_record_allocator_layout(
    name: *const u8,
    managed: u64,
    live: u64,
    free_blocks: u64,
    largest_free: u64,
) {
    let st = thread_state();
    if st.prof_layout_len >= PROF_LAYOUT_CAP {
        return;
    }
    let e = &mut st.prof_layouts[st.prof_layout_len];
    st.prof_layout_len += 1;
    e.name = name;
    e.managed = managed;
    e.live = live;
    e.free_blocks = free_blocks;
    e.largest_free = largest_free;
}

/// External fragmentation, permille. Mirrors the interpreter's
/// `AllocatorLayoutReport::external_fragmentation_permille`.
fn prof_fragmentation(l: &ProfLayout) -> u64 {
    if l.managed <= l.live {
        return 0;
    }
    let free_total = l.managed - l.live;
    if free_total == 0 {
        return 0;
    }
    let scattered = free_total - l.largest_free;
    scattered * 1000 / free_total
}

/// The str name of a layout entry, resolved from its handle.
fn layout_name(l: &ProfLayout) -> String {
    if l.name.is_null() {
        return String::new();
    }
    let bytes = str_bytes(l.name);
    // The name is NUL-terminated by the str layout; strip anything
    // past the recorded length defensively.
    String::from_utf8_lossy(bytes).into_owned()
}

fn prof_report_layouts() {
    let st = thread_state();
    if st.prof_layout_len == 0 {
        return;
    }
    err_write("allocator layouts\n");
    for i in 0..st.prof_layout_len {
        let l = &st.prof_layouts[i];
        err_write(&format!(
            "  {}  managed {}  live {}  free_blocks {}  largest_free {}  external_fragmentation {} permille\n",
            layout_name(l),
            l.managed,
            l.live,
            l.free_blocks,
            l.largest_free,
            prof_fragmentation(l),
        ));
    }
}

fn prof_report_layouts_json() {
    let st = thread_state();
    if st.prof_layout_len == 0 {
        err_write("  \"layouts\": []\n");
        return;
    }
    err_write("  \"layouts\": [\n");
    for i in 0..st.prof_layout_len {
        let l = &st.prof_layouts[i];
        let comma = if i + 1 == st.prof_layout_len { "" } else { "," };
        err_write(&format!(
            "    {{\n      \"name\": \"{}\",\n      \"managed\": {},\n      \"live\": {},\n      \"free_blocks\": {},\n      \"largest_free\": {},\n      \"external_fragmentation_permille\": {}\n    }}{}\n",
            layout_name(l),
            l.managed,
            l.live,
            l.free_blocks,
            l.largest_free,
            prof_fragmentation(l),
            comma,
        ));
    }
    err_write("  ]\n");
}

/// Sites that still hold memory at exit, in source order (selection
/// sort by packed position: the table is tiny and insertion order is
/// not source order).
fn leak_sites_sorted() -> Vec<(u64, RtSiteStats)> {
    let st = thread_state();
    let mut leaked = Vec::new();
    for i in 0..st.prof_site_len {
        let s = &st.prof_sites[i];
        if s.live_count > 0 {
            leaked.push((
                s.site,
                RtSiteStats {
                    alloc_count: s.alloc_count,
                    cumulative_bytes: s.cumulative_bytes,
                    live_count: s.live_count,
                    live_bytes: s.live_bytes,
                },
            ));
        }
    }
    leaked.sort_by_key(|(site, _)| *site);
    leaked
}

fn prof_report_leaks() {
    let leaked = leak_sites_sorted();
    if leaked.is_empty() {
        return;
    }
    let count: u64 = leaked.iter().map(|(_, s)| s.live_count).sum();
    let bytes: u64 = leaked.iter().map(|(_, s)| s.live_bytes).sum();
    err_write(&format!(
        "leaks ({} sites, {count} allocations, {bytes} bytes)\n",
        leaked.len()
    ));
    for (site, s) in &leaked {
        err_write(&format!(
            "  {}:{}  {} allocations  {} bytes\n",
            site >> 32,
            site & 0xffff_ffff,
            s.live_count,
            s.live_bytes,
        ));
    }
}

fn prof_report_json() {
    let stats = thread_state().stats;
    err_write("{\n  \"memory_profile\": {\n");
    err_write(&format!("    \"alloc_count\": {},\n", stats.alloc_count));
    err_write(&format!("    \"free_count\": {},\n", stats.free_count));
    err_write(&format!("    \"realloc_count\": {},\n", stats.realloc_count));
    err_write(&format!("    \"cumulative_bytes\": {},\n", stats.cumulative_bytes));
    err_write(&format!("    \"live_bytes\": {},\n", stats.live_bytes));
    err_write(&format!("    \"peak_live_bytes\": {},\n", stats.peak_live_bytes));
    err_write(&format!("    \"peak_at_request\": {}\n", stats.peak_at_request));
    err_write("  },\n");

    let leaked = leak_sites_sorted();
    if leaked.is_empty() {
        err_write("  \"leaks\": [],\n");
    } else {
        err_write("  \"leaks\": [\n");
        for (i, (site, s)) in leaked.iter().enumerate() {
            let comma = if i + 1 == leaked.len() { "" } else { "," };
            err_write(&format!(
                "    {{\n      \"line\": {},\n      \"column\": {},\n      \"allocations\": {},\n      \"bytes\": {}\n    }}{}\n",
                site >> 32,
                site & 0xffff_ffff,
                s.live_count,
                s.live_bytes,
                comma,
            ));
        }
        err_write("  ],\n");
    }
    prof_report_layouts_json();
    err_write("}\n");
}

extern "C" fn toy_prof_report() {
    if thread_state().prof_json {
        prof_report_json();
        return;
    }
    let st = thread_state();
    err_write("memory profile\n");
    err_write(&format!("  {:<16}  {}\n", "alloc_count", st.stats.alloc_count));
    err_write(&format!("  {:<16}  {}\n", "free_count", st.stats.free_count));
    err_write(&format!("  {:<16}  {}\n", "realloc_count", st.stats.realloc_count));
    err_write(&format!("  {:<16}  {}\n", "cumulative_bytes", st.stats.cumulative_bytes));
    err_write(&format!("  {:<16}  {}\n", "live_bytes", st.stats.live_bytes));
    err_write(&format!("  {:<16}  {}\n", "peak_live_bytes", st.stats.peak_live_bytes));
    err_write(&format!("  {:<16}  {}\n", "peak_at_request", st.stats.peak_at_request));
    prof_report_leaks();
    prof_report_layouts();
}

/// Reset the profiler's per-thread state and enable counting. Used by
/// the compiler's JIT before a `--profile=mem` run; the AOT binary
/// relies on `TOY_PROFILE_MEM` / `toy_prof_force_counting` instead.
/// The injected program arguments are preserved — they are harness
/// state for the current run, not profiler state.
pub fn profiler_reset() {
    let st = thread_state();
    let io_args = st.io_args;
    let io_args_len = st.io_args_len;
    *st = ThreadState::default();
    st.io_args = io_args;
    st.io_args_len = io_args_len;
    st.prof_forced = true;
}

/// The calling thread's allocation totals (JIT accessor).
pub fn profiler_stats() -> RtMemoryStats {
    thread_state().stats
}

/// Leaking sites, in source order (JIT accessor). Sorted so the text
/// report reads the same as the interpreter's.
pub fn profiler_sites() -> Vec<(u64, RtSiteStats)> {
    leak_sites_sorted()
}

/// Registered allocator layouts, in registration order (JIT accessor).
pub fn profiler_layouts() -> Vec<RtLayout> {
    let st = thread_state();
    (0..st.prof_layout_len)
        .map(|i| RtLayout {
            name: layout_name(&st.prof_layouts[i]),
            managed: st.prof_layouts[i].managed,
            live: st.prof_layouts[i].live,
            free_blocks: st.prof_layouts[i].free_blocks,
            largest_free: st.prof_layouts[i].largest_free,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Dispatched alloc / realloc / free.
//
// Routed from the AOT-emitted `__builtin_heap_alloc` / `_realloc` /
// `_free` after they read `toy_alloc_current()`. The runtime arena /
// fixed_buffer registry has been retired — the toylang stdlib
// `Arena` / `FixedBuffer` (`core/std/allocator.t`) reimplements both
// policies on top of the default allocator. Today every dispatched
// call routes through the bump region below; the `handle` argument is
// preserved in the IR for forward compatibility but currently
// ignored.
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn toy_dispatched_alloc(_handle: u64, size: u64, site: u64) -> *mut u8 {
    // A zero-size request yields the null pointer and is not counted,
    // matching the interpreter. libc would hand back a unique
    // non-null pointer here, which would then differ.
    if size == 0 {
        return core::ptr::null_mut();
    }
    let p = bump_alloc_raw(size as usize);
    if !p.is_null() {
        // DROP-GLUE: the size table is maintained even when no report
        // was asked for — an always-on registry is what makes
        // `toy_dispatched_free` idempotent (see below).
        prof_put(p, size, site);
        if prof_enabled() {
            let st = thread_state();
            st.stats.alloc_count += 1;
            prof_obtained(st, size);
            let e = prof_site_for(st, site);
            if !e.is_null() {
                unsafe {
                    (*e).alloc_count += 1;
                    (*e).cumulative_bytes += size;
                    (*e).live_count += 1;
                    (*e).live_bytes += size;
                }
            }
        }
    }
    p
}

/// Free is *idempotent*: an address the runtime never handed out — or
/// already freed — is a no-op instead of a libc double-free. That is
/// what makes recursive drop glue safe under this language's aliasing
/// (`val b = a`, a `get()` copy, a boxed node shared by two paths):
/// the first free wins, later visits of the same address do nothing,
/// and the counters record the free exactly once, matching the
/// interpreter's `HeapManager::free`. The block itself is *not* handed
/// back to malloc — the bump region never reuses addresses, so a later
/// glue walk reads the block's original contents, not garbage.
#[unsafe(no_mangle)]
pub extern "C" fn toy_dispatched_free(_handle: u64, p: *mut u8) {
    if p.is_null() {
        return; // freeing null is a no-op and is not counted
    }
    let (size, site) = prof_take(p);
    if size == 0 {
        return; // already freed, or not this runtime's memory
    }
    if prof_enabled() {
        let st = thread_state();
        st.stats.free_count += 1;
        prof_released(st, size);
        let e = prof_site_for(st, site);
        if !e.is_null() {
            unsafe {
                (*e).live_count = (*e).live_count.saturating_sub(1);
                (*e).live_bytes = (*e).live_bytes.saturating_sub(size);
            }
        }
    }
    // No libc free: the bump region never reuses an address, so a
    // later drop-glue visit of this block reads its original contents.
}

/// # Safety
/// `p` must be null or a pointer this runtime handed out (unless
/// `new_size == 0`, which frees it).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn toy_dispatched_realloc(
    _handle: u64,
    p: *mut u8,
    new_size: u64,
) -> *mut u8 {
    if p.is_null() {
        return toy_dispatched_alloc(_handle, new_size, 0);
    }
    if new_size == 0 {
        toy_dispatched_free(_handle, p);
        return core::ptr::null_mut();
    }
    // DROP-GLUE: the registry is always maintained (see
    // `toy_dispatched_free`), so the old-size lookup is unconditional
    // too — a resize of an untracked pointer is a no-op bookkeeping
    // wise.
    let (old_size, site) = prof_take(p);
    if prof_enabled() {
        let st = thread_state();
        st.stats.realloc_count += 1;
        if new_size > old_size {
            prof_obtained(st, new_size - old_size);
        } else {
            prof_released(st, old_size - new_size);
        }
        // A resize keeps the site its block already had, so a leak
        // still points at where the memory came from.
        let e = prof_site_for(st, site);
        if !e.is_null() {
            unsafe {
                if new_size > old_size {
                    (*e).cumulative_bytes += new_size - old_size;
                    (*e).live_bytes += new_size - old_size;
                } else {
                    let shrank = old_size - new_size;
                    (*e).live_bytes = (*e).live_bytes.saturating_sub(shrank);
                }
            }
        }
    }
    // Bump-region move: a fresh block, old contents copied, the old
    // block left in place (it is never reused).
    let np = bump_alloc_raw(new_size as usize);
    if np.is_null() {
        prof_put(p, old_size, site); // restore tracking on failure
        return core::ptr::null_mut();
    }
    if old_size > 0 {
        unsafe {
            memcpy(np, p, (old_size.min(new_size)) as usize);
        }
    }
    prof_put(np, new_size, site);
    np
}

// ---------------------------------------------------------------------------
// Heap-allocated str helpers (string interpolation Phase 2).
//
// AOT `str` runtime layout, per `compiler/src/codegen/lower_inst.rs`
// `ConstStr` / `Print`:
//
//     [bytes...][NUL][u64 len LE]
//      ^                ^
//      byte_start       (str runtime value points here)
//
// Heap-allocated strings (produced by `__builtin_to_string` and
// `.concat()`) follow the exact same layout so every consumer of
// `str` (print / println / strlen / interpolation chain) is
// pointer-uniform: a `str` value always points at its u64 len field;
// `byte_start = s - len - 1`.
//
// Allocation goes through libc malloc directly rather than the
// active toylang allocator stack — interpolation strings are
// typically short-lived and routing them through the user-facing
// allocator could surprise programs that swap in a quota-limited
// fixed_buffer for a different purpose. `free` is the caller's
// responsibility (currently a no-op; relies on process exit).
// ---------------------------------------------------------------------------

/// Lay out a fresh heap str.
///
/// Public so the interpreter's own JIT can materialise string literals
/// in the same shape rather than keeping a second copy of the layout.
pub fn toy_str_alloc(bytes: &[u8]) -> *const u8 {
    let len = bytes.len();
    let base = unsafe { malloc(len + 1 + 8) };
    if base.is_null() {
        fatal("toy_str_alloc: out of memory\n");
    }
    if !bytes.is_empty() {
        unsafe {
            memcpy(base, bytes.as_ptr(), len);
        }
    }
    unsafe {
        *base.add(len) = 0; // NUL terminator
        // Length stored little-endian (host order = LE on every
        // cranelift target the compiler currently supports).
        (base.add(len + 1) as *mut u64).write_unaligned(len as u64);
    }
    unsafe { base.add(len + 1) as *const u8 }
}

/// `a == b` between two toylang str values: compare the bytes.
///
/// The runtime value points at the trailing u64 length, so the bytes
/// start at `p - len - 1`. Comparing the handles instead would make two
/// equal strings unequal unless they came from the same literal.
/// Returns i8 to match the cranelift Bool representation.
///
/// # Safety
/// Both handles must point at valid str layouts (`[bytes][NUL][u64 len]`,
/// value at the len field).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn toy_str_eq(a: *const u8, b: *const u8) -> i8 {
    if a == b {
        return 1;
    }
    if a.is_null() || b.is_null() {
        return 0;
    }
    let (la, lb) = unsafe {
        (
            (a as *const u64).read_unaligned(),
            (b as *const u64).read_unaligned(),
        )
    };
    if la != lb {
        return 0;
    }
    if la == 0 {
        return 1;
    }
    let (abytes, bbytes) = unsafe {
        (
            core::slice::from_raw_parts(a.sub(la as usize + 1), la as usize),
            core::slice::from_raw_parts(b.sub(lb as usize + 1), lb as usize),
        )
    };
    (abytes == bbytes) as i8
}

/// `__builtin_str_from_bytes(p, len)` — exported wrapper over the
/// allocator above so codegen can call it directly. Copies, so the
/// resulting str is unaffected by later writes to the buffer.
/// # Safety
/// `bytes` must be readable for `len` bytes when `len > 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn toy_str_from_bytes(bytes: *const u8, len: u64) -> *const u8 {
    if len == 0 || bytes.is_null() {
        return toy_str_alloc(&[]);
    }
    let slice = unsafe { core::slice::from_raw_parts(bytes, len as usize) };
    toy_str_alloc(slice)
}

/// Concatenate two toylang str values. Both arguments and the result
/// follow the runtime layout described above.
///
/// # Safety
/// Both handles must point at valid str layouts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn toy_str_concat(a: *const u8, b: *const u8) -> *const u8 {
    let (la, lb) = unsafe {
        (
            (a as *const u64).read_unaligned(),
            (b as *const u64).read_unaligned(),
        )
    };
    let a_bytes = unsafe { core::slice::from_raw_parts(a.sub(la as usize + 1), la as usize) };
    let b_bytes = unsafe { core::slice::from_raw_parts(b.sub(lb as usize + 1), lb as usize) };
    let total = la + lb;
    let base = unsafe { malloc(total as usize + 1 + 8) };
    if base.is_null() {
        fatal("toy_str_concat: out of memory\n");
    }
    if la > 0 {
        unsafe {
            memcpy(base, a_bytes.as_ptr(), la as usize);
        }
    }
    if lb > 0 {
        unsafe {
            memcpy(base.add(la as usize), b_bytes.as_ptr(), lb as usize);
        }
    }
    unsafe {
        *base.add(total as usize) = 0;
        (base.add(total as usize + 1) as *mut u64).write_unaligned(total);
    }
    unsafe { base.add(total as usize + 1) as *const u8 }
}

/// Format a value into a heap str. `fmt` formats into a stack buffer
/// with a String fallback, then copies into the str layout.
fn to_string_fmt(args: core::fmt::Arguments<'_>) -> *const u8 {
    let mut buf = StackBuf::<64>::new();
    if core::fmt::write(&mut buf, args).is_ok() {
        return toy_str_alloc(buf.as_slice());
    }
    let mut s = String::new();
    let _ = s.write_fmt(args);
    toy_str_alloc(s.as_bytes())
}

/// `__builtin_to_string(value)` lowering — one entry point per
/// primitive type. Each formats with the same conventions
/// `Object::to_display_string` uses in the interpreter so
/// interpreter / AOT stay byte-identical for string-interpolation
/// output.
#[unsafe(no_mangle)]
pub extern "C" fn toy_to_string_i64(v: i64) -> *const u8 {
    to_string_fmt(format_args!("{v}"))
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_to_string_u64(v: u64) -> *const u8 {
    to_string_fmt(format_args!("{v}"))
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_to_string_f64(v: f64) -> *const u8 {
    if v.is_finite() && v % 1.0 == 0.0 {
        to_string_fmt(format_args!("{v:.1}"))
    } else {
        to_string_fmt(format_args!("{v}"))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_to_string_bool(v: u8) -> *const u8 {
    if v != 0 {
        to_string_fmt(format_args!("true"))
    } else {
        to_string_fmt(format_args!("false"))
    }
}

// ---------------------------------------------------------------
// STR-INTERP-FMT: `__builtin_format(value, spec)`
//
// The spec is a compile-time constant packed by
// `frontend::format_spec::FormatSpec::pack`. This crate is `no_std`
// and dependency-free, so it decodes the same bit layout instead of
// sharing the type:
//
// | bits | field |
// |---|---|
// | 0-15 | width (0 = none) |
// | 16-23 | precision + 1 (0 = none) |
// | 24-25 | align (0 = default, 1 = left, 2 = right, 3 = center) |
// | 26 | zero-pad flag |
// | 27-29 | radix (0 = dec, 1 = hex, 2 = HEX, 3 = bin, 4 = oct) |
//
// Any change here has to be mirrored in `frontend/src/format_spec.rs`;
// the cross-backend consistency tests compare the rendered output.
// ---------------------------------------------------------------

struct Spec {
    align: u8,
    zero_pad: bool,
    width: usize,
    precision: Option<usize>,
    radix: u8,
}

impl Spec {
    fn unpack(code: u64) -> Spec {
        let precision = ((code >> 16) & 0xFF) as usize;
        Spec {
            align: ((code >> 24) & 0x3) as u8,
            zero_pad: (code >> 26) & 1 != 0,
            width: (code & 0xFFFF) as usize,
            precision: if precision == 0 {
                None
            } else {
                Some(precision - 1)
            },
            radix: ((code >> 27) & 0x7) as u8,
        }
    }

    /// Width / alignment, matching `FormatSpec::pad`.
    fn pad(&self, body: &str, numeric: bool) -> String {
        let len = body.chars().count();
        if len >= self.width {
            return String::from(body);
        }
        let fill = self.width - len;
        if self.zero_pad && numeric && self.align == 0 {
            let (sign, digits) = match body.strip_prefix('-') {
                Some(rest) => ("-", rest),
                None => ("", body),
            };
            let mut out = String::from(sign);
            for _ in 0..fill {
                out.push('0');
            }
            out.push_str(digits);
            return out;
        }
        let align = match self.align {
            0 if numeric => 2,
            0 => 1,
            other => other,
        };
        let mut out = String::new();
        let (left, right) = match align {
            1 => (0, fill),
            3 => (fill / 2, fill - fill / 2),
            _ => (fill, 0),
        };
        for _ in 0..left {
            out.push(' ');
        }
        out.push_str(body);
        for _ in 0..right {
            out.push(' ');
        }
        out
    }

    fn render_uint(&self, magnitude: u64, is_negative: bool, bits: u32) -> String {
        let body = if self.radix == 0 {
            if is_negative {
                format!("-{magnitude}")
            } else {
                format!("{magnitude}")
            }
        } else {
            let raw = if is_negative {
                let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
                magnitude.wrapping_neg() & mask
            } else {
                magnitude
            };
            match self.radix {
                1 => format!("{raw:x}"),
                2 => format!("{raw:X}"),
                3 => format!("{raw:b}"),
                _ => format!("{raw:o}"),
            }
        };
        self.pad(&body, true)
    }

    fn render_f64(&self, v: f64) -> String {
        let body = match self.precision {
            Some(p) => format!("{v:.*}", p),
            None => {
                if v.is_finite() && v % 1.0 == 0.0 {
                    format!("{v:.1}")
                } else {
                    format!("{v}")
                }
            }
        };
        self.pad(&body, true)
    }
}

/// Signed integers of every width. `bits` is the value's own width so
/// a non-decimal radix shows the two's-complement pattern at that
/// width (`{-1i32:x}` is `ffffffff`); codegen sign-extends the value
/// to i64 before the call and passes the original width here.
#[unsafe(no_mangle)]
pub extern "C" fn toy_format_i64(v: i64, spec: u64, bits: u64) -> *const u8 {
    let spec = Spec::unpack(spec);
    toy_str_alloc(spec.render_uint(v.unsigned_abs(), v < 0, bits as u32).as_bytes())
}

/// Unsigned integers of every width; codegen zero-extends to i64.
#[unsafe(no_mangle)]
pub extern "C" fn toy_format_u64(v: u64, spec: u64, bits: u64) -> *const u8 {
    let spec = Spec::unpack(spec);
    toy_str_alloc(spec.render_uint(v, false, bits as u32).as_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_format_f64(v: f64, spec: u64) -> *const u8 {
    let spec = Spec::unpack(spec);
    toy_str_alloc(spec.render_f64(v).as_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn toy_format_bool(v: u8, spec: u64) -> *const u8 {
    let spec = Spec::unpack(spec);
    let body = if v != 0 { "true" } else { "false" };
    toy_str_alloc(spec.pad(body, false).as_bytes())
}

/// Unlike `toy_to_string_str`, this cannot return the argument
/// unchanged: padding produces new bytes.
#[unsafe(no_mangle)]
pub extern "C" fn toy_format_str(s: *const u8, spec: u64) -> *const u8 {
    let spec = Spec::unpack(spec);
    let bytes = str_bytes(s);
    let text = core::str::from_utf8(bytes).unwrap_or("");
    toy_str_alloc(spec.pad(text, false).as_bytes())
}

/// str -> str: identity. The desugaring lifts every `{expr}` segment
/// through `__builtin_to_string`, even when `expr` is already `str`,
/// so the codegen call site can stay type-uniform. Returning the
/// original handle avoids a redundant heap copy.
#[unsafe(no_mangle)]
pub extern "C" fn toy_to_string_str(s: *const u8) -> *const u8 {
    s
}

/// Narrow integer to_string variants; each promotes through the wide
/// formatter of the matching width.
#[unsafe(no_mangle)]
pub extern "C" fn toy_to_string_i8(v: i8) -> *const u8 {
    toy_to_string_i64(v as i64)
}
#[unsafe(no_mangle)]
pub extern "C" fn toy_to_string_u8(v: u8) -> *const u8 {
    toy_to_string_u64(v as u64)
}
#[unsafe(no_mangle)]
pub extern "C" fn toy_to_string_i16(v: i16) -> *const u8 {
    toy_to_string_i64(v as i64)
}
#[unsafe(no_mangle)]
pub extern "C" fn toy_to_string_u16(v: u16) -> *const u8 {
    toy_to_string_u64(v as u64)
}
#[unsafe(no_mangle)]
pub extern "C" fn toy_to_string_i32(v: i32) -> *const u8 {
    toy_to_string_i64(v as i64)
}
#[unsafe(no_mangle)]
pub extern "C" fn toy_to_string_u32(v: u32) -> *const u8 {
    toy_to_string_u64(v as u64)
}

// ---------------------------------------------------------------------------
// RUNTIME-IO: stdlib I/O externs (core/std/io.t).
//
// Each `toy_io_*` below backs one `extern fn` declaration; the
// lowering maps the declared name to the symbol here via
// `compiler_lower::program::libm_import_name_for` (interpreter:
// `extern_io::build_io_registry`).
//
// `str` arguments arrive as toylang str handles — pointers to the
// trailing `u64 len` field of a `[bytes][NUL][u64 len]` blob. Results
// are returned the same way. Failure convention: `""` for "not found /
// unreadable" — the extern boundary cannot carry a `Result`, so
// callers probe with `toy_io_file_exists`.
// ---------------------------------------------------------------------------

/// Copy a str handle into a NUL-terminated C string (malloc'd via the
/// Vec's allocator).
fn str_to_cstring(s: *const u8) -> Vec<u8> {
    let bytes = if s.is_null() { &[][..] } else { str_bytes(s) };
    let mut out = Vec::with_capacity(bytes.len() + 1);
    out.extend_from_slice(bytes);
    out.push(0);
    out
}

/// Program argv. The compiled binary's `main` *is* the toylang main,
/// so there is no wrapper to receive argv at startup; macOS exposes
/// the original vector via `_NSGetArgc` / `_NSGetArgv`, and Linux is
/// served from `/proc/self/cmdline` on first use. The JIT injects its
/// own vector with [`set_program_args`], which takes precedence.
fn io_argv_vec() -> *mut *mut u8 {
    let st = thread_state();
    if !st.io_args.is_null() {
        return st.io_args as *mut *mut u8;
    }
    #[cfg(target_os = "macos")]
    {
        // Safe: `_NSGetArgv` returns the kernel-provided argv vector.
        unsafe { *_NSGetArgv() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let f = unsafe { fopen(c"/proc/self/cmdline".as_ptr().cast(), c"rb".as_ptr().cast()) };
        if f.is_null() {
            return core::ptr::null_mut();
        }
        // Read the whole file into a growing buffer.
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let got = unsafe { fread(chunk.as_mut_ptr(), 1, chunk.len(), f) };
            if got == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..got]);
        }
        unsafe { fclose(f) };
        // Split on NULs into an argv array. The buffer must outlive
        // the array, so leak it and carve the array from a fresh
        // allocation.
        let mut count = 0;
        for &b in &buf {
            if b == 0 {
                count += 1;
            }
        }
        if count == 0 {
            return core::ptr::null_mut();
        }
        let storage = unsafe { calloc(count + 1, core::mem::size_of::<*mut u8>()) } as *mut *mut u8;
        if storage.is_null() {
            return core::ptr::null_mut();
        }
        let leaked = Box::into_raw(buf.into_boxed_slice());
        let mut i = 0;
        for (off, &b) in (0..).zip(unsafe { &*leaked }.iter()) {
            if b == 0 && i < count {
                unsafe {
                    *storage.add(i) = leaked.add(off) as *mut u8;
                }
                i += 1;
            }
        }
        st.io_args = storage;
        storage
    }
}

fn io_argv_count() -> usize {
    let st = thread_state();
    if st.io_args_len != 0 {
        return st.io_args_len as usize;
    }
    #[cfg(target_os = "macos")]
    let count = unsafe { *_NSGetArgc() as usize };
    #[cfg(not(target_os = "macos"))]
    let count = {
        let argv = io_argv_vec();
        if argv.is_null() {
            return 0;
        }
        let mut count = 0;
        while !unsafe { *argv.add(count) }.is_null() {
            count += 1;
        }
        count
    };
    st.io_args_len = count as u64;
    count
}

/// Number of program arguments, excluding the program name.
#[unsafe(no_mangle)]
pub extern "C" fn toy_io_argc() -> u64 {
    let total = io_argv_count();
    (total.saturating_sub(1)) as u64
}

/// The `i`-th program argument (0-based, after the program name);
/// `""` out of range.
#[unsafe(no_mangle)]
pub extern "C" fn toy_io_arg(i: u64) -> *const u8 {
    let argv = io_argv_vec();
    let total = io_argv_count();
    if !argv.is_null() && total > 0 && (i + 1) < total as u64 {
        let s = unsafe { *argv.add((i + 1) as usize) };
        if s.is_null() {
            return toy_str_alloc(&[]);
        }
        let len = unsafe { strlen(s) };
        let bytes = unsafe { core::slice::from_raw_parts(s, len) };
        return toy_str_alloc(bytes);
    }
    toy_str_alloc(&[])
}

/// The value of the environment variable named by the toylang str
/// `name`; `""` when unset.
#[unsafe(no_mangle)]
pub extern "C" fn toy_io_env(name: *const u8) -> *const u8 {
    let key = str_to_cstring(name);
    let v = unsafe { getenv(key.as_ptr()) };
    if v.is_null() {
        return toy_str_alloc(&[]);
    }
    let len = unsafe { strlen(v) };
    let bytes = unsafe { core::slice::from_raw_parts(v, len) };
    toy_str_alloc(bytes)
}

/// The contents of the file at the toylang str `path`; `""` when it
/// cannot be read.
#[unsafe(no_mangle)]
pub extern "C" fn toy_io_read_file(path: *const u8) -> *const u8 {
    let p = str_to_cstring(path);
    let f = unsafe { fopen(p.as_ptr(), c"rb".as_ptr().cast()) };
    if f.is_null() {
        return toy_str_alloc(&[]);
    }
    if unsafe { fseek(f, 0, SEEK_END) } != 0 {
        unsafe { fclose(f) };
        return toy_str_alloc(&[]);
    }
    let n = unsafe { ftell(f) };
    if n < 0 {
        unsafe { fclose(f) };
        return toy_str_alloc(&[]);
    }
    unsafe { rewind(f) };
    let mut buf = vec![0u8; n.max(1) as usize];
    let got = if n > 0 {
        unsafe { fread(buf.as_mut_ptr(), 1, n as usize, f) }
    } else {
        0
    };
    unsafe { fclose(f) };
    buf.truncate(got);
    toy_str_alloc(&buf)
}

/// Whether the file at the toylang str `path` exists.
#[unsafe(no_mangle)]
pub extern "C" fn toy_io_file_exists(path: *const u8) -> u8 {
    let p = str_to_cstring(path);
    let exists = unsafe { access(p.as_ptr(), F_OK) } == 0;
    exists as u8
}

/// A pseudo-random u64. xorshift64* seeded from the clock and the
/// process id on first use — or from an explicit `toy_io_random_seed`
/// call, which makes the sequence reproducible (and therefore
/// testable across backends). Deliberately not reproducible when
/// never seeded.
#[unsafe(no_mangle)]
pub extern "C" fn toy_io_random() -> u64 {
    let st = thread_state();
    if !st.random_seeded {
        st.random_state = ((unsafe { time(core::ptr::null_mut()) } as u64) << 32)
            ^ (unsafe { getpid() } as u64);
        if st.random_state == 0 {
            st.random_state = 0x9E3779B97F4A7C15;
        }
        st.random_seeded = true;
    }
    let mut x = st.random_state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    st.random_state = x;
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

/// Re-seed the `toy_io_random` generator. `seed == 0` is honoured
/// literally (the sequence stays at 0) rather than re-derived, so
/// `random_seed(0)` is deterministic too.
#[unsafe(no_mangle)]
pub extern "C" fn toy_io_random_seed(seed: u64) {
    let st = thread_state();
    st.random_state = seed;
    st.random_seeded = true;
}

/// Format Unix epoch seconds as a UTC date/time string. A documented
/// subset of C `strftime` specifiers, implemented in pure Rust so the
/// interpreter, the JIT and AOT binaries share one implementation
/// (byte-identical output). Unknown specifiers pass through literally
/// (`%q` → `%q`), matching libc.
///
/// Supported: `%% %a %A %b %B %C %d %D %e %F %H %I %j %m %M %n %p %R
/// %S %s %t %T %u %w %y %Y %z %Z`. `%z` is always `+0000` and `%Z`
/// always `UTC` — the conversion is deliberately UTC, never local
/// time, so a fixed timestamp formats identically regardless of the
/// host timezone.
pub fn strftime_utc(fmt: &str, secs: i64) -> String {
    const DAY_ABBR: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const DAY_FULL: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    const MON_ABBR: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const MON_FULL: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];

    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = (secs_of_day / 3600) as u32;
    let mm = ((secs_of_day % 3600) / 60) as u32;
    let ss = (secs_of_day % 60) as u32;
    // 1970-01-01 was a Thursday. days=0 → 0=Sunday.
    let wd = days.rem_euclid(7) + 4;
    // `%u`: Monday=1 .. Sunday=7.
    let wd_mon = days.rem_euclid(7) + 3;
    let doy = day_of_year(y, m, d);

    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(spec) = chars.next() else {
            out.push('%');
            break;
        };
        match spec {
            '%' => out.push('%'),
            'a' => out.push_str(DAY_ABBR[(wd % 7) as usize]),
            'A' => out.push_str(DAY_FULL[(wd % 7) as usize]),
            'b' => out.push_str(MON_ABBR[(m - 1) as usize]),
            'B' => out.push_str(MON_FULL[(m - 1) as usize]),
            'C' => out.push_str(&format!("{:02}", y.div_euclid(100))),
            'd' => out.push_str(&format!("{:02}", d)),
            'D' => out.push_str(&format!("{:02}/{:02}/{:02}", m, d, y.rem_euclid(100))),
            'e' => out.push_str(&format!("{:2}", d)),
            'F' => out.push_str(&format!("{:04}-{:02}-{:02}", y, m, d)),
            'H' => out.push_str(&format!("{:02}", hh)),
            'I' => out.push_str(&format!("{:02}", ((hh + 11) % 12) + 1)),
            'j' => out.push_str(&format!("{:03}", doy)),
            'm' => out.push_str(&format!("{:02}", m)),
            'M' => out.push_str(&format!("{:02}", mm)),
            'n' => out.push('\n'),
            'p' => out.push_str(if hh < 12 { "AM" } else { "PM" }),
            'R' => out.push_str(&format!("{:02}:{:02}", hh, mm)),
            'S' => out.push_str(&format!("{:02}", ss)),
            's' => out.push_str(&format!("{secs}")),
            't' => out.push('\t'),
            'T' => out.push_str(&format!("{:02}:{:02}:{:02}", hh, mm, ss)),
            'u' => out.push_str(&format!("{}", ((wd_mon % 7) + 1))),
            'w' => out.push_str(&format!("{}", wd % 7)),
            'y' => out.push_str(&format!("{:02}", y.rem_euclid(100))),
            'Y' => out.push_str(&format!("{:04}", y)),
            'z' => out.push_str("+0000"),
            'Z' => out.push_str("UTC"),
            _ => {
                out.push('%');
                out.push(spec);
            }
        }
    }
    out
}

/// Days since 1970-01-01 of the given proleptic Gregorian date.
/// Howard Hinnant's `days_from_civil`.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m + 9) % 12; // [0, 11]
    let doy = ((153 * mp + 2) / 5 + d - 1) as i64; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// The proleptic Gregorian date of the given days-since-1970-01-01.
/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Day of year (1..=366) for the given date.
fn day_of_year(y: i64, m: u32, d: u32) -> u32 {
    let first = days_from_civil(y, 1, 1);
    let cur = days_from_civil(y, m, d);
    (cur - first + 1) as u32
}

/// The `strftime` extern for `core/std/io.t`: format Unix epoch
/// seconds as a UTC date/time string. `fmt` is a toylang str handle.
#[unsafe(no_mangle)]
pub extern "C" fn toy_io_strftime(fmt: *const u8, secs: u64) -> *const u8 {
    let bytes = if fmt.is_null() { &[][..] } else { str_bytes(fmt) };
    let f = core::str::from_utf8(bytes).unwrap_or("");
    let out = strftime_utc(f, secs as i64);
    toy_str_alloc(out.as_bytes())
}

/// The process environment as a null-terminated array of `name=value`
/// C strings, or null if `environ` is unavailable.
fn io_env_vec() -> *mut *mut u8 {
    unsafe { environ }
}

/// Number of environment variables.
#[unsafe(no_mangle)]
pub extern "C" fn toy_io_env_count() -> u64 {
    let mut env = io_env_vec();
    let mut n = 0u64;
    while !env.is_null() && !unsafe { *env }.is_null() {
        n += 1;
        env = unsafe { env.add(1) };
    }
    n
}

/// The name (before `=`) of the `i`-th environment variable;
/// `""` out of range.
#[unsafe(no_mangle)]
pub extern "C" fn toy_io_env_name(i: u64) -> *const u8 {
    io_env_entry(i)
        .and_then(|bytes| {
            bytes
                .iter()
                .position(|&b| b == b'=')
                .map(|eq| toy_str_alloc(&bytes[..eq]))
        })
        .unwrap_or_else(|| toy_str_alloc(&[]))
}

/// The value (after `=`) of the `i`-th environment variable;
/// `""` out of range or when the entry has no `=`.
#[unsafe(no_mangle)]
pub extern "C" fn toy_io_env_value(i: u64) -> *const u8 {
    io_env_entry(i)
        .and_then(|bytes| {
            bytes
                .iter()
                .position(|&b| b == b'=')
                .map(|eq| toy_str_alloc(&bytes[eq + 1..]))
        })
        .unwrap_or_else(|| toy_str_alloc(&[]))
}

/// The raw bytes of the `i`-th environment entry (`name=value`).
fn io_env_entry(i: u64) -> Option<&'static [u8]> {
    let mut env = io_env_vec();
    let mut k = 0u64;
    while !env.is_null() && !unsafe { *env }.is_null() {
        if k == i {
            let s = unsafe { *env };
            let len = unsafe { strlen(s) };
            return Some(unsafe { core::slice::from_raw_parts(s, len) });
        }
        k += 1;
        env = unsafe { env.add(1) };
    }
    None
}

/// Program arguments for the JIT's `toy_io_argc` / `toy_io_arg`.
/// In-process JIT runs do not have their own process argv (the
/// compiler's own argv is not the program's), so the harness injects
/// them here; the default is empty, matching a compiled binary
/// launched with no arguments. The AOT binary never calls this — it
/// reads the real argv.
///
/// Arguments are stored as NUL-terminated C strings (the same shape
/// the real argv has), so `toy_io_arg`'s `strlen`-based reader works
/// for both sources.
pub fn set_program_args(args: Vec<Vec<u8>>) {
    let st = thread_state();
    // Build an argv array: [prog_name?][args...]. The AOT convention
    // excludes the program name (argc = argv.len() - 1), so there is
    // no program-name entry here and `toy_io_argc` reads the count
    // minus one — which is why `io_argv_count` returns args.len() + 1
    // below.
    let total = args.len() + 1;
    let storage = unsafe { calloc(total, core::mem::size_of::<*const u8>()) } as *mut *const u8;
    if storage.is_null() {
        return;
    }
    for (i, arg) in args.iter().enumerate() {
        let cstr = unsafe { malloc(arg.len() + 1) };
        if !cstr.is_null() {
            if !arg.is_empty() {
                unsafe {
                    memcpy(cstr, arg.as_ptr(), arg.len());
                }
            }
            unsafe {
                *cstr.add(arg.len()) = 0;
            }
        }
        unsafe {
            *storage.add(i + 1) = cstr;
        }
    }
    st.io_args = storage;
    st.io_args_len = total as u64;
}

// ---------------------------------------------------------------------------
// staticlib-only glue (`--cfg toylang_rt_standalone`, set by
// `compiler/build.rs`). The rlib build (used by the compiler crate for
// the JIT) inherits the host binary's handlers instead.
// ---------------------------------------------------------------------------

#[cfg(toylang_rt_standalone)]
struct MallocAlloc;

#[cfg(toylang_rt_standalone)]
unsafe impl alloc::alloc::GlobalAlloc for MallocAlloc {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        unsafe { malloc(layout.size()) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: core::alloc::Layout) {
        unsafe { free(ptr) }
    }
    unsafe fn realloc(
        &self,
        ptr: *mut u8,
        _layout: core::alloc::Layout,
        new_size: usize,
    ) -> *mut u8 {
        unsafe { realloc(ptr, new_size) }
    }
}

#[cfg(toylang_rt_standalone)]
#[global_allocator]
static GLOBAL: MallocAlloc = MallocAlloc;

#[cfg(toylang_rt_standalone)]
#[panic_handler]
fn rt_panic(_: &core::panic::PanicInfo) -> ! {
    // A panic in the runtime is a codegen/runtime bug; the C runtime
    // would have aborted via exit(1). Loop to satisfy `!`.
    loop {
        unsafe { exit(1) };
    }
}

/// The sysroot `alloc` rlib is built unwind-aware; `-C panic=abort`
/// still references the personality routine, so provide a stub.
#[cfg(toylang_rt_standalone)]
#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality() {}

// ---------------------------------------------------------------------------
// Unit tests (the crate builds with std under `cargo test`).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::cell::RefCell;

    std::thread_local! {
        static CAPTURE: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }

    extern "C" fn capture_sink(bytes: *const u8, len: usize) {
        CAPTURE.with(|c| {
            if len > 0 && !bytes.is_null() {
                c.borrow_mut().extend_from_slice(unsafe {
                    core::slice::from_raw_parts(bytes, len)
                });
            }
        });
    }

    fn with_capture(f: impl FnOnce()) -> Vec<u8> {
        CAPTURE.with(|c| c.borrow_mut().clear());
        set_sink(Some(capture_sink));
        f();
        set_sink(None);
        CAPTURE.with(|c| c.borrow().clone())
    }

    #[test]
    fn print_helpers_format_like_the_interpreter() {
        assert_eq!(with_capture(|| toy_print_i64(-7)), b"-7".to_vec());
        assert_eq!(with_capture(|| toy_println_u64(42)), b"42\n".to_vec());
        assert_eq!(with_capture(|| toy_print_bool(0)), b"false".to_vec());
        assert_eq!(with_capture(|| toy_println_bool(1)), b"true\n".to_vec());
        assert_eq!(with_capture(|| toy_print_i8(-1)), b"-1".to_vec());
        assert_eq!(with_capture(|| toy_print_u32(4294967295)), b"4294967295".to_vec());
    }

    #[test]
    fn f64_display_uses_rust_display_with_trailing_zero_for_integrals() {
        assert_eq!(with_capture(|| toy_print_f64(1.0)), b"1.0".to_vec());
        assert_eq!(with_capture(|| toy_print_f64(-2.5)), b"-2.5".to_vec());
        assert_eq!(with_capture(|| toy_print_f64(0.1 + 0.2)), b"0.30000000000000004".to_vec());
        assert_eq!(with_capture(|| toy_println_f64(1234567.75)), b"1234567.75\n".to_vec());
    }

    #[test]
    fn str_layout_round_trips() {
        let s = toy_str_alloc(b"hello");
        assert_eq!(unsafe { (s as *const u64).read_unaligned() }, 5);
        assert_eq!(str_bytes(s), b"hello");
        let t = toy_str_alloc(b"hello");
        assert_eq!(unsafe { toy_str_eq(s, t) }, 1);
        assert_eq!(unsafe { toy_str_eq(s, toy_str_alloc(b"world")) }, 0);
        let c = unsafe { toy_str_concat(s, t) };
        assert_eq!(str_bytes(c), b"hellohello");
    }

    #[test]
    fn to_string_variants_cover_primitive_widths() {
        assert_eq!(str_bytes(toy_to_string_i64(-5)), b"-5");
        assert_eq!(str_bytes(toy_to_string_u8(255)), b"255");
        assert_eq!(str_bytes(toy_to_string_f64(3.5)), b"3.5");
        assert_eq!(str_bytes(toy_to_string_bool(1)), b"true");
        let s = toy_str_alloc(b"id");
        assert_eq!(toy_to_string_str(s), s);
    }

    /// STR-INTERP-FMT: the packed spec is produced by
    /// `frontend::format_spec::FormatSpec::pack` and decoded here by
    /// hand, so the two bit layouts have to stay in step. The
    /// constants below are the ones `packed_bits_are_stable` pins on
    /// the frontend side; if either moves, one of the two tests
    /// fails instead of the mismatch showing up as wrong output in a
    /// compiled program.
    #[test]
    fn format_spec_bits_match_runtime() {
        // "08.3" — width 8, precision 3 (stored +1), zero-pad.
        let code = 8 | (4u64 << 16) | (1u64 << 26);
        assert_eq!(str_bytes(toy_format_f64(1.5, code)), b"0001.500");
        // "^10x" — width 10, center-aligned, hex.
        let code = 10 | (3u64 << 24) | (1u64 << 27);
        assert_eq!(str_bytes(toy_format_u64(255, code, 64)), b"    ff    ");
        // A negative value in a non-decimal radix masks to its own
        // width, so an i32 shows 8 digits rather than 16.
        let code = 1u64 << 27;
        assert_eq!(str_bytes(toy_format_i64(-1, code, 32)), b"ffffffff");
        assert_eq!(str_bytes(toy_format_i64(-1, code, 64)), b"ffffffffffffffff");
        // An all-zero spec is the default rendering.
        assert_eq!(str_bytes(toy_format_bool(1, 0)), b"true");
        let s = toy_str_alloc(b"hi");
        assert_eq!(str_bytes(toy_format_str(s, 6)), b"hi    ");
    }

    #[test]
    fn dispatched_alloc_free_is_idempotent() {
        profiler_reset();
        let p = toy_dispatched_alloc(0, 64, 1 << 32);
        assert!(!p.is_null());
        let q = toy_dispatched_alloc(0, 32, 2 << 32);
        assert!(!q.is_null());
        toy_dispatched_free(0, p);
        toy_dispatched_free(0, p); // double free is a no-op
        toy_dispatched_free(0, q);
        let stats = profiler_stats();
        assert_eq!(stats.alloc_count, 2);
        assert_eq!(stats.free_count, 2);
        // The bump region never reuses a freed address.
        let r = toy_dispatched_alloc(0, 64, 3 << 32);
        assert_ne!(r, p);
    }

    #[test]
    fn realloc_accounts_as_one_resize() {
        profiler_reset();
        let p = toy_dispatched_alloc(0, 64, 1 << 32);
        let r = unsafe { toy_dispatched_realloc(0, p, 160) };
        assert!(!r.is_null());
        let stats = profiler_stats();
        assert_eq!(stats.realloc_count, 1);
        assert_eq!(stats.alloc_count, 1);
        assert_eq!(stats.cumulative_bytes, 64 + 96);
    }

    #[test]
    fn str_from_bytes_copies() {
        let mut bytes = b"abc".to_vec();
        let s = unsafe { toy_str_from_bytes(bytes.as_ptr(), 3) };
        bytes[0] = b'x';
        assert_eq!(str_bytes(s), b"abc");
        // Null / zero-length requests yield empty strs.
        let a = unsafe { toy_str_from_bytes(core::ptr::null(), 0) };
        let b = unsafe { toy_str_from_bytes(core::ptr::null(), 0) };
        assert_eq!(str_bytes(a), b"");
        assert_eq!(str_bytes(b), b"");
    }

    #[test]
    fn io_args_round_trip() {
        set_program_args(vec![b"one".to_vec(), b"two".to_vec()]);
        assert_eq!(toy_io_argc(), 2);
        assert_eq!(str_bytes(toy_io_arg(0)), b"one");
        assert_eq!(str_bytes(toy_io_arg(1)), b"two");
        assert_eq!(str_bytes(toy_io_arg(2)), b"");
    }

    #[test]
    fn strftime_utc_matches_reference_values() {
        // 0 = 1970-01-01 00:00:00 UTC (Thursday).
        assert_eq!(strftime_utc("%Y-%m-%d %H:%M:%S", 0), "1970-01-01 00:00:00");
        assert_eq!(strftime_utc("%F %T", 1_700_000_000), "2023-11-14 22:13:20");
        assert_eq!(
            strftime_utc("%a %A %b %B %Y", 1_700_000_000),
            "Tue Tuesday Nov November 2023"
        );
        assert_eq!(strftime_utc("%s", 1_700_000_000), "1700000000");
        assert_eq!(strftime_utc("%j %u %w", 1_700_000_000), "318 2 2");
        assert_eq!(strftime_utc("%H:%M %I %p", 1_700_000_000), "22:13 10 PM");
        assert_eq!(strftime_utc("%z %Z %C%y", 1_700_000_000), "+0000 UTC 2023");
        // Unknown specifiers pass through literally (libc behaviour).
        assert_eq!(strftime_utc("%q", 0), "%q");
        assert_eq!(strftime_utc("100%%", 0), "100%");
        assert_eq!(strftime_utc("plain text", 0), "plain text");
    }

    #[test]
    fn seeded_random_is_reproducible() {
        toy_io_random_seed(42);
        let a = toy_io_random();
        let b = toy_io_random();
        toy_io_random_seed(42);
        let a2 = toy_io_random();
        let b2 = toy_io_random();
        assert_eq!((a, b), (a2, b2), "same seed must reproduce the sequence");
        // An explicit zero seed stays at zero rather than being
        // re-derived from the clock.
        toy_io_random_seed(0);
        assert_eq!(toy_io_random(), 0);
    }

    #[test]
    fn env_iteration_round_trips_through_the_entries() {
        // The test process env is visible through `environ`; a
        // variable we control is guaranteed present regardless of the
        // runner's environment.
        // Safety (edition 2024): the mutation is confined to this
        // single test and removed before it returns.
        unsafe { std::env::set_var("TOYLANG_RT_ENV_TEST", "hello=world") };
        let n = toy_io_env_count();
        assert!(n > 0);
        let mut found = false;
        let mut i = 0u64;
        while i < n {
            if str_bytes(toy_io_env_name(i)) == b"TOYLANG_RT_ENV_TEST"
                && str_bytes(toy_io_env_value(i)) == b"hello=world"
            {
                found = true;
            }
            i += 1;
        }
        unsafe { std::env::remove_var("TOYLANG_RT_ENV_TEST") };
        assert!(found, "the controlled variable must appear in the env list");
        assert_eq!(str_bytes(toy_io_env_name(n + 5)), b"");
    }
}
