# Tokenless CLI Reference

[中文版](../../../zh/token-saving/tokenless/cli-reference.md)

The `tokenless` CLI can compress schemas and responses, encode and decode TOON, retrieve Stash content, check tool environments, and query statistics. Agent adapters call the same capabilities internally.

## Command overview

| Command | Purpose |
|---------|---------|
| `tokenless compress-schema` | Compress Function Calling tool schemas |
| `tokenless compress-response` | Compress JSON/API/tool responses |
| `tokenless compress-toon` | Encode JSON as TOON |
| `tokenless decompress-toon` | Decode TOON to JSON |
| `tokenless retrieve` | Recover a payload truncated into Stash |
| `tokenless env-check` | Check tool dependencies and environment |
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

## `compress-schema`

Compress one OpenAI Function Calling schema:

```bash
tokenless compress-schema -f tool.json
```

Compress a JSON array:

```bash
cat tools.json | tokenless compress-schema --batch
```

An array input enables batch handling automatically. Common options:

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
| `--truncate-arrays-at <n>` | `32` | Maximum retained array items |
| `--max-depth <n>` | `8` | Maximum nesting depth |
| `--agent-id <id>` | `cli` | Agent identifier in statistics |
| `--session-id <id>` | — | Session identifier in statistics |
| `--tool-use-id <id>` | — | Tool-call identifier in statistics |
| `--no-stash` | off | Disable reversible Stash |
| `--stash-db <path>` | `~/.tokenless/stash.db` | Override the Stash database; an invalid path is rejected as an override and the CLI falls back to the environment or default path |

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

Stash applies only to truncation of strings, array tails, and deep subtrees. Blacklisted fields, `null`, and empty values are removed without a retrieval marker.

Most adapters override these standalone defaults. Their shared shell profile uses `65536`, `128`, and `8`; the other-structured-tool profile uses `1048576`, `65536`, and `32`. Content-retrieval tools are skipped. See [Framework integration · Adapter processing rules](framework-integration.md#adapter-processing-rules).

## `compress-toon` and `decompress-toon`

JSON to TOON:

```bash
echo '{"name":"Alice","age":30}' | tokenless compress-toon
```

TOON to JSON:

```bash
printf 'name: Alice\nage: 30\n' | tokenless decompress-toon
```

Round-trip verification:

```bash
echo '{"name":"test","value":42}' \
  | tokenless compress-toon \
  | tokenless decompress-toon
```

`compress-toon` supports `--agent-id`, `--session-id`, and `--tool-use-id`. When encoding provides no savings, it returns the original JSON and does not record that operation.

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

Check one tool:

```bash
tokenless env-check --tool Shell
```

Check all declared tools:

```bash
tokenless env-check --all
tokenless env-check --all --json
tokenless env-check --all --checklist
```

Status meanings:

| Status | Meaning |
|--------|---------|
| `READY` | Required and recommended dependencies, configuration, and permissions are satisfied |
| `PARTIAL` | Required dependencies and permissions are satisfied, but a recommended dependency, configuration item, or network check is missing |
| `NOT_READY` | A required dependency or permission is missing; the tool should not be retried |
| `UNKNOWN` | The dependency specification does not contain the tool |

Automatic repair:

```bash
tokenless env-check --tool Shell --fix
```

> `--fix` attempts only missing required dependencies, not recommended ones. It may invoke a system package manager, install dependencies, or create links. Read the normal check output first and use it only after accepting those environment changes. Follow the output when administrator access is required.

## `stats`

```bash
tokenless stats summary
tokenless stats summary --json
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
