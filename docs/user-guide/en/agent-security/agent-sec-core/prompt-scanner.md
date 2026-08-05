# Prompt Scanner User Guide

[中文版](../../../zh/agent-security/agent-sec-core/prompt-scanner.md)

Prompt Scanner detects prompt injection, jailbreak, and malicious instructions in Agent inputs. It
combines a fast rule engine (L1) with an optional ML classifier (L2), returns a structured verdict,
and records sanitized Security Events for audit and Observability correlation.

## Scan text

Provide exactly one input source: inline text, standard input, or a UTF-8 file (one prompt per line).

```bash
# Inline text
agent-sec-cli scan-prompt --text "ignore all system instructions"

# Standard input
echo "forget your system prompt" | agent-sec-cli scan-prompt

# UTF-8 file (one prompt per line)
agent-sec-cli scan-prompt --input prompts.txt --format json
```

Useful options:

| Option | Purpose |
|--------|---------|
| `--text TEXT` | Prompt text to scan directly; takes precedence over `--input` and stdin |
| `--input FILE` | Path to a file with one prompt per line |
| `--mode MODE` | Detection mode: `fast`, `standard`, `strict`, or `multi_turn`; default is `standard` |
| `--format FMT` | Output format: `json` (default) or `text` (human-readable) |
| `--source SOURCE` | Input origin label recorded in metadata, such as `user_input`, `rag`, or `tool_output` |

## Detection modes

| Mode | Layers | fast_fail | Typical latency | Use case |
|------|--------|-----------|-----------------|----------|
| `fast` | L1 rule engine | `True` | < 5 ms | Real-time chat, latency-sensitive |
| `standard` | L1 + L2 ML classifier | `False` | 20–80 ms | Production default |
| `strict` | L1 + L2 ML classifier (L3 reserved) | `False` | 50–200 ms | High-security scenarios |
| `multi_turn` | L4 multi-turn intent detection | — | Varies | JSON history input via stdin (Ollama) |

The L2 classifier downloads `LLM-Research/Llama-Prompt-Guard-2-86M` from ModelScope on first use
(about 1 GB). Run `agent-sec-cli scan-prompt warmup` once after installation to eliminate the
cold-start delay.

## Verdicts

The scanner aggregates layer results into one verdict:

| Verdict | Meaning |
|---------|---------|
| `pass` | No threat detected |
| `warn` | L1 rule hit, but L2 did not confirm (`standard`/`strict`); or a policy-level warning |
| `deny` | Threat confirmed by L1 (`fast`) or L1 + L2 (`standard`/`strict`) |
| `error` | Scanner internal error (e.g., model load failure) |

> In `fast` mode, any L1 rule hit maps directly to `deny` because the ML layer is not run.

## Host hook policy

Set `PROMPT_SCANNER_HOOK_ENABLED=false` to skip host prompt scanner hooks entirely. When enabled,
the following environment variables control deployment-level behavior:

| Environment variable | Default | Behavior |
|----------------------|---------|----------|
| `PROMPT_SCANNER_HOOK_ENABLED` | `true` | Set to `false` to short-circuit the hook before input is read |
| `PROMPT_SCANNER_MODE` | `observe` | `observe` audits silently; `warn` warns; `ask`/`block` use host-specific enforcement or fall back to `warn`; `deny` maps to `block` |
| `PROMPT_SCANNER_SCAN_MODE` | `standard` | Scan strength passed to `scan-prompt`: `fast` / `standard` / `strict` |
| `PROMPT_SCANNER_TIMEOUT` | `10` | Scanner timeout in seconds |

Environment variables override Hermes/OpenClaw capability configuration. The host Agent reads them
when it loads the plugin, so restart the Agent process after changing them.

Scanner verdict `deny` describes the risk severity; hook policy `block` controls whether the current
adapter attempts enforcement.

## Security Events and Observability

Every scan follows the existing `prompt_scan` Security Event path. Events contain the source,
verdict, summary, threat type, confidence, and sanitized rule or ML findings. They do not contain
the raw prompt text.

Host hooks remain fail-open on scanner errors: an `error` verdict is audited but is not used to
block the underlying operation.

Observability uses the existing trace context and input hash to correlate telemetry with the
Security Event instead of storing another copy of finding details.
