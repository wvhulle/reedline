//! Non-blocking LSP client for diagnostics.
//!
//! Uses a background worker thread to communicate with the LSP server,
//! so the main editor thread is never blocked by slow LSP responses.

use std::{
    thread,
    time::{Duration, Instant},
};

use crossbeam::channel::{bounded, Receiver};
use async_lsp::lsp_types::{CodeAction, Diagnostic};
use tokio::sync::mpsc as tokio_mpsc;

use super::{diagnostic::ByteBufferSpan, worker::LspWorker};

/// LSP server configuration.
#[derive(Debug, Clone)]
pub struct LspConfig {
    /// Full command to start the LSP server (e.g., "nu-lint --lsp")
    pub command: String,
    /// URI scheme for the document URI sent to the LSP server.
    /// Use `"file"` (default) for broad compatibility with all servers.
    /// Use `"repl"` for servers that filter REPL-inappropriate actions (e.g. nu-lint).
    pub uri_scheme: String,
    /// Language identifier sent to the LSP server (e.g., "nushell", "bash")
    pub language_id: String,
}

// Channel capacity for commands and responses
const CHANNEL_CAPACITY: usize = 32;

/// Commands sent from main thread to worker.
pub(super) enum LspCommand {
    UpdateContent(String),
    RequestCodeActions {
        content: String,
        span: ByteBufferSpan,
        diagnostics: Vec<Diagnostic>,
    },
    ExecuteCommand {
        command: String,
        arguments: Vec<serde_json::Value>,
    },
    Shutdown,
}

/// Responses sent from worker to main thread.
pub(super) enum LspResponse {
    Diagnostics(Vec<Diagnostic>),
    CodeActions(Vec<CodeAction>),
    CommandExecuted(bool),
}

/// LSP diagnostics provider (main thread interface).
///
/// Provides a non-blocking interface to LSP diagnostics.
/// All communication with the LSP server happens in a background thread.
pub struct LspDiagnosticsProvider {
    command_tx: tokio_mpsc::Sender<LspCommand>,
    response_rx: Receiver<LspResponse>,
    wake_rx: Receiver<()>,
    diagnostics: Vec<Diagnostic>,
    last_content_hash: u64,
}

impl LspDiagnosticsProvider {
    /// Create new provider and spawn worker thread.
    #[must_use]
    pub fn new(config: LspConfig) -> Self {
        let (command_tx, command_rx) = tokio_mpsc::channel(CHANNEL_CAPACITY);
        let (response_tx, response_rx) = bounded(CHANNEL_CAPACITY);
        let (wake_tx, wake_rx) = bounded(1);

        let worker = LspWorker {
            uri: make_document_uri(&config),
            config,
            version: 0,
            command_rx,
            response_tx,
            wake_tx,
        };

        thread::spawn(move || worker.run());

        Self {
            command_tx,
            response_rx,
            wake_rx,
            diagnostics: Vec::new(),
            last_content_hash: 0,
        }
    }

    /// Reset provider state, discarding cached diagnostics and any pending
    /// worker responses so stale results don't resurface on the next paint.
    fn reset(&mut self) {
        self.diagnostics.clear();
        self.last_content_hash = 0;
        while self.response_rx.try_recv().is_ok() {}
        while self.wake_rx.try_recv().is_ok() {}
    }

    /// Update content (non-blocking). Sends to worker if content changed.
    /// An empty buffer resets provider state.
    pub fn update_content(&mut self, content: &str) {
        if content.is_empty() {
            self.reset();
            return;
        }

        let hash = hash_str(content);
        if hash != self.last_content_hash {
            self.last_content_hash = hash;
            let _ = self
                .command_tx
                .try_send(LspCommand::UpdateContent(content.to_string()));
        }
    }

    /// Get current diagnostics, polling for any new responses first.
    pub fn diagnostics(&mut self) -> &[Diagnostic] {
        self.poll_responses();
        &self.diagnostics
    }

    /// Get code actions for a given span.
    pub fn code_actions(&mut self, content: &str, span: ByteBufferSpan) -> Vec<CodeAction> {
        let _ = self.command_tx.try_send(LspCommand::RequestCodeActions {
            content: content.to_string(),
            span,
            diagnostics: self.diagnostics.clone(),
        });

        // Brief wait for response
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(100) {
            match self.response_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(LspResponse::CodeActions(actions)) => return actions,
                Ok(LspResponse::Diagnostics(diags)) => self.diagnostics = diags,
                Ok(LspResponse::CommandExecuted(_)) => {}
                Err(_) => {}
            }
        }
        Vec::new()
    }

    /// Execute an LSP command on the server.
    ///
    /// Returns `true` if the command was executed successfully.
    pub fn execute_command(&mut self, command: &str, arguments: Vec<serde_json::Value>) -> bool {
        let _ = self.command_tx.try_send(LspCommand::ExecuteCommand {
            command: command.to_string(),
            arguments,
        });

        // Wait for response
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            match self.response_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(LspResponse::CommandExecuted(success)) => return success,
                Ok(LspResponse::Diagnostics(diags)) => self.diagnostics = diags,
                Ok(LspResponse::CodeActions(_)) => {}
                Err(_) => {}
            }
        }
        false
    }

    /// Poll for responses from worker (non-blocking).
    fn poll_responses(&mut self) {
        while let Ok(response) = self.response_rx.try_recv() {
            match response {
                LspResponse::Diagnostics(diags) => self.diagnostics = diags,
                LspResponse::CodeActions(_) | LspResponse::CommandExecuted(_) => {}
            }
        }
    }

    /// Check if worker has signaled new diagnostics are available.
    /// If so, polls responses and returns true.
    pub fn check_wake(&mut self) -> bool {
        if self.wake_rx.try_recv().is_ok() {
            self.poll_responses();
            true
        } else {
            false
        }
    }

}

impl Drop for LspDiagnosticsProvider {
    fn drop(&mut self) {
        let _ = self.command_tx.try_send(LspCommand::Shutdown);
        // Worker will also exit when the sender is dropped and the channel closes
    }
}

/// Build a document URI that LSP servers can use to identify the language.
///
/// For `file` scheme: uses `cwd/.reedline-repl.{ext}` so servers that resolve
/// paths (e.g. ast-grep) work correctly.
/// For other schemes (e.g. `repl`): uses `{scheme}:///repl.{ext}`.
fn make_document_uri(config: &LspConfig) -> String {
    let ext = language_id_to_extension(&config.language_id);
    if config.uri_scheme == "file" {
        let dir = std::env::current_dir().unwrap_or_default();
        let path = dir.join(format!(".reedline-repl.{ext}"));
        async_lsp::lsp_types::Url::from_file_path(path)
            .map(|u| u.to_string())
            .unwrap_or_else(|()| format!("file:///tmp/.reedline-repl.{ext}"))
    } else {
        format!("{}:///repl.{ext}", config.uri_scheme)
    }
}

fn language_id_to_extension(language_id: &str) -> &str {
    match language_id {
        "bash" | "shellscript" => "sh",
        "nushell" | "nu" => "nu",
        "python" => "py",
        "rust" => "rs",
        "javascript" => "js",
        "typescript" => "ts",
        "go" => "go",
        "c" => "c",
        "cpp" => "cpp",
        "ruby" => "rb",
        "lua" => "lua",
        "perl" => "pl",
        "fish" => "fish",
        "zsh" => "zsh",
        other => other,
    }
}

fn hash_str(s: &str) -> u64 {
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
    };
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
