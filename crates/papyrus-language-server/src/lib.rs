//! Core Papyrus analysis and Language Server Protocol implementation.

mod config;
mod diagnostics;
mod documents;
mod line_index;
mod server;
mod structure;
mod symbols;
mod workspace;

pub use config::{PapyrusDialect, WorkspaceConfig};
pub use diagnostics::PapyrusAnalyzer;
pub use server::run_connection;
