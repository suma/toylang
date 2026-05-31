//! Shared layout helpers for compound-type flattening.
//!
//! Both the AOT codegen and the interpreter VM use these routines
//! so struct / tuple / enum leaf ordering stays identical across
//! all backends.

use crate::{Module, Type};

/// Flatten an IR `Type` (struct / tuple / enum / scalar) to the leaf
/// scalar types in canonical declaration order.
///
/// This mirrors the walk that `flatten_struct_to_cranelift_tys` does
/// in the cranelift backend, but stays in IR `Type` space so it can be
/// used by any backend (AOT, JIT, or the interpreter VM) without
/// pulling in cranelift types.
pub fn flatten_compound_leaf_types(module: &Module, ty: Type, out: &mut Vec<Type>) {
    match ty {
        Type::Struct(id) => {
            let def = module.struct_def(id);
            let field_tys: Vec<Type> = def.fields.iter().map(|(_, t)| *t).collect();
            for ft in field_tys {
                flatten_compound_leaf_types(module, ft, out);
            }
        }
        Type::Tuple(id) => {
            let elem_tys: Vec<Type> = module
                .tuple_defs
                .get(id.0 as usize)
                .cloned()
                .unwrap_or_default();
            for et in elem_tys {
                flatten_compound_leaf_types(module, et, out);
            }
        }
        Type::Enum(id) => {
            // Tag (U64) + each variant's payload leaves, mirroring
            // codegen's enum boundary layout.
            out.push(Type::U64);
            let def = module.enum_def(id);
            let payload_tys: Vec<Type> = def
                .variants
                .iter()
                .flat_map(|v| v.payload_types.iter().copied())
                .collect();
            for pt in payload_tys {
                flatten_compound_leaf_types(module, pt, out);
            }
        }
        // Scalars contribute themselves directly.
        Type::I64 | Type::U64 | Type::I8 | Type::U8 | Type::I16 | Type::U16
        | Type::I32 | Type::U32 | Type::F64 | Type::Bool | Type::Str => out.push(ty),
        Type::Unit => {} // skip
    }
}
