# Architecture

## Repository responsibility

This repository is the single source of truth for the Papyrus parse grammar and editor-neutral language analysis. The grammar is stored under `grammar/` using the conventional Tree-sitter layout and exposes both generated C sources and a Rust binding.

Editor integrations consume an immutable revision of this repository. They may retain editor-specific metadata and Tree-sitter query files, but they must not maintain a second Papyrus grammar implementation.

## Components

### Canonical grammar

`tree-sitter-papyrus` recognizes the Papyrus dialects used by Skyrim Anniversary Edition, Fallout 4, and Starfield. The generated parser targets Tree-sitter ABI 15.

The Rust language-server crate links the generated parser directly. Zed and other consumers can build the same grammar from `grammar/` at a pinned repository revision.

### Native diagnostics

The first diagnostic layer traverses Tree-sitter `ERROR` and `MISSING` nodes. Tree-sitter recovery can identify malformed syntax but does not always explain an omitted block closer precisely.

A second, deliberately narrow structural validator tracks Papyrus block pairs while ignoring strings, line comments, documentation comments, and slash-delimited block comments. When an outer closer conflicts with an open inner block, it reports the missing inner keyword at the conflicting closer. For example:

```papyrus
Function Run()
    If True
EndFunction
```

produces `Missing EndIf before EndFunction` at `EndFunction`.

### LSP transport

The server uses a synchronous stdio message loop. It advertises UTF-16 positions and full-text synchronization and publishes diagnostics for open, changed, and saved buffers. Closing a document clears its diagnostics.

### Workspace and symbols

Initialization options select `auto`, `skyrim`, `fallout4`, or `starfield` and may provide project source roots and import directories. File-based LSP workspace folders become source roots when explicit roots are absent. `auto` is currently a safe unspecified value rather than dialect inference.

The server recursively indexes `.psc` files under those roots in memory. Open documents overlay their disk-backed entries so symbol requests reflect unsaved text; closing a document restores its disk version. Tree-sitter declaration nodes produce hierarchical document symbols and a flattened, case-insensitive workspace-symbol view. Duplicate names remain in the index for later semantic resolution.

Standard output is reserved for protocol traffic. Fatal operational errors are written to standard error.

## Approved dependencies

| Dependency | Responsibility | License |
| --- | --- | --- |
| `tree-sitter` | Incremental native parser runtime | MIT |
| `tree-sitter-language` | Version-independent generated grammar handle | MIT |
| `cc` | Compile the generated C parser during Rust builds | MIT OR Apache-2.0 |
| `lsp-server` | Synchronous JSON-RPC/LSP transport scaffold | MIT OR Apache-2.0 |
| `lsp-types` | Language Server Protocol data types | MIT |
| `serde` and `serde_json` | Protocol serialization and deserialization | MIT OR Apache-2.0 |
| `tree-sitter-cli` | Parser generation and native corpus tests | MIT |
| `web-tree-sitter` | Cross-platform WebAssembly fixture validation | MIT |

The Node packages are development-only. Exact resolved Rust and Node versions are recorded in `Cargo.lock` and `package-lock.json`.

## Deferred layers

Symbol resolution, completion, navigation, semantic checking, compiler discovery, automatic compilation, and debugging remain deferred. Compiler integration must remain optional and must never redistribute Bethesda tools or source.
