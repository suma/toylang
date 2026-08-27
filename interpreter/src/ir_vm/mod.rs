//! Interpreter-facing entry points for the IR VM.
//!
//! The VM itself lives in the `compiler_vm` crate (COMPILE-TIME-EVAL
//! C6); this module wires it to the interpreter's world — heap
//! manager, allocator stack, output sink — through
//! [`host::InterpreterHost`], and keeps the historical entry points
//! installing a fresh runtime state for each run, exactly as the
//! pre-C6 VM did for itself. The compile-time fold
//! (`crate::const_eval`) calls `compiler_vm` directly with the same
//! host, which is what makes the two paths share one engine.

pub use compiler_vm::{eligibility, frame, run_module, slot, Vm, VmResult};

pub mod host;
pub mod lift;

use compiler_ir::{FuncId, Module};
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::runtime_state::{RT, RuntimeState};
use slot::RawSlot;

/// Run `f` with a fresh runtime state installed (heap manager +
/// allocator stack), tearing it down afterwards. Every VM run needs a
/// state to reach for; the tree-walker keeps its own heap in its
/// context, so only this path installs one.
fn with_runtime_state<R>(f: impl FnOnce() -> R) -> R {
    RT.with(|s| *s.borrow_mut() = Some(RuntimeState::new()));
    let result = f();
    RT.with(|s| *s.borrow_mut() = None);
    result
}

/// Run a lowered IR module, returning the exit code.
pub fn run_module_with_interner(
    module: &Module,
    interner: Option<&DefaultStringInterner>,
) -> Result<i64, String> {
    with_runtime_state(|| compiler_vm::run_module_with_interner(module, interner, &host::InterpreterHost))
}

/// Like [`run_module_with_interner`] but, when `want_str` is set, also
/// reads the `main` exit value as a `str`. Also returns the full flat
/// leaf list so compound `main` returns can be reconstructed.
/// As [`run_module_capturing`], handing back the failure in parts so
/// the driver can both render it and serialise it.
pub fn run_module_capturing_reporting(
    module: &Module,
    interner: Option<&DefaultStringInterner>,
    want_str: bool,
) -> Result<(i64, String, Vec<RawSlot>), compiler_vm::Divergence> {
    with_runtime_state(|| {
        compiler_vm::run_module_capturing_reporting(
            module,
            interner,
            &host::InterpreterHost,
            want_str,
        )
    })
}

pub fn run_module_capturing(
    module: &Module,
    interner: Option<&DefaultStringInterner>,
    want_str: bool,
) -> Result<(i64, String, Vec<RawSlot>), String> {
    with_runtime_state(|| {
        compiler_vm::run_module_capturing(module, interner, &host::InterpreterHost, want_str)
    })
}

/// Run one function with pre-evaluated argument slots and return its
/// return slots (COMPILE-TIME-EVAL C6). `budget` caps loop
/// back-edges; the fold passes one so a non-terminating `const`
/// initialiser stops the compile instead of hanging it.
pub fn run_function(
    module: &Module,
    interner: &DefaultStringInterner,
    func_id: FuncId,
    args: Vec<RawSlot>,
    budget: Option<u64>,
) -> Result<Vec<RawSlot>, String> {
    with_runtime_state(|| {
        compiler_vm::run_function(module, Some(interner), &host::InterpreterHost, func_id, args, budget)
    })
}

/// Host-bound helpers for interpreter-side callers: a thin layer over
/// `compiler_vm`'s host-parameterised helpers, bound to
/// [`host::InterpreterHost`] so interpreter code can call them without
/// threading the host around.
pub mod heap {
    use compiler_vm::host::VmHost;

    pub fn read_str(value: u64) -> String {
        VmHost::read_str(&super::host::InterpreterHost, value)
    }
}

/// The symbol the interpreter's own code uses to name a `const`
/// wrapper function in the CTFE fold's lowering. Kept here (rather
/// than in `const_eval`) so the name-shape lives next to the VM's
/// entry points.
pub(crate) fn const_wrapper_symbol(interner: &mut DefaultStringInterner, index: usize) -> DefaultSymbol {
    interner.get_or_intern(format!("__ctfe_wrapper_{index}"))
}