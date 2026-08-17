use std::{cmp::Ordering, collections::HashSet};

use lsp_types::{
    Documentation, Hover, HoverContents, Location, MarkupContent, MarkupKind, ParameterInformation,
    ParameterLabel, Position, Range, SignatureHelp, SignatureInformation, Uri,
};

use crate::{
    line_index::LineIndex,
    semantic::{
        Declaration, DeclarationKind, SemanticExpression, SemanticOccurrence,
        SemanticOccurrenceKind,
    },
};

use super::{IndexedDocument, WorkspaceIndex, inference, is_identifier, receiver_before_dot};

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

    pub(crate) fn signature_help(&self, uri: &Uri, position: Position) -> Option<SignatureHelp> {
        let current = self.documents.get(uri)?;
        let text = &current.semantic.text;
        if text.is_empty() {
            return None;
        }
        let offset = LineIndex::new(text).byte_offset(text, position);
        let call = current
            .semantic
            .call_sites
            .iter()
            .filter(|call| call.contains_offset(offset))
            .min_by_key(|call| call.argument_span())?;
        let declaration = self.resolve_at(uri, call.callee_range.start)?;
        if !matches!(
            declaration.kind,
            DeclarationKind::Function | DeclarationKind::Event
        ) {
            return None;
        }
        let (argument_index, argument_name) = call.argument_at(offset)?;
        let active_parameter = if let Some(name) = argument_name {
            declaration
                .parameters
                .iter()
                .position(|parameter| parameter.name.eq_ignore_ascii_case(name))
        } else {
            (argument_index < declaration.parameters.len()).then_some(argument_index)
        }
        .and_then(|index| u32::try_from(index).ok());
        let parameters = declaration
            .parameters
            .iter()
            .map(|parameter| ParameterInformation {
                label: ParameterLabel::Simple(format!(
                    "{} {}",
                    parameter.ty.display(),
                    parameter.name
                )),
                documentation: None,
            })
            .collect();
        Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label: declaration.signature(),
                documentation: declaration.documentation.clone().map(Documentation::String),
                parameters: Some(parameters),
                active_parameter: None,
            }],
            active_signature: Some(0),
            active_parameter,
        })
    }

    pub(super) fn selection_range_at(
        &self,
        current: &IndexedDocument,
        position: Position,
    ) -> Option<Range> {
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

    pub(super) fn declaration_location(&self, declaration: &Declaration) -> Option<(Uri, Range)> {
        let document = self.documents.values().find(|entry| {
            entry
                .semantic
                .declarations
                .iter()
                .any(|candidate| std::ptr::eq(candidate, declaration))
        })?;
        Some((document.semantic.uri.clone(), declaration.selection_range))
    }

    pub(super) fn resolve_at(&self, uri: &Uri, position: Position) -> Option<&Declaration> {
        self.resolve_at_outcome(uri, position).into_option()
    }

    pub(super) fn resolve_at_outcome<'a>(
        &'a self,
        uri: &Uri,
        position: Position,
    ) -> inference::Resolution<&'a Declaration> {
        let Some(current) = self.documents.get(uri) else {
            return inference::Resolution::Unsupported;
        };
        if let Some(declaration) = current
            .semantic
            .declarations
            .iter()
            .find(|declaration| range_contains(declaration.selection_range, position))
        {
            return inference::Resolution::Resolved(declaration);
        }
        if let Some(occurrence) = current
            .semantic
            .occurrences
            .iter()
            .find(|occurrence| range_contains(occurrence.selection_range, position))
        {
            return self.resolve_occurrence_outcome(current, occurrence);
        }
        self.resolve_text_at(current, position).map_or(
            inference::Resolution::Unsupported,
            inference::Resolution::Resolved,
        )
    }

    fn resolve_occurrence<'a>(
        &'a self,
        current: &'a IndexedDocument,
        occurrence: &SemanticOccurrence,
    ) -> Option<&'a Declaration> {
        self.resolve_occurrence_outcome(current, occurrence)
            .into_option()
    }

    pub(super) fn resolve_occurrence_outcome<'a>(
        &'a self,
        current: &'a IndexedDocument,
        occurrence: &SemanticOccurrence,
    ) -> inference::Resolution<&'a Declaration> {
        if occurrence.kind == SemanticOccurrenceKind::NamedArgument {
            return self
                .resolve_named_argument_parameter(current, occurrence)
                .map_or(
                    inference::Resolution::Unsupported,
                    inference::Resolution::Resolved,
                );
        }
        if occurrence.kind == SemanticOccurrenceKind::Import {
            return self
                .unique_script_outcome(&occurrence.name)
                .map(|(_, declaration)| declaration);
        }
        if occurrence.kind == SemanticOccurrenceKind::Type {
            return self.resolve_type_name_outcome(current, &occurrence.name);
        }
        if occurrence.name.eq_ignore_ascii_case("self") {
            return current.semantic.script_name.as_deref().map_or(
                inference::Resolution::Unsupported,
                |name| {
                    self.unique_script_outcome(name)
                        .map(|(_, declaration)| declaration)
                },
            );
        }
        if occurrence.name.eq_ignore_ascii_case("parent") {
            return current.semantic.parent_script.as_deref().map_or(
                inference::Resolution::Unsupported,
                |name| {
                    self.unique_script_outcome(name)
                        .map(|(_, declaration)| declaration)
                },
            );
        }
        if let Some(receiver) = &occurrence.receiver {
            return inference::resolve_member_expression_outcome(
                self,
                current,
                receiver,
                &occurrence.name,
            );
        }
        match self.resolve_visible_name_outcome(current, &occurrence.name, occurrence.byte_offset) {
            inference::Resolution::Missing => self
                .unique_script_outcome(&occurrence.name)
                .map(|(_, declaration)| declaration),
            outcome => outcome,
        }
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
            return inference::resolve_member_expression(
                self,
                current,
                &SemanticExpression::Identifier {
                    name: receiver,
                    byte_offset: offset,
                },
                &name,
            );
        }
        self.resolve_visible_name(current, &name, offset)
            .or_else(|| {
                self.unique_script(&name)
                    .map(|(_, declaration)| declaration)
            })
    }

    pub(super) fn resolve_named_argument_parameter<'a>(
        &'a self,
        current: &'a IndexedDocument,
        occurrence: &SemanticOccurrence,
    ) -> Option<&'a Declaration> {
        let call = current
            .semantic
            .call_sites
            .iter()
            .filter(|call| call.contains_offset(occurrence.byte_offset))
            .filter(|call| {
                call.argument_at(occurrence.byte_offset)
                    .and_then(|(_, name)| name)
                    .is_some_and(|name| name.eq_ignore_ascii_case(&occurrence.name))
            })
            .min_by_key(|call| call.argument_span())?;
        let callee = self.resolve_at(&current.semantic.uri, call.callee_range.start)?;
        if !matches!(
            callee.kind,
            DeclarationKind::Function | DeclarationKind::Event
        ) {
            return None;
        }
        let (uri, _) = self.declaration_location(callee)?;
        self.documents
            .get(&uri)?
            .semantic
            .declarations
            .iter()
            .find(|declaration| {
                declaration.kind == DeclarationKind::Parameter
                    && declaration.name.eq_ignore_ascii_case(&occurrence.name)
                    && declaration
                        .container
                        .as_deref()
                        .is_some_and(|container| container.eq_ignore_ascii_case(&callee.name))
                    && declaration.owner_script == callee.owner_script
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

    pub(super) fn resolve_visible_name_outcome<'a>(
        &'a self,
        current: &'a IndexedDocument,
        name: &str,
        offset: usize,
    ) -> inference::Resolution<&'a Declaration> {
        let scoped = current
            .semantic
            .declarations
            .iter()
            .filter(|declaration| declaration.name.eq_ignore_ascii_case(name))
            .filter(|declaration| {
                declaration.scope.contains(&offset) && declaration.container.is_some()
            })
            .collect::<Vec<_>>();
        match named_outcome(scoped, name) {
            inference::Resolution::Missing => {}
            outcome => return outcome,
        }

        let top_level = current
            .semantic
            .declarations
            .iter()
            .filter(|declaration| {
                declaration.name.eq_ignore_ascii_case(name) && declaration.container.is_none()
            })
            .collect::<Vec<_>>();
        match named_outcome(top_level, name) {
            inference::Resolution::Missing => {}
            outcome => return outcome,
        }

        let Some(script_name) = current.semantic.script_name.as_deref() else {
            return inference::Resolution::Unsupported;
        };
        let script = match self.unique_script_outcome(script_name) {
            inference::Resolution::Resolved((_, script)) => script,
            inference::Resolution::Missing | inference::Resolution::Unsupported => {
                return inference::Resolution::Unsupported;
            }
            inference::Resolution::Ambiguous => return inference::Resolution::Ambiguous,
        };
        match self.members_of_outcome(script) {
            inference::Resolution::Resolved(members) => match named_outcome(members, name) {
                inference::Resolution::Missing => {}
                outcome => return outcome,
            },
            inference::Resolution::Missing | inference::Resolution::Unsupported => {
                return inference::Resolution::Unsupported;
            }
            inference::Resolution::Ambiguous => return inference::Resolution::Ambiguous,
        }

        self.resolve_imported_outcome(current, name, |declaration| {
            declaration.kind == DeclarationKind::Struct
                || (declaration.kind == DeclarationKind::Function && declaration.is_global)
        })
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

    fn resolve_imported_outcome<'a>(
        &'a self,
        current: &'a IndexedDocument,
        name: &str,
        include: impl Fn(&Declaration) -> bool,
    ) -> inference::Resolution<&'a Declaration> {
        let mut matches = Vec::new();
        let mut incomplete = false;
        for module in &current.semantic.imports {
            match self.unique_script_outcome(module) {
                inference::Resolution::Resolved((document, _)) => {
                    matches.extend(
                        document
                            .semantic
                            .declarations
                            .iter()
                            .filter(|declaration| declaration.name.eq_ignore_ascii_case(name))
                            .filter(|declaration| include(declaration)),
                    );
                }
                inference::Resolution::Missing | inference::Resolution::Unsupported => {
                    incomplete = true;
                }
                inference::Resolution::Ambiguous => return inference::Resolution::Ambiguous,
            }
        }
        match named_outcome(matches, name) {
            inference::Resolution::Resolved(_) | inference::Resolution::Ambiguous if incomplete => {
                inference::Resolution::Ambiguous
            }
            inference::Resolution::Missing if incomplete => inference::Resolution::Unsupported,
            outcome => outcome,
        }
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
        self.unique_script_outcome(name).into_option()
    }

    pub(super) fn unique_script_outcome(
        &self,
        name: &str,
    ) -> inference::Resolution<(&IndexedDocument, &Declaration)> {
        let Some(uris) = self.scripts_by_name.get(&name.to_ascii_lowercase()) else {
            return inference::Resolution::Missing;
        };
        let mut candidates = uris
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
        let Some(priority) = candidates.iter().map(|(entry, _)| entry.priority).min() else {
            return inference::Resolution::Missing;
        };
        candidates.retain(|(entry, _)| entry.priority == priority);
        let Some(first_hash) = candidates.first().map(|candidate| candidate.0.content_hash) else {
            return inference::Resolution::Missing;
        };
        if candidates
            .iter()
            .any(|(entry, _)| entry.content_hash != first_hash)
        {
            return inference::Resolution::Ambiguous;
        }
        candidates.sort_by_key(|(entry, _)| navigation_key(entry));
        inference::Resolution::Resolved(candidates.remove(0))
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

    fn members_of_outcome<'a>(
        &'a self,
        script: &'a Declaration,
    ) -> inference::Resolution<Vec<&'a Declaration>> {
        let mut members = Vec::new();
        let mut visited = HashSet::new();
        let mut current = Some(script.name.clone());
        while let Some(name) = current {
            if !visited.insert(name.to_ascii_lowercase()) {
                return inference::Resolution::Unsupported;
            }
            let document = match self.unique_script_outcome(&name) {
                inference::Resolution::Resolved((document, _)) => document,
                inference::Resolution::Ambiguous => return inference::Resolution::Ambiguous,
                inference::Resolution::Missing | inference::Resolution::Unsupported => {
                    return inference::Resolution::Unsupported;
                }
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
        inference::Resolution::Resolved(members)
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

    pub(super) fn members_of_type_outcome<'a>(
        &'a self,
        current: &'a IndexedDocument,
        type_name: &str,
    ) -> inference::Resolution<Vec<&'a Declaration>> {
        if is_primitive_type(type_name) {
            return inference::Resolution::Unsupported;
        }
        match self.unique_script_outcome(type_name) {
            inference::Resolution::Resolved((_, script)) => {
                return self.members_of_outcome(script);
            }
            inference::Resolution::Ambiguous => return inference::Resolution::Ambiguous,
            inference::Resolution::Missing | inference::Resolution::Unsupported => {}
        }

        let structure = match self.resolve_structure_outcome(current, type_name) {
            inference::Resolution::Resolved(structure) => structure,
            inference::Resolution::Ambiguous => return inference::Resolution::Ambiguous,
            inference::Resolution::Missing | inference::Resolution::Unsupported => {
                return inference::Resolution::Unsupported;
            }
        };
        let Some((uri, _)) = self.declaration_location(structure) else {
            return inference::Resolution::Unsupported;
        };
        let Some(document) = self.documents.get(&uri) else {
            return inference::Resolution::Unsupported;
        };
        inference::Resolution::Resolved(
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
                .collect(),
        )
    }

    pub(super) fn resolve_type_name_outcome<'a>(
        &'a self,
        current: &'a IndexedDocument,
        type_name: &str,
    ) -> inference::Resolution<&'a Declaration> {
        if is_primitive_type(type_name) {
            return inference::Resolution::Unsupported;
        }

        let local = current
            .semantic
            .declarations
            .iter()
            .filter(|declaration| {
                declaration.kind == DeclarationKind::Struct
                    && declaration.container.is_none()
                    && declaration.name.eq_ignore_ascii_case(type_name)
            })
            .collect::<Vec<_>>();
        match named_outcome(local, type_name) {
            inference::Resolution::Missing => {}
            outcome => return outcome,
        }

        match self.unique_script_outcome(type_name) {
            inference::Resolution::Resolved((_, script)) => {
                return inference::Resolution::Resolved(script);
            }
            inference::Resolution::Ambiguous => return inference::Resolution::Ambiguous,
            inference::Resolution::Missing | inference::Resolution::Unsupported => {}
        }

        self.resolve_structure_outcome(current, type_name)
    }

    fn resolve_structure_outcome<'a>(
        &'a self,
        current: &'a IndexedDocument,
        type_name: &str,
    ) -> inference::Resolution<&'a Declaration> {
        let local = current
            .semantic
            .declarations
            .iter()
            .filter(|declaration| {
                declaration.kind == DeclarationKind::Struct
                    && declaration.container.is_none()
                    && declaration.name.eq_ignore_ascii_case(type_name)
            })
            .collect::<Vec<_>>();
        match named_outcome(local, type_name) {
            inference::Resolution::Missing => {}
            outcome => return outcome,
        }

        if let Some((script_name, structure_name)) = type_name.rsplit_once(':') {
            match self.unique_script_outcome(script_name) {
                inference::Resolution::Resolved((document, _)) => {
                    let structures = document
                        .semantic
                        .declarations
                        .iter()
                        .filter(|declaration| {
                            declaration.kind == DeclarationKind::Struct
                                && declaration.container.is_none()
                                && declaration.name.eq_ignore_ascii_case(structure_name)
                        })
                        .collect::<Vec<_>>();
                    return named_outcome(structures, structure_name);
                }
                inference::Resolution::Ambiguous => return inference::Resolution::Ambiguous,
                inference::Resolution::Missing | inference::Resolution::Unsupported => {}
            }
        }

        self.resolve_imported_outcome(current, type_name, |declaration| {
            declaration.kind == DeclarationKind::Struct
        })
    }

    pub(super) fn canonical_declaration<'a>(
        &'a self,
        declaration: &'a Declaration,
    ) -> &'a Declaration {
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

