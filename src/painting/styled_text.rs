use nu_ansi_term::{Color, Style};

use crate::terminal_extensions::semantic_prompt::{PromptKind, SemanticPromptMarkers};
use crate::Prompt;

use super::utils::strip_ansi;

/// A representation of a buffer with styling, used for doing syntax highlighting
#[derive(Clone)]
pub struct StyledText {
    /// The component, styled parts of the text
    pub buffer: Vec<(Style, String)>,
    /// Byte ranges with colored curly underlines (start, end, color, severity).
    /// Lower severity values are more severe (1 = error, 4 = hint).
    underline_colors: Vec<(usize, usize, Color, u32)>,
}

impl Default for StyledText {
    fn default() -> Self {
        Self::new()
    }
}

impl StyledText {
    /// Construct a new `StyledText`
    pub const fn new() -> Self {
        Self {
            buffer: vec![],
            underline_colors: vec![],
        }
    }

    /// Add a new styled string to the buffer
    pub fn push(&mut self, styled_string: (Style, String)) {
        self.buffer.push(styled_string);
    }

    /// Style range with the provided style (replaces existing style)
    pub fn style_range(&mut self, from: usize, to: usize, new_style: Style) {
        self.transform_style_range(from, to, |_| new_style);
    }

    /// Transform styles in a range using the provided function.
    /// Unlike `style_range` which replaces styles, this preserves and modifies existing styles.
    /// Useful for adding attributes (like underline) while preserving colors.
    pub fn transform_style_range<F>(&mut self, from: usize, to: usize, f: F)
    where
        F: Fn(Style) -> Style,
    {
        let (from, to) = if from > to { (to, from) } else { (from, to) };
        let mut current_idx = 0;
        let mut pair_idx = 0;
        while pair_idx < self.buffer.len() {
            let pair = &mut self.buffer[pair_idx];
            let end_idx = current_idx + pair.1.len();
            enum Position {
                Before,
                In,
                After,
            }
            let start_position = if current_idx < from {
                Position::Before
            } else if current_idx >= to {
                Position::After
            } else {
                Position::In
            };
            let end_position = if end_idx < from {
                Position::Before
            } else if end_idx > to {
                Position::After
            } else {
                Position::In
            };
            match (start_position, end_position) {
                (Position::Before, Position::After) => {
                    let mut in_range = pair.1.split_off(from - current_idx);
                    let after_range = in_range.split_off(to - from);
                    let in_range = (f(pair.0), in_range);
                    let after_range = (pair.0, after_range);
                    self.buffer.insert(pair_idx + 1, in_range);
                    self.buffer.insert(pair_idx + 2, after_range);
                    break;
                }
                (Position::Before, Position::In) => {
                    let in_range = pair.1.split_off(from - current_idx);
                    let transformed_style = f(pair.0);
                    pair_idx += 1;
                    self.buffer.insert(pair_idx, (transformed_style, in_range));
                }
                (Position::In, Position::After) => {
                    let after_range = pair.1.split_off(to - current_idx);
                    let old_style = pair.0;
                    pair.0 = f(old_style);
                    if !after_range.is_empty() {
                        self.buffer.insert(pair_idx + 1, (old_style, after_range));
                    }
                    break;
                }
                (Position::In, Position::In) => pair.0 = f(pair.0),

                (Position::After, _) => break,
                _ => (),
            }
            current_idx = end_idx;
            pair_idx += 1;
        }
    }

    /// Mark a byte range to be rendered with a colored curly underline.
    ///
    /// `severity` is a [`DiagnosticSeverity`](async_lsp::lsp_types::DiagnosticSeverity)
    /// numeric value (1 = error … 4 = hint). Lower values are more severe and
    /// win when ranges overlap.
    ///
    /// The underline color is emitted as raw ANSI (SGR 4:3 + SGR 58) at render
    /// time, independently of `nu_ansi_term::Style`.
    pub fn set_underline_color_range(
        &mut self,
        from: usize,
        to: usize,
        color: Color,
        severity: u32,
    ) {
        self.underline_colors.push((from, to, color, severity));
    }

