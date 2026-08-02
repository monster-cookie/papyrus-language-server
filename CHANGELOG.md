# Changelog

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
