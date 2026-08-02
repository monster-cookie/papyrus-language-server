//! Core Papyrus analysis and Language Server Protocol implementation.

mod diagnostics;
mod documents;
mod line_index;
mod server;
mod structure;

pub use diagnostics::PapyrusAnalyzer;
pub use server::run_connection;
