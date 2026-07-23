use nu_ansi_term::{Color, Style};

pub use async_lsp::lsp_types::{CodeAction, Diagnostic, DiagnosticSeverity, Range, TextEdit};

/// Get the semantic ANSI color for a diagnostic severity.
fn severity_color(severity: DiagnosticSeverity) -> Color {
    match severity {
        DiagnosticSeverity::ERROR => Color::Red,
        DiagnosticSeverity::WARNING => Color::Yellow,
        DiagnosticSeverity::INFORMATION => Color::Blue,
        DiagnosticSeverity::HINT => Color::DarkGray,
        _ => Color::DarkGray,
    }
}

/// Get a style for diagnostic messages by severity.
pub fn message_style(severity: DiagnosticSeverity) -> Style {
    Style::new().fg(severity_color(severity))
}

/// Get the underline color for a diagnostic severity.
pub fn underline_color(severity: DiagnosticSeverity) -> Color {
    severity_color(severity)
}

/// A byte span within the input buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ByteBufferSpan {
    /// Start byte offset (inclusive)
    pub start: usize,
    /// End byte offset (exclusive)
    pub end: usize,
}

impl ByteBufferSpan {
    /// Convert an LSP Range to a byte span.
    pub fn from_range(content: &str, range: &Range) -> Self {
        let start = position_to_offset(content, &range.start);
        let end = position_to_offset(content, &range.end);
        Self { start, end }
    }

    /// Get the visual column of the start position.
    pub fn start_column(&self, content: &str) -> usize {
        byte_offset_to_column(content, self.start)
    }

    /// Get the visual column of the end position.
    pub fn end_column(&self, content: &str) -> usize {
        byte_offset_to_column(content, self.end)
    }
}

/// Convert a byte offset to a visual column position.
///
/// Accounts for unicode character widths (e.g., CJK characters are 2 columns wide).
fn byte_offset_to_column(s: &str, byte_offset: usize) -> usize {
    use unicode_width::UnicodeWidthChar;
    s.char_indices()
        .take_while(|(pos, _)| *pos < byte_offset.min(s.len()))
        .map(|(_, ch)| ch.width().unwrap_or(0))
        .sum()
}

/// Convert an LSP Position to a byte offset.
fn position_to_offset(content: &str, pos: &async_lsp::lsp_types::Position) -> usize {
    let target_line = pos.line as usize;
    content
        .lines()
        .enumerate()
        .scan(0usize, |offset, (i, line)| {
            let current_offset = *offset;
            *offset += line.len() + 1;
            Some((i, line, current_offset))
        })
        .find(|(i, _, _)| *i == target_line)
        .map(|(_, line, offset)| offset + (pos.character as usize).min(line.len()))
        .unwrap_or(content.len())
}

/// Format diagnostic messages as right-aligned colored lines.
///
/// Long messages are wrapped at word boundaries so each line fits within
/// `terminal_columns`, and every line is right-aligned.
/// Underlines on the buffer text are applied separately via `StyledText::transform_style_range`.
pub fn format_diagnostic_messages(
    diagnostics: &[Diagnostic],
    terminal_columns: usize,
    use_ansi_coloring: bool,
) -> String {
    use unicode_width::UnicodeWidthStr;

    use async_lsp::lsp_types::NumberOrString;

    let cols = terminal_columns.max(1);
    diagnostics
        .iter()
        .flat_map(|d| {
            let severity = d.severity.unwrap_or(DiagnosticSeverity::WARNING);
            let msg = match &d.code {
                Some(NumberOrString::String(s)) => format!("{} [{}]", d.message, s),
                Some(NumberOrString::Number(n)) => format!("{} [{}]", d.message, n),
                None => d.message.clone(),
            };
            wrap_words(&msg, cols).into_iter().map(move |line| {
                let pad = cols.saturating_sub(line.width());
                let spaces = " ".repeat(pad);
                if use_ansi_coloring {
                    format!("{spaces}{}", message_style(severity).paint(&line))
                } else {
                    format!("{spaces}{line}")
                }
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split text into lines that fit within `max_width` columns, breaking at word boundaries.
fn wrap_words(text: &str, max_width: usize) -> Vec<String> {
    use itertools::Itertools;
    use unicode_width::UnicodeWidthStr;

    text.split_whitespace()
        .scan(0usize, |width, word| {
            let new_width = if *width == 0 {
                word.width()
            } else {
                *width + 1 + word.width()
            };
            let fits = *width == 0 || new_width <= max_width;
            *width = if fits { new_width } else { word.width() };
            Some((fits, word))
        })
        .peekable()
        .batching(|iter| {
            iter.next().map(|(_, first)| {
                std::iter::once(first)
                    .chain(iter.peeking_take_while(|(fits, _)| *fits).map(|(_, w)| w))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        })
        .collect()
}