    /// Render the styled string. We use the insertion point to render around so that
    /// we can properly write out the styled string to the screen and find the correct
    /// place to put the cursor. This assumes a logic that prints the first part of the
    /// string, saves the cursor position, prints the second half, and then restores
    /// the cursor position
    ///
    /// Also inserts the multiline continuation prompt with optional semantic markers
    pub fn render_around_insertion_point(
        &self,
        insertion_point: usize,
        prompt: &dyn Prompt,
        use_ansi_coloring: bool,
        semantic_markers: Option<&dyn SemanticPromptMarkers>,
    ) -> (String, String) {
        let mut current_idx = 0;
        let mut left_string = String::new();
        let mut right_string = String::new();

        let multiline_prompt = prompt.render_prompt_multiline_indicator();
        let prompt_style = Style::new().fg(prompt.get_prompt_multiline_color());

        for pair in &self.buffer {
            let seg_len = pair.1.len();
            if current_idx >= insertion_point {
                let rendered = render_as_string(
                    pair,
                    &prompt_style,
                    &multiline_prompt,
                    semantic_markers,
                );
                right_string.push_str(&wrap_underline_color(&rendered, current_idx, seg_len, &self.underline_colors));
            } else if seg_len + current_idx <= insertion_point {
                let rendered = render_as_string(
                    pair,
                    &prompt_style,
                    &multiline_prompt,
                    semantic_markers,
                );
                left_string.push_str(&wrap_underline_color(&rendered, current_idx, seg_len, &self.underline_colors));
            } else if seg_len + current_idx > insertion_point {
                let offset = insertion_point - current_idx;

                let left_side = pair.1[..offset].to_string();
                let right_side = pair.1[offset..].to_string();

                let left_rendered = render_as_string(
                    &(pair.0, left_side),
                    &prompt_style,
                    &multiline_prompt,
                    semantic_markers,
                );
                left_string.push_str(&wrap_underline_color(&left_rendered, current_idx, offset, &self.underline_colors));
                let right_rendered = render_as_string(
                    &(pair.0, right_side),
                    &prompt_style,
                    &multiline_prompt,
                    semantic_markers,
                );
                right_string.push_str(&wrap_underline_color(&right_rendered, current_idx + offset, seg_len - offset, &self.underline_colors));
            }
            current_idx += seg_len;
        }

        if use_ansi_coloring {
            (left_string, right_string)
        } else {
            (strip_ansi(&left_string), strip_ansi(&right_string))
        }
    }

    /// Apply the ANSI style formatting to the full string.
    pub fn render_simple(&self) -> String {
        let mut current_idx = 0;
        self.buffer
            .iter()
            .map(|(style, text)| {
                let painted = style.paint(text.as_str()).to_string();
                let result = wrap_underline_color(&painted, current_idx, text.len(), &self.underline_colors);
                current_idx += text.len();
                result
            })
            .collect()
    }

    /// Get the unformatted text as a single continuous string.
    pub fn raw_string(&self) -> String {
        self.buffer.iter().map(|(_, str)| str.as_str()).collect()
    }
}

fn render_as_string(
    renderable: &(Style, String),
    prompt_style: &Style,
    multiline_prompt: &str,
    semantic_markers: Option<&dyn SemanticPromptMarkers>,
) -> String {
    let mut rendered = String::new();

    // Build the formatted multiline prompt with optional semantic markers
    let formatted_multiline_prompt = if let Some(markers) = semantic_markers {
        // Wrap multiline indicator with secondary prompt markers:
        // \n + A;k=s + multiline_prompt + B
        format!(
            "\n{}{}{}",
            markers.prompt_start(PromptKind::Secondary),
            multiline_prompt,
            markers.command_input_start()
        )
    } else {
        format!("\n{multiline_prompt}")
    };

    for (line_number, line) in renderable.1.split('\n').enumerate() {
        if line_number != 0 {
            rendered.push_str(&prompt_style.paint(&formatted_multiline_prompt).to_string());
        }
        rendered.push_str(&renderable.0.paint(line).to_string());
    }
    rendered
}

