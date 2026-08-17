use std::collections::HashSet;

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Range, Uri};

use crate::semantic::{Declaration, DeclarationKind, SemanticCallSite, SemanticOccurrenceKind};

use super::{WorkspaceIndex, inference, navigation::is_primitive_type};

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
                    if matches!(
                        self.resolve_occurrence_outcome(current, occurrence),
                        inference::Resolution::Missing
                    ) {
                        issues.push(SemanticIssue {
                            range: occurrence.selection_range,
                            code: "unresolved-type",
                            message: format!("Unresolved type '{}'", occurrence.name),
                        });
                    }
                }
                SemanticOccurrenceKind::Member | SemanticOccurrenceKind::Reference => {
                    if matches!(
                        self.resolve_occurrence_outcome(current, occurrence),
                        inference::Resolution::Missing
                    ) {
                        let (code, label) = if occurrence.kind == SemanticOccurrenceKind::Member {
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
                }
            }
        }

        for call in &current.semantic.call_sites {
            validate_call(self, uri, call, &mut issues);
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
        inference::Resolution::Missing
        | inference::Resolution::Ambiguous
        | inference::Resolution::Unsupported => return,
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
