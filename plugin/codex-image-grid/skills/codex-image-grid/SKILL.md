---
name: codex-image-grid
description: "Image generation/画像生成 and image editing/画像編集 via codex_image_grid/generate_image_grid: Prompt Batch, character references, 部分修正, thumbnails/サムネイル, project/article/video visuals for CodexVideo and RelayPress. Native SwiftUI auto-opens."
---

# Codex Image Grid

This `SKILL.md` is the local execution contract for this skill when the skill
is selected. Claude Code must treat its routing, workflow, tool, file, and handoff
instructions as binding within this skill's scope.

## Primary route

Call `codex_image_grid/generate_image_grid` for generation and image editing.
Preserve the user's requested prompts, batch intent, generation options, and
reference-image inputs when they are supported by the current tool schema.
Treat that schema and the MCP result as authoritative for accepted inputs,
limits, defaults, output fields, and artifact validity; do not duplicate or
override those rules in this skill.

Choose `operation` from the user's request:

- New images, including a character reference used in a new scene: `generate`
  (the default). Pass the reference through `referenceImagePath` and supplied
  identity notes through `referencePremise`.
- Changes to an existing image, such as "ハンバーガーをクレープに変更して",
  "ここだけ修正", or "edit this image": `edit`. Pass the source image's absolute
  path as `referenceImagePath` and the requested changes as `prompts`. Use the
  intended image from the current conversation when its local path is known;
  ask for the source only when it cannot be identified or accessed.

The runtime owns reference-image requirements and the separate generation/edit
instructions. For edits, the tool schema documents which generation settings
are ignored. Preserve batch intent: each prompt and variant edits the supplied
source independently. To edit a preceding result, supply that result's path.

For CodexVideo, RelayPress, or another parent workflow, return the tool's
generated paths and handoff to the caller that requested the visuals. The
native SwiftUI app opens automatically through this route. Do not start, call,
or fall back to the separate retired Electron project.

## Source, cache, install, and refresh boundaries

- Public plugin source authority is this plugin directory under
  `plugin/codex-image-grid/` in the source repository.
- Its Rust and Swift implementation source is the repository root.
- Claude Code plugin cache is generated runtime state, not an edit target.
- An installed plugin or cached copy is an activation surface, not source.
  Never repair source behavior by patching it in place.
- Build, installation, and active-session pickup are separate actions. Apply
  a plugin source change with `claude-plugin-refresh` only when activation is
  authorized; do not claim that a source edit activated the plugin. The shared
  `~/Applications/Codex Image Grid Native.app` is not built or installed from
  this checkout.

## Stop conditions

Stop and report the exact boundary when `codex_image_grid/generate_image_grid`
is unavailable, its live schema cannot be read, the installed route resolves
to a different source, or the native launch fails. Do not substitute the
separate retired Electron runtime, edit cache or installed files, or silently
choose an ad hoc image generator.

## Handoff

Return the MCP result, generated paths, and handoff fields needed by the user
or parent workflow. Report any tool or activation blocker without inventing a
replacement output contract.
