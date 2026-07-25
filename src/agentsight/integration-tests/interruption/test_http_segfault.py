#!/usr/bin/env python3
"""
Self-crashing LLM SSE client: SIGSEGV mid-stream.

Spawned by scenario_test.py as a monitored python3 process. After receiving a
few SSE chunks of a streaming LLM response, it dereferences NULL via
ctypes.string_at(0), which raises a real SIGSEGV in the C layer (no Python
exception, no faulthandler) — the kernel terminates the process with
signal 11 while the HTTPS stream is still open.

Timing contract mirrors test_http_crash.py: sleeps >=10s before the first
request so the SSL uprobe is attached (see README "SSL probe attach 时序").

Markers printed to stdout (parsed by scenario_test.py):
    PID=<pid>            after startup
    ATTACH_WAIT_DONE     after the SSL-probe attach sleep
    STREAM_STARTED       response headers received
    SEGFAULT_NOW         about to dereference NULL (last line ever printed)
    ERROR <msg>          request failed (retries so the crash still happens)

Python 3 stdlib only: argparse/ctypes/json/os/ssl/sys/time/urllib.
"""
import argparse
import ctypes
import json
import os
import ssl
import sys
import time
import urllib.request

DEFAULT_URL = "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
ATTACH_WAIT = 10  # seconds before first HTTPS request (SSL probe attach)
CHUNKS_BEFORE_CRASH = 3


def log(msg):
    sys.stdout.write(msg + "\n")
    sys.stdout.flush()


def stream_and_crash(api_key, base_url):
    payload = json.dumps({
        "model": "qwen-max",
        "messages": [{"role": "user", "content":
                      "Write a detailed 3000 word essay about the history of "
                      "computing from 1940 to 2025."}],
        "max_tokens": 4000,
        "stream": True,
    }).encode("utf-8")
    headers = {
        "Content-Type": "application/json",
        "Authorization": "Bearer {}".format(api_key),
    }
    req = urllib.request.Request(base_url, data=payload, headers=headers, method="POST")
    ctx = ssl.create_default_context()
    resp = urllib.request.urlopen(req, context=ctx, timeout=120)
    log("STREAM_STARTED")
    chunks = 0
    while True:
        chunk = resp.read(32)
        if not chunk:
            return
        chunks += 1
        if chunks >= CHUNKS_BEFORE_CRASH:
            log("SEGFAULT_NOW")
            # NULL dereference in C: kernel delivers SIGSEGV, process dies
            # with signal 11 while the SSE stream is still in flight.
            ctypes.string_at(0)
        time.sleep(0.5)


def main():
    parser = argparse.ArgumentParser(description="SSE client that segfaults mid-stream")
    parser.add_argument("--api-key", default=os.environ.get("DASHSCOPE_API_KEY", ""))
    parser.add_argument("--base-url", default=DEFAULT_URL)
    parser.add_argument("--attach-wait", type=int, default=ATTACH_WAIT)
    args = parser.parse_args()

    log("PID={}".format(os.getpid()))
    time.sleep(args.attach_wait)
    log("ATTACH_WAIT_DONE")

    # Retry until the request goes through — the whole point is to die by
    # SIGSEGV mid-stream, so we never give up on transient request errors.
    while True:
        try:
            stream_and_crash(args.api_key, args.base_url)
        except Exception as e:
            log("ERROR {}".format(e))
            time.sleep(5)
        else:
            # Stream completed without reaching the crash threshold; retry.
            time.sleep(1)


if __name__ == "__main__":
    main()
