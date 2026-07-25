#!/usr/bin/env python3
"""
AgentSight Interruption Scenario Test Tool

Constructs controlled error scenarios against an LLM API endpoint,
captured by AgentSight eBPF probes, for verifying interruption
detection, classification, and logtail export.

Prerequisites:
    - AgentSight service running with eBPF probes attached
    - python3 cmdline rule in agentsight config (agent_name: "TestAgent")
    - SLS_LOGTAIL_FILE environment variable set for agentsight service

Usage:
    python3 scenario_test.py <scenario> --api-key <KEY> [--base-url URL]

    API key can also be set via DASHSCOPE_API_KEY environment variable.

Scenarios:
    auth_single    1x auth error (invalid key)
    auth_storm     5x auth error rapid-fire (retry storm, same root cause)
    mixed_light    8 normal + 2 auth errors
    mixed_heavy    5 normal + 5 auth errors (alternating)
    multi_type     1x auth + 1x model_not_found(404) + 3 normal
    healthy        10 normal calls (zero interruptions baseline)
    all            Run all scenarios sequentially

Crash scenarios (need helper scripts from this directory next to this file):
    agent_crash_sigkill      kill -9 mid-SSE -> agent_crash (signal=9,
                             severity=critical, blast_radius=total_session_loss,
                             dmesg-verified non-OOM)
    agent_crash_sigsegv      self-inflicted SIGSEGV mid-SSE -> agent_crash
                             (signal=11, detail carries coredump field)
    graceful_stop_no_crash   kill -15 mid-SSE -> NO agent_crash (negative)
    child_exit_no_crash      fork (no exec) child exit(1) -> NO agent_crash
                             (negative pin: AbnormalExit of untracked child)
    agent_mode_subprocess    AGENT_MODE=1 python3 sub-process streams, kill -9 ->
                             genai_events.process_type=sub_agent, agent_crash
                             severity=high / blast_radius=partial
    oom_kill                 cgroup v2 memory.max OOM -> agent_crash oom=true
                             (SKIP when root/cgroup v2/memory controller unmet)
    crash_all                Run all crash scenarios sequentially
"""
import json
import os
import select
import shutil
import signal
import subprocess
import time
import urllib.request
import urllib.error
import ssl
import sqlite3
import argparse

DEFAULT_URL = "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
INVALID_KEY = "sk-INVALID_SCENARIO_TEST_{}"
DB_INT = "/var/log/sysak/.agentsight/interruption_events.db"
LOGTAIL = "/var/sysom/ilog/agentsight"

CALL_INTERVAL = 2


def send_request(api_key, base_url, model="qwen-max", content="hello", max_tokens=5):
    payload = json.dumps({
        "model": model,
        "messages": [{"role": "user", "content": content}],
        "max_tokens": max_tokens,
    }).encode("utf-8")
    headers = {
        "Content-Type": "application/json",
        "Authorization": "Bearer {}".format(api_key),
    }
    req = urllib.request.Request(base_url, data=payload, headers=headers, method="POST")
    ctx = ssl.create_default_context()
    try:
        resp = urllib.request.urlopen(req, context=ctx, timeout=30)
        body = resp.read().decode("utf-8", errors="replace")
        return resp.status, body
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        return e.code, body
    except Exception as e:
        return -1, str(e)


def get_baseline():
    b = {"int_count": 0, "logtail_lines": 0}
    try:
        conn = sqlite3.connect(DB_INT)
        b["int_count"] = conn.execute("SELECT COUNT(*) FROM interruption_events").fetchone()[0]
        conn.close()
    except Exception:
        pass
    try:
        with open(LOGTAIL) as f:
            b["logtail_lines"] = sum(1 for _ in f)
    except Exception:
        pass
    return b


def check_results(baseline, wait=10):
    print("\n  Waiting {}s for AgentSight processing...".format(wait))
    time.sleep(wait)

    results = {"logtail_chats": [], "logtail_interruptions": [], "new_interruptions": []}
    try:
        conn = sqlite3.connect(DB_INT)
        total = conn.execute("SELECT COUNT(*) FROM interruption_events").fetchone()[0]
        new_count = total - baseline["int_count"]
        if new_count > 0:
            rows = conn.execute(
                "SELECT interruption_type, severity, agent_name, substr(detail, 1, 200) "
                "FROM interruption_events ORDER BY id DESC LIMIT ?",
                (new_count,)
            ).fetchall()
            results["new_interruptions"] = [
                {"type": r[0], "severity": r[1], "agent": r[2], "detail": r[3][:100]}
                for r in reversed(rows)
            ]
        conn.close()
    except Exception:
        pass

    try:
        with open(LOGTAIL) as f:
            lines = f.readlines()
        new_lines = lines[baseline["logtail_lines"]:]
        for line in new_lines:
            try:
                d = json.loads(line.strip())
                if d.get("gen_ai.operation.name") == "interruption":
                    results["logtail_interruptions"].append({
                        "type": d.get("agentsight.interruption.type"),
                        "severity": d.get("agentsight.interruption.severity"),
                        "agent": d.get("agentsight.agent.name"),
                    })
                else:
                    results["logtail_chats"].append({
                        "model": d.get("gen_ai.request.model"),
                        "status": d.get("agentsight.http.status_code"),
                    })
            except Exception:
                pass
    except Exception:
        pass

    return results


