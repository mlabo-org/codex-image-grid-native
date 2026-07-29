#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
temporary_root="$(mktemp -d "${TMPDIR:-/tmp}/codex-image-grid-native-smoke.XXXXXX")"
temporary_root="$(cd "$temporary_root" && pwd -P)"
server_pid=""

cleanup() {
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf -- "$temporary_root"
}
trap cleanup EXIT

command -v curl >/dev/null
command -v jq >/dev/null

cargo build \
  --manifest-path "$repo_root/Cargo.toml" \
  --package image-grid-server \
  --package image-grid-mcp

server_stdout="$temporary_root/server.stdout"
server_stderr="$temporary_root/server.stderr"
data_root="$temporary_root/data"
fake_codex="$temporary_root/fake-codex"

printf '%s\n' \
  '#!/bin/sh' \
  'test "$1" = "app-server" || exit 2' \
  'while IFS= read -r line; do' \
  '  case "$line" in' \
  '    *'"'"'"method":"initialize"'"'"'*)' \
  '      printf '"'"'%s\n'"'"' '"'"'{"id":1,"result":{"userAgent":"fixture","codexHome":"/tmp/fixture","platformFamily":"unix","platformOs":"macos"}}'"'"'' \
  '      ;;' \
  '    *'"'"'"method":"initialized"'"'"'*)' \
  '      ;;' \
  '    *'"'"'"method":"thread/start"'"'"'*)' \
  '      printf '"'"'%s\n'"'"' '"'"'{"id":2,"result":{"thread":{"id":"fixture-thread"}}}'"'"'' \
  '      ;;' \
  '    *'"'"'"method":"turn/start"'"'"'*)' \
  '      printf '"'"'%s\n'"'"' '"'"'{"id":3,"result":{"turn":{"id":"fixture-turn"}}}'"'"'' \
  '      printf '"'"'%s\n'"'"' '"'"'{"method":"item/completed","params":{"threadId":"fixture-thread","turnId":"fixture-turn","item":{"type":"imageGeneration","id":"fixture-image","status":"completed","revisedPrompt":null,"result":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="}}}'"'"'' \
  '      printf '"'"'%s\n'"'"' '"'"'{"method":"turn/completed","params":{"threadId":"fixture-thread","turn":{"id":"fixture-turn","items":[],"itemsView":"full","status":"completed","error":null,"startedAt":null,"completedAt":null,"durationMs":1}}}'"'"'' \
  '      ;;' \
  '  esac' \
  'done' \
  >"$fake_codex"
chmod 755 "$fake_codex"

IMAGE_GRID_CODEX_BIN="$fake_codex" "$repo_root/target/debug/image-grid-server" \
  --bind 127.0.0.1:0 \
  --data-root "$data_root" \
  --server-root "$repo_root" \
  >"$server_stdout" 2>"$server_stderr" &
server_pid="$!"

server_url=""
for _ in $(seq 1 100); do
  server_url="$(sed -n 's/^listening: //p' "$server_stdout" | tail -n 1)"
  if [[ -n "$server_url" ]]; then
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    sed -n '1,120p' "$server_stderr" >&2
    exit 1
  fi
  sleep 0.05
done
if [[ -z "$server_url" ]]; then
  echo "server did not report its listener" >&2
  exit 1
fi

health_json="$temporary_root/health.json"
curl --fail --silent --show-error "$server_url/api/health" >"$health_json"
jq --exit-status \
  --arg serverRoot "$repo_root" \
  --arg dataDir "$data_root" \
  '
    .ok == true
    and .jobs == 0
    and .appServerImage == false
    and .appServerImageReady == false
    and .appServerImageDiagnostics.status == "not-started"
    and .appServerImageDiagnostics.ready == false
    and .app == "codex-image-grid-native"
    and .serverRoot == $serverRoot
    and .dataDir == $dataDir
    and .generatedDir == ($dataDir + "/generated")
    and .runDir == ($dataDir + "/.run")
    and .workspaceDir == $dataDir
    and .launchTarget == "server"
    and .packageName == "codex-image-grid-native"
    and .packageVersion == "0.1.0"
    and .packageRootKind == "source"
    and .codexAppServer.status == "not-started"
    and .codexAppServer.ready == false
    and .appServerImageScheduler == {
      "configuredMax": 24,
      "adaptive": false,
      "currentLimit": 24,
      "active": 0,
      "queued": 0
    }
    and .identity.app == .app
    and .identity.serverRoot == .serverRoot
    and .identity.dataDir == .dataDir
    and .identity.codexAppServer == .codexAppServer
    and .identity.appServerImageScheduler == .appServerImageScheduler
  ' "$health_json" >/dev/null

preflight_json="$temporary_root/preflight.json"
curl --fail --silent --show-error \
  --request POST \
  "$server_url/api/preflight/app-server-image" \
  >"$preflight_json"
