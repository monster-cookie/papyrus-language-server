use std::{
    collections::HashSet,
    error::Error,
    sync::mpsc::TryRecvError,
    time::{Duration, Instant},
};

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    CompletionParams, CompletionResponse, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentSymbolParams,
    DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, HoverParams,
    InitializeParams, NumberOrString, ProgressParams, ProgressParamsValue,
    PublishDiagnosticsParams, ReferenceParams, RenameParams, ResourceOperationKind,
    SignatureHelpParams, TextDocumentPositionParams, Uri, WorkDoneProgress, WorkDoneProgressBegin,
    WorkDoneProgressCancelParams, WorkDoneProgressCreateParams, WorkDoneProgressEnd,
    WorkDoneProgressReport, WorkspaceSymbolParams,
};

use crate::{
    config::WorkspaceConfig,
    diagnostics::PapyrusAnalyzer,
    documents::DocumentStore,
    indexing::{IndexingEvent, IndexingProgress, IndexingTask},
    workspace::WorkspaceIndex,
};

type ServerResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
const INDEX_PROGRESS_ID: &str = "papyrus-workspace-index-create";
const INDEX_PROGRESS_TOKEN: &str = "papyrus-workspace-index";
const MAX_PENDING_INDEX_REQUESTS: usize = 256;

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
    let rename_support = RenameSupport::from_initialize(&initialize_params);
    let progress_support = initialize_params
        .capabilities
        .window
        .as_ref()
        .and_then(|window| window.work_done_progress)
        .unwrap_or(false);
    let config = WorkspaceConfig::from_initialize(&initialize_params);
    let initialize_result = serde_json::json!({
        "capabilities": {
            "positionEncoding": "utf-16",
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": true,
            "completionProvider": { "triggerCharacters": ["."] },
            "hoverProvider": true,
            "definitionProvider": true,
            "referencesProvider": true,
            "renameProvider": { "prepareProvider": true },
            "signatureHelpProvider": {
                "triggerCharacters": ["("],
                "retriggerCharacters": [","]
            },
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

    let mut server = Server::new(connection, &config, rename_support, progress_support)?;
    loop {
        server.poll_indexing(connection)?;
        let message = match connection.receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(message) => message,
            Err(error) if error.is_timeout() => continue,
            Err(_) => break,
        };
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    server.cancel_indexing();
                    return Ok(());
                }
                server.handle_request(connection, request)?;
            }
            Message::Notification(notification) => {
                if notification.method == "exit" {
                    server.cancel_indexing();
                    return Ok(());
                }
                server.handle_notification(connection, notification)?;
            }
            Message::Response(response) => server.handle_response(connection, response)?,
        }
    }

    Ok(())
}

struct Server {
    analyzer: PapyrusAnalyzer,
    documents: DocumentStore,
    dirty_disk_uris: HashSet<Uri>,
    indexing: Option<IndexingTask>,
    indexing_error: Option<IndexingRequestError>,
    latest_index_progress: Option<IndexingProgress>,
    pending_index_requests: Vec<Request>,
    progress_active: bool,
    progress_create_pending: bool,
    progress_finished: Option<String>,
    progress_supported: bool,
    rename_support: RenameSupport,
    workspace: WorkspaceIndex,
}

#[derive(Clone)]
struct IndexingRequestError {
    code: i32,
    message: String,
}

impl Server {
    fn new(
        connection: &Connection,
        config: &WorkspaceConfig,
        rename_support: RenameSupport,
        progress_support: bool,
    ) -> ServerResult<Self> {
        let analyzer = PapyrusAnalyzer::new().map_err(std::io::Error::other)?;
        let indexing = IndexingTask::start(config.clone()).map_err(std::io::Error::other)?;
        let mut server = Self {
            analyzer,
            documents: DocumentStore::default(),
            dirty_disk_uris: HashSet::new(),
            indexing: Some(indexing),
            indexing_error: None,
            latest_index_progress: None,
            pending_index_requests: Vec::new(),
            progress_active: false,
            progress_create_pending: false,
            progress_finished: None,
            progress_supported: progress_support,
            rename_support,
            workspace: WorkspaceIndex::empty(config).map_err(std::io::Error::other)?,
        };
        server.request_progress(connection)?;
        Ok(server)
    }

