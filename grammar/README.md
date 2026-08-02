# Tree-sitter Papyrus grammar

This directory contains the canonical Tree-sitter grammar used by the Papyrus language server and editor integrations. It is a custom implementation and does not derive from or incorporate another Papyrus grammar.

The grammar targets Tree-sitter ABI 15 and recognizes the core modern Papyrus structures used by Starfield, including:

- scripts, imports, variables, properties, groups, structs, custom events, functions, events, and states;
- arrays, qualified struct types, member access, calls, casts, type tests, and `new` expressions;
- `Guard`, `RequiresGuard`, `LockGuard`, `TryLockGuard`, `ElseTryLockGuard`, and related block endings;
- Papyrus comments, documentation comments, and line continuations.

Tree-sitter provides tolerant structural parsing for editor features and the language server's native syntax diagnostics. Semantic validation, symbol resolution, and optional compiler diagnostics are separate language-server responsibilities.

## Development

From the repository root:

```powershell
npm install
npm run grammar:generate
npm run grammar:build
npm run grammar:test
```

`grammar:generate` refreshes the committed ABI-15 C parser and node metadata. `grammar:build` creates the ignored `grammar/papyrus.wasm` artifact. `grammar:test` loads that artifact with `web-tree-sitter`, requires valid Starfield, Skyrim, and Fallout 4 fixtures to parse without errors, requires invalid fixtures to produce their expected syntax issue, and checks coverage of the major syntax nodes.

The conventional corpus is under `test/corpus`. `npm run grammar:test:native` runs it when a native C compiler is available.
