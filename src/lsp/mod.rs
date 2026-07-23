//! LSP integration for inline diagnostics.
//!
//! This module provides a minimal LSP client for real-time diagnostics in the REPL.
//!
//! # Example
//!
//! ```ignore
//! use reedline::{LspConfig, LspDiagnosticsProvider};
//!
//! let config = LspConfig::new("nu-lint").with_args(vec!["--lsp".into()]);
//! let mut provider = LspDiagnosticsProvider::new(config);
//!
//! provider.update_content("let x = 1");
//! for diag in provider.diagnostics() {
//!     println!("{:?}: {}", diag.severity, diag.message);
//! }
//! ```

mod actions;
mod client;
mod diagnostic;
mod engine_integration;
mod worker;

pub use client::{LspConfig, LspDiagnosticsProvider};
pub use diagnostic::{CodeAction, Diagnostic, DiagnosticSeverity, ByteBufferSpan, TextEdit};
pub(crate) use engine_integration::{
    apply_diagnostic_underlines, create_diagnostic_fix_menu, format_diagnostics_for_prompt,
};
