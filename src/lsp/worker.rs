//! Background worker for LSP communication.
//!
//! Runs in a separate thread with a tokio current-thread runtime,
//! using `async-lsp` to drive the LSP protocol.

use std::{ops::ControlFlow, process::Stdio};

use async_lsp::{
    lsp_types::{
        self, CodeActionParams, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
        ExecuteCommandParams, InitializeParams, InitializedParams, PublishDiagnosticsParams,
        TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
        VersionedTextDocumentIdentifier,
    },
    router::Router,
    LanguageClient, LanguageServer, MainLoop, ResponseError, ServerSocket,
};
use crossbeam::channel::Sender;
use futures::future::BoxFuture;
use tokio::sync::mpsc as tokio_mpsc;

use super::{
    client::{LspCommand, LspResponse},
    diagnostic::ByteBufferSpan,
    LspConfig,
};

/// Client state passed to the async-lsp router.
struct ClientState {
    response_tx: Sender<LspResponse>,
    wake_tx: Sender<()>,
}

impl LanguageClient for ClientState {
    type Error = ResponseError;
    type NotifyResult = ControlFlow<async_lsp::Result<()>>;

    fn publish_diagnostics(&mut self, params: PublishDiagnosticsParams) -> Self::NotifyResult {
        let _ = self
            .response_tx
            .try_send(LspResponse::Diagnostics(params.diagnostics));
        let _ = self.wake_tx.try_send(());
        ControlFlow::Continue(())
    }

    fn log_message(&mut self, _params: lsp_types::LogMessageParams) -> Self::NotifyResult {
        ControlFlow::Continue(())
    }

    fn show_message(&mut self, _params: lsp_types::ShowMessageParams) -> Self::NotifyResult {
        ControlFlow::Continue(())
    }

    fn telemetry_event(
        &mut self,
        _params: lsp_types::OneOf<
            serde_json::Map<String, serde_json::Value>,
            Vec<serde_json::Value>,
        >,
    ) -> Self::NotifyResult {
        ControlFlow::Continue(())
    }

    fn log_trace(&mut self, _params: lsp_types::LogTraceParams) -> Self::NotifyResult {
        ControlFlow::Continue(())
    }

    fn progress(&mut self, _params: lsp_types::ProgressParams) -> Self::NotifyResult {
        ControlFlow::Continue(())
    }

    fn register_capability(
        &mut self,
        _params: lsp_types::RegistrationParams,
    ) -> BoxFuture<'static, Result<(), ResponseError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn unregister_capability(
        &mut self,
        _params: lsp_types::UnregistrationParams,
    ) -> BoxFuture<'static, Result<(), ResponseError>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

/// Background worker that owns the LSP connection.
pub(super) struct LspWorker {
    pub config: LspConfig,
    pub uri: String,
    pub version: i32,
    pub command_rx: tokio_mpsc::Receiver<LspCommand>,
    pub response_tx: Sender<LspResponse>,
    pub wake_tx: Sender<()>,
}

