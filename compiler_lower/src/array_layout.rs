//! Static layout helpers for array stack slots.
//!
//! Array elements are stored in a single backing `StackSlot` per
//! `Binding::Array` (or one per leaf under `soa`). A scalar element
//! takes its own width and is indexed by element. A struct / tuple /
//! enum element is **packed** (NUM-W-AOT-pack Phase 3): its leaves sit
//! at their own widths and alignments in declaration order, the order
//! `flatten_struct_locals` and `flatten_tuple_element_locals` produce
//! at function boundaries, and the slot is indexed in bytes --
//! `element_index * size + offsets[j]` (`PackedElement`,
//! `interleaved_units`). Codegen and the IR VM compute
//! `index * stride` with stride 1 for such a slot, so neither has to
//! know the layout.

use crate::ir::{Module, Type};

use super::bindings::ArrayStorage;
use super::FunctionLower;

/// The stride `elem_stride_bytes` answers for a compound element
/// type. Only asked of types that never back a slot directly -- a
/// compound element's slot is byte-addressed (`PackedElement`) -- so
/// it is a conservative placeholder, not a layout.
pub(super) const ARRAY_LEAF_STRIDE: u32 = 8;

/// How many leaf scalar slots one element of `ty` occupies in an
/// array's backing buffer. Scalars take one; structs and tuples
/// recursively flatten through their fields / elements.
pub(super) fn leaf_scalar_count(module: &Module, ty: Type) -> usize {
    match ty {
        Type::I64 | Type::U64 | Type::F64 | Type::F32 | Type::Bool | Type::Str => 1,
        // NUM-W-AOT: a narrow int is one leaf like its wide siblings;
        // its width decides its offset in a packed element.
        Type::I8 | Type::U8 | Type::I16 | Type::U16 | Type::I32 | Type::U32 => 1,
        // SIMD: a vector is one SSA value, but it does not fit an
        // 8-byte leaf slot, so arrays of vectors are not supported.
        // `leaf_scalar_count` is only consulted for array elements,
        // and the type checker has no `[f64x2; N]` path yet.
        Type::Vector(_) => 2,
        Type::Unit => 0,
        Type::Struct(id) => {
            let fields = module.struct_def(id).fields.clone();
            fields
                .iter()
                .map(|(_, ft)| leaf_scalar_count(module, *ft))
                .sum()
        }
        Type::Tuple(id) => {
            let elems = module.tuple_defs[id.0 as usize].clone();
            elems.iter().map(|t| leaf_scalar_count(module, *t)).sum()
        }
        // DATA-ORIENTED Phase 3: an enum element flattens the way
        // `collect_leaves` and `flatten_enum_storage_locals` already
        // flatten one — the u64 tag, then every variant's payload in
        // declaration order. `__builtin_sizeof`'s enum rule is the
        // same shape, which is why an enum needed no new layout
        // thinking to become an array element: it was already flat.
        //
        // Under `soa` that puts the tag in a column of its own, so a
        // loop that only asks *which variant* touches one byte per
        // element instead of the whole payload — the case a tagged
        // union cannot give you at all.
        Type::Enum(enum_id) => {
            let def = module.enum_def(enum_id);
            let payloads: Vec<Type> = def
                .variants
                .iter()
                .flat_map(|v| v.payload_types.iter().copied())
                .collect();
            1 + payloads
                .iter()
                .map(|t| leaf_scalar_count(module, *t))
                .sum::<usize>()
        }
    }
}

