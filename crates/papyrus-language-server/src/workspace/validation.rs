use std::collections::HashSet;

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Range, Uri};

use crate::semantic::{
    Declaration, DeclarationKind, SemanticAssignmentOperator, SemanticBinaryOperator,
    SemanticCallSite, SemanticExpression, SemanticOccurrenceKind, SemanticTypeCheck, TypeRef,
};

use super::{
    IndexedDocument, WorkspaceIndex, inference,
    navigation::is_primitive_type,
    type_system::{self, Compatibility, ValueType},
};

const DIAGNOSTIC_SOURCE: &str = "papyrus-language-server";

struct SemanticIssue {
    range: Range,
    code: &'static str,
    message: String,
}

impl WorkspaceIndex {
    pub(crate) fn semantic_diagnostics(&self, uri: &Uri) -> Vec<Diagnostic> {
        let Some(current) = self.documents.get(uri) else {
            return Vec::new();
        };
        let mut issues = Vec::new();

        for occurrence in &current.semantic.occurrences {
            match occurrence.kind {
                SemanticOccurrenceKind::NamedArgument => {}
                SemanticOccurrenceKind::Type | SemanticOccurrenceKind::Import => {
                    if is_primitive_type(&occurrence.name) {
                        continue;
                    }
                    match self.resolve_occurrence_outcome(current, occurrence) {
                        inference::Resolution::Missing => issues.push(SemanticIssue {
                            range: occurrence.selection_range,
                            code: "unresolved-type",
                            message: format!("Unresolved type '{}'", occurrence.name),
                        }),
                        inference::Resolution::Ambiguous => issues.push(SemanticIssue {
                            range: occurrence.selection_range,
                            code: "ambiguous-type",
                            message: format!("Ambiguous type '{}'", occurrence.name),
                        }),
                        inference::Resolution::Resolved(_) | inference::Resolution::Unsupported => {
                        }
                    }
                }
                SemanticOccurrenceKind::Member | SemanticOccurrenceKind::Reference => {
                    match self.resolve_occurrence_outcome(current, occurrence) {
                        inference::Resolution::Missing => {
                            let (code, label) = if occurrence.kind == SemanticOccurrenceKind::Member
                            {
                                ("unresolved-member", "member")
                            } else {
                                ("unresolved-reference", "reference")
                            };
                            issues.push(SemanticIssue {
                                range: occurrence.selection_range,
                                code,
                                message: format!("Unresolved {label} '{}'", occurrence.name),
                            });
                        }
                        inference::Resolution::Ambiguous => {
                            let (code, label) = if occurrence.kind == SemanticOccurrenceKind::Member
                            {
                                ("ambiguous-member", "member")
                            } else {
                                ("ambiguous-reference", "reference")
                            };
                            issues.push(SemanticIssue {
                                range: occurrence.selection_range,
                                code,
                                message: format!("Ambiguous {label} '{}'", occurrence.name),
                            });
                        }
                        inference::Resolution::Resolved(_) | inference::Resolution::Unsupported => {
                        }
                    }
                }
            }
        }

        for call in &current.semantic.call_sites {
            validate_call(self, uri, call, &mut issues);
        }
        for check in &current.semantic.type_checks {
            validate_type_check(self, current, check, &mut issues);
        }

        issues.sort_by(|left, right| {
            left.range
                .start
                .cmp(&right.range.start)
                .then_with(|| left.range.end.cmp(&right.range.end))
                .then_with(|| left.code.cmp(right.code))
                .then_with(|| left.message.cmp(&right.message))
        });
        issues.dedup_by(|left, right| {
            left.range == right.range && left.code == right.code && left.message == right.message
        });
        issues.into_iter().map(issue_diagnostic).collect()
    }
}

