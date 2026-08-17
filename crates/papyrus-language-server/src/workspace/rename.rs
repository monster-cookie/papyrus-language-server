use std::collections::{HashMap, HashSet};

use lsp_types::{
    DocumentChangeOperation, DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier,
    Position, PrepareRenameResponse, RenameFile, ResourceOp, TextDocumentEdit, TextEdit, Uri,
    WorkspaceEdit,
};

use crate::semantic::{Declaration, DeclarationKind, SemanticOccurrence};

use super::{IndexedDocument, WorkspaceIndex, path_to_file_uri, path_to_file_uri_lexical};

const PAPYRUS_KEYWORDS: &[&str] = &[
    "as",
    "auto",
    "autoreadonly",
    "betaonly",
    "bool",
    "collapsed",
    "collapsedonbase",
    "collapsedonref",
    "collapseonbase",
    "collapseonref",
    "conditional",
    "const",
    "customevent",
    "debugonly",
    "default",
    "else",
    "elseif",
    "elselockguard",
    "elsetrylockguard",
    "endevent",
    "endfunction",
    "endgroup",
    "endif",
    "endlockguard",
    "endproperty",
    "endstate",
    "endstruct",
    "endtrylockguard",
    "endwhile",
    "event",
    "extends",
    "false",
    "float",
    "function",
    "global",
    "group",
    "guard",
    "hidden",
    "if",
    "import",
    "int",
    "internal",
    "is",
    "lockguard",
    "mandatory",
    "native",
    "new",
    "none",
    "private",
    "property",
    "protected",
    "protectsfunctionlogic",
    "public",
    "requiresguard",
    "return",
    "scriptname",
    "selfonly",
    "state",
    "string",
    "struct",
    "true",
    "trylockguard",
    "var",
    "while",
];

impl WorkspaceIndex {
    pub(crate) fn prepare_rename(
        &self,
        uri: &Uri,
        position: Position,
        supports_file_rename: bool,
    ) -> Option<PrepareRenameResponse> {
        let current = self.documents.get(uri)?;
        let range = self.selection_range_at(current, position)?;
        let declaration = self.canonical_declaration(self.resolve_at(uri, position)?);
        self.rename_target_document(declaration)?;
        if declaration.kind == DeclarationKind::Script && !supports_file_rename {
            return None;
        }
        Some(PrepareRenameResponse::RangeWithPlaceholder {
            range,
            placeholder: declaration.name.clone(),
        })
    }

    // WorkspaceEdit's legacy `changes` fallback requires HashMap<Uri, _>; URI cache state is not mutated.
    #[cfg(test)]
    #[allow(clippy::mutable_key_type)]
    pub(crate) fn rename(
        &self,
        uri: &Uri,
        position: Position,
        new_name: &str,
        supports_document_changes: bool,
        supports_file_rename: bool,
    ) -> Result<Option<WorkspaceEdit>, String> {
        self.rename_with_versions(
            uri,
            position,
            new_name,
            supports_document_changes,
            supports_file_rename,
            &[],
        )
    }

