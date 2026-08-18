# Papyrus Language Server

An editor-neutral language server and canonical Tree-sitter grammar for Bethesda's Papyrus scripting language.

The project targets the Papyrus dialects used by Skyrim Anniversary Edition, Fallout 4, and Starfield. It provides native syntax and conservative semantic diagnostics, source-derived completion, hover, go to definition, find references, rename, signature help, document symbols, and a workspace symbol index.

No Bethesda compiler, flags file, or game source is distributed by this repository. The committed fixtures are original synthetic examples.

## Development

Requirements:

- Rust installed through `rustup`;
- `cargo-audit` 0.22.2 for the dependency security check;
- Node.js and npm;
- a native C compiler supported by the Rust toolchain.

Run the complete local validation suite from the repository root:

```powershell
npm ci
npm run grammar:generate
npm run grammar:build
npm run grammar:test
npm run grammar:test:native
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo audit
```

The language server communicates over standard input and standard output:

```powershell
cargo run --package papyrus-language-server
```

Protocol output is reserved for LSP messages. Operational errors are written to standard error.

## Workspace configuration

Editors may supply settings through `initializationOptions.papyrus`:

```json
{
  "papyrus": {
    "dialect": "auto",
    "sourceRoots": ["C:\\Projects\\MyMod\\Scripts\\Source"],
    "importDirectories": ["C:\\Games\\Example\\Data\\Scripts\\Source"]
  }
}
```

`dialect` accepts `auto`, `skyrim`, `fallout4`, or `starfield` and defaults to `auto`. When `sourceRoots` is omitted, the server indexes the file-based LSP workspace folders. Import directories are indexed alongside project roots. The selected dialect controls dialect-specific source discovery; `auto` does not infer a dialect or enable dialect-specific discovery.

When `dialect` is `starfield`, the server also discovers Steam's Starfield Creation Kit installation. It indexes an installed `Data/Scripts/Source` tree when present; otherwise, it extracts reusable `.psc` files from `Tools/ContentResources.zip` into the platform's private per-user cache. Automatically discovered sources exclude generated `Fragments` and `QF_`, `PF_`, `TIF_`, and `SF_` scripts. Installed or cached source remains local and provides navigable definitions; project files are never filtered.

Initialization completes before source discovery, archive extraction, or recursive indexing begins. Those operations run on a cancellable background worker with file-count, depth, individual-file, total-byte, archive-entry, and extraction-byte limits. Clients that advertise LSP work-done progress receive indexing status. Requests that require the complete workspace index wait for the worker; syntax diagnostics and document symbols remain available for open buffers while it runs. When the completed index is published, current open-buffer overlays and disk changes observed during indexing are replayed before semantic diagnostics become available.

IntelliSense is deliberately conservative. Member completion is returned only when the receiver's declared type resolves uniquely, including inherited members. Hover, definition, references, rename, and signature help use the same indexed declaration and never synthesize missing types, APIs, or documentation. Find References searches syntax-backed identifiers across the workspace, excludes comments and strings, respects local scopes and source precedence, and returns no claim for ambiguous or unsupported expressions. Rename reuses those resolved locations, validates Papyrus identifiers and case-insensitive collisions, and edits project source roots only; configured imports and discovered SDK sources remain read-only. Signature help tracks positional and named arguments, including incomplete and nested calls, and suppresses unresolved or ambiguous callees.

Semantic diagnostics reuse that resolution layer for open documents. They report unresolved and ambiguous references, types, and members; invalid call targets and argument binding; incompatible assignments, initializers, and returns; invalid unary, binary, compound-assignment, cast, type-test, array, and basic control-condition expressions; and value-return mistakes. Papyrus implicit conversions, including `Int` to `Float`, values to `String` or `Bool`, and child scripts to parent types, are honored. Syntax-invalid documents and incomplete calls suppress derivative semantic cascades, while navigation continues to decline ambiguous targets rather than choosing one. Defaulted parameters are optional. Changes to any open overlay revalidate every open document because one buffer can supply declarations used by another.

Renaming a project script updates its declaration and resolved references and, when the editor advertises LSP rename-file support, renames the matching `.psc` file in the same directory. The namespace prefix must remain unchanged, the current filename must match the script's terminal name, and the destination must not already exist. Case-only filename changes are supported. Namespace-directory moves are deliberately not inferred. Before returning any rename, the server builds a post-edit semantic view and verifies that every edited reference still resolves uniquely to the renamed declaration. Versioned workspace edits carry the current version of each open document.

Imported scripts contribute their declared structs and `Global` functions to completion, hover, go to definition, references, and signature help. Script-qualified global calls such as `Game.GetPlayer()` resolve against the indexed script source. Top-level `Const` values and `Global` functions are excluded from instance-member completion.

## Semantic index cache

The server fingerprints normalized source text with BLAKE3. Scripts with the same case-insensitive script name and content fingerprint share one semantic identity even when projects contain identical `Papyrus` and `Staging` copies. A same-name script with different contents remains ambiguous; the server does not silently choose one implementation.

Parsed declarations, parameter-default metadata, complete call and argument ranges, spanned expression trees and type-check sites, expression receivers, and role-classified syntax-backed identifier occurrences are persisted in immutable schema-v10 generations under the platform cache directory:

- Windows: `%LOCALAPPDATA%\papyrus-language-server\cache`
- Linux: `${XDG_CACHE_HOME:-$HOME/.cache}/papyrus-language-server/cache`
- macOS: `$HOME/Library/Caches/papyrus-language-server/cache`

There is no shared temporary-directory fallback; if a private per-user cache cannot be established, persistence is disabled for that session. Source text itself is not duplicated in the cache. Cache keys preserve the platform's native path representation, and a hit requires the current source-content fingerprint in addition to path, size, modification time, and schema. New generations are written exclusively, synchronized, and atomically published without replacing an existing cache file; only the two newest owned generations are retained. The cache contains no authoritative project data and can be deleted safely to force a complete rebuild.

Startup logs report indexed files, cache hits and misses, identical aliases, and elapsed indexing time. Language-server requests taking at least 250 ms are also reported to standard error.

## Project documentation

- [Changelog](CHANGELOG.md)
- [Known issues and deferred work](KNOWN-ISSUES.md)
- [Testing and validation evidence](docs/TESTING.md)
- [Roadmap](docs/ROADMAP.md)
