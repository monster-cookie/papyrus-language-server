//! Core Papyrus analysis and Language Server Protocol implementation.

mod cache;
mod config;
mod diagnostics;
mod discovery;
mod documents;
mod line_index;
mod semantic;
mod server;
mod source_filter;
mod structure;
mod symbols;
mod workspace;

pub use config::{PapyrusDialect, WorkspaceConfig};
pub use diagnostics::PapyrusAnalyzer;
pub use server::run_connection;