fn validate_type_check(
    workspace: &WorkspaceIndex,
    current: &IndexedDocument,
    check: &SemanticTypeCheck,
    issues: &mut Vec<SemanticIssue>,
) {
    match check {
        SemanticTypeCheck::Assignment {
            target,
            operator,
            operator_range,
            value,
        } => validate_assignment(
            workspace,
            current,
            target,
            *operator,
            *operator_range,
            value,
            issues,
        ),
        SemanticTypeCheck::Initializer { expected, value } => {
            let value_type = validate_expression(workspace, current, value, issues);
            validate_compatible_value(
                workspace,
                value_type,
                expected,
                value.range(),
                "incompatible-assignment",
                "Cannot initialize",
                issues,
            );
        }
        SemanticTypeCheck::Return {
            expected,
            value,
            range,
        } => validate_return(
            workspace,
            current,
            expected.as_ref(),
            value.as_ref(),
            *range,
            issues,
        ),
        SemanticTypeCheck::Condition { value } => {
            match validate_expression(workspace, current, value, issues) {
                inference::Resolution::Resolved(value_type)
                    if !type_system::is_boolean_convertible(&value_type) =>
                {
                    issues.push(SemanticIssue {
                        range: value.range(),
                        code: "invalid-condition",
                        message: format!(
                            "Condition requires a value, but expression produces {}",
                            value_type.display()
                        ),
                    });
                }
                inference::Resolution::Resolved(_)
                | inference::Resolution::Missing
                | inference::Resolution::Ambiguous
                | inference::Resolution::Unsupported => {}
            }
        }
        SemanticTypeCheck::Expression { value } => {
            let _ = validate_expression(workspace, current, value, issues);
        }
    }
}

fn validate_assignment(
    workspace: &WorkspaceIndex,
    current: &IndexedDocument,
    target: &SemanticExpression,
    operator: SemanticAssignmentOperator,
    operator_range: Range,
    value: &SemanticExpression,
    issues: &mut Vec<SemanticIssue>,
) {
    let target_type = validate_expression(workspace, current, target, issues);
    let value_type = validate_expression(workspace, current, value, issues);

    if matches!(target, SemanticExpression::Subscript { .. }) {
        if operator != SemanticAssignmentOperator::Assign {
            issues.push(SemanticIssue {
                range: operator_range,
                code: "invalid-compound-assignment",
                message: "Array elements support only simple '=' assignment".to_owned(),
            });
            return;
        }
    } else {
        match inference::resolve_expression_declaration_outcome(workspace, current, target) {
            inference::Resolution::Resolved(declaration)
                if declaration.is_read_only
                    || !matches!(
                        declaration.kind,
                        DeclarationKind::Variable
                            | DeclarationKind::Property
                            | DeclarationKind::Parameter
                    ) =>
            {
                issues.push(SemanticIssue {
                    range: target.range(),
                    code: "invalid-assignment-target",
                    message: format!("'{}' is not writable", declaration.name),
                });
                return;
            }
            inference::Resolution::Unsupported => {
                issues.push(SemanticIssue {
                    range: target.range(),
                    code: "invalid-assignment-target",
                    message: "Expression is not a writable assignment target".to_owned(),
                });
                return;
            }
            inference::Resolution::Resolved(_)
            | inference::Resolution::Missing
            | inference::Resolution::Ambiguous => {}
        }
    }

    let target_type = match target_type {
        inference::Resolution::Resolved(ValueType::Known(target_type)) => target_type,
        inference::Resolution::Resolved(ValueType::None | ValueType::Void)
        | inference::Resolution::Missing
        | inference::Resolution::Ambiguous
        | inference::Resolution::Unsupported => {
            if matches!(
                &value_type,
                inference::Resolution::Resolved(ValueType::Void)
            ) {
                push_void_value(value.range(), issues);
            }
            return;
        }
    };

    if operator == SemanticAssignmentOperator::Assign {
        validate_compatible_value(
            workspace,
            value_type,
            &target_type,
            value.range(),
            "incompatible-assignment",
            "Cannot assign",
            issues,
        );
        return;
    }

    let inference::Resolution::Resolved(value_type) = value_type else {
        return;
    };
    if value_type == ValueType::Void {
        push_void_value(value.range(), issues);
        return;
    }
    let binary_operator = assignment_binary_operator(operator);
    let Some(result) = type_system::binary_result(
        binary_operator,
        &ValueType::Known(target_type.clone()),
        &value_type,
    ) else {
        issues.push(SemanticIssue {
            range: operator_range,
            code: "invalid-compound-assignment",
            message: format!(
                "Operator '{}' cannot combine {} and {}",
                assignment_operator_text(operator),
                target_type.display(),
                value_type.display()
            ),
        });
        return;
    };
    if workspace.type_compatibility(&result, &target_type) == Compatibility::Incompatible {
        issues.push(SemanticIssue {
            range: value.range(),
            code: "incompatible-assignment",
            message: format!(
                "Compound assignment produces {}, which cannot be assigned to {}",
                result.display(),
                target_type.display()
            ),
        });
    }
}

