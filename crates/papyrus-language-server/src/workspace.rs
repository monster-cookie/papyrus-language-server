use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Read as _,
    path::{Path, PathBuf},
    str::FromStr,
    time::Instant,
};

mod inference;
mod navigation;
mod rename;
mod scanning;
mod validation;

use lsp_types::{
    CompletionItem, CompletionItemKind, DocumentSymbol, Documentation, Location, Position,
    SymbolInformation, Uri,
};

use crate::{
    config::WorkspaceConfig,
    index_cache::{CachedDocument, IndexCache},
    indexing::{IndexingControl, IndexingLimits},
    line_index::LineIndex,
    semantic::{Declaration, DeclarationKind, SemanticDocument, SemanticExtractor},
    source_filter::is_generated_source,
};

pub(crate) struct WorkspaceIndex {
    config: WorkspaceConfig,
    documents: HashMap<Uri, IndexedDocument>,
    semantic_extractor: SemanticExtractor,
    scripts_by_name: HashMap<String, Vec<Uri>>,
    occurrences_by_name: HashMap<String, Vec<(Uri, usize)>>,
    index_cache: IndexCache,
}

struct IndexedDocument {
    path: Option<PathBuf>,
    priority: u8,
    symbols: Vec<DocumentSymbol>,
    semantic: SemanticDocument,
    content_hash: blake3::Hash,
}

enum DiskIndexResult {
    Ignored,
    Indexed(u64),
    TooLarge,
}

impl WorkspaceIndex {
    #[cfg(test)]
    pub(crate) fn new(config: &WorkspaceConfig) -> Result<Self, String> {
        Self::build(config, None)
    }

    pub(crate) fn new_with_control(
        config: &WorkspaceConfig,
        control: &IndexingControl,
    ) -> Result<Self, String> {
        Self::build(config, Some(control))
    }

    pub(crate) fn empty(config: &WorkspaceConfig) -> Result<Self, String> {
        Ok(Self {
            config: config.clone(),
            documents: HashMap::new(),
            semantic_extractor: SemanticExtractor::new()?,
            scripts_by_name: HashMap::new(),
            occurrences_by_name: HashMap::new(),
            index_cache: IndexCache::load(),
        })
    }

    fn build(config: &WorkspaceConfig, control: Option<&IndexingControl>) -> Result<Self, String> {
        let started = Instant::now();
        let mut index = Self::empty(config)?;
        index.scan(control);
        if control.is_some_and(IndexingControl::is_cancelled) {
            return Err("workspace indexing cancelled".to_owned());
        }
        index.rebuild_lookups();
        if let Err(error) = index.index_cache.save() {
            eprintln!("papyrus-language-server: failed to save semantic index cache: {error}");
        }
        eprintln!(
            "papyrus-language-server: indexed {} files in {} ms (cache hits {}, misses {}, identical aliases {})",
            index.documents.len(),
            started.elapsed().as_millis(),
            index.index_cache.hits,
            index.index_cache.misses,
            index.identical_alias_count()
        );
        Ok(index)
    }

    fn index_disk_file(&mut self, path: &Path) {
        let _ = self.index_disk_file_bounded(path, IndexingLimits::default().max_file_bytes);
    }

