#!/usr/bin/env python3
"""
Parent/child lineage driver for AgentSight crash scenarios.

The parent runs as a monitored python3 process (TestAgent, *python3* cmdline
rule) and drives one of two child shapes:

Default (sub-agent full chain, scenario agent_mode_subprocess):
    Parent sets AGENT_MODE=1 in the child environment, then execs a python3
    child (test_http_crash.py) that streams an LLM SSE request. Because the
    parent is a tracked Agent and the child's cmdline also matches the agent
    pattern, the lineage tree classifies the child as SubAgent
    (process_type="sub_agent"). scenario_test.py then kill -9s the child and
    asserts severity=high / blast_radius=partial.

--exit1 (negative pin, scenario child_exit_no_crash):
    Parent os.fork()s a child that immediately os._exit(1) — fork WITHOUT
    exec, so the child is never scanner-tracked (procmon Create fires on
    exec). Its non-zero exit is an AbnormalExit of an untracked child, which
    unified.rs deliberately does NOT report as agent_crash (only SignalCrash
    is reported for non-scanner processes). The parent then exits 0
    (NormalExit — also no record).

Markers printed to stdout (parsed by scenario_test.py):
    PID=<pid>            parent pid, after startup
    CHILD_PID=<pid>      child pid, right after fork/exec
    CHILD: <line>        forwarded child stdout (e.g. CHILD: STREAM_STARTED)
    CHILD_EXITED=<code>  child reaped (exit status, or -signal)

Python 3 stdlib only: argparse/os/subprocess/sys/time.
"""
import argparse
import os
import subprocess
import sys
import time

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
ATTACH_WAIT = 10  # settle time: scanner must track the parent as Agent first


def log(msg):
    sys.stdout.write(msg + "\n")
    sys.stdout.flush()


def run_exit1_child():
    """fork (no exec) a child that exit(1)s; parent reaps and stays alive."""
    pid = os.fork()
    if pid == 0:
        # Child: plain fork, no exec — must stay untracked by the scanner.
        os._exit(1)
    log("CHILD_PID={}".format(pid))
    _, status = os.waitpid(pid, 0)
    code = os.WEXITSTATUS(status) if os.WIFEXITED(status) else -os.WTERMSIG(status)
    log("CHILD_EXITED={}".format(code))
    # Stay alive through the scenario's negative observation window, then
    # exit 0 — NormalExit must not generate an agent_crash either.
    time.sleep(15)


def run_llm_child(api_key, base_url):
    """exec a python3 SSE child with AGENT_MODE=1 -> SubAgent lineage."""
    env = dict(os.environ)
    env["AGENT_MODE"] = "1"
    child = subprocess.Popen(
        ["python3", os.path.join(SCRIPT_DIR, "test_http_crash.py"),
         "--api-key", api_key, "--base-url", base_url],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=env,
    )
    log("CHILD_PID={}".format(child.pid))
    # Forward child markers (PID= / STREAM_STARTED / ...) until it dies.
    for raw in child.stdout:
        log("CHILD: {}".format(raw.decode("utf-8", errors="replace").strip()))
    rc = child.wait()
    log("CHILD_EXITED={}".format(rc))
    # Keep the parent Agent alive briefly so crash detection for the child
    # runs while its lineage parent still exists, then exit 0 gracefully.
    time.sleep(5)


def main():
    parser = argparse.ArgumentParser(description="parent/child lineage driver")
    parser.add_argument("--api-key", default=os.environ.get("DASHSCOPE_API_KEY", ""))
    parser.add_argument("--base-url",
                        default="https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions")
    parser.add_argument("--attach-wait", type=int, default=ATTACH_WAIT)
    parser.add_argument("--exit1", action="store_true",
                        help="fork (no exec) a child that exit(1)s (negative pin)")
    args = parser.parse_args()

    log("PID={}".format(os.getpid()))
    # The parent sends no HTTPS traffic itself; this wait is for procmon +
    # scanner to track it as Agent before the child is created, so the
    # child's classification sees a tracked Agent parent.
    time.sleep(args.attach_wait)

    if args.exit1:
        run_exit1_child()
    else:
        run_llm_child(args.api_key, args.base_url)
    sys.exit(0)


if __name__ == "__main__":
    main()
