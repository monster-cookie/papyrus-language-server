# Known issues and deferred work

## Workspace-foundation milestone

The server provides syntax diagnostics and syntactic document/workspace symbols. It does not yet provide:

- completion or signature help;
- hover information;
- go to definition, references, or rename;
- semantic symbol resolution or project-aware import resolution;
- semantic type checking;
- compiler discovery or automatic compilation;
- formatting;
- debugging.

These features are planned as later milestones. Debugging requires a separate Debug Adapter Protocol design and runtime transport.

## Diagnostic locations

For a missing closing keyword, the server reports the conflicting closer or the end of the document where the missing keyword becomes unambiguous. It does not invent an earlier insertion point when more than one repair could be valid.

## Workspace scope

The index scans configured source roots and import directories and overlays open buffers. Indexing is synchronous and in-memory, and `auto` does not yet infer a game dialect. The server does not yet resolve imports or types across files.

## Prebuilt platforms

The release workflow produces native archives for:

- Windows x64;
- Linux x64 using glibc;
- macOS Intel;
- macOS Apple Silicon.

Other operating systems, architectures, and Linux C library environments must build the server from source. The Linux and macOS archives have hosted build and packaging coverage but have not yet received manual runtime acceptance through Zed.

## Compiler and game discovery

The server does not read the Windows registry, Steam libraries, Creation Kit installations, compiler flags, installed game scripts, or user projects automatically. Future compiler integration will require explicit user configuration with optional discovery as a convenience; it must not distribute Bethesda tools or source.
