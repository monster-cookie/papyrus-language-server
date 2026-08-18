use std::collections::HashSet;

use crate::semantic::{SemanticBinaryOperator, SemanticUnaryOperator, TypeRef};

use super::{WorkspaceIndex, inference::Resolution};

const MAX_INHERITANCE_DEPTH: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ValueType {
    Known(TypeRef),
    None,
    Void,
}

impl ValueType {
    pub(super) fn display(&self) -> String {
        match self {
            Self::Known(ty) => ty.display(),
            Self::None => "None".to_owned(),
            Self::Void => "no value".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Compatibility {
    Compatible,
    Incompatible,
    Ambiguous,
    Unresolved,
}

pub(super) fn known(name: &str) -> ValueType {
    ValueType::Known(TypeRef {
        name: name.to_owned(),
        array: false,
    })
}

pub(super) fn is_numeric(value: &ValueType) -> bool {
    matches!(
        value,
        ValueType::Known(ty)
            if !ty.array
                && ["Int", "Float", "Var"]
                    .iter()
                    .any(|name| ty.name.eq_ignore_ascii_case(name))
    )
}

pub(super) fn is_int(value: &ValueType) -> bool {
    matches!(
        value,
        ValueType::Known(ty)
            if !ty.array
                && ["Int", "Var"]
                    .iter()
                    .any(|name| ty.name.eq_ignore_ascii_case(name))
    )
}

pub(super) fn is_boolean_convertible(value: &ValueType) -> bool {
    !matches!(value, ValueType::Void)
}

pub(super) fn unary_result(
    operator: SemanticUnaryOperator,
    argument: &ValueType,
) -> Option<ValueType> {
    match operator {
        SemanticUnaryOperator::LogicalNot if is_boolean_convertible(argument) => {
            Some(known("Bool"))
        }
        SemanticUnaryOperator::Negate if is_numeric(argument) => Some(argument.clone()),
        SemanticUnaryOperator::LogicalNot | SemanticUnaryOperator::Negate => None,
    }
}

pub(super) fn binary_result(
    operator: SemanticBinaryOperator,
    left: &ValueType,
    right: &ValueType,
) -> Option<ValueType> {
    match operator {
        SemanticBinaryOperator::LogicalOr | SemanticBinaryOperator::LogicalAnd
            if is_boolean_convertible(left) && is_boolean_convertible(right) =>
        {
            Some(known("Bool"))
        }
        SemanticBinaryOperator::Equal | SemanticBinaryOperator::NotEqual
            if !matches!(left, ValueType::Void) && !matches!(right, ValueType::Void) =>
        {
            Some(known("Bool"))
        }
        SemanticBinaryOperator::Less
        | SemanticBinaryOperator::LessEqual
        | SemanticBinaryOperator::Greater
        | SemanticBinaryOperator::GreaterEqual
            if is_numeric(left) && is_numeric(right) =>
        {
            Some(known("Bool"))
        }
        SemanticBinaryOperator::Add if is_string(left) || is_string(right) => Some(known("String")),
        SemanticBinaryOperator::Add
        | SemanticBinaryOperator::Subtract
        | SemanticBinaryOperator::Multiply
        | SemanticBinaryOperator::Divide
            if is_numeric(left) && is_numeric(right) =>
        {
            Some(numeric_result(left, right))
        }
        SemanticBinaryOperator::Modulo if is_int(left) && is_int(right) => {
            if is_var(left) || is_var(right) {
                Some(known("Var"))
            } else {
                Some(known("Int"))
            }
        }
        _ => None,
    }
}

impl WorkspaceIndex {
    pub(super) fn type_compatibility(&self, source: &ValueType, target: &TypeRef) -> Compatibility {
        match source {
            ValueType::Void => Compatibility::Incompatible,
            ValueType::None => {
                if target.array
                    || is_var_name(&target.name)
                    || target.name.eq_ignore_ascii_case("Bool")
                    || target.name.eq_ignore_ascii_case("String")
                    || !is_primitive_name(&target.name)
                {
                    Compatibility::Compatible
                } else {
                    Compatibility::Incompatible
                }
            }
            ValueType::Known(source) => self.known_type_compatibility(source, target),
        }
    }

    pub(super) fn equality_compatibility(
        &self,
        left: &ValueType,
        right: &ValueType,
    ) -> Compatibility {
        if is_numeric(left) && is_numeric(right) {
            return Compatibility::Compatible;
        }
        match (left, right) {
            (ValueType::Void, _) | (_, ValueType::Void) => Compatibility::Incompatible,
            (ValueType::None, ValueType::None) => Compatibility::Compatible,
            (ValueType::Known(left), right) => {
                let forward = self.type_compatibility(right, left);
                if forward == Compatibility::Compatible {
                    return forward;
                }
                let ValueType::Known(right) = right else {
                    return forward;
                };
                combine_compatibility(
                    forward,
                    self.type_compatibility(&ValueType::Known(left.clone()), right),
                )
            }
            (ValueType::None, ValueType::Known(right)) => {
                self.type_compatibility(&ValueType::None, right)
            }
        }
    }

    pub(super) fn cast_compatibility(&self, source: &ValueType, target: &TypeRef) -> Compatibility {
        match source {
            ValueType::Void => Compatibility::Incompatible,
            ValueType::None => self.type_compatibility(source, target),
            ValueType::Known(source) if source.array || target.array => {
                if source.array == target.array
                    && (source.name.eq_ignore_ascii_case(&target.name)
                        || is_var_name(&source.name)
                        || is_var_name(&target.name))
                {
                    Compatibility::Compatible
                } else {
                    Compatibility::Incompatible
                }
            }
            ValueType::Known(source)
                if is_var_name(&source.name)
                    || is_var_name(&target.name)
                    || is_primitive_name(&target.name) =>
            {
                Compatibility::Compatible
            }
            ValueType::Known(source) if !is_primitive_name(&source.name) => {
                Compatibility::Compatible
            }
            ValueType::Known(_) => Compatibility::Incompatible,
        }
    }

    fn known_type_compatibility(&self, source: &TypeRef, target: &TypeRef) -> Compatibility {
        if source.array || target.array {
            return if (source.array == target.array
                && source.name.eq_ignore_ascii_case(&target.name))
                || is_var_name(&source.name)
                || is_var_name(&target.name)
            {
                Compatibility::Compatible
            } else {
                Compatibility::Incompatible
            };
        }
        if source.name.eq_ignore_ascii_case(&target.name)
            || is_var_name(&source.name)
            || is_var_name(&target.name)
            || target.name.eq_ignore_ascii_case("String")
            || target.name.eq_ignore_ascii_case("Bool")
            || (source.name.eq_ignore_ascii_case("Int")
                && target.name.eq_ignore_ascii_case("Float"))
        {
            return Compatibility::Compatible;
        }
        if is_primitive_name(&source.name) || is_primitive_name(&target.name) {
            return Compatibility::Incompatible;
        }
        self.inheritance_compatibility(&source.name, &target.name)
    }

    fn inheritance_compatibility(&self, source: &str, target: &str) -> Compatibility {
        let mut current = source.to_owned();
        let mut visited = HashSet::new();
        for _ in 0..MAX_INHERITANCE_DEPTH {
            if !visited.insert(current.to_ascii_lowercase()) {
                return Compatibility::Unresolved;
            }
            let document = match self.unique_script_outcome(&current) {
                Resolution::Resolved((document, _)) => document,
                Resolution::Missing | Resolution::Unsupported => return Compatibility::Unresolved,
                Resolution::Ambiguous => return Compatibility::Ambiguous,
            };
            let Some(parent) = document.semantic.parent_script.as_deref() else {
                return Compatibility::Incompatible;
            };
            if parent.eq_ignore_ascii_case(target) {
                return Compatibility::Compatible;
            }
            current = parent.to_owned();
        }
        Compatibility::Unresolved
    }
}

fn combine_compatibility(left: Compatibility, right: Compatibility) -> Compatibility {
    if left == Compatibility::Compatible || right == Compatibility::Compatible {
        Compatibility::Compatible
    } else if left == Compatibility::Ambiguous || right == Compatibility::Ambiguous {
        Compatibility::Ambiguous
    } else if left == Compatibility::Unresolved || right == Compatibility::Unresolved {
        Compatibility::Unresolved
    } else {
        Compatibility::Incompatible
    }
}

fn numeric_result(left: &ValueType, right: &ValueType) -> ValueType {
    if is_var(left) || is_var(right) {
        known("Var")
    } else if is_float(left) || is_float(right) {
        known("Float")
    } else {
        known("Int")
    }
}

fn is_float(value: &ValueType) -> bool {
    matches!(
        value,
        ValueType::Known(ty) if !ty.array && ty.name.eq_ignore_ascii_case("Float")
    )
}

fn is_string(value: &ValueType) -> bool {
    matches!(
        value,
        ValueType::Known(ty) if !ty.array && ty.name.eq_ignore_ascii_case("String")
    )
}

fn is_var(value: &ValueType) -> bool {
    matches!(value, ValueType::Known(ty) if !ty.array && is_var_name(&ty.name))
}

fn is_var_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("Var")
}

fn is_primitive_name(name: &str) -> bool {
    ["Bool", "Float", "Int", "String", "Var"]
        .iter()
        .any(|primitive| primitive.eq_ignore_ascii_case(name))
}
