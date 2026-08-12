use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use lsp_types::{
    CompletionItem, CompletionItemKind, DocumentSymbol, Documentation, Hover, HoverContents,
    Location, MarkupContent, MarkupKind, Position, SymbolInformation, Uri,
};

use crate::{
    config::WorkspaceConfig,
    line_index::LineIndex,
    semantic::{Declaration, DeclarationKind, SemanticDocument, SemanticExtractor},
    symbols::SymbolExtractor,
};

pub(crate) struct WorkspaceIndex {
    config: WorkspaceConfig,
    documents: HashMap<Uri, IndexedDocument>,
    symbol_extractor: SymbolExtractor,
    semantic_extractor: SemanticExtractor,
}

struct IndexedDocument {
    path: Option<PathBuf>,
    priority: u8,
    symbols: Vec<DocumentSymbol>,
    semantic: SemanticDocument,
}

impl WorkspaceIndex {
    pub(crate) fn new(config: &WorkspaceConfig) -> Result<Self, String> {
        let mut index = Self {
            config: config.clone(),
            documents: HashMap::new(),
            symbol_extractor: SymbolExtractor::new()?,
            semantic_extractor: SemanticExtractor::new()?,
        };
        index.scan();
        Ok(index)
    }

    fn scan(&mut self) {
        let mut visited = HashSet::new();
        let roots = self.config.roots().cloned().collect::<Vec<_>>();
        for root in roots {
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
        let Ok(bytes) = fs::read(path) else {
            return;
        };
        let text = String::from_utf8_lossy(&bytes);
        let Some(uri) = path_to_file_uri(path) else {
            return;
        };
        let priority = self.path_priority(path);
        self.index_text(uri, Some(path.to_owned()), priority, &text);
    }

    fn path_priority(&self, path: &Path) -> u8 {
        if self
            .config
            .source_roots
            .iter()
            .any(|root| path.starts_with(root))
        {
            1
        } else if self
            .config
            .import_directories
            .iter()
            .any(|root| path.starts_with(root))
        {
            2
        } else {
            3
        }
    }

    pub(crate) fn overlay(&mut self, uri: Uri, text: &str) {
        let path = self
            .documents
            .get(&uri)
            .and_then(|entry| entry.path.clone());
        self.index_text(uri, path, 0, text);
    }

    pub(crate) fn close(&mut self, uri: &Uri) {
        let path = self.documents.get(uri).and_then(|entry| entry.path.clone());
        if let Some(path) = path {
            self.index_disk_file(&path);
        } else {
            self.documents.remove(uri);
        }
    }

    fn index_text(&mut self, uri: Uri, path: Option<PathBuf>, priority: u8, text: &str) {
        self.documents.insert(
            uri.clone(),
            IndexedDocument {
                path,
                priority,
                symbols: self.symbol_extractor.extract(text),
                semantic: self.semantic_extractor.extract(uri, text),
            },
        );
    }

    pub(crate) fn document_symbols(&self, uri: &Uri) -> Vec<DocumentSymbol> {
        self.documents
            .get(uri)
            .map(|entry| entry.symbols.clone())
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

    pub(crate) fn completion(&self, uri: &Uri, position: Position) -> Vec<CompletionItem> {
        let Some(current) = self.documents.get(uri) else {
            return Vec::new();
        };
        let offset =
            LineIndex::new(&current.semantic.text).byte_offset(&current.semantic.text, position);
        let declarations =
            if let Some(receiver) = receiver_before_dot(&current.semantic.text, offset) {
                self.resolve_visible_name(current, &receiver, offset)
                    .and_then(|declaration| declaration.ty.as_ref())
                    .and_then(|ty| self.unique_script(&ty.name))
                    .or_else(|| self.unique_script(&receiver))
                    .map(|(_, script)| self.members_of(script))
                    .unwrap_or_default()
            } else {
                let mut visible = current
                    .semantic
                    .declarations
                    .iter()
                    .filter(|declaration| {
                        declaration.scope.contains(&offset) || declaration.container.is_none()
                    })
                    .collect::<Vec<_>>();
                if let Some(script_name) = &current.semantic.script_name
                    && let Some((_, script)) = self.unique_script(script_name)
                {
                    visible.extend(self.members_of(script));
                }
                visible.extend(self.unique_scripts());
                visible
            };
        let mut items = deduplicated_completion_items(declarations);
        if receiver_before_dot(&current.semantic.text, offset).is_none() {
            for primitive in ["Bool", "Float", "Int", "String", "Var"] {
                if !items
                    .iter()
                    .any(|item| item.label.eq_ignore_ascii_case(primitive))
                {
                    items.push(CompletionItem {
                        label: primitive.to_owned(),
                        kind: Some(CompletionItemKind::TYPE_PARAMETER),
                        detail: Some("Papyrus primitive type".to_owned()),
                        ..CompletionItem::default()
                    });
                }
            }
            items.sort_by_key(|item| item.label.to_ascii_lowercase());
        }
        items
    }

    pub(crate) fn hover(&self, uri: &Uri, position: Position) -> Option<Hover> {
        let declaration = self.resolve_at(uri, position)?;
        let mut value = format!("```papyrus\n{}\n```", declaration.signature());
        if let Some(owner) = &declaration.owner_script {
            value.push_str(&format!("\n\nDeclared by `{owner}`."));
        }
        if let Some(documentation) = &declaration.documentation {
            value.push_str("\n\n");
            value.push_str(documentation);
        }
        if let Some((source_uri, _)) = self.declaration_location(declaration) {
            value.push_str(&format!("\n\nSource: `{}`", source_uri.as_str()));
        }
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(declaration.selection_range),
        })
    }

