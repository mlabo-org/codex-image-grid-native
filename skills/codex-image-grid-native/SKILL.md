---
name: codex-image-grid-native
description: "Operate and develop the native Codex Image Grid plugin: Rust runtime, SwiftUI macOS app, MCP launch, local reference-image paths, and parity with the frozen Electron baseline. Use for native image-grid generation or runtime verification; do not use for frozen Electron or browser-only work."
---

# Codex Image Grid Native

This `SKILL.md` is the local execution contract for this skill when it is
selected. Codex must treat its trigger assumptions, workflow, tool boundaries,
file boundaries, and output shape as binding within this skill's scope. It does
not override system instructions, developer instructions, explicit user
requests, applicable `AGENTS.md` files, or more-local contracts.

## Trigger and non-use

Use this skill for requests involving the native Rust + SwiftUI Image Grid
runtime, its Codex MCP integration, local reference-image paths, native app
launch, or parity verification against the frozen Electron baseline.

Do not use it for the frozen repository at
`/Users/suzukimakoto/plugins/codex-image-grid`, generic image generation, or a
browser-only workflow that does not involve the native runtime.

Until `.mcp.json` names a built MCP binary and the runtime primary path passes
its acceptance bundle, treat this repository as source-development work. Do
not claim that native generation or MCP activation succeeded.

## Primary route

1. Read the repository `README.md`, `docs/architecture.md`, and
   `docs/development-contract.md` before changing source.
2. Keep the Rust workspace under `crates/` as the authority for runtime
   behavior and public validation.
3. Keep SwiftUI concerns under `macos/`; it owns native UI, file selection,
   preferences, and runtime lifecycle, not job semantics.
4. Keep plugin metadata under `.codex-plugin/` and MCP launch configuration in
   `.mcp.json`. Do not patch Codex plugin cache copies.
5. Use `scripts/check.sh` for the minimum scaffold validation, then add only
   the affected focused checks required by the changed surface.

## Interface contract

The final plugin accepts Prompt Batch requests through `generate_image_grid`
and returns the existing run status, manifest, handoff, output paths,
display-safe URLs, and Codex Markdown contract. Native and MCP reference
images are local absolute paths. The Rust runtime stages a validated copy
before starting jobs. Browser compatibility uses a separate binary/staged
upload route.

## Stop conditions

Stop and report the blocker when the Rust toolchain, Swift toolchain, source
boundary, frozen baseline behavior, runtime identity, or MCP executable path
cannot be resolved. Do not silently fall back to the frozen Electron runtime,
cache files, generated output, or an unbuilt placeholder binary.

## Handoff

Report changed source paths, the Rust/Swift validation commands and results,
whether MCP activation remains intentionally withheld, any parity evidence,
and the smallest next implementation boundary.

