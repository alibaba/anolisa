#!/usr/bin/env python3
"""
Killable LLM SSE client for AgentSight crash scenarios.

Spawned by scenario_test.py as a monitored python3 process (matches the
"TestAgent" *python3* cmdline rule). Timing contract:
  - Sleeps ATTACH_WAIT (>=10s) BEFORE the first HTTPS request, because the
    SSL uprobe attach chain (procmon exec -> scanner match -> ELF parse ->
    attach) takes ~8-15s; requests sent earlier are invisible to AgentSight.
  - Reads SSE chunks slowly (32 bytes / 0.5s) so the stream stays open and an
    external SIGKILL/SIGTERM always lands mid-stream.
  - Never exits on request errors: it retries forever so the pid stays
    killable and the scenario never races an already-exited process.

Markers printed to stdout (parsed by scenario_test.py):
    PID=<pid>            after startup
    ATTACH_WAIT_DONE     after the SSL-probe attach sleep
    STREAM_STARTED       response headers received, SSE body being read
    OOM_ALLOC_STARTED    (--oom only) memory hogging began
    ERROR <msg>          request failed (process stays alive and retries)

With --oom the process starts allocating 10 MiB chunks after receiving a few
SSE chunks, to be OOM-killed inside a memory-limited cgroup (scenario
oom_kill).

Python 3 stdlib only: argparse/json/os/ssl/sys/time/urllib.
"""
import argparse
import json
import os
import ssl
import sys
import time
import urllib.request

DEFAULT_URL = "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
ATTACH_WAIT = 10  # seconds before first HTTPS request (SSL probe attach)


def log(msg):
    sys.stdout.write(msg + "\n")
    sys.stdout.flush()


def stream_once(api_key, base_url, oom=False):
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
    hog = []
    while True:
        chunk = resp.read(32)
        if not chunk:
            return
        chunks += 1
        if oom and chunks >= 3:
            # cgroup OOM scenario: allocate until the kernel OOM-kills us.
            # bytearray() zero-fills, so every page is really committed.
            log("OOM_ALLOC_STARTED")
            while True:
                hog.append(bytearray(10 * 1024 * 1024))
        # Read slowly so the SSE stream stays open for the kill window
        time.sleep(0.5)


def main():
    parser = argparse.ArgumentParser(description="killable SSE client for crash scenarios")
    parser.add_argument("--api-key", default=os.environ.get("DASHSCOPE_API_KEY", ""))
    parser.add_argument("--base-url", default=DEFAULT_URL)
    parser.add_argument("--attach-wait", type=int, default=ATTACH_WAIT)
    parser.add_argument("--oom", action="store_true",
                        help="allocate memory after a few SSE chunks (cgroup OOM scenario)")
    args = parser.parse_args()

    log("PID={}".format(os.getpid()))
    time.sleep(args.attach_wait)
    log("ATTACH_WAIT_DONE")

    while True:
        try:
            stream_once(args.api_key, args.base_url, oom=args.oom)
        except Exception as e:
            log("ERROR {}".format(e))
            time.sleep(5)
        else:
            time.sleep(1)


if __name__ == "__main__":
    main()
