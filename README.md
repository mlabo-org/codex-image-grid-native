# Codex Image Grid

Native Rust + SwiftUI implementation repository for the public
`codex-image-grid` plugin. The installable public plugin package lives at
`plugin/codex-image-grid/`; the repository root remains the implementation
workspace.

Codex routes English or Japanese image-generation requests, Prompt Batch,
thumbnails, and project, article, or video visuals through
`codex_image_grid/generate_image_grid`. The same public route is used when
CodexVideo or RelayPress requests visuals. Calling the tool
launches or joins the native runtime and automatically opens the SwiftUI app.
The live tool schema is authoritative for accepted inputs and returned
artifacts.

The separate Electron project that supplied the behavioral baseline is retired
and archived. It is not this repository's Git parent or prior checkout, and it
is never an implicit runtime fallback.

The Rust server, MCP binary, and SwiftUI primary path have passed the
provider-free and live App Server acceptance slices. The public plugin's
`.mcp.json` launches the MCP binary bundled inside the installed native app.

## Public and internal identity

- Public plugin and skill identity: `codex-image-grid`.
- Public MCP route: `codex_image_grid/generate_image_grid`.
- Public plugin source:
  `/Users/suzukimakoto/plugins/codex-image-grid-native/plugin/codex-image-grid`.
- Rust and Swift implementation source:
  `/Users/suzukimakoto/plugins/codex-image-grid-native`.
- Public runtime, health, manifest, and MCP server identity:
  `codex-image-grid`.
- Internal loopback port: `127.0.0.1:4322`.
- Compatible runtime data and image history:
  `~/Library/Application Support/codex-image-grid`.
- Installed app:
  `~/Applications/Codex Image Grid Native.app`.
- Internal bundle identifier and executable:
  `local.codex.image-grid.native` / `CodexImageGridNative`.

The nested public root lets its folder and manifest both use
`codex-image-grid` while this `codex-image-grid-native` repository remains a
separate project with its own Git history. The native port, bundle, executable,
and install identities remain isolated, while user-visible generated images,
manifests, handoffs, and restored history use the baseline-compatible runtime
data root. The separate retired Electron runtime must remain stopped so both
projects never write that shared runtime data root concurrently.

The detailed baseline contract is [docs/frozen-baseline-spec.md](docs/frozen-baseline-spec.md).
It records the frozen source commit, observable API/MCP/job/artifact behavior,
and the existing test evidence that the native implementation must reproduce.

## Target runtime

- `image-grid-core`: Rust domain model, validation, job state, retry policy,
  artifact contract, and reference-image staging.
- `image-grid-server`: Rust local runtime exposing the compatibility HTTP/SSE
  surface and Codex App Server JSON-RPC bridge.
- `image-grid-mcp`: Rust stdio MCP server used by the Codex plugin.
- `macos/`: SwiftUI native app. It owns the window, native file picker,
  display preferences, image grid, and lifecycle of the Rust runtime.

The SwiftUI app passes a validated local reference-image path to the Rust
runtime. The MCP tool also accepts a local absolute path, but snapshots the
file before startup and submits the frozen inline data-URL HTTP shape. Browser
clients use that same bounded inline HTTP contract. In every case, the Rust
runtime stages an owned copy in the run directory before starting jobs.

## Current status

The provider-free first runnable slice now includes:

- reference-image validation and copied staging as
  `reference.png`, `reference.jpg`, or `reference.webp`;
- an internal native server on `127.0.0.1:4322` with the baseline-compatible
  `GET /api/health` identity shape;
- deterministic Codex executable selection, an owned `codex app-server`
  JSONL child, and compatible `GET`/`POST
  `/api/preflight/app-server-image` diagnostics;
- compatible `/api/run`, `/api/run-batch`, `/api/runs`, `/events`, generated
  file, manifest, handoff, and safe artifact-view routes for
  `app-server-image`;
- a separate concurrency-one `codex-svg` App Server path that writes only to
  its exact native run output and preserves the same job/artifact lifecycle;
- the frozen `queued → starting → running → done|error` primary job state,
  global 24-slot image scheduler, exact output naming, prompt construction,
  stable App Server image notifications, atomic image/artifact writes, and
  attempt-scoped timeout/rate-limit/missing-output recovery;
- compatible `POST /api/analyze-reference` staging, ephemeral read-only App
  Server analysis, bounded JSON input, and cleanup;
- stdio MCP JSONL handling for `initialize`, `ping`, `tools/list`, and
  `tools/call`, including the frozen `generate_image_grid` schema and
  validation errors, bounded native-server launch/join, health and App Server
  preflight checks, and compatible success summaries/structured content;
- a responsive SwiftUI runtime client with the frozen language, theme, prompt,
  generation-option, reference-image, and result-filter controls, plus
  health/preflight/run calls, SSE progress, validated/downscaled
  choose/drop/paste references, reference analysis states, native file
  actions, and adaptive result cards;
- persisted draft/reference restoration, bounded long-session result
  retention, restart-safe run restoration, Finder-compatible host routes, and
  graceful owned-runtime shutdown with joined-runtime preservation.

The provider-free fixture now completes real HTTP and MCP runs through the
owned App Server transport and validates generated images, local reference
copy staging, compatible MCP handoff fields, run responses, manifests,
history, artifact routes, bounded recovery, and reference analysis.
The live acceptance slice completed one real MCP-launched App Server image run,
then restored that run in the SwiftUI app. It also exercised native
choose/clear reference handling, language/theme and engine switches,
single/batch prompts, flexible two-column and narrow one-column layouts, and
owned/joined runtime shutdown.

## Build and checks

```bash
scripts/check.sh
scripts/install-native-app.sh
scripts/install-native-app.sh --execute
```

The check script validates the Rust workspace and Swift package scaffold. It
also runs the provider-free health, fake-App-Server preflight, reference
analysis, complete one-image run/artifact, and MCP process smoke with an
isolated temporary native data root. It does not start a provider, launch or
modify the separate retired Electron app, refresh plugin cache, activate the plugin, or
write runtime state into this repository.

The installer is dry-run by default. `--execute` builds the Rust and Swift
release products, assembles and ad-hoc signs the native app, verifies it, and
atomically installs it at the path above. The public plugin's `.mcp.json`
executes the MCP binary inside that bundle; a valid tool call opens or
re-activates the app before joining its SwiftUI-owned runtime. Runtime data
remains under the isolated native data directory above.