    // WorkspaceEdit's legacy `changes` fallback requires HashMap<Uri, _>; URI cache state is not mutated.
    #[allow(clippy::mutable_key_type)]
    pub(crate) fn rename_with_versions(
        &self,
        uri: &Uri,
        position: Position,
        new_name: &str,
        supports_document_changes: bool,
        supports_file_rename: bool,
        document_versions: &[(Uri, i32)],
    ) -> Result<Option<WorkspaceEdit>, String> {
        let declaration = self
            .resolve_at(uri, position)
            .map(|declaration| self.canonical_declaration(declaration))
            .ok_or_else(|| "The rename target does not resolve uniquely.".to_owned())?;
        let target_document = self.rename_target_document(declaration).ok_or_else(|| {
            "Only declarations in project source roots can be renamed.".to_owned()
        })?;

        validate_name(new_name, declaration.kind == DeclarationKind::Script)?;
        self.validate_collision(target_document, declaration, new_name)?;

        let file_rename = if declaration.kind == DeclarationKind::Script {
            if !supports_file_rename {
                return Err(
                    "The editor does not support the file operation required to rename a Papyrus script."
                        .to_owned(),
                );
            }
            self.script_file_rename(target_document, declaration, new_name)?
        } else {
            None
        };

        let mut changes = HashMap::<Uri, Vec<TextEdit>>::new();
        for location in self.references(uri, position, true) {
            if !self.is_project_uri(&location.uri) {
                continue;
            }
            changes.entry(location.uri).or_default().push(TextEdit {
                range: location.range,
                new_text: new_name.to_owned(),
            });
        }
        for edits in changes.values_mut() {
            edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
            edits.dedup_by(|left, right| left.range == right.range);
        }

        if changes.is_empty() {
            return Ok(None);
        }
        self.validate_renamed_references(target_document, declaration, new_name, &changes)?;
        if file_rename.is_none() && !supports_document_changes {
            return Ok(Some(WorkspaceEdit::new(changes)));
        }

        let mut documents = changes.into_iter().collect::<Vec<_>>();
        documents.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
        let mut operations = documents
            .into_iter()
            .map(|(uri, edits)| {
                let version = document_versions
                    .iter()
                    .find_map(|(candidate, version)| (candidate == &uri).then_some(*version));
                DocumentChangeOperation::Edit(TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier { uri, version },
                    edits: edits.into_iter().map(OneOf::Left).collect(),
                })
            })
            .collect::<Vec<_>>();
        if let Some(rename) = file_rename {
            operations.push(DocumentChangeOperation::Op(ResourceOp::Rename(rename)));
        }
        Ok(Some(WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Operations(operations)),
            change_annotations: None,
        }))
    }

    #[allow(clippy::mutable_key_type)]
    fn validate_renamed_references(
        &self,
        target_document: &IndexedDocument,
        target: &Declaration,
        new_name: &str,
        changes: &HashMap<Uri, Vec<TextEdit>>,
    ) -> Result<(), String> {
        let resolver = RenamedResolver {
            workspace: self,
            target_document,
            target,
            new_name,
        };
        let (target_uri, target_range) = self
            .declaration_location(target)
            .ok_or_else(|| "The rename target location is no longer indexed.".to_owned())?;
        let mut found_target = false;
        for (uri, edits) in changes {
            let document = self.documents.get(uri).ok_or_else(|| {
                format!("The rename source `{}` is no longer indexed.", uri.as_str())
            })?;
            for edit in edits {
                if uri == &target_uri && edit.range == target_range {
                    found_target = true;
                    continue;
                }
                let occurrence = document
                    .semantic
                    .occurrences
                    .iter()
                    .find(|occurrence| occurrence.selection_range == edit.range)
                    .ok_or_else(|| {
                        format!(
                            "The edited reference in `{}` is no longer indexed.",
                            uri.as_str()
                        )
                    })?;
                let Some(resolved) = resolver.resolve_occurrence(document, occurrence) else {
                    return Err(format!(
                        "Renaming to `{new_name}` would make a reference in `{}` ambiguous or unresolved.",
                        uri.as_str()
                    ));
                };
                if !std::ptr::eq(self.canonical_declaration(resolved), target) {
                    return Err(format!(
                        "Renaming to `{new_name}` would change what a reference in `{}` resolves to.",
                        uri.as_str()
                    ));
                }
            }
        }
        found_target
            .then_some(())
            .ok_or_else(|| "The rename target was not included in the edit set.".to_owned())
    }

    fn rename_target_document(&self, declaration: &Declaration) -> Option<&IndexedDocument> {
        let (uri, _) = self.declaration_location(declaration)?;
        let document = self.documents.get(&uri)?;
        self.is_project_document(document).then_some(document)
    }

    fn is_project_uri(&self, uri: &Uri) -> bool {
        self.documents
            .get(uri)
            .is_some_and(|document| self.is_project_document(document))
    }

    fn is_project_document(&self, document: &IndexedDocument) -> bool {
        document.path.as_deref().is_some_and(|path| {
            self.config
                .source_roots
                .iter()
                .any(|root| path.starts_with(root))
        })
    }

    fn validate_collision(
        &self,
        target_document: &IndexedDocument,
        target: &Declaration,
        new_name: &str,
    ) -> Result<(), String> {
        if target.name.eq_ignore_ascii_case(new_name) {
            return Ok(());
        }
        if target.kind == DeclarationKind::Script {
            if self
                .scripts_by_name
                .contains_key(&new_name.to_ascii_lowercase())
            {
                return Err(format!("A script named `{new_name}` is already indexed."));
            }
            return Ok(());
        }
        if self
            .resolve_visible_name(target_document, new_name, target.scope.start)
            .is_some()
        {
            return Err(format!(
                "The name `{new_name}` is already visible in the target declaration's scope."
            ));
        }
        let collision = target_document
            .semantic
            .declarations
            .iter()
            .filter(|candidate| !std::ptr::eq(*candidate, target))
            .any(|candidate| {
                candidate.name.eq_ignore_ascii_case(new_name)
                    && option_eq_ignore_ascii_case(
                        candidate.owner_script.as_deref(),
                        target.owner_script.as_deref(),
                    )
                    && option_eq_ignore_ascii_case(
                        candidate.container.as_deref(),
                        target.container.as_deref(),
                    )
            });
        if collision {
            Err(format!(
                "The name `{new_name}` already exists in the target declaration's scope."
            ))
        } else {
            Ok(())
        }
    }

    fn script_file_rename(
        &self,
        target_document: &IndexedDocument,
        target: &Declaration,
        new_name: &str,
    ) -> Result<Option<RenameFile>, String> {
        let (old_namespace, old_leaf) = split_script_name(&target.name);
        let (new_namespace, new_leaf) = split_script_name(new_name);
        if !old_namespace.eq_ignore_ascii_case(new_namespace) {
            return Err("Renaming a script across namespaces is not supported.".to_owned());
        }
        let old_path = target_document.path.as_deref().ok_or_else(|| {
            "The script does not have a file path that can be renamed.".to_owned()
        })?;
        let old_stem = old_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| "The script filename is not valid Unicode.".to_owned())?;
        if !old_stem.eq_ignore_ascii_case(old_leaf) {
            return Err(format!(
                "Script `{}` is stored in `{}`; the filename must match the script name before it can be renamed.",
                target.name,
                old_path.display()
            ));
        }
        if old_leaf == new_leaf {
            return Ok(None);
        }
        let new_path = old_path.with_file_name(format!("{new_leaf}.psc"));
        let case_only = old_leaf.eq_ignore_ascii_case(new_leaf);
        if new_path.exists() && !case_only {
            return Err(format!(
                "Cannot rename the script because `{}` already exists.",
                new_path.display()
            ));
        }
        let old_uri = path_to_file_uri(old_path).ok_or_else(|| {
            "The current script path cannot be represented as a file URI.".to_owned()
        })?;
        let new_uri = path_to_file_uri_lexical(&new_path).ok_or_else(|| {
            "The renamed script path cannot be represented as a file URI.".to_owned()
        })?;
        Ok(Some(RenameFile {
            old_uri,
            new_uri,
            options: None,
            annotation_id: None,
        }))
    }
}