    fn request_progress(&mut self, connection: &Connection) -> ServerResult<()> {
        if !self.progress_supported || self.progress_create_pending || self.progress_active {
            return Ok(());
        }
        let request = Request::new(
            RequestId::from(INDEX_PROGRESS_ID.to_owned()),
            "window/workDoneProgress/create".to_owned(),
            WorkDoneProgressCreateParams {
                token: progress_token(),
            },
        );
        connection.sender.send(Message::Request(request))?;
        self.progress_create_pending = true;
        Ok(())
    }

    fn cancel_indexing(&self) {
        if let Some(indexing) = &self.indexing {
            indexing.cancel();
        }
    }

    fn poll_indexing(&mut self, connection: &Connection) -> ServerResult<()> {
        let mut indexing_finished = false;
        let mut indexing_succeeded = false;
        loop {
            let event = match self.indexing.as_ref().map(IndexingTask::try_recv) {
                Some(Ok(event)) => event,
                Some(Err(TryRecvError::Empty)) => break,
                Some(Err(TryRecvError::Disconnected)) => {
                    self.indexing = None;
                    let message = "Workspace indexing failed unexpectedly".to_owned();
                    eprintln!("papyrus-language-server: {message}");
                    self.indexing_error = Some(IndexingRequestError {
                        code: ErrorCode::InternalError as i32,
                        message: message.clone(),
                    });
                    self.progress_finished = Some(message.clone());
                    if self.progress_active {
                        send_progress(
                            connection,
                            WorkDoneProgress::End(WorkDoneProgressEnd {
                                message: Some(message),
                            }),
                        )?;
                        self.progress_active = false;
                        self.progress_finished = None;
                    }
                    indexing_finished = true;
                    break;
                }
                None => break,
            };
            match event {
                IndexingEvent::Progress(progress) => {
                    self.latest_index_progress = Some(progress.clone());
                    if self.progress_active {
                        send_progress(
                            connection,
                            WorkDoneProgress::Report(WorkDoneProgressReport {
                                cancellable: Some(true),
                                message: Some(progress_message(&progress)),
                                percentage: None,
                            }),
                        )?;
                    }
                }
                IndexingEvent::Completed(result) => {
                    let message = match *result {
                        Ok(mut workspace) => {
                            for uri in &self.dirty_disk_uris {
                                workspace.close(uri);
                            }
                            for (uri, document) in self.documents.iter() {
                                workspace.overlay(uri.clone(), &document.text);
                            }
                            self.workspace = workspace;
                            self.dirty_disk_uris.clear();
                            indexing_succeeded = true;
                            "Workspace indexing complete".to_owned()
                        }
                        Err(error) if error == "workspace indexing cancelled" => {
                            self.indexing_error = Some(IndexingRequestError {
                                code: ErrorCode::ServerCancelled as i32,
                                message: "Workspace indexing was cancelled; index-dependent requests cannot be completed."
                                    .to_owned(),
                            });
                            "Workspace indexing cancelled".to_owned()
                        }
                        Err(error) => {
                            eprintln!(
                                "papyrus-language-server: workspace indexing failed: {error}"
                            );
                            self.indexing_error = Some(IndexingRequestError {
                                code: ErrorCode::InternalError as i32,
                                message: format!("Workspace indexing failed: {error}"),
                            });
                            format!("Workspace indexing failed: {error}")
                        }
                    };
                    self.progress_finished = Some(message.clone());
                    if self.progress_active {
                        send_progress(
                            connection,
                            WorkDoneProgress::End(WorkDoneProgressEnd {
                                message: Some(message),
                            }),
                        )?;
                        self.progress_active = false;
                        self.progress_finished = None;
                    }
                    self.indexing = None;
                    indexing_finished = true;
                    break;
                }
            }
        }
        if indexing_succeeded {
            for request in std::mem::take(&mut self.pending_index_requests) {
                self.handle_request(connection, request)?;
            }
            self.publish_indexed_semantic_diagnostics(connection)?;
        } else if indexing_finished && let Some(error) = &self.indexing_error {
            for request in std::mem::take(&mut self.pending_index_requests) {
                connection.sender.send(Message::Response(Response::new_err(
                    request.id,
                    error.code,
                    error.message.clone(),
                )))?;
            }
        }
        Ok(())
    }

