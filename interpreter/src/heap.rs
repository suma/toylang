use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

/// Runtime allocator abstraction.
///
/// Implementations plug into the ambient allocator stack so `with allocator = ...`
/// blocks redirect `__builtin_heap_alloc` and friends to a scoped allocator.
/// Methods take `&self` and rely on interior mutability so multiple stack
/// entries (and `Object::Allocator` values) can share the same underlying
/// state via `Rc`.
pub trait Allocator: fmt::Debug {
    fn alloc(&self, size: usize) -> usize;
    fn free(&self, addr: usize) -> bool;
    fn realloc(&self, addr: usize, new_size: usize) -> usize;
}

/// Default allocator backed by the process-wide `HeapManager`. Every
/// `EvaluationContext` creates one `GlobalAllocator` at initialization and
/// keeps it at the bottom of the allocator stack so code outside any
/// `with` block still has a valid target for heap operations.
#[derive(Debug, Clone)]
pub struct GlobalAllocator {
    inner: Rc<RefCell<HeapManager>>,
}

impl GlobalAllocator {
    pub fn new(inner: Rc<RefCell<HeapManager>>) -> Self {
        Self { inner }
    }
}

impl Allocator for GlobalAllocator {
    fn alloc(&self, size: usize) -> usize {
        self.inner.borrow_mut().alloc(size)
    }

    fn free(&self, addr: usize) -> bool {
        self.inner.borrow_mut().free(addr)
    }

    fn realloc(&self, addr: usize, new_size: usize) -> usize {
        self.inner.borrow_mut().realloc(addr, new_size)
    }
}

// `ArenaAllocator` / `FixedBufferAllocator` (runtime-side wrappers
// around the shared `HeapManager`) used to live here. The toylang
// stdlib `Arena` / `FixedBuffer` (`core/std/allocator.t`) replaces
// them: tracking + bulk-free + quota enforcement happen in toylang
// code on top of the default allocator. The runtime types and their
// builtins (`__builtin_arena_allocator` / `__builtin_arena_drop` /
// `__builtin_fixed_buffer_allocator` / `__builtin_fixed_buffer_drop`)
// were retired together.

/// Simple heap memory manager for pointer operations
#[derive(Debug)]
pub struct HeapManager {
    memory: Vec<u8>,
    allocations: HashMap<usize, usize>, // address -> size
    next_addr: usize,
    // Typed-slot storage keyed by (base address, byte offset). When a write
    // stores a non-u64 value (bool, i64, user struct, enum variant, ...)
    // the evaluator records the `RcObject` here so a matching `ptr_read`
    // can return it verbatim without round-tripping through the byte buffer.
    // u64 writes also update the byte buffer for backward compatibility with
    // byte-level reads, but still deposit the Rc here to keep a single source
    // of truth.
    typed_slots: HashMap<(usize, usize), crate::object::RcObject>,
}

impl HeapManager {
    pub fn new() -> Self {
        Self {
            memory: Vec::new(),
            allocations: HashMap::new(),
            next_addr: 1, // 0 is reserved for null pointer
            typed_slots: HashMap::new(),
        }
    }

    /// Record a typed slot so a later `typed_read` can return the exact Rc.
    pub fn typed_write(&mut self, addr: usize, offset: usize, value: crate::object::RcObject) {
        if addr != 0 {
            self.typed_slots.insert((addr, offset), value);
        }
    }

    /// Look up a previously-stored typed value, if any.
    pub fn typed_read(&self, addr: usize, offset: usize) -> Option<crate::object::RcObject> {
        self.typed_slots.get(&(addr, offset)).cloned()
    }
    
    /// Allocate memory and return address
    pub fn alloc(&mut self, size: usize) -> usize {
        if size == 0 {
            return 0; // null pointer for zero-size allocations
        }
        
        let addr = self.next_addr;
        self.memory.resize(self.memory.len() + size, 0);
        self.allocations.insert(addr, size);
        self.next_addr += size;
        addr
    }
    
    /// Free memory at address
    pub fn free(&mut self, addr: usize) -> bool {
        if addr == 0 {
            return true; // freeing null pointer is a no-op
        }
        
        self.allocations.remove(&addr).is_some()
    }
    
