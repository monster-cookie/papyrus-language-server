use lsp_types::{DocumentSymbol, Range, SymbolKind};
use tree_sitter::Node;

use crate::line_index::LineIndex;

fn declaration_children(node: Node<'_>, source: &str, index: &LineIndex) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(mut symbol) = declaration(child, source, index) {
            let children = declaration_children(child, source, index);
            if !children.is_empty() {
                symbol.children = Some(children);
            }
            symbols.push(symbol);
        } else {
            symbols.extend(declaration_children(child, source, index));
        }
    }
    symbols
}

pub(crate) fn extract_from_tree(
    root: Node<'_>,
    source: &str,
    index: &LineIndex,
) -> Vec<DocumentSymbol> {
    declaration_children(root, source, index)
}

#[allow(deprecated)]
fn declaration(node: Node<'_>, source: &str, index: &LineIndex) -> Option<DocumentSymbol> {
    let kind = match node.kind() {
        "script_declaration" => SymbolKind::CLASS,
        "state_declaration" => SymbolKind::NAMESPACE,
        "function_definition" | "native_function_declaration" => SymbolKind::FUNCTION,
        "event_definition" | "native_event_declaration" => SymbolKind::EVENT,
        "property_definition" | "auto_property_definition" => SymbolKind::PROPERTY,
        "struct_declaration" => SymbolKind::STRUCT,
        "group_declaration" => SymbolKind::NAMESPACE,
        "parameter" => SymbolKind::VARIABLE,
        "variable_declaration" => SymbolKind::VARIABLE,
        "guard_declaration" => SymbolKind::VARIABLE,
        "custom_event_declaration" => SymbolKind::EVENT,
        _ => return None,
    };
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(source.as_bytes()).ok()?.to_owned();
    Some(DocumentSymbol {
        name,
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range: node_range(node, source, index),
        selection_range: node_range(name_node, source, index),
        children: None,
    })
}

fn node_range(node: Node<'_>, source: &str, index: &LineIndex) -> Range {
    index.range(source, node.start_byte()..node.end_byte())
}

#[cfg(test)]
mod tests {
    use lsp_types::SymbolKind;
    use tree_sitter::Parser;

    use crate::line_index::LineIndex;

    use super::extract_from_tree;

    #[test]
    fn extracts_nested_declarations_and_utf16_ranges() {
        let source = concat!(
            "ScriptName Example\n",
            "Int Property Count Auto\n",
            "Function Run(String Label)\n",
            "  String Local = \"😀\"\n",
            "EndFunction\n",
        );
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_papyrus::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        let symbols = extract_from_tree(tree.root_node(), source, &LineIndex::new(source));
        assert_eq!(symbols[0].name, "Example");
        assert_eq!(symbols[0].kind, SymbolKind::CLASS);
        assert!(symbols.iter().any(|symbol| symbol.name == "Count"));
        let function = symbols.iter().find(|symbol| symbol.name == "Run").unwrap();
        assert!(
            function
                .children
                .as_ref()
                .unwrap()
                .iter()
                .any(|symbol| symbol.name == "Label")
        );
    }
}
