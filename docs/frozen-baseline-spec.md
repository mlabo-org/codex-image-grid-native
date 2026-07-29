# Frozen Image Grid Behavioral Specification

Status: observed baseline for the native replacement.

Evidence snapshot:

- Repository: `/Users/suzukimakoto/plugins/codex-image-grid`
- Git commit: `b92af946c1ceca3d4826406c0b63305cbcfb02bb`
- Branch: `codex/image-grid-app-server-queue-diagnostics`
- Observed: 2026-07-29

This document fixes externally observable behavior for the Rust + SwiftUI
replacement. It does not require preserving Node modules, Electron internals,
DOM structure, or accidental implementation details. The upstream image
provider is stochastic, so byte-for-byte image equality is not a parity
requirement.

## 1. Product identity and runtime modes

The baseline product identity is `codex-image-grid`, package version `0.1.0`.
It has three related routes:

1. Web server: serves the local UI and the HTTP/SSE runtime.
2. Electron desktop: owns an embedded server, verifies its health identity,
   starts App Server image preflight, and stops its owned server on exit.
3. Codex plugin: exposes one stdio MCP tool and launches or verifies the local
   server before submitting a run.

The native replacement makes SwiftUI the primary route. The local Rust runtime
remains the shared execution boundary. A browser client is compatibility-only.

During migration, the native runtime must use a separate port, bundle/app
identity, and data directory. It must not attach to or silently fall back to
the frozen Electron runtime.

## 2. Shared generation contract

The baseline executable contract is:

| Field | Values |
| --- | --- |
| `maxPrompts` | 12 |
| `maxVariantsPerPrompt` | 6 |
| `maxRunJobs` | 24 |
| `maxReferenceImageBytes` | 100 MiB |
| `maxWaitMs` | 120,000 |
| `mood` | `warm-mascot`, `clean-thumbnail`, `editorial-soft`, `cinematic`, `minimal-product` |
| `engine` | `app-server-image`, `codex-svg` |
| `aspectRatio` | `16:9`, `4:3`, `1:1`, `3:4`, `9:16` |
| default `count` | 1 |
| default `mood` | `warm-mascot` |
| default `engine` | `app-server-image` |
| default `aspectRatio` | `16:9` |
| default `waitMs` | 0 |

Validation rules:

- `prompts` is a non-empty array of non-empty strings and has at most 12
  entries.
- `count` is an integer from 1 through 6.
- `prompts.length * count` is at most 24.
- MCP validates enum and scalar types strictly before server startup.
- The HTTP server applies documented defaults for unknown optional values:
  unknown mood and aspect ratio use defaults; only `codex-svg` selects the
  SVG engine and every other engine value uses the default image engine.
- MCP `waitMs` must be an integer from 0 through 120,000. The HTTP body/query
  normalization rounds finite numeric values and clamps them to that range.
- MCP reference paths must be absolute regular PNG, JPEG, or WebP files no
  larger than 100 MiB. MCP validates and snapshots the file before server
  startup, embeds the bytes as the frozen data-URL HTTP shape, and the server
  stages one owned run copy. SwiftUI additionally has a native local-path
  extension with the same semantic constraints.

## 3. HTTP and event surface

The Rust server must preserve the following meanings:

| Route | Baseline behavior |
| --- | --- |
| `GET /api/health` | Always available without starting Codex App Server. Returns `ok: true`, product identity, server root, package metadata, data paths, launch target, App Server diagnostics, and scheduler snapshot. |
| `GET /events` | Opens an SSE stream, immediately sends a `snapshot` containing current jobs, then sends run/job/server-log/server-status events until the client closes. |
| `POST /api/run` | Accepts one `prompt` or a compatible `prompts` array. Creates a run and returns `202` while incomplete or `200` when the requested wait completes. |
| `POST /api/run-batch` | Requires a `prompts` array and applies the shared batch contract. Returns run id, status URL, artifact paths, jobs, counts, outputs, diagnostics, and server data. |
| `GET /api/runs` | Lists current and restored runs ordered by most recent output update. |
| `GET /api/runs/<runId>` | Returns the run response; rejects malformed ids with `400` and unknown ids with `404`. |
| `POST /api/analyze-reference` | Sends a validated reference image to App Server analysis and returns a concise premise. Failure is a `400` response with an error. |
| `GET/POST /api/preflight/app-server-image` | Performs App Server selection/initialization. Returns `200` when ready and `503` with complete diagnostics otherwise. |
| `GET /generated/<run>/<file>` | Serves generated files only inside the generated root. Traversal is rejected. |
| `GET /artifacts/<run>/manifest` | Renders the manifest as a safe artifact view. |
| `GET /artifacts/<run>/handoff` | Renders the handoff as a safe artifact view. |
| `GET /artifacts/<run>/image?file=...` | Renders a safe image artifact view. |
| `POST /api/open-generated-dir` | Opens the generated directory through the host OS. |
| `POST /api/open-generated-file` | Validates ownership and opens or reveals one generated file. |

Every response that represents a run preserves `runId`, `status`,
`completed`, `counts`, `statusUrl`, `manifest*`, `handoff*`, `server`,
`request`, `diagnostics`, and `outputs` with the fields currently exposed by
the baseline manifest.

## 4. Run, job, and scheduler semantics

Job statuses are `queued`, `starting`, `running`, `done`, and `error`.
Aggregate run status is:

- `running` while any job is active;
- `error` when no job is active and at least one job failed;
- `done` when every job is done;
- `completed` is true whenever no job remains active, including a failed run.

`app-server-image` jobs share one global queue. The default configured limit is
24, adaptive mode is disabled, and a full accepted batch starts all 24 jobs
without a serial ramp. Overlapping batches share the same global cap.

