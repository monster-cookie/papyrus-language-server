# Known issues and deferred work

## Diagnostic-first milestone

Version 0.1 provides syntax and structural diagnostics for open buffers. It does not yet provide:

- completion or signature help;
- hover information;
- go to definition, references, rename, or workspace symbols;
- workspace indexing or project-aware import resolution;
- semantic type checking;
- compiler discovery or automatic compilation;
- formatting;
- debugging.

These features are planned as later milestones. Debugging requires a separate Debug Adapter Protocol design and runtime transport.

## Diagnostic locations

For a missing closing keyword, the server reports the conflicting closer or the end of the document where the missing keyword becomes unambiguous. It does not invent an earlier insertion point when more than one repair could be valid.

## Workspace scope

Version 0.1 analyzes the current text supplied by an editor. It does not scan neighboring scripts, resolve imports, or infer a game dialect from a project layout.

## Prebuilt platforms

The release workflow produces native archives for:

- Windows x64;
- Linux x64 using glibc;
- macOS Intel;
- macOS Apple Silicon.

Other operating systems, architectures, and Linux C library environments must build the server from source. The Linux and macOS archives have hosted build and packaging coverage but have not yet received manual runtime acceptance through Zed.

## Compiler and game discovery

The server does not read the Windows registry, Steam libraries, Creation Kit installations, compiler flags, installed game scripts, or user projects automatically. Future compiler integration will require explicit user configuration with optional discovery as a convenience; it must not distribute Bethesda tools or source.
