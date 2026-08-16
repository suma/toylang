//! FFI_PLAN P1-MVP-B: dispatch `extern fn ... from "lib"` calls on the
//! interpreter.
//!
//! The interpreter's *own* io externs (`extern_io.rs`) keep their
//! Rust-side implementations — the registry is consulted first by the
//! caller (`evaluation/call.rs::dispatch_extern_fn`) — and everything
//! else declared `from` is resolved here: the library is dlopen'ed by
//! name and the symbol is looked up and called through a trampoline.
//!
//! ## Library resolution
//!
//! The `from "lib"` name is the `-l` name (no `lib` prefix /
//! extension). The AOT linker gets `-l<lib>`; here we search
//! `TOYLANG_LINK_PATHS` (colon-separated, the same variable the
//! driver reads for `-L`) followed by the default loader paths, for
//! `lib<lib>.dylib` on macOS / `lib<lib>.so` on Linux. The special
//! name `"c"` maps to the already-linked libc via the process handle.
//! Loaded libraries are cached per thread for the process lifetime.
//!
//! ## Trampoline
//!
//! dlsym hands back a raw address; calling it with a signature only
//! known at runtime needs a concrete function pointer type, and
//! integer-class args (i64/u64/bool/ptr/narrow ints) travel in
//! different registers than f64 args — a plain `transmute` to one
//! signature cannot serve both. FFI_PLAN 論点2 (a): enumerate the
//! arg-class patterns (2^4 × arity 0..=4) and the return class
//! (int / f64 / void) as explicit function-pointer types. Arity 5+
//! is a clean error (P1 constraint).

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use frontend::type_decl::TypeDecl;
use libloading::os::unix::{Library, RTLD_LAZY};


use crate::error::InterpreterError;
use crate::object::Object;
use crate::value::Value;

thread_local! {
    /// Resolved `from "lib"` libraries, keyed by the declaration's lib
    /// name. Kept per thread so parallel test workers don't fight over
    /// a single handle table. `BTreeMap` because its `new` is const
    /// (`HashMap::new` is not, and a `thread_local!` initialiser must
    /// be).
    static FFI_LIBS: RefCell<BTreeMap<String, Rc<Library>>> =
        const { RefCell::new(BTreeMap::new()) };
}

/// The candidate file names for `-l`-style lib `name`, in search
/// order: every `TOYLANG_LINK_PATHS` directory first, then the bare
/// name (the loader's own search path).
fn lib_candidates(name: &str) -> Vec<std::ffi::OsString> {
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let file = format!("lib{name}.{ext}");
    let mut out: Vec<std::ffi::OsString> = Vec::new();
    if let Some(paths) = std::env::var_os("TOYLANG_LINK_PATHS") {
        for dir in std::env::split_paths(&paths) {
            out.push(dir.join(&file).into_os_string());
        }
    }
    out.push(file.into());
    out
}

/// Resolve `from "lib"` to a dlopen'ed handle, cached per thread.
pub fn resolve_lib(name: &str) -> Result<Rc<Library>, InterpreterError> {
    FFI_LIBS.with(|m| {
        if let Some(lib) = m.borrow().get(name) {
            return Ok(lib.clone());
        }
        let lib = load_lib(name)?;
        m.borrow_mut().insert(name.to_string(), lib.clone());
        Ok(lib)
    })
}

fn load_lib(name: &str) -> Result<Rc<Library>, InterpreterError> {
    if name == "c" {
        // libc is already linked into the interpreter process; the
        // process handle (`dlopen(NULL)`) resolves its symbols without
        // loading a file.
        let lib = unsafe {
            Library::open(None::<&std::ffi::OsStr>, RTLD_LAZY).map_err(|e| {
                InterpreterError::InternalError(format!("FFI: dlopen(NULL): {e}"))
            })?
        };
        return Ok(Rc::new(lib));
    }
    let mut last_err: Option<libloading::Error> = None;
    for candidate in lib_candidates(name) {
        match unsafe { Library::new(&candidate) } {
            Ok(lib) => return Ok(Rc::new(lib)),
            Err(e) => last_err = Some(e),
        }
    }
    Err(InterpreterError::InternalError(format!(
        "FFI: could not load library `{name}` (tried {}): {}",
        lib_candidates(name)
            .iter()
            .map(|c| c.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", "),
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "no candidate".to_string())
    )))
}

