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
analysis_reference_capture="$temporary_root/analysis-reference-capture.jpeg"

printf '%s\n' \
  '#!/bin/sh' \
  'test "$1" = "app-server" || exit 2' \
  'while IFS= read -r line; do' \
  '  case "$line" in' \
  '    *'"'"'"method":"initialize"'"'"'*)' \
  '      request_id="$(printf "%s" "$line" | jq --raw-output ".id")"' \
  '      printf '"'"'{"id":%s,"result":{"userAgent":"fixture","codexHome":"/tmp/fixture","platformFamily":"unix","platformOs":"macos"}}\n'"'"' "$request_id"' \
  '      ;;' \
  '    *'"'"'"method":"initialized"'"'"'*)' \
  '      ;;' \
  '    *'"'"'"method":"thread/start"'"'"'*)' \
  '      request_id="$(printf "%s" "$line" | jq --raw-output ".id")"' \
  '      service_name="$(printf "%s" "$line" | jq --raw-output ".params.serviceName // empty")"' \
  '      if [ "$service_name" = "codex_image_grid_reference_analysis" ]; then' \
  '        printf '"'"'{"id":%s,"result":{"thread":{"id":"analysis-thread"}}}\n'"'"' "$request_id"' \
  '      else' \
  '        printf '"'"'{"id":%s,"result":{"thread":{"id":"fixture-thread"}}}\n'"'"' "$request_id"' \
  '      fi' \
  '      ;;' \
  '    *'"'"'"method":"turn/start"'"'"'*)' \
  '      request_id="$(printf "%s" "$line" | jq --raw-output ".id")"' \
  '      thread_id="$(printf "%s" "$line" | jq --raw-output ".params.threadId // empty")"' \
  '      if [ "$thread_id" = "analysis-thread" ]; then' \
  '        local_image_path="$(printf "%s" "$line" | jq --raw-output '"'"'.params.input[] | select(.type == "localImage") | .path'"'"' | head -n 1)"' \
  '        cp "$local_image_path" "$IMAGE_GRID_SMOKE_ANALYSIS_CAPTURE"' \
  '        printf '"'"'{"id":%s,"result":{"turn":{"id":"analysis-turn"}}}\n'"'"' "$request_id"' \
  '        printf '"'"'%s\n'"'"' '"'"'{"method":"item/completed","params":{"threadId":"analysis-thread","turnId":"analysis-turn","item":{"type":"agentMessage","id":"analysis-message","text":"- 青いマスコット\n- 柔らかな光"}}}'"'"'' \
  '        printf '"'"'%s\n'"'"' '"'"'{"method":"turn/completed","params":{"threadId":"analysis-thread","turn":{"id":"analysis-turn","items":[],"itemsView":"full","status":"completed","error":null,"startedAt":null,"completedAt":null,"durationMs":1}}}'"'"'' \
  '      else' \
  '        printf '"'"'{"id":%s,"result":{"turn":{"id":"fixture-turn"}}}\n'"'"' "$request_id"' \
  '        printf '"'"'%s\n'"'"' '"'"'{"method":"item/completed","params":{"threadId":"fixture-thread","turnId":"fixture-turn","item":{"type":"imageGeneration","id":"fixture-image","status":"completed","revisedPrompt":null,"result":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="}}}'"'"'' \
  '        printf '"'"'%s\n'"'"' '"'"'{"method":"turn/completed","params":{"threadId":"fixture-thread","turn":{"id":"fixture-turn","items":[],"itemsView":"full","status":"completed","error":null,"startedAt":null,"completedAt":null,"durationMs":1}}}'"'"'' \
  '      fi' \
  '      ;;' \
  '  esac' \
  'done' \
  >"$fake_codex"
chmod 755 "$fake_codex"

expected_image_path="$temporary_root/expected.png"
reference_image="$temporary_root/reference.jpeg"
printf '%s' \
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=' \
  | /usr/bin/base64 -D >"$expected_image_path"
cp "$expected_image_path" "$reference_image"

IMAGE_GRID_SMOKE_ANALYSIS_CAPTURE="$analysis_reference_capture" \
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
    and .app == "codex-image-grid"
    and .serverRoot == $serverRoot
    and .dataDir == $dataDir
    and .generatedDir == ($dataDir + "/generated")
    and .runDir == ($dataDir + "/.run")
    and .workspaceDir == $dataDir
    and .launchTarget == "server"
    and .packageName == "codex-image-grid"
    and .packageVersion == "0.2.2"
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

analysis_json="$temporary_root/analysis.json"
analysis_request="$temporary_root/analysis-request.json"
jq --null-input \
  --arg referenceImagePath "$reference_image" \
  '{referenceImagePath: $referenceImagePath}' \
  >"$analysis_request"
