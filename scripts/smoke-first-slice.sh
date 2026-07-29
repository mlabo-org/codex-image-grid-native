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

"$repo_root/target/debug/image-grid-server" \
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