/// The register class a declared type travels in.
enum Class {
    /// Integer-class register (ints of every width, bool, ptr).
    Int,
    /// Floating-point register.
    Float,
}

fn declared_class(ty: &TypeDecl) -> Option<Class> {
    match ty {
        TypeDecl::Bool
        | TypeDecl::Int8
        | TypeDecl::Int16
        | TypeDecl::Int32
        | TypeDecl::Int64
        | TypeDecl::UInt8
        | TypeDecl::UInt16
        | TypeDecl::UInt32
        | TypeDecl::UInt64
        | TypeDecl::Ptr => Some(Class::Int),
        TypeDecl::Float64 => Some(Class::Float),
        _ => None,
    }
}

/// Whether the declared return type comes back in an integer register.
fn is_int_return(ty: &TypeDecl) -> bool {
    matches!(declared_class(ty), Some(Class::Int))
}

/// Extract an integer-class argument as the raw register value.
fn value_to_int(value: &Value) -> Result<u64, InterpreterError> {
    match value {
        Value::Int64(v) => Ok(*v as u64),
        Value::UInt64(v) => Ok(*v),
        Value::Bool(b) => Ok(*b as u64),
        Value::Pointer(addr) => Ok(*addr as u64),
        Value::Int8(v) => Ok(*v as u64),
        Value::UInt8(v) => Ok(*v as u64),
        Value::Int16(v) => Ok(*v as u64),
        Value::UInt16(v) => Ok(*v as u64),
        Value::Int32(v) => Ok(*v as u64),
        Value::UInt32(v) => Ok(*v as u64),
        other => Err(InterpreterError::InternalError(format!(
            "FFI: cannot pass {other:?} as an integer-class argument"
        ))),
    }
}

fn value_to_f64(value: &Value) -> Result<f64, InterpreterError> {
    match value {
        Value::Float64(v) => Ok(*v),
        other => Err(InterpreterError::InternalError(format!(
            "FFI: cannot pass {other:?} as an f64 argument"
        ))),
    }
}

/// Shape the raw integer-register result into the declared return
/// type.
fn wrap_int_return(raw: u64, ty: &TypeDecl) -> Value {
    match ty {
        TypeDecl::Bool => Value::Bool(raw != 0),
        TypeDecl::Int8 => Value::Int8(raw as i8),
        TypeDecl::UInt8 => Value::UInt8(raw as u8),
        TypeDecl::Int16 => Value::Int16(raw as i16),
        TypeDecl::UInt16 => Value::UInt16(raw as u16),
        TypeDecl::Int32 => Value::Int32(raw as i32),
        TypeDecl::UInt32 => Value::UInt32(raw as u32),
        TypeDecl::Int64 => Value::Int64(raw as i64),
        TypeDecl::Ptr => Value::Pointer(raw as usize),
        _ => Value::UInt64(raw),
    }
}

/// The result of one trampoline call, before shaping.
enum RawOut {
    Int(u64),
    Float(f64),
    Void,
}

/// Pull one argument from its class's array and advance the counter.
/// Everything arrives as `$`-parameters so the identifiers resolve at
/// the invocation site (macro bodies resolve free identifiers at the
/// *definition* site, which is module scope here).
macro_rules! pick {
    (I, $int_args:expr, $i:expr) => {{
        let v = $int_args[$i.get()];
        $i.set($i.get() + 1);
        v
    }};
    (F, $f64_args:expr, $f:expr) => {{
        let v = $f64_args[$f.get()];
        $f.set($f.get() + 1);
        v
    }};
}

/// Route one `$cls` class token to its array + counter.
macro_rules! pick_arg {
    (I, $int_args:expr, $f64_args:expr, $i:expr, $f:expr) => {
        pick!(I, $int_args, $i)
    };
    (F, $int_args:expr, $f64_args:expr, $i:expr, $f:expr) => {
        pick!(F, $f64_args, $f)
    };
}

