# Tokenless Configuration and Data Privacy

[中文版](../../../zh/token-saving/tokenless/configuration-and-privacy.md)

Tokenless enables compression, local statistics, and SLS metrics by default. Because local statistics and Stash may contain complete tool output or truncated original payloads, review these defaults before processing source code, credentials, or production logs.

## Configuration precedence

In the normal path, each toggle uses:

```text
Environment variable > ~/.tokenless/config.json > default
```

An empty environment variable is treated as unset. For Boolean environment variables, `1`, `true`, and `yes` are true, case-insensitively; any other non-empty value is false. Prefer explicit `true` or `false` values for readability.

There is one current implementation exception: when both `TOKENLESS_STATS_ENABLED` and `TOKENLESS_SLS_ENABLED` are non-empty, the config file is skipped completely. In that branch, compression uses `TOKENLESS_COMPRESSION_ENABLED` when set and otherwise defaults to `true`. If you export both recording variables, export the compression variable explicitly as well.

## Configuration file

Configuration path:

```text
~/.tokenless/config.json
```

Complete example:

```json
{
  "stats_enabled": true,
  "sls_enabled": true,
  "compression_enabled": true
}
```

A missing, unreadable, or invalid JSON file is silently replaced by the all-`true` defaults in memory. Validate a manually edited file with:

```bash
jq . ~/.tokenless/config.json
```

| Field | Default | Actual behavior |
|-------|---------|-----------------|
| `stats_enabled` | `true` | Writes complete before/after text and metrics to local SQLite |
| `sls_enabled` | `true` | Appends a metrics-only record when the target JSONL file exists |
| `compression_enabled` | `true` | Returns compressed output when true; false runs dry-run and returns the original |

When Tokenless writes the configuration, it restricts the mode to `0600`. Confirm the mode after creating it manually:

```bash
chmod 600 ~/.tokenless/config.json
```

The `stats` subcommands change only `stats_enabled`:

```bash
tokenless stats status
tokenless stats enable
tokenless stats disable
```

An environment override still wins after these commands. For example, `TOKENLESS_STATS_ENABLED=0 tokenless stats enable` saves `true` to the file, but recording remains disabled for processes that keep the environment override.

## Environment variables

### Common user variables

| Variable | Purpose | Constraint |
|----------|---------|------------|
| `TOKENLESS_STATS_ENABLED` | Override local statistics | Does not affect SLS or Stash |
| `TOKENLESS_SLS_ENABLED` | Override SLS metrics | Does not affect local statistics |
| `TOKENLESS_COMPRESSION_ENABLED` | Override active compression | False is dry-run, not a full stop |
| `TOKENLESS_DATA_DIR` | Directory containing `stats.db` and `stash.db` | Any accessible absolute directory except filesystem root; no parent traversal |
| `TOKENLESS_STATS_DB` | Override the statistics database | Must be under the real user home or selected data directory |
| `TOKENLESS_STASH_DB` | Override the Stash database | Must be under the real user home or selected data directory |
| `TOKENLESS_SLS_PATH` | Override the SLS JSONL path | Must be under `/var/log/` or `/tmp/` |

### Adapter and diagnostic variables

| Variable | Purpose |
|----------|---------|
| `TOKENLESS_AGENT_ID` | Agent identifier injected by an adapter |
| `TOKENLESS_SESSION_ID` | Session identifier injected by an adapter |
| `TOKENLESS_TOOL_USE_ID` | Tool-call identifier injected by an adapter |
| `TOKENLESS_TOOL_READY_SPEC` | Override the Tool Ready dependency specification |
| `TOKENLESS_ENV_FIX_SCRIPT` | Override the environment repair script |
| `TOKENLESS_PACKAGE_MANAGER` | Override package-manager detection, mainly for tests |

Tool Ready is hard-disabled in this build. Its specification and repair-script overrides are retained for the dormant legacy implementation but have no runtime effect. They are subject to trusted-path validation and are not recommended for normal users.

