use std::ops::Range as ByteRange;

use serde::{Deserialize, Serialize};
use tree_sitter::Node;

use super::{TypeRef, text, type_ref};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum SemanticExpression {
    Identifier {
        name: String,
        byte_offset: usize,
    },
    Member {
        object: Box<SemanticExpression>,
        member: String,
        byte_offset: usize,
    },
    Call {
        function: Box<SemanticExpression>,
    },
    Subscript {
        array: Box<SemanticExpression>,
    },
    Cast {
        ty: TypeRef,
    },
    Parenthesized {
        value: Box<SemanticExpression>,
    },
    New {
        ty: TypeRef,
    },
}

impl SemanticExpression {
    pub(super) fn from_node(node: Node<'_>, source: &str) -> Option<Self> {
        match node.kind() {
            "identifier" | "qualified_identifier" => Some(Self::Identifier {
                name: text(node, source)?,
                byte_offset: node.start_byte(),
            }),
            "member_expression" => {
                let object = node.child_by_field_name("object")?;
                let member = node.child_by_field_name("member")?;
                Some(Self::Member {
                    object: Box::new(Self::from_node(object, source)?),
                    member: text(member, source)?,
                    byte_offset: member.start_byte(),
                })
            }
            "call_expression" => Some(Self::Call {
                function: Box::new(Self::from_node(
                    node.child_by_field_name("function")?,
                    source,
                )?),
            }),
            "subscript_expression" => Some(Self::Subscript {
                array: Box::new(Self::from_node(node.child_by_field_name("array")?, source)?),
            }),
            "cast_expression" => Some(Self::Cast {
                ty: type_ref(node.child_by_field_name("type")?, source)?,
            }),
            "parenthesized_expression" => Some(Self::Parenthesized {
                value: Box::new(Self::from_node(node.named_child(0)?, source)?),
            }),
            "new_expression" => {
                let type_node = node.child_by_field_name("type")?;
                Some(Self::New {
                    ty: TypeRef {
                        name: text(type_node, source)?,
                        array: node.child_by_field_name("size").is_some(),
                    },
                })
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SemanticMemberAccess {
    pub(crate) receiver: SemanticExpression,
    completion_range: ByteRange<usize>,
}

impl SemanticMemberAccess {
    pub(super) fn from_node(node: Node<'_>, source: &str) -> Option<Self> {
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
            receiver: SemanticExpression::from_node(object, source)?,
            completion_range: dot.end_byte()..member_end.max(dot.end_byte()),
        })
    }

    pub(super) fn recover_from_children(node: Node<'_>, source: &str) -> Vec<Self> {
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
            .filter_map(|(index, error)| {
                let receiver = children[..index]
                    .iter()
                    .rev()
                    .find_map(|candidate| expression_from_node_or_child(*candidate, source))?;
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

fn expression_from_node_or_child(node: Node<'_>, source: &str) -> Option<SemanticExpression> {
    SemanticExpression::from_node(node, source).or_else(|| {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .filter_map(|child| SemanticExpression::from_node(child, source))
            .last()
    })
}