    /// Reallocate memory
    pub fn realloc(&mut self, addr: usize, new_size: usize) -> usize {
        if addr == 0 {
            // Reallocating null pointer is equivalent to alloc
            return self.alloc(new_size);
        }
        
        if new_size == 0 {
            // Reallocating to zero size is equivalent to free
            self.free(addr);
            return 0;
        }
        
        if let Some(old_size) = self.allocations.get(&addr).copied() {
            // Allocate new memory
            let new_addr = self.alloc(new_size);

            // Copy old data to new location
            let copy_size = old_size.min(new_size);
            // First get the source data to avoid borrowing conflicts
            if let Some(src) = self.get_memory_slice(addr, copy_size) {
                let temp_data: Vec<u8> = src.to_vec();
                if let Some(dest) = self.get_memory_slice_mut(new_addr, copy_size) {
                    dest.copy_from_slice(&temp_data);
                }
            }

            // Relocate typed slots from the old address to the new one so
            // values stashed under the previous base keep matching ptr_read
            // after realloc.
            let moved: Vec<(usize, crate::object::RcObject)> = self.typed_slots
                .iter()
                .filter_map(|((a, off), v)| {
                    if *a == addr && *off < copy_size {
                        Some((*off, v.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            self.typed_slots.retain(|(a, _), _| *a != addr);
            for (off, v) in moved {
                self.typed_slots.insert((new_addr, off), v);
            }

            // Free old memory
            self.free(addr);

            new_addr
        } else {
            0 // Invalid address
        }
    }
    
    /// Read u64 from memory at address + offset
    pub fn read_u64(&self, addr: usize, offset: usize) -> Option<u64> {
        if addr == 0 {
            return None; // null pointer access
        }
        
        let size = self.allocations.get(&addr)?;
        if offset + 8 > *size {
            return None; // out of bounds
        }
        
        let memory_offset = self.addr_to_memory_offset(addr)?;
        let slice = &self.memory[memory_offset + offset..memory_offset + offset + 8];
        Some(u64::from_le_bytes(slice.try_into().ok()?))
    }
    
    /// Write u64 to memory at address + offset
    pub fn write_u64(&mut self, addr: usize, offset: usize, value: u64) -> bool {
        if addr == 0 {
            return false; // null pointer access
        }
        
        let size = match self.allocations.get(&addr) {
            Some(s) => *s,
            None => return false,
        };
        
        if offset + 8 > size {
            return false; // out of bounds
        }
        
        if let Some(memory_offset) = self.addr_to_memory_offset(addr) {
            let bytes = value.to_le_bytes();
            self.memory[memory_offset + offset..memory_offset + offset + 8]
                .copy_from_slice(&bytes);
            true
        } else {
            false
        }
    }
    
    /// Copy memory from src to dest. Walks both the raw byte buffer
    /// (the classic mem_copy semantic) **and** the typed_slots map
    /// (so values stashed by `__builtin_str_to_ptr` /
    /// `__builtin_ptr_write` survive a `mem_copy` into a fresh
    /// destination buffer). The typed copy is offset-relative —
    /// every entry under `(src_addr, off)` for `off < size` is
    /// re-keyed under `(dest_addr, off)` so per-byte / per-element
    /// reads at the destination see the same values as the source.
    pub fn copy_memory(&mut self, src_addr: usize, dest_addr: usize, size: usize) -> bool {
        if size == 0 {
            // Zero-byte copy is a no-op success — including when
            // either pointer is null. Matches the AOT path's call
            // into libc memcpy(3), which is also a no-op for n==0,
            // and unblocks the `Vec::from_str("")` /
            // `heap_alloc(0)` + `heap_realloc(p, 0)` chain in
            // `core/std/collections/vec.t::from_str`.
            return true;
        }
        if src_addr == 0 || dest_addr == 0 {
            return false; // null pointer access
        }

        let mut copied_any = false;

        // Raw byte buffer copy — covers AOT-style buffers built by
        // `heap_alloc` + raw `ptr_write`.
        if let Some(src_slice) = self.get_memory_slice(src_addr, size) {
            let temp_data: Vec<u8> = src_slice.to_vec();
            if let Some(dest_slice) = self.get_memory_slice_mut(dest_addr, size) {
                dest_slice.copy_from_slice(&temp_data);
                copied_any = true;
            }
        }

        // typed_slots range copy — covers buffers populated by
        // `__builtin_str_to_ptr` (writes one `Object::U8` per byte)
        // or by `__builtin_ptr_write(p, off, value)` for non-u64
        // typed values. Without this, the AOT-style `mem_copy(src,
        // dest, n)` over a `s.as_ptr()` source returns no data on
        // the interpreter (the bytes only live in typed_slots).
        let snapshot: Vec<(usize, crate::object::RcObject)> = self
            .typed_slots
            .iter()
            .filter_map(|((a, off), v)| {
                if *a == src_addr && *off < size {
                    Some((*off, v.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (off, value) in snapshot {
            self.typed_slots.insert((dest_addr, off), value);
            copied_any = true;
        }

        copied_any
    }
    
    /// Move memory from src to dest (handles overlapping regions)
    pub fn move_memory(&mut self, src_addr: usize, dest_addr: usize, size: usize) -> bool {
        if size == 0 {
            return true; // no-op success — mirrors libc memmove(3)
        }
        if src_addr == 0 || dest_addr == 0 {
            return false; // null pointer access
        }
        
        // For simplicity, we'll copy the data to a temporary buffer first
        if let Some(src_slice) = self.get_memory_slice(src_addr, size) {
            let temp_data: Vec<u8> = src_slice.to_vec();
            if let Some(dest_slice) = self.get_memory_slice_mut(dest_addr, size) {
                dest_slice.copy_from_slice(&temp_data);
                return true;
            }
        }
        false
    }
    
    /// Set memory region to a specific byte value
    pub fn set_memory(&mut self, addr: usize, value: u8, size: usize) -> bool {
        if size == 0 {
            return true; // no-op success — mirrors libc memset(3)
        }
        if addr == 0 {
            return false; // null pointer access
        }
        
        if let Some(slice) = self.get_memory_slice_mut(addr, size) {
            slice.fill(value);
            true
        } else {
            false
        }
    }
    
    /// Check if address is valid
    pub fn is_valid_address(&self, addr: usize) -> bool {
        addr == 0 || self.allocations.contains_key(&addr)
    }
    
    // Helper methods
    
    fn addr_to_memory_offset(&self, addr: usize) -> Option<usize> {
        // Simple linear mapping for now
        // In a real implementation, this would be more complex
        if self.allocations.contains_key(&addr) {
            Some(addr - 1) // subtract 1 because addresses start at 1
        } else {
            None
        }
    }
    
    fn get_memory_slice(&self, addr: usize, size: usize) -> Option<&[u8]> {
        let alloc_size = self.allocations.get(&addr)?;
        if size > *alloc_size {
            return None;
        }
        
        let memory_offset = self.addr_to_memory_offset(addr)?;
        if memory_offset + size <= self.memory.len() {
            Some(&self.memory[memory_offset..memory_offset + size])
        } else {
            None
        }
    }
    
    fn get_memory_slice_mut(&mut self, addr: usize, size: usize) -> Option<&mut [u8]> {
        let alloc_size = self.allocations.get(&addr).copied()?;
        if size > alloc_size {
            return None;
        }
        
        let memory_offset = self.addr_to_memory_offset(addr)?;
        if memory_offset + size <= self.memory.len() {
            Some(&mut self.memory[memory_offset..memory_offset + size])
        } else {
            None
        }
    }
}

impl Default for HeapManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heap_alloc_free() {
        let mut heap = HeapManager::new();
        
        // Allocate memory
        let addr = heap.alloc(64);
        assert_ne!(addr, 0);
        assert!(heap.is_valid_address(addr));
        
        // Free memory
        assert!(heap.free(addr));
        
        // Free null pointer should succeed
        assert!(heap.free(0));
    }
    
    #[test]
    fn test_heap_read_write() {
        let mut heap = HeapManager::new();
        
        let addr = heap.alloc(64);
        assert_ne!(addr, 0);
        
        // Write and read u64
        assert!(heap.write_u64(addr, 0, 0x1234567890abcdef));
        assert_eq!(heap.read_u64(addr, 0), Some(0x1234567890abcdef));
        
        // Out of bounds access should fail
        assert_eq!(heap.read_u64(addr, 64), None);
        assert!(!heap.write_u64(addr, 64, 0));
        
        // Null pointer access should fail
        assert_eq!(heap.read_u64(0, 0), None);
        assert!(!heap.write_u64(0, 0, 0));
    }
    
    #[test]
    fn test_heap_copy_move_set() {
        let mut heap = HeapManager::new();
        
        let src_addr = heap.alloc(64);
        let dest_addr = heap.alloc(64);
        
        // Write some data to source
        assert!(heap.write_u64(src_addr, 0, 0x1111111111111111));
        assert!(heap.write_u64(src_addr, 8, 0x2222222222222222));
        
        // Copy memory
        assert!(heap.copy_memory(src_addr, dest_addr, 16));
        assert_eq!(heap.read_u64(dest_addr, 0), Some(0x1111111111111111));
        assert_eq!(heap.read_u64(dest_addr, 8), Some(0x2222222222222222));
        
        // Set memory
        assert!(heap.set_memory(dest_addr, 0xff, 16));
        assert_eq!(heap.read_u64(dest_addr, 0), Some(0xffffffffffffffff));
        assert_eq!(heap.read_u64(dest_addr, 8), Some(0xffffffffffffffff));
    }

    #[test]
    fn test_global_allocator_delegates_to_heap_manager() {
        let heap = Rc::new(RefCell::new(HeapManager::new()));
        let allocator = GlobalAllocator::new(heap.clone());

        let addr = allocator.alloc(32);
        assert_ne!(addr, 0);
        assert!(heap.borrow().is_valid_address(addr));

        assert!(allocator.free(addr));
        assert!(!heap.borrow().is_valid_address(addr));
    }

    // Arena / FixedBuffer runtime tests removed when the runtime
    // arena/fixed_buffer types were retired. Equivalent contracts
    // are now covered end-to-end by the consistency suite against
    // the toylang stdlib `Arena` / `FixedBuffer` (`compiler/tests/
    // consistency.rs::aot_arena_bytes_used_and_reset` etc.).
}