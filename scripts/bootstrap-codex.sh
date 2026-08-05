#!/bin/bash

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
USER_HOME="${HOME:?HOME must be set to bootstrap Codex Image Grid}"
PLUGIN_ROOT="$REPO_ROOT/plugin/codex-image-grid"
MARKETPLACE_NAME="codex-image-grid-native"
PLUGIN_NAME="codex-image-grid"
PLUGIN_SPEC="$PLUGIN_NAME@$MARKETPLACE_NAME"
APP_PATH="$USER_HOME/Applications/Codex Image Grid Native.app"
RECEIPT_DIR="$REPO_ROOT/.run"
RECEIPT_PATH="$RECEIPT_DIR/codex-bootstrap-v1.json"
BOOTSTRAP_VERSION="1"

mode="execute"
force=0
case "${1:-}" in
    "")
        ;;
    "--dry-run")
        mode="dry-run"
        ;;
    "--force")
        force=1
        ;;
    *)
        echo "usage: scripts/bootstrap-codex.sh [--dry-run|--force]" >&2
        exit 64
        ;;
esac
if (( $# > 1 )); then
    echo "usage: scripts/bootstrap-codex.sh [--dry-run|--force]" >&2
    exit 64
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "Codex Image Grid automatic installation currently requires macOS." >&2
    exit 69
fi

for tool in cargo rustc swift jq git shasum codex; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "required tool is unavailable: $tool" >&2
        exit 69
    fi
done

source_fingerprint() {
    (
        cd "$REPO_ROOT"
        git rev-parse HEAD
        git diff --binary -- .
        while IFS= read -r path; do
            printf '%s\n' "$path"
            shasum -a 256 -- "$path"
        done < <(git ls-files --others --exclude-standard)
    ) | shasum -a 256 | awk '{print $1}'
}

FINGERPRINT="$(source_fingerprint)"
APP_EXECUTABLE="$APP_PATH/Contents/MacOS/CodexImageGridNative"
MCP_EXECUTABLE="$APP_PATH/Contents/Resources/image-grid-mcp"
SERVER_EXECUTABLE="$APP_PATH/Contents/Resources/image-grid-server"

PLUGIN_STATE="$(codex plugin list --json)"
MARKETPLACE_STATE="$(codex plugin marketplace list --json)"

if jq -e --arg name "$PLUGIN_NAME" --arg path "$PLUGIN_ROOT" \
    '.installed[]? | select(.name == $name and (.source.path // "") != $path)' \
    <<<"$PLUGIN_STATE" >/dev/null; then
    echo "a different source already owns the installed $PLUGIN_NAME plugin; no changes were made" >&2
    exit 73
fi

if jq -e --arg name "$MARKETPLACE_NAME" --arg root "$REPO_ROOT" \
    '.marketplaces[]? | select(.name == $name and (.root // "") != $root)' \
    <<<"$MARKETPLACE_STATE" >/dev/null; then
    echo "a different source already owns the $MARKETPLACE_NAME marketplace name; no changes were made" >&2
    exit 73
fi

plugin_owned=0
plugin_marketplace=""
if jq -e --arg name "$PLUGIN_NAME" --arg path "$PLUGIN_ROOT" \
    '.installed[]? | select(.name == $name and (.source.path // "") == $path)' \
    <<<"$PLUGIN_STATE" >/dev/null; then
    plugin_owned=1
    plugin_marketplace="$(
        jq -r --arg name "$PLUGIN_NAME" --arg path "$PLUGIN_ROOT" \
            '.installed[]? | select(.name == $name and (.source.path // "") == $path) | .marketplaceName' \
            <<<"$PLUGIN_STATE" | head -n 1
    )"
fi

marketplace_owned=0
if jq -e --arg name "$MARKETPLACE_NAME" --arg root "$REPO_ROOT" \
    '.marketplaces[]? | select(.name == $name and (.root // "") == $root)' \
    <<<"$MARKETPLACE_STATE" >/dev/null; then
    marketplace_owned=1
fi

if [[ $force -eq 0 && $plugin_owned -eq 1 && -f "$RECEIPT_PATH" && -x "$APP_EXECUTABLE" && -x "$MCP_EXECUTABLE" && -x "$SERVER_EXECUTABLE" ]]; then
    if jq -e \
        --arg version "$BOOTSTRAP_VERSION" \
        --arg root "$REPO_ROOT" \
        --arg fingerprint "$FINGERPRINT" \
        '.bootstrapVersion == $version and .repoRoot == $root and .sourceFingerprint == $fingerprint' \
        "$RECEIPT_PATH" >/dev/null; then
        echo "result: up-to-date"
        echo "installedApp: $APP_PATH"
        exit 0
    fi
fi

echo "mode: $mode"
echo "repoRoot: $REPO_ROOT"
echo "installedApp: $APP_PATH"
echo "pluginSource: $PLUGIN_ROOT"
echo "marketplace: $MARKETPLACE_NAME"

if [[ "$mode" == "dry-run" ]]; then
    "$REPO_ROOT/scripts/install-native-app.sh" --dry-run
    if [[ $force -eq 1 && $plugin_owned -eq 1 && "$plugin_marketplace" != "$MARKETPLACE_NAME" ]]; then
        if [[ $marketplace_owned -eq 0 ]]; then
            echo "pluginAction: codex plugin marketplace add $REPO_ROOT"
        fi
        echo "pluginAction: migrate $PLUGIN_NAME from $plugin_marketplace to $MARKETPLACE_NAME"
    elif [[ $force -eq 1 && "$plugin_marketplace" == "$MARKETPLACE_NAME" ]]; then
        echo "pluginAction: reinstall $PLUGIN_SPEC from the updated source"
    elif [[ $plugin_owned -eq 1 ]]; then
        echo "pluginAction: keep existing registration from this source"
    elif [[ $marketplace_owned -eq 1 ]]; then
        echo "pluginAction: codex plugin add $PLUGIN_SPEC"
    else
        echo "pluginAction: codex plugin marketplace add $REPO_ROOT"
        echo "pluginAction: codex plugin add $PLUGIN_SPEC"
    fi
    echo "result: dry-run only; no build, install, or registration was performed"
    exit 0
fi

"$REPO_ROOT/scripts/install-native-app.sh" --execute

if [[ $force -eq 1 && $plugin_owned -eq 1 && "$plugin_marketplace" != "$MARKETPLACE_NAME" ]]; then
    if [[ $marketplace_owned -eq 0 ]]; then
        codex plugin marketplace add "$REPO_ROOT"
    fi
    codex plugin remove "$PLUGIN_NAME@$plugin_marketplace"
    codex plugin add "$PLUGIN_SPEC"
elif [[ $force -eq 1 && "$plugin_marketplace" == "$MARKETPLACE_NAME" ]]; then
    codex plugin remove "$PLUGIN_SPEC"
    codex plugin add "$PLUGIN_SPEC"
elif [[ $plugin_owned -eq 0 ]]; then
    if [[ $marketplace_owned -eq 0 ]]; then
        codex plugin marketplace add "$REPO_ROOT"
    fi
    codex plugin add "$PLUGIN_SPEC"
fi

/bin/mkdir -p "$RECEIPT_DIR"
jq -n \
    --arg version "$BOOTSTRAP_VERSION" \
    --arg root "$REPO_ROOT" \
    --arg fingerprint "$FINGERPRINT" \
    --arg app "$APP_PATH" \
    --arg plugin "$PLUGIN_NAME" \
    --arg marketplace "$MARKETPLACE_NAME" \
    '{
        bootstrapVersion: $version,
        repoRoot: $root,
        sourceFingerprint: $fingerprint,
        installedApp: $app,
        plugin: $plugin,
        marketplace: $marketplace
    }' >"$RECEIPT_PATH"

echo "result: built, installed, and registered for Codex"
echo "receipt: $RECEIPT_PATH"
