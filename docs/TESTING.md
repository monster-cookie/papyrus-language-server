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
- initialize, initialized, shutdown, and exit complete successfully over an in-memory LSP connection.

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

The ignored `installed_source_audit` test reads a source root from `PAPYRUS_AUDIT_ROOT`; the path is never stored in the repository. A small number of legacy Skyrim and Fallout 4 files are not valid UTF-8, so the audit decodes input lossily to model the Unicode buffer that an LSP client sends to the server. No installed source content is copied or retained.