/// If the segment at `byte_offset..byte_offset+len` overlaps with any
/// underline-color range, wrap `painted` with raw ANSI for curly underline
/// (SGR 4:3) and underline color (SGR 58), resetting afterwards (SGR 24 + 59).
///
/// When multiple ranges overlap, the most severe one wins (lowest severity
/// number: 1 = error, 2 = warning, …).
fn wrap_underline_color(
    painted: &str,
    byte_offset: usize,
    len: usize,
    ranges: &[(usize, usize, Color, u32)],
) -> String {
    if len == 0 {
        return painted.to_string();
    }
    let seg_end = byte_offset + len;
    // Pick the color of the most-severe overlapping range.
    let color = ranges
        .iter()
        .filter(|(start, end, _, _)| byte_offset < *end && seg_end > *start)
        .min_by_key(|(_, _, _, sev)| *sev)
        .map(|(_, _, c, _)| *c);
    match color {
        Some(c) => {
            // SGR 4:3 = curly underline, SGR 58;… = underline color
            let set = format!("\x1b[4:3m{}", underline_color_escape(c));
            // SGR 24 = underline off, SGR 59 = default underline color
            let reset = "\x1b[24m\x1b[59m";
            format!("{set}{painted}{reset}")
        }
        None => painted.to_string(),
    }
}

/// Build the SGR 58 escape sequence for a [`Color`].
fn underline_color_escape(color: Color) -> String {
    match color {
        Color::Black => "\x1b[58;5;0m".into(),
        Color::Red => "\x1b[58;5;1m".into(),
        Color::Green => "\x1b[58;5;2m".into(),
        Color::Yellow => "\x1b[58;5;3m".into(),
        Color::Blue => "\x1b[58;5;4m".into(),
        Color::Purple | Color::Magenta => "\x1b[58;5;5m".into(),
        Color::Cyan => "\x1b[58;5;6m".into(),
        Color::White => "\x1b[58;5;7m".into(),
        Color::DarkGray => "\x1b[58;5;8m".into(),
        Color::LightRed => "\x1b[58;5;9m".into(),
        Color::LightGreen => "\x1b[58;5;10m".into(),
        Color::LightYellow => "\x1b[58;5;11m".into(),
        Color::LightBlue => "\x1b[58;5;12m".into(),
        Color::LightPurple | Color::LightMagenta => "\x1b[58;5;13m".into(),
        Color::LightCyan => "\x1b[58;5;14m".into(),
        Color::LightGray => "\x1b[58;5;15m".into(),
        Color::Fixed(n) => format!("\x1b[58;5;{n}m"),
        Color::Rgb(r, g, b) => format!("\x1b[58;2;{r};{g};{b}m"),
        Color::Default => String::new(),
    }
}

#[cfg(test)]
mod test {
    use nu_ansi_term::{Color, Style};

    use crate::StyledText;

