use std::ops::Range;

/// A precise structural problem discovered independently of parser recovery.
pub(crate) struct StructuralIssue {
    /// UTF-8 byte range to underline.
    pub(crate) range: Range<usize>,
    /// Stable diagnostic identifier.
    pub(crate) code: &'static str,
    /// Closing keyword that the diagnostic reports as missing, when applicable.
    pub(crate) missing_keyword: Option<&'static str>,
    /// Human-readable explanation.
    pub(crate) message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlockKind {
    Event,
    Function,
    Group,
    If,
    LockGuard,
    State,
    Struct,
    TryLockGuard,
    While,
}

impl BlockKind {
    fn closing_keyword(self) -> &'static str {
        match self {
            Self::Event => "EndEvent",
            Self::Function => "EndFunction",
            Self::Group => "EndGroup",
            Self::If => "EndIf",
            Self::LockGuard => "EndLockGuard",
            Self::State => "EndState",
            Self::Struct => "EndStruct",
            Self::TryLockGuard => "EndTryLockGuard",
            Self::While => "EndWhile",
        }
    }
}

struct OpenBlock {
    kind: BlockKind,
    range: Range<usize>,
}

struct Token {
    normalized: String,
    range: Range<usize>,
}

/// Finds mismatched and unclosed Papyrus blocks while ignoring comments and strings.
pub(crate) fn validate(source: &str) -> Vec<StructuralIssue> {
    let sanitized = sanitize(source);
    let mut issues = Vec::new();
    let mut stack = Vec::<OpenBlock>::new();
    let mut line_start = 0;
    let mut statement_start = 0;
    let mut statement = String::new();

    for line in sanitized.split_inclusive('\n') {
        if statement.is_empty() {
            statement_start = line_start;
        }
        statement.push_str(line);
        line_start += line.len();
        if line
            .trim_end_matches(['\r', '\n'])
            .trim_end()
            .ends_with('\\')
        {
            continue;
        }

        process_statement(&statement, statement_start, &mut stack, &mut issues);
        statement.clear();
    }

    if !statement.is_empty() {
        process_statement(&statement, statement_start, &mut stack, &mut issues);
    }

    for open in stack.into_iter().rev() {
        issues.push(StructuralIssue {
            range: open.range,
            code: "missing-closing-keyword",
            missing_keyword: Some(open.kind.closing_keyword()),
            message: format!("Missing {}", open.kind.closing_keyword()),
        });
    }

    issues
}

fn process_statement(
    statement: &str,
    statement_start: usize,
    stack: &mut Vec<OpenBlock>,
    issues: &mut Vec<StructuralIssue>,
) {
    let tokens = tokens(statement, statement_start);
    let Some(first) = tokens.first() else {
        return;
    };

    if let Some(closing_kind) = closing_kind(&first.normalized) {
        close_block(closing_kind, first, stack, issues);
        return;
    }

    if let Some(opening_kind) = opening_kind(statement, statement_start, &tokens) {
        let keyword = opening_token(opening_kind, &tokens).unwrap_or(first);
        stack.push(OpenBlock {
            kind: opening_kind,
            range: keyword.range.clone(),
        });
    }
}

fn close_block(
    closing_kind: BlockKind,
    closing_token: &Token,
    stack: &mut Vec<OpenBlock>,
    issues: &mut Vec<StructuralIssue>,
) {
    let Some(matching_index) = stack.iter().rposition(|open| open.kind == closing_kind) else {
        issues.push(StructuralIssue {
            range: closing_token.range.clone(),
            code: "unexpected-closing-keyword",
            missing_keyword: None,
            message: format!("Unexpected {}", closing_kind.closing_keyword()),
        });
        return;
    };

    while stack.len() - 1 > matching_index {
        let Some(unclosed) = stack.pop() else {
            break;
        };
        issues.push(StructuralIssue {
            range: closing_token.range.clone(),
            code: "missing-closing-keyword",
            missing_keyword: Some(unclosed.kind.closing_keyword()),
            message: format!(
                "Missing {} before {}",
                unclosed.kind.closing_keyword(),
                closing_kind.closing_keyword()
            ),
        });
    }

    stack.pop();
}

fn opening_kind(line: &str, line_start: usize, tokens: &[Token]) -> Option<BlockKind> {
    let first = tokens.first()?.normalized.as_str();
    let contains_native = tokens.iter().any(|token| token.normalized == "native");

    match first {
        "if" => Some(BlockKind::If),
        "while" => Some(BlockKind::While),
        "event" if !contains_native => Some(BlockKind::Event),
        "state" => Some(BlockKind::State),
        "auto"
            if tokens
                .get(1)
                .is_some_and(|token| token.normalized == "state") =>
        {
            Some(BlockKind::State)
        }
        "struct" => Some(BlockKind::Struct),
        "group" => Some(BlockKind::Group),
        "lockguard" => Some(BlockKind::LockGuard),
        "trylockguard" => Some(BlockKind::TryLockGuard),
        _ if !contains_native && function_token(line, line_start, tokens).is_some() => {
            Some(BlockKind::Function)
        }
        _ => None,
    }
}

fn opening_token(kind: BlockKind, tokens: &[Token]) -> Option<&Token> {
    let keyword = match kind {
        BlockKind::Event => "event",
        BlockKind::Function => "function",
        BlockKind::Group => "group",
        BlockKind::If => "if",
        BlockKind::LockGuard => "lockguard",
        BlockKind::State => "state",
        BlockKind::Struct => "struct",
        BlockKind::TryLockGuard => "trylockguard",
        BlockKind::While => "while",
    };
    tokens.iter().find(|token| token.normalized == keyword)
}