fn named_outcome<'a>(
    declarations: Vec<&'a Declaration>,
    name: &str,
) -> inference::Resolution<&'a Declaration> {
    let mut matches = declarations
        .into_iter()
        .filter(|declaration| declaration.name.eq_ignore_ascii_case(name));
    let Some(first) = matches.next() else {
        return inference::Resolution::Missing;
    };
    if matches.next().is_some() {
        inference::Resolution::Ambiguous
    } else {
        inference::Resolution::Resolved(first)
    }
}

pub(super) fn is_primitive_type(name: &str) -> bool {
    ["Bool", "Float", "Int", "String", "Var"]
        .iter()
        .any(|primitive| primitive.eq_ignore_ascii_case(name))
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use lsp_types::{ParameterLabel, Position};

    use crate::{
        config::WorkspaceConfig,
        line_index::LineIndex,
        workspace::{WorkspaceIndex, path_to_file_uri},
    };

    #[test]
    fn provides_signature_help_for_inherited_imported_named_and_nested_calls() {
        let root = temp_root("signature-help");
        fs::write(
            root.join("Base.psc"),
            concat!(
                "ScriptName Base\n",
                "{Jump documentation}\n",
                "Function Jump(Int Count, String Label)\n",
                "EndFunction\n",
            ),
        )
        .unwrap();
        fs::write(root.join("Actor.psc"), "ScriptName Actor Extends Base\n").unwrap();
        fs::write(
            root.join("Utility.psc"),
            concat!(
                "ScriptName Utility\n",
                "{Log documentation}\n",
                "Function Log(String Text, Int Level) Global\n",
                "EndFunction\n",
            ),
        )
        .unwrap();
        let source = concat!(
            "ScriptName Project\n",
            "Import Utility\n",
            "Actor Target\n",
            "Function Test()\n",
            "  Target.Jump(1, \"local\")\n",
            "  Utility.Log(\"qualified\", 2)\n",
            "  Log(Level = 3, Text = \"named\")\n",
            "  Target.Jump(1, Utility.Log(\"nested\", 4))\n",
            "  Missing(1)\n",
            "EndFunction\n",
        );
        let project = root.join("Project.psc");
        fs::write(&project, source).unwrap();
        let mut index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let uri = path_to_file_uri(&project).unwrap();

        let inherited = index
            .signature_help(&uri, position_in(source, "\"local\"", 1))
            .unwrap();
        assert_eq!(
            inherited.signatures[0].label,
            "Jump(Int Count, String Label)"
        );
        assert_eq!(inherited.active_parameter, Some(1));
        assert_eq!(
            inherited.signatures[0].documentation,
            Some(lsp_types::Documentation::String(
                "Jump documentation".to_owned()
            ))
        );

        let qualified = index
            .signature_help(&uri, position_in(source, "qualified\", 2", 12))
            .unwrap();
        assert_eq!(qualified.signatures[0].label, "Log(String Text, Int Level)");
        assert_eq!(qualified.active_parameter, Some(1));

        let named = index
            .signature_help(&uri, position_in(source, "Text = \"named\"", 1))
            .unwrap();
        assert_eq!(named.active_parameter, Some(0));
        assert_eq!(
            named.signatures[0].parameters.as_ref().unwrap()[0].label,
            ParameterLabel::Simple("String Text".to_owned())
        );

        let reordered_named = index
            .signature_help(&uri, position_in(source, "Level = 3", 1))
            .unwrap();
        assert_eq!(reordered_named.active_parameter, Some(1));

        let nested = index
            .signature_help(&uri, position_in(source, "\"nested\"", 1))
            .unwrap();
        assert_eq!(nested.signatures[0].label, "Log(String Text, Int Level)");
        assert_eq!(nested.active_parameter, Some(0));

        assert!(
            index
                .signature_help(&uri, position_in(source, "Missing(", "Missing(".len()))
                .is_none()
        );

        let incomplete = concat!(
            "ScriptName Project\n",
            "Actor Target\n",
            "Function Test()\n",
            "  Target.Jump(1,\n",
            "EndFunction\n",
        );
        index.overlay(uri.clone(), incomplete);
        let incomplete_help = index
            .signature_help(
                &uri,
                position_in(incomplete, "Target.Jump(1,", "Target.Jump(1,".len()),
            )
            .unwrap();
        assert_eq!(
            incomplete_help.signatures[0].label,
            "Jump(Int Count, String Label)"
        );
        assert_eq!(incomplete_help.active_parameter, Some(1));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_namespaced_script_qualified_global_calls() {
        let root = temp_root("namespaced-qualified-call");
        let utility = root.join("Utility.psc");
        fs::write(
            &utility,
            concat!(
                "ScriptName Venworks:Core:Utility\n",
                "Function Log(String Text) Global\n",
                "EndFunction\n",
            ),
        )
        .unwrap();
        let source = concat!(
            "ScriptName Project\n",
            "Function Test()\n",
            "  Venworks:Core:Utility.Log(\"message\")\n",
            "EndFunction\n",
        );
        let project = root.join("Project.psc");
        fs::write(&project, source).unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let project_uri = path_to_file_uri(&project).unwrap();
        let utility_uri = path_to_file_uri(&utility).unwrap();
        let log = position_in(source, "Log(\"message\")", 1);

        assert_eq!(
            index.definition(&project_uri, log).unwrap().uri,
            utility_uri
        );
        assert!(index.hover(&project_uri, log).is_some());
        let signature = index
            .signature_help(&project_uri, position_in(source, "\"message\"", 1))
            .unwrap();
        assert_eq!(signature.signatures[0].label, "Log(String Text)");
        let references = index.references(&utility_uri, Position::new(1, 10), false);
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].uri, project_uri);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn returns_no_signature_help_for_an_ambiguous_receiver_type() {
        let root = temp_root("ambiguous-signature-help");
        let first = root.join("first");
        let second = root.join("second");
        let project_root = root.join("project");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::create_dir_all(&project_root).unwrap();
        fs::write(
            first.join("Actor.psc"),
            "ScriptName Actor\nFunction Jump(Int Count)\nEndFunction\n",
        )
        .unwrap();
        fs::write(
            second.join("Actor.psc"),
            "ScriptName Actor\nFunction Jump(String Label)\nEndFunction\n",
        )
        .unwrap();
        let source = concat!(
            "ScriptName Project\n",
            "Actor Target\n",
            "Function Test()\n",
            "  Target.Jump(1)\n",
            "EndFunction\n",
        );
        let project = project_root.join("Project.psc");
        fs::write(&project, source).unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![project_root],
            import_directories: vec![first, second],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let uri = path_to_file_uri(&project).unwrap();

        assert!(
            index
                .signature_help(&uri, position_in(source, "Jump(", "Jump(".len()))
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn position_in(source: &str, needle: &str, relative_offset: usize) -> Position {
        let offset = source.find(needle).unwrap() + relative_offset;
        LineIndex::new(source).position(source, offset)
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