curl --fail --silent --show-error \
  --request POST \
  --header 'content-type: application/json' \
  --data-binary @"$analysis_request" \
  "$server_url/api/analyze-reference" \
  >"$analysis_json"
jq --exit-status \
  '.premise == "- 青いマスコット\n- 柔らかな光"' \
  "$analysis_json" >/dev/null
cmp "$reference_image" "$analysis_reference_capture"
analysis_staging_root="$data_root/.run/reference-analysis"
if [[ -d "$analysis_staging_root" ]] \
  && [[ -n "$(find "$analysis_staging_root" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
  echo "reference analysis staging directory was not cleaned up" >&2
  exit 1
fi

run_json="$temporary_root/run.json"
run_request="$temporary_root/run-request.json"
jq --null-input \
  --arg referenceImagePath "$reference_image" \
  '{
    prompts: ["fixture prompt"],
    count: 1,
    mood: "warm-mascot",
    engine: "app-server-image",
    aspectRatio: "16:9",
    referenceImagePath: $referenceImagePath,
    waitMs: 5000
  }' \
  >"$run_request"
curl --fail --silent --show-error \
  --request POST \
  --header 'content-type: application/json' \
  --data-binary @"$run_request" \
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
    and (.outputs[0].referenceImagePath | endswith("/reference.jpg"))
    and (.outputs[0].referenceImageUrl | endswith("/reference.jpg"))
  ' "$run_json" >/dev/null

image_url="$(jq --raw-output '.outputs[0].imageUrl' "$run_json")"
manifest_url="$(jq --raw-output '.manifestUrl' "$run_json")"
handoff_url="$(jq --raw-output '.handoffUrl' "$run_json")"
run_id="$(jq --raw-output '.runId' "$run_json")"
image_path="$temporary_root/generated.png"
curl --fail --silent --show-error "$server_url$image_url" >"$image_path"
cmp "$expected_image_path" "$image_path"
cmp "$reference_image" "$data_root/generated/$run_id/reference.jpg"

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

mcp_input="$temporary_root/mcp-input.jsonl"
mcp_output="$temporary_root/mcp-output.jsonl"
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"native-smoke","version":"0.1.0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"generate_image_grid","arguments":{"prompts":[]}}}' \
  >"$mcp_input"
jq --compact-output --null-input \
  --arg referenceImagePath "$reference_image" \
  '{
    jsonrpc: "2.0",
    id: 4,
    method: "tools/call",
    params: {
      name: "generate_image_grid",
      arguments: {
        prompts: ["MCP fixture prompt"],
        count: 1,
        mood: "warm-mascot",
        engine: "app-server-image",
        aspectRatio: "16:9",
        referenceImagePath: $referenceImagePath,
        waitMs: 5000
      }
    }
  }' \
  >>"$mcp_input"
IMAGE_GRID_URL="$server_url" \
  "$repo_root/target/debug/image-grid-mcp" <"$mcp_input" >"$mcp_output"

jq --slurp --exit-status \
  --arg serverUrl "$server_url" \
  '
    length == 4
    and .[0].id == 1
    and .[0].result.protocolVersion == "2025-06-18"
    and .[0].result.serverInfo.name == "codex-image-grid"
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
    and .[3].id == 4
    and .[3].result.isError == false
    and .[3].result.structuredContent.status == "done"
    and .[3].result.structuredContent.completed == true
    and .[3].result.structuredContent.serverStarted == false
    and .[3].result.structuredContent.health.app == "codex-image-grid"
    and .[3].result.structuredContent.health.appServerImageReady == true
    and .[3].result.structuredContent.server.app == "codex-image-grid"
    and .[3].result.structuredContent.statusUrl
      == ($serverUrl + "/api/runs/" + .[3].result.structuredContent.runId)
    and (.[3].result.structuredContent.outputPaths | length) == 1
    and (.[3].result.structuredContent.imageUrls | length) == 1
    and (.[3].result.structuredContent.imageUrls[0] | startswith($serverUrl + "/generated/"))
    and (.[3].result.structuredContent.codexMarkdown | contains($serverUrl + "/generated/"))
    and (.[3].result.structuredContent.outputs[0].referenceImagePath
      | endswith("/reference.jpg"))
    and (.[3].result.content[0].text | contains("runId: "))
  ' "$mcp_output" >/dev/null

mcp_run_id="$(jq --slurp --raw-output '.[3].result.structuredContent.runId' "$mcp_output")"
mcp_image_url="$(jq --slurp --raw-output '.[3].result.structuredContent.imageUrls[0]' "$mcp_output")"
mcp_image_path="$temporary_root/mcp-generated.png"
curl --fail --silent --show-error "$mcp_image_url" >"$mcp_image_path"
cmp "$expected_image_path" "$mcp_image_path"
cmp "$reference_image" "$data_root/generated/$mcp_run_id/reference.jpg"

echo "first runnable slice smoke: ok"
