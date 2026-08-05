# Codex Image Grid Repository Contract

This file is the scoped source of truth for Codex work inside this repository.
It inherits higher-priority Codex instructions and does not replace them.

## First-task bootstrap

- On macOS, before the first setup, build, test, run, or source-change task in
  a fresh clone, run `scripts/bootstrap-codex.sh` from the repository root.
- If the script reports `up-to-date`, continue without reinstalling.
- After changing native or plugin source when activation is part of the task,
  run `scripts/bootstrap-codex.sh --force` once.
- On non-macOS, in a read-only checkout, or when a required tool is missing,
  do not attempt installation. Report the exact unsupported boundary.
- If a different source already owns the installed `codex-image-grid` plugin
  or the `codex-image-grid-native` marketplace name, stop and report it. Never
  overwrite or remove that registration automatically.

## Source and acceptance boundary

- Edit this repository, never Codex plugin cache or installed app contents.
- Preserve unrelated worktree changes.
- Use `scripts/check.sh` as the repository's single buildable-slice acceptance
  command unless the current task declares a narrower focused check.
- Source changes, local app installation, Codex plugin activation, Git commit,
  and publication are separate actions.
- Any change to this file requires the applicable `agents-md-clarifier` check
  before commit or handoff.
