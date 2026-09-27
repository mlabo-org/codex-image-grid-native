# Codex Image Grid Repository Contract

This file is the scoped source of truth for host-agent work (Codex or Claude
Code) inside this repository. It inherits higher-priority instructions of the
active host and does not replace them. This one repository is the source for
both hosts; `CLAUDE.md` only points here.

## First-task bootstrap and activation

- In Codex, on macOS, before the first setup, build, test, run, or
  source-change task in a fresh clone, run `scripts/bootstrap-codex.sh` from
  the repository root. If the script reports `up-to-date`, continue without
  reinstalling. After changing native or plugin source when activation is part
  of the task, run `scripts/bootstrap-codex.sh --force` once.
- In Claude Code, the plugin `plugin/codex-image-grid/` is loaded from the
  `suzuki-local-plugins` marketplace. Apply plugin source changes with
  `claude-plugin-refresh codex-image-grid --execute` only when activation is
  part of the task. Do not run `scripts/bootstrap-codex.sh`; it registers the
  Codex marketplace.
- Both hosts use the same `~/Applications/Codex Image Grid Native.app`. Build
  or reinstall it (`scripts/bootstrap-codex.sh --force` in Codex,
  `scripts/install-native-app.sh --execute` otherwise) only when native source
  changed and installation is authorized.
- On non-macOS, in a read-only checkout, or when a required tool is missing,
  do not attempt installation. Report the exact unsupported boundary.
- If a different source already owns the installed `codex-image-grid` plugin
  or the `codex-image-grid-native` marketplace name, stop and report it. Never
  overwrite or remove that registration automatically.

## Source and acceptance boundary

- Edit this repository, never a Codex or Claude Code plugin cache or installed
  app contents.
- Preserve unrelated worktree changes.
- Use `scripts/check.sh` as the repository's single buildable-slice acceptance
  command unless the current task declares a narrower focused check.
- Source changes, local app installation, plugin activation, Git commit, and
  publication are separate actions.
- Any change to this file requires the applicable `agents-md-clarifier` check
  before commit or handoff.
