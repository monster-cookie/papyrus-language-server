use std::ops::Range as ByteRange;

use lsp_types::{Range, Uri};
use tree_sitter::{Node, Parser};

use crate::line_index::LineIndex;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TypeRef {
    pub(crate) name: String,
    pub(crate) array: bool,
}

impl TypeRef {
    pub(crate) fn display(&self) -> String {
        format!("{}{}", self.name, if self.array { "[]" } else { "" })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Parameter {
    pub(crate) name: String,
    pub(crate) ty: TypeRef,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
pub(crate) struct SemanticDocument {
    pub(crate) uri: Uri,
    pub(crate) text: String,
    pub(crate) script_name: Option<String>,
    pub(crate) parent_script: Option<String>,
    pub(crate) declarations: Vec<Declaration>,
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
            declarations: Vec::new(),
        };
        let Some(tree) = self.parser.parse(source, None) else {
            return document;
        };
        let line_index = LineIndex::new(source);
        let root = tree.root_node();
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            if child.kind() == "script_declaration" {
                document.script_name = field_text(child, "name", source);
                document.parent_script = field_text(child, "parent", source);
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
        document
    }
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
            | "group_declaration"
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
                    })
                })
                .collect()
        })
        .unwrap_or_default();
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
    })
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

    use super::{DeclarationKind, SemanticExtractor};

    #[test]
    fn extracts_parent_types_signatures_scopes_and_documentation() {
        let source = concat!(
            "ScriptName Child Extends Parent\n",
            "{Count docs}\n",
            "Int Property Count Auto\n",
            "Actor Function Resolve(ObjectReference Target)\n",
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
        assert!(
            document
                .declarations
                .iter()
                .any(|item| item.name == "LocalActor"
                    && item.container.as_deref() == Some("Resolve"))
        );
    }
}
