use std::collections::HashMap;

use lsp_types::Uri;

/// An open text document tracked by the language server.
pub(crate) struct Document {
    /// Current full text supplied by the client.
    pub(crate) text: String,
    /// Most recent client document version.
    pub(crate) version: Option<i32>,
}

/// In-memory store for documents currently open in the editor.
#[derive(Default)]
pub(crate) struct DocumentStore {
    documents: HashMap<Uri, Document>,
}

impl DocumentStore {
    /// Inserts or replaces an open document.
    pub(crate) fn open(&mut self, uri: Uri, text: String, version: i32) {
        self.documents.insert(
            uri,
            Document {
                text,
                version: Some(version),
            },
        );
    }

    /// Replaces the full text and version of an existing document.
    pub(crate) fn change(&mut self, uri: &Uri, text: String, version: i32) -> bool {
        let Some(document) = self.documents.get_mut(uri) else {
            return false;
        };
        document.text = text;
        document.version = Some(version);
        true
    }

    /// Replaces document text included with a save notification.
    pub(crate) fn save_text(&mut self, uri: &Uri, text: String) -> bool {
        let Some(document) = self.documents.get_mut(uri) else {
            return false;
        };
        document.text = text;
        true
    }

    /// Returns an open document by URI.
    pub(crate) fn get(&self, uri: &Uri) -> Option<&Document> {
        self.documents.get(uri)
    }

    /// Removes a closed document.
    pub(crate) fn close(&mut self, uri: &Uri) -> Option<Document> {
        self.documents.remove(uri)
    }
}