    fn index_disk_file_bounded(&mut self, path: &Path, max_bytes: u64) -> DiskIndexResult {
        if !is_papyrus_file(path) {
            return DiskIndexResult::Ignored;
        }
        if self
            .config
            .discovered_import_directories
            .iter()
            .filter_map(|root| path.strip_prefix(root).ok())
            .any(is_generated_source)
        {
            return DiskIndexResult::Ignored;
        }
        let Some(uri) = path_to_file_uri(path) else {
            return DiskIndexResult::Ignored;
        };
        let Ok(metadata) = fs::metadata(path) else {
            return DiskIndexResult::Ignored;
        };
        if metadata.len() > max_bytes {
            return DiskIndexResult::TooLarge;
        }
        let Ok(file) = File::open(path) else {
            return DiskIndexResult::Ignored;
        };
        let mut bytes = Vec::with_capacity(metadata.len().min(max_bytes) as usize);
        let Ok(_) = file
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
        else {
            return DiskIndexResult::Ignored;
        };
        if bytes.len() as u64 > max_bytes {
            return DiskIndexResult::TooLarge;
        };
        let byte_count = bytes.len() as u64;
        let text = String::from_utf8_lossy(&bytes);
        let content_hash = normalized_content_hash(&text);
        let priority = self.path_priority(path);
        if let Some(mut cached) = self.index_cache.get(path, content_hash) {
            cached.semantic.uri = uri.clone();
            cached.semantic.text = text.into_owned();
            self.documents.insert(
                uri,
                IndexedDocument {
                    path: Some(path.to_owned()),
                    priority,
                    symbols: cached.symbols,
                    semantic: cached.semantic,
                    content_hash: cached.content_hash,
                },
            );
            return DiskIndexResult::Indexed(byte_count);
        }
        self.index_text(uri, Some(path.to_owned()), priority, &text, true);
        DiskIndexResult::Indexed(byte_count)
    }

    fn path_priority(&self, path: &Path) -> u8 {
        if self
            .config
            .import_directories
            .iter()
            .any(|root| path.starts_with(root))
        {
            2
        } else if self
            .config
            .discovered_import_directories
            .iter()
            .any(|root| path.starts_with(root))
        {
            3
        } else if self
            .config
            .source_roots
            .iter()
            .any(|root| path.starts_with(root))
        {
            1
        } else {
            3
        }
    }

    pub(crate) fn overlay(&mut self, uri: Uri, text: &str) {
        let existing = self.documents.get(&uri);
        let path = existing
            .and_then(|entry| entry.path.clone())
            .or_else(|| self.workspace_path(&uri));
        let priority = existing
            .map(|entry| entry.priority)
            .or_else(|| path.as_deref().map(|path| self.path_priority(path)))
            .unwrap_or(0);
        self.index_text(uri, path, priority, text, false);
        self.rebuild_lookups();
    }

    pub(crate) fn close(&mut self, uri: &Uri) {
        let path = self
            .documents
            .get(uri)
            .and_then(|entry| entry.path.clone())
            .or_else(|| self.workspace_path(uri));
        self.documents.remove(uri);
        if let Some(path) = path {
            self.index_disk_file(&path);
        }
        self.rebuild_lookups();
    }

    fn workspace_path(&self, uri: &Uri) -> Option<PathBuf> {
        let path = crate::config::file_uri_to_path(uri.as_str())?;
        self.config
            .roots()
            .any(|root| path.starts_with(root))
            .then_some(path)
    }

    fn index_text(
        &mut self,
        uri: Uri,
        path: Option<PathBuf>,
        priority: u8,
        text: &str,
        cache: bool,
    ) {
        let content_hash = normalized_content_hash(text);
        let semantic = self.semantic_extractor.extract(uri.clone(), text);
        let symbols = semantic.symbols.clone();
        if cache && let Some(cache_path) = path.as_deref() {
            self.index_cache.insert(
                cache_path,
                &CachedDocument {
                    symbols: symbols.clone(),
                    semantic: semantic.clone(),
                    content_hash,
                },
            );
        }
        self.documents.insert(
            uri.clone(),
            IndexedDocument {
                path,
                priority,
                symbols,
                semantic,
                content_hash,
            },
        );
    }

    fn rebuild_lookups(&mut self) {
        self.scripts_by_name.clear();
        self.occurrences_by_name.clear();
        for (uri, document) in &self.documents {
            if let Some(name) = &document.semantic.script_name {
                self.scripts_by_name
                    .entry(name.to_ascii_lowercase())
                    .or_default()
                    .push(uri.clone());
            }
            for (index, occurrence) in document.semantic.occurrences.iter().enumerate() {
                self.occurrences_by_name
                    .entry(occurrence.name.to_ascii_lowercase())
                    .or_default()
                    .push((uri.clone(), index));
            }
        }
    }

