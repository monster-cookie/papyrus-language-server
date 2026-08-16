# Changelog

## Unreleased

- Added `auto`, Skyrim, Fallout 4, and Starfield workspace configuration.
- Added recursive project-source and import-directory indexing.
- Added hierarchical document symbols and case-insensitive workspace symbol search.
- Added unsaved-buffer symbol overlays with disk restoration on close.
- Added source-derived, inheritance-aware completion, hover, and go to definition.
- Added Windows Steam discovery for Starfield Creation Kit source archives.
- Added a fingerprinted local cache of navigable SFCK sources with generated-fragment filtering.
- Added conservative ambiguity handling that returns no semantic claim instead of guessing.
- Filtered constant-style members from instance completion while retaining explicit hover and go to definition.
- Fixed LSP file URIs produced from Windows extended-length drive and UNC paths.
- Added conservative, workspace-wide find references with scope, inheritance, import, ambiguity, overlay, and identical-alias handling.
- Added source-derived signature help for positional, named, inherited, imported, script-qualified, nested, and incomplete calls.
- Persisted syntax-backed identifier occurrences in semantic cache schema v4 without storing source text.
- Split semantic navigation and shared resolution from the workspace indexing and completion module.

## Version 0.1.0 (August 2nd, 2026)

- Added an original Tree-sitter grammar covering the Papyrus dialects used by Skyrim Anniversary Edition, Fallout 4, and Starfield.
- Added a native Rust language server that communicates over standard input and standard output.
- Added syntax and structural diagnostics for unsaved open buffers.
- Added human-readable diagnostics for missing and unexpected block-closing keywords.
- Added UTF-16 position conversion for Language Server Protocol diagnostic ranges.
- Added original Basic, Advanced, and Invalid fixtures for all three supported game dialects.
- Added grammar generation, WebAssembly parsing, native corpus, protocol-session, diagnostic-range, and structural-recovery tests.
- Added local installed-source audit support without copying or retaining Bethesda source files.
- Validated the grammar against installed Starfield, Skyrim Anniversary Edition, and Fallout 4 source trees.
- Added release packaging for Windows x64, Linux x64 using glibc, macOS Intel, and macOS Apple Silicon.
- Validated local-binary diagnostics and unsaved-buffer updates through the Zed editor on Windows x64.
