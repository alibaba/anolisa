# Tokenless CLI Reference

[中文版](../../../zh/token-saving/tokenless/cli-reference.md)

The `tokenless` CLI exposes a unified content-aware compression command, direct schema/JSON/TOON
operations, Stash retrieval, environment status, and statistics. Shared Agent hooks use
`tokenless compress`; the direct commands remain useful for scripts and diagnosis.

## Command overview

| Command | Purpose |
|---------|---------|
| `tokenless compress` | Send model-visible content through the content-aware Pipeline |
| `tokenless compress-schema` | Compress Function Calling tool schemas |
| `tokenless compress-response` | Compress JSON/API/tool responses |
| `tokenless compress-toon` | Encode JSON as TOON |
| `tokenless decompress-toon` | Decode TOON to JSON |
| `tokenless retrieve` | Recover a payload truncated into Stash |
| `tokenless env-check` | Report the hard-disabled state of the legacy environment check |
| `tokenless stats` | Query and control local statistics |
| `tokenless mcp serve` | Start an MCP stdio server for retrieval |

Use the installed version's help as the final argument reference:

```bash
tokenless --help
tokenless <command> --help
```

## Common input rules

Compression and encoding commands accept input in two ways:

```bash
tokenless compress-response --file response.json

cat response.json | tokenless compress-response
```

- `-f` is the short form of `--file`.
- Without `--file`, input must be provided on stdin.
- The per-call input limit is 64 MiB.
- JSON commands require valid JSON.
- If compression does not reduce the estimated token count, the CLI explains this on stderr and returns the original.

### Minimum useful payload

`compress-schema` and `compress-response` have no fixed minimum input size. For
every accepted valid JSON input, the CLI builds a candidate and estimates both
versions as one token per CJK character plus one token per four other characters,
rounded up. In active mode, it emits the candidate only when its estimate is strictly
lower than the original (`after < before`). Otherwise stdout receives the original
input, stderr reports `did not reduce size`, and no statistics record is written.
In dry-run mode (`TOKENLESS_COMPRESSION_ENABLED=0` or
`compression_enabled=false`), stdout always receives the original input; a smaller
candidate is recorded as a predicted saving when statistics or SLS recording is enabled.

