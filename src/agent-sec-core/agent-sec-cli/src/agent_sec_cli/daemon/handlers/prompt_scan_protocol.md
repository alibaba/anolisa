# scan-prompt daemon protocol

This document defines the response contract for the daemon `scan-prompt` method.
It is the method-level contract for callers such as the CLI, Cosh hooks, and
other subprocess clients.

## Response layers

`scan-prompt` responses have three layers. Callers must handle them in this
order.

### 1. Transport failure: no `DaemonResponse`

The client did not receive a valid daemon response.

Examples:

- daemon socket does not exist
- daemon connection or request timeout
- daemon process exits before writing a response
- daemon response is not valid protocol data
- daemon response exceeds the configured size limit
- daemon runtime path cannot be resolved, for example when `XDG_RUNTIME_DIR` is
  not set and no explicit daemon socket path is provided

Caller behavior:

```python
try:
    response = client.call("scan-prompt", params=params)
except (DaemonClientError, DaemonRuntimePathError):
    # Daemon is unreachable, the protocol is broken, or the local runtime
    # socket path cannot be resolved.
    exit(1)
```

### 2. Daemon failure: `ok=false`

The daemon received the request, but the method could not be dispatched or
executed at the daemon/method boundary.

`ok=false` responses are not scan results. Callers must not parse `data` or
`stdout` as an action result.

Expected shape:

```json
{
  "request_id": "4f56f7b6-0c77-4a71-a6b0-708f6a4f7ec7",
  "ok": false,
  "data": {},
  "stdout": "",
  "stderr": "daemon request timed out after 30000 ms",
  "exit_code": 1,
  "error": {
    "code": "timeout",
    "message": "daemon request timed out after 30000 ms"
  }
}
```

`scan-prompt` daemon failures include:

- unknown daemon method
- malformed daemon request
- daemon method timeout
- unexpected handler crash

Unexpected handler crashes should be logged by the daemon and returned as
`internal_error` without exposing arbitrary exception details to callers.

Caller behavior:

```python
if not response.ok:
    echo_error(response.stderr or response.error["message"])
    exit(response.exit_code or 1)
```

### 3. Action result: `ok=true`

The daemon successfully dispatched `scan-prompt`, and the handler returned a
scan action result.

For `ok=true`, `exit_code` is the action/CLI semantic exit code. It may be
non-zero even though daemon dispatch succeeded.

Expected successful scan shape:

```json
{
  "request_id": "4f56f7b6-0c77-4a71-a6b0-708f6a4f7ec7",
  "ok": true,
  "data": {
    "ok": true,
    "verdict": "pass"
  },
  "stdout": "{...same scan result as JSON...}",
  "stderr": "",
  "exit_code": 0
}
```

Expected scanner error result shape:

```json
{
  "request_id": "4f56f7b6-0c77-4a71-a6b0-708f6a4f7ec7",
  "ok": true,
  "data": {
    "ok": false,
    "verdict": "error",
    "summary": "Scanner error: model exploded"
  },
  "stdout": "{...same error verdict as JSON...}",
  "stderr": "Scanner error: model exploded",
  "exit_code": 1
}
```

`scan-prompt` action results include:

- `PASS`, `WARN`, and `DENY` scan verdicts: `ok=true`, `exit_code=0`
- backend validation failures, such as missing or empty `text`:
  `ok=true`, `exit_code=1`, with `stderr` describing the validation error
- scanner-produced `ERROR` verdicts: `ok=true`, `exit_code=1`, with structured
  error verdict data
- scanner domain exceptions that can be converted to an error verdict:
  `ok=true`, `exit_code=1`, with structured error verdict data
- `standard` or `strict` requests made while the model is not ready run in
  `fast` mode and return `ok=true` with `degraded=true` and a
  `degraded_reason`; a degraded `deny` is rewritten to `warn` while the
  original verdict is retained in `degraded_original_verdict`

Caller behavior:

```python
if response.ok:
    rendered = render_action_output_if_present(response)
    if response.exit_code != 0:
        if not rendered:
            echo_error(response.stderr or "scan-prompt failed")
        exit(response.exit_code)
    exit(0)
```

Callers should render structured action output before exiting with a non-zero
action `exit_code`, so JSON consumers can still parse the error verdict. If an
action failure has no structured output, callers should display `stderr`.

## Prompt model preload state machine

`daemon.health.data.prompt_scan.status` starts as `pending`. The one-shot
preload job tries the local model load and probe before starting any download:

```text
pending -> loading -- success ------------------------> ready
              |
              +-- failure --> downloading
                                |
                                +-- failure --> degraded
                                |
                                +-- success --> loading (retry once)
                                                 |
                                                 +-- success --> ready
                                                 +-- failure --> degraded

active state -- cancellation --> stopped
```

On a cache hit, the state therefore moves directly from `loading` to `ready`
and no download child is started. Any ordinary exception from the first local
load/probe attempt triggers exactly one download/repair child and one
load/probe retry, preserving the previous recovery opportunity without adding
an unbounded retry loop. A child failure or retry failure moves the prompt
runtime to `degraded`; the daemon process remains available and model-dependent
scans use the `fast` fallback described above. Cancellation is not treated as a
repairable failure. The one-shot job does not retry again until the daemon is
restarted.

## Request parameters

`scan-prompt` request params:

```json
{
  "text": "prompt text to scan",
  "mode": "fast|standard|strict",
  "source": "optional input source label"
}
```

Rules:

- `text` is required and must contain non-whitespace content.
- `mode` is optional and defaults to `standard`.
- `mode` must be one of `fast`, `standard`, or `strict`.
- `source` is optional and defaults to an empty string.

Missing or empty `text` is handled by the prompt scan backend and returned as
an action failure: `ok=true`, `exit_code=1`. An unsupported `mode` is rejected
at the daemon boundary as `ok=false` with error code `bad_request`.
