---
name: codex-image-grid
description: "Image generation/画像生成 and image editing/画像編集 via codex_image_grid/generate_image_grid: Prompt Batch, character references, 部分修正, thumbnails/サムネイル, single visuals; article image sets start at relaypress-image-builder. Native app opens."
---

# Codex Image Grid

This `SKILL.md` is the local execution contract for this skill when the skill
is selected. The host agent (Codex, Claude Code, or Antigravity) must treat its routing, workflow, tool, file, and handoff
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

Image Grid does not choose the engine or a generation's aspect ratio, and
nothing selected in its app applies to a tool call. Set `engine` and
`aspectRatio` from what the user or the calling workflow decided. When neither
has decided an engine, pass `engine: grok-imagine-2` (Grok Imagine 2.0, the
user's standing default); do not leave `engine` out. When neither has decided an aspect
ratio, ask the user; do not pick one yourself. A call that leaves one open
returns that question instead of images: answer it by asking the user, then
call again.

Both Grok engines draw through the local Grok CLI. `grok-imagine-2` draws with
Grok Imagine 2.0, the web version's style, and is the one to use when the user
just says Grok. `grok-imagine` is the CLI's older default model; use it only
when the user asks for the older model. `app-server-image` draws GPT Image on
the ChatGPT subscription. `openai-image-api` draws GPT Image through the OpenAI
API and bills each image: use it only when the user has agreed to that charge
for this request. When the user names a GPT Image version for it, pass the
exact OpenAI model ID as `model` (2.5 and 2.6 need a variant, such as
`gpt-image-2.6-sunburst` or `gpt-image-2.6-flare`); ask which variant when
the user names only the version. A result with code `OpenAiApiNeedsApproval`
(the subscription has no paid ChatGPT plan, or a Grok job lost its sign-in,
plan, or usage allowance) means nothing was drawn and the OpenAI API can draw
it: ask the user once whether to draw with `openai-image-api`, which bills
each image, and only on a yes call again with that engine and the same
prompts, reference, and aspect ratio. When a result says the subscription cannot draw the run, or
that Codex image generation is rate-limited, tell the user and offer the
options it names; do not rerun with another engine until the user says so.

With a Grok engine, a `generate` prompt whose pose matches the reference
image's pose gets the reference figure back with only the background redrawn,
so a camera-angle change alone does not take effect. To change the angle,
describe a clearly different pose in the same prompt. Check hands and small
facial marks on Grok output before handing it over.

## Prompt and result rules

These hold for every call this skill makes, whichever engine draws.

- Text in images: the engines invent fake characters wherever a label would
  sit, even when the prompt excludes text, and the user never accepts them.
  For any object that naturally carries text (breaker, switch, sign, screen,
  button, book cover), either name the exact short text in the prompt
  (「ラベルの文字は『非常停止』の4文字だけ、それ以外の文字は描かない」) or say the
  plate is blank. Before handing results over, look at every label area; a
  gibberish label means regenerating that image.
- Variants: return every variant to the user. You may say which one you
  recommend, but never drop one because it looks nearly identical to you. For
  fine effects (blur, depth, line weight, skin texture) the user's eye decides;
  describe what you see as an impression, not a fact.
- Character references: choose each reference image one by one; never pass a
  whole expression set. Drop images whose marks or props are not part of the
  character's permanent look (anger marks, sweat drops, tears, blush effects):
  the engines keep them as identity features. When the output will show the
  full body and the references are bust-up icons, add one clean full-body
  reference so the outfit below the chest is not invented.
- Chibi じぴこ: pass her chibi character sheet,
  `~/plugins/relaypress/skills/relaypress-image-builder/assets/jipiko-reference/chibi-character-sheet.jpg`,
  as the only `referenceImagePath`, never her normal sheets (`variant-01.png`,
  `variant-02.png`), which pull the figure back to full height. Her side
  ponytail is on her left (viewer's right when she faces front); CodexVideo's
  chibi assets are drawn mirrored, so flip them before using one as a
  reference.
- Anime women: with a character sheet as the reference, write no fixed head
  count; tell the engine to keep the sheet's own proportions (head size, waist
  height, torso-to-leg ratio), and check those against the sheet before small
  details such as ribbons or fingers. Without a sheet, state about 160 cm and
  5.5 heads, the user's taste. Either way add the
  line "NOT tall, NOT a model, NOT long-legged, NOT voluptuous", fix the style
  to clean 2D anime (no semi-real, 2.5D, or webtoon rendering), and do not
  stack glamour words or heels unless the user asks. "Not the generic face"
  means shifting the modern anime face's proportions toward human ones
  (smaller eyes, a small visible nose bridge, a soft jaw), never adding
  unhealthy or unkempt flaws; at most one small charm point such as a mole.

For CodexVideo, RelayPress, or another parent workflow, return the tool's
generated paths and handoff to the caller that requested the visuals. The
native SwiftUI app opens automatically through this route. Do not start, call,
or fall back to the separate retired Electron project.

## Source, cache, install, and refresh boundaries

- Public plugin source authority is this plugin directory under
  `plugin/codex-image-grid/` in the source repository.
- Its Rust and Swift implementation source is the repository root.
- Codex and Claude Code plugin caches are generated runtime state, not edit
  targets.
- An installed plugin or cached copy is an activation surface, not source.
  Never repair source behavior by patching it in place.
- Build, installation, and active-session pickup are separate actions. Apply
  them only when repository setup or activation is authorized, and do not
  claim that a source edit activated the plugin. In Codex, use
  `scripts/bootstrap-codex.sh` in a source checkout; in Claude Code, a
  marketplace added from GitHub is updated with
  `claude plugin marketplace update codex-image-grid-native` and
  `claude plugin update codex-image-grid@codex-image-grid-native`; one added
  from a local clone loads the plugin in place, so a source change takes
  effect at the next session start or `/reload-plugins`. Both
  hosts share `~/Applications/Codex Image Grid Native.app`.

## Stop conditions

Stop and report the exact boundary when `codex_image_grid/generate_image_grid`
is unavailable, its live schema cannot be read, the installed route resolves
to a different source, or the native launch fails. Do not substitute the
separate retired Electron runtime, edit cache or installed files, or silently
choose an ad hoc image generator.

## Handoff

Return the MCP result, generated paths, and handoff fields needed by the user
or parent workflow. Each PNG or JPEG output has a lossy WebP copy beside it for
web upload, listed in `webpPaths`; the PNG or JPEG stays the original. Report any tool or activation blocker without inventing a
replacement output contract.