`codex-svg` uses a separate queue with default concurrency 1.

Baseline App Server image defaults:

- max retries: 1;
- retry base delay: 4,000 ms;
- rate-limit cooldown: 45,000 ms;
- rate-limit cooldown maximum: 180,000 ms;
- job timeout: 900,000 ms;
- preflight timeout: 15,000 ms.

Retry rules:

- retry causes are limited to explicit rate-limit evidence and a completed
  turn that produced no image output without an explicit upstream failure;
- rate-limit retry applies cooldown and preserves diagnostics;
- missing-output retry is bounded and does not start a rate-limit cooldown;
- an explicit non-retryable upstream failure suppresses missing-output retry;
- stale notifications, helpers, and pending image writes from a retired
  attempt cannot mutate or satisfy a newer attempt;
- output writes drain before a retry starts;
- after the retry budget is exhausted, the terminal error code and diagnostic
  evidence remain stable.

Timeout and shutdown rules:

- a timed-out App Server job attempts `turn/interrupt`, records timeout
  diagnostics, marks the job `error`, and unbinds its thread;
- a stopped runtime marks active jobs `RuntimeClosed`, drains scheduler and
  artifact writers, closes SSE clients, and closes the server idempotently.

## 5. Codex App Server bridge

Executable selection is deterministic:

1. `IMAGE_GRID_CODEX_BIN`;
2. `CODEX_CLI_PATH`;
3. ChatGPT.app bundled Codex;
4. executable `codex` candidates from `PATH`.

Candidates must be absolute executable files. Selection diagnostics expose
selected, rejected, skipped, and unavailable candidates.

The bridge starts `codex app-server`, performs JSON-RPC `initialize`, sends
`initialized`, routes responses by id, routes notifications by thread id, and
records stderr/stdout diagnostics. A preflight failure leaves health available
so callers can receive the full diagnostic payload.

## 6. MCP contract

The baseline plugin exposes exactly one tool: `generate_image_grid`.

The tool accepts the shared generation fields plus optional
`referencePremise`, `referenceImagePath`, and `waitMs`. It auto-launches or
joins one local server startup, verifies health identity and App Server
readiness, submits `/api/run-batch`, and returns:

- text summary containing run id, status, status URL, manifest/handoff paths,
  output paths, image URLs, diagnostics, and Codex Markdown;
- structured content containing run metadata, health, launch plan, artifact
  URLs/paths, output records, diagnostics, counts, and per-output Markdown.

The installed server route uses cache-relative `IMAGE_GRID_APP_DIR` and
`IMAGE_GRID_START_COMMAND` with strict app-dir validation. Strict mode rejects
missing configuration, mismatched server roots, stale package versions,
foreign listeners, and fallback to the source repository. The server route is
the default; Electron launch is explicit and must not be an implicit fallback.

Startup deadlines are bounded and shared across concurrent tool calls. A
cross-process lock permits only one launch command. A launch or health failure
is reported as a runtime failure; it is not converted into a generic image
generation fallback.

## 7. Storage and artifact contract

On macOS the default data root is:

`~/Library/Application Support/codex-image-grid`

Under it:

- `generated/<runId>/` contains outputs, `manifest.json`, `handoff.md`, and a
  staged reference image when present;
- `.run/` contains runtime state, PID/launcher records, and reference-analysis
  state;
- `generated/<runId>/manifest.json` is atomically replaced and records schema
  version 1, request, server identity, diagnostics, and every output;
- `generated/<runId>/handoff.md` is atomically replaced and includes request,
  diagnostics, output files, browser URLs, and prompts.

On restart, manifests are scanned. Active jobs from a prior process become
interrupted/error or restored/done according to the output file that exists;
paths outside the generated root are not restored.

## 8. Electron and UI behavior that remains user-visible

The native replacement must preserve user-visible intent even though its
implementation is SwiftUI:

- Japanese / English / System language choices;
- Light / Dark / System theme choices;
- persisted display preferences with local, session, and memory fallback;
- failed-result hiding by default with persistent opt-in;
- keyboard and assistive-technology readable controls;
- lazy image loading and bounded long-session UI state;
- artifact preview, close/focus restoration, open/reveal generated file, and
  generated-directory actions.

The old Electron security policy is a baseline for equivalent native behavior:
single-instance ownership, external URL delegation, blocked non-web schemes,
artifact-only child windows, and clean shutdown of an owned server.

## 9. Parity evidence map

The frozen repository's tests are the initial executable evidence set:

- shared limits and schema: `tests/benchmark-contract.test.mjs`,
  `tests/contract-parity.test.mjs`;
- HTTP/server state, persistence, atomic writes, timeouts, and shutdown:
  `tests/server-modules.test.mjs`, `tests/server-optimization.test.mjs`;
- concurrency and retry diagnostics:
  `tests/server-batch-parallelism.test.mjs`,
  `tests/server-adaptive-concurrency.test.mjs`,
  `tests/server-diagnostics.test.mjs`;
- App Server selection and preflight:
  `tests/app-server-preflight.test.mjs`;
- MCP launch, strict identity, deadlines, and ownership:
  `tests/mcp-startup-lifecycle.test.mjs`,
  `tests/mcp-strict-app-dir.test.mjs`;
- Electron lifecycle, packaging, and security:
  `tests/electron-packaging.test.mjs`, `tests/electron-security.test.mjs`;
- UI preferences, accessibility, reference validation, and performance:
  `tests/ui-display-preferences.test.mjs`, `tests/ui-performance.test.mjs`.

The native project must convert each required behavior into an executable Rust
or Swift test, a protocol fixture, or a focused parity harness. This document
is the index and explanation; it is not the sole runtime validator.