    fn handle_response(&mut self, connection: &Connection, response: Response) -> ServerResult<()> {
        if !self.progress_create_pending
            || response.id != RequestId::from(INDEX_PROGRESS_ID.to_owned())
        {
            return Ok(());
        }
        self.progress_create_pending = false;
        if response.response_result.is_err() {
            return Ok(());
        }
        self.progress_active = true;
        send_progress(
            connection,
            WorkDoneProgress::Begin(WorkDoneProgressBegin {
                title: "Indexing Papyrus workspace".to_owned(),
                cancellable: Some(true),
                message: self
                    .latest_index_progress
                    .as_ref()
                    .map(progress_message)
                    .or_else(|| Some("Starting".to_owned())),
                percentage: None,
            }),
        )?;
        if let Some(message) = self.progress_finished.take() {
            send_progress(
                connection,
                WorkDoneProgress::End(WorkDoneProgressEnd {
                    message: Some(message),
                }),
            )?;
            self.progress_active = false;
        }
        Ok(())
    }

    fn handle_request(&mut self, connection: &Connection, request: Request) -> ServerResult<()> {
        macro_rules! request_params {
            ($ty:ty, $name:literal) => {
                match deserialize_request::<$ty>(request.params.clone(), $name) {
                    Ok(params) => params,
                    Err(message) => {
                        connection.sender.send(Message::Response(Response::new_err(
                            request.id.clone(),
                            ErrorCode::InvalidParams as i32,
                            message,
                        )))?;
                        return Ok(());
                    }
                }
            };
        }
        if requires_complete_index(&request.method) {
            if let Some(error) = &self.indexing_error {
                connection.sender.send(Message::Response(Response::new_err(
                    request.id,
                    error.code,
                    error.message.clone(),
                )))?;
                return Ok(());
            }
            if self.indexing.is_some() {
                if self.pending_index_requests.len() >= MAX_PENDING_INDEX_REQUESTS {
                    connection.sender.send(Message::Response(Response::new_err(
                        request.id,
                        ErrorCode::ServerCancelled as i32,
                        "Workspace indexing is still in progress; retry this request.".to_owned(),
                    )))?;
                } else {
                    self.pending_index_requests.push(request);
                }
                return Ok(());
            }
        }
        let started = Instant::now();
        let method = request.method.clone();
        let response = match request.method.as_str() {
            "textDocument/documentSymbol" => {
                let params = request_params!(DocumentSymbolParams, "documentSymbol");
                let symbols = self.workspace.document_symbols(&params.text_document.uri);
                Response::new_ok(
                    request.id,
                    serde_json::to_value(DocumentSymbolResponse::Nested(symbols))?,
                )
            }
            "workspace/symbol" => {
                let params = request_params!(WorkspaceSymbolParams, "workspaceSymbol");
                let symbols = self.workspace.workspace_symbols(&params.query);
                Response::new_ok(request.id, serde_json::to_value(symbols)?)
            }
            "textDocument/completion" => {
                let params = request_params!(CompletionParams, "completion");
                let items = self.workspace.completion(
                    &params.text_document_position.text_document.uri,
                    params.text_document_position.position,
                );
                Response::new_ok(
                    request.id,
                    serde_json::to_value(CompletionResponse::Array(items))?,
                )
            }
            "textDocument/hover" => {
                let params = request_params!(HoverParams, "hover");
                let result = self.workspace.hover(
                    &params.text_document_position_params.text_document.uri,
                    params.text_document_position_params.position,
                );
                Response::new_ok(request.id, serde_json::to_value(result)?)
            }
            "textDocument/definition" => {
                let params = request_params!(GotoDefinitionParams, "definition");
                let result = self
                    .workspace
                    .definition(
                        &params.text_document_position_params.text_document.uri,
                        params.text_document_position_params.position,
                    )
                    .map(GotoDefinitionResponse::Scalar);
                Response::new_ok(request.id, serde_json::to_value(result)?)
            }
            "textDocument/references" => {
                let params = request_params!(ReferenceParams, "references");
                let result = self.workspace.references(
                    &params.text_document_position.text_document.uri,
                    params.text_document_position.position,
                    params.context.include_declaration,
                );
                Response::new_ok(request.id, serde_json::to_value(result)?)
            }
            "textDocument/prepareRename" => {
                let params = request_params!(TextDocumentPositionParams, "prepareRename");
                let result = self.workspace.prepare_rename(
                    &params.text_document.uri,
                    params.position,
                    self.rename_support.file_rename,
                );
                Response::new_ok(request.id, serde_json::to_value(result)?)
            }
            "textDocument/rename" => {
                let params = request_params!(RenameParams, "rename");
                match self.workspace.rename_with_versions(
                    &params.text_document_position.text_document.uri,
                    params.text_document_position.position,
                    &params.new_name,
                    self.rename_support.document_changes,
                    self.rename_support.file_rename,
                    &self.documents.versions(),
                ) {
                    Ok(result) => Response::new_ok(request.id, serde_json::to_value(result)?),
                    Err(message) => {
                        Response::new_err(request.id, ErrorCode::InvalidParams as i32, message)
                    }
                }
            }
            "textDocument/signatureHelp" => {
                let params = request_params!(SignatureHelpParams, "signatureHelp");
                let result = self.workspace.signature_help(
                    &params.text_document_position_params.text_document.uri,
                    params.text_document_position_params.position,
                );
                Response::new_ok(request.id, serde_json::to_value(result)?)
            }
            _ => Response::new_err(
                request.id,
                ErrorCode::MethodNotFound as i32,
                format!("unsupported request: {}", request.method),
            ),
        };
        connection.sender.send(Message::Response(response))?;
        if started.elapsed().as_millis() >= 250 {
            eprintln!(
                "papyrus-language-server: slow {method} request: {} ms",
                started.elapsed().as_millis()
            );
        }
        Ok(())
    }

