# Development Contract

This repository is the source of truth for the public `codex-image-grid`
plugin's native implementation. Its installable plugin package is rooted at
`plugin/codex-image-grid/`, where the folder and manifest identity match. The
parent plugin contract at `/Users/suzukimakoto/plugins/AGENTS.md` applies.

The frozen behavior target is recorded in
`docs/frozen-baseline-spec.md`. That file is an evidence-backed contract
index; executable Rust/Swift tests and protocol fixtures remain the validators.

## Source boundaries

- `plugin/codex-image-grid/` is the source of truth for public plugin
  registration and skill discovery.
- This repository root is the source of truth for the native Rust and Swift
  implementation.
- The frozen baseline at `/Users/suzukimakoto/plugins/codex-image-grid` is a
  read-only historical behavioral reference, not an active runtime route.
- Codex plugin cache, generated images, run manifests, logs, and build output
  are not source.
- No runtime state is stored in this repository.

## Registration and isolation boundary

- Public plugin and skill name: `codex-image-grid`.
- Public MCP route: `codex_image_grid/generate_image_grid`.
- Public plugin source:
  `/Users/suzukimakoto/plugins/codex-image-grid-native/plugin/codex-image-grid`.
- Native implementation workspace:
  `/Users/suzukimakoto/plugins/codex-image-grid-native`.
- Public runtime, health, manifest, and MCP server identity:
  `codex-image-grid`.
- Compatible generated-image and history root:
  `~/Library/Application Support/codex-image-grid`.
- Internal loopback endpoint: `127.0.0.1:4322`.
- Installed app: `~/Applications/Codex Image Grid Native.app`.
- Internal bundle identifier and executable:
  `local.codex.image-grid.native` / `CodexImageGridNative`.

Source edits, cache refresh, installation, and active-session pickup are
separate boundaries. Cache or installed copies are not edited in place. The
old Electron plugin is frozen and out of scope; no registration or failure
path may dispatch to it.

The frozen Electron runtime must remain stopped because rollback and Native
share the original user-visible data root and may not write it concurrently.

## Parent-orchestrated work units

Each future work unit must have one owner and one contract-complete handoff:

| Work unit | Source scope | Output | Minimum validation | Stop condition |
| --- | --- | --- | --- | --- |
| Contract | `docs/`, `crates/image-grid-core` | behavior/schema decision | focused Rust tests | baseline behavior is ambiguous |
| Core | `crates/image-grid-core` | typed state/validation implementation | affected `cargo test` | contract input is missing |
| Runtime | `crates/image-grid-server` | runnable local server | server smoke + affected Rust tests | Codex RPC contract is unresolved |
| MCP | `crates/image-grid-mcp`, `plugin/codex-image-grid/.mcp.json` | stdio tool and installed-app launch route | initialize/list/call smoke | binary or launch identity is unavailable |
| UI | `macos/` | SwiftUI native surface | `swift test` and representative app build | Rust runtime boundary is not stable |
| Parity | `docs/parity/`, tests | machine-readable comparison receipt | old/new external-contract comparison | a declared behavior differs |

The parent keeps the global acceptance decision. A worker returns changed
paths, artifact paths, validation results, blocker, and remaining unknowns;
it does not silently expand scope or activate the plugin.

## Release acceptance bundle

The native release acceptance bundle covers:

1. Rust core contract tests;
2. local server health and primary run smoke;
3. MCP initialize/list/call smoke;
4. SwiftUI launch and native reference-file selection;
5. parity of status, artifacts, diagnostics, and error semantics against the
   frozen baseline.

The public plugin `.mcp.json` executes the MCP binary inside the verified
installed app. A valid call opens or re-activates that exact app, requires its
packaged `0.2.4` SwiftUI-owned health identity on the isolated port, and never
uses the frozen Electron or a headless server fallback.
