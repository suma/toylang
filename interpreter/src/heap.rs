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

/// Allocation counters (MEMORY_PROFILING M0).
///
/// **Every field is defined on what the program *requested*, never on
/// what the allocator did with the request.** That is the whole point:
/// the interpreter's heap is a bump allocator that never reuses an
/// address, while the AOT path is libc `malloc`, which does — a
/// program can observe the difference today. Any counter derived from
/// addresses or region layout would therefore disagree between
/// backends by construction. Sizes and request order do not.
///
/// Fragmentation is deliberately absent here. It is a property of an
/// allocator's layout, so it belongs to `trait Alloc` and lands in M3;
/// deriving it from these numbers would be inventing it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryStats {
    /// Number of allocation requests. A `realloc` is *not* counted
    /// here even when the implementation services it by allocating —
    /// the program asked for one resize, not for an allocate plus a
    /// free, and an implementation that grows in place must produce
    /// the same number.
    pub alloc_count: u64,
    /// Number of free requests. Freeing a null pointer is a no-op and
    /// is not counted.
    pub free_count: u64,
    /// Number of resize requests, whether the block moved or not.
    pub realloc_count: u64,
    /// Total bytes ever obtained. Never decreases. A `realloc` that
    /// grows contributes the growth (`new - old`); one that shrinks
    /// contributes nothing, because no new bytes were obtained.
    pub cumulative_bytes: u64,
    /// Bytes currently held: obtained and not yet released. A shrinking
    /// `realloc` lowers it.
    pub live_bytes: u64,
    /// Highest value `live_bytes` reached.
    pub peak_live_bytes: u64,
    /// Value of `alloc_count + realloc_count` when `peak_live_bytes`
    /// was last raised — the reproducible stand-in for "when".
    ///
    /// Wall-clock time is deliberately not recorded: a report has to be
    /// byte-identical between runs to be diffable or assertable, and a
    /// timestamp makes that impossible.
    pub peak_at_request: u64,
}

impl MemoryStats {
    /// Requests that obtained memory, in program order. Used as the
    /// reproducible time axis.
    fn request_seq(&self) -> u64 {
        self.alloc_count + self.realloc_count
    }

    /// Record `bytes` newly obtained and refresh the peak.
    fn obtained(&mut self, bytes: u64) {
        self.cumulative_bytes += bytes;
        self.live_bytes += bytes;
        if self.live_bytes > self.peak_live_bytes {
            self.peak_live_bytes = self.live_bytes;
            self.peak_at_request = self.request_seq();
        }
    }

    /// Record `bytes` released. Saturating because a double free or a
    /// free of an untracked address must not wrap the counter into a
    /// nonsense number; the accounting stays monotone even when the
    /// program misbehaves.
    fn released(&mut self, bytes: u64) {
        self.live_bytes = self.live_bytes.saturating_sub(bytes);
    }
}

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
    /// MEMORY_PROFILING M0. Updated by the public `alloc` / `free` /
    /// `realloc` entry points only — never by the internal calls
    /// `realloc` makes to service a move, which would count one resize
    /// as an allocate plus a free and make the numbers depend on the
    /// implementation strategy.
    stats: MemoryStats,
}

impl HeapManager {
    pub fn new() -> Self {
        Self {
            memory: Vec::new(),
            allocations: HashMap::new(),
            next_addr: 1, // 0 is reserved for null pointer
            typed_slots: HashMap::new(),
            stats: MemoryStats::default(),
        }
    }

    /// Allocation counters for this heap.
    pub fn stats(&self) -> MemoryStats {
        self.stats
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
        let addr = self.alloc_uncounted(size);
        if addr != 0 {
            self.stats.alloc_count += 1;
            self.stats.obtained(size as u64);
        }
        addr
    }

    /// The allocation itself, without touching the counters.
    ///
    /// `realloc` services a move through this so one resize request
    /// stays one counted event. Note there is no free list and no
    /// reuse: `next_addr` only ever moves forward. That is the current
    /// behaviour, recorded rather than endorsed —
    /// `interpreter_heap_does_not_reuse_addresses` pins it.
    fn alloc_uncounted(&mut self, size: usize) -> usize {
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
        match self.free_uncounted(addr) {
            Some(size) => {
                self.stats.free_count += 1;
                self.stats.released(size as u64);
                true
            }
            None => false,
        }
    }

