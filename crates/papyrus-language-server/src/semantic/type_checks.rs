use lsp_types::Range;
use serde::{Deserialize, Serialize};
use tree_sitter::Node;

use crate::line_index::LineIndex;

use super::{SemanticExpression, TypeRef, text, type_ref};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum SemanticAssignmentOperator {
    Assign,
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum SemanticTypeCheck {
    Assignment {
        target: SemanticExpression,
        operator: SemanticAssignmentOperator,
        operator_range: Range,
        value: SemanticExpression,
    },
    Initializer {
        expected: TypeRef,
        value: SemanticExpression,
    },
    Return {
        expected: Option<TypeRef>,
        value: Option<SemanticExpression>,
        range: Range,
    },
    Condition {
        value: SemanticExpression,
    },
    Expression {
        value: SemanticExpression,
    },
}

pub(super) fn collect_type_checks(
    root: Node<'_>,
    source: &str,
    index: &LineIndex,
) -> Vec<SemanticTypeCheck> {
    let mut output = Vec::new();
    collect(root, source, index, None, &mut output);
    output
}

fn collect(
    node: Node<'_>,
    source: &str,
    index: &LineIndex,
    expected_return: Option<Option<TypeRef>>,
    output: &mut Vec<SemanticTypeCheck>,
) {
    let expected_return = match node.kind() {
        "function_definition" => Some(
            node.child_by_field_name("return_type")
                .and_then(|return_type| type_ref(return_type, source)),
        ),
        "event_definition" => Some(None),
        _ => expected_return,
    };

    match node.kind() {
        "assignment_statement" => {
            if let (Some(target), Some(operator), Some(value)) = (
                node.child_by_field_name("left")
                    .and_then(|target| SemanticExpression::from_node(target, source, index)),
                node.child_by_field_name("operator"),
                node.child_by_field_name("right")
                    .and_then(|value| SemanticExpression::from_node(value, source, index)),
            ) && let Some(operator_kind) = assignment_operator(operator, source)
            {
                output.push(SemanticTypeCheck::Assignment {
                    target,
                    operator: operator_kind,
                    operator_range: index.range(source, operator.byte_range()),
                    value,
                });
            }
            return;
        }
        "variable_declaration" | "struct_member" | "auto_property_definition" => {
            if let (Some(expected), Some(value)) = (
                node.child_by_field_name("type")
                    .and_then(|expected| type_ref(expected, source)),
                node.child_by_field_name("value")
                    .and_then(|value| SemanticExpression::from_node(value, source, index)),
            ) {
                output.push(SemanticTypeCheck::Initializer { expected, value });
            }
            return;
        }
        "parameter" => {
            if let (Some(expected), Some(value)) = (
                node.child_by_field_name("type")
                    .and_then(|expected| type_ref(expected, source)),
                node.child_by_field_name("default")
                    .and_then(|value| SemanticExpression::from_node(value, source, index)),
            ) {
                output.push(SemanticTypeCheck::Initializer { expected, value });
            }
            return;
        }
        "return_statement" => {
            if let Some(expected) = expected_return {
                output.push(SemanticTypeCheck::Return {
                    expected,
                    value: node
                        .named_child(0)
                        .and_then(|value| SemanticExpression::from_node(value, source, index)),
                    range: index.range(source, node.byte_range()),
                });
            }
            return;
        }
        "if_statement" | "elseif_clause" | "while_statement" => {
            if let Some(value) = node
                .child_by_field_name("condition")
                .and_then(|value| SemanticExpression::from_node(value, source, index))
            {
                output.push(SemanticTypeCheck::Condition { value });
            }
        }
        "expression_statement" => {
            if let Some(value) = node
                .named_child(0)
                .and_then(|value| SemanticExpression::from_node(value, source, index))
            {
                output.push(SemanticTypeCheck::Expression { value });
            }
            return;
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(child, source, index, expected_return.clone(), output);
    }
}

fn assignment_operator(node: Node<'_>, source: &str) -> Option<SemanticAssignmentOperator> {
    match text(node, source)?.as_str() {
        "=" => Some(SemanticAssignmentOperator::Assign),
        "+=" => Some(SemanticAssignmentOperator::Add),
        "-=" => Some(SemanticAssignmentOperator::Subtract),
        "*=" => Some(SemanticAssignmentOperator::Multiply),
        "/=" => Some(SemanticAssignmentOperator::Divide),
        "%=" => Some(SemanticAssignmentOperator::Modulo),
        _ => None,
    }
}