jq --exit-status \
  --arg fakeCodex "$fake_codex" \
  '
    .ok == true
    and .appServerImage == true
    and .appServerImageReady == true
    and .diagnostics.status == "ready"
    and .diagnostics.ready == true
    and .diagnostics.selectedCommand == $fakeCodex
    and .diagnostics.selectedSource == "IMAGE_GRID_CODEX_BIN"
    and .diagnostics.platformOs == "macos"
    and .diagnostics.error == null
  ' "$preflight_json" >/dev/null

ready_health_json="$temporary_root/health-ready.json"
curl --fail --silent --show-error "$server_url/api/health" >"$ready_health_json"
jq --exit-status \
  '
    .ok == true
    and .appServerImage == true
    and .appServerImageReady == true
    and .appServerImageDiagnostics.status == "ready"
    and .codexAppServer.status == "ready"
    and .identity.codexAppServer.status == "ready"
  ' "$ready_health_json" >/dev/null

run_json="$temporary_root/run.json"
curl --fail --silent --show-error \
  --request POST \
  --header 'content-type: application/json' \
  --data '{"prompts":["fixture prompt"],"count":1,"mood":"warm-mascot","engine":"app-server-image","aspectRatio":"16:9","waitMs":5000}' \
  "$server_url/api/run-batch" \
  >"$run_json"
jq --exit-status \
  '
    .status == "done"
    and .completed == true
    and .counts == {"total":1,"done":1,"running":0,"failed":0}
    and .jobs[0].status == "queued"
    and .outputs[0].status == "done"
    and .outputs[0].filename == "variant-01.png"
    and .outputs[0].threadId == "fixture-thread"
    and .outputs[0].turnId == "fixture-turn"
  ' "$run_json" >/dev/null

image_url="$(jq --raw-output '.outputs[0].imageUrl' "$run_json")"
manifest_url="$(jq --raw-output '.manifestUrl' "$run_json")"
handoff_url="$(jq --raw-output '.handoffUrl' "$run_json")"
run_id="$(jq --raw-output '.runId' "$run_json")"
image_path="$temporary_root/generated.png"
expected_image_path="$temporary_root/expected.png"
curl --fail --silent --show-error "$server_url$image_url" >"$image_path"
printf '%s' \
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=' \
  | /usr/bin/base64 -D >"$expected_image_path"
cmp "$expected_image_path" "$image_path"

manifest_path="$temporary_root/manifest.json"
handoff_path="$temporary_root/handoff.md"
curl --fail --silent --show-error "$server_url$manifest_url" >"$manifest_path"
curl --fail --silent --show-error "$server_url$handoff_url" >"$handoff_path"
jq --exit-status \
  --arg runId "$run_id" \
  '.schemaVersion == 1 and .runId == $runId and .outputs[0].status == "done"' \
  "$manifest_path" >/dev/null
rg --quiet '^# Codex Image Grid Handoff$' "$handoff_path"
rg --quiet '^## Request$' "$handoff_path"
rg --quiet '^## Diagnostics$' "$handoff_path"
rg --quiet '^## Outputs$' "$handoff_path"

for route in \
  "/api/runs/$run_id" \
  "/api/runs" \
  "/api/generated" \
  "$(jq --raw-output '.manifestViewUrl' "$run_json")" \
  "$(jq --raw-output '.handoffViewUrl' "$run_json")" \
  "/artifacts/$run_id/image?file=variant-01.png"; do
  curl --fail --silent --show-error "$server_url$route" >/dev/null
done

mcp_output="$temporary_root/mcp.jsonl"
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"native-smoke","version":"0.1.0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"generate_image_grid","arguments":{"prompts":[]}}}' \
  | "$repo_root/target/debug/image-grid-mcp" >"$mcp_output"

jq --slurp --exit-status \
  '
    length == 3
    and .[0].id == 1
    and .[0].result.protocolVersion == "2025-06-18"
    and .[0].result.serverInfo.name == "codex-image-grid-native"
    and .[0].result.capabilities.tools.listChanged == false
    and .[1].id == 2
    and (.[1].result.tools | length) == 1
    and .[1].result.tools[0].name == "generate_image_grid"
    and .[1].result.tools[0].inputSchema.required == ["prompts"]
    and .[1].result.tools[0].inputSchema.properties.prompts.maxItems == 12
    and .[1].result.tools[0].inputSchema.properties.count.maximum == 6
    and .[2] == {
      "jsonrpc": "2.0",
      "id": 3,
      "result": {
        "content": [{
          "type": "text",
          "text": "prompts array must contain at least one prompt"
        }],
        "isError": true
      }
    }
  ' "$mcp_output" >/dev/null

echo "first runnable slice smoke: ok"
