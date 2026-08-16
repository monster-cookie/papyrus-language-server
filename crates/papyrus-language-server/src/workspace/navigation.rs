use std::{cmp::Ordering, collections::HashSet};

use lsp_types::{Hover, HoverContents, Location, MarkupContent, MarkupKind, Position, Range, Uri};

use crate::{
    line_index::LineIndex,
    semantic::{Declaration, DeclarationKind, SemanticOccurrence},
};

use super::{IndexedDocument, WorkspaceIndex, is_identifier, receiver_before_dot};

impl WorkspaceIndex {
    pub(crate) fn hover(&self, uri: &Uri, position: Position) -> Option<Hover> {
        let current = self.documents.get(uri)?;
        let hovered_range = self.selection_range_at(current, position);
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
            range: hovered_range,
        })
    }

    pub(crate) fn definition(&self, uri: &Uri, position: Position) -> Option<Location> {
        let declaration = self.resolve_at(uri, position)?;
        let (source_uri, range) = self.declaration_location(declaration)?;
        Some(Location::new(source_uri, range))
    }

    pub(crate) fn references(
        &self,
        uri: &Uri,
        position: Position,
        include_declaration: bool,
    ) -> Vec<Location> {
        let Some(target) = self
            .resolve_at(uri, position)
            .map(|declaration| self.canonical_declaration(declaration))
        else {
            return Vec::new();
        };
        let mut locations = Vec::new();
        if include_declaration && let Some((uri, range)) = self.declaration_location(target) {
            locations.push(Location::new(uri, range));
        }
        if let Some(candidates) = self
            .occurrences_by_name
            .get(&target.name.to_ascii_lowercase())
        {
            for (candidate_uri, occurrence_index) in candidates {
                let Some(document) = self.documents.get(candidate_uri) else {
                    continue;
                };
                if !self.is_navigation_document(document) {
                    continue;
                }
                let Some(occurrence) = document.semantic.occurrences.get(*occurrence_index) else {
                    continue;
                };
                let Some(resolved) = self.resolve_occurrence(document, occurrence) else {
                    continue;
                };
                if std::ptr::eq(self.canonical_declaration(resolved), target) {
                    locations.push(Location::new(
                        candidate_uri.clone(),
                        occurrence.selection_range,
                    ));
                }
            }
        }
        locations.sort_by(compare_locations);
        locations.dedup_by(|left, right| left.uri == right.uri && left.range == right.range);
        locations
    }

    fn selection_range_at(&self, current: &IndexedDocument, position: Position) -> Option<Range> {
        current
            .semantic
            .declarations
            .iter()
            .find(|declaration| range_contains(declaration.selection_range, position))
            .map(|declaration| declaration.selection_range)
            .or_else(|| {
                current
                    .semantic
                    .occurrences
                    .iter()
                    .find(|occurrence| range_contains(occurrence.selection_range, position))
                    .map(|occurrence| occurrence.selection_range)
            })
            .or_else(|| {
                let text = &current.semantic.text;
                if text.is_empty() {
                    return None;
                }
                let index = LineIndex::new(text);
                let offset = index.byte_offset(text, position);
                word_byte_range(text, offset).map(|range| index.range(text, range))
            })
    }

    fn declaration_location(&self, declaration: &Declaration) -> Option<(Uri, Range)> {
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
        if let Some(declaration) = current
            .semantic
            .declarations
            .iter()
            .find(|declaration| range_contains(declaration.selection_range, position))
        {
            return Some(declaration);
        }
        if let Some(occurrence) = current
            .semantic
            .occurrences
            .iter()
            .find(|occurrence| range_contains(occurrence.selection_range, position))
        {
            return self.resolve_occurrence(current, occurrence);
        }
        self.resolve_text_at(current, position)
    }

    fn resolve_occurrence<'a>(
        &'a self,
        current: &'a IndexedDocument,
        occurrence: &SemanticOccurrence,
    ) -> Option<&'a Declaration> {
        if let Some(receiver) = &occurrence.receiver {
            let receiver_declaration =
                self.resolve_visible_name(current, receiver, occurrence.byte_offset)?;
            let ty = &receiver_declaration.ty.as_ref()?.name;
            return unique_named(self.members_of_type(current, ty), &occurrence.name);
        }
        self.resolve_visible_name(current, &occurrence.name, occurrence.byte_offset)
            .or_else(|| {
                self.unique_script(&occurrence.name)
                    .map(|(_, declaration)| declaration)
            })
    }

    fn resolve_text_at<'a>(
        &'a self,
        current: &'a IndexedDocument,
        position: Position,
    ) -> Option<&'a Declaration> {
        let text = &current.semantic.text;
        if text.is_empty() {
            return None;
        }
        let offset = LineIndex::new(text).byte_offset(text, position);
        let name = word_at(text, offset)?;
        if let Some(receiver) = receiver_before_member(text, offset) {
            let receiver_declaration = self.resolve_visible_name(current, &receiver, offset)?;
            let ty = &receiver_declaration.ty.as_ref()?.name;
            return unique_named(self.members_of_type(current, ty), &name);
        }
        self.resolve_visible_name(current, &name, offset)
            .or_else(|| {
                self.unique_script(&name)
                    .map(|(_, declaration)| declaration)
            })
    }

    pub(super) fn resolve_visible_name<'a>(
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
        unique_named(self.members_of(script), name).or_else(|| self.resolve_imported(current, name))
    }

    fn resolve_imported<'a>(
        &'a self,
        current: &'a IndexedDocument,
        name: &str,
    ) -> Option<&'a Declaration> {
        let matches = current
            .semantic
            .imports
            .iter()
            .filter_map(|module| self.unique_script(module).map(|value| value.0))
            .flat_map(|document| &document.semantic.declarations)
            .filter(|declaration| declaration.name.eq_ignore_ascii_case(name))
            .filter(|declaration| {
                declaration.kind == DeclarationKind::Struct
                    || (declaration.kind == DeclarationKind::Function && declaration.is_global)
            })
            .collect::<Vec<_>>();
        unique_named(matches, name)
    }

    pub(super) fn unique_scripts(&self) -> Vec<&Declaration> {
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

    pub(super) fn unique_script(&self, name: &str) -> Option<(&IndexedDocument, &Declaration)> {
        let mut candidates = self
            .scripts_by_name
            .get(&name.to_ascii_lowercase())?
            .iter()
            .filter_map(|uri| self.documents.get(uri))
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
        let first_hash = candidates.first()?.0.content_hash;
        if candidates
            .iter()
            .any(|(entry, _)| entry.content_hash != first_hash)
        {
            return None;
        }
        candidates.sort_by_key(|(entry, _)| navigation_key(entry));
        Some(candidates.remove(0))
    }

    pub(super) fn members_of<'a>(&'a self, script: &'a Declaration) -> Vec<&'a Declaration> {
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
                    && !declaration.is_global
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

    pub(super) fn members_of_type<'a>(
        &'a self,
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
        let Some((_, document)) = self
            .declaration_location(structure)
            .and_then(|(uri, _)| self.documents.get(&uri).map(|document| (uri, document)))
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

    fn canonical_declaration<'a>(&'a self, declaration: &'a Declaration) -> &'a Declaration {
        if declaration.kind == DeclarationKind::Script {
            return self
                .unique_script(&declaration.name)
                .map(|(_, canonical)| canonical)
                .unwrap_or(declaration);
        }
        let Some(owner) = declaration.owner_script.as_deref() else {
            return declaration;
        };
        let Some((document, _)) = self.unique_script(owner) else {
            return declaration;
        };
        document
            .semantic
            .declarations
            .iter()
            .find(|candidate| {
                candidate.kind == declaration.kind
                    && candidate.name.eq_ignore_ascii_case(&declaration.name)
                    && candidate.container == declaration.container
                    && candidate.selection_range == declaration.selection_range
            })
            .unwrap_or(declaration)
    }

    fn is_navigation_document(&self, document: &IndexedDocument) -> bool {
        let Some(script_name) = document.semantic.script_name.as_deref() else {
            return true;
        };
        let Some((canonical, _)) = self.unique_script(script_name) else {
            return true;
        };
        std::ptr::eq(document, canonical)
    }
}

fn range_contains(range: Range, position: Position) -> bool {
    range.start <= position && position < range.end
}

fn compare_locations(left: &Location, right: &Location) -> Ordering {
    left.uri
        .as_str()
        .cmp(right.uri.as_str())
        .then_with(|| left.range.start.cmp(&right.range.start))
        .then_with(|| left.range.end.cmp(&right.range.end))
}

fn navigation_key(document: &IndexedDocument) -> (bool, String) {
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

fn unique_named<'a>(declarations: Vec<&'a Declaration>, name: &str) -> Option<&'a Declaration> {
    let mut matches = declarations
        .into_iter()
        .filter(|declaration| declaration.name.eq_ignore_ascii_case(name));
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn word_at(text: &str, offset: usize) -> Option<String> {
    let range = word_byte_range(text, offset)?;
    Some(text[range].to_owned())
}

fn word_byte_range(text: &str, offset: usize) -> Option<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    let mut start = offset.min(bytes.len());
    let mut end = start;
    while start > 0 && is_identifier(bytes[start - 1]) {
        start -= 1;
    }
    while end < bytes.len() && is_identifier(bytes[end]) {
        end += 1;
    }
    (start < end).then_some(start..end)
}

fn receiver_before_member(text: &str, offset: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let mut start = offset.min(bytes.len());
    while start > 0 && is_identifier(bytes[start - 1]) {
        start -= 1;
    }
    receiver_before_dot(text, start)
}
