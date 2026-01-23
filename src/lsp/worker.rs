//! Background worker for LSP communication.
//!
//! Runs in a separate thread to avoid blocking the main editor thread.

use std::{
    io::{BufRead, BufReader, BufWriter, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crossbeam::channel::{Receiver, Sender};
use lsp_types::{
    Diagnostic, DidChangeTextDocumentParams, DidOpenTextDocumentParams, ExecuteCommandParams,
    InitializeParams, InitializedParams, PublishDiagnosticsParams, TextDocumentContentChangeEvent,
    TextDocumentItem, VersionedTextDocumentIdentifier,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    actions::request_code_actions,
    client::{LspCommand, LspResponse},
    diagnostic::Span,
    LspConfig,
};

/// Background worker that owns the LSP connection.
pub(super) struct LspWorker {
    pub config: LspConfig,
    pub conn: Option<Connection>,
    pub uri: String,
    pub version: i32,
    pub command_rx: Receiver<LspCommand>,
    pub response_tx: Sender<LspResponse>,
    pub wake_tx: Sender<()>,
}

pub(super) struct Connection {
    #[allow(dead_code)]
    pub child: Child,
    pub writer: BufWriter<ChildStdin>,
    pub reader: BufReader<ChildStdout>,
    pub next_id: i32,
}

impl LspWorker {
    pub fn run(mut self) {
        loop {
            // Block waiting for commands (with timeout to allow graceful shutdown)
            match self.command_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(LspCommand::Shutdown) => {
                    self.shutdown();
                    return;
                }
                Ok(LspCommand::UpdateContent(content)) => {
                    self.handle_update_content(&content);
                }
                Ok(LspCommand::RequestCodeActions { content, span }) => {
                    self.handle_code_actions_request(&content, span);
                }
                Ok(LspCommand::ExecuteCommand { command, arguments }) => {
                    self.handle_execute_command(&command, &arguments);
                }
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                    self.shutdown();
                    return;
                }
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                    // No commands, continue loop
                }
            }
        }
    }

    fn handle_update_content(&mut self, content: &str) {
        if content.is_empty() {
            self.send_diagnostics(Vec::new());
            return;
        }

        if !self.ensure_init() {
            return;
        }

        self.version += 1;
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        let Some(uri) = self.uri.parse().ok() else {
            return;
        };

        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri,
                version: self.version,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: content.into(),
            }],
        };
        let _ = notify(conn, "textDocument/didChange", &params);

        self.poll_for_diagnostics();
    }

    fn send_diagnostics(&self, diagnostics: Vec<Diagnostic>) {
        let _ = self
            .response_tx
            .try_send(LspResponse::Diagnostics(diagnostics));
        let _ = self.wake_tx.try_send(());
    }

    fn handle_code_actions_request(&mut self, content: &str, span: Span) {
        let actions = self
            .conn
            .as_mut()
            .map(|conn| {
                request_code_actions(
                    &self.uri,
                    content,
                    span,
                    self.config.timeout_ms,
                    |method, params, timeout| request(conn, method, params, timeout),
                )
            })
            .unwrap_or_default();

        let _ = self.response_tx.try_send(LspResponse::CodeActions(actions));
    }

    fn handle_execute_command(&mut self, command: &str, arguments: &[Value]) {
        let success = self
            .conn
            .as_mut()
            .and_then(|conn| {
                let params = ExecuteCommandParams {
                    command: command.to_string(),
                    arguments: arguments.to_vec(),
                    work_done_progress_params: Default::default(),
                };
                request(
                    conn,
                    "workspace/executeCommand",
                    &params,
                    self.config.timeout_ms,
                )
            })
            .is_some();

        let _ = self
            .response_tx
            .try_send(LspResponse::CommandExecuted(success));
    }

    fn poll_for_diagnostics(&mut self) {
        let Some(conn) = &mut self.conn else { return };

        let timeout = Duration::from_millis(self.config.timeout_ms);
        let start = Instant::now();

        let diagnostics =
            std::iter::from_fn(|| read_msg(&mut conn.reader, Duration::from_millis(5)))
                .take_while(|_| start.elapsed() < timeout)
                .filter(|msg| msg.method.as_deref() == Some("textDocument/publishDiagnostics"))
                .filter_map(|msg| msg.params)
                .filter_map(|params| {
                    serde_json::from_value::<PublishDiagnosticsParams>(params).ok()
                })
                .next()
                .map(|p| p.diagnostics);

        if let Some(diagnostics) = diagnostics {
            self.send_diagnostics(diagnostics);
        }
    }

    fn ensure_init(&mut self) -> bool {
        if self.conn.is_some() {
            return true;
        }
        self.conn = self.try_init();
        self.conn.is_some()
    }

    fn try_init(&self) -> Option<Connection> {
        let mut parts = self.config.command.split_whitespace();
        let bin = parts.next()?;
        let args: Vec<&str> = parts.collect();

        let mut child = Command::new(bin)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let mut conn = Connection {
            writer: BufWriter::new(child.stdin.take()?),
            reader: BufReader::new(child.stdout.take()?),
            child,
            next_id: 1,
        };

        let init_params = InitializeParams {
            process_id: Some(std::process::id()),
            client_info: Some(lsp_types::ClientInfo {
                name: "reedline".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            ..Default::default()
        };

        request(
            &mut conn,
            "initialize",
            &init_params,
            self.config.timeout_ms * 5,
        )?;
        notify(&mut conn, "initialized", &InitializedParams {})?;
        notify(
            &mut conn,
            "textDocument/didOpen",
            &DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: self.uri.parse().ok()?,
                    language_id: "nushell".into(),
                    version: 0,
                    text: String::new(),
                },
            },
        )?;

        Some(conn)
    }

    fn shutdown(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            let _ = request(&mut conn, "shutdown", &(), 100);
            let _ = notify(&mut conn, "exit", &());
            thread::sleep(Duration::from_millis(20));
            let _ = conn.child.kill();
        }
    }
}