    fn identical_alias_count(&self) -> usize {
        self.scripts_by_name
            .values()
            .map(|uris| {
                let identities = uris
                    .iter()
                    .filter_map(|uri| self.documents.get(uri))
                    .map(|document| document.content_hash)
                    .collect::<HashSet<_>>();
                uris.len().saturating_sub(identities.len())
            })
            .sum()
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
        let receiver = current
            .semantic
            .member_accesses
            .iter()
            .filter(|access| access.contains_offset(offset))
            .min_by_key(|access| access.span())
            .map(|access| &access.receiver);
        let declarations = if let Some(receiver) = receiver {
            inference::members_for_expression(self, current, receiver)
                .into_iter()
                .filter(|declaration| is_instance_completion_candidate(declaration))
                .collect()
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
            for module in &current.semantic.imports {
                if let Some((document, _)) = self.unique_script(module) {
                    visible.extend(document.semantic.declarations.iter().filter(|declaration| {
                        declaration.kind == DeclarationKind::Struct
                            || (declaration.kind == DeclarationKind::Function
                                && declaration.is_global)
                    }));
                }
            }
            visible.extend(self.unique_scripts());
            visible
        };
        let mut items = deduplicated_completion_items(declarations);
        if receiver.is_none() {
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
}

fn is_instance_completion_candidate(declaration: &Declaration) -> bool {
    !declaration.is_const
        && !declaration
            .name
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("CONST_"))
}

fn normalized_content_hash(text: &str) -> blake3::Hash {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    blake3::hash(normalized.as_bytes())
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

fn receiver_before_dot(text: &str, offset: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let mut index = offset.min(bytes.len());
    while index > 0 && is_identifier(bytes[index - 1]) {
        index -= 1;
    }
    while index > 0 && is_inline_whitespace(bytes[index - 1]) {
        index -= 1;
    }
    if index == 0 || bytes[index - 1] != b'.' {
        return None;
    }
    index -= 1;
    while index > 0 && is_inline_whitespace(bytes[index - 1]) {
        index -= 1;
    }
    let end = index;
    while index > 0 && (is_identifier(bytes[index - 1]) || bytes[index - 1] == b':') {
        index -= 1;
    }
    let receiver = &text[index..end];
    is_qualified_identifier(receiver).then(|| receiver.to_owned())
}

fn is_qualified_identifier(value: &str) -> bool {
    value
        .split(':')
        .all(|segment| !segment.is_empty() && segment.bytes().all(is_identifier))
}

fn is_identifier(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_inline_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
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
    path_to_file_uri_lexical(&absolute)
}

fn path_to_file_uri_lexical(path: &Path) -> Option<Uri> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let display = normalize_windows_path(&absolute.to_string_lossy()).replace('\\', "/");
    let prefix = if display.starts_with("//") {
        "file:"
    } else if display.starts_with('/') {
        "file://"
    } else {
        "file:///"
    };
    Uri::from_str(&format!("{prefix}{}", percent_encode_path(&display))).ok()
}

fn normalize_windows_path(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
    }
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

