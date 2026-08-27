use crate::ast::{ExprRef, StmtRef, Node};
use crate::type_checker::{TypeCheckerVisitor, SourceLocation};

/// Source-location helpers for `TypeCheckerVisitor`.
impl<'a> TypeCheckerVisitor<'a> {
    /// Convert a raw byte offset in the source text to a (line, column)
    /// pair (both 1-based). Walks the source counting `\n` characters
    /// to find the line, then uses the offset after the last newline
    /// as the column.
    pub fn calculate_line_col_from_offset(&self, offset: usize) -> (u32, u32) {
        if let Some(source) = self.source_code {
            let mut line = 1u32;
            let mut column = 1u32;
            for (i, ch) in source.char_indices() {
                if i >= offset {
                    break;
                }
                if ch == '\n' {
                    line += 1;
                    column = 1;
                } else {
                    column += 1;
                }
            }
            (line, column)
        } else {
            (1, 1)
        }
    }

    /// Create a `SourceLocation` from a `Node` using the recorded start
    /// offset.
    pub fn node_to_source_location(&self, node: &Node) -> SourceLocation {
        let (line, column) = self.calculate_line_col_from_offset(node.start);
        SourceLocation::new(line, column, node.start as u32, node.end as u32)
    }

    /// Look up the `SourceLocation` for an expression, if one was
    /// recorded during parsing.
    pub fn get_expr_location(&self, expr_ref: &ExprRef) -> Option<SourceLocation> {
        self.core.location_pool.get_expr_location(expr_ref).cloned()
    }

    /// Look up the `SourceLocation` for a statement, if one was
    /// recorded during parsing.
    pub fn get_stmt_location(&self, stmt_ref: &StmtRef) -> Option<SourceLocation> {
        self.core.location_pool.get_stmt_location(stmt_ref).cloned()
    }
}