/// Per-leaf stride stored in `ArraySlotInfo`.
///
/// For *homogeneous scalar* element arrays the stride is the
/// scalar's actual byte size — `[u8; N]` packs to 1 byte per
/// leaf, `[u16; N]` to 2, `[u32; N]` to 4, `[u64; N]` /
/// `[i64; N]` / `[f64; N]` to 8. This shaves the per-element
/// memory cost of narrow-int arrays by up to 8× without changing
/// any of the lowering's leaf-index addressing math: the leaf
/// index for a homogeneous scalar element is just the element
/// index, and `byte_offset = leaf_idx * stride` lands on the
/// correct narrow slot.
///
/// A *compound* (struct / tuple) element has no single stride: its
/// slot is byte-addressed with per-leaf offsets (`PackedElement`),
/// and this answers `ARRAY_LEAF_STRIDE` only as a placeholder.
pub(super) fn elem_stride_bytes(ty: Type, _module: &Module) -> u32 {
    match ty {
        Type::I8 | Type::U8 | Type::Bool => 1,
        Type::I16 | Type::U16 => 2,
        Type::I32 | Type::U32 => 4,
        // SIMD-F32: native 4-byte stride for f32 element arrays.
        Type::F32 => 4,
        Type::I64 | Type::U64 | Type::F64 | Type::Str => 8,
        // Compound elements are byte-addressed (`PackedElement`);
        // this is never a slot's stride.
        Type::Struct(_) | Type::Tuple(_) => ARRAY_LEAF_STRIDE,
        // Unit / enum aren't valid array element types today; if
        // they ever reach here, the conservative 8-byte slot
        // stays correct.
        Type::Unit | Type::Enum(_) => ARRAY_LEAF_STRIDE,
        // SIMD: 128 bits, whatever the lane type.
        Type::Vector(_) => 16,
    }
}

/// NUM-W-AOT-pack Phase 3: how one compound element is laid out in an
/// interleaved (AoS) array slot.
///
/// Each leaf sits at its own width, aligned to it, in declaration
/// order, and the element is rounded up to its widest leaf -- the C
/// rule, so `[PackedRgba; N]` (four `u8`) is 4 bytes an element
/// instead of the 32 the uniform 8-byte leaf slot cost, and a `u64`
/// leaf is never split across an alignment boundary.
///
/// A compound-element slot is addressed in **bytes** (stride 1): leaf
/// `j` of element `i` is at index `i * size + offsets[j]`. Codegen and
/// the IR VM compute `index * stride`, so neither learns the layout.
pub(super) struct PackedElement {
    pub size: u64,
    pub offsets: Vec<u64>,
}

/// Whether an interleaved slot of `element_ty` is byte-addressed with
/// [`PackedElement`] offsets. Scalars keep their native stride and
/// are indexed by element.
pub(super) fn is_packed_element(element_ty: Type) -> bool {
    matches!(element_ty, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_))
}

pub(super) fn packed_element(module: &Module, element_ty: Type) -> PackedElement {
    let count = leaf_scalar_count(module, element_ty);
    let mut offsets = Vec::with_capacity(count);
    let mut at: u64 = 0;
    let mut align: u64 = 1;
    for j in 0..count {
        let width = elem_stride_bytes(leaf_type_at(module, element_ty, j), module) as u64;
        at = at.div_ceil(width) * width;
        offsets.push(at);
        at += width;
        align = align.max(width);
    }
    PackedElement { size: at.div_ceil(align).max(1) * align, offsets }
}

/// The IR type of leaf `j` (0-indexed) within an array element of
/// `element_ty`. Walks struct fields / tuple elements in
/// declaration order to match the layout `flatten_struct_locals`
/// and `flatten_tuple_element_locals` produce at the function
/// boundary.
pub(super) fn leaf_type_at(module: &Module, element_ty: Type, j: usize) -> Type {
    match element_ty {
        Type::Struct(id) => {
            let fields = module.struct_def(id).fields.clone();
            let mut acc = 0usize;
            for (_, ft) in &fields {
                let cnt = leaf_scalar_count(module, *ft);
                if j < acc + cnt {
                    return leaf_type_at(module, *ft, j - acc);
                }
                acc += cnt;
            }
            element_ty
        }
        Type::Tuple(id) => {
            let elems = module.tuple_defs[id.0 as usize].clone();
            let mut acc = 0usize;
            for et in &elems {
                let cnt = leaf_scalar_count(module, *et);
                if j < acc + cnt {
                    return leaf_type_at(module, *et, j - acc);
                }
                acc += cnt;
            }
            element_ty
        }
        // DATA-ORIENTED Phase 3: leaf 0 is the tag; the rest walk the
        // variants' payloads in the order `collect_leaves` writes
        // them.
        Type::Enum(enum_id) => {
            if j == 0 {
                return Type::U64;
            }
            let def = module.enum_def(enum_id);
            let payloads: Vec<Type> = def
                .variants
                .iter()
                .flat_map(|v| v.payload_types.iter().copied())
                .collect();
            let mut acc = 1usize;
            for pt in &payloads {
                let cnt = leaf_scalar_count(module, *pt);
                if j < acc + cnt {
                    return leaf_type_at(module, *pt, j - acc);
                }
                acc += cnt;
            }
            element_ty
        }
        _ => element_ty,
    }
}