    /// Drop the tracking entry, returning the size it held. Counter-free
    /// for the same reason as `alloc_uncounted`.
    fn free_uncounted(&mut self, addr: usize) -> Option<usize> {
        self.allocations.remove(&addr)
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
            // MEMORY_PROFILING M0: one resize request, counted once and
            // in terms of the size change the program asked for. The
            // move below is this implementation's way of servicing it —
            // an allocator that grew the block in place would have to
            // report the same numbers, so the internal calls are the
            // uncounted ones.
            self.stats.realloc_count += 1;
            if new_size > old_size {
                self.stats.obtained((new_size - old_size) as u64);
            } else {
                self.stats.released((old_size - new_size) as u64);
            }
            // Allocate new memory
            let new_addr = self.alloc_uncounted(new_size);

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
            self.free_uncounted(addr);

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
    
    /// Read `width` (1/2/4/8) bytes little-endian from the raw byte buffer
    /// at `addr + offset`, zero-extended into a u64. Returns `None` on null
    /// or out-of-bounds. Used as a fallback when no typed slot exists (e.g.
    /// bytes written via `copy_memory` into a fresh destination buffer).
    pub fn read_scalar_bytes(&self, addr: usize, offset: usize, width: usize) -> Option<u64> {
        if addr == 0 || width == 0 {
            return None;
        }
        let slice = self.get_memory_slice(addr, offset + width)?;
        let bytes = &slice[offset..offset + width];
        let mut buf = [0u8; 8];
        buf[..width].copy_from_slice(bytes);
        Some(u64::from_le_bytes(buf))
    }

    /// Raw, base-agnostic byte read. Unlike `read_scalar_bytes`, `addr`
    /// need not be an allocation base — any interior address works because
    /// the byte buffer is contiguous and 1-based (`memory_offset = addr-1`).
    /// Used by the `str` runtime layout, whose value points at the trailing
    /// `u64 len` field (interior to its allocation).
    pub fn read_bytes_raw(&self, addr: usize, len: usize) -> Option<Vec<u8>> {
        if addr == 0 {
            return None;
        }
        let off = addr - 1;
        // Checked add: a misread length (e.g. interpreting a scalar as a str
        // handle) must not overflow-panic — fail the bounds check instead.
        match off.checked_add(len) {
            Some(end) if end <= self.memory.len() => Some(self.memory[off..end].to_vec()),
            _ => None,
        }
    }

    /// Raw, base-agnostic u64 read (little-endian) from an interior address.
    pub fn read_u64_raw(&self, addr: usize) -> Option<u64> {
        let bytes = self.read_bytes_raw(addr, 8)?;
        Some(u64::from_le_bytes(bytes.try_into().ok()?))
    }

    /// Raw, base-agnostic byte write. `addr` may be interior; the caller is
    /// responsible for the region being within an allocation it owns.
    pub fn write_bytes_raw(&mut self, addr: usize, bytes: &[u8]) -> bool {
        if addr == 0 {
            return false;
        }
        let off = addr - 1;
        if off + bytes.len() > self.memory.len() {
            return false;
        }
        self.memory[off..off + bytes.len()].copy_from_slice(bytes);
        true
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
#[cfg(test)]
mod memory_stats_tests {
    use super::*;

    /// The counters are defined on what the program asked for, so they
    /// are checked against hand-computed numbers rather than against
    /// whatever the implementation happened to do.
    #[test]
    fn alloc_and_free_account_exactly() {
        let mut heap = HeapManager::new();
        let a = heap.alloc(64);
        let b = heap.alloc(32);
        assert_eq!(heap.stats().live_bytes, 96);
        assert_eq!(heap.stats().peak_live_bytes, 96);

        heap.free(a);
        let s = heap.stats();
        assert_eq!(s.alloc_count, 2);
        assert_eq!(s.free_count, 1);
        assert_eq!(s.live_bytes, 32);
        assert_eq!(s.cumulative_bytes, 96, "cumulative never decreases");
        assert_eq!(s.peak_live_bytes, 96, "peak survives the free");

        heap.free(b);
        assert_eq!(heap.stats().live_bytes, 0);
    }

    #[test]
    fn realloc_is_one_request_not_an_alloc_plus_a_free() {
        // This implementation services a growing realloc by moving the
        // block. An allocator that grew it in place has to report the
        // same numbers, so the counts must not leak the strategy.
        let mut heap = HeapManager::new();
        let p = heap.alloc(16);
        heap.realloc(p, 64);

        let s = heap.stats();
        assert_eq!(s.alloc_count, 1, "the move must not count as an allocation");
        assert_eq!(s.free_count, 0, "the move must not count as a free");
        assert_eq!(s.realloc_count, 1);
        assert_eq!(s.live_bytes, 64);
        assert_eq!(s.cumulative_bytes, 64, "16 obtained, then 48 more");
    }

    #[test]
    fn shrinking_realloc_lowers_live_but_not_cumulative() {
        let mut heap = HeapManager::new();
        let p = heap.alloc(64);
        heap.realloc(p, 16);

        let s = heap.stats();
        assert_eq!(s.live_bytes, 16);
        assert_eq!(s.cumulative_bytes, 64, "shrinking obtains nothing");
        assert_eq!(s.peak_live_bytes, 64);
    }

    #[test]
    fn peak_records_the_request_it_happened_at() {
        let mut heap = HeapManager::new();
        heap.alloc(10); // request 1
        let big = heap.alloc(100); // request 2 — peak here, 110 live
        heap.free(big);
        heap.alloc(5); // request 3

        let s = heap.stats();
        assert_eq!(s.peak_live_bytes, 110);
        assert_eq!(s.peak_at_request, 2);
        assert_eq!(s.live_bytes, 15);
    }

    #[test]
    fn zero_size_and_null_are_not_counted() {
        let mut heap = HeapManager::new();
        assert_eq!(heap.alloc(0), 0, "zero-size allocation yields the null pointer");
        heap.free(0);
        let s = heap.stats();
        assert_eq!(s.alloc_count, 0);
        assert_eq!(s.free_count, 0);
        assert_eq!(s.live_bytes, 0);
    }

    #[test]
    fn a_double_free_cannot_drive_live_bytes_negative() {
        // Saturating accounting: a misbehaving program produces wrong
        // numbers, not nonsensical ones.
        let mut heap = HeapManager::new();
        let p = heap.alloc(32);
        heap.free(p);
        heap.free(p);
        let s = heap.stats();
        assert_eq!(s.live_bytes, 0);
        assert_eq!(s.free_count, 1, "the second free tracked nothing");
    }
}
