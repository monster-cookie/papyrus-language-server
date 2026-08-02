//! Papyrus language support for the Tree-sitter parsing library.

use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_papyrus() -> *const ();
}

/// The Tree-sitter language function for the Papyrus grammar.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_papyrus) };

/// The generated static node-type description for the Papyrus grammar.
pub const NODE_TYPES: &str = include_str!("../../src/node-types.json");

#[cfg(test)]
mod tests {
    #[test]
    fn grammar_loads_in_tree_sitter() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("Papyrus grammar should load");
    }
}