    use super::{DiskIndexResult, WorkspaceIndex, normalize_windows_path, path_to_file_uri};

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
            "  Target.BaseMember()\n",
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
        let references = index.references(
            &path_to_file_uri(&root.join("ScriptObject.psc")).unwrap(),
            Position::new(1, 10),
            false,
        );
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].uri, uri);
        assert_eq!(references[0].range.start.line, 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_grouped_properties_and_rejects_array_element_members() {
        let root = temp_root("grouped-and-array-members");
        let actor_path = root.join("Actor.psc");
        fs::write(
            &actor_path,
            concat!(
                "ScriptName Actor\n",
                "Group Configuration\n",
                "  Bool Property Grouped Auto\n",
                "EndGroup\n",
                "Function Jump()\n",
                "EndFunction\n",
            ),
        )
        .unwrap();
        fs::write(root.join("Child.psc"), "ScriptName Child Extends Actor\n").unwrap();
        let project_path = root.join("Project.psc");
        fs::write(
            &project_path,
            concat!(
                "ScriptName Project\n",
                "Child Target\n",
                "Child[] Targets\n",
                "Function Test()\n",
                "  Target.\n",
                "  Target.Grouped\n",
                "  Targets.\n",
                "  Targets.Jump()\n",
                "EndFunction\n",
            ),
        )
        .unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let project_uri = path_to_file_uri(&project_path).unwrap();

        let target_members = index.completion(&project_uri, Position::new(4, 9));
        assert!(target_members.iter().any(|item| item.label == "Grouped"));
        assert!(target_members.iter().any(|item| item.label == "Jump"));
        assert_eq!(
            index
                .definition(&project_uri, Position::new(5, 11))
                .unwrap()
                .uri,
            path_to_file_uri(&actor_path).unwrap()
        );

        assert!(
            index
                .completion(&project_uri, Position::new(6, 10))
                .is_empty()
        );
        assert!(index.hover(&project_uri, Position::new(7, 12)).is_none());
        assert!(
            index
                .definition(&project_uri, Position::new(7, 12))
                .is_none()
        );
        assert!(
            index
                .signature_help(&project_uri, Position::new(7, 15))
                .is_none()
        );
        assert!(
            index
                .prepare_rename(&project_uri, Position::new(7, 12), false)
                .is_none()
        );
        assert!(
            index
                .references(
                    &path_to_file_uri(&actor_path).unwrap(),
                    Position::new(4, 10),
                    false,
                )
                .is_empty()
        );
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
        assert_eq!(hover.range.unwrap().start.line, 3);
        let definition = index.definition(&uri, position).unwrap();
        assert_eq!(definition.uri, path_to_file_uri(&actor_path).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn script_qualified_globals_resolve_from_discovered_sources_and_prefer_projects() {
        let root = temp_root("script-qualified");
        let project_root = root.join("project");
        let sdk_root = root.join("Fragments").join("sdk");
        fs::create_dir_all(&project_root).unwrap();
        fs::create_dir_all(sdk_root.join("Fragments")).unwrap();
        let sdk_game_path = sdk_root.join("Game.psc");
        fs::write(
            &sdk_game_path,
            "ScriptName Game Hidden\nActor Function GetPlayer() Native Global\n",
        )
        .unwrap();
        fs::write(sdk_root.join("Fragments/QF_Sdk.psc"), "ScriptName QF_Sdk\n").unwrap();
        fs::write(
            project_root.join("QF_Project.psc"),
            "ScriptName QF_Project\n",
        )
        .unwrap();
        let project_path = project_root.join("Project.psc");
        let project = "ScriptName Project\nFunction Test()\n  Game.GetPlayer()\nEndFunction\n";
        fs::write(&project_path, project).unwrap();
        let mut config = WorkspaceConfig {
            source_roots: vec![project_root.clone()],
            ..WorkspaceConfig::default()
        };
        config.add_discovered_import(sdk_root.clone());
        let index = WorkspaceIndex::new(&config).unwrap();
        let project_uri = path_to_file_uri(&project_path).unwrap();
        let sdk_game_uri = path_to_file_uri(&sdk_game_path).unwrap();

        assert!(index.unique_script("QF_Sdk").is_none());
        assert!(index.unique_script("QF_Project").is_some());
        assert_eq!(
            index
                .definition(&project_uri, Position::new(2, 3))
                .unwrap()
                .uri,
            sdk_game_uri
        );
        assert_eq!(
            index
                .definition(&project_uri, Position::new(2, 9))
                .unwrap()
                .uri,
            sdk_game_uri
        );
        assert!(
            format!(
                "{:?}",
                index
                    .hover(&project_uri, Position::new(2, 9))
                    .unwrap()
                    .contents
            )
            .contains("GetPlayer")
        );
        let references = index.references(&sdk_game_uri, Position::new(1, 16), false);
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].uri, project_uri);
        assert_eq!(references[0].range.start.line, 2);

        let project_game_path = project_root.join("Game.psc");
        fs::write(
            &project_game_path,
            "ScriptName Game Hidden\n{Project override}\nActor Function GetPlayer() Native Global\n",
        )
        .unwrap();
        let index = WorkspaceIndex::new(&config).unwrap();
        assert_eq!(
            index
                .definition(&project_uri, Position::new(2, 9))
                .unwrap()
                .uri,
            path_to_file_uri(&project_game_path).unwrap()
        );

        fs::write(
            &project_path,
            "ScriptName Project\nGame Game\nFunction Test()\n  Game.GetPlayer()\nEndFunction\n",
        )
        .unwrap();
        let index = WorkspaceIndex::new(&config).unwrap();
        assert!(
            index
                .definition(&project_uri, Position::new(3, 9))
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opening_a_nested_import_preserves_project_source_precedence() {
        let root = temp_root("overlay-priority");
        let project_root = root.join("project");
        let import_root = project_root.join("vendor");
        fs::create_dir_all(&import_root).unwrap();
        let project_shared = project_root.join("Shared.psc");
        let imported_shared = import_root.join("Shared.psc");
        fs::write(
            &project_shared,
            "ScriptName Shared\nFunction ProjectVersion() Global\nEndFunction\n",
        )
        .unwrap();
        fs::write(
            &imported_shared,
            "ScriptName Shared\nFunction ImportVersion() Global\nEndFunction\n",
        )
        .unwrap();
        let mut index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![project_root.clone()],
            import_directories: vec![import_root],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let project_uri = path_to_file_uri(&project_shared).unwrap();
        let imported_uri = path_to_file_uri(&imported_shared).unwrap();

        assert_eq!(
            index.unique_script("Shared").unwrap().0.semantic.uri,
            project_uri
        );
        assert_eq!(index.documents.get(&imported_uri).unwrap().priority, 2);
        index.overlay(
            imported_uri.clone(),
            "ScriptName Shared\nFunction UnsavedImportVersion() Global\nEndFunction\n",
        );
        assert_eq!(index.documents.get(&imported_uri).unwrap().priority, 2);
        assert_eq!(
            index.unique_script("Shared").unwrap().0.semantic.uri,
            project_uri
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires PAPYRUS_AUDIT_ROOT to reference locally installed game sources"]
    fn installed_source_navigation_audit() {
        let installed_root = std::env::var_os("PAPYRUS_AUDIT_ROOT")
            .map(std::path::PathBuf::from)
            .expect("PAPYRUS_AUDIT_ROOT must reference an installed source directory");
        let installed_game_path = installed_root.join("Game.psc");
        assert!(installed_game_path.is_file());
        let root = temp_root("installed-navigation");
        let project_path = root.join("Project.psc");
        fs::write(
            &project_path,
            "ScriptName Project\nFunction Test()\n  Game.GetPlayer()\nEndFunction\n",
        )
        .unwrap();
        let mut config = WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        };
        config.add_discovered_import(installed_root);
        let index = WorkspaceIndex::new(&config).unwrap();
        let project_uri = path_to_file_uri(&project_path).unwrap();
        let installed_game_uri = path_to_file_uri(&installed_game_path).unwrap();
        assert_eq!(
            index
                .definition(&project_uri, Position::new(2, 3))
                .unwrap()
                .uri,
            installed_game_uri
        );
        assert_eq!(
            index
                .definition(&project_uri, Position::new(2, 9))
                .unwrap()
                .uri,
            installed_game_uri
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finds_resolved_references_across_scopes_overlays_and_cached_text() {
        let root = temp_root("references");
        let actor_path = root.join("Actor.psc");
        fs::write(
            &actor_path,
            "ScriptName Actor\nFunction Jump()\nEndFunction\n",
        )
        .unwrap();
        let project_path = root.join("Project.psc");
        let project = concat!(
            "ScriptName Project\n",
            "Actor Target\n",
            "Function Test(Actor Other)\n",
            "  Target.Jump()\n",
            "  Other.Jump()\n",
            "  ; Target.Jump()\n",
            "  String Evidence = \"Target.Jump()\"\n",
            "EndFunction\n",
            "Function Shadow()\n",
            "  Actor Target\n",
            "  Target.Jump()\n",
            "EndFunction\n",
        );
        fs::write(&project_path, project).unwrap();
        let mut index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let actor_uri = path_to_file_uri(&actor_path).unwrap();
        let project_uri = path_to_file_uri(&project_path).unwrap();

        let jump_references = index.references(&actor_uri, Position::new(1, 10), false);
        assert_eq!(jump_references.len(), 3);
        assert_eq!(
            jump_references
                .iter()
                .map(|location| location.range.start.line)
                .collect::<Vec<_>>(),
            [3, 4, 10]
        );
        let with_declaration = index.references(&actor_uri, Position::new(1, 10), true);
        assert_eq!(with_declaration.len(), 4);
        assert!(with_declaration.iter().any(|location| {
            location.uri == actor_uri && location.range.start == Position::new(1, 9)
        }));

        let target_references = index.references(&project_uri, Position::new(1, 7), false);
        assert_eq!(target_references.len(), 1);
        assert_eq!(target_references[0].range.start.line, 3);
        let parameter_references = index.references(&project_uri, Position::new(2, 21), false);
        assert_eq!(parameter_references.len(), 1);
        assert_eq!(parameter_references[0].range.start.line, 4);

        for document in index.documents.values_mut() {
            document.semantic.text.clear();
        }
        assert_eq!(
            index
                .references(&actor_uri, Position::new(1, 10), false)
                .len(),
            3
        );

        let overlay = project.replace("  Target.Jump()\n", "  Target.Jump()\n  Target.Jump()\n");
        index.overlay(project_uri.clone(), &overlay);
        assert_eq!(
            index
                .references(&actor_uri, Position::new(1, 10), false)
                .len(),
            5
        );
        index.close(&project_uri);
        assert_eq!(
            index
                .references(&actor_uri, Position::new(1, 10), false)
                .len(),
            3
        );
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
        fs::write(
            root.join("Factory.psc"),
            "ScriptName Factory\nActor Function GetActor()\nEndFunction\n",
        )
        .unwrap();
        let path = root.join("Project.psc");
        fs::write(
            &path,
            concat!(
                "ScriptName Project\n",
                "Actor Target\n",
                "Factory Source\n",
                "Function Test()\n",
                "  Target.One()\n",
                "  Source.GetActor().\n",
                "EndFunction\n",
            ),
        )
        .unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        assert!(
            index
                .completion(&path_to_file_uri(&path).unwrap(), Position::new(4, 9))
                .is_empty()
        );
        assert!(
            index
                .completion(&path_to_file_uri(&path).unwrap(), Position::new(5, 20))
                .is_empty()
        );
        let global = index.completion(&path_to_file_uri(&path).unwrap(), Position::new(1, 0));
        assert!(!global.iter().any(|item| item.label == "Actor"));
        assert!(
            index
                .references(
                    &path_to_file_uri(&path).unwrap(),
                    Position::new(4, 11),
                    false
                )
                .is_empty()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn identical_duplicate_scripts_collapse_but_conflicts_remain_ambiguous() {
        let root = temp_root("duplicates");
        fs::create_dir_all(root.join("Papyrus")).unwrap();
        fs::create_dir_all(root.join("Staging")).unwrap();
        let actor = concat!(
            "ScriptName Actor\n",
            "Function Jump()\nEndFunction\n",
            "Function Test()\n  Jump()\nEndFunction\n",
        );
        fs::write(root.join("Papyrus/Actor.psc"), actor).unwrap();
        fs::write(root.join("Staging/Actor.psc"), actor.replace('\n', "\r\n")).unwrap();
        let project_path = root.join("Project.psc");
        fs::write(
            &project_path,
            "ScriptName Project\nActor Target\nFunction Test()\n  Target.Jump()\nEndFunction\n",
        )
        .unwrap();
        let config = WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        };
        let index = WorkspaceIndex::new(&config).unwrap();
        let project_uri = path_to_file_uri(&project_path).unwrap();
        let items = index.completion(&project_uri, Position::new(3, 9));
        assert!(items.iter().any(|item| item.label == "Jump"));
        let references = index.references(&project_uri, Position::new(3, 10), false);
        assert_eq!(references.len(), 2);
        assert!(
            references
                .iter()
                .any(|location| location.uri == project_uri)
        );
        assert!(references.iter().any(|location| {
            location
                .uri
                .as_str()
                .to_ascii_lowercase()
                .contains("/papyrus/actor.psc")
        }));
        assert!(!references.iter().any(|location| {
            location
                .uri
                .as_str()
                .to_ascii_lowercase()
                .contains("/staging/actor.psc")
        }));

        fs::write(
            root.join("Staging/Actor.psc"),
            "ScriptName Actor\nFunction Lie()\nEndFunction\n",
        )
        .unwrap();
        let index = WorkspaceIndex::new(&config).unwrap();
        assert!(
            index
                .completion(&project_uri, Position::new(3, 9))
                .is_empty()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_globals_structs_and_struct_members_resolve() {
        let root = temp_root("imports");
        let module_path = root.join("Enumerations.psc");
        fs::write(
            &module_path,
            concat!(
                "ScriptName Venworks:Core:Enumerations\n",
                "Struct LogSeverity\n  Int Info\nEndStruct\n",
                "Function LogSystem() Global\nEndFunction\n",
            ),
        )
        .unwrap();
        let project_path = root.join("Project.psc");
        let project = concat!(
            "ScriptName Project\n",
            "Import Venworks:Core:Enumerations\n",
            "Function Test(ObjectReference akTerminalRef)\n",
            "  LogSeverity severityTable = new LogSeverity\n",
            "  severityTable.Info\n",
            "  LogSystem()\n",
            "  akTerminal\n",
            "EndFunction\n",
        );
        fs::write(&project_path, project).unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let uri = path_to_file_uri(&project_path).unwrap();
        let members = index.completion(&uri, Position::new(4, 16));
        assert!(members.iter().any(|item| item.label == "Info"));
        let visible = index.completion(&uri, Position::new(6, 12));
        assert!(visible.iter().any(|item| item.label == "akTerminalRef"));
        assert!(visible.iter().any(|item| item.label == "LogSeverity"));
        assert!(visible.iter().any(|item| item.label == "LogSystem"));
        assert!(index.hover(&uri, Position::new(5, 5)).is_some());
        assert_eq!(
            index.definition(&uri, Position::new(5, 5)).unwrap().uri,
            path_to_file_uri(&module_path).unwrap()
        );
        let references = index.references(
            &path_to_file_uri(&module_path).unwrap(),
            Position::new(4, 10),
            false,
        );
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].uri, uri);
        assert_eq!(references[0].range.start.line, 5);
        let module_uri = path_to_file_uri(&module_path).unwrap();
        let module_references = index.references(&module_uri, Position::new(0, 12), false);
        assert_eq!(module_references.len(), 1);
        assert_eq!(module_references[0].uri, uri);
        assert_eq!(module_references[0].range.start.line, 1);
        let struct_references = index.references(&module_uri, Position::new(1, 8), false);
        assert_eq!(struct_references.len(), 2);
        assert!(
            struct_references
                .iter()
                .all(|location| location.uri == uri && location.range.start.line == 3)
        );
        let member_references = index.references(&module_uri, Position::new(2, 7), false);
        assert_eq!(member_references.len(), 1);
        assert_eq!(member_references[0].uri, uri);
        assert_eq!(member_references[0].range.start.line, 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn constants_and_globals_are_not_instance_members() {
        let root = temp_root("member-modifiers");
        fs::write(
            root.join("Actor.psc"),
            concat!(
                "ScriptName Actor\n",
                "Int CONST_Distance = 1 Const\n",
                "Int Property CONST_NearDistance_Close = 0 AutoReadOnly\n",
                "Int Property ReadOnlyValue = 1 AutoReadOnly\n",
                "Function Build() Global\nEndFunction\n",
                "Function SetValueInt()\nEndFunction\n",
            ),
        )
        .unwrap();
        let project_path = root.join("Project.psc");
        fs::write(
            &project_path,
            "ScriptName Project\nActor player\nFunction Test()\n  player.\nEndFunction\n",
        )
        .unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let items = index.completion(
            &path_to_file_uri(&project_path).unwrap(),
            Position::new(3, 9),
        );
        assert!(items.iter().any(|item| item.label == "SetValueInt"));
        assert!(items.iter().any(|item| item.label == "ReadOnlyValue"));
        assert!(!items.iter().any(|item| item.label == "CONST_Distance"));
        assert!(
            !items
                .iter()
                .any(|item| item.label == "CONST_NearDistance_Close")
        );
        assert!(!items.iter().any(|item| item.label == "Build"));

        fs::write(
            &project_path,
            "ScriptName Project\nActor player\nFunction Test()\n  player.CONST_NearDistance_Close\nEndFunction\n",
        )
        .unwrap();
        let uri = path_to_file_uri(&project_path).unwrap();
        let mut index = index;
        index.overlay(
            uri.clone(),
            "ScriptName Project\nActor player\nFunction Test()\n  player.CONST_NearDistance_Close\nEndFunction\n",
        );
        assert!(index.hover(&uri, Position::new(3, 12)).is_some());
        assert_eq!(
            index.definition(&uri, Position::new(3, 12)).unwrap().uri,
            path_to_file_uri(&root.join("Actor.psc")).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn closing_an_overlay_for_a_deleted_file_removes_stale_symbols() {
        let root = temp_root("deleted-overlay");
        let path = root.join("Deleted.psc");
        fs::write(&path, "ScriptName Deleted\n").unwrap();
        let uri = path_to_file_uri(&path).unwrap();
        let mut index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        index.overlay(
            uri.clone(),
            "ScriptName Deleted\nFunction Unsaved()\nEndFunction\n",
        );
        assert!(!index.document_symbols(&uri).is_empty());
        fs::remove_file(&path).unwrap();
        index.close(&uri);
        assert!(index.document_symbols(&uri).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bounded_disk_indexing_rejects_an_oversized_source() {
        let root = temp_root("oversized-source");
        let path = root.join("Large.psc");
        fs::write(&path, "ScriptName Large\n").unwrap();
        let mut index = WorkspaceIndex::empty(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        assert!(matches!(
            index.index_disk_file_bounded(&path, 4),
            DiskIndexResult::TooLarge
        ));
        assert!(index.documents.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normalizes_windows_extended_paths() {
        assert_eq!(
            normalize_windows_path(r"\\?\C:\Repositories\Project\Script.psc"),
            r"C:\Repositories\Project\Script.psc"
        );
        assert_eq!(
            normalize_windows_path(r"\\?\UNC\server\share\Script.psc"),
            r"\\server\share\Script.psc"
        );
    }

    #[cfg(windows)]
    #[test]
    fn converts_windows_extended_paths_to_file_uris() {
        assert_eq!(
            path_to_file_uri(std::path::Path::new(
                r"\\?\C:\Repositories\Project\Script.psc"
            ))
            .unwrap()
            .as_str(),
            "file:///C:/Repositories/Project/Script.psc"
        );
        assert_eq!(
            path_to_file_uri(std::path::Path::new(r"\\?\UNC\server\share\Script.psc"))
                .unwrap()
                .as_str(),
            "file://server/share/Script.psc"
        );
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
