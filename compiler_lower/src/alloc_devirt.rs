//! #121 Phase B leftover: settle the allocator a heap operation goes
//! through when the whole program answers it.
//!
//! Every `HeapAlloc` / `HeapRealloc` / `HeapFree` is lowered with
//! `AllocatorBinding::Ambient`: the allocator is whatever the runtime
//! stack holds at that moment, and a function cannot know that on its
//! own -- `with allocator = a { f() }` makes `f`'s allocations go
//! through `a` although `f` names no allocator. So each operation asks
//! the runtime (`toy_alloc_current`) first.
//!
//! The one case the *module* can answer is a program that never pushes
//! an allocator: no `AllocPush` anywhere means the stack is empty for
//! the whole run, and the active allocator is the default one (handle
//! 0) at every heap operation. Those operations are marked
//! `Static(0)`, and codegen passes the constant instead of calling the
//! runtime. A program with a single `with` keeps every operation
//! `Ambient`.

use crate::ir::{AllocatorBinding, InstKind, Module};

/// The handle the runtime uses for the default allocator (the empty
/// stack's answer in `toy_alloc_current`).
const DEFAULT_ALLOCATOR: u32 = 0;

pub(crate) fn devirtualize_heap_bindings(module: &mut Module) {
    let pushes = module.functions.iter().any(|f| {
        f.blocks
            .iter()
            .any(|b| b.instructions.iter().any(|i| matches!(i.kind, InstKind::AllocPush { .. })))
    });
    if pushes {
        return;
    }
    for f in module.functions.iter_mut() {
        for b in f.blocks.iter_mut() {
            for inst in b.instructions.iter_mut() {
                match &mut inst.kind {
                    InstKind::HeapAlloc { binding, .. }
                    | InstKind::HeapRealloc { binding, .. }
                    | InstKind::HeapFree { binding, .. } => {
                        if *binding == AllocatorBinding::Ambient {
                            *binding = AllocatorBinding::Static(DEFAULT_ALLOCATOR);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