fn function_token<'a>(line: &str, line_start: usize, tokens: &'a [Token]) -> Option<&'a Token> {
    let opening_parenthesis = line.find('(').unwrap_or(line.len());
    tokens.iter().find(|token| {
        if token.normalized != "function" || token.range.start - line_start > opening_parenthesis {
            return false;
        }
        let relative_start = token.range.start - line_start;
        relative_start == 0 || line.as_bytes().get(relative_start - 1) != Some(&b'.')
    })
}

fn closing_kind(keyword: &str) -> Option<BlockKind> {
    match keyword {
        "endevent" => Some(BlockKind::Event),
        "endfunction" => Some(BlockKind::Function),
        "endgroup" => Some(BlockKind::Group),
        "endif" => Some(BlockKind::If),
        "endlockguard" => Some(BlockKind::LockGuard),
        "endstate" => Some(BlockKind::State),
        "endstruct" => Some(BlockKind::Struct),
        "endtrylockguard" => Some(BlockKind::TryLockGuard),
        "endwhile" => Some(BlockKind::While),
        _ => None,
    }
}

fn tokens(line: &str, line_start: usize) -> Vec<Token> {
    let bytes = line.as_bytes();
    let mut result = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        if !bytes[index].is_ascii_alphabetic() && bytes[index] != b'_' {
            index += 1;
            continue;
        }

        let start = index;
        index += 1;
        while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
        {
            index += 1;
        }

        result.push(Token {
            normalized: line[start..index].to_ascii_lowercase(),
            range: line_start + start..line_start + index,
        });
    }

    result
}

#[derive(Clone, Copy)]
enum SanitizeState {
    Code,
    DocumentationComment,
    LineComment,
    SlashComment,
    String,
}

fn sanitize(source: &str) -> String {
    let mut output = source.as_bytes().to_vec();
    let source_bytes = source.as_bytes();
    let mut state = SanitizeState::Code;
    let mut index = 0;

    while index < source_bytes.len() {
        match state {
            SanitizeState::Code => match source_bytes[index] {
                b'"' => {
                    mask(&mut output, index);
                    state = SanitizeState::String;
                    index += 1;
                }
                b'{' => {
                    mask(&mut output, index);
                    state = SanitizeState::DocumentationComment;
                    index += 1;
                }
                b';' if source_bytes.get(index + 1) == Some(&b'/') => {
                    mask(&mut output, index);
                    mask(&mut output, index + 1);
                    state = SanitizeState::SlashComment;
                    index += 2;
                }
                b';' => {
                    mask(&mut output, index);
                    state = SanitizeState::LineComment;
                    index += 1;
                }
                _ => index += 1,
            },
            SanitizeState::String => {
                if source_bytes[index] == b'\\' {
                    mask(&mut output, index);
                    if index + 1 < source_bytes.len() {
                        mask(&mut output, index + 1);
                    }
                    index += 2;
                } else if source_bytes[index] == b'"' {
                    mask(&mut output, index);
                    state = SanitizeState::Code;
                    index += 1;
                } else {
                    mask(&mut output, index);
                    index += 1;
                }
            }
            SanitizeState::DocumentationComment => {
                let is_end = source_bytes[index] == b'}';
                mask(&mut output, index);
                index += 1;
                if is_end {
                    state = SanitizeState::Code;
                }
            }
            SanitizeState::SlashComment => {
                if source_bytes[index] == b'/' && source_bytes.get(index + 1) == Some(&b';') {
                    mask(&mut output, index);
                    mask(&mut output, index + 1);
                    state = SanitizeState::Code;
                    index += 2;
                } else {
                    mask(&mut output, index);
                    index += 1;
                }
            }
            SanitizeState::LineComment => {
                if source_bytes[index] == b'\n' {
                    state = SanitizeState::Code;
                } else {
                    mask(&mut output, index);
                }
                index += 1;
            }
        }
    }

    String::from_utf8(output).unwrap_or_else(|_| source.to_owned())
}

fn mask(output: &mut [u8], index: usize) {
    if let Some(byte) = output.get_mut(index) {
        if *byte != b'\r' && *byte != b'\n' {
            *byte = b' ';
        }
    }
}

#[cfg(test)]
mod tests {
    use super::validate;

    #[test]
    fn identifies_missing_inner_closer() {
        let issues = validate("Function Run()\nIf True\nEndFunction\n");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].message, "Missing EndIf before EndFunction");
    }

    #[test]
    fn ignores_keywords_in_comments_and_strings() {
        let source = concat!(
            "Function Run()\n",
            "  Debug.Trace(\"If EndFunction\")\n",
            "  ; If\n",
            "  { EndFunction }\n",
            "  ;/ While EndWhile /;\n",
            "EndFunction\n",
        );
        assert!(validate(source).is_empty());
    }

    #[test]
    fn native_declarations_do_not_open_blocks() {
        let source = "Function NativeCall() Native\nEvent NativeEvent() Native\n";
        assert!(validate(source).is_empty());
    }

    #[test]
    fn keywords_are_case_insensitive_with_crlf_input() {
        let source = "function Run()\r\nIF True\r\nendfunction\r\n";
        let issues = validate(source);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].message, "Missing EndIf before EndFunction");
    }

    #[test]
    fn native_declaration_can_continue_onto_another_line() {
        let source = concat!(
            "Function NativeCall(Int Value, \\\n",
            "    Int OtherValue) Native\n",
        );
        assert!(validate(source).is_empty());
    }
}