impl LspWorker {
    pub fn run(self) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(self.run_async());
    }

    async fn run_async(mut self) {
        if let Err(e) = self.try_run().await {
            log::error!("[{}] worker error: {e}", self.config.command);
        }
    }

    async fn try_run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut parts = self.config.command.split_whitespace();
        let bin = parts.next().ok_or("empty LSP command")?;
        let args: Vec<&str> = parts.collect();

        let mut child = async_process::Command::new(bin)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;

        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stdin = child.stdin.take().ok_or("no stdin")?;

        let state = ClientState {
            response_tx: self.response_tx.clone(),
            wake_tx: self.wake_tx.clone(),
        };

        let (mainloop, mut server) =
            MainLoop::new_client(|_| Router::from_language_client(state));

        let mut mainloop_handle = tokio::spawn(mainloop.run_buffered(stdout, stdin));

        let root_uri = std::env::current_dir()
            .ok()
            .and_then(|p| lsp_types::Url::from_file_path(p).ok());

        #[allow(deprecated)] // root_uri needed for older LSP servers
        let init_params = InitializeParams {
            process_id: Some(std::process::id()),
            client_info: Some(lsp_types::ClientInfo {
                name: "reedline".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            root_uri: root_uri.clone(),
            workspace_folders: root_uri.map(|uri| {
                vec![lsp_types::WorkspaceFolder {
                    uri,
                    name: "repl".into(),
                }]
            }),
            ..Default::default()
        };

        server.initialize(init_params).await?;
        server.initialized(InitializedParams {})?;

        let doc_uri: lsp_types::Url = self.uri.parse()?;
        server.did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: doc_uri.clone(),
                language_id: self.config.language_id.clone(),
                version: 0,
                text: String::new(),
            },
        })?;

        loop {
            tokio::select! {
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(LspCommand::UpdateContent(mut content)) => {
                            // Drain queued content updates — only the latest matters
                            loop {
                                match self.command_rx.try_recv() {
                                    Ok(LspCommand::UpdateContent(newer)) => content = newer,
                                    _ => break,
                                }
                            }
                            if content.is_empty() {
                                let _ = self
                                    .response_tx
                                    .try_send(LspResponse::Diagnostics(Vec::new()));
                                let _ = self.wake_tx.try_send(());
                                continue;
                            }
                            self.version += 1;
                            server.did_change(DidChangeTextDocumentParams {
                                text_document: VersionedTextDocumentIdentifier {
                                    uri: doc_uri.clone(),
                                    version: self.version,
                                },
                                content_changes: vec![TextDocumentContentChangeEvent {
                                    range: None,
                                    range_length: None,
                                    text: content,
                                }],
                            })?;
                        }
                        Some(LspCommand::RequestCodeActions { content, span, diagnostics }) => {
                            let actions = request_code_actions_async(
                                &self.uri,
                                &content,
                                span,
                                diagnostics,
                                &mut server,
                            )
                            .await;
                            let _ = self.response_tx.try_send(LspResponse::CodeActions(actions));
                        }
                        Some(LspCommand::ExecuteCommand { command, arguments }) => {
                            let params = ExecuteCommandParams {
                                command,
                                arguments,
                                work_done_progress_params: Default::default(),
                            };
                            let success = server.execute_command(params).await.is_ok();
                            let _ = self
                                .response_tx
                                .try_send(LspResponse::CommandExecuted(success));
                        }
                        Some(LspCommand::Shutdown) | None => {
                            let _ = server.shutdown(()).await;
                            let _ = server.exit(());
                            break;
                        }
                    }
                }
                result = &mut mainloop_handle => {
                    match result {
                        Ok(Err(e)) => log::error!("[{}] mainloop error: {e}", self.config.command),
                        Err(e) => log::error!("[{}] mainloop panicked: {e}", self.config.command),
                        Ok(Ok(())) => {}
                    }
                    break;
                }
            }
        }

        Ok(())
    }
}

/// Request code actions from the server using the async socket.
async fn request_code_actions_async(
    uri: &str,
    content: &str,
    span: ByteBufferSpan,
    diagnostics: Vec<lsp_types::Diagnostic>,
    server: &mut ServerSocket,
) -> Vec<lsp_types::CodeAction> {
    use super::actions::span_to_range;

    let Some(parsed_uri) = uri.parse().ok() else {
        return Vec::new();
    };

    let params = CodeActionParams {
        text_document: TextDocumentIdentifier { uri: parsed_uri },
        range: span_to_range(content, span),
        context: lsp_types::CodeActionContext {
            diagnostics,
            only: None,
            trigger_kind: None,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };

    match server.code_action(params).await {
        Ok(Some(actions)) => actions
            .into_iter()
            .filter_map(|a| match a {
                lsp_types::CodeActionOrCommand::CodeAction(action) => Some(action),
                lsp_types::CodeActionOrCommand::Command(_) => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}
