---
name: agentos-baseline
phase: 1
description: Use the only supported Phase 1 flow: `agent-sec-cli harden --mode scan`.
---

# Phase 1: SEHarden

Unless the user explicitly asks for another language, keep operator-facing output in Simplified Chinese.

## Fixed Baseline

Phase 1 only supports this baseline:

- Tool: `loongshield seharden`
- Profile: `agentos_baseline`

Do not switch to another profile.
Do not replace this flow with another shell script, wrapper, or hardening tool.

## Modes

Read `$ARGUMENTS` and map it to one of these modes:

- Empty or `scan`
- `dry-run`
- `reinforce`

If `$ARGUMENTS` is anything else, stop and tell the user:

```text
支持的模式: scan | dry-run | reinforce
```

## Exact Commands

- `scan`: `agent-sec-cli harden --mode scan`
- `dry-run`: `agent-sec-cli harden --mode dry-run`
- `reinforce`: `agent-sec-cli harden --mode reinforce`

Always keep `--config agentos_baseline` explicit.
The `--config agentos_baseline` profile is the built-in default in `agent-sec-cli`.

## Execution Rules

1. Verify `loongshield` is installed before running anything.
2. Never run `reinforce` unless the user explicitly requested it.
3. `reinforce` requires root. Do not add `sudo` silently.
4. Run only the selected `loongshield seharden` command.
5. Show the command output directly.

## Result Handling

Treat the run as non-compliant if either of these is true:

- the command exits non-zero
- the output contains `FAIL`, `MANUAL`, `DRY-RUN`, `FAILED-TO-FIX`, `ENFORCE-ERROR`, or `Engine Error`

Use a short report:

- success: `结果：合规`
- failure: `结果：不合规`

If `scan` is non-compliant, add:

```text
建议：如需修复，可先执行 `dry-run` 预览，再执行 `reinforce`。
```

If `dry-run` is non-compliant, add:

```text
建议：确认预演结果后，可执行 `reinforce`。
```

If `reinforce` succeeds, say the baseline has been applied successfully.

## Status Line Output

After completing the result handling above, you must output exactly one of the following status lines:

- On success: `[Phase 1] PASS`
- On failure: `[Phase 1] FAIL: <failing rule IDs or summary>`
- If `loongshield` is not installed: `[Phase 1] NOT_RUN: loongshield not found`
