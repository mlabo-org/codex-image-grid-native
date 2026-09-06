#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

source "$repo_root/scripts/build-paths.sh"
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  CARGO_TARGET_DIR_WAS_SET=1
else
  CARGO_TARGET_DIR_WAS_SET=0
fi
export CARGO_TARGET_DIR_WAS_SET
image_grid_init_build_paths "$repo_root"
image_grid_prepare_cargo_target "$repo_root/Cargo.toml"
# The child smoke script receives the already-selected target and must reuse it
# without cleaning it a second time.
CARGO_TARGET_DIR_WAS_SET=1
export CARGO_TARGET_DIR_WAS_SET
trap image_grid_cleanup_build_paths EXIT

cargo test --workspace --locked --manifest-path "$repo_root/Cargo.toml"
"$repo_root/scripts/smoke-first-slice.sh"
swift test --scratch-path "$CODEX_IMAGE_GRID_SWIFT_SCRATCH_PATH" --package-path "$repo_root/macos"

if rg -n \
  "server is not activated yet|MCP is not activated yet|Rust runtime scaffold; native UI and runtime wiring are not activated yet" \
  "$repo_root/crates" "$repo_root/macos/Sources"; then
  echo "superseded scaffold placeholder remains in active source" >&2
  exit 1
fi
