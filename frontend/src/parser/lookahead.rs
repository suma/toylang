use crate::token::{Token, Kind};
use std::collections::VecDeque;

/// High-performance lookahead buffer for token management with ring buffer optimization
pub struct LookaheadBuffer {
    /// Ring buffer for efficient token storage and access
    buffer: VecDeque<Token>,
    /// Current position in the buffer
    position: usize,
    /// Maximum buffer size before cleanup (dynamic threshold)
    max_size: usize,
    /// Minimum buffer size to maintain after cleanup
    min_size: usize,
    /// Track total tokens consumed for statistics
    consumed_count: usize,
}

impl LookaheadBuffer {
    /// Create a new lookahead buffer with default settings
    pub fn new() -> Self {
        Self::with_capacity(64, 32)
    }

    /// Create a new lookahead buffer with custom capacity settings
    pub fn with_capacity(max_size: usize, min_size: usize) -> Self {
        LookaheadBuffer {
            buffer: VecDeque::with_capacity(max_size),
            position: 0,
            max_size,
            min_size,
            consumed_count: 0,
        }
    }

    /// Get the current token without consuming it
    pub fn peek(&self) -> Option<&Kind> {
        self.buffer.get(self.position).map(|t| &t.kind)
    }

    /// Get a token at relative position from current without consuming
    pub fn peek_at(&self, relative_pos: usize) -> Option<&Kind> {
        self.buffer.get(self.position + relative_pos).map(|t| &t.kind)
    }

    /// Get position information for a token at relative position
    pub fn peek_position_at(&self, relative_pos: usize) -> Option<&std::ops::Range<usize>> {
        self.buffer.get(self.position + relative_pos).map(|t| &t.position)
    }

    /// Consume the current token and move to the next
    pub fn advance(&mut self) {
        if self.position < self.buffer.len() {
            self.position += 1;
            self.consumed_count += 1;
            
            // Trigger cleanup if needed
            if self.should_cleanup() {
                self.cleanup();
            }
        }
    }

    /// Add a new token to the buffer
    pub fn push(&mut self, token: Token) {
        self.buffer.push_back(token);
    }

    /// Insert a token at the current position (for token splitting like >> to > >)
    /// The inserted token will be the next token to be consumed
    pub fn insert_at_current(&mut self, token: Token) {
        self.buffer.insert(self.position, token);
    }

    /// Check if we have enough tokens available for the given relative position
    pub fn has_token_at(&self, relative_pos: usize) -> bool {
        self.position + relative_pos < self.buffer.len()
    }

    /// Get the number of available tokens from current position
    pub fn available_tokens(&self) -> usize {
        self.buffer.len().saturating_sub(self.position)
    }

    /// Check if cleanup should be triggered
    fn should_cleanup(&self) -> bool {
        // Cleanup when buffer is large and we've consumed significant portion
        self.buffer.len() > self.max_size && self.position > self.max_size / 2
    }

    /// Perform efficient cleanup by removing consumed tokens
    fn cleanup(&mut self) {
        if self.position > 0 {
            // Remove consumed tokens from the front, but keep minimum tokens if possible
            let tokens_to_remove = if self.buffer.len() > self.min_size {
                // Keep at least min_size tokens, or remove consumed tokens if less
                std::cmp::min(self.position, self.buffer.len().saturating_sub(self.min_size))
            } else {
                // If we have less than min_size tokens, don't remove any
                0
            };
            
            for _ in 0..tokens_to_remove {
                self.buffer.pop_front();
            }
            self.position = self.position.saturating_sub(tokens_to_remove);
        }
        
        // Shrink capacity if buffer is much larger than needed
        if self.buffer.capacity() > self.max_size * 2 {
            self.buffer.shrink_to(self.max_size);
        }
    }

    /// Force cleanup regardless of thresholds (useful for memory pressure)
    pub fn force_cleanup(&mut self) {
        // Force cleanup: remove all consumed tokens and reset position to 0
        for _ in 0..self.position {
            self.buffer.pop_front();
        }
        self.position = 0;
    }

    /// Get buffer statistics for debugging and optimization
    pub fn stats(&self) -> BufferStats {
        BufferStats {
            buffer_size: self.buffer.len(),
            buffer_capacity: self.buffer.capacity(),
            current_position: self.position,
            available_tokens: self.available_tokens(),
            consumed_count: self.consumed_count,
        }
    }

    /// Reset the buffer completely (useful for parser resets)
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.position = 0;
        self.consumed_count = 0;
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.available_tokens() == 0
    }

    /// Ensure we have at least `min_tokens` available from current position
    /// Returns true if we have enough, false if more tokens are needed
    pub fn ensure_available(&self, min_tokens: usize) -> bool {
        self.available_tokens() >= min_tokens
    }

    /// Get current buffer efficiency ratio (0.0 to 1.0)
    /// Higher values indicate better memory utilization
    pub fn efficiency_ratio(&self) -> f64 {
        if self.buffer.is_empty() {
            1.0
        } else {
            self.available_tokens() as f64 / self.buffer.len() as f64
        }
    }
}

