# Known issues and deferred work

## Semantic-navigation milestone

The server provides syntax diagnostics, semantic completion, hover, definition, and document/workspace symbols. It does not yet provide:

- references, rename, or signature help;
- full expression type inference;
- semantic type checking;
- compiler discovery or automatic compilation;
- formatting;
- debugging.

These features are planned as later milestones. Debugging requires a separate Debug Adapter Protocol design and runtime transport.

## Diagnostic locations

For a missing closing keyword, the server reports the conflicting closer or the end of the document where the missing keyword becomes unambiguous. It does not invent an earlier insertion point when more than one repair could be valid.

## Workspace scope

The index scans configured source roots and import directories and overlays open buffers. Indexing is synchronous, the active index is held in memory, and parsed declarations are restored from a local cache when source metadata and fingerprints match. `auto` does not infer a game dialect. Starfield SDK discovery requires an installed Steam Creation Kit and an explicit `starfield` dialect. Unsupported or ambiguous expressions intentionally produce no IntelliSense result.

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
