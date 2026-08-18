# Changelog

## Unreleased

- Added conservative expression type inference for chained function returns, casts, parenthesized values, array elements, and `Self` across completion and semantic navigation.
- Added conservative open-document semantic diagnostics for definite unresolved references and types, missing members, invalid call targets, and named, excess, or missing call arguments, with syntax, ambiguity, incomplete-hierarchy, and overlay safeguards.
- Added complete assignment, initializer, return, operator, cast, type-test, array, and basic control-condition type diagnostics with Papyrus implicit conversions, ambiguity reporting, and cascade suppression.
- Persisted structured expression receivers in semantic cache schema v7 without storing source text.
- Persisted occurrence roles, parameter-default metadata, call completeness, and argument diagnostic ranges in semantic cache schema v8.
- Persisted spanned expression trees and statement-level type-check sites in semantic cache schema v9.
- Invalidated semantic cache schema v9 with v10 so corrected group-label filtering and getter-only property writability are rebuilt.

## Version 0.2.0 (August 16th, 2026)

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
- Added conservative project-source rename with identifier and collision validation, ambiguity safeguards, external-source protection, and same-namespace Papyrus script file renames through LSP workspace edits.
- Persisted syntax-backed identifier occurrences in semantic cache schema v6 without storing source text.
- Split semantic navigation and shared resolution from the workspace indexing and completion module.
- Moved discovery, bounded scanning, and Starfield source extraction behind initialization onto a cancellable background worker with LSP progress reporting and overlay replay.
- Hardened semantic and Starfield caches with private platform directories, content verification, resource limits, exclusive staging, and immutable atomic generations.
- Contained malformed LSP parameters without terminating the session and corrected inbound Windows UNC file URI handling.
- Made rename validate every edit in a post-rename semantic view, attach open-document versions, and support case-only script file renames.
- Fixed Unicode-safe generated-source filtering, root-relative fragment filtering, and stale overlays after a backing file is deleted.
- Added Windows, Linux, and macOS Rust CI coverage plus a pinned RustSec dependency audit.

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
