---
name: codex-image-grid
description: "Route image generation/画像生成 to Codex Image Grid via codex_image_grid/generate_image_grid. Use for Prompt Batch, thumbnails/サムネイル, project, article, or video visuals, including CodexVideo, Agentic StructCiv, and RelayPress. Native SwiftUI auto-opens; not for image editing or the frozen Electron app."
---

# Codex Image Grid

## Primary route

Call `codex_image_grid/generate_image_grid` as the primary generation route.
Preserve the user's requested prompts, batch intent, generation options, and
reference-image inputs when they are supported by the current tool schema.
Treat that schema and the MCP result as authoritative for accepted inputs,
limits, defaults, output fields, and artifact validity; do not duplicate or
override those rules in this skill.

For CodexVideo, Agentic StructCiv, RelayPress, or another parent workflow,
return the tool's generated paths and handoff to the caller that requested the
visuals. The native SwiftUI app opens automatically through this route. Do not
start, call, or fall back to the frozen Electron app.

## Source, cache, install, and refresh boundaries

- Public plugin source authority is
  `/Users/suzukimakoto/plugins/codex-image-grid-native/plugin/codex-image-grid`.
- Its Rust and Swift implementation source remains in
  `/Users/suzukimakoto/plugins/codex-image-grid-native`.
- Codex plugin cache under `/Users/suzukimakoto/.codex/plugins/cache/` is
  generated runtime state, not an edit target.
- An installed plugin or cached copy is an activation surface, not source.
  Never repair source behavior by patching it in place.
- Refresh and installation are separate activation actions. After a source
  change, use the source-first `refresh-codex-plugin` flow only when that
  action is authorized; do not claim that a source edit activated the plugin.

## Stop conditions

Stop and report the exact boundary when `codex_image_grid/generate_image_grid`
is unavailable, its live schema cannot be read, the installed route resolves
to a different source, or the native launch fails. Do not substitute the old
Electron runtime, edit cache or installed files, or silently choose an ad hoc
image generator.

## Handoff

Return the MCP result, generated paths, and handoff fields needed by the user
or parent workflow. Report any tool or activation blocker without inventing a
replacement output contract.