/// One call shape: `$cls` is the per-arg register class (`I` / `F`),
/// `$ret` the return class (`I` / `F` / `V`). Expands to a call
/// through a concrete `extern "C" fn` pointer, pulling arguments from
/// the two class-indexed arrays in positional order.
macro_rules! ffi_arm {
    ($fn_ptr:expr, $int_args:expr, $f64_args:expr, [$($cls:ident),*], $ret:ident) => {{
        let i = Cell::new(0usize);
        let f = Cell::new(0usize);
        #[allow(unused_macros)]
        macro_rules! ret_ty {
            (I) => { u64 };
            (F) => { f64 };
            (V) => { () };
        }
        #[allow(unused_macros)]
        macro_rules! arg_ty {
            (I) => { u64 };
            (F) => { f64 };
        }
        unsafe {
            let call: extern "C" fn($(arg_ty!($cls)),*) -> ret_ty!($ret) =
                std::mem::transmute($fn_ptr);
            let out = call($(pick_arg!($cls, $int_args, $f64_args, &i, &f)),*);
            // Every argument must have been consumed exactly once.
            debug_assert_eq!(i.get(), $int_args.len());
            debug_assert_eq!(f.get(), $f64_args.len());
            out
        }
    }};
}

/// Dispatch one `from`-declared call. `params` and `return_type` are
/// the declaration's types (the C side must match — FFI_PLAN 論点 4:
/// signature correctness is the user's responsibility).
pub fn call_extern(
    fn_ptr: usize,
    params: &[(string_interner::DefaultSymbol, TypeDecl)],
    args: &[Value],
    return_type: &TypeDecl,
) -> Result<Value, InterpreterError> {
    if args.len() > 4 {
        return Err(InterpreterError::InternalError(format!(
            "FFI: arity {} is not supported (FFI_PLAN P1 allows up to 4 arguments)",
            args.len()
        )));
    }
    let mut int_args: Vec<u64> = Vec::with_capacity(args.len());
    let mut f64_args: Vec<f64> = Vec::with_capacity(args.len());
    let mut mask = 0usize;
    for (i, arg) in args.iter().enumerate() {
        let declared = params
            .get(i)
            .ok_or_else(|| {
                InterpreterError::InternalError(format!(
                    "FFI: argument {} has no declared parameter type",
                    i
                ))
            })?
            .1
            .clone();
        match declared_class(&declared) {
            Some(Class::Int) => int_args.push(value_to_int(arg)?),
            Some(Class::Float) => {
                f64_args.push(value_to_f64(arg)?);
                mask |= 1 << i;
            }
            None => {
                return Err(InterpreterError::InternalError(format!(
                    "FFI: parameter type {declared:?} cannot cross the C ABI boundary"
                )))
            }
        }
    }
    let ret_class = if *return_type == TypeDecl::Unit {
        2usize // Void
    } else if is_int_return(return_type) {
        0usize // Int
    } else {
        1usize // Float
    };
    let raw = match (args.len(), mask, ret_class) {
        (0, 0, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [], I)),
        (0, 0, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [], F)),
        (0, 0, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [], V);
            RawOut::Void
        }
        (1, 0, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I], I)),
        (1, 1, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F], I)),
        (1, 0, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I], F)),
        (1, 1, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F], F)),
        (1, 0, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I], V);
            RawOut::Void
        }
        (1, 1, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F], V);
            RawOut::Void
        }
        (2, 0, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, I], I)),
        (2, 1, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, I], I)),
        (2, 2, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, F], I)),
        (2, 3, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, F], I)),
        (2, 0, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, I], F)),
        (2, 1, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, I], F)),
        (2, 2, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, F], F)),
        (2, 3, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, F], F)),
        (2, 0, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, I], V);
            RawOut::Void
        }
        (2, 1, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, I], V);
            RawOut::Void
        }
        (2, 2, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, F], V);
            RawOut::Void
        }
        (2, 3, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, F], V);
            RawOut::Void
        }
        (3, 0, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, I, I], I)),
        (3, 1, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, I, I], I)),
        (3, 2, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, F, I], I)),
        (3, 3, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, F, I], I)),
        (3, 4, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, I, F], I)),
        (3, 5, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, I, F], I)),
        (3, 6, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, F, F], I)),
        (3, 7, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, F, F], I)),
        (3, 0, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, I, I], F)),
        (3, 1, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, I, I], F)),
        (3, 2, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, F, I], F)),
        (3, 3, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, F, I], F)),
        (3, 4, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, I, F], F)),
        (3, 5, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, I, F], F)),
        (3, 6, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, F, F], F)),
        (3, 7, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, F, F], F)),
        (3, 0, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, I, I], V);
            RawOut::Void
        }
        (3, 1, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, I, I], V);
            RawOut::Void
        }
        (3, 2, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, F, I], V);
            RawOut::Void
        }
        (3, 3, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, F, I], V);
            RawOut::Void
        }
        (3, 4, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, I, F], V);
            RawOut::Void
        }
        (3, 5, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, I, F], V);
            RawOut::Void
        }
        (3, 6, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, F, F], V);
            RawOut::Void
        }
        (3, 7, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, F, F], V);
            RawOut::Void
        }
        (4, 0, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, I, I, I], I)),
        (4, 1, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, I, I, I], I)),
        (4, 2, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, F, I, I], I)),
        (4, 3, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, F, I, I], I)),
        (4, 4, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, I, F, I], I)),
        (4, 5, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, I, F, I], I)),
        (4, 6, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, F, F, I], I)),
        (4, 7, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, F, F, I], I)),
        (4, 8, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, I, I, F], I)),
        (4, 9, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, I, I, F], I)),
        (4, 10, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, F, I, F], I)),
        (4, 11, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, F, I, F], I)),
        (4, 12, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, I, F, F], I)),
        (4, 13, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, I, F, F], I)),
        (4, 14, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [I, F, F, F], I)),
        (4, 15, 0) => RawOut::Int(ffi_arm!(fn_ptr, int_args, f64_args, [F, F, F, F], I)),
        (4, 0, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, I, I, I], F)),
        (4, 1, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, I, I, I], F)),
        (4, 2, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, F, I, I], F)),
        (4, 3, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, F, I, I], F)),
        (4, 4, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, I, F, I], F)),
        (4, 5, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, I, F, I], F)),
        (4, 6, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, F, F, I], F)),
        (4, 7, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, F, F, I], F)),
        (4, 8, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, I, I, F], F)),
        (4, 9, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, I, I, F], F)),
        (4, 10, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, F, I, F], F)),
        (4, 11, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, F, I, F], F)),
        (4, 12, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, I, F, F], F)),
        (4, 13, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, I, F, F], F)),
        (4, 14, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [I, F, F, F], F)),
        (4, 15, 1) => RawOut::Float(ffi_arm!(fn_ptr, int_args, f64_args, [F, F, F, F], F)),
        (4, 0, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, I, I, I], V);
            RawOut::Void
        }
        (4, 1, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, I, I, I], V);
            RawOut::Void
        }
        (4, 2, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, F, I, I], V);
            RawOut::Void
        }
        (4, 3, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, F, I, I], V);
            RawOut::Void
        }
        (4, 4, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, I, F, I], V);
            RawOut::Void
        }
        (4, 5, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, I, F, I], V);
            RawOut::Void
        }
        (4, 6, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, F, F, I], V);
            RawOut::Void
        }
        (4, 7, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, F, F, I], V);
            RawOut::Void
        }
        (4, 8, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, I, I, F], V);
            RawOut::Void
        }
        (4, 9, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, I, I, F], V);
            RawOut::Void
        }
        (4, 10, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, F, I, F], V);
            RawOut::Void
        }
        (4, 11, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, F, I, F], V);
            RawOut::Void
        }
        (4, 12, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, I, F, F], V);
            RawOut::Void
        }
        (4, 13, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, I, F, F], V);
            RawOut::Void
        }
        (4, 14, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [I, F, F, F], V);
            RawOut::Void
        }
        (4, 15, 2) => {
            ffi_arm!(fn_ptr, int_args, f64_args, [F, F, F, F], V);
            RawOut::Void
        }
        (_, _, _) => {
            return Err(InterpreterError::InternalError(format!(
                "FFI: unsupported call shape (arity {}, arg mask {mask:#x})",
                args.len()
            )))
        }
    };
    let wrapped = match raw {
        RawOut::Int(v) => wrap_int_return(v, return_type),
        RawOut::Float(v) => Value::Float64(v),
        RawOut::Void => Object::Unit.into(),
    };
    Ok(wrapped)
}

/// Resolve `extern fn ... from "lib" as "sym"` to a callable address.
pub fn resolve_symbol(lib: &str, symbol: &str) -> Result<usize, InterpreterError> {
    let lib = resolve_lib(lib)?;
    // `get` wants the symbol as NUL-terminated bytes.
    let mut name: Vec<u8> = symbol.as_bytes().to_vec();
    name.push(0);
    let sym = unsafe { lib.get::<*mut u8>(&name) };
    let sym = sym.map_err(|e| {
        InterpreterError::InternalError(format!(
            "FFI: symbol `{symbol}` not found in library `{name:?}`: {e}"
        ))
    })?;
    Ok(sym.into_raw() as usize)
}
