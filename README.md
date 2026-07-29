# Codex Image Grid Native

Native-first successor project for Codex Image Grid.

This repository is a new Rust + SwiftUI implementation whose externally
observable behavior will match the frozen Electron baseline at:

`/Users/suzukimakoto/plugins/codex-image-grid`

The old repository remains the baseline and is not edited by this project.
The Rust server, MCP binary, and SwiftUI primary path have passed the
provider-free and live App Server acceptance slices. `.mcp.json` now connects
the validated native release binaries on the separate development port 4322.

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
- a native development server on `127.0.0.1:4322` with the baseline-compatible
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
owned/joined runtime shutdown. `.mcp.json` is connected only after those checks.

## Build and checks

```bash
scripts/check.sh
cargo build --release --workspace
swift build -c release --package-path macos
```

The check script validates the Rust workspace and Swift package scaffold. It
also runs the provider-free health, fake-App-Server preflight, reference
analysis, complete one-image run/artifact, and MCP process smoke with an
isolated temporary native data root. It does not start a provider, launch or
modify the frozen Electron app, refresh plugin cache, connect `.mcp.json`, or
write runtime state into this repository.

The release builds materialize the binaries referenced by `.mcp.json` and the
native app executable. Runtime data remains under
`~/Library/Application Support/codex-image-grid-native`.

## Runtime identity during migration

The native project must use a separate development identity, port, bundle id,
and data directory while the Electron baseline remains available. The final
cutover may take over the public `codex-image-grid` identity only after the
parity bundle passes and the old runtime is explicitly frozen.