    fn get_styled_text_template() -> (super::StyledText, Style, Style) {
        let before_style = Style::new().on(Color::Black);
        let after_style = Style::new().on(Color::Red);
        (
            super::StyledText {
                underline_colors: vec![],
                buffer: vec![
                    (before_style, "aaa".into()),
                    (before_style, "bbb".into()),
                    (before_style, "ccc".into()),
                ],
            },
            before_style,
            after_style,
        )
    }
    #[test]
    fn style_range_partial_update_one_part() {
        let (styled_text_template, before_style, after_style) = get_styled_text_template();
        let mut styled_text = styled_text_template.clone();
        styled_text.style_range(0, 1, after_style);
        assert_eq!(styled_text.buffer[0], (after_style, "a".into()));
        assert_eq!(styled_text.buffer[1], (before_style, "aa".into()));
        assert_eq!(styled_text.buffer[2], (before_style, "bbb".into()));
        assert_eq!(styled_text.buffer[3], (before_style, "ccc".into()));
    }
    #[test]
    fn style_range_complete_update_one_part() {
        let (styled_text_template, before_style, after_style) = get_styled_text_template();
        let mut styled_text = styled_text_template.clone();
        styled_text.style_range(0, 3, after_style);
        assert_eq!(styled_text.buffer[0], (after_style, "aaa".into()));
        assert_eq!(styled_text.buffer[1], (before_style, "bbb".into()));
        assert_eq!(styled_text.buffer[2], (before_style, "ccc".into()));
        assert_eq!(styled_text.buffer.len(), 3);
    }
    #[test]
    fn style_range_update_over_boundary() {
        let (styled_text_template, before_style, after_style) = get_styled_text_template();
        let mut styled_text = styled_text_template;
        styled_text.style_range(0, 5, after_style);
        assert_eq!(styled_text.buffer[0], (after_style, "aaa".into()));
        assert_eq!(styled_text.buffer[1], (after_style, "bb".into()));
        assert_eq!(styled_text.buffer[2], (before_style, "b".into()));
        assert_eq!(styled_text.buffer[3], (before_style, "ccc".into()));
    }
    #[test]
    fn style_range_update_over_part() {
        let (styled_text_template, before_style, after_style) = get_styled_text_template();
        let mut styled_text = styled_text_template;
        styled_text.style_range(1, 7, after_style);
        assert_eq!(styled_text.buffer[0], (before_style, "a".into()));
        assert_eq!(styled_text.buffer[1], (after_style, "aa".into()));
        assert_eq!(styled_text.buffer[2], (after_style, "bbb".into()));
        assert_eq!(styled_text.buffer[3], (after_style, "c".into()));
        assert_eq!(styled_text.buffer[4], (before_style, "cc".into()));
    }
    #[test]
    fn style_range_last_letter() {
        let (_, before_style, after_style) = get_styled_text_template();
        let mut styled_text = StyledText {
            underline_colors: vec![],
            buffer: vec![(before_style, "asdf".into())],
        };
        styled_text.style_range(3, 4, after_style);
        assert_eq!(styled_text.buffer[0], (before_style, "asd".into()));
        assert_eq!(styled_text.buffer[1], (after_style, "f".into()));
    }
    #[test]
    fn style_range_from_second_to_last() {
        let (_, before_style, after_style) = get_styled_text_template();
        let mut styled_text = StyledText {
            underline_colors: vec![],
            buffer: vec![(before_style, "asdf".into())],
        };
        styled_text.style_range(2, 3, after_style);
        assert_eq!(styled_text.buffer[0], (before_style, "as".into()));
        assert_eq!(styled_text.buffer[1], (after_style, "d".into()));
        assert_eq!(styled_text.buffer[2], (before_style, "f".into()));
    }
    #[test]
    fn regression_style_range_cargo_run() {
        let (_, before_style, after_style) = get_styled_text_template();
        let mut styled_text = StyledText {
            underline_colors: vec![],
            buffer: vec![
                (before_style, "cargo".into()),
                (before_style, " ".into()),
                (before_style, "run".into()),
            ],
        };
        styled_text.style_range(8, 7, after_style);
        assert_eq!(styled_text.buffer[0], (before_style, "cargo".into()));
        assert_eq!(styled_text.buffer[1], (before_style, " ".into()));
        assert_eq!(styled_text.buffer[2], (before_style, "r".into()));
        assert_eq!(styled_text.buffer[3], (after_style, "u".into()));
        assert_eq!(styled_text.buffer[4], (before_style, "n".into()));
    }

    #[test]
    fn test_render_multiline_without_semantic_markers() {
        let style = Style::new();
        let renderable = (style, "line1\nline2".to_string());
        let prompt_style = Style::new();
        let multiline_prompt = "::: ";

        // Without semantic markers, just get newline + multiline prompt
        let result = super::render_as_string(&renderable, &prompt_style, multiline_prompt, None);
        assert!(result.contains("\n::: "));
        assert!(!result.contains("\x1b]133;A;k=s"));
    }

    #[test]
    fn test_render_multiline_with_semantic_markers() {
        use crate::terminal_extensions::semantic_prompt::Osc133Markers;
        let style = Style::new();
        let renderable = (style, "line1\nline2".to_string());
        let prompt_style = Style::new();
        let multiline_prompt = "::: ";
        let markers = Osc133Markers;

        // With semantic markers, should wrap multiline prompt with A;k=s and B
        let result =
            super::render_as_string(&renderable, &prompt_style, multiline_prompt, Some(&markers));
        // The result should contain the secondary prompt marker before ::: and B after
        assert!(result.contains("\x1b]133;A;k=s\x1b\\"));
        assert!(result.contains("\x1b]133;B\x1b\\"));
    }

    #[test]
    fn test_render_single_line_no_markers_emitted() {
        use crate::terminal_extensions::semantic_prompt::Osc133Markers;
        let style = Style::new();
        let renderable = (style, "single line".to_string());
        let prompt_style = Style::new();
        let multiline_prompt = "::: ";
        let markers = Osc133Markers;

        // Single line should not emit any markers
        let result =
            super::render_as_string(&renderable, &prompt_style, multiline_prompt, Some(&markers));
        assert!(!result.contains("\x1b]133;A;k=s"));
        assert!(!result.contains("\x1b]133;B"));
    }
}