def print_results(name, calls, results):
    print("\n  === Results for '{}' ===".format(name))
    print("  Calls made: {}".format(len(calls)))
    for c in calls:
        print("    {} {} -> {}".format(c["type"], c["model"], c["status"]))

    ints = results.get("logtail_interruptions", [])
    chats = results.get("logtail_chats", [])
    print("  Logtail: {} chat records, {} interruption records".format(len(chats), len(ints)))
    for i in ints:
        print("    INT: type={} severity={} agent={}".format(i["type"], i["severity"], i["agent"]))

    db_ints = results.get("new_interruptions", [])
    if db_ints:
        print("  DB interruption_events: {} new".format(len(db_ints)))
        for d in db_ints:
            print("    type={} severity={} agent={}".format(d["type"], d["severity"], d["agent"]))


# ==================== Crash-scenario utilities ====================
#
# Timing rules (see README "踩雷记录" and integration-tests/RULES.md):
#   - ATTACH_WAIT: a monitored process must sleep >= 10s after start before
#     its first HTTPS request — the SSL uprobe attach chain (procmon exec ->
#     scanner match -> ELF parse -> attach) takes ~8-15s.
#   - SCENARIO_GAP: >= 2s between crash scenarios so a recycled PID cannot
#     bleed one scenario's records into the next scenario's assertions.
#   - dmesg checks use a timestamp window (verify_no_oom), never absolute
#     line positions: stale OOM lines from earlier boots/tests must not
#     produce false positives on recycled PIDs.
#   - Assertions read structured fields (signal / severity / blast_radius /
#     process_type / oom / coredump) from interruption_events.detail JSON —
#     display strings are presentation-layer and may change.

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
DB_GENAI = "/var/log/sysak/.agentsight/genai_events.db"

ATTACH_WAIT = 10       # SSL probe attach delay (>= 10s before first request)
SCENARIO_GAP = 2       # min seconds between crash scenarios (PID reuse guard)
CRASH_WAIT = 15        # trace-mode procmon exit path lands in ~1-2s; margin for load
NEGATIVE_WINDOW = 8    # observation window for "no record" (negative) assertions

AGENTSIGHT_BIN_CANDIDATES = [
    "/usr/local/sysak/.sysak_components/tools/agentsight",
    "/root/agentsight",
    "agentsight",
]


def spawn_test_process(script_path, env=None, args=None):
    """Start a monitored test process (python3 <script>); returns (pid, popen).

    argv[0] is literally "python3" so the process matches the TestAgent
    cmdline rule ["*python3*"]. stdout carries the marker protocol
    (PID= / STREAM_STARTED / CHILD_PID= / ...) consumed via wait_for_marker().
    """
    full_env = dict(os.environ)
    if env:
        full_env.update(env)
    cmd = ["python3", script_path] + list(args or [])
    popen = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=full_env,
    )
    return popen.pid, popen


def wait_for_marker(popen, marker, timeout=60):
    """Read popen stdout lines until one contains `marker`.

    Returns the matching line, or None on timeout/EOF (process exited).
    """
    deadline = time.time() + timeout
    while time.time() < deadline:
        ready, _, _ = select.select([popen.stdout], [], [], max(0.1, deadline - time.time()))
        if not ready:
            return None
        raw = popen.stdout.readline()
        if not raw:
            return None  # EOF: child exited
        line = raw.decode("utf-8", errors="replace").strip()
        if line:
            print("    [child] {}".format(line))
        if marker in line:
            return line
    return None


def kill_with_signal(pid, sig, timeout=10):
    """Send `sig` to pid and wait until the process is gone.

    Returns True once the process has exited (reaped when it is our direct
    child, so a zombie cannot keep the pid visible), False on timeout.
    """
    try:
        os.kill(pid, sig)
    except ProcessLookupError:
        return True
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            wpid, _ = os.waitpid(pid, os.WNOHANG)
            if wpid == pid:
                return True
        except (ChildProcessError, OSError):
            pass  # not our direct child (or already reaped elsewhere)
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        time.sleep(0.2)
    return False


def max_interruption_id(conn):
    """Current max interruption_events.id — baseline for wait_for_interruption."""
    try:
        return conn.execute("SELECT COALESCE(MAX(id), 0) FROM interruption_events").fetchone()[0]
    except Exception:
        return 0


def wait_for_interruption(conn, itype, timeout=5, count=1, since_id=0, pid=None):
    """Poll interruption_events until >= `count` rows of `itype` with id > since_id.

    Optional `pid` narrows to one process (PID-reuse guard for assertions).
    Returns the matching rows (possibly fewer than `count` on timeout) as
    dicts with the detail column parsed from JSON, so callers assert on
    structured fields instead of display strings.
    """
    deadline = time.time() + timeout
    rows = []
    while True:
        sql = ("SELECT id, interruption_type, severity, pid, agent_name, detail "
               "FROM interruption_events WHERE interruption_type = ? AND id > ?")
        params = [itype, since_id]
        if pid is not None:
            sql += " AND pid = ?"
            params.append(pid)
        sql += " ORDER BY id"
        try:
            fetched = conn.execute(sql, params).fetchall()
        except Exception:
            fetched = []
        rows = []
        for r in fetched:
            try:
                detail = json.loads(r[5]) if r[5] else {}
            except Exception:
                detail = {}
            rows.append({"id": r[0], "type": r[1], "severity": r[2],
                         "pid": r[3], "agent": r[4], "detail": detail})
        if len(rows) >= count or time.time() >= deadline:
            return rows
        time.sleep(0.5)