struct RenamedResolver<'a> {
    workspace: &'a WorkspaceIndex,
    target_document: &'a IndexedDocument,
    target: &'a Declaration,
    new_name: &'a str,
}

impl<'a> RenamedResolver<'a> {
    fn resolve_occurrence(
        &self,
        current: &'a IndexedDocument,
        occurrence: &SemanticOccurrence,
    ) -> Option<&'a Declaration> {
        if let Some(receiver) = &occurrence.receiver {
            return self.resolve_qualified_member(
                current,
                receiver,
                self.new_name,
                occurrence.byte_offset,
            );
        }
        self.resolve_visible_name(current, self.new_name, occurrence.byte_offset)
            .or_else(|| self.unique_script(self.new_name).map(|(_, script)| script))
    }

    fn resolve_qualified_member(
        &self,
        current: &'a IndexedDocument,
        receiver: &str,
        member: &str,
        offset: usize,
    ) -> Option<&'a Declaration> {
        if let Some(receiver_declaration) = self.resolve_visible_name(current, receiver, offset) {
            let ty = &receiver_declaration.ty.as_ref()?.name;
            return self.unique_named(self.members_of_type(current, ty), member);
        }
        let (document, script) = self.unique_script(receiver)?;
        self.unique_named(
            document
                .semantic
                .declarations
                .iter()
                .filter(|declaration| {
                    declaration.kind == DeclarationKind::Function
                        && declaration.is_global
                        && declaration.container.is_none()
                        && declaration
                            .owner_script
                            .as_deref()
                            .is_some_and(|owner| owner.eq_ignore_ascii_case(&script.name))
                })
                .collect(),
            member,
        )
    }

    fn resolve_visible_name(
        &self,
        current: &'a IndexedDocument,
        name: &str,
        offset: usize,
    ) -> Option<&'a Declaration> {
        let scoped = current
            .semantic
            .declarations
            .iter()
            .filter(|declaration| self.name_matches(declaration, name))
            .filter(|declaration| {
                declaration.scope.contains(&offset) && declaration.container.is_some()
            })
            .collect::<Vec<_>>();
        if let Some(declaration) = self.unique_named(scoped, name) {
            return Some(declaration);
        }
        let top_level = current
            .semantic
            .declarations
            .iter()
            .filter(|declaration| {
                self.name_matches(declaration, name) && declaration.container.is_none()
            })
            .collect::<Vec<_>>();
        if let Some(declaration) = self.unique_named(top_level, name) {
            return Some(declaration);
        }
        let script = current
            .semantic
            .script_name
            .as_deref()
            .and_then(|script_name| self.unique_script(script_name).map(|(_, script)| script))?;
        self.unique_named(self.members_of(script), name)
            .or_else(|| self.resolve_imported(current, name))
    }

    fn resolve_imported(
        &self,
        current: &'a IndexedDocument,
        name: &str,
    ) -> Option<&'a Declaration> {
        let matches = current
            .semantic
            .imports
            .iter()
            .filter_map(|module| self.unique_script(module).map(|(document, _)| document))
            .flat_map(|document| &document.semantic.declarations)
            .filter(|declaration| self.name_matches(declaration, name))
            .filter(|declaration| {
                declaration.kind == DeclarationKind::Struct
                    || (declaration.kind == DeclarationKind::Function && declaration.is_global)
            })
            .collect::<Vec<_>>();
        self.unique_named(matches, name)
    }

    fn unique_script(&self, name: &str) -> Option<(&'a IndexedDocument, &'a Declaration)> {
        let mut candidates = self
            .workspace
            .documents
            .values()
            .filter_map(|document| {
                document
                    .semantic
                    .declarations
                    .iter()
                    .find(|declaration| {
                        declaration.kind == DeclarationKind::Script
                            && self.name_matches(declaration, name)
                    })
                    .map(|script| (document, script))
            })
            .collect::<Vec<_>>();
        let priority = candidates
            .iter()
            .map(|(document, _)| document.priority)
            .min()?;
        candidates.retain(|(document, _)| document.priority == priority);
        let first = candidates.first()?.0;
        if candidates
            .iter()
            .any(|(document, _)| !self.same_effective_content(first, document))
        {
            return None;
        }
        candidates.sort_by_key(|(document, _)| rename_navigation_key(document));
        Some(candidates.remove(0))
    }

    fn members_of(&self, script: &'a Declaration) -> Vec<&'a Declaration> {
        let mut members = Vec::new();
        let mut visited = HashSet::new();
        let mut current = Some(self.declaration_name(script).to_owned());
        while let Some(name) = current {
            if !visited.insert(name.to_ascii_lowercase()) {
                break;
            }
            let Some((document, resolved_script)) = self.unique_script(&name) else {
                break;
            };
            for declaration in &document.semantic.declarations {
                if declaration
                    .owner_script
                    .as_deref()
                    .is_some_and(|owner| owner.eq_ignore_ascii_case(&resolved_script.name))
                    && declaration.container.is_none()
                    && !matches!(
                        declaration.kind,
                        DeclarationKind::Script | DeclarationKind::Parameter
                    )
                    && !declaration.is_global
                    && !members.iter().any(|existing: &&Declaration| {
                        self.declaration_name(existing)
                            .eq_ignore_ascii_case(self.declaration_name(declaration))
                    })
                {
                    members.push(declaration);
                }
            }
            current = document.semantic.parent_script.clone();
        }
        members
    }

    fn members_of_type(
        &self,
        current: &'a IndexedDocument,
        type_name: &str,
    ) -> Vec<&'a Declaration> {
        if let Some((_, script)) = self.unique_script(type_name) {
            return self.members_of(script);
        }
        let Some(structure) = self
            .resolve_imported(current, type_name)
            .filter(|declaration| declaration.kind == DeclarationKind::Struct)
        else {
            return Vec::new();
        };
        let Some(document) = self
            .workspace
            .declaration_location(structure)
            .and_then(|(uri, _)| self.workspace.documents.get(&uri))
        else {
            return Vec::new();
        };
        document
            .semantic
            .declarations
            .iter()
            .filter(|declaration| {
                declaration
                    .container
                    .as_deref()
                    .is_some_and(|container| container.eq_ignore_ascii_case(&structure.name))
            })
            .collect()
    }

    fn unique_named(
        &self,
        declarations: Vec<&'a Declaration>,
        name: &str,
    ) -> Option<&'a Declaration> {
        let mut matches = declarations
            .into_iter()
            .filter(|declaration| self.name_matches(declaration, name));
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    fn name_matches(&self, declaration: &Declaration, name: &str) -> bool {
        self.declaration_name(declaration)
            .eq_ignore_ascii_case(name)
    }

    fn declaration_name<'b>(&self, declaration: &'b Declaration) -> &'b str
    where
        'a: 'b,
    {
        if std::ptr::eq(declaration, self.target) {
            self.new_name
        } else {
            &declaration.name
        }
    }

    fn same_effective_content(&self, left: &IndexedDocument, right: &IndexedDocument) -> bool {
        if std::ptr::eq(left, right) {
            return true;
        }
        if std::ptr::eq(left, self.target_document) || std::ptr::eq(right, self.target_document) {
            return false;
        }
        left.content_hash == right.content_hash
    }
}

