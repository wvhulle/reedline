//! Engine integration helpers for LSP diagnostics.
//!
//! This module provides functions that integrate LSP diagnostics with the
//! Reedline engine, keeping the LSP-specific logic separate from the core engine.

use lsp_types::Diagnostic;
use unicode_width::UnicodeWidthStr;

use super::{
    diagnostic::{format_diagnostic_messages, range_to_span, Span},
    LspDiagnosticsProvider,
};
use crate::{menu::DiagnosticFixMenu, Highlighter, Menu, MenuEvent, Prompt, ReedlineMenu};

/// Strip ANSI escape sequences from a string.
///
/// Prompts contain color codes like `\x1b[32m` which would incorrectly inflate
/// width calculations. Uses the strip_ansi_escapes crate to handle all escape
/// sequence types (SGR, OSC, etc).
fn strip_ansi(s: &str) -> String {
    String::from_utf8(strip_ansi_escapes::strip(s)).unwrap_or_else(|_| s.to_string())
}

/// Format diagnostic messages for display below the prompt.
///
/// Renders diagnostics with vertical connecting lines and handlebars spanning the diagnostic:
/// ```text
/// ╎ ╰────╯ Unnecessary '^' prefix on external command 'head'
/// ╰ Use 'first N' to get the first N items
/// ```
pub fn format_diagnostics_for_prompt(
    provider: &mut LspDiagnosticsProvider,
    buffer: &str,
    prompt: &dyn Prompt,
    prompt_edit_mode: crate::PromptEditMode,
    use_ansi_coloring: bool,
) -> String {
    let diagnostics: Vec<Diagnostic> = provider.diagnostics().to_vec();

    if diagnostics.is_empty() {
        return String::new();
    }

    // Calculate prompt width (last line of prompt + indicator)
    // Strip ANSI escape sequences before measuring - they have no visual width
    let prompt_left = prompt.render_prompt_left();
    let prompt_indicator = prompt.render_prompt_indicator(prompt_edit_mode);
    let last_prompt_line = prompt_left.lines().last().unwrap_or("");
    let prompt_width = strip_ansi(last_prompt_line).width() + strip_ansi(&prompt_indicator).width();

    format_diagnostic_messages(&diagnostics, buffer, prompt_width, use_ansi_coloring)
}

/// Create a diagnostic fix menu for code actions at the cursor position.
///
/// Returns `Some(ReedlineMenu)` if there are code actions available,
/// `None` if there are no fixes at the cursor position.
///
/// When a highlighter is provided, the fix menu pre-highlights replacement text
/// at setup time, avoiding repeated highlighting work on each render pass.
pub fn create_diagnostic_fix_menu(
    provider: &mut LspDiagnosticsProvider,
    cursor_pos: usize,
    content: &str,
    highlighter: Option<&dyn Highlighter>,
) -> Option<ReedlineMenu> {
    // Find diagnostics at cursor position to determine the span for code actions
    let diagnostic_span = provider
        .diagnostics()
        .iter()
        .find(|d| {
            let span = range_to_span(content, &d.range);
            span.start <= cursor_pos && cursor_pos <= span.end
        })
        .map(|d| range_to_span(content, &d.range));

    let span = diagnostic_span.unwrap_or_else(|| {
        // No diagnostic at cursor, use cursor position as a point
        Span::new(cursor_pos, cursor_pos)
    });

    // Request code actions from the LSP server
    let code_actions = provider.code_actions(content, span);

    if code_actions.is_empty() {
        return None;
    }

    // Calculate the anchor column based on the span start
    let anchor_col = if span.start <= content.len() {
        content[..span.start].width() as u16
    } else {
        0
    };

    // Create a new menu with fixes, positioned at the start of the diagnostic span
    let mut fix_menu = DiagnosticFixMenu::default();
    fix_menu.set_fixes(code_actions, content, anchor_col, highlighter);
    fix_menu.set_command_sender(provider.command_sender());

    let mut menu = ReedlineMenu::EngineCompleter(Box::new(fix_menu));
    menu.menu_event(MenuEvent::Activate(false));

    Some(menu)
}
