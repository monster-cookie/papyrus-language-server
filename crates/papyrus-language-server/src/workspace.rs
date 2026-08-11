use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use lsp_types::{DocumentSymbol, Location, SymbolInformation, Uri};

use crate::{config::WorkspaceConfig, symbols::SymbolExtractor};

pub(crate) struct WorkspaceIndex {
    roots: Vec<PathBuf>,
    documents: HashMap<Uri, IndexedDocument>,
    extractor: SymbolExtractor,
}

struct IndexedDocument {
    path: Option<PathBuf>,
    symbols: Vec<DocumentSymbol>,
}

impl WorkspaceIndex {
    pub(crate) fn new(config: &WorkspaceConfig) -> Result<Self, String> {
        let mut index = Self {
            roots: config.roots().cloned().collect(),
            documents: HashMap::new(),
            extractor: SymbolExtractor::new()?,
        };
        index.scan();
        Ok(index)
    }

    fn scan(&mut self) {
        let mut visited = HashSet::new();
        for root in self.roots.clone() {
            self.scan_path(&root, &mut visited);
        }
    }

    fn scan_path(&mut self, path: &Path, visited: &mut HashSet<PathBuf>) {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if !visited.insert(canonical) {
            return;
        }
        if path.is_file() {
            self.index_disk_file(path);
            return;
        }
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() && !file_type.is_symlink() {
                self.scan_path(&path, visited);
            } else if file_type.is_file() {
                self.index_disk_file(&path);
            }
        }
    }

    fn index_disk_file(&mut self, path: &Path) {
        if !is_papyrus_file(path) {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        let Some(uri) = path_to_file_uri(path) else {
            return;
        };
        self.index_text(uri, Some(path.to_owned()), &text);
    }

    pub(crate) fn overlay(&mut self, uri: Uri, text: &str) {
        let path = self
            .documents
            .get(&uri)
            .and_then(|entry| entry.path.clone());
        self.index_text(uri, path, text);
    }

    pub(crate) fn close(&mut self, uri: &Uri) {
        let path = self.documents.get(uri).and_then(|entry| entry.path.clone());
        if let Some(path) = path {
            self.index_disk_file(&path);
        } else {
            self.documents.remove(uri);
        }
    }

    fn index_text(&mut self, uri: Uri, path: Option<PathBuf>, text: &str) {
        self.documents.insert(
            uri,
            IndexedDocument {
                path,
                symbols: self.extractor.extract(text),
            },
        );
    }

    pub(crate) fn document_symbols(&self, uri: &Uri) -> Vec<DocumentSymbol> {
        self.documents
            .get(uri)
            .map(|document| document.symbols.clone())
            .unwrap_or_default()
    }

    #[allow(deprecated)]
    pub(crate) fn workspace_symbols(&self, query: &str) -> Vec<SymbolInformation> {
        let query = query.to_ascii_lowercase();
        let mut output = Vec::new();
        for (uri, document) in &self.documents {
            flatten_symbols(&document.symbols, uri, None, &query, &mut output);
        }
        output.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left.location.uri.as_str().cmp(right.location.uri.as_str()))
                .then_with(|| left.location.range.start.cmp(&right.location.range.start))
        });
        output
    }
}

#[allow(deprecated)]
fn flatten_symbols(
    symbols: &[DocumentSymbol],
    uri: &Uri,
    container: Option<&str>,
    query: &str,
    output: &mut Vec<SymbolInformation>,
) {
    for symbol in symbols {
        if symbol.name.to_ascii_lowercase().contains(query) {
            output.push(SymbolInformation {
                name: symbol.name.clone(),
                kind: symbol.kind,
                tags: symbol.tags.clone(),
                deprecated: None,
                location: Location::new(uri.clone(), symbol.selection_range),
                container_name: container.map(str::to_owned),
            });
        }
        if let Some(children) = &symbol.children {
            flatten_symbols(children, uri, Some(&symbol.name), query, output);
        }
    }
}

fn is_papyrus_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("psc"))
}

fn path_to_file_uri(path: &Path) -> Option<Uri> {
    let absolute = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let display = absolute.to_string_lossy().replace('\\', "/");
    let prefix = if display.starts_with('/') {
        "file://"
    } else {
        "file:///"
    };
    Uri::from_str(&format!("{prefix}{}", percent_encode_path(&display))).ok()
}

fn percent_encode_path(path: &str) -> String {
    let mut output = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::WorkspaceConfig;

    use super::WorkspaceIndex;

    #[test]
    fn indexes_recursively_and_filters_case_insensitively() {
        let root = std::env::temp_dir().join(format!(
            "papyrus-language-server-workspace-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = root.join("Nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            nested.join("Example.PSC"),
            "ScriptName Example\nFunction Run()\nEndFunction\n",
        )
        .unwrap();
        let config = WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        };
        let index = WorkspaceIndex::new(&config).unwrap();
        let matches = index.workspace_symbols("run");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "Run");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsaved_overlay_is_replaced_by_disk_contents_on_close() {
        let root = std::env::temp_dir().join(format!(
            "papyrus-language-server-overlay-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("Example.psc");
        fs::write(
            &path,
            "ScriptName Example\nFunction OnDisk()\nEndFunction\n",
        )
        .unwrap();
        let config = WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        };
        let mut index = WorkspaceIndex::new(&config).unwrap();
        let uri = super::path_to_file_uri(&path).unwrap();
        index.overlay(
            uri.clone(),
            "ScriptName Example\nFunction Unsaved()\nEndFunction\n",
        );
        assert_eq!(index.workspace_symbols("unsaved").len(), 1);
        index.close(&uri);
        assert!(index.workspace_symbols("unsaved").is_empty());
        assert_eq!(index.workspace_symbols("ondisk").len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preserves_duplicate_script_candidates_and_survives_malformed_files() {
        let root = std::env::temp_dir().join(format!(
            "papyrus-language-server-duplicates-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("one")).unwrap();
        fs::create_dir_all(root.join("two")).unwrap();
        fs::write(root.join("one/Duplicate.psc"), "ScriptName Duplicate\n").unwrap();
        fs::write(root.join("two/Duplicate.psc"), "ScriptName Duplicate\n").unwrap();
        fs::write(root.join("Broken.psc"), "ScriptName Broken\nFunction (").unwrap();
        let config = WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        };
        let index = WorkspaceIndex::new(&config).unwrap();
        assert_eq!(index.workspace_symbols("duplicate").len(), 2);
        assert_eq!(index.workspace_symbols("broken").len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