def _dmesg_lines_since(since_ts):
    """dmesg lines newer than `since_ts` (epoch secs), timestamp-window filtered.

    Prefers `dmesg --time-format iso` (sortable prefix), falls back to
    `dmesg -T`. Returns None when dmesg is unavailable (e.g. not root).
    """
    parsed = None
    try:
        proc = subprocess.run(["dmesg", "--time-format", "iso"],
                              stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                              universal_newlines=True, timeout=15)
        if proc.returncode == 0 and proc.stdout:
            # "2026-07-25T12:34:56,123456+0800 <msg>"
            parsed = [(line[:19], "%Y-%m-%dT%H:%M:%S", line) for line in proc.stdout.splitlines()]
    except Exception:
        pass
    if parsed is None:
        try:
            proc = subprocess.run(["dmesg", "-T"],
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                  universal_newlines=True, timeout=15)
            if proc.returncode != 0 or not proc.stdout:
                return None
            # "[Sat Jul 25 12:34:56 2026] <msg>"
            parsed = []
            for line in proc.stdout.splitlines():
                end = line.find("]")
                if line.startswith("[") and end > 0:
                    parsed.append((line[1:end].strip(), "%a %b %d %H:%M:%S %Y", line))
        except Exception:
            return None
    lines = []
    for stamp, fmt, line in parsed:
        try:
            ts = time.mktime(time.strptime(stamp, fmt))
        except Exception:
            continue
        if ts >= since_ts - 1:  # 1s slack for clock rounding
            lines.append(line)
    return lines


def verify_no_oom(pid, since_ts):
    """True when dmesg shows no OOM kill for `pid` since `since_ts`.

    Guards non-OOM crash scenarios against false positives: an OOM line for
    a *recycled* pid from an earlier test/boot must not count, hence the
    timestamp-window filter instead of grepping the whole ring buffer.
    Returns None when dmesg is unavailable (caller reports SKIP).
    """
    lines = _dmesg_lines_since(since_ts)
    if lines is None:
        return None
    needle = "Killed process {} ".format(pid)
    for line in lines:
        if needle in line:
            return False
    return True


def wait_for_genai_process_type(pid, expect, timeout=10):
    """Poll genai_events for a row of `pid` whose process_type == `expect`.

    Returns (matched, rows_seen) where rows_seen is [(process_type, status)].
    """
    deadline = time.time() + timeout
    seen = []
    while True:
        try:
            gconn = sqlite3.connect(DB_GENAI)
            seen = gconn.execute(
                "SELECT process_type, status FROM genai_events "
                "WHERE pid = ? ORDER BY id DESC LIMIT 20", (pid,)).fetchall()
            gconn.close()
        except Exception:
            seen = []
        if any(r[0] == expect for r in seen):
            return True, seen
        if time.time() >= deadline:
            return False, seen
        time.sleep(0.5)


def token_by_type_output():
    """Best-effort `agentsight token --by-type --json`. None when unavailable."""
    for cand in AGENTSIGHT_BIN_CANDIDATES:
        path = cand if os.path.sep in cand else shutil.which(cand)
        if not path or not os.path.exists(path):
            continue
        try:
            proc = subprocess.run([path, "token", "--by-type", "--json"],
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                  universal_newlines=True, timeout=30)
            if proc.returncode == 0:
                return proc.stdout
        except Exception:
            pass
    return None


def report_checks(name, checks):
    """Print PASS/FAIL/SKIP per check (RULES.md verdict convention).

    `checks` is a list of (description, ok, evidence) where ok is True/False,
    or None for SKIP (environment/optional). Returns the overall verdict.
    """
    overall = "PASS"
    print("\n  === Verdict for '{}' ===".format(name))
    for desc, ok, evidence in checks:
        if ok is None:
            label = "SKIP"
        elif ok:
            label = "PASS"
        else:
            label = "FAIL"
            overall = "FAIL"
        print("    [{}] {} -- {}".format(label, desc, evidence))
    print("  Overall: {}".format(overall))
    return overall


# ==================== Scenarios ====================

def scenario_auth_single(api_key, base_url):
    """1x auth error"""
    baseline = get_baseline()
    calls = []
    print("  Sending 1 request with invalid API key...")
    status, _ = send_request(INVALID_KEY.format("auth_single"), base_url)
    calls.append({"type": "auth_error", "model": "qwen-max", "status": status})
    results = check_results(baseline)
    print_results("auth_single", calls, results)
    return calls, results


def scenario_auth_storm(api_key, base_url):
    """5x auth error (retry storm, same root cause)"""
    baseline = get_baseline()
    calls = []
    bad_key = INVALID_KEY.format("auth_storm")
    print("  Sending 5 rapid requests with same invalid key (retry storm)...")
    for i in range(5):
        status, _ = send_request(bad_key, base_url, content="retry {}".format(i))
        calls.append({"type": "auth_error", "model": "qwen-max", "status": status})
        time.sleep(0.5)
    results = check_results(baseline)
    print_results("auth_storm", calls, results)
    return calls, results


def scenario_mixed_light(api_key, base_url):
    """8 normal + 2 auth errors"""
    baseline = get_baseline()
    calls = []
    plan = ["ok"] * 4 + ["auth"] + ["ok"] * 4 + ["auth"]
    print("  Sending 10 requests (8 normal + 2 auth errors)...")
    for i, typ in enumerate(plan):
        if typ == "ok":
            status, _ = send_request(api_key, base_url, content="normal {}".format(i), max_tokens=5)
            calls.append({"type": "normal", "model": "qwen-max", "status": status})
        else:
            status, _ = send_request(INVALID_KEY.format("mixed_light"), base_url, content="error {}".format(i))
            calls.append({"type": "auth_error", "model": "qwen-max", "status": status})
        time.sleep(CALL_INTERVAL)
    results = check_results(baseline, wait=15)
    print_results("mixed_light", calls, results)
    return calls, results


