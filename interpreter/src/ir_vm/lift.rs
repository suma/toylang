//! AST → IR lifting bridge + opt-in IR VM execution entry point.
//!
//! The lowering itself lives in the `compiler_lower` crate (shared by the
//! `compiler` and `interpreter` crates so neither needs to depend on the
//! other). This module wires it to the IR VM so the interpreter can run a
//! type-checked program through the VM instead of the tree-walker.
//!
//! Activated per-process by `TOY_IR_VM=1`. When unset (the default) the
//! interpreter keeps using the tree-walker unchanged. When set,
//! `try_execute_main` runs the program through the IR VM and returns the
//! `main` result; if the program does not lower, is ineligible, diverges,
//! or returns a non-scalar (heap-backed) value, it returns `None` so the
//! caller transparently falls back to the tree-walker.

use std::cell::RefCell;
use std::rc::Rc;

use frontend::ast::{File, Function};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultStringInterner;

use crate::object::{Object, RcObject};

/// `true` when the IR VM execution path is enabled via `TOY_IR_VM`.
pub fn ir_vm_enabled_via_env() -> bool {
    std::env::var("TOY_IR_VM")
        .map(|v| v != "0" && !v.is_empty())
        .unwrap_or(false)
}

/// Emit a one-line coverage marker (`IRVM_TRACE <category>`) to stderr when
/// `TOY_IR_VM_TRACE` is set. Used to quantify how often `run_main_via_ir_vm`
/// actually runs vs. falls back (and why) across a test corpus.
fn trace(category: &str) {
    if std::env::var_os("TOY_IR_VM_TRACE").is_some() {
        eprintln!("IRVM_TRACE {category}");
    }
}

/// Attempt to run `program`'s `main` through the IR VM, returning the
/// wrapped scalar result. Returns `None` (→ tree-walker fallback) when the
/// path is disabled, `main` returns a non-scalar / heap-backed value, the
/// program fails to lower, the module is ineligible, or execution diverges.
pub fn try_execute_main(
    program: &File,
    interner: &DefaultStringInterner,
) -> Option<RcObject> {
    if !ir_vm_enabled_via_env() {
        return None;
    }
    run_main_via_ir_vm(program, interner)
}

/// Env-independent core of [`try_execute_main`]. Exposed so tests can drive
/// the IR VM path deterministically (and compare it against the tree-walker)
/// without toggling a process-global env var.
pub fn run_main_via_ir_vm(
    program: &File,
    interner: &DefaultStringInterner,
) -> Option<RcObject> {
    let main_fn = crate::find_main_function(program, interner).ok()?;
    // Scalar (non-heap) and `str` `main` returns can be reconstructed after
    // the VM's RuntimeState teardown (`str` bytes are captured first). Other
    // compound returns (struct / tuple / enum) bail to the tree-walker.
    let returns_str = matches!(main_fn.return_type, Some(TypeDecl::String));
    if !is_scalar_return(&main_fn.return_type) && !returns_str {
        trace("fb_nonscalar_return");
        return None;
    }

    // Lowering needs a mutable interner to intern the contract-violation
    // messages. Clone so the caller's interner stays immutable; the clone
    // carries every symbol the program already references plus the two new
    // ones, and is handed to the VM for panic / print symbol resolution.
    let mut interner_owned = interner.clone();
    let contract_msgs = compiler_lower::ContractMessages::intern(&mut interner_owned);
    let module = match compiler_lower::lower_program(program, &interner_owned, &contract_msgs, false)
    {
        Ok(m) => m,
        Err(_) => {
            trace("fb_lower_err");
            return None;
        }
    };
    if !super::eligibility::ir_vm_supported(&module) {
        trace("fb_ineligible");
        return None;
    }
    let (bits, captured_str) =
        match super::run_module_capturing(&module, Some(&interner_owned), returns_str) {
            Ok(v) => v,
            Err(_) => {
                trace("fb_diverge");
                return None;
            }
        };
    trace("ran");
    if returns_str {
        Some(Rc::new(RefCell::new(Object::String(captured_str))))
    } else {
        Some(wrap_scalar(bits, &main_fn))
    }
}

/// Whether a scalar `main` return type can be faithfully wrapped from the
/// raw 8-byte exit value. Heap-backed / compound types return `false`.
fn is_scalar_return(ty: &Option<TypeDecl>) -> bool {
    matches!(
        ty,
        Some(
            TypeDecl::Unit
                | TypeDecl::Int64
                | TypeDecl::UInt64
                | TypeDecl::Float64
                | TypeDecl::Bool
                | TypeDecl::Int8
                | TypeDecl::Int16
                | TypeDecl::Int32
                | TypeDecl::UInt8
                | TypeDecl::UInt16
                | TypeDecl::UInt32
        )
    )
}

/// Reinterpret the VM's raw i64 exit value as the typed `main` return.
fn wrap_scalar(bits: i64, main_fn: &Rc<Function>) -> RcObject {
    let u = bits as u64;
    let obj = match main_fn.return_type {
        Some(TypeDecl::Int64) => Object::Int64(bits),
        Some(TypeDecl::UInt64) => Object::UInt64(u),
        Some(TypeDecl::Float64) => Object::Float64(f64::from_bits(u)),
        Some(TypeDecl::Bool) => Object::Bool(u != 0),
        Some(TypeDecl::Int8) => Object::Int8(u as u8 as i8),
        Some(TypeDecl::Int16) => Object::Int16(u as u16 as i16),
        Some(TypeDecl::Int32) => Object::Int32(u as u32 as i32),
        Some(TypeDecl::UInt8) => Object::UInt8(u as u8),
        Some(TypeDecl::UInt16) => Object::UInt16(u as u16),
        Some(TypeDecl::UInt32) => Object::UInt32(u as u32),
        _ => Object::Unit,
    };
    Rc::new(RefCell::new(obj))
}
