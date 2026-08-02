# Papyrus Language Server

An editor-neutral language server and canonical Tree-sitter grammar for Bethesda's Papyrus scripting language.

The project targets the Papyrus dialects used by Skyrim Anniversary Edition, Fallout 4, and Starfield. Its first milestone provides native, unsaved-buffer syntax diagnostics with human-readable structural errors such as a missing `EndIf`.

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

## Project documentation

- [Changelog](CHANGELOG.md)
- [Known issues and deferred work](KNOWN-ISSUES.md)
- [Testing and validation evidence](docs/TESTING.md)
- [Roadmap](docs/ROADMAP.md)