    pub(crate) fn definition(&self, uri: &Uri, position: Position) -> Option<Location> {
        let declaration = self.resolve_at(uri, position)?;
        let (source_uri, range) = self.declaration_location(declaration)?;
        Some(Location::new(source_uri, range))
    }

    fn declaration_location(&self, declaration: &Declaration) -> Option<(Uri, lsp_types::Range)> {
        let document = self.documents.values().find(|entry| {
            entry
                .semantic
                .declarations
                .iter()
                .any(|candidate| std::ptr::eq(candidate, declaration))
        })?;
        Some((document.semantic.uri.clone(), declaration.selection_range))
    }

    fn resolve_at(&self, uri: &Uri, position: Position) -> Option<&Declaration> {
        let current = self.documents.get(uri)?;
        let offset =
            LineIndex::new(&current.semantic.text).byte_offset(&current.semantic.text, position);
        let name = word_at(&current.semantic.text, offset)?;
        if let Some(receiver) = receiver_before_member(&current.semantic.text, offset) {
            let receiver_declaration = self.resolve_visible_name(current, &receiver, offset)?;
            let script = self
                .unique_script(&receiver_declaration.ty.as_ref()?.name)?
                .1;
            return unique_named(self.members_of(script), &name);
        }
        self.resolve_visible_name(current, &name, offset)
            .or_else(|| {
                self.unique_script(&name)
                    .map(|(_, declaration)| declaration)
            })
    }