fn validate_return(
    workspace: &WorkspaceIndex,
    current: &IndexedDocument,
    expected: Option<&TypeRef>,
    value: Option<&SemanticExpression>,
    range: Range,
    issues: &mut Vec<SemanticIssue>,
) {
    match (expected, value) {
        (Some(expected), Some(value)) => {
            let value_type = validate_expression(workspace, current, value, issues);
            validate_compatible_value(
                workspace,
                value_type,
                expected,
                value.range(),
                "incompatible-return",
                "Cannot return",
                issues,
            );
        }
        (Some(expected), None) => issues.push(SemanticIssue {
            range,
            code: "missing-return-value",
            message: format!("Return statement requires a {} value", expected.display()),
        }),
        (None, Some(value)) => {
            let _ = validate_expression(workspace, current, value, issues);
            issues.push(SemanticIssue {
                range: value.range(),
                code: "unexpected-return-value",
                message: "This function or event does not return a value".to_owned(),
            });
        }
        (None, None) => {}
    }
}

fn validate_compatible_value(
    workspace: &WorkspaceIndex,
    value_type: inference::Resolution<ValueType>,
    expected: &TypeRef,
    range: Range,
    code: &'static str,
    action: &str,
    issues: &mut Vec<SemanticIssue>,
) {
    let inference::Resolution::Resolved(value_type) = value_type else {
        return;
    };
    if value_type == ValueType::Void {
        push_void_value(range, issues);
        return;
    }
    if workspace.type_compatibility(&value_type, expected) == Compatibility::Incompatible {
        issues.push(SemanticIssue {
            range,
            code,
            message: format!(
                "{action} {} to {}",
                value_type.display(),
                expected.display()
            ),
        });
    }
}

