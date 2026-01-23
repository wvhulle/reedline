//! Menu for displaying and applying diagnostic fixes.
//!
//! This menu shows available code fixes for diagnostics at the cursor position,
//! with a simple inline format: replacement text followed by title in parentheses.
//! The menu is positioned below the text being replaced, aligned with the anchor column.

use itertools::Itertools;
use lsp_types::{CodeAction, TextEdit};
use nu_ansi_term::{ansi::RESET, Style};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use super::{Menu, MenuBuilder, MenuEvent, MenuSettings};
use crate::Highlighter;
use crate::{
    core_editor::Editor,
    lsp::{range_to_span, LspCommandSender, Span},
    painting::Painter,
    Completer, Suggestion, UndoBehavior,
};

// Necessary because of indicator text of two characters `> ` to the left of selected menu item
const LEFT_PADDING: u16 = 2;

/// A single text edit with span, replacement, and original text.
#[derive(Debug, Clone)]
pub struct TextEditInfo {
    /// Byte span in the buffer
    pub span: Span,
    /// Replacement text (empty for deletions)
    pub replacement: String,
    /// Original text at this span (for display)
    pub original: String,
}

/// The action to perform for a fix.
#[derive(Debug, Clone)]
pub enum FixAction {
    /// Text edits to apply to the buffer
    TextEdits(Vec<TextEditInfo>),
    /// LSP command to execute on the server
    Command {
        command: String,
        arguments: Vec<Value>,
    },
}

/// Pre-computed fix with byte offsets for buffer manipulation.
#[derive(Debug, Clone)]
struct FixInfo {
    /// Title of the fix (shown in the menu)
    title: String,
    /// The action to perform
    action: FixAction,
}

/// Working details calculated during layout
#[derive(Default)]
struct WorkingDetails {
    /// Space to the left of the menu (includes prompt width + anchor offset)
    space_left: u16,
    /// Cursor column from set_cursor_pos (includes prompt width)
    cursor_col: u16,
}

/// Menu for displaying and applying diagnostic fixes.
///
/// Shows fix options as simple lines: `>replacement_text (title)`
pub struct DiagnosticFixMenu {
    /// Menu settings (name, color, etc.)
    settings: MenuSettings,
    /// Whether the menu is active
    active: bool,
    /// Available fixes with pre-computed byte offsets
    fixes: Vec<FixInfo>,
    /// Selected index
    selected: usize,
    /// Number of values to skip for scrolling
    skip_values: usize,
    /// Working details calculated during layout
    working_details: WorkingDetails,
    /// Max height of the menu
    max_height: u16,
    /// Anchor column position (start of text being replaced)
    anchor_col: u16,
    /// Command sender for executing LSP commands
    command_sender: Option<LspCommandSender>,
}

impl Default for DiagnosticFixMenu {
    fn default() -> Self {
        Self {
            settings: MenuSettings::default().with_name("diagnostic_fix_menu"),
            active: false,
            fixes: Vec::new(),
            selected: 0,
            skip_values: 0,
            working_details: WorkingDetails::default(),
            max_height: 10,
            anchor_col: 0,
            command_sender: None,
        }
    }
}

impl MenuBuilder for DiagnosticFixMenu {
    fn settings_mut(&mut self) -> &mut MenuSettings {
        &mut self.settings
    }
}

impl DiagnosticFixMenu {
    /// Update the available fixes from LSP code actions.
    ///
    /// Converts LSP ranges to byte offsets using the provided content.
    /// Supports both edit-based and command-based actions.
    pub fn set_fixes(&mut self, actions: Vec<CodeAction>, content: &str, anchor_col: u16) {
        self.fixes = actions
            .into_iter()
            .filter_map(|action| {
                // Try edit-based action first
                if let Some(edits) = extract_text_edits(&action) {
                    let edits: Vec<TextEditInfo> = edits
                        .into_iter()
                        .map(|edit| {
                            let span = range_to_span(content, &edit.range);
                            let original =
                                content.get(span.start..span.end).unwrap_or("").to_string();
                            TextEditInfo {
                                span,
                                replacement: edit.new_text,
                                original,
                            }
                        })
                        .collect();

                    if !edits.is_empty() {
                        return Some(FixInfo {
                            title: action.title,
                            action: FixAction::TextEdits(edits),
                        });
                    }
                }

                // Fall back to command-based action
                if let Some(cmd) = action.command {
                    return Some(FixInfo {
                        title: action.title,
                        action: FixAction::Command {
                            command: cmd.command,
                            arguments: cmd.arguments.unwrap_or_default(),
                        },
                    });
                }

                None
            })
            .collect();

        self.selected = 0;
        self.skip_values = 0;
        self.anchor_col = anchor_col;
    }