    fn resolve_visible_name<'a>(
        &'a self,
        current: &'a IndexedDocument,
        name: &str,
        offset: usize,
    ) -> Option<&'a Declaration> {
        let scoped = current
            .semantic
            .declarations
            .iter()
            .filter(|declaration| declaration.name.eq_ignore_ascii_case(name))
            .filter(|declaration| {
                declaration.scope.contains(&offset) && declaration.container.is_some()
            })
            .collect::<Vec<_>>();
        if let Some(declaration) = unique_named(scoped, name) {
            return Some(declaration);
        }
        let top_level = current
            .semantic
            .declarations
            .iter()
            .filter(|declaration| {
                declaration.name.eq_ignore_ascii_case(name) && declaration.container.is_none()
            })
            .collect::<Vec<_>>();
        if let Some(declaration) = unique_named(top_level, name) {
            return Some(declaration);
        }
        let script = current
            .semantic
            .script_name
            .as_deref()
            .and_then(|name| self.unique_script(name).map(|value| value.1))?;
        unique_named(self.members_of(script), name)
    }

    fn unique_scripts(&self) -> Vec<&Declaration> {
        let names = self
            .documents
            .values()
            .flat_map(|entry| &entry.semantic.declarations)
            .filter(|declaration| declaration.kind == DeclarationKind::Script)
            .map(|declaration| declaration.name.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        names
            .iter()
            .filter_map(|name| self.unique_script(name).map(|(_, declaration)| declaration))
            .collect()
    }

    fn unique_script(&self, name: &str) -> Option<(&IndexedDocument, &Declaration)> {
        let mut candidates = self
            .documents
            .values()
            .filter_map(|entry| {
                entry
                    .semantic
                    .declarations
                    .iter()
                    .find(|declaration| {
                        declaration.kind == DeclarationKind::Script
                            && declaration.name.eq_ignore_ascii_case(name)
                    })
                    .map(|declaration| (entry, declaration))
            })
            .collect::<Vec<_>>();
        let priority = candidates.iter().map(|(entry, _)| entry.priority).min()?;
        candidates.retain(|(entry, _)| entry.priority == priority);
        (candidates.len() == 1).then(|| candidates.remove(0))
    }

    fn members_of<'a>(&'a self, script: &'a Declaration) -> Vec<&'a Declaration> {
        let mut members = Vec::new();
        let mut visited = HashSet::new();
        let mut current = Some(script.name.clone());
        while let Some(name) = current {
            if !visited.insert(name.to_ascii_lowercase()) {
                break;
            }
            let Some((document, _)) = self.unique_script(&name) else {
                break;
            };
            for declaration in &document.semantic.declarations {
                if declaration
                    .owner_script
                    .as_deref()
                    .is_some_and(|owner| owner.eq_ignore_ascii_case(&name))
                    && declaration.container.is_none()
                    && !matches!(
                        declaration.kind,
                        DeclarationKind::Script | DeclarationKind::Parameter
                    )
                    && !members.iter().any(|existing: &&Declaration| {
                        existing.name.eq_ignore_ascii_case(&declaration.name)
                    })
                {
                    members.push(declaration);
                }
            }
            current = document.semantic.parent_script.clone();
        }
        members
    }
}

fn deduplicated_completion_items(declarations: Vec<&Declaration>) -> Vec<CompletionItem> {
    let mut seen = HashSet::new();
    let mut output = declarations
        .into_iter()
        .filter(|declaration| seen.insert(declaration.name.to_ascii_lowercase()))
        .map(|declaration| CompletionItem {
            label: declaration.name.clone(),
            kind: Some(completion_kind(declaration.kind)),
            detail: Some(declaration.signature()),
            documentation: declaration.documentation.clone().map(Documentation::String),
            ..CompletionItem::default()
        })
        .collect::<Vec<_>>();
    output.sort_by_key(|item| item.label.to_ascii_lowercase());
    output
}

fn completion_kind(kind: DeclarationKind) -> CompletionItemKind {
    match kind {
        DeclarationKind::Script | DeclarationKind::Struct => CompletionItemKind::CLASS,
        DeclarationKind::Property => CompletionItemKind::PROPERTY,
        DeclarationKind::Function => CompletionItemKind::FUNCTION,
        DeclarationKind::Event => CompletionItemKind::EVENT,
        DeclarationKind::Variable | DeclarationKind::Parameter | DeclarationKind::Guard => {
            CompletionItemKind::VARIABLE
        }
        DeclarationKind::State => CompletionItemKind::MODULE,
    }
}

fn unique_named<'a>(declarations: Vec<&'a Declaration>, name: &str) -> Option<&'a Declaration> {
    let mut matches = declarations
        .into_iter()
        .filter(|declaration| declaration.name.eq_ignore_ascii_case(name));
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn word_at(text: &str, offset: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let mut start = offset.min(bytes.len());
    let mut end = start;
    while start > 0 && is_identifier(bytes[start - 1]) {
        start -= 1;
    }
    while end < bytes.len() && is_identifier(bytes[end]) {
        end += 1;
    }
    (start < end).then(|| text[start..end].to_owned())
}