// JSON-RPC helpers

#[derive(Serialize, Deserialize)]
pub(super) struct Msg {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

pub(super) fn request<T: Serialize>(
    conn: &mut Connection,
    method: &str,
    params: &T,
    timeout_ms: u64,
) -> Option<Value> {
    let id = conn.next_id;
    conn.next_id += 1;

    let msg = Msg {
        jsonrpc: "2.0".into(),
        id: Some(id),
        method: Some(method.into()),
        params: serde_json::to_value(params).ok(),
        result: None,
        error: None,
    };
    write_msg(&mut conn.writer, &msg).ok()?;

    let timeout = Duration::from_millis(timeout_ms);
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(resp) = read_msg(&mut conn.reader, Duration::from_millis(10)) {
            if resp.id == Some(id) {
                return resp.result;
            }
        }
    }
    None
}

pub(super) fn notify<T: Serialize>(conn: &mut Connection, method: &str, params: &T) -> Option<()> {
    let msg = Msg {
        jsonrpc: "2.0".into(),
        id: None,
        method: Some(method.into()),
        params: serde_json::to_value(params).ok(),
        result: None,
        error: None,
    };
    write_msg(&mut conn.writer, &msg).ok()
}

fn write_msg<W: Write>(w: &mut W, msg: &Msg) -> std::io::Result<()> {
    let json = serde_json::to_string(msg)?;
    write!(w, "Content-Length: {}\r\n\r\n{}", json.len(), json)?;
    w.flush()
}

fn read_msg<R: BufRead>(r: &mut R, timeout: Duration) -> Option<Msg> {
    let start = Instant::now();
    let mut header = String::new();

    while start.elapsed() < timeout {
        header.clear();
        if r.read_line(&mut header).ok()? == 0 {
            return None;
        }
        if let Some(len) = header.strip_prefix("Content-Length:") {
            let len: usize = len.trim().parse().ok()?;
            let mut empty = String::new();
            r.read_line(&mut empty).ok()?;
            let mut buf = vec![0u8; len];
            r.read_exact(&mut buf).ok()?;
            return serde_json::from_slice(&buf).ok();
        }
    }
    None
}
