# Papyrus Language Server

An editor-neutral language server and canonical Tree-sitter grammar for Bethesda's Papyrus scripting language.

The project targets the Papyrus dialects used by Skyrim Anniversary Edition, Fallout 4, and Starfield. It provides native syntax diagnostics, source-derived completion, hover, go to definition, document symbols, and a workspace symbol index.

No Bethesda compiler, flags file, or game source is distributed by this repository. The committed fixtures are original synthetic examples.

## Development

Requirements:

- Rust installed through `rustup`;
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

When `dialect` is `starfield`, the server also discovers Steam's Starfield Creation Kit installation. It extracts reusable `.psc` files from `Tools/ContentResources.zip` into `%LOCALAPPDATA%\papyrus-language-server\cache`, excluding generated `Fragments` and `QF_`, `PF_`, `TIF_`, and `SF_` scripts. Cached source remains local and provides navigable definitions; project files are never filtered.

IntelliSense is deliberately conservative. Member completion is returned only when the receiver's declared type resolves uniquely, including inherited members. Hover and definition use the same indexed declaration and never synthesize missing types, APIs, or documentation.

Imported scripts contribute their declared structs and `Global` functions to completion, hover, and go to definition. Top-level `Const` values and `Global` functions are excluded from instance-member completion.

## Semantic index cache

The server fingerprints normalized source text with BLAKE3. Scripts with the same case-insensitive script name and content fingerprint share one semantic identity even when projects contain identical `Papyrus` and `Staging` copies. A same-name script with different contents remains ambiguous; the server does not silently choose one implementation.

Parsed declarations are persisted in `%LOCALAPPDATA%\papyrus-language-server\cache\semantic-index-v3.json`; source text itself is not duplicated in the cache. Unchanged files are restored by path, size, modification time, cache schema, and content fingerprint. Editing a file rebuilds that entry, while changing the semantic schema invalidates the cache automatically. The cache contains no authoritative project data and can be deleted safely to force a complete rebuild.

Startup logs report indexed files, cache hits and misses, identical aliases, and elapsed indexing time. Language-server requests taking at least 250 ms are also reported to standard error.

## Project documentation

- [Changelog](CHANGELOG.md)
- [Known issues and deferred work](KNOWN-ISSUES.md)
- [Testing and validation evidence](docs/TESTING.md)
- [Roadmap](docs/ROADMAP.md)
