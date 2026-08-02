use std::ops::Range as ByteRange;

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use tree_sitter::{Node, Parser};

use crate::{line_index::LineIndex, structure};

/// Parses Papyrus source and produces native syntax diagnostics.
pub struct PapyrusAnalyzer {
    parser: Parser,
}

impl PapyrusAnalyzer {
    /// Creates an analyzer backed by the canonical Papyrus grammar.
    ///
    /// # Errors
    ///
    /// Returns an error if the generated grammar cannot be loaded by Tree-sitter.
    pub fn new() -> Result<Self, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_papyrus::LANGUAGE.into())
            .map_err(|error| format!("failed to load the Papyrus grammar: {error}"))?;
        Ok(Self { parser })
    }

    /// Analyzes a complete source buffer and returns current syntax diagnostics.
    pub fn diagnostics(&mut self, source: &str) -> Vec<Diagnostic> {
        let line_index = LineIndex::new(source);
        let structural_issues = structure::validate(source);
        let structural_ranges = structural_issues
            .iter()
            .map(|issue| issue.range.clone())
            .collect::<Vec<_>>();
        let structural_missing_keywords = structural_issues
            .iter()
            .filter_map(|issue| issue.missing_keyword)
            .collect::<Vec<_>>();
        let mut diagnostics = structural_issues
            .into_iter()
            .map(|issue| {
                diagnostic(
                    line_index.range(source, issue.range),
                    issue.code,
                    issue.message,
                )
            })
            .collect::<Vec<_>>();

        let Some(tree) = self.parser.parse(source, None) else {
            diagnostics.push(diagnostic(
                line_index.range(source, 0..0),
                "parser-failure",
                "Tree-sitter could not parse the Papyrus document".to_owned(),
            ));
            return diagnostics;
        };

        let mut syntax_issues = Vec::new();
        collect_syntax_issues(
            tree.root_node(),
            &structural_ranges,
            &structural_missing_keywords,
            &mut syntax_issues,
        );
        diagnostics.extend(syntax_issues.into_iter().map(|issue| {
            diagnostic(
                line_index.range(source, issue.range),
                issue.code,
                issue.message,
            )
        }));
        diagnostics
    }
}

struct SyntaxIssue {
    range: ByteRange<usize>,
    code: &'static str,
    message: String,
}

fn collect_syntax_issues(
    node: Node<'_>,
    structural_ranges: &[ByteRange<usize>],
    structural_missing_keywords: &[&str],
    issues: &mut Vec<SyntaxIssue>,
) {
    let node_range = node.start_byte()..node.end_byte();
    let covered_by_structural_issue = structural_ranges
        .iter()
        .any(|range| contains(&node_range, range.start));

    if node.is_missing() && !covered_by_structural_issue {
        let display_name = display_missing_node(node.kind());
        if !structural_missing_keywords
            .iter()
            .any(|keyword| keyword.eq_ignore_ascii_case(&display_name))
        {
            issues.push(SyntaxIssue {
                range: node.start_byte()..node.start_byte(),
                code: "missing-syntax",
                message: format!("Missing {display_name}"),
            });
        }
    } else if node.is_error() && !covered_by_structural_issue {
        issues.push(SyntaxIssue {
            range: nonempty_range(node_range),
            code: "syntax-error",
            message: "Invalid Papyrus syntax".to_owned(),
        });
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_syntax_issues(
            child,
            structural_ranges,
            structural_missing_keywords,
            issues,
        );
    }
}

fn contains(range: &ByteRange<usize>, offset: usize) -> bool {
    range.start <= offset && offset <= range.end
}

fn nonempty_range(range: ByteRange<usize>) -> ByteRange<usize> {
    if range.start == range.end {
        range.start..range.start.saturating_add(1)
    } else {
        range
    }
}

fn display_missing_node(kind: &str) -> String {
    match kind.to_ascii_lowercase().as_str() {
        "endevent" => "EndEvent".to_owned(),
        "endfunction" => "EndFunction".to_owned(),
        "endgroup" => "EndGroup".to_owned(),
        "endif" => "EndIf".to_owned(),
        "endlockguard" => "EndLockGuard".to_owned(),
        "endproperty" => "EndProperty".to_owned(),
        "endstate" => "EndState".to_owned(),
        "endstruct" => "EndStruct".to_owned(),
        "endtrylockguard" => "EndTryLockGuard".to_owned(),
        "endwhile" => "EndWhile".to_owned(),
        _ => kind.to_owned(),
    }
}

fn diagnostic(range: lsp_types::Range, code: &'static str, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String(code.to_owned())),
        code_description: None,
        source: Some("papyrus-language-server".to_owned()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}