    fn handle_notification(
        &mut self,
        connection: &Connection,
        notification: Notification,
    ) -> ServerResult<()> {
        macro_rules! notification_params {
            ($ty:ty, $name:literal) => {
                match deserialize_notification::<$ty>(notification.params.clone(), $name) {
                    Ok(params) => params,
                    Err(message) => {
                        eprintln!("papyrus-language-server: ignoring {message}");
                        return Ok(());
                    }
                }
            };
        }
        match notification.method.as_str() {
            "textDocument/didOpen" => {
                let params =
                    notification_params!(DidOpenTextDocumentParams, "didOpen notification");
                let document = params.text_document;
                let uri = document.uri;
                self.documents
                    .open(uri.clone(), document.text, document.version);
                if let Some(document) = self.documents.get(&uri) {
                    self.workspace.overlay(uri.clone(), &document.text);
                }
                self.publish_open_documents(connection)?;
            }
            "textDocument/didChange" => {
                let params =
                    notification_params!(DidChangeTextDocumentParams, "didChange notification");
                let uri = params.text_document.uri;
                if let Some(change) = params.content_changes.into_iter().last() {
                    self.documents
                        .change(&uri, change.text, params.text_document.version);
                    if let Some(document) = self.documents.get(&uri) {
                        self.workspace.overlay(uri.clone(), &document.text);
                    }
                    self.publish_open_documents(connection)?;
                }
            }
            "textDocument/didSave" => {
                let params =
                    notification_params!(DidSaveTextDocumentParams, "didSave notification");
                let uri = params.text_document.uri;
                self.dirty_disk_uris.insert(uri.clone());
                if let Some(text) = params.text {
                    self.documents.save_text(&uri, text);
                }
                if let Some(document) = self.documents.get(&uri) {
                    self.workspace.overlay(uri.clone(), &document.text);
                }
                self.publish_open_documents(connection)?;
            }
            "textDocument/didClose" => {
                let params =
                    notification_params!(DidCloseTextDocumentParams, "didClose notification");
                let uri = params.text_document.uri;
                self.dirty_disk_uris.insert(uri.clone());
                self.documents.close(&uri);
                self.workspace.close(&uri);
                publish(connection, uri, Vec::new(), None)?;
                self.publish_open_documents(connection)?;
            }
            "window/workDoneProgress/cancel" => {
                let params = notification_params!(
                    WorkDoneProgressCancelParams,
                    "workDoneProgress cancel notification"
                );
                if params.token == progress_token() {
                    self.cancel_indexing();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn publish(&mut self, connection: &Connection, uri: &Uri) -> ServerResult<()> {
        let Some(document) = self.documents.get(uri) else {
            return Ok(());
        };
        let mut diagnostics = self.analyzer.diagnostics(&document.text);
        if diagnostics.is_empty() && self.indexing.is_none() && self.indexing_error.is_none() {
            diagnostics.extend(self.workspace.semantic_diagnostics(uri));
        }
        publish(connection, uri.clone(), diagnostics, document.version)
    }

    fn publish_open_documents(&mut self, connection: &Connection) -> ServerResult<()> {
        let uris = self
            .documents
            .iter()
            .map(|(uri, _)| uri.clone())
            .collect::<Vec<_>>();
        for uri in uris {
            self.publish(connection, &uri)?;
        }
        Ok(())
    }

    fn publish_indexed_semantic_diagnostics(
        &mut self,
        connection: &Connection,
    ) -> ServerResult<()> {
        let uris = self
            .documents
            .iter()
            .filter(|(_, document)| self.analyzer.diagnostics(&document.text).is_empty())
            .filter_map(|(uri, document)| {
                let diagnostics = self.workspace.semantic_diagnostics(uri);
                (!diagnostics.is_empty()).then(|| (uri.clone(), document.version, diagnostics))
            })
            .collect::<Vec<_>>();
        for (uri, version, diagnostics) in uris {
            publish(connection, uri, diagnostics, version)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct RenameSupport {
    document_changes: bool,
    file_rename: bool,
}

impl RenameSupport {
    fn from_initialize(params: &InitializeParams) -> Self {
        let workspace_edit = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.workspace_edit.as_ref());
        let document_changes = workspace_edit
            .and_then(|capabilities| capabilities.document_changes)
            .unwrap_or(false);
        let file_rename = document_changes
            && workspace_edit
                .and_then(|capabilities| capabilities.resource_operations.as_deref())
                .is_some_and(|operations| operations.contains(&ResourceOperationKind::Rename));
        Self {
            document_changes,
            file_rename,
        }
    }
}

fn deserialize_request<T>(value: serde_json::Value, name: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(value).map_err(|error| format!("invalid {name} request: {error}"))
}

fn deserialize_notification<T>(value: serde_json::Value, name: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(value).map_err(|error| format!("invalid {name}: {error}"))
}

fn progress_token() -> NumberOrString {
    NumberOrString::String(INDEX_PROGRESS_TOKEN.to_owned())
}

fn requires_complete_index(method: &str) -> bool {
    matches!(
        method,
        "workspace/symbol"
            | "textDocument/completion"
            | "textDocument/hover"
            | "textDocument/definition"
            | "textDocument/references"
            | "textDocument/prepareRename"
            | "textDocument/rename"
            | "textDocument/signatureHelp"
    )
}

fn progress_message(progress: &IndexingProgress) -> String {
    progress.message.clone().unwrap_or_else(|| {
        format!(
            "{}: {} files, {} MiB",
            progress.phase,
            progress.files,
            progress.bytes / (1024 * 1024)
        )
    })
}

fn send_progress(connection: &Connection, progress: WorkDoneProgress) -> ServerResult<()> {
    connection
        .sender
        .send(Message::Notification(Notification::new(
            "$/progress".to_owned(),
            ProgressParams {
                token: progress_token(),
                value: ProgressParamsValue::WorkDone(progress),
            },
        )))?;
    Ok(())
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
