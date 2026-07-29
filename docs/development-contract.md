# Development Contract

This project is prepared as a Codex plugin source repository. The parent
plugin contract at `/Users/suzukimakoto/plugins/AGENTS.md` applies.

The frozen behavior target is recorded in
`docs/frozen-baseline-spec.md`. That file is an evidence-backed contract
index; executable Rust/Swift tests and protocol fixtures remain the validators.

## Source boundaries

- This repository is the source of truth for the native plugin and app.
- The frozen baseline at `/Users/suzukimakoto/plugins/codex-image-grid` is a
  read-only behavioral reference during migration.
- Codex plugin cache, generated images, run manifests, logs, and build output
  are not source.
- No runtime state is stored in this repository.

## Parent-orchestrated work units

Each future work unit must have one owner and one contract-complete handoff:

| Work unit | Source scope | Output | Minimum validation | Stop condition |
| --- | --- | --- | --- | --- |
| Contract | `docs/`, `crates/image-grid-core` | behavior/schema decision | focused Rust tests | baseline behavior is ambiguous |
| Core | `crates/image-grid-core` | typed state/validation implementation | affected `cargo test` | contract input is missing |
| Runtime | `crates/image-grid-server` | runnable local server | server smoke + affected Rust tests | Codex RPC contract is unresolved |
| MCP | `crates/image-grid-mcp`, `.mcp.json` | stdio tool and launch route | initialize/list/call smoke | binary or launch identity is unavailable |
| UI | `macos/` | SwiftUI native surface | `swift test` and representative app build | Rust runtime boundary is not stable |
| Parity | `docs/parity/`, tests | machine-readable comparison receipt | old/new external-contract comparison | a declared behavior differs |

The parent keeps the global acceptance decision. A worker returns changed
paths, artifact paths, validation results, blocker, and remaining unknowns;
it does not silently expand scope or activate the plugin.

## Acceptance bundle for the first runnable release

The first runnable native release must contain one semantic bundle covering:

1. Rust core contract tests;
2. local server health and primary run smoke;
3. MCP initialize/list/call smoke;
4. SwiftUI launch and native reference-file selection;
5. parity of status, artifacts, diagnostics, and error semantics against the
   frozen baseline.

The plugin `.mcp.json` remains intentionally unconnected until the MCP binary
and launch route pass the first runnable slice.
