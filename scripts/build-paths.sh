#!/usr/bin/env bash

# Shared build-path contract for the repository's check, smoke, and install
# entry points. Source this file; it is not a standalone command.

image_grid_build_root_owned=0
image_grid_cargo_target_owned=0
image_grid_build_root=""
image_grid_swift_scratch=""

image_grid_init_build_paths() {
    local repo_root="$1"

    if [[ -n "${CODEX_IMAGE_GRID_BUILD_ROOT:-}" ]]; then
        image_grid_build_root="$(cd "$CODEX_IMAGE_GRID_BUILD_ROOT" 2>/dev/null && pwd -P)" || {
            echo "CODEX_IMAGE_GRID_BUILD_ROOT is not an existing directory: $CODEX_IMAGE_GRID_BUILD_ROOT" >&2
            return 64
        }
    else
        image_grid_build_root="$(mktemp -d "${TMPDIR:-/tmp}/codex-image-grid-native-build.XXXXXX")"
        image_grid_build_root="$(cd "$image_grid_build_root" && pwd -P)"
        image_grid_build_root_owned=1
    fi

    if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
        export CARGO_TARGET_DIR="$image_grid_build_root/cargo-target"
        image_grid_cargo_target_owned=1
    elif [[ "$CARGO_TARGET_DIR" != /* ]]; then
        # Cargo resolves a relative target directory from the invocation's
        # working directory. Normalize it once so all entry points use the
        # same user-selected directory even when they change directory.
        export CARGO_TARGET_DIR="$repo_root/$CARGO_TARGET_DIR"
    fi
    if [[ -n "${CODEX_IMAGE_GRID_SWIFT_SCRATCH_PATH:-}" ]]; then
        image_grid_swift_scratch="$(mkdir -p "$CODEX_IMAGE_GRID_SWIFT_SCRATCH_PATH" && cd "$CODEX_IMAGE_GRID_SWIFT_SCRATCH_PATH" && pwd -P)"
    else
        image_grid_swift_scratch="$image_grid_build_root/swift-scratch"
        mkdir -p "$image_grid_swift_scratch"
    fi
    export CODEX_IMAGE_GRID_SWIFT_SCRATCH_PATH="$image_grid_swift_scratch"
    export CODEX_IMAGE_GRID_BUILD_ROOT="$image_grid_build_root"
    export CODEX_IMAGE_GRID_REPO_ROOT="$repo_root"
}

image_grid_prepare_cargo_target() {
    local manifest_path="$1"
    if [[ -n "${CARGO_TARGET_DIR_WAS_SET:-}" ]]; then
        return 0
    fi
    cargo metadata --no-deps --format-version 1 --manifest-path "$manifest_path" >/dev/null
    cargo clean --manifest-path "$manifest_path" --target-dir "$CARGO_TARGET_DIR"
}

image_grid_cleanup_build_paths() {
    if [[ "$image_grid_build_root_owned" -ne 1 || -z "$image_grid_build_root" || ! -d "$image_grid_build_root" ]]; then
        return 0
    fi

    # Cargo owns the Rust target tree. Prove the manifest ownership and let
    # Cargo remove it before removing the remaining Swift task files.
    if [[ "$image_grid_cargo_target_owned" -eq 1 ]]; then
        if ! cargo metadata --no-deps --format-version 1 --manifest-path "$CODEX_IMAGE_GRID_REPO_ROOT/Cargo.toml" >/dev/null \
            || ! cargo clean --manifest-path "$CODEX_IMAGE_GRID_REPO_ROOT/Cargo.toml" --target-dir "$CARGO_TARGET_DIR"; then
            echo "preserving task build root because Cargo cleanup could not be verified: $image_grid_build_root" >&2
            return 0
        fi
    fi

    # Remove only entries below this exact task-owned root, deepest first, so
    # no broad recursive deletion can reach outside the selected root.
    while IFS= read -r -d '' path; do
        if [[ -d "$path" && ! -L "$path" ]]; then
            rmdir -- "$path" 2>/dev/null || true
        else
            rm -f -- "$path"
        fi
    done < <(find "$image_grid_build_root" -mindepth 1 -depth -print0)
    rmdir -- "$image_grid_build_root" 2>/dev/null || true
}