Database path priority is:

- Stats: `TOKENLESS_STATS_DB` > `TOKENLESS_DATA_DIR/stats.db` > `~/.tokenless/stats.db`
- Stash: `--stash-db` > `TOKENLESS_STASH_DB` > `TOKENLESS_DATA_DIR/stash.db` > `~/.tokenless/stash.db`

`TOKENLESS_DATA_DIR` is an explicit directory-level relocation and may point outside the real user home, including to a managed service directory under `/var/lib`. Both the CLI and bundled RTK writer reject filesystem root, relative paths, parent traversal, and existing non-directory targets. Without a valid higher-priority file override, an invalid explicit data directory disables that operation's SQLite state instead of silently falling back to home.

An empty value is treated as unset. `TOKENLESS_DATA_DIR` may name a directory that does not exist yet; Tokenless canonicalizes its nearest existing ancestor before creating it. File-level overrides are accepted only beneath the canonical real home or selected data directory, and existing database symlinks are rejected. `TOKENLESS_DATA_DIR` does not relocate `~/.tokenless/config.json` or the SLS JSONL output.

## Local and external data

| Data | Default path | Default content | Retention | Stop new data |
|------|--------------|-----------------|-----------|---------------|
| Local statistics | `~/.tokenless/stats.db` | Complete before/after text, identifiers, and metrics | No automatic TTL; retained until cleared | `tokenless stats disable` |
| Stash | `~/.tokenless/stash.db` | Original strings, dropped middle segments of truncated arrays, deep subtrees, schema descriptions removed by truncation, and build/log gaps | One-hour TTL and 10,000 live entries; expired rows are purged lazily | CLI: `--no-stash`; agent: disable the adapter |
| Configuration | `~/.tokenless/config.json` | Three Boolean toggles | Persistent | Not applicable |
| SLS JSONL | `/var/log/anolisa/sls/ops/tokenless.jsonl` | Metrics and identifiers, no compressed source text | Managed by SLS/Logtail infrastructure | `TOKENLESS_SLS_ENABLED=0` or config false |

### Sensitivity of local statistics

`before_text` and `after_text` in `stats.db` preserve complete content. `tokenless stats show` prints that content, while record-level and tool-use-level `tokenless stats diff` commands can render changed lines from it. They may contain:

- Source code and patches.
- Paths, user names, or environment details from command output.
- Business data returned by an API.
- Access tokens, cookies, or credentials found in logs.

The `tokenless` CLI's SQLite recorder attempts to set `stats.db` to `0600` whenever it opens the database. The bundled RTK statistics patch can create or open the same file directly and does not apply that permission change itself. Do not rely on the process umask; verify the deployed database and sidecars:

```bash
ls -l ~/.tokenless/stats.db*
```

### Sensitivity of Stash

Stash saves the original content removed by truncation, not a summary. It does not save fields removed solely because they are blacklisted, `null`, or empty. The `tokenless` CLI restricts its path to the real user home or selected data directory, but also verify that the database and SQLite sidecar files are not readable by other local users:

```bash
ls -l ~/.tokenless/stash.db*
```

TTL means that `retrieve` no longer returns an entry after one hour. Expired rows are deleted lazily during a later retrieval; TTL is not an immediate secure-erasure guarantee for disk data. When more than 10,000 live entries exist, the store evicts entries with the earliest expiry first, so retrieval can fail before one hour under heavy use.

### SLS excludes original text

Tokenless SLS JSONL includes the component, operation, session/tool-use identifiers, and character/token metrics. It does not include `before_text` or `after_text`. Identifiers can still be organizational runtime metadata and should follow the platform's log policy.

## Guidance for sensitive workloads

### Compress without recording

```bash
TOKENLESS_STATS_ENABLED=0 \
TOKENLESS_SLS_ENABLED=0 \
  tokenless compress-response --no-stash -f response.json
```

This applies to standalone CLI use. Agent adapters may use Stash by default. If the framework does not provide an appropriate exclusion rule, disable the adapter for sensitive tasks.

