//! Standard-input and standard-output entry point for the Papyrus language server.

use std::error::Error;

fn main() {
    if let Err(error) = run() {
        eprintln!("papyrus-language-server: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let (connection, io_threads) = lsp_server::Connection::stdio();
    papyrus_language_server::run_connection(&connection)?;
    io_threads.join()?;
    Ok(())
}
