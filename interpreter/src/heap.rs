use std::collections::HashMap;

/// Simple heap memory manager for pointer operations
#[derive(Debug)]
pub struct HeapManager {
    memory: Vec<u8>,
    allocations: HashMap<usize, usize>, // address -> size
    next_addr: usize,
}

impl HeapManager {
    pub fn new() -> Self {
        Self {
            memory: Vec::new(),
            allocations: HashMap::new(),
            next_addr: 1, // 0 is reserved for null pointer
        }
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
    
    /// Copy memory from src to dest
    pub fn copy_memory(&mut self, src_addr: usize, dest_addr: usize, size: usize) -> bool {
        if src_addr == 0 || dest_addr == 0 {
            return false; // null pointer access
        }
        
        // First get the source data to avoid borrowing conflicts
        if let Some(src_slice) = self.get_memory_slice(src_addr, size) {
            let temp_data: Vec<u8> = src_slice.to_vec();
            if let Some(dest_slice) = self.get_memory_slice_mut(dest_addr, size) {
                dest_slice.copy_from_slice(&temp_data);
                return true;
            }
        }
        false
    }
    
    /// Move memory from src to dest (handles overlapping regions)
    pub fn move_memory(&mut self, src_addr: usize, dest_addr: usize, size: usize) -> bool {
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
}