fn validate_expression(
    workspace: &WorkspaceIndex,
    current: &IndexedDocument,
    expression: &SemanticExpression,
    issues: &mut Vec<SemanticIssue>,
) -> inference::Resolution<ValueType> {
    match expression {
        SemanticExpression::Identifier { .. } | SemanticExpression::Literal { .. } => {
            inference::expression_type_outcome(workspace, current, expression, 0)
        }
        SemanticExpression::Member { object, .. } => {
            if matches!(
                validate_expression(workspace, current, object, issues),
                inference::Resolution::Resolved(ValueType::Void)
            ) {
                push_void_value(object.range(), issues);
                return inference::Resolution::Unsupported;
            }
            inference::expression_type_outcome(workspace, current, expression, 0)
        }
        SemanticExpression::Call {
            function,
            arguments,
            ..
        } => {
            let _ = validate_expression(workspace, current, function, issues);
            for argument in arguments {
                let _ = validate_expression(workspace, current, argument, issues);
            }
            inference::expression_type_outcome(workspace, current, expression, 0)
        }
        SemanticExpression::Subscript { array, index, .. } => {
            let array_type = validate_expression(workspace, current, array, issues);
            let index_type = validate_expression(workspace, current, index, issues);
            if let inference::Resolution::Resolved(index_type) = index_type {
                if index_type == ValueType::Void {
                    push_void_value(index.range(), issues);
                } else if !type_system::is_int(&index_type) {
                    issues.push(SemanticIssue {
                        range: index.range(),
                        code: "invalid-subscript-index",
                        message: format!("Array index must be Int, not {}", index_type.display()),
                    });
                }
            }
            if let inference::Resolution::Resolved(array_type) = &array_type {
                if array_type == &ValueType::Void {
                    push_void_value(array.range(), issues);
                } else if !matches!(array_type, ValueType::Known(ty) if ty.array) {
                    issues.push(SemanticIssue {
                        range: array.range(),
                        code: "invalid-subscript-target",
                        message: format!("Cannot subscript {}", array_type.display()),
                    });
                }
            }
            inference::expression_type_outcome(workspace, current, expression, 0)
        }
        SemanticExpression::Cast { value, ty, .. } => {
            let value_type = validate_expression(workspace, current, value, issues);
            if let inference::Resolution::Resolved(value_type) = &value_type
                && workspace.cast_compatibility(value_type, ty) == Compatibility::Incompatible
            {
                issues.push(SemanticIssue {
                    range: expression.range(),
                    code: "invalid-cast",
                    message: format!("Cannot cast {} to {}", value_type.display(), ty.display()),
                });
            }
            inference::expression_type_outcome(workspace, current, expression, 0)
        }
        SemanticExpression::TypeTest { value, ty, .. } => {
            let value_type = validate_expression(workspace, current, value, issues);
            if let inference::Resolution::Resolved(value_type) = &value_type
                && workspace.cast_compatibility(value_type, ty) == Compatibility::Incompatible
            {
                issues.push(SemanticIssue {
                    range: expression.range(),
                    code: "invalid-type-test",
                    message: format!(
                        "Cannot test {} against {}",
                        value_type.display(),
                        ty.display()
                    ),
                });
            }
            inference::expression_type_outcome(workspace, current, expression, 0)
        }
        SemanticExpression::Parenthesized { value, .. } => {
            validate_expression(workspace, current, value, issues)
        }
        SemanticExpression::New { size, .. } => {
            if let Some(size) = size {
                match validate_expression(workspace, current, size, issues) {
                    inference::Resolution::Resolved(ValueType::Void) => {
                        push_void_value(size.range(), issues);
                    }
                    inference::Resolution::Resolved(size_type)
                        if !type_system::is_int(&size_type) =>
                    {
                        issues.push(SemanticIssue {
                            range: size.range(),
                            code: "invalid-array-size",
                            message: format!("Array size must be Int, not {}", size_type.display()),
                        });
                    }
                    inference::Resolution::Resolved(_)
                    | inference::Resolution::Missing
                    | inference::Resolution::Ambiguous
                    | inference::Resolution::Unsupported => {}
                }
            }
            inference::expression_type_outcome(workspace, current, expression, 0)
        }
        SemanticExpression::Unary {
            operator,
            operator_range,
            argument,
            ..
        } => {
            let argument_type = validate_expression(workspace, current, argument, issues);
            let argument_type = match argument_type {
                inference::Resolution::Resolved(argument_type) => argument_type,
                inference::Resolution::Missing => return inference::Resolution::Missing,
                inference::Resolution::Ambiguous => return inference::Resolution::Ambiguous,
                inference::Resolution::Unsupported => return inference::Resolution::Unsupported,
            };
            let Some(result) = type_system::unary_result(*operator, &argument_type) else {
                issues.push(SemanticIssue {
                    range: *operator_range,
                    code: "invalid-unary-operand",
                    message: format!(
                        "Operator '{}' cannot be applied to {}",
                        unary_operator_text(*operator),
                        argument_type.display()
                    ),
                });
                return inference::Resolution::Unsupported;
            };
            inference::Resolution::Resolved(result)
        }
        SemanticExpression::Binary {
            left,
            operator,
            operator_range,
            right,
            ..
        } => {
            let left_type = validate_expression(workspace, current, left, issues);
            let right_type = validate_expression(workspace, current, right, issues);
            let (left_type, right_type) = match (left_type, right_type) {
                (
                    inference::Resolution::Resolved(left_type),
                    inference::Resolution::Resolved(right_type),
                ) => (left_type, right_type),
                (left, right) => return merge_failures(left, right),
            };
            let compatibility = if matches!(
                operator,
                SemanticBinaryOperator::Equal | SemanticBinaryOperator::NotEqual
            ) {
                workspace.equality_compatibility(&left_type, &right_type)
            } else if type_system::binary_result(*operator, &left_type, &right_type).is_some() {
                Compatibility::Compatible
            } else {
                Compatibility::Incompatible
            };
            if compatibility == Compatibility::Incompatible {
                issues.push(SemanticIssue {
                    range: *operator_range,
                    code: "invalid-binary-operands",
                    message: format!(
                        "Operator '{}' cannot combine {} and {}",
                        binary_operator_text(*operator),
                        left_type.display(),
                        right_type.display()
                    ),
                });
                return inference::Resolution::Unsupported;
            }
            type_system::binary_result(*operator, &left_type, &right_type).map_or(
                inference::Resolution::Unsupported,
                inference::Resolution::Resolved,
            )
        }
    }
}

