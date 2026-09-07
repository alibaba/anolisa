#!/usr/bin/env bash
set -euo pipefail

readonly CODEX_PACKAGE="@agentclientprotocol/codex-acp"
readonly CODEX_VERSION="1.2.0"
readonly CLAUDE_PACKAGE="@agentclientprotocol/claude-agent-acp"
readonly CLAUDE_VERSION="0.66.0"

usage() {
  cat >&2 <<'USAGE'
usage:
  run-acp-conformance.sh fake --gateway ABSOLUTE_BINARY --workspace ABSOLUTE_DIRECTORY
  run-acp-conformance.sh real --gateway ABSOLUTE_BINARY --workspace ABSOLUTE_DIRECTORY \
    --profile codex|claude-code --adapter ABSOLUTE_BINARY --acknowledge-provider-run

The real run reads exactly one prompt from stdin. It validates JSONL in memory
and emits only event counts; prompts and Agent text are never written to an
evidence file or echoed by this harness.
USAGE
}

[[ $# -ge 1 ]] || { usage; exit 2; }
mode="$1"
shift

gateway=""
workspace=""
profile=""
adapter=""
acknowledge_provider_run=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --gateway)
      [[ $# -ge 2 ]] || { usage; exit 2; }
      gateway="$2"
      shift 2
      ;;
    --workspace)
      [[ $# -ge 2 ]] || { usage; exit 2; }
      workspace="$2"
      shift 2
      ;;
    --profile)
      [[ $# -ge 2 ]] || { usage; exit 2; }
      profile="$2"
      shift 2
      ;;
    --adapter)
      [[ $# -ge 2 ]] || { usage; exit 2; }
      adapter="$2"
      shift 2
      ;;
    --acknowledge-provider-run)
      acknowledge_provider_run=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

[[ "$mode" == fake || "$mode" == real ]] || { usage; exit 2; }
[[ "$gateway" = /* && -f "$gateway" && -x "$gateway" ]] || {
  echo "--gateway must be an absolute executable file" >&2
  exit 2
}
[[ "$workspace" = /* && -d "$workspace" ]] || {
  echo "--workspace must be an absolute existing directory" >&2
  exit 2
}
command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }

validate_events() {
  local scenario="$1"
  python3 -c '
import json
import sys

scenario = sys.argv[1]
events = []

def terminal_safe(value):
    normalized = " ".join(value.split())[:240]
    return "".join(
        character if character.isprintable() else f"\\u{ord(character):04x}"
        for character in normalized
    )

for line in sys.stdin:
    record = json.loads(line)
    event = record.get("event")
    if not isinstance(event, str):
        raise SystemExit("COSH emitted a JSONL record without an event")
    if event == "error":
        code = record.get("code", "unknown_error")
        message = record.get("message", "no diagnostic available")
        if not isinstance(code, str):
            code = "unknown_error"
        if not isinstance(message, str):
            message = "no diagnostic available"
        raise SystemExit(
            f"COSH error [{terminal_safe(code)}]: {terminal_safe(message)}"
        )
    events.append(event)

required = {
    "doctor": ["initialized", "session_opened", "terminal", "doctor_ok"],
    "run": ["initialized", "session_opened", "session_update",
            "prompt_finished", "terminal"],
}[scenario]
cursor = 0
for expected in required:
    try:
        cursor = events.index(expected, cursor) + 1
    except ValueError:
        raise SystemExit(f"missing ordered event {expected}") from None
if events.count("terminal") != 1:
    raise SystemExit("expected exactly one terminal event")
if scenario == "run" and events.count("session_update") < 2:
    raise SystemExit("expected at least two streamed text updates")

counts = {name: events.count(name) for name in required}
print(json.dumps({"scenario": scenario, "status": "pass", "events": counts},
                 sort_keys=True, separators=(",", ":")))
' "$scenario"
}

run_doctor() {
  local selected_profile="$1"
  local selected_adapter="$2"
  "$gateway" doctor \
    --profile "$selected_profile" \
    --adapter "$selected_adapter" \
    --workspace "$workspace" \
    --output jsonl 2>/dev/null | validate_events doctor
}

run_prompt() {
  local selected_profile="$1"
  local selected_adapter="$2"
  "$gateway" run \
    --profile "$selected_profile" \
    --adapter "$selected_adapter" \
    --workspace "$workspace" \
    --output jsonl 2>/dev/null | validate_events run
}

verify_real_adapter() {
  local selected_profile="$1"
  local candidate="$2"
  local package_name expected_version command_name
  case "$selected_profile" in
    codex)
      package_name="$CODEX_PACKAGE"
      expected_version="$CODEX_VERSION"
      command_name="codex-acp"
      ;;
    claude-code)
      package_name="$CLAUDE_PACKAGE"
      expected_version="$CLAUDE_VERSION"
      command_name="claude-agent-acp"
      ;;
    *)
      echo "real mode requires --profile codex or claude-code" >&2
      exit 2
      ;;
  esac
  [[ "$candidate" = /* && "$(basename -- "$candidate")" == "$command_name" ]] || {
    echo "--adapter must be an absolute profile-matching executable" >&2
    exit 2
  }
  [[ -x "$candidate" ]] || { echo "adapter is not executable" >&2; exit 2; }

  node - "$candidate" "$package_name" "$expected_version" "$command_name" <<'NODE'
const fs = require("fs");
const path = require("path");

const [candidate, expectedName, expectedVersion, commandName] = process.argv.slice(2);
const target = fs.realpathSync(candidate);
const marker = `${path.sep}node_modules${path.sep}${expectedName}${path.sep}`;
const markerIndex = target.lastIndexOf(marker);
if (markerIndex < 0) throw new Error("adapter is not from the pinned npm package");
const packageDir = target.slice(0, markerIndex + marker.length - 1);
const manifest = JSON.parse(fs.readFileSync(path.join(packageDir, "package.json"), "utf8"));
const bin = typeof manifest.bin === "string" ? manifest.bin : manifest.bin?.[commandName];
if (manifest.name !== expectedName || manifest.version !== expectedVersion ||
    typeof bin !== "string" || fs.realpathSync(path.join(packageDir, bin)) !== target) {
  throw new Error("adapter package provenance mismatch");
}
NODE
}

if [[ "$mode" == fake ]]; then
  [[ -z "$profile" && -z "$adapter" && "$acknowledge_provider_run" == false ]] || {
    echo "fake mode does not accept real-adapter options" >&2
    exit 2
  }
  temp_root=$(mktemp -d)
  trap 'rm -rf -- "$temp_root"' EXIT
  fake_adapter="$temp_root/codex-acp"
  python_path=$(command -v python3)
  printf '#!%s\n' "$python_path" >"$fake_adapter"
  cat >>"$fake_adapter" <<'PY'
import json
import sys

session_id = "cosh-conformance-session"
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    request_id = request.get("id")
    if method == "initialize":
        result = {
            "protocolVersion": 1,
            "agentCapabilities": {},
            "agentInfo": {"name": "cosh-conformance-fake", "version": "1.0"},
        }
        print(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}), flush=True)
    elif method == "session/new":
        print(json.dumps({"jsonrpc": "2.0", "id": request_id,
                          "result": {"sessionId": session_id}}), flush=True)
    elif method == "session/prompt":
        content = request.get("params", {}).get("prompt", [])
        if len(content) != 1 or content[0].get("type") != "text" or not content[0].get("text"):
            raise SystemExit("expected one non-empty text prompt")
        for text in ("fake-first", "fake-second"):
            update = {"sessionUpdate": "agent_message_chunk",
                      "content": {"type": "text", "text": text}}
            print(json.dumps({"jsonrpc": "2.0", "method": "session/update",
                              "params": {"sessionId": session_id, "update": update}}), flush=True)
        print(json.dumps({"jsonrpc": "2.0", "id": request_id,
                          "result": {"stopReason": "end_turn"}}), flush=True)
    else:
        raise SystemExit(f"unexpected ACP method: {method}")
PY
  chmod 0700 "$fake_adapter"

  run_doctor codex "$fake_adapter"
  printf '%s\n' "deterministic fake prompt" | run_prompt codex "$fake_adapter"
  printf '%s\n' '{"profile":"fake","status":"pass","raw_output_persisted":false}'
  exit 0
fi

[[ "$acknowledge_provider_run" == true ]] || {
  echo "real mode requires --acknowledge-provider-run" >&2
  exit 2
}
[[ ! -t 0 ]] || {
  echo "pipe one explicit prompt to stdin; interactive prompt capture is disabled" >&2
  exit 2
}
command -v node >/dev/null 2>&1 || { echo "node is required" >&2; exit 1; }
verify_real_adapter "$profile" "$adapter"
run_doctor "$profile" "$adapter"
run_prompt "$profile" "$adapter"
printf '%s\n' \
  "{\"profile\":\"$profile\",\"status\":\"pass\",\"raw_output_persisted\":false}"