impl Default for LookaheadBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about buffer performance for monitoring and optimization
#[derive(Debug, Clone)]
pub struct BufferStats {
    pub buffer_size: usize,
    pub buffer_capacity: usize,
    pub current_position: usize,
    pub available_tokens: usize,
    pub consumed_count: usize,
}

impl std::fmt::Display for BufferStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "LookaheadBuffer Stats: size={}, capacity={}, pos={}, available={}, consumed={}",
            self.buffer_size, self.buffer_capacity, self.current_position,
            self.available_tokens, self.consumed_count
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{Token, Kind};

    fn create_test_token(kind: Kind, start: usize, end: usize) -> Token {
        Token {
            kind,
            position: start..end,
        }
    }

    #[test]
    fn test_basic_operations() {
        let mut buffer = LookaheadBuffer::new();
        
        // Test empty buffer
        assert!(buffer.is_empty());
        assert_eq!(buffer.peek(), None);
        assert_eq!(buffer.available_tokens(), 0);
        
        // Add tokens
        buffer.push(create_test_token(Kind::UInt64(42), 0, 2));
        buffer.push(create_test_token(Kind::IAdd, 3, 4));
        buffer.push(create_test_token(Kind::UInt64(24), 5, 7));
        
        // Test peek operations
        assert_eq!(buffer.peek(), Some(&Kind::UInt64(42)));
        assert_eq!(buffer.peek_at(1), Some(&Kind::IAdd));
        assert_eq!(buffer.peek_at(2), Some(&Kind::UInt64(24)));
        assert_eq!(buffer.available_tokens(), 3);
        
        // Test advance
        buffer.advance();
        assert_eq!(buffer.peek(), Some(&Kind::IAdd));
        assert_eq!(buffer.available_tokens(), 2);
        
        buffer.advance();
        assert_eq!(buffer.peek(), Some(&Kind::UInt64(24)));
        assert_eq!(buffer.available_tokens(), 1);
    }

    #[test]
    fn test_cleanup_mechanism() {
        let mut buffer = LookaheadBuffer::with_capacity(4, 2);
        
        // Fill buffer beyond threshold
        for i in 0..6 {
            buffer.push(create_test_token(Kind::UInt64(i as u64), i, i + 1));
        }
        
        // Advance to trigger cleanup
        for _ in 0..3 {
            buffer.advance();
        }
        
        // Should still be able to access remaining tokens
        assert!(buffer.available_tokens() > 0);
        assert!(buffer.stats().buffer_size <= 6);
    }

    #[test]
    fn test_efficiency_ratio() {
        let mut buffer = LookaheadBuffer::new();
        
        // Empty buffer should have 100% efficiency
        assert_eq!(buffer.efficiency_ratio(), 1.0);
        
        // Add 3 tokens
        for i in 0..3 {
            buffer.push(create_test_token(Kind::UInt64(i as u64), i, i + 1));
        }
        
        // All tokens available: 100% efficiency
        assert_eq!(buffer.efficiency_ratio(), 1.0);
        
        // Consume 1 token: 66% efficiency
        buffer.advance();
        assert!((buffer.efficiency_ratio() - 0.666).abs() < 0.01);
    }

    #[test]
    fn test_position_tracking() {
        let mut buffer = LookaheadBuffer::new();
        
        buffer.push(create_test_token(Kind::UInt64(42), 10, 12));
        buffer.push(create_test_token(Kind::IAdd, 13, 14));
        
        // Test position access
        assert_eq!(buffer.peek_position_at(0), Some(&(10..12)));
        assert_eq!(buffer.peek_position_at(1), Some(&(13..14)));
        
        buffer.advance();
        assert_eq!(buffer.peek_position_at(0), Some(&(13..14)));
    }

    #[test]
    fn test_force_cleanup() {
        let mut buffer = LookaheadBuffer::new();
        
        // Add many tokens
        for i in 0..10 {
            buffer.push(create_test_token(Kind::UInt64(i as u64), i, i + 1));
        }
        
        // Advance several positions
        for _ in 0..5 {
            buffer.advance();
        }
        
        let stats_before = buffer.stats();
        buffer.force_cleanup();
        let stats_after = buffer.stats();
        
        // Position should be reset, but available tokens preserved
        assert_eq!(stats_after.current_position, 0);
        assert_eq!(stats_after.available_tokens, stats_before.available_tokens);
    }

    #[test]
    fn test_reset() {
        let mut buffer = LookaheadBuffer::new();
        
        buffer.push(create_test_token(Kind::UInt64(42), 0, 2));
        buffer.push(create_test_token(Kind::IAdd, 3, 4));
        buffer.advance();
        
        assert!(!buffer.is_empty()); // Still has tokens available
        
        buffer.reset();
        
        assert!(buffer.is_empty());
        assert_eq!(buffer.stats().consumed_count, 0);
    }
}