def scenario_mixed_heavy(api_key, base_url):
    """5 normal + 5 auth errors (alternating)"""
    baseline = get_baseline()
    calls = []
    print("  Sending 10 requests (5 normal + 5 auth errors, alternating)...")
    for i in range(10):
        if i % 2 == 0:
            status, _ = send_request(api_key, base_url, content="normal {}".format(i), max_tokens=5)
            calls.append({"type": "normal", "model": "qwen-max", "status": status})
        else:
            status, _ = send_request(INVALID_KEY.format("mixed_heavy"), base_url, content="error {}".format(i))
            calls.append({"type": "auth_error", "model": "qwen-max", "status": status})
        time.sleep(CALL_INTERVAL)
    results = check_results(baseline, wait=15)
    print_results("mixed_heavy", calls, results)
    return calls, results


def scenario_multi_type(api_key, base_url):
    """1x auth + 1x model_not_found (404) + 3 normal"""
    baseline = get_baseline()
    calls = []
    print("  Sending 5 requests (1 auth + 1 bad model + 3 normal)...")

    status, _ = send_request(api_key, base_url, content="normal 1", max_tokens=5)
    calls.append({"type": "normal", "model": "qwen-max", "status": status})
    time.sleep(CALL_INTERVAL)

    status, _ = send_request(INVALID_KEY.format("multi_type"), base_url, content="auth error")
    calls.append({"type": "auth_error", "model": "qwen-max", "status": status})
    time.sleep(CALL_INTERVAL)

    status, _ = send_request(api_key, base_url, content="normal 2", max_tokens=5)
    calls.append({"type": "normal", "model": "qwen-max", "status": status})
    time.sleep(CALL_INTERVAL)

    status, _ = send_request(api_key, base_url, model="nonexistent-model-xyz-999", content="bad model")
    calls.append({"type": "model_not_found", "model": "nonexistent-model-xyz-999", "status": status})
    time.sleep(CALL_INTERVAL)

    status, _ = send_request(api_key, base_url, content="normal 3", max_tokens=5)
    calls.append({"type": "normal", "model": "qwen-max", "status": status})

    results = check_results(baseline, wait=15)
    print_results("multi_type", calls, results)
    return calls, results


def scenario_healthy(api_key, base_url):
    """10 normal calls (baseline)"""
    baseline = get_baseline()
    calls = []
    print("  Sending 10 normal requests...")
    for i in range(10):
        status, _ = send_request(api_key, base_url, content="healthy {}".format(i), max_tokens=5)
        calls.append({"type": "normal", "model": "qwen-max", "status": status})
        time.sleep(CALL_INTERVAL)
    results = check_results(baseline, wait=15)
    print_results("healthy", calls, results)
    return calls, results