The break-even point therefore depends on content and JSON shape, not only on
bytes or characters. A small payload with a removable field can compress, while
a larger already-compact payload can pass through unchanged. The description,
string, array, and depth thresholds below only decide when individual
transformations run; they are not minimum total payload sizes. Agent adapters
can apply separate pre-spawn size gates; see
[Adapter processing rules](framework-integration.md#adapter-processing-rules).

## `compress`

`compress` is the stable JSON boundary used by shared Agent hooks. It accepts one
`CompressionRequest` on stdin or through `--file` and always writes one `CompressionResponse` JSON
object when the request is decodable:

```bash
jq -n \
  --rawfile content build.log \
  '{
    protocol_version: 1,
    content: $content,
    agent_id: "my-agent",
    session_id: "session-42",
    tool_use_id: "tool-7",
    tool_name: "Bash",
    seam: "post_tool",
    capabilities: {
      replace_output: true,
      publish_retrieve_tool: true,
      replace_with_text: true
    }
  }' \
  | tokenless compress \
  | jq '{disposition, content_type, compressor_chain, reversibility, output}'
```

Request fields:

| Field | Required | Meaning |
|-------|----------|---------|
| `protocol_version` | Yes | Compression request format version; currently `1` |
| `content` | Yes | The exact model-visible string to consider |
| `agent_id` | Yes | Stable frontend identifier |
| `session_id`, `tool_use_id`, `tool_name` | No | Attribution and tool routing data |
| `seam` | Yes | `before_model`, `pre_tool`, `post_tool`, or `proxy` |
| `capabilities.replace_output` | No; default `false` | The host can replace the original model-visible value |
| `capabilities.publish_retrieve_tool` | No; default `false` | The host exposes a usable retrieval Tool |
| `capabilities.replace_with_text` | No; default `false` | The replacement slot accepts arbitrary text instead of schema-stable JSON |

The pipeline detects `json_records`, `search_results`, `build_log`, `stack_trace`, `diff`, `html`,
`tabular`, `source_code`, `plain_text`, or `unknown`, then runs only compressors compatible with the
seam and declared host capabilities. Current production routing is:

- `before_model` JSON tool arrays: schema compression.
- `post_tool` JSON records: structural response cleanup; TOON may win for text-capable slots.
- `post_tool` build logs and long plain text: lossless terminal cleanup followed by retrievable
  build/log compression when replacement, retrieval, and text capabilities are all present.
- Other detected content types: passthrough until a matching compressor is implemented.

Only `disposition: "applied"` means `output` differs from the original. `dry_run`, `passthrough`,
`no_savings`, `reversibility_unavailable`, `timeout`, and `error` all carry the original `content` in
`output`, so adapters can emit it unconditionally. Responses also report `content_type`, ordered
`compressor_chain`, `reversibility`, token estimates, committed `stash_keys`, `tokenizer_id`, and an
optional bounded diagnostic.

Unreadable, oversized, malformed, or unsupported-version requests exit with status `2` because the
CLI cannot recover the original content from a valid request. Once decoding succeeds, compression
failures are represented by a fail-open response and exit status `0`. `--stash-db` overrides the
Stash path under the same path-safety rules as the direct commands.

The build/log compressor engages only for real multi-line logs, preserves signal and trace regions,
keeps fixed head/tail and failure-context windows, and replaces each eligible omitted gap with a
retrievable marker. It rejects candidates saving fewer than 200 characters. Generic plain text uses
a conservative 40-line head/tail window once it reaches 100 lines, plus a 16,384-character
head/tail safety path for text at least 65,536 characters long.

## `compress-schema`

Compress one OpenAI Function Calling schema:

```bash
tokenless compress-schema -f tool.json
```

Compress a JSON array:

```bash
cat tools.json | tokenless compress-schema --batch
```

Accepted item shapes (detected per item):

- OpenAI function wrapper: `{"function": {"name", "description", "parameters"}}`
- Direct schema: `{"name", "description", "parameters"}`
- Gemini / copilot-shell wrapper: `{"functionDeclarations": [{"name", "description", "parameters" | "parametersJsonSchema"}, ...]}`; copilot-shell BeforeModel hooks deliver tool declarations in this shape (`llm_request.config.tools`). Declarations inside the wrapper are compressed individually (the parameter schema is read from `parametersJsonSchema` when present, otherwise from `parameters`); the wrapper itself and any sibling fields are preserved.

An array input enables batch handling automatically.

A complete request object with a top-level `tools` array is also accepted. Its
Function Calling entries may be OpenAI `{"function": {...}}` wrappers, Gemini
`{"functionDeclarations": [...]}` tool objects, or bare
`{name, description, parameters}` declarations. Do not pass `--batch` for this
shape; non-function tools and fields outside `tools` are preserved.

```bash
tokenless compress-schema -f request.json
```

Common options:

| Option | Description |
|--------|-------------|
| `-f, --file <path>` | Input file; omit to read stdin |
| `--batch` | Treat the input as a schema array |
| `--agent-id <id>` | Agent identifier in statistics |
| `--session-id <id>` | Session identifier in statistics |
| `--tool-use-id <id>` | Tool-call identifier in statistics |
| `--no-stash` | Do not save truncated descriptions; truncation becomes irreversible |
| `--stash-db <path>` | Override the Stash database; an invalid path is rejected as an override and falls back to the environment or default path |

Default processing rules:

| Item | Default |
|------|---------|
| Maximum function-description length | 256 characters |
| Maximum parameter-description length | 160 characters |
| Drop `examples` | yes |
| Drop `title` | yes |
| Remove fenced and inline code, then collapse whitespace in descriptions | yes |
| Maximum recursion depth | 32 |

Example:

```bash
tokenless compress-schema -f tools.json --batch \
  --agent-id copilot-shell --session-id session-001
```

## `compress-response`

Compress a JSON response:

```bash
tokenless compress-response -f response.json
```

By default it removes exact, case-sensitive blacklisted keys, `null`, and empty strings/arrays/objects, including empty items inside arrays. It then truncates long strings, long arrays, and values beyond the configured nesting limit. Common options:

| Option | Default | Description |
|--------|---------|-------------|
| `-f, --file <path>` | stdin | Input file |
| `--truncate-strings-at <n>` | `4096` | String truncation threshold |
| `--truncate-arrays-at <n>` | `32` | Array length that triggers truncation; the first `n` items are kept |
| `--array-tail-preserve <n>` | `8` | Items preserved from the tail of truncated arrays; `0` disables tail preservation |
| `--max-depth <n>` | `8` | Maximum nesting depth |
| `--agent-id <id>` | `cli` | Agent identifier in statistics |
| `--session-id <id>` | — | Session identifier in statistics |
| `--tool-use-id <id>` | — | Tool-call identifier in statistics |
| `--no-stash` | off | Disable reversible Stash |
| `--stash-db <path>` | `~/.tokenless/stash.db` | Override the Stash database; an invalid path is rejected as an override and the CLI falls back to the environment or default path |

Array truncation keeps a head window of `--truncate-arrays-at` items and a tail window of `--array-tail-preserve` items, with a truncation marker in between. Middle items are dropped only when the array is longer than both windows combined, so under the defaults a command can retain `n + 8` items plus the marker; when the two windows cover the whole array, every item is retained without a marker. Set `--array-tail-preserve 0` for head-only truncation.

Override thresholds:

```bash
tokenless compress-response -f response.json \
  --truncate-strings-at 2048 \
  --truncate-arrays-at 16 \
  --max-depth 6
```

The default field-name blacklist is:

```text
debug, trace, traces, stack, stacktrace, logs, logging
```

Field matching and truncation change the response representation seen by the model. Save representative samples and compare the result before processing critical payloads.

Stash applies only to truncation of strings, the dropped middle segment of truncated arrays, and deep subtrees. Tail items are kept inline, not stashed. Blacklisted fields, `null`, and empty values are removed without a retrieval marker.

Most adapters override these standalone defaults. Their shared shell profile uses `65536`, `128`, and `8`; the other-structured-tool profile uses `1048576`, `65536`, and `32`. Content-retrieval tools are skipped. See [Agent integration · Adapter processing rules](framework-integration.md#adapter-processing-rules).

## `compress-toon` and `decompress-toon`

JSON to TOON:

```bash
echo '{"name":"Alice","age":30}' | tokenless compress-toon --min-toon-chars 0
```

Payloads shorter than 500 characters pass through unchanged by default: TOON savings on small JSON are near-zero, so the CLI applies the same minimum length as the adapter hooks. Pass `--min-toon-chars 0` to encode any payload that yields token savings anyway. Input is validated as JSON before the length check, so invalid JSON exits with code 2 even when it is below the threshold.

TOON to JSON:

```bash
printf 'name: Alice\nage: 30\n' | tokenless decompress-toon
```

Round-trip verification:

```bash
echo '{"name":"test","value":42}' \
  | tokenless compress-toon --min-toon-chars 0 \
  | tokenless decompress-toon
```

`compress-toon` supports `--agent-id`, `--session-id`, `--tool-use-id`, and `--min-toon-chars`. When a payload is below the minimum length or encoding provides no savings, it returns the original JSON and does not record that operation. The exit code is still `0` in these passthrough cases, and the note on stderr is informational only: when scripting, detect whether encoding happened by comparing stdout with the input payload instead of relying on stderr. Passthrough and no-savings runs reproduce the input byte-for-byte on stdout (no trailing newline is added or stripped), so any byte difference means the payload was encoded; checking whether stdout is still valid JSON works as a fallback.

## `retrieve`

This marker in compressed output means that removed content was written to Stash:

```text
<<tokenless:0123456789abcdef01234567>>
```

Retrieve by bare hash:

```bash
tokenless retrieve 0123456789abcdef01234567
```

You may also paste a complete line containing the marker:

```bash
tokenless retrieve \
  '<... 12 items truncated, retrieve with <<tokenless:0123456789abcdef01234567>>'
```

Override the database:

```bash
tokenless retrieve 0123456789abcdef01234567 \
  --stash-db ~/.tokenless/stash.db
```

The hash must contain 24 hexadecimal characters and is case-insensitive. The default SQLite Stash TTL is one hour and its live-entry capacity is 10,000. Retrieval fails after expiry or capacity eviction, with `--no-stash`, in dry-run mode, after a failed write, or when a different database path is used.

## `mcp serve`

Start the stdio MCP server:

```bash
tokenless mcp serve
```

It exposes `tokenless_retrieve`, allowing an MCP-capable agent to recover Stash content without a shell call. The MCP server must use the same user and Stash database as the compression flow.

## `env-check`

Tool Ready is hard-disabled. Text output reports that state without reading the
specification or changing the environment. No environment variable can
re-enable it.

Every JSON invocation returns exactly three fields:

```json
{"tool":"Shell","status":"UNKNOWN","enabled":false}
```

`tool` is the requested tool name, `all`, or `checklist`. The hard-disabled
contract never includes the dormant legacy checklist's `tools` or `summary`
fields.

Report the disabled state for one tool:

```bash
tokenless env-check --tool Shell
```

Report the disabled state for all-tools or checklist mode:

```bash
tokenless env-check --all
tokenless env-check --all --json
tokenless env-check --checklist
tokenless env-check --checklist --json
```

Automatic repair:

```bash
tokenless env-check --tool Shell --fix
```

> While the hard bypass is active, `--fix` does not invoke a package manager or modify the environment. The retained legacy implementation would attempt only missing required dependencies if it were redesigned and re-enabled in a future release.

## `stats`

```bash
tokenless stats summary
tokenless stats summary --json
tokenless stats summary --limit 1000
tokenless stats list --limit 20
tokenless stats show <record-id>
tokenless stats diff <record-id>
tokenless stats diff --session <session-id>
tokenless stats status
tokenless stats enable
tokenless stats disable
tokenless stats clear --yes
```

Dual-run comparison:

```bash
tokenless stats summary --compare <baseline-session> <active-session>
```

A missing session ID fails with a non-zero exit instead of a 0% comparison, matching `stats diff --session`. `stats summary --limit` must be a positive integer; `--limit 0` is rejected at parse time, matching `stats diff --limit`.

Inspect one record or the verified stages of one tool call:

```bash
tokenless stats diff <record-id> -U 5
tokenless stats diff --session <session-id> \
  --tool-use-id <tool-use-id>
```

`stats show` prints the complete stored before/after text. `stats diff` explains estimated savings and renders changed lines. Its main options are:

| Option | Applies to | Behavior |
|--------|------------|----------|
| `<record-id>` | One record | Conflicts with `--session` |
| `--session <id>` | Session | Shows a metrics-only overview |
| `--tool-use-id <id>` | Session | Expands one tool call; requires `--session` |
| `-l, --limit <n>` | Session overview | Maximum chains, default `20` |
| `--sort saved\|time` | Session overview | Largest saving first by default, or newest first |
| `-U, --context <n>` | Content diff | Unchanged lines around changes, default `3` |
| `--no-color` | Text output | Disables ANSI colors |
| `--json` | Any scope | Emits schema `1.0` JSON with structured diff hunks |

Content diffing is omitted when either endpoint is unavailable or exceeds 1 MiB, and rendered hunks stop after 500 lines. Take care when using a shared terminal or collecting output because record and tool-use diffs can contain stored source text. See [Measuring savings](measuring-savings.md) and [Configuration and data privacy](configuration-and-privacy.md).

`stats status` reports the local-statistics and SLS switches and their source. The current status path does not read the compression switch, so it does not display `compression_enabled`; inspect `TOKENLESS_COMPRESSION_ENABLED` and `~/.tokenless/config.json` for that setting.

## Errors and degradation

- CLI errors are written to stderr and return a non-zero exit status.
- Hooks and plugins normally catch errors and pass through the original response.
- No compression savings is not an error; the CLI returns the original.
- Compression may continue after a Stash write failure, but the related truncated content cannot be retrieved.

See [Troubleshooting](troubleshooting.md) for input, database, and adapter errors.
