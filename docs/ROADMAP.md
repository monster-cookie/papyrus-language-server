# Roadmap

## 0.1: Native syntax diagnostics

- Canonical cross-dialect Tree-sitter grammar.
- Rust stdio language server.
- Unsaved-buffer syntax diagnostics.
- Human-readable missing and unexpected block closers.
- Native release archives for Windows, Linux, and macOS.
- Zed extension download and launch integration.

## 0.2: Workspace foundation

- Game and dialect selection with a safe `auto` mode.
- Configured import directories and project source roots.
- Workspace symbol index.
- Document and workspace symbol providers.

## 0.3: Semantic navigation

- Source-derived completion, hover, and go to definition.
- Starfield Creation Kit source discovery and local navigable cache.
- References and signature help.
- Conservative project-source rename, including same-namespace script file renames.
- Semantic diagnostics.

## 0.4: Optional compiler integration

- Explicit compiler, flags, import, and output settings.
- Windows registry and Steam-library discovery in the native server.
- User configuration always overrides discovery.
- Opt-in compile-on-save and explicit compile commands.
- Compiler output converted into source diagnostics.
- No compiler, game source, flags file, or generated `.pex` redistribution.

Linux and macOS compiler execution require separate Wine or compatibility-layer research and must not be required for native syntax diagnostics.

## Long-term debugging research

Debugging is a separate Debug Adapter Protocol concern, not an LSP feature. Its feasibility depends on a reliable runtime transport for each game. Any future debugger may reuse the source index and compiler configuration, but it requires an independent design and validation milestone.