fn rename_navigation_key(document: &IndexedDocument) -> (bool, String) {
    let path = document
        .path
        .as_deref()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| document.semantic.uri.as_str().to_owned());
    (
        path.to_ascii_lowercase().contains("/staging/"),
        path.to_ascii_lowercase(),
    )
}

fn validate_name(name: &str, qualified: bool) -> Result<(), String> {
    if name.is_empty() {
        return Err("The new name cannot be empty.".to_owned());
    }
    let components = name.split(':').collect::<Vec<_>>();
    if !qualified && components.len() != 1 {
        return Err("Only script names may contain namespace separators (`:`).".to_owned());
    }
    if components.iter().any(|component| !is_identifier(component)) {
        return Err(format!("`{name}` is not a valid Papyrus identifier."));
    }
    if let Some(keyword) = components.iter().find(|component| {
        PAPYRUS_KEYWORDS
            .iter()
            .any(|keyword| component.eq_ignore_ascii_case(keyword))
    }) {
        return Err(format!("`{keyword}` is a reserved Papyrus keyword."));
    }
    Ok(())
}

fn is_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn split_script_name(name: &str) -> (&str, &str) {
    name.rsplit_once(':').unwrap_or(("", name))
}

fn option_eq_ignore_ascii_case(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use lsp_types::{DocumentChanges, Position, ResourceOp, WorkspaceEdit};

    use crate::{config::WorkspaceConfig, workspace::path_to_file_uri};

    use super::WorkspaceIndex;

    #[test]
    fn renames_scoped_symbols_from_unsaved_text_without_touching_comments_or_strings() {
        let root = temp_root("rename-local");
        let path = root.join("Project.psc");
        let source = concat!(
            "ScriptName Project\n",
            "Function Test(Int Input)\n",
            "  Int Local = Input\n",
            "  ; Input\n",
            "  String Evidence = \"Input\"\n",
            "EndFunction\n",
        );
        fs::write(&path, source).unwrap();
        let mut index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let uri = path_to_file_uri(&path).unwrap();
        let overlay = source.replace(
            "  Int Local = Input\n",
            "  Int Local = Input\n  Input = Local\n",
        );
        index.overlay(uri.clone(), &overlay);

        let edit = index
            .rename(&uri, Position::new(1, 19), "RenamedInput", false, false)
            .unwrap()
            .unwrap();
        let edits = text_edits(&edit);
        assert_eq!(edits.len(), 3);
        assert!(
            edits
                .iter()
                .all(|(_, _, replacement)| replacement == "RenamedInput")
        );
        assert_eq!(
            edits.iter().map(|(_, line, _)| *line).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renames_inherited_members_imported_globals_and_structs() {
        let root = temp_root("rename-navigation");
        let base = root.join("Base.psc");
        fs::write(&base, "ScriptName Base\nFunction Jump()\nEndFunction\n").unwrap();
        fs::write(root.join("Actor.psc"), "ScriptName Actor Extends Base\n").unwrap();
        let utility = root.join("Utility.psc");
        fs::write(
            &utility,
            concat!(
                "ScriptName Utility\n",
                "Struct Payload\n  Int Value\nEndStruct\n",
                "Function Log() Global\nEndFunction\n",
            ),
        )
        .unwrap();
        let project = root.join("Project.psc");
        fs::write(
            &project,
            concat!(
                "ScriptName Project\n",
                "Import Utility\n",
                "Actor Target\n",
                "Payload Data\n",
                "Function Test()\n",
                "  Target.Jump()\n",
                "  Log()\n",
                "EndFunction\n",
            ),
        )
        .unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();

        let jump = index
            .rename(
                &path_to_file_uri(&base).unwrap(),
                Position::new(1, 10),
                "Leap",
                false,
                false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(text_edits(&jump).len(), 2);

        let log = index
            .rename(
                &path_to_file_uri(&utility).unwrap(),
                Position::new(4, 10),
                "WriteLog",
                false,
                false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(text_edits(&log).len(), 2);

        let payload = index
            .rename(
                &path_to_file_uri(&utility).unwrap(),
                Position::new(1, 8),
                "MessagePayload",
                false,
                false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(text_edits(&payload).len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_colliding_ambiguous_and_external_renames() {
        let root = temp_root("rename-rejections");
        let project_root = root.join("project");
        let first_import = root.join("first");
        let second_import = root.join("second");
        fs::create_dir_all(&project_root).unwrap();
        fs::create_dir_all(&first_import).unwrap();
        fs::create_dir_all(&second_import).unwrap();
        fs::write(
            project_root.join("Project.psc"),
            concat!(
                "ScriptName Project\n",
                "Function Test(Int First, Int Second)\n",
                "  First = Second\n",
                "  Game.FadeOutGame()\n",
                "  Actor Target\n",
                "EndFunction\n",
            ),
        )
        .unwrap();
        let game = first_import.join("Game.psc");
        fs::write(
            &game,
            "ScriptName Game\nFunction FadeOutGame() Global\nEndFunction\n",
        )
        .unwrap();
        fs::write(
            first_import.join("Actor.psc"),
            "ScriptName Actor\nFunction FirstVersion()\nEndFunction\n",
        )
        .unwrap();
        fs::write(
            second_import.join("Actor.psc"),
            "ScriptName Actor\nFunction SecondVersion()\nEndFunction\n",
        )
        .unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![project_root.clone()],
            import_directories: vec![first_import, second_import],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let project_uri = path_to_file_uri(&project_root.join("Project.psc")).unwrap();

        assert!(
            index
                .rename(&project_uri, Position::new(1, 18), "Second", false, false,)
                .unwrap_err()
                .contains("already visible")
        );
        assert!(
            index
                .rename(&project_uri, Position::new(1, 18), "Test", false, false,)
                .unwrap_err()
                .contains("already visible")
        );
        assert!(
            index
                .rename(&project_uri, Position::new(1, 18), "EndIf", false, false,)
                .unwrap_err()
                .contains("reserved")
        );
        assert!(
            index
                .rename(&project_uri, Position::new(1, 18), "9Invalid", false, false,)
                .unwrap_err()
                .contains("not a valid")
        );
        assert!(
            index
                .rename(&project_uri, Position::new(3, 9), "FadeAway", false, false,)
                .unwrap_err()
                .contains("project source roots")
        );
        assert!(
            index
                .prepare_rename(
                    &path_to_file_uri(&game).unwrap(),
                    Position::new(1, 10),
                    true
                )
                .is_none()
        );
        assert!(
            index
                .rename(
                    &project_uri,
                    Position::new(4, 3),
                    "RenamedActor",
                    false,
                    true,
                )
                .unwrap_err()
                .contains("does not resolve uniquely")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_rename_that_would_rebind_an_edited_reference() {
        let root = temp_root("rename-rebind");
        let path = root.join("Project.psc");
        fs::write(
            &path,
            concat!(
                "ScriptName Project\n",
                "Int Property Count Auto\n",
                "Function Test()\n",
                "  Int Value = 1\n",
                "  Count = Value\n",
                "EndFunction\n",
            ),
        )
        .unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let error = index
            .rename(
                &path_to_file_uri(&path).unwrap(),
                Position::new(1, 13),
                "Value",
                true,
                false,
            )
            .unwrap_err();
        assert!(error.contains("would change what a reference"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn versioned_document_changes_use_open_document_versions() {
        let root = temp_root("rename-versions");
        let path = root.join("Project.psc");
        fs::write(
            &path,
            "ScriptName Project\nFunction Test(Int Input)\n  Input = 1\nEndFunction\n",
        )
        .unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let uri = path_to_file_uri(&path).unwrap();
        let edit = index
            .rename_with_versions(
                &uri,
                Position::new(1, 18),
                "Renamed",
                true,
                false,
                &[(uri.clone(), 42)],
            )
            .unwrap()
            .unwrap();
        let DocumentChanges::Operations(operations) = edit.document_changes.unwrap() else {
            panic!("expected document change operations");
        };
        let version = operations
            .into_iter()
            .find_map(|operation| match operation {
                lsp_types::DocumentChangeOperation::Edit(edit) => edit.text_document.version,
                lsp_types::DocumentChangeOperation::Op(_) => None,
            });
        assert_eq!(version, Some(42));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn returns_text_edits_and_a_same_namespace_script_file_rename() {
        let root = temp_root("rename-script");
        let namespace = root.join("Venworks").join("Core");
        fs::create_dir_all(&namespace).unwrap();
        let actor = namespace.join("Actor.psc");
        fs::write(&actor, "ScriptName Venworks:Core:Actor\n").unwrap();
        let project = root.join("Project.psc");
        fs::write(
            &project,
            concat!(
                "ScriptName Project\n",
                "Import Venworks:Core:Actor\n",
                "Venworks:Core:Actor Target\n",
            ),
        )
        .unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let actor_uri = path_to_file_uri(&actor).unwrap();

        assert!(
            index
                .prepare_rename(&actor_uri, Position::new(0, 25), false)
                .is_none()
        );
        assert!(
            index
                .rename(
                    &actor_uri,
                    Position::new(0, 25),
                    "Other:Core:RenamedActor",
                    true,
                    true,
                )
                .unwrap_err()
                .contains("across namespaces")
        );
        let edit = index
            .rename(
                &actor_uri,
                Position::new(0, 25),
                "Venworks:Core:RenamedActor",
                true,
                true,
            )
            .unwrap()
            .unwrap();
        assert_eq!(text_edits(&edit).len(), 3);
        let rename = file_rename(&edit).unwrap();
        assert!(rename.old_uri.as_str().ends_with("/Actor.psc"));
        assert!(rename.new_uri.as_str().ends_with("/RenamedActor.psc"));

        let case_only = index
            .rename(
                &actor_uri,
                Position::new(0, 25),
                "Venworks:Core:actor",
                true,
                true,
            )
            .unwrap()
            .unwrap();
        let rename = file_rename(&case_only).unwrap();
        assert!(rename.old_uri.as_str().ends_with("/Actor.psc"));
        assert!(rename.new_uri.as_str().ends_with("/actor.psc"));

        fs::write(namespace.join("RenamedActor.psc"), "; occupied\n").unwrap();
        assert!(
            index
                .rename(
                    &actor_uri,
                    Position::new(0, 25),
                    "Venworks:Core:RenamedActor",
                    true,
                    true,
                )
                .unwrap_err()
                .contains("already exists")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_script_files_that_do_not_match_the_declared_name() {
        let root = temp_root("rename-script-mismatch");
        let path = root.join("WrongName.psc");
        fs::write(&path, "ScriptName CorrectName\n").unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        assert!(
            index
                .rename(
                    &path_to_file_uri(&path).unwrap(),
                    Position::new(0, 15),
                    "Renamed",
                    true,
                    true,
                )
                .unwrap_err()
                .contains("filename must match")
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn text_edits(edit: &WorkspaceEdit) -> Vec<(String, u32, String)> {
        if let Some(changes) = &edit.changes {
            let mut edits = changes
                .iter()
                .flat_map(|(uri, edits)| {
                    edits.iter().map(|edit| {
                        (
                            uri.as_str().to_owned(),
                            edit.range.start.line,
                            edit.new_text.clone(),
                        )
                    })
                })
                .collect::<Vec<_>>();
            edits.sort();
            return edits;
        }
        let Some(DocumentChanges::Operations(operations)) = &edit.document_changes else {
            return Vec::new();
        };
        let mut edits = operations
            .iter()
            .filter_map(|operation| match operation {
                lsp_types::DocumentChangeOperation::Edit(edit) => Some(edit),
                lsp_types::DocumentChangeOperation::Op(_) => None,
            })
            .flat_map(|document| {
                document.edits.iter().filter_map(|edit| match edit {
                    lsp_types::OneOf::Left(edit) => Some((
                        document.text_document.uri.as_str().to_owned(),
                        edit.range.start.line,
                        edit.new_text.clone(),
                    )),
                    lsp_types::OneOf::Right(_) => None,
                })
            })
            .collect::<Vec<_>>();
        edits.sort();
        edits
    }

    fn file_rename(edit: &WorkspaceEdit) -> Option<&lsp_types::RenameFile> {
        let DocumentChanges::Operations(operations) = edit.document_changes.as_ref()? else {
            return None;
        };
        operations.iter().find_map(|operation| match operation {
            lsp_types::DocumentChangeOperation::Op(ResourceOp::Rename(rename)) => Some(rename),
            _ => None,
        })
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "papyrus-language-server-{label}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
