use std::ops::Range as ByteRange;

use lsp_types::Range;
use serde::{Deserialize, Serialize};
use tree_sitter::Node;

use crate::line_index::LineIndex;

use super::{TypeRef, text, type_ref};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum SemanticLiteralKind {
    Bool,
    Int,
    Float,
    String,
    None,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum SemanticUnaryOperator {
    LogicalNot,
    Negate,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum SemanticBinaryOperator {
    LogicalOr,
    LogicalAnd,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum SemanticExpression {
    Identifier {
        name: String,
        byte_offset: usize,
        range: Range,
    },
    Member {
        object: Box<SemanticExpression>,
        member: String,
        byte_offset: usize,
        range: Range,
    },
    Call {
        function: Box<SemanticExpression>,
        arguments: Vec<SemanticExpression>,
        range: Range,
    },
    Subscript {
        array: Box<SemanticExpression>,
        index: Box<SemanticExpression>,
        range: Range,
    },
    Cast {
        value: Box<SemanticExpression>,
        ty: TypeRef,
        range: Range,
    },
    TypeTest {
        value: Box<SemanticExpression>,
        ty: TypeRef,
        range: Range,
    },
    Parenthesized {
        value: Box<SemanticExpression>,
        range: Range,
    },
    New {
        ty: TypeRef,
        size: Option<Box<SemanticExpression>>,
        range: Range,
    },
    Literal {
        kind: SemanticLiteralKind,
        range: Range,
    },
    Unary {
        operator: SemanticUnaryOperator,
        operator_range: Range,
        argument: Box<SemanticExpression>,
        range: Range,
    },
    Binary {
        left: Box<SemanticExpression>,
        operator: SemanticBinaryOperator,
        operator_range: Range,
        right: Box<SemanticExpression>,
        range: Range,
    },
}

impl SemanticExpression {
    pub(super) fn from_node(node: Node<'_>, source: &str, index: &LineIndex) -> Option<Self> {
        let range = index.range(source, node.byte_range());
        match node.kind() {
            "identifier" | "qualified_identifier" => Some(Self::Identifier {
                name: text(node, source)?,
                byte_offset: node.start_byte(),
                range,
            }),
            "member_expression" => {
                let object = node.child_by_field_name("object")?;
                let member = node.child_by_field_name("member")?;
                Some(Self::Member {
                    object: Box::new(Self::from_node(object, source, index)?),
                    member: text(member, source)?,
                    byte_offset: member.start_byte(),
                    range,
                })
            }
            "call_expression" => {
                let arguments = node
                    .child_by_field_name("arguments")
                    .map(|arguments| {
                        let mut cursor = arguments.walk();
                        arguments
                            .named_children(&mut cursor)
                            .filter(|argument| argument.kind() == "argument")
                            .filter_map(|argument| argument.child_by_field_name("value"))
                            .filter_map(|value| Self::from_node(value, source, index))
                            .collect()
                    })
                    .unwrap_or_default();
                Some(Self::Call {
                    function: Box::new(Self::from_node(
                        node.child_by_field_name("function")?,
                        source,
                        index,
                    )?),
                    arguments,
                    range,
                })
            }
            "subscript_expression" => Some(Self::Subscript {
                array: Box::new(Self::from_node(
                    node.child_by_field_name("array")?,
                    source,
                    index,
                )?),
                index: Box::new(Self::from_node(
                    node.child_by_field_name("index")?,
                    source,
                    index,
                )?),
                range,
            }),
            "cast_expression" => Some(Self::Cast {
                value: Box::new(Self::from_node(
                    node.child_by_field_name("value")?,
                    source,
                    index,
                )?),
                ty: type_ref(node.child_by_field_name("type")?, source)?,
                range,
            }),
            "type_test_expression" => Some(Self::TypeTest {
                value: Box::new(Self::from_node(
                    node.child_by_field_name("value")?,
                    source,
                    index,
                )?),
                ty: type_ref(node.child_by_field_name("type")?, source)?,
                range,
            }),
            "parenthesized_expression" => Some(Self::Parenthesized {
                value: Box::new(Self::from_node(node.named_child(0)?, source, index)?),
                range,
            }),
            "new_expression" => {
                let type_node = node.child_by_field_name("type")?;
                let size = node
                    .child_by_field_name("size")
                    .and_then(|size| Self::from_node(size, source, index))
                    .map(Box::new);
                Some(Self::New {
                    ty: TypeRef {
                        name: text(type_node, source)?,
                        array: size.is_some(),
                    },
                    size,
                    range,
                })
            }
            "boolean" => Some(Self::Literal {
                kind: SemanticLiteralKind::Bool,
                range,
            }),
            "integer" => Some(Self::Literal {
                kind: SemanticLiteralKind::Int,
                range,
            }),
            "float" => Some(Self::Literal {
                kind: SemanticLiteralKind::Float,
                range,
            }),
            "string" => Some(Self::Literal {
                kind: SemanticLiteralKind::String,
                range,
            }),
            "none" => Some(Self::Literal {
                kind: SemanticLiteralKind::None,
                range,
            }),
            "unary_expression" => {
                let operator = node.child_by_field_name("operator")?;
                Some(Self::Unary {
                    operator: match text(operator, source)?.as_str() {
                        "!" => SemanticUnaryOperator::LogicalNot,
                        "-" => SemanticUnaryOperator::Negate,
                        _ => return None,
                    },
                    operator_range: index.range(source, operator.byte_range()),
                    argument: Box::new(Self::from_node(
                        node.child_by_field_name("argument")?,
                        source,
                        index,
                    )?),
                    range,
                })
            }
            "binary_expression" => {
                let operator = node.child_by_field_name("operator")?;
                Some(Self::Binary {
                    left: Box::new(Self::from_node(
                        node.child_by_field_name("left")?,
                        source,
                        index,
                    )?),
                    operator: match text(operator, source)?.as_str() {
                        "||" => SemanticBinaryOperator::LogicalOr,
                        "&&" => SemanticBinaryOperator::LogicalAnd,
                        "==" => SemanticBinaryOperator::Equal,
                        "!=" => SemanticBinaryOperator::NotEqual,
                        "<" => SemanticBinaryOperator::Less,
                        "<=" => SemanticBinaryOperator::LessEqual,
                        ">" => SemanticBinaryOperator::Greater,
                        ">=" => SemanticBinaryOperator::GreaterEqual,
                        "+" => SemanticBinaryOperator::Add,
                        "-" => SemanticBinaryOperator::Subtract,
                        "*" => SemanticBinaryOperator::Multiply,
                        "/" => SemanticBinaryOperator::Divide,
                        "%" => SemanticBinaryOperator::Modulo,
                        _ => return None,
                    },
                    operator_range: index.range(source, operator.byte_range()),
                    right: Box::new(Self::from_node(
                        node.child_by_field_name("right")?,
                        source,
                        index,
                    )?),
                    range,
                })
            }
            _ => None,
        }
    }

    pub(crate) fn range(&self) -> Range {
        match self {
            Self::Identifier { range, .. }
            | Self::Member { range, .. }
            | Self::Call { range, .. }
            | Self::Subscript { range, .. }
            | Self::Cast { range, .. }
            | Self::TypeTest { range, .. }
            | Self::Parenthesized { range, .. }
            | Self::New { range, .. }
            | Self::Literal { range, .. }
            | Self::Unary { range, .. }
            | Self::Binary { range, .. } => *range,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SemanticMemberAccess {
    pub(crate) receiver: SemanticExpression,
    completion_range: ByteRange<usize>,
}

impl SemanticMemberAccess {
    pub(super) fn from_node(node: Node<'_>, source: &str, index: &LineIndex) -> Option<Self> {
        if node.kind() != "member_expression" {
            return None;
        }
        let object = node.child_by_field_name("object")?;
        let mut cursor = node.walk();
        let dot = node
            .children(&mut cursor)
            .find(|child| child.kind() == ".")?;
        let member_end = node
            .child_by_field_name("member")
            .map(|member| member.end_byte())
            .unwrap_or_else(|| dot.end_byte());
        Some(Self {
            receiver: SemanticExpression::from_node(object, source, index)?,
            completion_range: dot.end_byte()..member_end.max(dot.end_byte()),
        })
    }

    pub(super) fn recover_from_children(
        node: Node<'_>,
        source: &str,
        index: &LineIndex,
    ) -> Vec<Self> {
        let mut cursor = node.walk();
        let children = node.children(&mut cursor).collect::<Vec<_>>();
        children
            .iter()
            .enumerate()
            .filter(|(_, child)| {
                child.kind() == "ERROR"
                    && source
                        .get(child.byte_range())
                        .is_some_and(|value| value.trim() == ".")
            })
            .filter_map(|(child_index, error)| {
                let receiver = children[..child_index].iter().rev().find_map(|candidate| {
                    expression_from_node_or_child(*candidate, source, index)
                })?;
                Some(Self {
                    receiver,
                    completion_range: error.end_byte()..error.end_byte(),
                })
            })
            .collect()
    }

    pub(crate) fn contains_offset(&self, offset: usize) -> bool {
        self.completion_range.start <= offset && offset <= self.completion_range.end
    }

    pub(crate) fn span(&self) -> usize {
        self.completion_range
            .end
            .saturating_sub(self.completion_range.start)
    }
}

fn expression_from_node_or_child(
    node: Node<'_>,
    source: &str,
    index: &LineIndex,
) -> Option<SemanticExpression> {
    SemanticExpression::from_node(node, source, index).or_else(|| {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .filter_map(|child| SemanticExpression::from_node(child, source, index))
            .last()
    })
}
