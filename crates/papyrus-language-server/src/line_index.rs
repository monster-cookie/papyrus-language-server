use lsp_types::{Position, Range};

/// Converts UTF-8 byte offsets from Tree-sitter into UTF-16 LSP positions.
pub(crate) struct LineIndex {
    line_starts: Vec<usize>,
}

impl LineIndex {
    /// Builds an index for the supplied source text.
    pub(crate) fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(offset + 1);
            }
        }
        Self { line_starts }
    }

    /// Converts a UTF-8 byte offset into an LSP position.
    pub(crate) fn position(&self, text: &str, byte_offset: usize) -> Position {
        let mut offset = byte_offset.min(text.len());
        while offset > 0 && !text.is_char_boundary(offset) {
            offset -= 1;
        }

        let line = self
            .line_starts
            .partition_point(|line_start| *line_start <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line];
        let utf16_column = text[line_start..offset].encode_utf16().count();

        Position {
            line: saturating_u32(line),
            character: saturating_u32(utf16_column),
        }
    }

    /// Converts a half-open UTF-8 byte range into an LSP range.
    pub(crate) fn range(&self, text: &str, byte_range: std::ops::Range<usize>) -> Range {
        Range {
            start: self.position(text, byte_range.start),
            end: self.position(text, byte_range.end),
        }
    }

    /// Converts a UTF-16 LSP position into a UTF-8 byte offset.
    pub(crate) fn byte_offset(&self, text: &str, position: Position) -> usize {
        let line = usize::try_from(position.line).unwrap_or(usize::MAX);
        let start = *self.line_starts.get(line).unwrap_or(&text.len());
        let end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(text.len());
        let target = usize::try_from(position.character).unwrap_or(usize::MAX);
        let mut utf16 = 0;
        for (relative, character) in text[start..end].char_indices() {
            if utf16 >= target {
                return start + relative;
            }
            utf16 += character.len_utf16();
        }
        end
    }
}

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::LineIndex;

    #[test]
    fn counts_utf16_code_units() {
        let text = "A😀B\r\nNext";
        let index = LineIndex::new(text);

        assert_eq!(index.position(text, "A😀".len()).line, 0);
        assert_eq!(index.position(text, "A😀".len()).character, 3);
        assert_eq!(index.position(text, "A😀B\r\n".len()).line, 1);
        assert_eq!(index.position(text, "A😀B\r\n".len()).character, 0);
    }
}