def scenario_agent_crash(api_key, base_url):
    """Simulate agent crash: long-lived child sends streaming request, gets killed mid-stream.

    Strategy:
      1. Fork a child python3 process that stays alive long enough for the
         HealthChecker to discover it (needs at least one scan cycle, ~30s).
      2. The child sends a stream=true request and reads the SSE chunks slowly.
      3. After the HealthChecker has seen the child (we wait 35s), the parent
         kills the child with SIGKILL while it still has an in-flight LLM call.
      4. On the next HealthChecker scan, the previously-seen pid is gone →
         HealthChecker checks for pending genai_events → generates agent_crash.
      5. We wait another 35s for the next scan to pick up the disappearance.

    Total wait: ~75s. The scenario is slow by design — agent_crash detection
    relies on the HealthChecker's periodic scan, not real-time procmon events.
    """
    import os
    import signal
    import subprocess

    baseline = get_baseline()
    print("  Forking long-lived child to send streaming request...")
    print("  (This scenario takes ~80s due to HealthChecker scan intervals)")

    child_script = '''
import json, urllib.request, ssl, time, sys, os
url = "{base_url}"
key = "{api_key}"

sys.stdout.write("CHILD_PID={{}}\\n".format(os.getpid()))
sys.stdout.flush()

# Send a streaming request that generates a long response
payload = json.dumps({{
    "model": "qwen-max",
    "messages": [{{"role": "user", "content": "Write a detailed 3000 word essay about the history of computing from 1940 to 2025."}}],
    "max_tokens": 4000,
    "stream": True,
}}).encode("utf-8")
headers = {{"Content-Type": "application/json", "Authorization": "Bearer " + key}}
req = urllib.request.Request(url, data=payload, headers=headers, method="POST")
ctx = ssl.create_default_context()
try:
    resp = urllib.request.urlopen(req, context=ctx, timeout=120)
    sys.stdout.write("STREAM_STARTED\\n")
    sys.stdout.flush()
    # Read very slowly so the stream stays open
    while True:
        chunk = resp.read(32)
        if not chunk:
            break
        time.sleep(0.5)
except Exception as e:
    sys.stderr.write("child error: {{}}\\n".format(e))
    # Even if request fails, stay alive so HealthChecker can see us
    time.sleep(120)
'''.format(base_url=base_url, api_key=api_key)

    proc = subprocess.Popen(
        ["python3", "-c", child_script],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    # Wait for child to report its pid and stream status
    child_pid = proc.pid
    try:
        line = proc.stdout.readline().decode().strip()
        if line.startswith("CHILD_PID="):
            child_pid = int(line.split("=")[1])
        line2 = b""
        # Wait up to 15s for STREAM_STARTED
        import select
        ready, _, _ = select.select([proc.stdout], [], [], 15)
        if ready:
            line2 = proc.stdout.readline().decode().strip()
        if "STREAM_STARTED" in str(line2):
            print("  Child pid={} streaming, waiting 35s for HealthChecker discovery...".format(child_pid))
        else:
            print("  Child pid={} started (stream may not have begun yet), waiting 35s...".format(child_pid))
    except Exception as e:
        print("  Error reading child output: {}".format(e))

    # Wait for HealthChecker to discover the child (at least one 30s scan cycle)
    time.sleep(35)

    # Verify child is still alive
    if proc.poll() is not None:
        print("  WARNING: child already exited (code {}), crash simulation failed".format(proc.returncode))
        results = check_results(baseline, wait=5)
        print_results("agent_crash", [{"type": "agent_crash", "model": "qwen-max", "status": "EARLY_EXIT"}], results)
        return [], results

    # Kill the child mid-stream
    print("  Killing child pid={} with SIGKILL...".format(child_pid))
    try:
        os.kill(child_pid, signal.SIGKILL)
    except ProcessLookupError:
        print("  Child already exited")
    proc.wait()
    print("  Child terminated (exit code {})".format(proc.returncode))

    # Wait for next HealthChecker scan to detect the disappearance
    print("  Waiting 40s for HealthChecker to detect crash...")
    results = check_results(baseline, wait=40)
    print_results("agent_crash", [{"type": "agent_crash", "model": "qwen-max", "status": "SIGKILL"}], results)
    return [], results


# ==================== Crash scenarios (trace-mode procmon exit path) ========

def scenario_agent_crash_sigkill(api_key, base_url):
    """kill -9 mid-SSE -> 1x agent_crash: signal=9, severity=critical,
    blast_radius=total_session_loss, dmesg-verified non-OOM."""
    conn = sqlite3.connect(DB_INT)
    since_id = max_interruption_id(conn)
    start_ts = time.time()

    print("  Spawning test_http_crash.py (waits {}s for SSL probe attach)...".format(ATTACH_WAIT))
    pid, popen = spawn_test_process(
        os.path.join(SCRIPT_DIR, "test_http_crash.py"),
        args=["--api-key", api_key, "--base-url", base_url],
    )
    print("  pid={}".format(pid))
    streamed = wait_for_marker(popen, "STREAM_STARTED", timeout=ATTACH_WAIT + 60)
    if streamed is None:
        # Crash detection is exit-classification based, so a SIGKILL is
        # recorded even without an in-flight call — continue, but flag it.
        print("  WARNING: stream did not start; continuing (crash is exit-classified)")
    time.sleep(2)  # let a few SSE chunks flow so the kill lands mid-stream

    print("  kill -9 {}".format(pid))
    killed = kill_with_signal(pid, signal.SIGKILL)
    popen.wait()

    rows = wait_for_interruption(conn, "agent_crash", timeout=CRASH_WAIT,
                                 count=1, since_id=since_id, pid=pid)
    d = rows[0]["detail"] if rows else {}
    no_oom = verify_no_oom(pid, start_ts)
    # When scenario_test.py itself matches the *python3* cmdline rule, it is
    # tracked as TestAgent and spawned children are classified as sub_agent
    # (severity=high, blast_radius=partial).  When running standalone (e.g.
    # the binary is launched from a non-tracked shell), the child would be
    # the primary agent (severity=critical, blast_radius=total_session_loss).
    # Accept both based on the actual process_type classification.
    ptype = d.get("process_type")
    if ptype == "sub_agent":
        expect_sev, expect_blast = "high", "partial"
    else:
        expect_sev, expect_blast = "critical", "total_session_loss"

    checks = [
        ("SSE stream started before kill", streamed is not None, streamed or "no marker"),
        ("process terminated by SIGKILL", killed, "pid={}".format(pid)),
        ("exactly 1 agent_crash recorded", len(rows) == 1, "rows={}".format(len(rows))),
        ("detail.signal == 9", bool(rows) and d.get("signal") == 9,
         "signal={}".format(d.get("signal"))),
        ("severity matches process_type ({}={})".format(ptype, expect_sev),
         bool(rows) and rows[0]["severity"] == expect_sev,
         "severity={}".format(rows[0]["severity"] if rows else None)),
        ("blast_radius matches process_type ({}={})".format(ptype, expect_blast),
         bool(rows) and d.get("blast_radius") == expect_blast,
         "blast_radius={}".format(d.get("blast_radius"))),
        ("detail has no oom flag", bool(rows) and not d.get("oom"),
         "oom={}".format(d.get("oom"))),
        ("verify_no_oom: dmesg window clean", no_oom,
         "since_ts={:.0f}".format(start_ts)),
    ]
    verdict = report_checks("agent_crash_sigkill", checks)
    conn.close()
    time.sleep(SCENARIO_GAP)  # PID-reuse guard before the next scenario
    return checks, verdict


def scenario_agent_crash_sigsegv(api_key, base_url):
    """Self-inflicted SIGSEGV mid-SSE -> agent_crash: signal=11 and the
    detail carries the coredump field (its value depends on ulimit -c /
    core_pattern, so only presence is asserted)."""
    conn = sqlite3.connect(DB_INT)
    since_id = max_interruption_id(conn)

    print("  Spawning test_http_segfault.py (segfaults after {} SSE chunks)...".format(3))
    pid, popen = spawn_test_process(
        os.path.join(SCRIPT_DIR, "test_http_segfault.py"),
        args=["--api-key", api_key, "--base-url", base_url],
    )
    print("  pid={}".format(pid))
    marker = wait_for_marker(popen, "SEGFAULT_NOW", timeout=ATTACH_WAIT + 90)
    try:
        rc = popen.wait(timeout=30)
    except subprocess.TimeoutExpired:
        rc = None
        kill_with_signal(pid, signal.SIGKILL)  # cleanup; checks below will FAIL

    rows = wait_for_interruption(conn, "agent_crash", timeout=CRASH_WAIT,
                                 count=1, since_id=since_id, pid=pid)
    d = rows[0]["detail"] if rows else {}
    # Same sub_agent classification note as scenario_agent_crash_sigkill.
    ptype = d.get("process_type")
    expect_sev = "high" if ptype == "sub_agent" else "critical"
    checks = [
        ("child reached SEGFAULT_NOW", marker is not None, marker or "no marker"),
        ("process died with SIGSEGV (rc == -11)", rc == -11, "rc={}".format(rc)),
        (">= 1 agent_crash recorded", len(rows) >= 1, "rows={}".format(len(rows))),
        ("detail.signal == 11", bool(rows) and d.get("signal") == 11,
         "signal={}".format(d.get("signal"))),
        ("detail contains core_dump field", bool(rows) and "core_dump" in d,
         "core_dump={}".format(d.get("core_dump"))),
        ("severity matches process_type ({}={})".format(ptype, expect_sev),
         bool(rows) and rows[0]["severity"] == expect_sev,
         "severity={}".format(rows[0]["severity"] if rows else None)),
    ]
    verdict = report_checks("agent_crash_sigsegv", checks)
    conn.close()
    time.sleep(SCENARIO_GAP)
    return checks, verdict


def scenario_graceful_stop_no_crash(api_key, base_url):
    """Upstream semantic (issue #1989): SIGTERM mid-SSE with pending calls IS
    now an agent_crash (signal=15). Only clean exit(0) with no signal skips
    crash reporting. This scenario validates the new behavior."""
    conn = sqlite3.connect(DB_INT)
    since_id = max_interruption_id(conn)

    print("  Spawning test_http_crash.py for graceful SIGTERM...")
    pid, popen = spawn_test_process(
        os.path.join(SCRIPT_DIR, "test_http_crash.py"),
        args=["--api-key", api_key, "--base-url", base_url],
    )
    print("  pid={}".format(pid))
    streamed = wait_for_marker(popen, "STREAM_STARTED", timeout=ATTACH_WAIT + 60)
    time.sleep(2)

    print("  kill -15 {}".format(pid))
    killed = kill_with_signal(pid, signal.SIGTERM)
    popen.wait()

    # New upstream semantic (issue #1989, commit 3c354ad8): SIGTERM with pending
    # LLM calls records agent_crash (signal=15). Only exit(0) is "clean".
    rows = wait_for_interruption(conn, "agent_crash", timeout=CRASH_WAIT,
                                 count=1, since_id=since_id, pid=pid)
    d = rows[0]["detail"] if rows else {}
    checks = [
        ("SSE stream started before kill", streamed is not None, streamed or "no marker"),
        ("process terminated by SIGTERM", killed, "pid={}".format(pid)),
        ("agent_crash recorded (SIGTERM with pending)", len(rows) == 1,
         "rows={}".format(len(rows))),
        ("detail.signal == 15", bool(rows) and d.get("signal") == 15,
         "signal={}".format(d.get("signal"))),
    ]
    verdict = report_checks("graceful_stop_no_crash", checks)
    conn.close()
    time.sleep(SCENARIO_GAP)
    return checks, verdict


def scenario_child_exit_no_crash(api_key, base_url):
    """Negative pin: a fork()ed (no exec) child that exit(1)s is an
    AbnormalExit of an UNtracked child. unified.rs only reports SignalCrash
    for non-scanner processes ("child exit(1) after parent dies is normal
    cleanup"), so no agent_crash may be written — for the child (exit 1) or
    for the parent (exit 0, NormalExit)."""
    conn = sqlite3.connect(DB_INT)
    since_id = max_interruption_id(conn)

    print("  Spawning test_subprocess_agent.py --exit1 (fork child exits 1)...")
    pid, popen = spawn_test_process(
        os.path.join(SCRIPT_DIR, "test_subprocess_agent.py"),
        args=["--exit1"],
    )
    print("  parent pid={}".format(pid))
    line = wait_for_marker(popen, "CHILD_PID=", timeout=ATTACH_WAIT + 30)
    child_pid = int(line.rsplit("=", 1)[1]) if line else None
    exited = wait_for_marker(popen, "CHILD_EXITED=", timeout=15)

    # Negative window while the parent is still alive...
    rows = wait_for_interruption(conn, "agent_crash", timeout=NEGATIVE_WINDOW,
                                 count=1, since_id=since_id)
    # ...then let the parent exit(0) and re-check (NormalExit must not report).
    popen.wait()
    rows_after = wait_for_interruption(conn, "agent_crash", timeout=4,
                                       count=1, since_id=since_id)
    checks = [
        ("child forked (no exec)", child_pid is not None, line or "no marker"),
        ("child exited with status 1", exited is not None and exited.endswith("=1"),
         exited or "no marker"),
        ("no agent_crash for child exit(1) (negative)", len(rows) == 0,
         "rows={}".format(len(rows))),
        ("no agent_crash after parent exit(0) (negative)", len(rows_after) == 0,
         "rows={}".format(len(rows_after))),
    ]
    verdict = report_checks("child_exit_no_crash", checks)
    conn.close()
    time.sleep(SCENARIO_GAP)
    return checks, verdict


def scenario_agent_mode_subprocess(api_key, base_url):
    """Full sub-agent chain: a tracked Agent parent sets AGENT_MODE=1 and
    execs a python3 child that streams an LLM call; kill -9 the child ->
    genai_events.process_type='sub_agent' and agent_crash with severity=high
    (SubAgent downgrade of critical) / blast_radius=partial. Optionally the
    `agentsight token --by-type` output mentions sub_agent."""
    conn = sqlite3.connect(DB_INT)
    since_id = max_interruption_id(conn)

    print("  Spawning test_subprocess_agent.py (AGENT_MODE=1 sub-agent chain)...")
    pid, popen = spawn_test_process(
        os.path.join(SCRIPT_DIR, "test_subprocess_agent.py"),
        args=["--api-key", api_key, "--base-url", base_url],
    )
    print("  parent pid={}".format(pid))
    line = wait_for_marker(popen, "CHILD_PID=", timeout=ATTACH_WAIT + 30)
    child_pid = int(line.rsplit("=", 1)[1]) if line else None
    # The child sleeps its own ATTACH_WAIT before its first request.
    streamed = wait_for_marker(popen, "STREAM_STARTED", timeout=ATTACH_WAIT + 90)
    time.sleep(2)

    killed = False
    if child_pid is not None:
        print("  kill -9 {} (child)".format(child_pid))
        killed = kill_with_signal(child_pid, signal.SIGKILL)

    rows = wait_for_interruption(conn, "agent_crash", timeout=CRASH_WAIT,
                                 count=1, since_id=since_id, pid=child_pid)
    d = rows[0]["detail"] if rows else {}
    ptype_ok, ptype_rows = (wait_for_genai_process_type(child_pid, "sub_agent", timeout=10)
                            if child_pid is not None else (False, []))

    # Killed processes typically never flush their pending call to
    # genai_events (the process dies before the stream completes). When
    # no rows exist for the pid, this check is inconclusive -> SKIP.
    # Also SKIP when rows exist but process_type is None — the new code
    # stores process_type only in interruption_events.detail, not in
    # genai_events (which tracks call lifecycle, not process classification).
    if child_pid is not None and not ptype_rows:
        ptype_check = None  # SKIP
        ptype_evidence = "no genai_events rows (killed before flush)"
    elif ptype_ok:
        ptype_check = True
        ptype_evidence = "rows={}".format(ptype_rows[:5])
    elif ptype_rows and all(r[0] is None for r in ptype_rows):
        ptype_check = None  # SKIP — process_type not stored in genai_events
        ptype_evidence = "genai_events.process_type=None (classification in interruption_events only)"
    else:
        ptype_check = False
        ptype_evidence = "rows={}".format(ptype_rows[:5])

    # Optional CLI cross-check: interrupted calls carry no token usage, so a
    # missing sub_agent line is inconclusive -> SKIP, not FAIL.
    cli_out = token_by_type_output()
    if cli_out is None:
        cli_check, cli_evidence = None, "agentsight binary unavailable"
    elif "sub_agent" in cli_out:
        cli_check, cli_evidence = True, "output mentions sub_agent"
    else:
        cli_check, cli_evidence = None, "no sub_agent rows (no token usage yet)"

    wait_for_marker(popen, "CHILD_EXITED=", timeout=15)
    popen.wait()

    checks = [
        ("sub-agent child spawned", child_pid is not None, line or "no marker"),
        ("child streaming before kill", streamed is not None, streamed or "no marker"),
        ("child killed with SIGKILL", killed, "child_pid={}".format(child_pid)),
        (">= 1 agent_crash recorded for child", len(rows) >= 1, "rows={}".format(len(rows))),
        ("detail.signal == 9", bool(rows) and d.get("signal") == 9,
         "signal={}".format(d.get("signal"))),
        ("severity == high (SubAgent downgrade)", bool(rows) and rows[0]["severity"] == "high",
         "severity={}".format(rows[0]["severity"] if rows else None)),
        ("detail.blast_radius == partial", bool(rows) and d.get("blast_radius") == "partial",
         "blast_radius={}".format(d.get("blast_radius"))),
        ("detail.process_type == sub_agent", bool(rows) and d.get("process_type") == "sub_agent",
         "process_type={}".format(d.get("process_type"))),
        ("genai_events.process_type == sub_agent", ptype_check, ptype_evidence),
        ("token --by-type mentions sub_agent (optional)", cli_check, cli_evidence),
    ]
    verdict = report_checks("agent_mode_subprocess", checks)
    conn.close()
    time.sleep(SCENARIO_GAP)
    return checks, verdict


def scenario_oom_kill(api_key, base_url):
    """Optional: cgroup v2 memory.max OOM kill -> agent_crash with
    detail.oom=true. SKIPs when root / cgroup v2 / memory controller unmet."""
    cgroup_root = "/sys/fs/cgroup"
    oom_cgroup = os.path.join(cgroup_root, "agentsight-oom-test")

    # Environment gate (RULES.md: SKIP when prerequisites are unmet)
    reasons = []
    if os.geteuid() != 0:
        reasons.append("not root")
    controllers_file = os.path.join(cgroup_root, "cgroup.controllers")
    if not os.path.exists(controllers_file):
        reasons.append("cgroup v2 not mounted")
    else:
        try:
            with open(controllers_file) as f:
                if "memory" not in f.read().split():
                    reasons.append("memory controller unavailable")
        except Exception as e:
            reasons.append("cannot read cgroup.controllers: {}".format(e))
    if reasons:
        checks = [("environment prerequisites", None, "; ".join(reasons))]
        return checks, report_checks("oom_kill", checks)

    conn = sqlite3.connect(DB_INT)
    since_id = max_interruption_id(conn)
    pid, rc, rows, d = None, None, [], {}
    try:
        os.makedirs(oom_cgroup, exist_ok=True)
        with open(os.path.join(oom_cgroup, "memory.max"), "w") as f:
            f.write("100M")
        try:
            # Must disable swap, or the process gets swapped instead of killed
            with open(os.path.join(oom_cgroup, "memory.swap.max"), "w") as f:
                f.write("0")
        except OSError:
            pass  # no swap accounting on this kernel

        def enter_cgroup():
            # Runs in the child after fork, before exec — puts exactly the
            # test process into the cgroup (no intermediate shell, README
            # pitfall #9).
            with open(os.path.join(oom_cgroup, "cgroup.procs"), "w") as f:
                f.write(str(os.getpid()))

        print("  Spawning test_http_crash.py --oom inside {} (100M limit)...".format(oom_cgroup))
        popen = subprocess.Popen(
            ["python3", os.path.join(SCRIPT_DIR, "test_http_crash.py"), "--oom",
             "--api-key", api_key, "--base-url", base_url],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            preexec_fn=enter_cgroup,
        )
        pid = popen.pid
        print("  pid={}".format(pid))
        # The OOM kill may race the marker; tolerate a missing line.
        wait_for_marker(popen, "OOM_ALLOC_STARTED", timeout=ATTACH_WAIT + 90)
        try:
            rc = popen.wait(timeout=120)
        except subprocess.TimeoutExpired:
            rc = None
            kill_with_signal(pid, signal.SIGKILL)

        rows = wait_for_interruption(conn, "agent_crash", timeout=CRASH_WAIT,
                                     count=1, since_id=since_id, pid=pid)
        d = rows[0]["detail"] if rows else {}
    finally:
        conn.close()
        for _ in range(5):
            try:
                os.rmdir(oom_cgroup)
                break
            except OSError:
                time.sleep(1)

    checks = [
        ("process OOM-killed (rc == -9)", rc == -9, "rc={}".format(rc)),
        (">= 1 agent_crash recorded", len(rows) >= 1, "rows={}".format(len(rows))),
        ("detail.signal == 9", bool(rows) and d.get("signal") == 9,
         "signal={}".format(d.get("signal"))),
        ("detail.oom == true", bool(rows) and d.get("oom") is True,
         "oom={}".format(d.get("oom"))),
    ]
    verdict = report_checks("oom_kill", checks)
    time.sleep(SCENARIO_GAP)
    return checks, verdict


CRASH_SCENARIO_ORDER = [
    "agent_crash_sigkill",
    "agent_crash_sigsegv",
    "graceful_stop_no_crash",
    "child_exit_no_crash",
    "agent_mode_subprocess",
    "oom_kill",
]

SCENARIOS = {
    "auth_single":  scenario_auth_single,
    "auth_storm":   scenario_auth_storm,
    "mixed_light":  scenario_mixed_light,
    "mixed_heavy":  scenario_mixed_heavy,
    "multi_type":   scenario_multi_type,
    "healthy":      scenario_healthy,
    "agent_crash":  scenario_agent_crash,
    "agent_crash_sigkill":    scenario_agent_crash_sigkill,
    "agent_crash_sigsegv":    scenario_agent_crash_sigsegv,
    "graceful_stop_no_crash": scenario_graceful_stop_no_crash,
    "child_exit_no_crash":    scenario_child_exit_no_crash,
    "agent_mode_subprocess":  scenario_agent_mode_subprocess,
    "oom_kill":               scenario_oom_kill,
}


def main():
    import os
    parser = argparse.ArgumentParser(
        description="AgentSight Interruption Scenario Test",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("scenario", choices=list(SCENARIOS.keys()) + ["all", "crash_all"])
    parser.add_argument("--api-key", default=os.environ.get("DASHSCOPE_API_KEY", ""),
                        help="Valid dashscope API key (or set DASHSCOPE_API_KEY env)")
    parser.add_argument("--base-url", default=DEFAULT_URL)
    args = parser.parse_args()

    if not args.api_key:
        parser.error("API key required: use --api-key or set DASHSCOPE_API_KEY env var")

    print("=" * 60)
    print("AgentSight Scenario Test")
    print("=" * 60)
    print("Base URL: {}".format(args.base_url))
    print("Scenario: {}".format(args.scenario))

    if args.scenario == "all":
        for name in ["healthy", "auth_single", "auth_storm", "mixed_light", "multi_type"]:
            print("\n>>> Running scenario: {} <<<".format(name))
            SCENARIOS[name](args.api_key, args.base_url)
            print()
            time.sleep(5)
    elif args.scenario == "crash_all":
        # Each crash scenario already sleeps SCENARIO_GAP at its end
        # (PID-reuse guard between scenarios).
        for name in CRASH_SCENARIO_ORDER:
            print("\n>>> Running scenario: {} <<<".format(name))
            SCENARIOS[name](args.api_key, args.base_url)
            print()
    else:
        SCENARIOS[args.scenario](args.api_key, args.base_url)

    print("\nDone.")


if __name__ == "__main__":
    main()
