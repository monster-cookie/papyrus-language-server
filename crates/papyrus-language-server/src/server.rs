use std::error::Error;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, PublishDiagnosticsParams, Uri,
};

use crate::{diagnostics::PapyrusAnalyzer, documents::DocumentStore};

type ServerResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Runs a complete Papyrus LSP session over the supplied connection.
///
/// The function performs the initialization handshake, processes full-text document
/// synchronization notifications, publishes syntax diagnostics, and honors shutdown.
///
/// # Errors
///
/// Returns protocol, serialization, grammar initialization, or transport errors.
pub fn run_connection(connection: &Connection) -> ServerResult<()> {
    let (initialize_id, _) = connection.initialize_start()?;
    let initialize_result = serde_json::json!({
        "capabilities": {
            "positionEncoding": "utf-16",
            "textDocumentSync": {
                "openClose": true,
                "change": 1,
                "save": { "includeText": true }
            }
        },
        "serverInfo": {
            "name": "papyrus-language-server",
            "version": env!("CARGO_PKG_VERSION")
        }
    });
    connection.initialize_finish(initialize_id, initialize_result)?;

    let mut server = Server::new()?;
    while let Ok(message) = connection.receiver.recv() {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                respond_method_not_found(connection, request)?;
            }
            Message::Notification(notification) => {
                if notification.method == "exit" {
                    return Ok(());
                }
                server.handle_notification(connection, notification)?;
            }
            Message::Response(_) => {}
        }
    }

    Ok(())
}

struct Server {
    analyzer: PapyrusAnalyzer,
    documents: DocumentStore,
}

impl Server {
    fn new() -> ServerResult<Self> {
        let analyzer = PapyrusAnalyzer::new().map_err(std::io::Error::other)?;
        Ok(Self {
            analyzer,
            documents: DocumentStore::default(),
        })
    }

    fn handle_notification(
        &mut self,
        connection: &Connection,
        notification: Notification,
    ) -> ServerResult<()> {
        match notification.method.as_str() {
            "textDocument/didOpen" => {
                let params: DidOpenTextDocumentParams =
                    deserialize_notification(notification.params, "didOpen")?;
                let document = params.text_document;
                let uri = document.uri;
                self.documents
                    .open(uri.clone(), document.text, document.version);
                self.publish(connection, &uri)?;
            }
            "textDocument/didChange" => {
                let params: DidChangeTextDocumentParams =
                    deserialize_notification(notification.params, "didChange")?;
                let uri = params.text_document.uri;
                if let Some(change) = params.content_changes.into_iter().last() {
                    self.documents
                        .change(&uri, change.text, params.text_document.version);
                    self.publish(connection, &uri)?;
                }
            }
            "textDocument/didSave" => {
                let params: DidSaveTextDocumentParams =
                    deserialize_notification(notification.params, "didSave")?;
                let uri = params.text_document.uri;
                if let Some(text) = params.text {
                    self.documents.save_text(&uri, text);
                }
                self.publish(connection, &uri)?;
            }
            "textDocument/didClose" => {
                let params: DidCloseTextDocumentParams =
                    deserialize_notification(notification.params, "didClose")?;
                let uri = params.text_document.uri;
                self.documents.close(&uri);
                publish(connection, uri, Vec::new(), None)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn publish(&mut self, connection: &Connection, uri: &Uri) -> ServerResult<()> {
        let Some(document) = self.documents.get(uri) else {
            return Ok(());
        };
        let diagnostics = self.analyzer.diagnostics(&document.text);
        publish(connection, uri.clone(), diagnostics, document.version)
    }
}

fn deserialize_notification<T>(value: serde_json::Value, name: &str) -> ServerResult<T>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(value).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid {name} notification: {error}"),
        )
        .into()
    })
}

fn publish(
    connection: &Connection,
    uri: Uri,
    diagnostics: Vec<lsp_types::Diagnostic>,
    version: Option<i32>,
) -> ServerResult<()> {
    let params = PublishDiagnosticsParams {
        uri,
        diagnostics,
        version,
    };
    connection
        .sender
        .send(Message::Notification(Notification::new(
            "textDocument/publishDiagnostics".to_owned(),
            params,
        )))?;
    Ok(())
}

fn respond_method_not_found(connection: &Connection, request: Request) -> ServerResult<()> {
    let response = Response::new_err(
        request.id,
        ErrorCode::MethodNotFound as i32,
        format!("unsupported request: {}", request.method),
    );
    connection.sender.send(Message::Response(response))?;
    Ok(())
}
