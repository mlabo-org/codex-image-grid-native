# Codex Image Grid Native Architecture

## Goal

Provide the Rust + SwiftUI implementation behind the public
`codex-image-grid` generation route while preserving the observable Image Grid
contract established by the frozen baseline.

The browser UI is not the primary product surface. The primary surface is the
SwiftUI macOS app. The local Rust runtime remains a compatibility boundary for
the Codex plugin and an optional browser client.

## Public and isolation identities

The public plugin and skill identity is `codex-image-grid`, and its MCP route is
`codex_image_grid/generate_image_grid`. Generic image generation/画像生成,
Prompt Batch, thumbnail, project, article, and video-visual requests use this
route, including requests originating in CodexVideo, Agentic StructCiv, and
RelayPress. The route automatically opens the native SwiftUI app.

The implementation keeps separate internal identities:

- public plugin root:
  `/Users/suzukimakoto/plugins/codex-image-grid-native/plugin/codex-image-grid`;
- implementation workspace:
  `/Users/suzukimakoto/plugins/codex-image-grid-native`;
- public runtime, health, manifest, and MCP server identity:
  `codex-image-grid`;
- loopback endpoint: `127.0.0.1:4322`;
- data root: `~/Library/Application Support/codex-image-grid-native`;
- installed app: `~/Applications/Codex Image Grid Native.app`;
- bundle identifier and Swift executable:
  `local.codex.image-grid.native` / `CodexImageGridNative`.

The nested public root gives the plugin a folder matching its public manifest
name without taking over the frozen repository path. The isolated port, data
root, bundle identifier, executable, and install path prevent process and state
collisions; they are not alternate public plugin names. The old Electron app
and repository are frozen, out of scope for runtime dispatch, and never a
fallback.

## Ownership

### SwiftUI app

SwiftUI owns:

- windows, menus, commands, and application lifecycle;
- native file selection and Finder actions;
- image-grid presentation and progress display;
- Japanese / English / System language selection;
- Light / Dark / System theme selection;
- persisted display preferences;
- starting, stopping, and health-checking the Rust runtime.

SwiftUI does not own generation scheduling, retry policy, artifact naming, or
Codex App Server protocol semantics.

### Rust core

Rust owns:

- request validation and the shared public contract;
- job and run state machines;
- global concurrency, retries, timeouts, cooldowns, and diagnostics;
- reference-image validation and staging;
- manifest, handoff, and generated-file paths;
- Codex App Server JSON-RPC transport and notification routing.

### Rust server

The server owns the local runtime boundary:

- loopback HTTP API compatible with the frozen baseline;
- SSE event stream for the web compatibility client;
- health and preflight identity;
- runtime data directory and graceful shutdown;
- optional Unix-domain-socket/native transport when that reduces UI coupling.

### Rust MCP

The MCP binary owns stdio JSON-RPC and the `generate_image_grid` tool. It must
validate and snapshot a requested local reference-image path before any server
startup, convert the snapshot to the frozen inline data-URL HTTP shape, open or
re-activate the exact installed native app, verify its SwiftUI-owned runtime,
and return the existing handoff/manifest/output contract. The active public
plugin route may not start the Rust server headlessly or fall back to Electron.

The tool's live description and input schema are authoritative for routing
fields, accepted inputs, limits, defaults, and artifact validity. Plugin and
skill prose describe ownership and dispatch only; they do not redefine that
contract.

## Reference-image policy

The SwiftUI app may send a validated local absolute path as a native extension.
The MCP public argument is also a local absolute path, but the MCP binary
validates and reads it before health/startup, then submits an inline
`referenceImage` data URL to `/api/run-batch`. Browser clients submit the same
bounded inline HTTP shape. The Rust server stages either accepted form once in
the run directory, and jobs use that owned copy rather than a mutable external
path.

## Compatibility boundary

The following are public behavior and must remain stable at cutover:

- MCP tool name, input schema, error meaning, and returned Markdown;
- `/api/run`, `/api/run-batch`, `/api/runs`, `/api/health`, preflight, and
  artifact routes;
- SSE event names and payload meaning;
- `app-server-image` and `codex-svg` engines;
- maximums: 12 prompts, 6 variants per prompt, 24 total jobs;
- retry, timeout, rate-limit, missing-output, and diagnostic semantics;
- manifest, handoff, generated image paths, and display-safe URLs.

The exact image bytes are not a parity target because the upstream provider is
stochastic. Parity means equivalent accepted inputs, state transitions,
failure semantics, artifacts, and user-visible results.
