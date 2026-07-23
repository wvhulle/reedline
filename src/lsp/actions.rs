//! Code actions support for LSP integration.
//!
//! This module provides helper functions for converting between byte spans
//! and LSP positions/ranges.

use super::diagnostic::ByteBufferSpan;
use async_lsp::lsp_types::{self, Range};

/// Convert a byte span to an LSP Range.
pub(super) fn span_to_range(content: &str, span: ByteBufferSpan) -> Range {
    Range {
        start: offset_to_position(content, span.start),
        end: offset_to_position(content, span.end),
    }
}

/// Convert a byte offset to an LSP Position.
fn offset_to_position(content: &str, offset: usize) -> lsp_types::Position {
    let (line, character) = content
        .char_indices()
        .take_while(|(i, _)| *i < offset)
        .fold((0u32, 0u32), |(line, col), (_, c)| {
            if c == '\n' {
                (line + 1, 0)
            } else {
                (line, col + 1)
            }
        });

    lsp_types::Position { line, character }
}
