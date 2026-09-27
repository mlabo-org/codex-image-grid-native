# Codex Image Grid Repository Contract

This file is the scoped source of truth for Claude Code work inside this repository.
It inherits higher-priority Claude Code instructions and does not replace them.

## Plugin activation

- The Claude Code plugin is `plugin/codex-image-grid/`. Apply its source
  changes with `claude-plugin-refresh codex-image-grid --execute` only when
  activation is part of the task.
- The shared `~/Applications/Codex Image Grid Native.app` is owned by the Codex
  side; do not build, install, or replace it from this checkout.

## Source and acceptance boundary

- Edit this repository, never Claude Code plugin cache or installed app contents.
- Preserve unrelated worktree changes.
- Use `scripts/check.sh` as the repository's single buildable-slice acceptance
  command unless the current task declares a narrower focused check.
- Source changes, local app installation, Claude Code plugin activation, Git commit,
  and publication are separate actions.
- Any change to this file requires the applicable `agents-md-clarifier` check
  before commit or handoff.
