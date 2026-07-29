#!/bin/bash

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
INSTALL_PARENT="/Users/suzukimakoto/Applications"
APP_NAME="Codex Image Grid Native.app"
INSTALL_TARGET="$INSTALL_PARENT/$APP_NAME"
APP_EXECUTABLE="CodexImageGridNative"
MCP_EXECUTABLE="image-grid-mcp"
SERVER_EXECUTABLE="image-grid-server"
PACKAGE_VERSION="0.2.4"
BUNDLE_IDENTIFIER="local.codex.image-grid.native"
DATA_ROOT="/Users/suzukimakoto/Library/Application Support/codex-image-grid"
INFO_PLIST_SOURCE="$REPO_ROOT/macos/App/Info.plist"
APP_ICON_SOURCE="$REPO_ROOT/macos/App/AppIcon.icns"

mode="dry-run"
case "${1:-}" in
    "")
        ;;
    "--dry-run")
        ;;
    "--execute")
        mode="execute"
        ;;
    *)
        echo "usage: scripts/install-native-app.sh [--dry-run|--execute]" >&2
        exit 64
        ;;
esac
if (( $# > 1 )); then
    echo "usage: scripts/install-native-app.sh [--dry-run|--execute]" >&2
    exit 64
fi

print_plan() {
    echo "mode: $mode"
    echo "installTarget: $INSTALL_TARGET"
    echo "bundleIdentifier: $BUNDLE_IDENTIFIER"
    echo "displayName: Codex Image Grid"
    echo "packageVersion: $PACKAGE_VERSION"
    echo "dataRoot: $DATA_ROOT"
    echo "appServerImageMaxRetries: 1 (frozen baseline runtime default; no app override)"
    echo "infoPlistSource: $INFO_PLIST_SOURCE"
    echo "appIconSource: $APP_ICON_SOURCE"
    echo "build:"
    echo "- cargo build --release -p image-grid-mcp -p image-grid-server"
    echo "- swift build -c release --package-path $REPO_ROOT/macos"
    echo "bundleContents:"
    echo "- Contents/Info.plist"
    echo "- Contents/MacOS/$APP_EXECUTABLE"
    echo "- Contents/Resources/AppIcon.icns"
    echo "- Contents/Resources/$MCP_EXECUTABLE"
    echo "- Contents/Resources/$SERVER_EXECUTABLE"
    echo "signing: ad-hoc codesign followed by deep strict verification"
    echo "install: stage beside the target, then rename the complete signed bundle into place"
}

print_plan
if [[ "$mode" == "dry-run" ]]; then
    echo "result: dry-run only; no build or install was performed"
    exit 0
fi

for tool in cargo swift codesign plutil; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "required tool is unavailable: $tool" >&2
        exit 69
    fi
done
/usr/bin/plutil -lint "$INFO_PLIST_SOURCE" >/dev/null
if [[ ! -f "$APP_ICON_SOURCE" ]]; then
    echo "app icon is unavailable: $APP_ICON_SOURCE" >&2
    exit 66
fi
if [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$INFO_PLIST_SOURCE")" != "$BUNDLE_IDENTIFIER" ]]; then
    echo "Info.plist bundle identifier does not match $BUNDLE_IDENTIFIER" >&2
    exit 65
fi
if [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$INFO_PLIST_SOURCE")" != "$PACKAGE_VERSION" ]]; then
    echo "Info.plist package version does not match $PACKAGE_VERSION" >&2
    exit 65
fi
if [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$INFO_PLIST_SOURCE")" != "AppIcon.icns" ]]; then
    echo "Info.plist must declare AppIcon.icns" >&2
    exit 65
fi
if /usr/libexec/PlistBuddy -c 'Print :LSEnvironment:IMAGE_GRID_APP_SERVER_IMAGE_MAX_RETRIES' "$INFO_PLIST_SOURCE" >/dev/null 2>&1; then
    echo "Info.plist must not override the frozen baseline App Server image retry default" >&2
    exit 65
fi

(
    cd "$REPO_ROOT"
    cargo build --release -p image-grid-mcp -p image-grid-server
)
swift build -c release --package-path "$REPO_ROOT/macos"

