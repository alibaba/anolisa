#!/usr/bin/env python3
"""Mock `tokenless` binary for the adapter contract suite.

Speaks the protocol-v1 `compress` entry point. The behavior is selected by
the TOKENLESS_MOCK_BEHAVIOR environment variable:

  applied           truncate every string > 20 chars in the content to its
                    first 20 and respond `applied`
  no_savings        respond `no_savings` with the original content
  passthrough       respond `passthrough` with the original content
  error_disposition respond `error` with the original content
  timeout           sleep past every hook timeout (the hook must kill and
                    fail open)
  nonzero_exit      exit 1 without output
  malformed_stdout  print garbage instead of a protocol response

Every invocation appends its argv to the file named by TOKENLESS_MOCK_LOG
(when set) so the runner can assert the one-subprocess gate (§5.6). The
request itself is validated: a malformed request from an adapter exits
non-zero, surfacing request-construction bugs as envelope mismatches.
"""

import json
import os
import sys
import time


def truncate_strings(value):
    if isinstance(value, str):
        return value[:20] if len(value) > 20 else value
    if isinstance(value, list):
        return [truncate_strings(item) for item in value]
    if isinstance(value, dict):
        return {key: truncate_strings(item) for key, item in value.items()}
    return value


def applied_output(content: str) -> str:
    data = json.loads(content)
    if isinstance(data, str):
        data = json.loads(data)
    return json.dumps(truncate_strings(data), separators=(",", ":"), ensure_ascii=False)


def respond(output: str, disposition: str, seam: str) -> None:
    chain = []
    if disposition == "applied":
        chain = ["schema-compress"] if seam == "before_model" else ["response-cleanup"]
    print(json.dumps({
        "protocol_version": 1,
        "output": output,
        "disposition": disposition,
        "compressor_chain": chain,
        "reversibility": "lossless",
        "before_tokens": 100,
        "after_tokens": 50 if disposition == "applied" else 100,
        "stash_keys": [],
        "tokenizer_id": "heuristic-v1",
    }))


def main() -> int:
    log_path = os.environ.get("TOKENLESS_MOCK_LOG")
    if log_path:
        with open(log_path, "a") as log:
            log.write(" ".join(sys.argv[1:]) + "\n")

    behavior = os.environ.get("TOKENLESS_MOCK_BEHAVIOR", "applied")
    if sys.argv[1:] != ["compress"]:
        return 2
    raw = sys.stdin.read()

    if behavior == "timeout":
        time.sleep(60)
        return 0
    if behavior == "nonzero_exit":
        return 1
    if behavior == "malformed_stdout":
        print("this is not a protocol response")
        return 0

    request = json.loads(raw)
    if (
        request.get("protocol_version") != 1
        or "capabilities" not in request
        or "seam" not in request
        or not isinstance(request.get("content"), str)
    ):
        return 3
    content = request["content"]
    seam = request["seam"]

    if behavior == "applied":
        respond(applied_output(content), "applied", seam)
    elif behavior == "no_savings":
        respond(content, "no_savings", seam)
    elif behavior == "passthrough":
        respond(content, "passthrough", seam)
    elif behavior == "error_disposition":
        respond(content, "error", seam)
    else:
        return 4
    return 0


if __name__ == "__main__":
    sys.exit(main())