fn receiver_before_dot(text: &str, offset: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let mut index = offset.min(bytes.len());
    while index > 0 && is_identifier(bytes[index - 1]) {
        index -= 1;
    }
    while index > 0 && bytes[index - 1].is_ascii_whitespace() {
        index -= 1;
    }
    if index == 0 || bytes[index - 1] != b'.' {
        return None;
    }
    index -= 1;
    while index > 0 && bytes[index - 1].is_ascii_whitespace() {
        index -= 1;
    }
    let end = index;
    while index > 0 && is_identifier(bytes[index - 1]) {
        index -= 1;
    }
    (index < end).then(|| text[index..end].to_owned())
}

fn receiver_before_member(text: &str, offset: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let mut start = offset.min(bytes.len());
    while start > 0 && is_identifier(bytes[start - 1]) {
        start -= 1;
    }
    receiver_before_dot(text, start)
}

fn is_identifier(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
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

    use lsp_types::Position;

    use crate::WorkspaceConfig;

    use super::{WorkspaceIndex, path_to_file_uri};

    #[test]
    fn completes_only_resolved_members_and_follows_inheritance() {
        let root = temp_root("semantic");
        fs::write(
            root.join("ScriptObject.psc"),
            "ScriptName ScriptObject\nFunction BaseMember()\nEndFunction\n",
        )
        .unwrap();
        fs::write(
            root.join("Actor.psc"),
            "ScriptName Actor Extends ScriptObject\nFunction ActorMember()\nEndFunction\n",
        )
        .unwrap();
        let project = concat!(
            "ScriptName Project\n",
            "Actor Target\n",
            "Function Test()\n",
            "  Target.\n",
            "EndFunction\n",
        );
        let project_path = root.join("Project.psc");
        fs::write(&project_path, project).unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let uri = path_to_file_uri(&project_path).unwrap();
        let items = index.completion(&uri, Position::new(3, 9));
        assert!(items.iter().any(|item| item.label == "ActorMember"));
        assert!(items.iter().any(|item| item.label == "BaseMember"));
        assert!(!items.iter().any(|item| item.label == "Project"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hover_and_definition_share_the_resolved_member() {
        let root = temp_root("definition");
        let actor_path = root.join("Actor.psc");
        fs::write(
            &actor_path,
            "ScriptName Actor\n{Evidence}\nFunction ActorMember()\nEndFunction\n",
        )
        .unwrap();
        let project_path = root.join("Project.psc");
        fs::write(&project_path, "ScriptName Project\nActor Target\nFunction Test()\n  Target.ActorMember()\nEndFunction\n").unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let uri = path_to_file_uri(&project_path).unwrap();
        let position = Position::new(3, 12);
        let hover = index.hover(&uri, position).unwrap();
        assert!(format!("{:?}", hover.contents).contains("ActorMember"));
        let definition = index.definition(&uri, position).unwrap();
        assert_eq!(definition.uri, path_to_file_uri(&actor_path).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ambiguous_receiver_type_returns_no_member_claims() {
        let root = temp_root("ambiguous");
        fs::create_dir_all(root.join("one")).unwrap();
        fs::create_dir_all(root.join("two")).unwrap();
        fs::write(
            root.join("one/Actor.psc"),
            "ScriptName Actor\nFunction One()\nEndFunction\n",
        )
        .unwrap();
        fs::write(
            root.join("two/Actor.psc"),
            "ScriptName Actor\nFunction Two()\nEndFunction\n",
        )
        .unwrap();
        let path = root.join("Project.psc");
        fs::write(
            &path,
            "ScriptName Project\nActor Target\nFunction Test()\n  Target.\nEndFunction\n",
        )
        .unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        assert!(
            index
                .completion(&path_to_file_uri(&path).unwrap(), Position::new(3, 9))
                .is_empty()
        );
        let global = index.completion(&path_to_file_uri(&path).unwrap(), Position::new(1, 0));
        assert!(!global.iter().any(|item| item.label == "Actor"));
        fs::remove_dir_all(root).unwrap();
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "papyrus-{label}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
