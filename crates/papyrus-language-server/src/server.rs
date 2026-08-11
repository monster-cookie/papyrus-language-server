use std::error::Error;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentSymbolParams, DocumentSymbolResponse, InitializeParams,
    PublishDiagnosticsParams, Uri, WorkspaceSymbolParams,
};

use crate::{
    config::WorkspaceConfig, diagnostics::PapyrusAnalyzer, documents::DocumentStore,
    workspace::WorkspaceIndex,
};

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
    let (initialize_id, initialize_value) = connection.initialize_start()?;
    let initialize_params: InitializeParams = serde_json::from_value(initialize_value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let config = WorkspaceConfig::from_initialize(&initialize_params);
    let initialize_result = serde_json::json!({
        "capabilities": {
            "positionEncoding": "utf-16",
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": true,
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

    let mut server = Server::new(&config)?;
    while let Ok(message) = connection.receiver.recv() {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                server.handle_request(connection, request)?;
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
    workspace: WorkspaceIndex,
}

impl Server {
    fn new(config: &WorkspaceConfig) -> ServerResult<Self> {
        let analyzer = PapyrusAnalyzer::new().map_err(std::io::Error::other)?;
        Ok(Self {
            analyzer,
            documents: DocumentStore::default(),
            workspace: WorkspaceIndex::new(config).map_err(std::io::Error::other)?,
        })
    }

    fn handle_request(&mut self, connection: &Connection, request: Request) -> ServerResult<()> {
        let response = match request.method.as_str() {
            "textDocument/documentSymbol" => {
                let params: DocumentSymbolParams =
                    deserialize_request(request.params, "documentSymbol")?;
                let symbols = self.workspace.document_symbols(&params.text_document.uri);
                Response::new_ok(
                    request.id,
                    serde_json::to_value(DocumentSymbolResponse::Nested(symbols))?,
                )
            }
            "workspace/symbol" => {
                let params: WorkspaceSymbolParams =
                    deserialize_request(request.params, "workspaceSymbol")?;
                let symbols = self.workspace.workspace_symbols(&params.query);
                Response::new_ok(request.id, serde_json::to_value(symbols)?)
            }
            _ => Response::new_err(
                request.id,
                ErrorCode::MethodNotFound as i32,
                format!("unsupported request: {}", request.method),
            ),
        };
        connection.sender.send(Message::Response(response))?;
        Ok(())
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
                if let Some(document) = self.documents.get(&uri) {
                    self.workspace.overlay(uri.clone(), &document.text);
                }
                self.publish(connection, &uri)?;
            }
            "textDocument/didChange" => {
                let params: DidChangeTextDocumentParams =
                    deserialize_notification(notification.params, "didChange")?;
                let uri = params.text_document.uri;
                if let Some(change) = params.content_changes.into_iter().last() {
                    self.documents
                        .change(&uri, change.text, params.text_document.version);
                    if let Some(document) = self.documents.get(&uri) {
                        self.workspace.overlay(uri.clone(), &document.text);
                    }
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
                if let Some(document) = self.documents.get(&uri) {
                    self.workspace.overlay(uri.clone(), &document.text);
                }
                self.publish(connection, &uri)?;
            }
            "textDocument/didClose" => {
                let params: DidCloseTextDocumentParams =
                    deserialize_notification(notification.params, "didClose")?;
                let uri = params.text_document.uri;
                self.documents.close(&uri);
                self.workspace.close(&uri);
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

fn deserialize_request<T>(value: serde_json::Value, name: &str) -> ServerResult<T>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(value).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid {name} request: {error}"),
        )
        .into()
    })
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
