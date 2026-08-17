# Known issues and deferred work

## Semantic-navigation milestone

The server provides syntax and conservative reference/call diagnostics, semantic completion, hover, definition, references, rename, signature help, and document/workspace symbols. It does not yet provide:

- assignment, return, operator, or control-flow type checking;
- compiler discovery or automatic compilation;
- formatting;
- debugging.

Semantic diagnostics intentionally remain silent for ambiguous definitions, unresolved receiver chains, incomplete inheritance, unsupported expression shapes, incomplete calls, and files with syntax diagnostics. These safeguards avoid claims that require compiler flags or unavailable SDK sources. Broader type checking is planned as a later milestone. Debugging requires a separate Debug Adapter Protocol design and runtime transport.

## Diagnostic locations

For a missing closing keyword, the server reports the conflicting closer or the end of the document where the missing keyword becomes unambiguous. It does not invent an earlier insertion point when more than one repair could be valid.

## Workspace scope

The index scans configured source roots and import directories on a bounded background worker and overlays open buffers. The active index is held in memory, and parsed declarations, call sites, and identifier occurrences are restored from a private per-user cache only when source metadata and the current content fingerprint match. Requests requiring the complete workspace index wait for initial indexing; syntax diagnostics and document symbols remain available for open buffers during indexing, while semantic diagnostics begin after a successful index. `auto` does not infer a game dialect. Starfield SDK discovery requires an installed Steam Creation Kit and an explicit `starfield` dialect. Unsupported or ambiguous expressions intentionally produce no IntelliSense, reference, or semantic-diagnostic claim.

Rename edits declarations and resolved occurrences in project source roots only. Configured imports and discovered SDK sources are read-only. Script rename can rename the matching `.psc` file when the client supports LSP rename-file operations, but the namespace prefix cannot change and namespace-directory moves are not inferred.

Old fingerprinted SDK cache generations are not automatically removed yet.

## Prebuilt platforms

The release workflow produces native archives for:

- Windows x64;
- Linux x64 using glibc;
- macOS Intel;
- macOS Apple Silicon.

Other operating systems, architectures, and Linux C library environments must build the server from source. The Linux and macOS archives have hosted build and packaging coverage but have not yet received manual runtime acceptance through Zed.

## Compiler and game discovery

With the `starfield` dialect selected on Windows, the server reads Steam installation metadata to locate the Starfield Creation Kit source archive. It does not automatically discover compiler executables, compiler flags, installed game scripts outside that archive, or user projects. Future compiler integration will require explicit user configuration with optional discovery as a convenience; it must not distribute Bethesda tools or source.
