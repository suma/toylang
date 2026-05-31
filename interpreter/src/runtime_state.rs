//! Shared runtime state for interpreter, JIT, and IR VM.
//!
//! This module centralizes the heap manager, allocator registry, and
//! active-allocator stack so all execution paths (tree-walker, JIT,
//! and the upcoming IR VM) see the same runtime state.

use std::cell::RefCell;
use std::rc::Rc;

use crate::heap::{Allocator, GlobalAllocator, HeapManager};

/// Runtime state shared by tree-walker, JIT, and IR VM.
/// Lives in a thread-local for the duration of program execution.
pub struct RuntimeState {
    pub heap: Rc<RefCell<HeapManager>>,
    /// Every allocator created during this run. Index 0 is the
    /// `GlobalAllocator`; arenas allocated via `__builtin_arena_allocator`
    /// land at later indices. Both JIT and IR VM treat indices as
    /// opaque u64 handles.
    pub registry: Vec<Rc<dyn Allocator>>,
    /// Active allocator stack — indices into `registry`. The bottom is
    /// always the global allocator (index 0).
    pub active: Vec<usize>,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeState {
    pub fn new() -> Self {
        let heap = Rc::new(RefCell::new(HeapManager::new()));
        let global = Rc::new(GlobalAllocator::new(Rc::clone(&heap)));
        Self {
            heap: Rc::clone(&heap),
            registry: vec![global],
            active: vec![0],
        }
    }

    /// Push an allocator handle onto the active stack.
    pub fn alloc_push(&mut self, handle: u64) {
        self.active.push(handle as usize);
    }

    /// Pop the top allocator from the active stack.
    pub fn alloc_pop(&mut self) {
        if self.active.len() > 1 {
            self.active.pop();
        }
    }

    /// Return the current top-of-stack allocator handle.
    pub fn alloc_current(&self) -> u64 {
        self.active.last().copied().unwrap_or(0) as u64
    }
}

thread_local! {
    /// Thread-local runtime state. Installed before execution and cleared
    /// after so extern "C" callbacks can reach in safely.
    pub static RT: RefCell<Option<RuntimeState>> = const { RefCell::new(None) };
}

/// Run a closure with access to the heap manager, returning `None` if
/// no runtime state is installed.
pub fn with_heap<R>(f: impl FnOnce(&mut HeapManager) -> R) -> Option<R> {
    RT.with(|slot| {
        let borrowed = slot.borrow();
        borrowed.as_ref().map(|rt| f(&mut rt.heap.borrow_mut()))
    })
}

/// Look up the active allocator (top of stack); falls back to `None`
/// when the runtime hasn't been installed.
pub fn with_active_allocator<R>(f: impl FnOnce(&Rc<dyn Allocator>) -> R) -> Option<R> {
    RT.with(|slot| {
        let borrowed = slot.borrow();
        let rt = borrowed.as_ref()?;
        let idx = rt.active.last().copied()?;
        rt.registry.get(idx).map(f)
    })
}
