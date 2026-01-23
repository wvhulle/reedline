//! Non-blocking LSP client for diagnostics.
//!
//! Uses a background worker thread to communicate with the LSP server,
//! so the main editor thread is never blocked by slow LSP responses.

use std::{
    thread,
    time::{Duration, Instant},
};

use crossbeam::channel::{bounded, Receiver, Sender};
use lsp_types::{CodeAction, Diagnostic};

use super::{diagnostic::Span, worker::LspWorker};

/// LSP server configuration.
#[derive(Debug, Clone)]
pub struct LspConfig {
    /// Full command to start the LSP server (e.g., "nu-lint --lsp")
    pub command: String,
    /// Response timeout in milliseconds
    pub timeout_ms: u64,
    /// URI scheme (default: "repl")
    pub uri_scheme: String,
}

// Channel capacity for commands and responses
const CHANNEL_CAPACITY: usize = 32;

/// Commands sent from main thread to worker.
pub(super) enum LspCommand {
    UpdateContent(String),
    RequestCodeActions {
        content: String,
        span: Span,
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

/// Handle for sending LSP commands from outside the provider.
///
/// Used by `DiagnosticFixMenu` to execute command-based code actions.
#[derive(Clone)]
pub struct LspCommandSender {
    tx: Sender<LspCommand>,
}

impl LspCommandSender {
    /// Execute an LSP command (fire-and-forget, non-blocking).
    pub fn execute_command(&self, command: String, arguments: Vec<serde_json::Value>) {
        let _ = self
            .tx
            .try_send(LspCommand::ExecuteCommand { command, arguments });
    }
}

/// LSP diagnostics provider (main thread interface).
///
/// Provides a non-blocking interface to LSP diagnostics.
/// All communication with the LSP server happens in a background thread.
pub struct LspDiagnosticsProvider {
    command_tx: Sender<LspCommand>,
    response_rx: Receiver<LspResponse>,
    wake_rx: Receiver<()>,
    diagnostics: Vec<Diagnostic>,
    last_content_hash: u64,
}

impl LspDiagnosticsProvider {
    /// Create new provider and spawn worker thread.
    #[must_use]
    pub fn new(config: LspConfig) -> Self {
        let (command_tx, command_rx) = bounded(CHANNEL_CAPACITY);
        let (response_tx, response_rx) = bounded(CHANNEL_CAPACITY);
        let (wake_tx, wake_rx) = bounded(1);

        let worker = LspWorker {
            uri: format!("{}:/session/repl", config.uri_scheme),
            config,
            conn: None,
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

    /// Update content (non-blocking). Sends to worker if content changed.
    pub fn update_content(&mut self, content: &str) {
        if content.is_empty() {
            self.diagnostics.clear();
            return;
        }

        // Only send if content changed to avoid flooding the worker
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
    pub fn code_actions(&mut self, content: &str, span: Span) -> Vec<CodeAction> {
        let _ = self.command_tx.try_send(LspCommand::RequestCodeActions {
            content: content.to_string(),
            span,
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

    /// Get a command sender for executing LSP commands from menus.
    pub fn command_sender(&self) -> LspCommandSender {
        LspCommandSender {
            tx: self.command_tx.clone(),
        }
    }
}

impl Drop for LspDiagnosticsProvider {
    fn drop(&mut self) {
        let _ = self.command_tx.try_send(LspCommand::Shutdown);
        // Worker will exit when channel disconnects
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