    /// Check if there are any fixes available.
    pub fn has_fixes(&self) -> bool {
        !self.fixes.is_empty()
    }

    /// Set the command sender for executing LSP commands.
    pub fn set_command_sender(&mut self, sender: LspCommandSender) {
        self.command_sender = Some(sender);
    }

    /// Get the currently selected fix.
    fn get_selected_fix(&self) -> Option<&FixInfo> {
        self.fixes.get(self.selected)
    }

    /// Format a single fix line with optional syntax highlighting
    fn format_fix_line(
        &self,
        fix: &FixInfo,
        index: usize,
        use_ansi_coloring: bool,
        highlighter: Option<&dyn Highlighter>,
    ) -> String {
        let is_selected = index == self.selected;
        let indicator = if is_selected { "> " } else { "  " };

        let title_style = if use_ansi_coloring {
            Style::new().italic()
        } else {
            Style::new()
        };

        match &fix.action {
            FixAction::TextEdits(edits) => {
                // "Fix all" type actions: multiple edits, show title only
                if edits.len() > 1 {
                    return format!("{indicator}{}{}{RESET}", title_style.prefix(), fix.title,);
                }

                let first_edit = edits.first();
                let replacement_text = first_edit.map_or("", |e| e.replacement.as_str());
                let original_text = first_edit.map_or("", |e| e.original.as_str());

                if replacement_text.is_empty() {
                    // Deletion: show original text with strikethrough
                    let strikethrough_style = if use_ansi_coloring {
                        Style::new().strikethrough()
                    } else {
                        Style::new()
                    };

                    format!(
                        "{indicator}{}{}{} {}({}){RESET}",
                        strikethrough_style.prefix(),
                        original_text,
                        strikethrough_style.suffix(),
                        title_style.prefix(),
                        fix.title,
                    )
                } else {
                    // Replacement: show new text with syntax highlighting
                    let styled_replacement = if use_ansi_coloring {
                        if let Some(h) = highlighter {
                            let styled = h.highlight(replacement_text, replacement_text.len());
                            styled.render_simple()
                        } else {
                            replacement_text.to_string()
                        }
                    } else {
                        replacement_text.to_string()
                    };

                    format!(
                        "{indicator}{styled_replacement} {}({}){RESET}",
                        title_style.prefix(),
                        fix.title,
                    )
                }
            }
            FixAction::Command { .. } => {
                // Command-only: show title without parentheses
                format!("{indicator}{}{}{RESET}", title_style.prefix(), fix.title,)
            }
        }
    }

    /// Move selection forward, wrapping around
    fn select_next(&mut self) {
        if self.fixes.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.fixes.len();
        self.adjust_scroll_forward();
    }

    /// Move selection backward, wrapping around
    fn select_previous(&mut self) {
        if self.fixes.is_empty() {
            return;
        }
        self.selected = self.selected.checked_sub(1).unwrap_or(self.fixes.len() - 1);
        self.adjust_scroll_backward();
    }

    /// Adjust scroll position when moving forward
    fn adjust_scroll_forward(&mut self) {
        let visible_items = self.max_height as usize;
        if self.selected >= self.skip_values + visible_items {
            self.skip_values = self.selected.saturating_sub(visible_items - 1);
        } else if self.selected < self.skip_values {
            self.skip_values = self.selected;
        }
    }

    /// Adjust scroll position when moving backward
    fn adjust_scroll_backward(&mut self) {
        if self.selected < self.skip_values {
            self.skip_values = self.selected;
        }
    }
}

/// Extract text edits from a code action's workspace edit.
fn extract_text_edits(action: &CodeAction) -> Option<Vec<TextEdit>> {
    action
        .edit
        .as_ref()?
        .changes
        .as_ref()?
        .values()
        .next()
        .cloned()
}

impl Menu for DiagnosticFixMenu {
    fn settings(&self) -> &MenuSettings {
        &self.settings
    }

    fn is_active(&self) -> bool {
        self.active
    }

    fn can_quick_complete(&self) -> bool {
        true
    }

    fn can_partially_complete(
        &mut self,
        _values_updated: bool,
        _editor: &mut Editor,
        _completer: &mut dyn Completer,
    ) -> bool {
        false
    }