fn merge_failures(
    left: inference::Resolution<ValueType>,
    right: inference::Resolution<ValueType>,
) -> inference::Resolution<ValueType> {
    match (left, right) {
        (inference::Resolution::Ambiguous, _) | (_, inference::Resolution::Ambiguous) => {
            inference::Resolution::Ambiguous
        }
        (inference::Resolution::Missing, _) | (_, inference::Resolution::Missing) => {
            inference::Resolution::Missing
        }
        (inference::Resolution::Unsupported, _) | (_, inference::Resolution::Unsupported) => {
            inference::Resolution::Unsupported
        }
        (inference::Resolution::Resolved(_), inference::Resolution::Resolved(_)) => {
            inference::Resolution::Unsupported
        }
    }
}

fn push_void_value(range: Range, issues: &mut Vec<SemanticIssue>) {
    issues.push(SemanticIssue {
        range,
        code: "void-value-use",
        message: "Expression does not produce a value".to_owned(),
    });
}

fn validate_call(
    workspace: &WorkspaceIndex,
    uri: &Uri,
    call: &SemanticCallSite,
    issues: &mut Vec<SemanticIssue>,
) {
    if !call.is_complete() {
        return;
    }
    let callee = match workspace.resolve_at_outcome(uri, call.callee_range.start) {
        inference::Resolution::Resolved(callee) => callee,
        inference::Resolution::Ambiguous => {
            if !issues.iter().any(|issue| {
                issue.range == call.callee_range && issue.code.starts_with("ambiguous-")
            }) {
                issues.push(SemanticIssue {
                    range: call.callee_range,
                    code: "ambiguous-call-target",
                    message: "Call target is ambiguous".to_owned(),
                });
            }
            return;
        }
        inference::Resolution::Missing | inference::Resolution::Unsupported => return,
    };
    if !matches!(
        callee.kind,
        DeclarationKind::Function | DeclarationKind::Event
    ) {
        issues.push(SemanticIssue {
            range: call.callee_range,
            code: "invalid-call-target",
            message: format!("'{}' is not callable", callee.name),
        });
        return;
    }
    validate_arguments(callee, call, issues);
}