impl<'a> FunctionLower<'a> {
    /// How an interleaved slot is indexed: one element spans `unit`
    /// index steps, and leaf `j` sits `offsets[j]` steps into it. For a
    /// packed compound slot those are bytes (NUM-W-AOT-pack Phase 3);
    /// for a scalar slot one element is one step.
    pub(super) fn interleaved_units(&self, slot: crate::ir::ArraySlotId) -> (u64, Vec<u64>) {
        let element_ty = self.module.function(self.func_id).array_slots[slot.0 as usize].element_ty;
        if is_packed_element(element_ty) {
            let packed = packed_element(self.module, element_ty);
            (packed.size, packed.offsets)
        } else {
            let n = leaf_scalar_count(self.module, element_ty) as u64;
            (n, (0..n).collect())
        }
    }

    /// DATA-ORIENTED Phase 0: allocate the backing slots for one
    /// array binding of `element_ty` x `length`.
    ///
    /// AoS (or `soa` with a scalar element -- one leaf is one
    /// column, so the two layouts coincide) takes the historical
    /// single interleaved slot. `soa` with a compound element takes
    /// one homogeneous slot per leaf scalar, each `length` long --
    /// a column is just an ordinary scalar array slot, so codegen
    /// and the IR VM address it with the existing `index * stride`
    /// math and never learn SoA exists.
    ///
    /// DATA-ORIENTED Phase 0.5: each column strides by its leaf's
    /// real width. A column is homogeneous, so this is the packing
    /// `elem_stride_bytes` already gives a scalar array (`[u8; N]` is
    /// 1 byte per element). NUM-W-AOT-pack Phase 3 packs the
    /// interleaved element too (`PackedElement`), so the two layouts
    /// now differ only by the element's alignment padding. Values are
    /// unaffected either way, which is why the pins are *frame* tests.
    pub(super) fn allocate_array_storage(
        &mut self,
        element_ty: Type,
        length: usize,
        soa: bool,
    ) -> ArrayStorage {
        let leaf_count = leaf_scalar_count(self.module, element_ty);
        if !soa || leaf_count == 1 {
            // NUM-W-AOT-pack Phase 3: a compound element is packed and
            // byte-addressed; `length` counts bytes for such a slot.
            let slot = if is_packed_element(element_ty) {
                let packed = packed_element(self.module, element_ty);
                self.module
                    .function_mut(self.func_id)
                    .add_array_slot(element_ty, length * packed.size as usize, 1)
            } else {
                let stride = elem_stride_bytes(element_ty, self.module);
                self.module
                    .function_mut(self.func_id)
                    .add_array_slot(element_ty, length * leaf_count, stride)
            };
            return ArrayStorage::Interleaved(slot);
        }
        let mut columns = Vec::with_capacity(leaf_count);
        for j in 0..leaf_count {
            let leaf_ty = leaf_type_at(self.module, element_ty, j);
            let stride = elem_stride_bytes(leaf_ty, self.module);
            let slot = self
                .module
                .function_mut(self.func_id)
                .add_array_slot(leaf_ty, length, stride);
            columns.push(slot);
        }
        ArrayStorage::Columns(columns)
    }
}
