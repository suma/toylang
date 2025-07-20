use crate::token::{Token, Kind};
use super::lookahead::LookaheadBuffer;
use anyhow::Result;

/// Trait for token sources that can provide tokens to the parser
pub trait TokenSource {
    /// Get the next token from the source
    fn next_token(&mut self) -> Result<Option<Token>>;
    
    /// Get current line count for error reporting
    fn line_count(&self) -> usize;
}

/// Token provider that combines a TokenSource with an optimized LookaheadBuffer
pub struct TokenProvider<T: TokenSource> {
    source: T,
    buffer: LookaheadBuffer,
}

impl<T: TokenSource> TokenProvider<T> {
    /// Create a new token provider with the given source
    pub fn new(source: T) -> Self {
        TokenProvider {
            source,
            buffer: LookaheadBuffer::new(),
        }
    }

    /// Create a new token provider with custom buffer settings
    pub fn with_buffer_capacity(source: T, max_size: usize, min_size: usize) -> Self {
        TokenProvider {
            source,
            buffer: LookaheadBuffer::with_capacity(max_size, min_size),
        }
    }

    /// Peek at the current token without consuming it
    pub fn peek(&mut self) -> Option<&Kind> {
        self.ensure_token_available(0);
        self.buffer.peek()
    }

    /// Peek at a token at relative position without consuming
    pub fn peek_at(&mut self, relative_pos: usize) -> Option<&Kind> {
        self.ensure_token_available(relative_pos);
        self.buffer.peek_at(relative_pos)
    }

    /// Get position information for a token at relative position
    pub fn peek_position_at(&mut self, relative_pos: usize) -> Option<&std::ops::Range<usize>> {
        self.ensure_token_available(relative_pos);
        self.buffer.peek_position_at(relative_pos)
    }

    /// Consume the current token and advance to the next
    pub fn advance(&mut self) {
        if self.buffer.available_tokens() > 0 {
            self.buffer.advance();
        }
    }

    /// Ensure we have a token available at the given relative position
    fn ensure_token_available(&mut self, relative_pos: usize) {
        while !self.buffer.ensure_available(relative_pos + 1) {
            match self.fetch_next_token() {
                Ok(Some(token)) => self.buffer.push(token),
                Ok(None) | Err(_) => break,
            }
        }
    }

    /// Fetch the next token from the source, filtering comments
    fn fetch_next_token(&mut self) -> Result<Option<Token>> {
        loop {
            match self.source.next_token()? {
                Some(token) => {
                    // Skip comment tokens automatically
                    if matches!(token.kind, Kind::Comment(_)) {
                        continue;
                    }
                    return Ok(Some(token));
                }
                None => return Ok(None),
            }
        }
    }

    /// Get current line count from the source
    pub fn line_count(&self) -> usize {
        self.source.line_count()
    }

    /// Get buffer statistics for monitoring
    pub fn buffer_stats(&self) -> super::lookahead::BufferStats {
        self.buffer.stats()
    }

    /// Force buffer cleanup (useful for memory management)
    pub fn cleanup_buffer(&mut self) {
        self.buffer.force_cleanup();
    }

    /// Reset the provider (clears buffer, keeps source)
    pub fn reset(&mut self) {
        self.buffer.reset();
    }
}

/// Lexer wrapper that implements TokenSource
pub struct LexerTokenSource<'a> {
    lexer: crate::parser::core::lexer::Lexer<'a>,
}

impl<'a> LexerTokenSource<'a> {
    pub fn new(input: &'a str) -> Self {
        LexerTokenSource {
            lexer: crate::parser::core::lexer::Lexer::new(input, 1u64),
        }
    }
}

impl<'a> TokenSource for LexerTokenSource<'a> {
    fn next_token(&mut self) -> Result<Option<Token>> {
        match self.lexer.yylex() {
            Ok(token) => Ok(Some(token)),
            Err(_) => Ok(None), // End of input or error
        }
    }

    fn line_count(&self) -> usize {
        self.lexer.get_current_line_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{Token, Kind};

    // Mock token source for testing
    struct MockTokenSource {
        tokens: Vec<Token>,
        position: usize,
        line_count: usize,
    }

    impl MockTokenSource {
        fn new(tokens: Vec<Token>) -> Self {
            MockTokenSource {
                tokens,
                position: 0,
                line_count: 1,
            }
        }
    }

    impl TokenSource for MockTokenSource {
        fn next_token(&mut self) -> Result<Option<Token>> {
            if self.position < self.tokens.len() {
                let token = self.tokens[self.position].clone();
                self.position += 1;
                Ok(Some(token))
            } else {
                Ok(None)
            }
        }

        fn line_count(&self) -> usize {
            self.line_count
        }
    }

    fn create_test_token(kind: Kind, start: usize, end: usize) -> Token {
        Token {
            kind,
            position: start..end,
        }
    }

    #[test]
    fn test_token_provider_basic_operations() {
        let tokens = vec![
            create_test_token(Kind::UInt64(42), 0, 2),
            create_test_token(Kind::IAdd, 3, 4),
            create_test_token(Kind::UInt64(24), 5, 7),
        ];
        
        let source = MockTokenSource::new(tokens);
        let mut provider = TokenProvider::new(source);

        // Test peek operations
        assert_eq!(provider.peek(), Some(&Kind::UInt64(42)));
        assert_eq!(provider.peek_at(1), Some(&Kind::IAdd));
        assert_eq!(provider.peek_at(2), Some(&Kind::UInt64(24)));

        // Test advance
        provider.advance();
        assert_eq!(provider.peek(), Some(&Kind::IAdd));

        provider.advance();
        assert_eq!(provider.peek(), Some(&Kind::UInt64(24)));

        provider.advance();
        assert_eq!(provider.peek(), None);
    }

    #[test]
    fn test_comment_filtering() {
        let tokens = vec![
            create_test_token(Kind::UInt64(42), 0, 2),
            create_test_token(Kind::Comment("test comment".to_string()), 3, 16),
            create_test_token(Kind::IAdd, 17, 18),
        ];
        
        let source = MockTokenSource::new(tokens);
        let mut provider = TokenProvider::new(source);

        // Comments should be automatically filtered
        assert_eq!(provider.peek(), Some(&Kind::UInt64(42)));
        provider.advance();
        assert_eq!(provider.peek(), Some(&Kind::IAdd)); // Comment skipped
    }

    #[test]
    fn test_position_tracking() {
        let tokens = vec![
            create_test_token(Kind::UInt64(42), 10, 12),
            create_test_token(Kind::IAdd, 13, 14),
        ];
        
        let source = MockTokenSource::new(tokens);
        let mut provider = TokenProvider::new(source);

        assert_eq!(provider.peek_position_at(0), Some(&(10..12)));
        assert_eq!(provider.peek_position_at(1), Some(&(13..14)));
    }

    #[test]
    fn test_buffer_stats() {
        let tokens = vec![
            create_test_token(Kind::UInt64(42), 0, 2),
            create_test_token(Kind::IAdd, 3, 4),
        ];
        
        let source = MockTokenSource::new(tokens);
        let mut provider = TokenProvider::new(source);

        // Load tokens into buffer
        provider.peek();
        provider.peek_at(1);

        let stats = provider.buffer_stats();
        assert!(stats.buffer_size >= 2);
        assert_eq!(stats.current_position, 0);
    }
}