fn validate_arguments(
    callee: &Declaration,
    call: &SemanticCallSite,
    issues: &mut Vec<SemanticIssue>,
) {
    let mut parameter_names = HashSet::new();
    if callee
        .parameters
        .iter()
        .any(|parameter| !parameter_names.insert(parameter.name.to_ascii_lowercase()))
    {
        return;
    }

    let mut bound = vec![false; callee.parameters.len()];
    let mut next_positional = 0;
    let mut first_extra = None;
    for argument in call.arguments() {
        if let Some(name) = argument.name.as_deref() {
            let Some(parameter_index) = callee
                .parameters
                .iter()
                .position(|parameter| parameter.name.eq_ignore_ascii_case(name))
            else {
                issues.push(SemanticIssue {
                    range: argument.name_range.unwrap_or(argument.range),
                    code: "unknown-named-argument",
                    message: format!("Unknown named argument '{name}' for '{}'", callee.name),
                });
                continue;
            };
            if bound[parameter_index] {
                issues.push(SemanticIssue {
                    range: argument.name_range.unwrap_or(argument.range),
                    code: "duplicate-named-argument",
                    message: format!("Duplicate argument for parameter '{name}'"),
                });
            } else {
                bound[parameter_index] = true;
            }
            continue;
        }

        while next_positional < bound.len() && bound[next_positional] {
            next_positional += 1;
        }
        if next_positional == bound.len() {
            first_extra.get_or_insert(argument.range);
        } else {
            bound[next_positional] = true;
            next_positional += 1;
        }
    }

    if let Some(range) = first_extra {
        issues.push(SemanticIssue {
            range,
            code: "too-many-arguments",
            message: format!(
                "Too many arguments for '{}': expected at most {}",
                callee.name,
                callee.parameters.len()
            ),
        });
    }

    let missing = callee
        .parameters
        .iter()
        .zip(bound)
        .filter(|(parameter, bound)| !bound && !parameter.has_default)
        .map(|(parameter, _)| parameter.name.as_str())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        issues.push(SemanticIssue {
            range: call.callee_range,
            code: "missing-required-argument",
            message: format!(
                "Missing required argument{} for '{}': {}",
                if missing.len() == 1 { "" } else { "s" },
                callee.name,
                missing.join(", ")
            ),
        });
    }
}

fn assignment_binary_operator(operator: SemanticAssignmentOperator) -> SemanticBinaryOperator {
    match operator {
        SemanticAssignmentOperator::Assign => unreachable!("simple assignment has no operator"),
        SemanticAssignmentOperator::Add => SemanticBinaryOperator::Add,
        SemanticAssignmentOperator::Subtract => SemanticBinaryOperator::Subtract,
        SemanticAssignmentOperator::Multiply => SemanticBinaryOperator::Multiply,
        SemanticAssignmentOperator::Divide => SemanticBinaryOperator::Divide,
        SemanticAssignmentOperator::Modulo => SemanticBinaryOperator::Modulo,
    }
}

fn assignment_operator_text(operator: SemanticAssignmentOperator) -> &'static str {
    match operator {
        SemanticAssignmentOperator::Assign => "=",
        SemanticAssignmentOperator::Add => "+=",
        SemanticAssignmentOperator::Subtract => "-=",
        SemanticAssignmentOperator::Multiply => "*=",
        SemanticAssignmentOperator::Divide => "/=",
        SemanticAssignmentOperator::Modulo => "%=",
    }
}

fn unary_operator_text(operator: crate::semantic::SemanticUnaryOperator) -> &'static str {
    match operator {
        crate::semantic::SemanticUnaryOperator::LogicalNot => "!",
        crate::semantic::SemanticUnaryOperator::Negate => "-",
    }
}

fn binary_operator_text(operator: SemanticBinaryOperator) -> &'static str {
    match operator {
        SemanticBinaryOperator::LogicalOr => "||",
        SemanticBinaryOperator::LogicalAnd => "&&",
        SemanticBinaryOperator::Equal => "==",
        SemanticBinaryOperator::NotEqual => "!=",
        SemanticBinaryOperator::Less => "<",
        SemanticBinaryOperator::LessEqual => "<=",
        SemanticBinaryOperator::Greater => ">",
        SemanticBinaryOperator::GreaterEqual => ">=",
        SemanticBinaryOperator::Add => "+",
        SemanticBinaryOperator::Subtract => "-",
        SemanticBinaryOperator::Multiply => "*",
        SemanticBinaryOperator::Divide => "/",
        SemanticBinaryOperator::Modulo => "%",
    }
}

