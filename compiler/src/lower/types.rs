//! Small IR-level type helpers shared across the lowering pass.

use crate::ir::{Module, Type, TupleId};
use frontend::type_decl::TypeDecl;

/// Lower a `TypeDecl` to one of the IR's scalar `Type`s. Returns
/// `None` for compound shapes (struct / tuple / enum / array) — the
/// caller routes those through dedicated paths because they don't
/// fit a single SSA value.
pub(super) fn lower_scalar(ty: &TypeDecl) -> Option<Type> {
    match ty {
        TypeDecl::Int64 => Some(Type::I64),
        TypeDecl::UInt64 | TypeDecl::Number => Some(Type::U64),
        // NUM-W-AOT: narrow integer scalar lowering. Each maps to
        // the matching IR `Type` variant; cranelift codegen picks
        // up the width via `ir_to_cranelift_ty`.
        TypeDecl::Int8 => Some(Type::I8),
        TypeDecl::UInt8 => Some(Type::U8),
        TypeDecl::Int16 => Some(Type::I16),
        TypeDecl::UInt16 => Some(Type::U16),
        TypeDecl::Int32 => Some(Type::I32),
        TypeDecl::UInt32 => Some(Type::U32),
        TypeDecl::Float64 => Some(Type::F64),
        TypeDecl::Bool => Some(Type::Bool),
        TypeDecl::Unit => Some(Type::Unit),
        TypeDecl::String => Some(Type::Str),
        // #121: pointer types (`ptr` keyword) used by user-space
        // collections like `Dict<K, V>` (keys/vals fields). The
        // pointer is just a host-width address, so it lowers to
        // U64 in the IR — the heap builtin instructions
        // (`HeapAlloc` / `HeapRealloc` / `HeapFree` / `PtrRead` /
        // `PtrWrite`) all read and write through `i64`-typed
        // values for the address.
        TypeDecl::Ptr => Some(Type::U64),
        // #121 Phase B-min: `Allocator` handles are opaque
        // pointer-sized values (u64 in the IR). Sentinel 0 means
        // the default global allocator; other values would be
        // backend handles for arena / fixed_buffer once those land.
        TypeDecl::Allocator => Some(Type::U64),
        _ => None,
    }
}

/// Intern a tuple shape in the module's `tuple_defs` registry.
/// Linear-search dedup is fine because tuple shapes are sparse
/// (one entry per unique signature element list) and the IR is
/// built once per compile.
pub(super) fn intern_tuple(module: &mut Module, elements: Vec<Type>) -> TupleId {
    for (i, existing) in module.tuple_defs.iter().enumerate() {
        if *existing == elements {
            return TupleId(i as u32);
        }
    }
    let id = TupleId(module.tuple_defs.len() as u32);
    module.tuple_defs.push(elements);
    id
}
