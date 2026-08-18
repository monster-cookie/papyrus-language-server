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

After successful workspace indexing, a semantic diagnostic layer reuses the same declaration and expression resolver as navigation. It validates role-classified identifier occurrences, complete call sites, and persisted statement-level type-check sites. In addition to reference and call errors, it checks assignments, initializers, returns, every unary and binary operator, casts, type tests, array access and construction, and `If`, `ElseIf`, and `While` conditions against Papyrus conversion rules. Ambiguous definitions produce diagnostics without selecting a target. Syntax-invalid documents and incomplete calls suppress derivative semantic cascades. Only open documents receive diagnostics, but every open overlay participates in resolution and triggers revalidation of the other open buffers.

### LSP transport

The server uses a synchronous stdio message loop with a separate cancellable workspace-indexing worker. It advertises UTF-16 positions and full-text synchronization and publishes diagnostics for open, changed, and saved buffers. Syntax diagnostics are available immediately; semantic diagnostics require a successfully completed workspace index. Closing a document clears its diagnostics. Invalid request parameters produce an LSP `InvalidParams` response, while malformed notifications are logged and ignored without terminating the session.

### Workspace and symbols

Initialization options select `auto`, `skyrim`, `fallout4`, or `starfield` and may provide project source roots and import directories. File-based LSP workspace folders become source roots when explicit roots are absent. `auto` is currently a safe unspecified value rather than dialect inference.

Initialization responds before discovery, archive extraction, or recursive scanning. The worker applies entry-count, file-count, file-size, total-byte, and depth budgets while recursively indexing `.psc` files. Clients can cancel it through LSP work-done progress. The foreground starts with an empty index that accepts open-document overlays; once the worker completes, the server replays current overlays and disk refreshes before atomically replacing the foreground index. Closing a document restores its current disk version or removes the entry if the backing file was deleted. Tree-sitter declaration nodes produce hierarchical document symbols and a flattened, case-insensitive workspace-symbol view. Duplicate names remain in the index for later semantic resolution.

The semantic index retains declared types, signatures, scopes, inheritance, source documentation, locations, syntax-backed identifier occurrences, and transient call sites for open buffers. Project sources have precedence over configured imports, which have precedence over discovered SDK sources. Duplicate candidates at the same precedence are ambiguous and deliberately do not produce hover, definition, reference, signature-help, or member-completion claims.

Workspace indexing orchestration lives in `indexing.rs`, bounded filesystem traversal in `workspace/scanning.rs`, completion in `workspace.rs`, and semantic diagnostic construction in `workspace/validation.rs`. Persisted statement-level checks are extracted in `semantic/type_checks.rs`, and Papyrus type compatibility and operator results live in `workspace/type_system.rs`. Shared declaration resolution, hover, definition, references, and signature help are isolated in `workspace/navigation.rs`, while rename validation and workspace-edit construction live in `workspace/rename.rs`. Find References uses a case-insensitive occurrence lookup to narrow candidates, then resolves each candidate to the selected declaration. It never treats textual matches in comments or strings as references. Results honor lexical scopes and imports, use the canonical navigation copy for identical aliases, and are sorted and deduplicated by URI and range. Rename converts those resolved project-source locations into non-overlapping text edits after validating the new Papyrus identifier and checking case-insensitive scope collisions, then uses a borrowed name-override resolver to require every changed reference to resolve to the renamed declaration without duplicating the workspace index. Configured imports and discovered SDK sources are never edited.

Script rename additionally requires LSP `documentChanges` and the `rename` resource operation. It emits versioned text-document edits followed by a `RenameFile` operation when the old and new script names share a namespace prefix, the source filename matches the old terminal script name, and the destination is either absent or demonstrably the same entry for a case-only rename. Distinct occupied paths and symlink destinations are rejected, while accepted case-only leaf-name changes preserve the requested destination URI casing. This keeps client application atomic without guessing namespace-directory moves. Signature help selects the innermost syntax-backed call at the cursor, maps positional or named arguments to the uniquely resolved declaration, and persists call-site metadata without persisting source text.

Semantic cache schema v9 persists declarations, parameter defaults, complete call and argument metadata, symbols, spanned expression trees, statement-level type-check sites, structured expression receivers, and role-classified identifier occurrences so navigation and diagnostics work on cache hits without retaining source text. Cache roots are selected from platform-standard private per-user locations with no shared-temporary fallback. Native path keys are encoded losslessly, and current content is hashed before a cached record is accepted. Writers use exclusive temporary files and uniquely named immutable generations, synchronize content before atomic publication, retain two owned generations, and never remove unrelated files.

On Windows with the Starfield dialect selected, Steam metadata locates app `2722710`. An installed `Data/Scripts/Source` tree is indexed directly when present; otherwise, reusable `Scripts/Source` entries from `Tools/ContentResources.zip` are extracted into a content-addressed staging directory and atomically published to the private platform cache. Archive size, entry count, retained-file count, per-file bytes, extracted bytes, and path depth are bounded. The direct and cached sources are alternatives rather than peers, preventing duplicate SDK definitions. Generated fragment paths and standard fragment-name prefixes are evaluated relative to each discovered root and excluded only from discovered SDK sources.

Windows extended-length drive and UNC paths are normalized before conversion to LSP file URIs so navigation locations remain consumable by editor clients. Inbound file URIs preserve non-local authorities as UNC paths on Windows and reject non-local authorities on Unix.

Standard output is reserved for protocol traffic. Fatal operational errors are written to standard error.

## Approved dependencies

| Dependency | Responsibility | License |
| --- | --- | --- |
| `blake3` | Semantic source fingerprinting | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception |
| `tree-sitter` | Incremental native parser runtime | MIT |
| `tree-sitter-language` | Version-independent generated grammar handle | MIT |
| `cc` | Compile the generated C parser during Rust builds | MIT OR Apache-2.0 |
| `lsp-server` | Synchronous JSON-RPC/LSP transport scaffold | MIT OR Apache-2.0 |
| `lsp-types` | Language Server Protocol data types | MIT |
| `serde` and `serde_json` | Protocol serialization and deserialization | MIT OR Apache-2.0 |
| `winreg` | Windows Steam installation discovery | MIT |
| `zip` | Starfield Creation Kit source archive extraction | MIT |
| `tree-sitter-cli` | Parser generation and native corpus tests | MIT |
| `web-tree-sitter` | Cross-platform WebAssembly fixture validation | MIT |

The Node packages are development-only. Exact resolved Rust and Node versions are recorded in `Cargo.lock` and `package-lock.json`. The `tree-sitter-cli` install script is approved only for its exact locked version so npm can install the required native executable.

## Deferred layers

Compiler discovery, automatic compilation, formatting, advanced control-flow analysis, and debugging remain deferred. Compiler integration must remain optional and must never redistribute Bethesda tools or source.
