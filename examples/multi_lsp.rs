//! Example demonstrating multiple LSP servers running simultaneously.
//!
//! This spawns two LSP servers and displays diagnostics from both inline.
//!
//! Run with defaults (ast-grep + bash-language-server):
//!   cargo run --example multi_lsp --features lsp_diagnostics
//!
//! Or specify custom servers as pairs of <command> <language_id>:
//!   cargo run --example multi_lsp --features lsp_diagnostics -- \
//!     "nu-lint --lsp" nushell "ast-grep lsp" bash
//!
//! Try typing:
//! - `git commit` (ast-grep: suggest jj equivalent)
//! - `echo $undefined` (bash-language-server: warnings)

use crossterm::event::{KeyCode, KeyModifiers};
use reedline::{
    default_emacs_keybindings, DefaultPrompt, Emacs, Keybindings, LspConfig,
    LspDiagnosticsProvider, Reedline, ReedlineEvent, Signal,
};
use std::io;

fn main() -> io::Result<()> {
    let log_file = std::fs::File::create("/tmp/reedline-lsp.log")
        .expect("failed to create /tmp/reedline-lsp.log");
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Debug)
        .target(env_logger::Target::Pipe(Box::new(log_file)))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.len() % 3 != 0 {
        eprintln!("Usage: multi_lsp [<command> <language_id> <uri_scheme> ...]");
        eprintln!();
        eprintln!("uri_scheme: \"file\" (default, works with all servers) or \"repl\" (for servers that filter REPL-only actions)");
        std::process::exit(1);
    }

    let defaults: Vec<String> = vec![
        "nu-lint --lsp".into(),
        "nushell".into(),
        "repl".into(),
        "nu-lint --lsp".into(),
        "nushell".into(),
        "repl".into(),
    ];
    let args = if args.is_empty() { &defaults } else { &args };

    let mut keybindings = default_emacs_keybindings();
    add_diagnostic_fix_keybinding(&mut keybindings);
    let edit_mode = Box::new(Emacs::new(keybindings));

    let mut line_editor = Reedline::create().with_edit_mode(edit_mode);

    for triple in args.chunks(3) {
        let command = &triple[0];
        let language_id = &triple[1];
        let uri_scheme = &triple[2];
        eprintln!("Adding LSP server: command={command:?} language_id={language_id:?} uri_scheme={uri_scheme:?}");
        let provider = LspDiagnosticsProvider::new(LspConfig {
            command: command.clone(),

            uri_scheme: uri_scheme.clone(),
            language_id: language_id.clone(),
        });
        line_editor = line_editor.with_lsp_diagnostics(provider);
    }

    let prompt = DefaultPrompt::default();

    println!();
    println!("Multi-LSP Demo");
    println!("==============");
    println!("Type code to see diagnostics from all servers.");
    println!("Press Alt+f or Ctrl+. for fix menu. Ctrl+C to exit.");
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
    keybindings.add_binding(KeyModifiers::NONE, KeyCode::Tab, ReedlineEvent::MenuNext);
    keybindings.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::BackTab,
        ReedlineEvent::MenuPrevious,
    );
}
