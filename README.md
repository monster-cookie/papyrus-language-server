# Papyrus Language Server

An editor-neutral language server and canonical Tree-sitter grammar for Bethesda's Papyrus scripting language.

The project targets the Papyrus dialects used by Skyrim Anniversary Edition, Fallout 4, and Starfield. It provides native syntax diagnostics, document symbols, and an in-memory workspace symbol index.

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

`dialect` accepts `auto`, `skyrim`, `fallout4`, or `starfield` and defaults to `auto`. When `sourceRoots` is omitted, the server indexes the file-based LSP workspace folders. Import directories are indexed alongside project roots. The current milestone records the dialect for later semantic work; `auto` does not yet infer a dialect.

## Project documentation

- [Changelog](CHANGELOG.md)
- [Known issues and deferred work](KNOWN-ISSUES.md)
- [Testing and validation evidence](docs/TESTING.md)
- [Roadmap](docs/ROADMAP.md)
