use std::ops::Range as ByteRange;

use lsp_types::{DocumentSymbol, Range, Uri};
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser};

use crate::line_index::LineIndex;

mod expression;
mod type_checks;

pub(crate) use expression::{
    SemanticBinaryOperator, SemanticExpression, SemanticLiteralKind, SemanticMemberAccess,
    SemanticUnaryOperator,
};
pub(crate) use type_checks::{SemanticAssignmentOperator, SemanticTypeCheck};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct TypeRef {
    pub(crate) name: String,
    pub(crate) array: bool,
}

impl TypeRef {
    pub(crate) fn display(&self) -> String {
        format!("{}{}", self.name, if self.array { "[]" } else { "" })
    }

    pub(crate) fn scalar_name(&self) -> Option<&str> {
        (!self.array).then_some(self.name.as_str())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Parameter {
    pub(crate) name: String,
    pub(crate) ty: TypeRef,
    pub(crate) has_default: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum DeclarationKind {
    Script,
    Property,
    Variable,
    Function,
    Event,
    Struct,
    State,
    Parameter,
    Guard,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Declaration {
    pub(crate) name: String,
    pub(crate) kind: DeclarationKind,
    pub(crate) ty: Option<TypeRef>,
    pub(crate) parameters: Vec<Parameter>,
    pub(crate) owner_script: Option<String>,
    pub(crate) container: Option<String>,
    pub(crate) selection_range: Range,
    pub(crate) scope: ByteRange<usize>,
    pub(crate) documentation: Option<String>,
    pub(crate) is_const: bool,
    pub(crate) is_read_only: bool,
    pub(crate) is_global: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum SemanticOccurrenceKind {
    Reference,
    Type,
    Import,
    Member,
    NamedArgument,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SemanticOccurrence {
    pub(crate) name: String,
    pub(crate) receiver: Option<SemanticExpression>,
    pub(crate) selection_range: Range,
    pub(crate) byte_offset: usize,
    pub(crate) is_named_argument_label: bool,
    pub(crate) kind: SemanticOccurrenceKind,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SemanticCallSite {
    pub(crate) callee_range: Range,
    argument_range: ByteRange<usize>,
    separators: Vec<usize>,
    arguments: Vec<SemanticCallArgument>,
    complete: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SemanticCallArgument {
    pub(crate) name: Option<String>,
    pub(crate) range: Range,
    pub(crate) name_range: Option<Range>,
    byte_range: ByteRange<usize>,
}

impl SemanticCallSite {
    pub(crate) fn contains_offset(&self, offset: usize) -> bool {
        self.argument_range.start <= offset && offset <= self.argument_range.end
    }

    pub(crate) fn argument_at(&self, offset: usize) -> Option<(usize, Option<&str>)> {
        if !self.contains_offset(offset) {
            return None;
        }
        let index = self
            .separators
            .partition_point(|separator| *separator < offset);
        let name = self
            .arguments
            .get(index)
            .filter(|argument| argument.byte_range.start <= offset)
            .and_then(|argument| argument.name.as_deref());
        Some((index, name))
    }

    pub(crate) fn argument_span(&self) -> usize {
        self.argument_range
            .end
            .saturating_sub(self.argument_range.start)
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }

    pub(crate) fn arguments(&self) -> &[SemanticCallArgument] {
        &self.arguments
    }
}

impl Declaration {
    pub(crate) fn signature(&self) -> String {
        match self.kind {
            DeclarationKind::Function | DeclarationKind::Event => {
                let parameters = self
                    .parameters
                    .iter()
                    .map(|parameter| format!("{} {}", parameter.ty.display(), parameter.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                let prefix = self.ty.as_ref().map(TypeRef::display).unwrap_or_default();
                format!(
                    "{}{}{}({parameters})",
                    prefix,
                    if prefix.is_empty() { "" } else { " " },
                    self.name
                )
            }
            _ => self
                .ty
                .as_ref()
                .map(|ty| format!("{} {}", ty.display(), self.name))
                .unwrap_or_else(|| self.name.clone()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SemanticDocument {
    pub(crate) uri: Uri,
    #[serde(skip)]
    pub(crate) text: String,
    pub(crate) script_name: Option<String>,
    pub(crate) parent_script: Option<String>,
    pub(crate) imports: Vec<String>,
    pub(crate) declarations: Vec<Declaration>,
    pub(crate) occurrences: Vec<SemanticOccurrence>,
    pub(crate) call_sites: Vec<SemanticCallSite>,
    pub(crate) member_accesses: Vec<SemanticMemberAccess>,
    pub(crate) type_checks: Vec<SemanticTypeCheck>,
    pub(crate) symbols: Vec<DocumentSymbol>,
}

pub(crate) struct SemanticExtractor {
    parser: Parser,
}

impl SemanticExtractor {
    pub(crate) fn new() -> Result<Self, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_papyrus::LANGUAGE.into())
            .map_err(|error| format!("failed to load the Papyrus grammar: {error}"))?;
        Ok(Self { parser })
    }

    pub(crate) fn extract(&mut self, uri: Uri, source: &str) -> SemanticDocument {
        let mut document = SemanticDocument {
            uri,
            text: source.to_owned(),
            script_name: None,
            parent_script: None,
            imports: Vec::new(),
            declarations: Vec::new(),
            occurrences: Vec::new(),
            call_sites: Vec::new(),
            member_accesses: Vec::new(),
            type_checks: Vec::new(),
            symbols: Vec::new(),
        };
        let Some(tree) = self.parser.parse(source, None) else {
            return document;
        };
        let line_index = LineIndex::new(source);
        let root = tree.root_node();
        document.symbols = crate::symbols::extract_from_tree(root, source, &line_index);
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            if child.kind() == "script_declaration" {
                document.script_name = field_text(child, "name", source);
                document.parent_script = field_text(child, "parent", source);
            } else if child.kind() == "import_declaration"
                && let Some(module) = field_text(child, "module", source)
            {
                document.imports.push(module);
            }
        }
        let script = document.script_name.clone();
        collect(
            root,
            source,
            &line_index,
            script.as_deref(),
            None,
            root.byte_range(),
            &mut document.declarations,
        );
        collect_occurrences(
            root,
            source,
            &line_index,
            &document.declarations,
            &mut document.occurrences,
        );
        collect_call_sites(
            root,
            source,
            &line_index,
            &document.declarations,
            &mut document.call_sites,
        );
        collect_member_accesses(root, source, &line_index, &mut document.member_accesses);
        document.type_checks = type_checks::collect_type_checks(root, source, &line_index);
        document.call_sites.sort_by_key(|call| {
            (
                call.callee_range.start,
                call.argument_range.start,
                call.argument_range.end,
            )
        });
        document.call_sites.dedup_by(|left, right| {
            left.callee_range == right.callee_range && left.argument_range == right.argument_range
        });
        document
    }
}

fn collect_call_sites(
    node: Node<'_>,
    source: &str,
    index: &LineIndex,
    declarations: &[Declaration],
    output: &mut Vec<SemanticCallSite>,
) {
    if node.kind() == "call_expression"
        && let Some(call_site) = call_site(node, source, index)
    {
        output.push(call_site);
    } else if node.kind() == "ERROR" {
        output.extend(recovered_call_sites(node, source, index, declarations));
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_call_sites(child, source, index, declarations, output);
    }
}

fn call_site(node: Node<'_>, source: &str, index: &LineIndex) -> Option<SemanticCallSite> {
    let function = node.child_by_field_name("function")?;
    let callee = if function.kind() == "member_expression" {
        function.child_by_field_name("member")?
    } else {
        function
    };
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let children = arguments.children(&mut cursor).collect::<Vec<_>>();
    call_site_from_children(
        callee.byte_range(),
        &children,
        arguments.end_byte(),
        source,
        index,
    )
}

fn recovered_call_sites(
    node: Node<'_>,
    source: &str,
    index: &LineIndex,
    declarations: &[Declaration],
) -> Vec<SemanticCallSite> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor).collect::<Vec<_>>();
    children
        .iter()
        .enumerate()
        .filter(|(_, child)| child.kind() == "(" && !child.is_missing())
        .filter_map(|(open_index, open)| {
            let callee = identifier_range_before(source, open.start_byte())?;
            let callee_range = index.range(source, callee.clone());
            if declarations
                .iter()
                .any(|declaration| declaration.selection_range == callee_range)
            {
                return None;
            }
            call_site_from_children(
                callee,
                &children[open_index..],
                node.end_byte(),
                source,
                index,
            )
        })
        .collect()
}

fn call_site_from_children(
    callee: ByteRange<usize>,
    children: &[Node<'_>],
    fallback_end: usize,
    source: &str,
    index: &LineIndex,
) -> Option<SemanticCallSite> {
    let open = children
        .iter()
        .find(|child| child.kind() == "(" && !child.is_missing())?;
    let close = children
        .iter()
        .find(|child| child.kind() == ")" && !child.is_missing());
    let argument_start = open.end_byte();
    let argument_end = close
        .map(|child| child.start_byte())
        .unwrap_or(fallback_end)
        .max(argument_start);
    let separators = children
        .iter()
        .filter(|child| {
            child.kind() == ","
                && argument_start <= child.start_byte()
                && child.start_byte() <= argument_end
        })
        .map(|child| child.start_byte())
        .collect();
    let arguments = children
        .iter()
        .filter(|child| {
            child.kind() == "argument"
                && argument_start <= child.start_byte()
                && child.end_byte() <= argument_end
        })
        .map(|argument| {
            let name_node = argument.child_by_field_name("name");
            SemanticCallArgument {
                name: name_node.and_then(|name| text(name, source)),
                range: index.range(source, argument.byte_range()),
                name_range: name_node.map(|name| index.range(source, name.byte_range())),
                byte_range: argument.byte_range(),
            }
        })
        .collect();
    Some(SemanticCallSite {
        callee_range: index.range(source, callee),
        argument_range: argument_start..argument_end,
        separators,
        arguments,
        complete: close.is_some(),
    })
}

fn identifier_range_before(source: &str, offset: usize) -> Option<ByteRange<usize>> {
    let bytes = source.as_bytes();
    let mut end = offset.min(bytes.len());
    while end > 0 && matches!(bytes[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    (start < end).then_some(start..end)
}

fn collect_occurrences(
    node: Node<'_>,
    source: &str,
    index: &LineIndex,
    declarations: &[Declaration],
    output: &mut Vec<SemanticOccurrence>,
) {
    if matches!(node.kind(), "identifier" | "qualified_identifier") {
        let selection_range = index.range(source, node.byte_range());
        if declarations
            .iter()
            .any(|declaration| declaration.selection_range == selection_range)
        {
            return;
        }
        let kind = occurrence_kind(node);
        let is_named_argument_label = kind == SemanticOccurrenceKind::NamedArgument;
        let Some(receiver) = occurrence_receiver(node, source, index) else {
            return;
        };
        if let Some(name) = text(node, source) {
            output.push(SemanticOccurrence {
                name,
                receiver,
                selection_range,
                byte_offset: node.start_byte(),
                is_named_argument_label,
                kind,
            });
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_occurrences(child, source, index, declarations, output);
    }
}

fn occurrence_receiver(
    node: Node<'_>,
    source: &str,
    index: &LineIndex,
) -> Option<Option<SemanticExpression>> {
    let Some(parent) = node.parent() else {
        return Some(None);
    };
    let is_member = parent
        .child_by_field_name("member")
        .is_some_and(|member| member.id() == node.id());
    if !is_member {
        return Some(None);
    }
    let object = parent.child_by_field_name("object")?;
    SemanticExpression::from_node(object, source, index).map(Some)
}

fn collect_member_accesses(
    node: Node<'_>,
    source: &str,
    index: &LineIndex,
    output: &mut Vec<SemanticMemberAccess>,
) {
    if let Some(access) = SemanticMemberAccess::from_node(node, source, index) {
        output.push(access);
    }
    output.extend(SemanticMemberAccess::recover_from_children(
        node, source, index,
    ));
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_member_accesses(child, source, index, output);
    }
}

fn is_named_argument_label(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        parent.kind() == "argument"
            && parent
                .child_by_field_name("name")
                .is_some_and(|name| name.id() == node.id())
    })
}

fn occurrence_kind(node: Node<'_>) -> SemanticOccurrenceKind {
    if is_named_argument_label(node) {
        return SemanticOccurrenceKind::NamedArgument;
    }
    if node.parent().is_some_and(|parent| {
        parent
            .child_by_field_name("member")
            .is_some_and(|member| member.id() == node.id())
    }) {
        return SemanticOccurrenceKind::Member;
    }
    if is_import_occurrence(node) {
        return SemanticOccurrenceKind::Import;
    }
    if is_type_occurrence(node) {
        return SemanticOccurrenceKind::Type;
    }
    SemanticOccurrenceKind::Reference
}

fn is_import_occurrence(node: Node<'_>) -> bool {
    let value = node;
    let Some(parent) = value.parent() else {
        return false;
    };
    parent.kind() == "import_declaration"
        && parent
            .child_by_field_name("module")
            .is_some_and(|module| module.id() == value.id())
}

fn is_type_occurrence(node: Node<'_>) -> bool {
    let mut value = node;
    while let Some(parent) = value.parent() {
        if parent.kind() == "type" {
            return true;
        }
        if matches!(
            parent.kind(),
            "script_declaration" | "import_declaration" | "new_expression"
        ) && ["parent", "module", "type"].iter().any(|field| {
            parent
                .child_by_field_name(field)
                .is_some_and(|field_node| field_node.id() == value.id())
        }) {
            return true;
        }
        if parent.kind() != "qualified_identifier" {
            return false;
        }
        value = parent;
    }
    false
}

fn collect(
    node: Node<'_>,
    source: &str,
    index: &LineIndex,
    script: Option<&str>,
    container: Option<&str>,
    enclosing_scope: ByteRange<usize>,
    output: &mut Vec<Declaration>,
) {
    let scope = if matches!(
        node.kind(),
        "function_definition"
            | "event_definition"
            | "native_function_declaration"
            | "native_event_declaration"
    ) {
        node.byte_range()
    } else {
        enclosing_scope
    };
    let next_container = field_text(node, "name", source);
    if let Some(declaration) = declaration(node, source, index, script, container, scope.clone()) {
        output.push(declaration);
    }
    let nested_container = if matches!(
        node.kind(),
        "function_definition"
            | "event_definition"
            | "native_function_declaration"
            | "native_event_declaration"
            | "state_declaration"
            | "struct_declaration"
            | "property_definition"
    ) {
        next_container.as_deref().or(container)
    } else {
        container
    };
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(
            child,
            source,
            index,
            script,
            nested_container,
            scope.clone(),
            output,
        );
    }
}

fn declaration(
    node: Node<'_>,
    source: &str,
    index: &LineIndex,
    script: Option<&str>,
    container: Option<&str>,
    scope: ByteRange<usize>,
) -> Option<Declaration> {
    let kind = match node.kind() {
        "script_declaration" => DeclarationKind::Script,
        "property_definition" | "auto_property_definition" => DeclarationKind::Property,
        "variable_declaration" | "struct_member" => DeclarationKind::Variable,
        "function_definition" | "native_function_declaration" => DeclarationKind::Function,
        "event_definition" | "native_event_declaration" | "custom_event_declaration" => {
            DeclarationKind::Event
        }
        "struct_declaration" => DeclarationKind::Struct,
        "state_declaration" => DeclarationKind::State,
        "parameter" => DeclarationKind::Parameter,
        "guard_declaration" => DeclarationKind::Guard,
        _ => return None,
    };
    let name_node = node.child_by_field_name("name")?;
    let name = text(name_node, source)?;
    let ty = node
        .child_by_field_name("type")
        .and_then(|child| type_ref(child, source))
        .or_else(|| {
            node.child_by_field_name("return_type")
                .and_then(|child| type_ref(child, source))
        });
    let parameters = node
        .child_by_field_name("parameters")
        .map(|parameters| {
            let mut cursor = parameters.walk();
            parameters
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "parameter")
                .filter_map(|child| {
                    Some(Parameter {
                        name: field_text(child, "name", source)?,
                        ty: child
                            .child_by_field_name("type")
                            .and_then(|ty| type_ref(ty, source))?,
                        has_default: child.child_by_field_name("default").is_some(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let is_const = has_named_child(node, "const");
    let is_read_only = is_const
        || field_text(node, "kind", source)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("AutoReadOnly"));
    Some(Declaration {
        name,
        kind,
        ty,
        parameters,
        owner_script: script.map(str::to_owned),
        container: container.map(str::to_owned),
        selection_range: index.range(source, name_node.byte_range()),
        scope,
        documentation: preceding_documentation(node, source),
        is_const,
        is_read_only,
        is_global: has_named_child(node, "global"),
    })
}

fn has_named_child(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind().eq_ignore_ascii_case(kind))
}

fn type_ref(node: Node<'_>, source: &str) -> Option<TypeRef> {
    let name = node
        .child_by_field_name("name")
        .and_then(|child| text(child, source))
        .or_else(|| text(node, source).map(|value| value.trim_end_matches("[]").to_owned()))?;
    Some(TypeRef {
        name,
        array: node.child_by_field_name("array").is_some(),
    })
}

fn field_text(node: Node<'_>, field: &str, source: &str) -> Option<String> {
    node.child_by_field_name(field)
        .and_then(|child| text(child, source))
}

fn text(node: Node<'_>, source: &str) -> Option<String> {
    node.utf8_text(source.as_bytes()).ok().map(str::to_owned)
}

fn preceding_documentation(node: Node<'_>, source: &str) -> Option<String> {
    let prefix = source.get(..node.start_byte())?.trim_end();
    if !prefix.ends_with('}') {
        return None;
    }
    let start = prefix.rfind('{')?;
    let value = prefix.get(start + 1..prefix.len() - 1)?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use lsp_types::Uri;

    use super::{
        DeclarationKind, SemanticExpression, SemanticExtractor, SemanticOccurrenceKind,
        SemanticTypeCheck,
    };

    #[test]
    fn extracts_parent_types_signatures_scopes_and_documentation() {
        let source = concat!(
            "ScriptName Child Extends Parent\n",
            "{Count docs}\n",
            "Int Property Count Auto\n",
            "Actor Function Resolve(ObjectReference Target, String Label = \"\")\n",
            "  Actor LocalActor\n",
            "EndFunction\n",
        );
        let document = SemanticExtractor::new()
            .unwrap()
            .extract(Uri::from_str("file:///Child.psc").unwrap(), source);
        assert_eq!(document.parent_script.as_deref(), Some("Parent"));
        let property = document
            .declarations
            .iter()
            .find(|item| item.name == "Count")
            .unwrap();
        assert_eq!(property.ty.as_ref().unwrap().display(), "Int");
        assert_eq!(property.documentation.as_deref(), Some("Count docs"));
        let function = document
            .declarations
            .iter()
            .find(|item| item.name == "Resolve")
            .unwrap();
        assert_eq!(function.kind, DeclarationKind::Function);
        assert_eq!(function.parameters[0].ty.name, "ObjectReference");
        assert!(!function.parameters[0].has_default);
        assert!(function.parameters[1].has_default);
        assert!(
            document
                .declarations
                .iter()
                .any(|item| item.name == "LocalActor"
                    && item.container.as_deref() == Some("Resolve"))
        );
    }

    #[test]
    fn extracts_imports_and_semantic_modifiers() {
        let source = concat!(
            "ScriptName Example\n",
            "Import Venworks:Core:Logging\n",
            "Int CONST_Value = 1 Const\n",
            "Function LogSystem() Global\nEndFunction\n",
        );
        let document = SemanticExtractor::new()
            .unwrap()
            .extract(Uri::from_str("file:///Example.psc").unwrap(), source);
        assert_eq!(document.imports, ["Venworks:Core:Logging"]);
        assert!(
            document
                .declarations
                .iter()
                .any(|item| item.name == "CONST_Value" && item.is_const)
        );
        assert!(
            document
                .declarations
                .iter()
                .any(|item| item.name == "LogSystem" && item.is_global)
        );
    }

    #[test]
    fn extracts_semantic_occurrences_without_textual_false_positives() {
        let source = concat!(
            "ScriptName Example\n",
            "Actor Target\n",
            "Function Test()\n",
            "  Target.Jump()\n",
            "  Log(value = Target)\n",
            "  Venworks:Core:Utility.Log()\n",
            "  ; Target.Jump()\n",
            "  String Evidence = \"Target.Jump()\"\n",
            "EndFunction\n",
        );
        let document = SemanticExtractor::new()
            .unwrap()
            .extract(Uri::from_str("file:///Example.psc").unwrap(), source);
        assert!(document.occurrences.iter().any(|occurrence| {
            occurrence.name == "Jump"
                && occurrence.kind == SemanticOccurrenceKind::Member
                && matches!(
                    occurrence.receiver.as_ref(),
                    Some(SemanticExpression::Identifier { name, .. }) if name == "Target"
                )
        }));
        assert!(document.occurrences.iter().any(|occurrence| {
            occurrence.name == "Log"
                && matches!(
                    occurrence.receiver.as_ref(),
                    Some(SemanticExpression::Identifier { name, .. })
                        if name == "Venworks:Core:Utility"
                )
        }));
        assert_eq!(
            document
                .occurrences
                .iter()
                .filter(|occurrence| occurrence.name == "Target")
                .count(),
            2
        );
        assert!(
            document
                .occurrences
                .iter()
                .any(|occurrence| occurrence.name == "value"
                    && occurrence.kind == SemanticOccurrenceKind::NamedArgument
                    && occurrence.is_named_argument_label)
        );
        assert!(document.occurrences.iter().any(|occurrence| {
            occurrence.name == "Actor" && occurrence.kind == SemanticOccurrenceKind::Type
        }));
        assert!(
            !document
                .occurrences
                .iter()
                .any(|occurrence| occurrence.name == "Example")
        );
    }

    #[test]
    fn keeps_grouped_properties_at_script_scope_and_symbols_grouped() {
        let source = concat!(
            "ScriptName Example\n",
            "Group Configuration\n",
            "  Bool Property Enabled Auto\n",
            "EndGroup\n",
        );
        let document = SemanticExtractor::new()
            .unwrap()
            .extract(Uri::from_str("file:///Example.psc").unwrap(), source);
        let property = document
            .declarations
            .iter()
            .find(|declaration| declaration.name == "Enabled")
            .unwrap();
        assert_eq!(property.container, None);
        let group = document
            .symbols
            .iter()
            .find(|symbol| symbol.name == "Configuration")
            .unwrap();
        assert!(
            group
                .children
                .as_ref()
                .unwrap()
                .iter()
                .any(|symbol| symbol.name == "Enabled")
        );
    }

    #[test]
    fn extracts_nested_call_sites_and_tracks_positional_and_named_arguments() {
        let source = concat!(
            "ScriptName Example\n",
            "Function Test()\n",
            "  Outer(First, Inner(name = Second, Third), Last)\n",
            "EndFunction\n",
        );
        let document = SemanticExtractor::new()
            .unwrap()
            .extract(Uri::from_str("file:///Example.psc").unwrap(), source);
        assert_eq!(document.call_sites.len(), 2);

        let outer = document
            .call_sites
            .iter()
            .find(|call| {
                call.callee_range.start.line == 2 && call.callee_range.start.character == 2
            })
            .unwrap();
        let last = source.find("Last").unwrap() + 1;
        assert_eq!(outer.argument_at(last), Some((2, None)));
        assert!(outer.is_complete());

        let inner = document
            .call_sites
            .iter()
            .find(|call| {
                call.callee_range.start.line == 2 && call.callee_range.start.character == 15
            })
            .unwrap();
        let named = source.find("name =").unwrap() + 1;
        assert_eq!(inner.argument_at(named), Some((0, Some("name"))));
        let third = source.find("Third").unwrap() + 1;
        assert_eq!(inner.argument_at(third), Some((1, None)));
        assert!(inner.argument_span() < outer.argument_span());
    }

    #[test]
    fn extracts_an_incomplete_call_after_a_trailing_separator() {
        let source = concat!(
            "ScriptName Example\n",
            "Function Test()\n",
            "  Resolve(First,\n",
            "EndFunction\n",
        );
        let document = SemanticExtractor::new()
            .unwrap()
            .extract(Uri::from_str("file:///Example.psc").unwrap(), source);
        let call = document.call_sites.first().unwrap();
        let offset = source.find("First,").unwrap() + "First,".len();
        assert_eq!(call.argument_at(offset), Some((1, None)));
        assert!(!call.is_complete());
    }

    #[test]
    fn extracts_spanned_type_check_sites() {
        let source = concat!(
            "ScriptName Example\n",
            "Int Function Test(Int Input = 1)\n",
            "  Int Result = Input + 1\n",
            "  Result += 2\n",
            "  If Result > 0\n",
            "    Return Result\n",
            "  EndIf\n",
            "  Return 0\n",
            "EndFunction\n",
        );
        let document = SemanticExtractor::new()
            .unwrap()
            .extract(Uri::from_str("file:///Example.psc").unwrap(), source);
        assert_eq!(
            document
                .type_checks
                .iter()
                .filter(|check| matches!(check, SemanticTypeCheck::Initializer { .. }))
                .count(),
            2
        );
        assert_eq!(
            document
                .type_checks
                .iter()
                .filter(|check| matches!(check, SemanticTypeCheck::Assignment { .. }))
                .count(),
            1
        );
        assert_eq!(
            document
                .type_checks
                .iter()
                .filter(|check| matches!(check, SemanticTypeCheck::Condition { .. }))
                .count(),
            1
        );
        assert_eq!(
            document
                .type_checks
                .iter()
                .filter(|check| matches!(check, SemanticTypeCheck::Return { .. }))
                .count(),
            2
        );
    }
}
