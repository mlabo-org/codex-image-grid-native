# Codex Image Grid Native

Native-first successor project for Codex Image Grid.

This repository is a new Rust + SwiftUI implementation whose externally
observable behavior will match the frozen Electron baseline at:

`/Users/suzukimakoto/plugins/codex-image-grid`

The old repository remains the baseline and is not edited by this project.
This repository is not activated as a Codex runtime until the Rust server and
MCP binary implement the declared contract. The empty `.mcp.json` is
intentional during this preparation phase: an incomplete MCP route must not be
advertised as runnable.

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

Native and MCP callers pass local reference-image paths. A browser client, if
retained, uses a separate streamed/staged upload route.

## Current status

The provider-free first runnable slice now includes:

- reference-image validation and copied staging as
  `reference.png`, `reference.jpg`, or `reference.webp`;
- a native development server on `127.0.0.1:4322` with the baseline-compatible
  `GET /api/health` identity shape;
- stdio MCP JSONL handling for `initialize`, `ping`, `tools/list`, and
  `tools/call`, including the frozen `generate_image_grid` schema and
  validation errors;
- a responsive SwiftUI compatibility shell with the frozen language, theme,
  prompt, generation-option, reference-image, and result-filter controls.

Image generation, App Server launch/verification, run artifacts, and successful
`generate_image_grid` execution remain outside this slice. A valid generation
call therefore reports that execution is not implemented instead of falling
back to the Electron runtime. `.mcp.json` remains intentionally empty until
that launch/run boundary is implemented and smoke-validated.

## First checks

```bash
scripts/check.sh
```

The check script validates the Rust workspace and Swift package scaffold. It
also runs the provider-free health and MCP process smoke with an isolated
temporary native data root. It does not start a provider, launch or modify the
frozen Electron app, refresh plugin cache, connect `.mcp.json`, or write runtime
state into this repository.

## Runtime identity during migration

The native project must use a separate development identity, port, bundle id,
and data directory while the Electron baseline remains available. The final
cutover may take over the public `codex-image-grid` identity only after the
parity bundle passes and the old runtime is explicitly frozen.