### Keep the adapter but do not apply compression

Set this in the environment used to start the agent:

```bash
export TOKENLESS_COMPRESSION_ENABLED=0
```

This is a dry-run and may still write local statistics or SLS. To avoid persistence, also set:

```bash
export TOKENLESS_STATS_ENABLED=0
export TOKENLESS_SLS_ENABLED=0
```

Dry-run does not create Stash entries, but it also does not disable RTK rewriting. Tool Ready is independently hard-disabled. Disable the adapter when all hook behavior must stop.

### Stop Tokenless completely in an agent

```bash
anolisa adapter disable tokenless <framework>
```

Restart the agent afterwards. Setting only `compression_enabled=false` does not stop hook or plugin execution.

## Clear data

Clear local statistics records:

```bash
tokenless stats clear --yes
```

This clears records from the statistics database resolved in the current environment, but it does not remove the database file or SQLite sidecars. Tokenless currently has no Stash clear subcommand. For irreversible local-database removal:

1. Disable every Tokenless adapter.
2. Exit agents and Tokenless processes that may still use the databases.
3. Confirm that statistics history and Stash retrieval are no longer needed.
4. Back up anything that must be retained.
5. Inspect path overrides in the actual environment used to start the agent, service, and Tokenless:

```bash
env | grep -E '^TOKENLESS_(DATA_DIR|STATS_DB|STASH_DB)='
```

The statistics path resolves in this order: `TOKENLESS_STATS_DB`, `TOKENLESS_DATA_DIR/stats.db`, then `~/.tokenless/stats.db`. The Stash path resolves in this order: command-line `--stash-db`, `TOKENLESS_STASH_DB`, `TOKENLESS_DATA_DIR/stash.db`, then `~/.tokenless/stash.db`. Write the final values as verified absolute paths; do not expand untrusted environment values directly into a removal command.

The following command works for default and custom paths. Replace and print both paths first, then confirm that they are the Tokenless databases to remove:

```bash
stats_db='/absolute/path/to/resolved/stats.db'
stash_db='/absolute/path/to/resolved/stash.db'
printf '%s\n' "$stats_db" "$stash_db"
rm -f -- \
  "$stats_db" \
  "$stats_db-wal" \
  "$stats_db-shm" \
  "$stats_db-journal" \
  "$stash_db" \
  "$stash_db-wal" \
  "$stash_db-shm" \
  "$stash_db-journal"
```

This cannot be undone. Do not recursively remove the data directory or `~/.tokenless/` because either location may contain configuration or other files that you want to keep.

## Fine-grained OpenClaw control

The OpenClaw plugin also provides framework-level options:

| Option | Purpose |
|--------|---------|
| `rtk_enabled` | Rewrite supported shell commands through RTK |
| `tool_ready_enabled` | OpenClaw-side Tool Ready registration gate |
| `post_tool_enabled` | Optimize supported persisted tool results |
| `verbose` | Plugin diagnostic logging |

The OpenClaw plugin does not compress tool schemas or provide content retrieval. Results that cannot
be safely optimized without retrieval pass through unchanged.

RTK, the OpenClaw-side Tool Ready registration gate, and PostTool default to on; verbose logging
defaults to off. The Tool Ready option currently has no operational effect because Tokenless
hard-disables the underlying check. Tokenless automatically decides whether JSON cleanup or TOON is
useful and which tool outputs must pass through unchanged. The removed
`response_compression_enabled`, `toon_compression_enabled`, `skip_tools`, and `shell_tools` keys no
longer control the adapter.

These values are managed by OpenClaw plugin configuration, not `~/.tokenless/config.json`. The
adapter requires OpenClaw Plugin API 2026.4.22 or later. Restart the gateway as instructed after
changing them.

## Related documents

- [Measuring savings](measuring-savings.md)
- [CLI reference](cli-reference.md)
- [Agent integration](framework-integration.md)
- [Troubleshooting](troubleshooting.md)