    fn menu_event(&mut self, event: MenuEvent) {
        match event {
            MenuEvent::Activate(_) => {
                self.active = true;
                self.selected = 0;
                self.skip_values = 0;
            }
            MenuEvent::Deactivate => self.active = false,
            // Handle both NextElement (Tab) and MoveDown (arrow key)
            MenuEvent::NextElement | MenuEvent::MoveDown => self.select_next(),
            // Handle both PreviousElement (Shift+Tab) and MoveUp (arrow key)
            MenuEvent::PreviousElement | MenuEvent::MoveUp => self.select_previous(),
            _ => {}
        }
    }

    fn update_values(&mut self, _editor: &mut Editor, _completer: &mut dyn Completer) {
        // Fixes are set via set_fixes(), nothing to update from completer
    }

    fn update_working_details(
        &mut self,
        editor: &mut Editor,
        _completer: &mut dyn Completer,
        _painter: &Painter,
    ) {
        // Calculate menu position: prompt_width + anchor_col
        // cursor_col = prompt_width + text_before_cursor_width (mod terminal width)
        // So: prompt_width = cursor_col - text_before_cursor_width
        let line_buffer = editor.line_buffer();
        let cursor_visual_width = line_buffer.get_buffer()[..line_buffer
            .insertion_point()
            .min(line_buffer.get_buffer().len())]
            .width() as u16;

        self.working_details.space_left = self
            .working_details
            .cursor_col
            .saturating_sub(cursor_visual_width)
            .saturating_add(self.anchor_col)
            .saturating_sub(LEFT_PADDING);
    }

    fn replace_in_buffer(&self, editor: &mut Editor) {
        let Some(fix) = self.get_selected_fix() else {
            return;
        };

        match &fix.action {
            FixAction::TextEdits(edits) => {
                // Sort edits by start position descending to apply from end to start
                let mut edits = edits.clone();
                edits.sort_by_key(|e| std::cmp::Reverse(e.span.start));

                let mut line_buffer = editor.line_buffer().clone();

                // Apply all edits using fold
                let new_buffer =
                    edits
                        .iter()
                        .fold(line_buffer.get_buffer().to_string(), |mut buf, edit| {
                            let start = edit.span.start.min(buf.len());
                            let end = edit.span.end.min(buf.len());
                            buf.replace_range(start..end, &edit.replacement);
                            buf
                        });

                // Place cursor at end of first edit
                let cursor_pos = edits
                    .last() // After sorting descending, last is first original edit
                    .map(|edit| edit.span.start + edit.replacement.len())
                    .unwrap_or_else(|| line_buffer.insertion_point());

                line_buffer.set_buffer(new_buffer);
                line_buffer.set_insertion_point(cursor_pos.min(line_buffer.get_buffer().len()));
                editor.set_line_buffer(line_buffer, UndoBehavior::CreateUndoPoint);
            }
            FixAction::Command { command, arguments } => {
                // Execute the command via the LSP provider
                if let Some(sender) = &self.command_sender {
                    sender.execute_command(command.clone(), arguments.clone());
                }
            }
        }
    }

    fn min_rows(&self) -> u16 {
        self.fixes.len() as u16
    }

    fn get_values(&self) -> &[Suggestion] {
        // Return empty - we don't use Suggestion directly
        &[]
    }

    fn menu_required_lines(&self, _terminal_columns: u16) -> u16 {
        (self.fixes.len() as u16).min(self.max_height)
    }

    fn menu_string(&self, available_lines: u16, use_ansi_coloring: bool) -> String {
        self.menu_string_with_highlighter(available_lines, use_ansi_coloring, None)
    }

    fn menu_string_with_highlighter(
        &self,
        available_lines: u16,
        use_ansi_coloring: bool,
        highlighter: Option<&dyn Highlighter>,
    ) -> String {
        if self.fixes.is_empty() {
            return String::from("No fixes available");
        }

        let visible_count = (available_lines.min(self.max_height)) as usize;
        let left_padding = " ".repeat(self.working_details.space_left as usize);

        self.fixes
            .iter()
            .enumerate()
            .skip(self.skip_values)
            .take(visible_count)
            .map(|(idx, fix)| {
                format!(
                    "{left_padding}{}",
                    self.format_fix_line(fix, idx, use_ansi_coloring, highlighter)
                )
            })
            .join("\r\n")
    }

    fn set_cursor_pos(&mut self, pos: (u16, u16)) {
        self.working_details.cursor_col = pos.0;
    }
}
