#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cargo test --workspace --manifest-path "$repo_root/Cargo.toml"
"$repo_root/scripts/smoke-first-slice.sh"
swift test --package-path "$repo_root/macos"

if rg -n \
  "server is not activated yet|MCP is not activated yet|Rust runtime scaffold; native UI and runtime wiring are not activated yet" \
  "$repo_root/crates" "$repo_root/macos/Sources"; then
  echo "superseded scaffold placeholder remains in active source" >&2
  exit 1
fi
