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
| `--model MODEL` | L2 backend model; overrides `PROMPT_SCANNER_L2_MODEL`, defaults to Qwen3Guard when unset |

## Detection modes

| Mode | Layers | fast_fail | Typical latency | Use case |
|------|--------|-----------|-----------------|----------|
| `fast` | L1 rule engine | `True` | < 5 ms | Real-time chat, latency-sensitive |
| `standard` | L1 + L2 ML classifier | `False` | 20–80 ms | Production default |
| `strict` | L1 + L2 ML classifier (L3 reserved) | `False` | 50–200 ms | High-security scenarios |
| `multi_turn` | L4 multi-turn intent detection | — | Varies | JSON history input via stdin (Ollama) |

By default the L2 classifier calls `modelscope.cn/ANOLISA/Qwen3Guard-Gen-0.6B-GGUF`, served by
Ollama from the project's ModelScope repository. Pull it once with
`ollama pull modelscope.cn/ANOLISA/Qwen3Guard-Gen-0.6B-GGUF` — then
run `agent-sec-cli scan-prompt warmup` to verify the model is available before the first scan.

### Switching the L2 backend

Set `PROMPT_SCANNER_L2_MODEL` to run L2 on the Warden-Gen model instead (or use
`--model` for a one-off override; precedence is `--model` > env var > default):

```bash
ollama pull modelscope.cn/ANOLISA/Warden-Gen-0.6B-GGUF

# Option 1: environment variable (applies to every host hook)
export PROMPT_SCANNER_L2_MODEL=modelscope.cn/ANOLISA/Warden-Gen-0.6B-GGUF
agent-sec-cli scan-prompt warmup

# Option 2: --model for a single command
agent-sec-cli scan-prompt --model modelscope.cn/ANOLISA/Warden-Gen-0.6B-GGUF --text "..."
```

Every host hook shells out to `scan-prompt`, so the variable applies to them as well. The value
must be the full model name used in the `ollama pull` above; a typo makes the scan fail loudly
instead of silently disabling L2. An empty or unset value keeps the Qwen3Guard default.

L2 runs exactly one backend at a time — no cascading, no voting.

To confirm which backend a host would use, run
`agent-sec-cli capabilities --capability prompt-scan --output json` from that
host's environment and read the `PROMPT_SCANNER_L2_MODEL` entry under `env`. It
reports the default backend when the variable is unset, and adds a diagnostic
when the configured name is not one the engine supports.

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

| Environment variable | Default | Hosts that read it | Behavior |
|----------------------|---------|--------------------|----------|
| `PROMPT_SCANNER_HOOK_ENABLED` | `true` | All six | Set to `false` to short-circuit the hook before input is read |
| `PROMPT_SCANNER_MODE` | `observe` | Qoder, Codex, Qwen Code | `observe` audits silently; `deny` blocks prompt-scanner `warn` or `deny` findings. `ask` and `block` are not valid prompt-scanner modes. |
| `PROMPT_SCANNER_SCAN_MODE` | `standard` | All six | Scan strength passed to `scan-prompt`: `fast` / `standard` / `strict` |
| `PROMPT_SCANNER_TIMEOUT` | `10` | Qoder, Codex, Qwen Code | Scanner timeout in seconds |

cosh, Hermes, and OpenClaw read only `PROMPT_SCANNER_HOOK_ENABLED` and
`PROMPT_SCANNER_SCAN_MODE`. Setting `PROMPT_SCANNER_MODE` or `PROMPT_SCANNER_TIMEOUT` has no
effect there. OpenClaw derives its enforcement from `promptScanBlock` and uses a fixed
10-second scanner timeout, while the Hermes `prompt-scan-user-input` capability is non-blocking
by design and has no block switch; cosh has no prompt policy switch either. For Qoder, Codex,
and Qwen Code, use `PROMPT_SCANNER_MODE=deny` to block prompt scanner findings.

Where an environment variable is read, it overrides the matching host configuration.
The host Agent reads these variables when it loads the plugin, so restart the Agent process
after changing them.

Scanner verdict `deny` describes risk severity. For Qoder, Codex, and Qwen Code prompt hooks,
`PROMPT_SCANNER_MODE=deny` is the deployment policy that turns prompt-scanner findings into a
blocking hook result.

## Security Events and Observability

Every scan follows the existing `prompt_scan` Security Event path. Events contain the source,
verdict, summary, threat type, confidence, and sanitized rule or ML findings. They do not contain
the raw prompt text.

Host hooks remain fail-open on scanner errors: an `error` verdict is audited but is not used to
block the underlying operation.

Observability uses the existing trace context and input hash to correlate telemetry with the
Security Event instead of storing another copy of finding details.
