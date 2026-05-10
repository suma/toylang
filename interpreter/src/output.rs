//! Captured-stdout abstraction for the `print` / `println` builtins.
//!
//! All textual output the interpreter emits — both the tree-walker's
//! `BuiltinFunction::Print/Println` and the JIT's `jit_print_*` /
//! `jit_println_*` helpers — funnels through `print_text` /
//! `println_text` here. By default that writes to the process stdout,
//! matching the previous `print!` / `println!` calls byte-for-byte.
//!
//! Tests can install a thread-local capture buffer via [`with_capture`]
//! to collect the output of an in-process run instead of redirecting at
//! the OS level (which would race with parallel test threads). The
//! capture is scoped to the current thread, so concurrent JIT and
//! tree-walker runs in different threads don't contaminate each other.

use std::cell::RefCell;
use std::io::Write;

thread_local! {
    static OUTPUT_SINK: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
    /// Per-thread stderr sink. Symmetric to `OUTPUT_SINK` but for the
    /// `eprintln!`-style helpers (JIT compile log, type-check
    /// diagnostics, panic messages from JIT-emitted code). Used by
    /// in-process integration tests that want to assert on stderr
    /// content without spawning the interpreter binary.
    static ERROR_SINK: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

/// Run `f` with stdout captured into a string. Restores the previous
/// capture state (which is normally `None` = "write to real stdout") on
/// return, even if `f` panics.
pub fn with_capture<R>(f: impl FnOnce() -> R) -> (R, String) {
    let prev = OUTPUT_SINK.with(|cell| cell.replace(Some(Vec::new())));
    struct Guard(Option<Vec<u8>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            let prev = self.0.take();
            OUTPUT_SINK.with(|cell| {
                *cell.borrow_mut() = prev;
            });
        }
    }
    let _guard = Guard(prev);
    let result = f();
    let captured = OUTPUT_SINK.with(|cell| cell.borrow().clone()).unwrap_or_default();
    let captured_str = String::from_utf8_lossy(&captured).into_owned();
    (result, captured_str)
}

/// Append `s` to the active capture sink, or write it to the process
/// stdout when no capture is active. Mirrors `print!` semantics.
pub fn print_text(s: &str) {
    OUTPUT_SINK.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(buf) = slot.as_mut() {
            buf.extend_from_slice(s.as_bytes());
        } else {
            // Drop the captured borrow before touching real stdout so
            // a `print!` inside the eventual `Stdout` impl can't
            // re-enter this thread-local.
            drop(slot);
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            let _ = handle.write_all(s.as_bytes());
        }
    });
}

/// Append `s` and a trailing newline. Mirrors `println!` semantics.
pub fn println_text(s: &str) {
    print_text(s);
    print_text("\n");
}

/// Run `f` with stderr captured into a string. Companion to
/// [`with_capture`] — separate sink so callers can assert on stdout
/// and stderr independently.
pub fn with_stderr_capture<R>(f: impl FnOnce() -> R) -> (R, String) {
    let prev = ERROR_SINK.with(|cell| cell.replace(Some(Vec::new())));
    struct Guard(Option<Vec<u8>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            let prev = self.0.take();
            ERROR_SINK.with(|cell| {
                *cell.borrow_mut() = prev;
            });
        }
    }
    let _guard = Guard(prev);
    let result = f();
    let captured = ERROR_SINK
        .with(|cell| cell.borrow().clone())
        .unwrap_or_default();
    let captured_str = String::from_utf8_lossy(&captured).into_owned();
    (result, captured_str)
}

/// Run `f` with both stdout and stderr captured into separate
/// strings. Convenience wrapper for callers that want both at once.
pub fn with_stdout_stderr_capture<R>(f: impl FnOnce() -> R) -> (R, String, String) {
    let (result, captured_stdout) = with_capture(|| {
        let (r, captured_stderr) = with_stderr_capture(f);
        (r, captured_stderr)
    });
    let (r, captured_stderr) = result;
    (r, captured_stdout, captured_stderr)
}

/// Append `s` to the active stderr capture sink, or write it to the
/// process stderr when no capture is active. Mirrors `eprint!`
/// semantics.
pub fn eprint_text(s: &str) {
    ERROR_SINK.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(buf) = slot.as_mut() {
            buf.extend_from_slice(s.as_bytes());
        } else {
            drop(slot);
            let stderr = std::io::stderr();
            let mut handle = stderr.lock();
            let _ = handle.write_all(s.as_bytes());
        }
    });
}

/// Append `s` and a trailing newline to the active stderr sink.
pub fn eprintln_text(s: &str) {
    eprint_text(s);
    eprint_text("\n");
}
