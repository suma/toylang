//! Optional JIT backend for the interpreter, gated behind the `jit` cargo
//! feature. Activated at runtime by setting `INTERPRETER_JIT=1`.
//!
//! Only a small numeric/bool subset of the language is currently handled;
//! anything outside the supported subset causes a silent fallback to the
//! tree-walking interpreter.

mod eligibility;
mod codegen;
mod runtime;

pub use runtime::try_execute_main;
pub use runtime::with_jit_override;
pub use runtime::with_jit_verbose_override;

/// DEBUG-OBS D4: what a backtrace calls a monomorph — `S::boom` for a
/// method, the bare name for a free function.
///
/// Deliberately not `mono_display_name`, which mangles the type
/// arguments in so the linker can tell specializations apart. A reader
/// wants the name they wrote.
pub(crate) fn frame_name_for(
    interner: &string_interner::DefaultStringInterner,
    key: &crate::jit::eligibility::MonoKey,
) -> String {
    use crate::jit::eligibility::MonoTarget;
    let name = |sym| interner.resolve(sym).unwrap_or("<unknown>").to_string();
    match &key.0 {
        MonoTarget::Function(sym) => name(*sym),
        MonoTarget::Method(ty, method) => format!("{}::{}", name(*ty), name(*method)),
    }
}

/// A `toylang_rt::ToyFrameInfo` record — `{ u64 line }{ name }{ 0 }` —
/// with the lifetime the compiled code needs.
///
/// Leaked, like the diagnostic blobs in `codegen`: the JIT's code lives
/// for the process, so anything it holds a raw pointer to must too. The
/// cache keeps that bounded to one record per distinct call site rather
/// than one per compile.
///
/// Backed by a `Vec<u64>` so the record is 8-aligned; a `Box<[u8]>`
/// would not be, and the runtime reads the line as a `u64`.
pub(crate) fn frame_record(name: &str, line: u32) -> *const u8 {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<(String, u32), usize>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (name.to_string(), line);
    if let Some(addr) = cache.lock().unwrap().get(&key) {
        return *addr as *const u8;
    }
    let mut words: Vec<u64> = vec![line as u64];
    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    while !bytes.len().is_multiple_of(8) {
        bytes.push(0);
    }
    for chunk in bytes.chunks(8) {
        words.push(u64::from_le_bytes(chunk.try_into().expect("8 bytes")));
    }
    let addr = Box::leak(words.into_boxed_slice()).as_ptr() as usize;
    cache.lock().unwrap().insert(key, addr);
    addr as *const u8
}
