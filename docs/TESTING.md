# Testing

## Synthetic fixtures

The committed fixtures are original project examples. They exercise representative valid and invalid syntax for Skyrim Anniversary Edition, Fallout 4, and Starfield without redistributing Bethesda source.

The grammar test requires every valid fixture to parse without `ERROR` or `MISSING` nodes. Each invalid fixture must contain its expected syntax issue. Focused native corpus tests assert exact trees for declarations, comments, expressions, statements, guards, dialect constructs, and recovery cases.

## Language-server acceptance

Rust tests verify:

- the generated grammar loads through its Rust binding;
- valid cross-dialect fixtures produce no diagnostics;
- invalid fixtures name their missing `EndIf`, `EndState`, or `EndStruct`;
- keywords in comments and strings do not affect block matching;
- UTF-8 byte offsets convert to UTF-16 LSP positions;
- diagnostics are published for an unsaved open buffer;
- a full-text change that inserts the missing closer clears the diagnostic;
- recursive workspace indexing accepts case-insensitive `.psc` extensions;
- document and workspace symbols reflect unsaved text;
- closing an overlaid document restores its disk-backed symbols;
- inheritance-aware member completion excludes unresolved and ambiguous receivers;
- instance completion excludes `Const` and conventional `CONST_` members while retaining ordinary `AutoReadOnly` properties;
- explicitly written constant-style members still resolve for hover and go to definition;
- hover and go to definition resolve the same source-derived declaration;
- signature help maps positional and named arguments to inherited, imported-global, and script-qualified declarations;
- nested and incomplete calls select the correct signature and active parameter without guessing ambiguous callees;
- script-qualified global calls resolve their script and global function while preserving variable shadowing and project-source precedence;
- find references honors declaration inclusion, local scopes, parameters, inherited members, imported globals and structs, and conservative ambiguity handling;
- reference extraction excludes declaration names, named-argument labels, comments, and strings;
- unsaved overlays rebuild references, closing restores disk-backed occurrences, and cached occurrences work without retained source text;
- identical source aliases contribute references only from the canonical navigation copy;
- rename reuses resolved declarations and references for scoped symbols, inherited members, imported globals and structs, and unsaved overlays while excluding comments and strings;
- rename rejects invalid or reserved identifiers, case-insensitive scope collisions, ambiguous targets, and declarations outside project source roots;
- script rename emits ordered text edits and a same-namespace `RenameFile`, rejects clients without file-operation support, and refuses namespace changes, mismatched filenames, and existing destinations;
- Windows extended-length drive and UNC paths normalize before conversion to LSP file URIs;
- Steam manifest discovery, installed-source preference, ZIP cache fallback, and discovered-source filtering use synthetic temporary fixtures;
- generated fragment filters retain reusable quest and perk base scripts;
- initialize advertises completion, hover, definition, references, rename with prepare support, and signature help; those requests plus initialized, shutdown, and exit complete successfully over an in-memory LSP connection;
- the in-memory LSP connection returns multi-document rename edits and a script `RenameFile` operation when the client advertises the required workspace-edit capabilities.

## Downstream Zed acceptance

Results recorded on Windows x64 on 2026-08-02 using a locally built release executable configured in Zed:

| Check | Result | Evidence |
| --- | --- | --- |
| Valid Starfield fixtures | Pass | The Basic and Advanced fixtures opened with no diagnostics. |
| Valid Skyrim fixtures | Pass | The Basic and Advanced fixtures opened with no diagnostics. |
| Valid Fallout 4 fixtures | Pass | The Basic and Advanced fixtures opened with no diagnostics. |
| Missing Starfield closer | Pass | The invalid fixture reported `Missing EndIf before EndFunction` at `EndFunction`. |
| Missing Skyrim closer | Pass | The invalid fixture reported `Missing EndState`. |
| Missing Fallout 4 closer | Pass | The invalid fixture reported `Missing EndStruct`. |
| Unsaved-buffer updates | Pass | Inserting `EndIf` cleared the diagnostic immediately, and undoing the insertion restored it. |
| Zed presentation | Pass | The editor underline, hover message, and diagnostics view displayed the diagnostic. |
| Process restart | Pass | Zed relaunched the configured server and diagnostics remained functional after restart. |

This validates the local executable path and protocol behavior through a real editor client. Automatic download and launch from the first published release remain a separate downstream acceptance check.

## Local commands

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

Successful grammar generation is not evidence that the Rust server compiled. Successful Rust compilation is not evidence that Zed downloaded or launched a release. Each layer is validated independently.

## Continuous integration

Pull requests targeting `master` and pushes to `master` run the grammar and Rust validation suite on Ubuntu. Tag pushes matching `v*.*.*` build native Windows x64, Linux x64, macOS Intel, and macOS ARM64 archives. The release workflow publishes SHA-256 files beside every archive.

Installed game-source audits remain local release checks. Their source files must not be copied into the repository or CI artifacts.

## Installed-source audit results

Local recursive audits completed on Windows x64 on 2026-08-01:

| Source tree | Files | Result |
| --- | ---: | --- |
| Starfield, including `Fragments` | 5,086 | No diagnostics |
| Skyrim Anniversary Edition | 14,301 | No diagnostics |
| Fallout 4 extracted source tree | 10,689 | No diagnostics |

The ignored `installed_source_audit` and `installed_source_navigation_audit` tests read a source root from `PAPYRUS_AUDIT_ROOT`; the path is never stored in the repository. The navigation audit verifies that a script-qualified `Game.GetPlayer()` call resolves both the script and global function to the installed `Game.psc`. A small number of legacy Skyrim and Fallout 4 files are not valid UTF-8, so the diagnostics audit decodes input lossily to model the Unicode buffer that an LSP client sends to the server. No installed source content is copied or retained.
