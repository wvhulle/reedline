//! Engine integration helpers for LSP diagnostics.
//!
//! This module provides functions that integrate LSP diagnostics with the
//! Reedline engine, keeping the LSP-specific logic separate from the core engine.

use async_lsp::lsp_types::Range;

use super::{
    diagnostic::{format_diagnostic_messages, underline_color, ByteBufferSpan},
    LspDiagnosticsProvider,
};
use crate::{
    menu::DiagnosticFixMenu, Highlighter, Menu, MenuEvent, Reedline, ReedlineMenu, StyledText,
};

/// Map [`DiagnosticSeverity`] to a comparable rank (lower = more severe).
fn severity_rank(s: async_lsp::lsp_types::DiagnosticSeverity) -> u32 {
    use async_lsp::lsp_types::DiagnosticSeverity;
    if s == DiagnosticSeverity::ERROR {
        1
    } else if s == DiagnosticSeverity::WARNING {
        2
    } else if s == DiagnosticSeverity::INFORMATION {
        3
    } else {
        4
    }
}

/// Format diagnostic messages as right-aligned colored lines for display below the prompt.
pub fn format_diagnostics_for_prompt(
    provider: &mut LspDiagnosticsProvider,
    terminal_columns: usize,
    use_ansi_coloring: bool,
) -> String {
    let diagnostics = provider.diagnostics();

    if diagnostics.is_empty() {
        return String::new();
    }

    format_diagnostic_messages(diagnostics, terminal_columns, use_ansi_coloring)
}

/// Apply underline styling to buffer text at diagnostic spans.
///
/// Each diagnostic span gets underlined in the severity color while preserving
/// existing syntax highlighting.
pub fn apply_diagnostic_underlines(
    styled_text: &mut StyledText,
    providers: &mut [LspDiagnosticsProvider],
    buffer: &str,
) {
    for provider in providers.iter_mut() {
        for diagnostic in provider.diagnostics() {
            let severity = diagnostic
                .severity
                .unwrap_or(async_lsp::lsp_types::DiagnosticSeverity::WARNING);
            let span = ByteBufferSpan::from_range(buffer, &diagnostic.range);
            if span.start < span.end && span.end <= buffer.len() {
                let rank = severity_rank(severity);
                styled_text.set_underline_color_range(
                    span.start,
                    span.end,
                    underline_color(severity),
                    rank,
                );
            }
        }
    }
}

/// Create a diagnostic fix menu for code actions at the cursor position.
///
/// Aggregates code actions from all providers that have a diagnostic at the
/// cursor position. Returns `Some(ReedlineMenu)` if any actions are available.
///
/// When a highlighter is provided, the fix menu pre-highlights replacement text
/// at setup time, avoiding repeated highlighting work on each render pass.
pub fn create_diagnostic_fix_menu(
    providers: &mut [LspDiagnosticsProvider],
    cursor_pos: usize,
    content: &str,
    highlighter: Option<&dyn Highlighter>,
) -> Option<ReedlineMenu> {
    let mut all_code_actions = Vec::new();

    for (idx, provider) in providers.iter_mut().enumerate() {
        let diagnostic_span = provider
            .diagnostics()
            .iter()
            .map(|d| {
                let range: &Range = &d.range;
                ByteBufferSpan::from_range(content, range)
            })
            .find(|span| span.start <= cursor_pos && cursor_pos <= span.end);

        let span = diagnostic_span.unwrap_or_else(|| {
            ByteBufferSpan { start: cursor_pos, end: cursor_pos }
        });

        let code_actions = provider.code_actions(content, span);
        if !code_actions.is_empty() {
            all_code_actions.extend(code_actions.into_iter().map(|a| (idx, a)));
        }
    }

    if all_code_actions.is_empty() {
        return None;
    }

    let mut fix_menu = DiagnosticFixMenu::default();
    fix_menu.set_fixes(all_code_actions, content, highlighter);

    let mut menu = ReedlineMenu::EngineCompleter(Box::new(fix_menu));
    menu.menu_event(MenuEvent::Activate(false));

    Some(menu)
}

impl Reedline {
    /// Whether the event loop should use polling mode for LSP wake signals.
    pub(crate) fn needs_lsp_polling(&self) -> bool {
        !self.lsp_providers.is_empty()
    }

    /// Check if any LSP provider has new diagnostics ready, triggering a repaint.
    pub(crate) fn check_lsp_wake(&mut self) -> bool {
        self.lsp_providers.iter_mut().any(|p| p.check_wake())
    }
}