fn issue_diagnostic(issue: SemanticIssue) -> Diagnostic {
    Diagnostic {
        range: issue.range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String(issue.code.to_owned())),
        code_description: None,
        source: Some(DIAGNOSTIC_SOURCE.to_owned()),
        message: issue.message,
        related_information: None,
        tags: None,
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use lsp_types::NumberOrString;

    use crate::config::WorkspaceConfig;

    use super::super::{WorkspaceIndex, path_to_file_uri};

    #[test]
    fn accepts_valid_assignment_operator_return_and_condition_types() {
        let root = temp_root("valid-type-checking");
        fs::write(root.join("Parent.psc"), "ScriptName Parent\n").unwrap();
        fs::write(root.join("Child.psc"), "ScriptName Child Extends Parent\n").unwrap();
        let source = concat!(
            "ScriptName Project\n",
            "Child ChildValue\n",
            "Int Count = 1\n",
            "Float Ratio = Count\n",
            "String Text = ChildValue\n",
            "Bool Ready = ChildValue\n",
            "Bool EmptyFlag = None\n",
            "String EmptyText = None\n",
            "Parent Upcast = ChildValue\n",
            "Parent EmptyValue = None\n",
            "Int[] Numbers = New Int[3]\n",
            "Int Function Calculate()\n",
            "  Count += 1\n",
            "  Ratio = Count + 0.5\n",
            "  Numbers[0] = Count\n",
            "  If ChildValue && Count\n",
            "  ElseIf Text\n",
            "    While Numbers\n",
            "      Count -= 1\n",
            "    EndWhile\n",
            "  EndIf\n",
            "  Return Count\n",
            "EndFunction\n",
        );
        let path = root.join("Project.psc");
        fs::write(&path, source).unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        assert_eq!(
            index.semantic_diagnostics(&path_to_file_uri(&path).unwrap()),
            []
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_assignment_operator_return_and_condition_type_errors() {
        let root = temp_root("invalid-type-checking");
        fs::write(root.join("Actor.psc"), "ScriptName Actor\n").unwrap();
        let source = concat!(
            "ScriptName Project\n",
            "Int ReadOnly = 1 Const\n",
            "Int Property Fixed = 1 AutoReadOnly\n",
            "Int[] Values\n",
            "Function NoValue()\n",
            "EndFunction\n",
            "Int Function WrongReturn()\n",
            "  Return \"bad\"\n",
            "EndFunction\n",
            "Function UnexpectedReturn()\n",
            "  Return 1\n",
            "EndFunction\n",
            "Int Function MissingReturnValue()\n",
            "  Return\n",
            "EndFunction\n",
            "Function Test()\n",
            "  Int Count = \"bad\"\n",
            "  Int NotNullable = None\n",
            "  Count = \"bad\"\n",
            "  ReadOnly = 2\n",
            "  Fixed = 2\n",
            "  Values[0] += 1\n",
            "  Count %= 2.0\n",
            "  Count = -\"bad\"\n",
            "  Count = 1 % 2.0\n",
            "  Count = Values[\"bad\"]\n",
            "  Count = Count[0]\n",
            "  Count = NoValue()\n",
            "  Int[] Other = New Int[\"bad\"]\n",
            "  Actor InvalidCast = 1 As Actor\n",
            "  Bool InvalidTest = 1 Is Actor\n",
            "  If NoValue()\n",
            "  ElseIf 1 + Values\n",
            "  EndIf\n",
            "EndFunction\n",
        );
        let path = root.join("Project.psc");
        fs::write(&path, source).unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let codes = index
            .semantic_diagnostics(&path_to_file_uri(&path).unwrap())
            .into_iter()
            .filter_map(|diagnostic| match diagnostic.code {
                Some(NumberOrString::String(code)) => Some(code),
                Some(NumberOrString::Number(_)) | None => None,
            })
            .collect::<Vec<_>>();
        for expected in [
            "incompatible-assignment",
            "invalid-assignment-target",
            "invalid-compound-assignment",
            "incompatible-return",
            "unexpected-return-value",
            "missing-return-value",
            "invalid-unary-operand",
            "invalid-binary-operands",
            "invalid-subscript-index",
            "invalid-subscript-target",
            "void-value-use",
            "invalid-array-size",
            "invalid-cast",
            "invalid-type-test",
            "invalid-condition",
        ] {
            assert!(
                codes.iter().any(|code| code == expected),
                "missing {expected} in {codes:?}"
            );
        }
        assert_eq!(
            codes
                .iter()
                .filter(|code| code.as_str() == "invalid-assignment-target")
                .count(),
            2
        );
        fs::remove_dir_all(root).unwrap();
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