SWIFT_BIN_DIR="$(swift build -c release --package-path "$REPO_ROOT/macos" --show-bin-path)"
APP_SOURCE="$SWIFT_BIN_DIR/$APP_EXECUTABLE"
MCP_SOURCE="$REPO_ROOT/target/release/$MCP_EXECUTABLE"
SERVER_SOURCE="$REPO_ROOT/target/release/$SERVER_EXECUTABLE"
for artifact in "$APP_SOURCE" "$MCP_SOURCE" "$SERVER_SOURCE"; do
    if [[ ! -f "$artifact" || ! -x "$artifact" ]]; then
        echo "release artifact is unavailable or not executable: $artifact" >&2
        exit 66
    fi
done

/bin/mkdir -p "$INSTALL_PARENT"
STAGE_ROOT="$(/usr/bin/mktemp -d "$INSTALL_PARENT/.codex-image-grid-native-stage.XXXXXX")"
STAGED_APP="$STAGE_ROOT/$APP_NAME"
PREVIOUS_APP=""
INSTALLED_NEW=0

cleanup() {
    if [[ -n "${STAGE_ROOT:-}" && -d "$STAGE_ROOT" ]]; then
        /bin/rm -rf -- "$STAGE_ROOT"
    fi
}
rollback() {
    local status=$?
    if [[ $status -ne 0 && $INSTALLED_NEW -eq 1 && -e "$INSTALL_TARGET" ]]; then
        /bin/mv "$INSTALL_TARGET" "$STAGE_ROOT/failed-$APP_NAME"
        INSTALLED_NEW=0
    fi
    if [[ $status -ne 0 && -n "$PREVIOUS_APP" && -d "$PREVIOUS_APP" && ! -e "$INSTALL_TARGET" ]]; then
        /bin/mv "$PREVIOUS_APP" "$INSTALL_TARGET"
        PREVIOUS_APP=""
    fi
    cleanup
    exit "$status"
}
trap rollback EXIT

/bin/mkdir -p "$STAGED_APP/Contents/MacOS" "$STAGED_APP/Contents/Resources"
/usr/bin/install -m 755 "$APP_SOURCE" "$STAGED_APP/Contents/MacOS/$APP_EXECUTABLE"
/usr/bin/install -m 644 "$APP_ICON_SOURCE" "$STAGED_APP/Contents/Resources/AppIcon.icns"
/usr/bin/install -m 755 "$MCP_SOURCE" "$STAGED_APP/Contents/Resources/$MCP_EXECUTABLE"
/usr/bin/install -m 755 "$SERVER_SOURCE" "$STAGED_APP/Contents/Resources/$SERVER_EXECUTABLE"
/usr/bin/install -m 644 "$INFO_PLIST_SOURCE" "$STAGED_APP/Contents/Info.plist"

/usr/bin/plutil -lint "$STAGED_APP/Contents/Info.plist" >/dev/null
/usr/bin/codesign --force --sign - "$STAGED_APP/Contents/MacOS/$APP_EXECUTABLE"
/usr/bin/codesign --force --sign - "$STAGED_APP/Contents/Resources/$MCP_EXECUTABLE"
/usr/bin/codesign --force --sign - "$STAGED_APP/Contents/Resources/$SERVER_EXECUTABLE"
/usr/bin/codesign --force --sign - "$STAGED_APP"
/usr/bin/codesign --verify --deep --strict --verbose=2 "$STAGED_APP"

if [[ -e "$INSTALL_TARGET" ]]; then
    PREVIOUS_APP="$INSTALL_PARENT/.Codex Image Grid Native.previous.$$.app"
    /bin/mv "$INSTALL_TARGET" "$PREVIOUS_APP"
fi
/bin/mv "$STAGED_APP" "$INSTALL_TARGET"
INSTALLED_NEW=1

/usr/bin/codesign --verify --deep --strict --verbose=2 "$INSTALL_TARGET"
if [[ -n "$PREVIOUS_APP" && -d "$PREVIOUS_APP" ]]; then
    TRASH_TARGET="/Users/suzukimakoto/.Trash/Codex Image Grid Native.before-$(
        /bin/date +%Y%m%d-%H%M%S
    )-$$.app"
    /bin/mv "$PREVIOUS_APP" "$TRASH_TARGET"
    PREVIOUS_APP=""
    echo "previousInstallMovedTo: $TRASH_TARGET"
fi

STAGE_ROOT=""
INSTALLED_NEW=0
trap - EXIT
echo "result: installed and verified"
echo "installedApp: $INSTALL_TARGET"
