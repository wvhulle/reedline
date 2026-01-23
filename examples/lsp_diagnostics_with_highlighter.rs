//! Example demonstrating LSP diagnostics integration with syntax highlighting.
//!
//! This example combines the LSP diagnostics feature with custom syntax highlighting.
//! The diagnostic fix menu will display replacement text with syntax highlighting
//! applied, making it easier to see what code will be inserted.
//!
//! Run with:
//!   REEDLINE_LS="nu-lint --lsp" cargo run --example lsp_diagnostics_with_highlighter --features lsp_diagnostics
//!
//! Prerequisites:
//! - An LSP server that supports diagnostics and code actions (e.g., nu-lint for nushell)
//!
//! Try typing nushell code with issues like:
//! - `let x = 1` (unused variable warning)
//! - `echo "hello"` (deprecated command)
//!
//! Press Alt+f or Ctrl+. to open the fix menu when cursor is on a diagnostic.
//! The replacement text in the menu will be syntax-highlighted!

use std::{env::var, io};

use crossterm::event::{KeyCode, KeyModifiers};
use reedline::{
    default_emacs_keybindings, DefaultPrompt, Emacs, ExampleHighlighter, Keybindings, LspConfig,
    LspDiagnosticsProvider, Reedline, ReedlineEvent, Signal,
};

fn main() -> io::Result<()> {
    // Use the same env var as nu-cli for consistency
    let Some(command) = var("REEDLINE_LS").ok() else {
        eprintln!("Error: REEDLINE_LS environment variable not set.");
        eprintln!("Set it to the full LSP server command (e.g., \"nu-lint --lsp\").");
        eprintln!();
        eprintln!("Example: REEDLINE_LS=\"nu-lint --lsp\" cargo run --example lsp_diagnostics_with_highlighter --features lsp_diagnostics");
        std::process::exit(1);
    };

    let config = LspConfig {
        command,
        timeout_ms: 100,
        uri_scheme: "repl".to_string(),
    };

    // Create the diagnostics provider
    let diagnostics = LspDiagnosticsProvider::new(config);

    // Create a custom highlighter with some example commands
    // In a real application, this would be replaced with a language-specific
    // highlighter (like NuHighlighter for nushell)
    let commands = vec![
        "let".into(),
        "mut".into(),
        "const".into(),
        "def".into(),
        "if".into(),
        "else".into(),
        "for".into(),
        "loop".into(),
        "while".into(),
        "match".into(),
        "print".into(),
        "echo".into(),
        "ls".into(),
        "cd".into(),
        "pwd".into(),
        "mkdir".into(),
        "rm".into(),
        "cp".into(),
        "mv".into(),
        "cat".into(),
        "open".into(),
        "save".into(),
        "where".into(),
        "select".into(),
        "get".into(),
        "each".into(),
        "filter".into(),
        "sort-by".into(),
        "reverse".into(),
        "length".into(),
        "first".into(),
        "last".into(),
        "skip".into(),
        "take".into(),
        "append".into(),
        "prepend".into(),
        "str".into(),
        "into".into(),
        "from".into(),
        "to".into(),
    ];
    let highlighter = Box::new(ExampleHighlighter::new(commands));

    // Set up keybindings with the diagnostic fix menu
    let mut keybindings = default_emacs_keybindings();
    add_diagnostic_fix_keybinding(&mut keybindings);

    let edit_mode = Box::new(Emacs::new(keybindings));

    // Create reedline with both LSP diagnostics and syntax highlighting
    // The diagnostic fix menu will now show syntax-highlighted replacement text!
    let mut line_editor = Reedline::create()
        .with_lsp_diagnostics(diagnostics)
        .with_highlighter(highlighter)
        .with_edit_mode(edit_mode);

    let prompt = DefaultPrompt::default();

    println!("LSP Diagnostics with Syntax Highlighting Demo");
    println!("==============================================");
    println!();
    println!("Type code to see diagnostics as underlines while typing.");
    println!("The input will be syntax-highlighted (known commands in green).");
    println!();
    println!("Press Alt+f or Ctrl+. to open the fix menu when on a diagnostic.");
    println!("Notice how the replacement text in the fix menu is also highlighted!");
    println!();
    println!("Press Ctrl+C to exit.");
    println!();

    loop {
        match line_editor.read_line(&prompt)? {
            Signal::Success(buffer) => {
                if buffer.trim() == "exit" {
                    break;
                }
                println!("You entered: {buffer}");
            }
            Signal::CtrlD | Signal::CtrlC => {
                println!("\nGoodbye!");
                break;
            }
        }
    }

    Ok(())
}

/// Add keybinding for the diagnostic fix menu (Alt+f and Ctrl+.)
fn add_diagnostic_fix_keybinding(keybindings: &mut Keybindings) {
    keybindings.add_binding(
        KeyModifiers::ALT,
        KeyCode::Char('f'),
        ReedlineEvent::OpenDiagnosticFixMenu,
    );
    keybindings.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('.'),
        ReedlineEvent::OpenDiagnosticFixMenu,
    );
    // Add Tab/Shift-Tab for menu navigation
    keybindings.add_binding(KeyModifiers::NONE, KeyCode::Tab, ReedlineEvent::MenuNext);
    keybindings.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::BackTab,
        ReedlineEvent::MenuPrevious,
    );
}
