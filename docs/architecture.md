# Native Architecture

## Goal

Build a greenfield Rust + SwiftUI application that reproduces the observable
behavior of the frozen Image Grid baseline without porting its Node/Electron
internals.

The browser UI is not the primary product surface. The primary surface is the
SwiftUI macOS app. The local Rust runtime remains a compatibility boundary for
the Codex plugin and an optional browser client.

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
launch or verify the native runtime, pass local reference-image paths without
base64 encoding, and return the existing handoff/manifest/output contract.

## Reference-image policy

Native and MCP requests carry a local absolute path. The Rust server resolves
the path, validates the regular file, extension/MIME, and size, then copies it
into the run directory before starting jobs. Jobs use the staged copy, not a
mutable external path.

Browser clients cannot provide a real local path from a normal browser
sandbox. They use a separate binary or staged-upload route and never change
the native/MCP path contract.

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

