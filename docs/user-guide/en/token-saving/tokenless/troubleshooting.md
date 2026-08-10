# Tokenless Troubleshooting

[中文版](../../../zh/token-saving/tokenless/troubleshooting.md)

First identify the failing layer: component installation, adapter integration, compression, statistics storage, or Stash retrieval. Do not begin by deleting configuration or reinstalling everything.

## Quick diagnostics

Run these in order:

```bash
tokenless --version
anolisa status tokenless
anolisa doctor tokenless
anolisa adapter status tokenless
tokenless stats status
tokenless env-check --all --checklist
```

When one command fails, resolve that layer before continuing. Preview the install plan without modifying the system:

```bash
anolisa --dry-run install tokenless
anolisa --dry-run --verbose install tokenless
```

Run adapter diagnostics as the user who owns the target Agent configuration and
adapter receipt. That user can inspect both user state and readable system
state:

```bash
anolisa doctor tokenless
```

## `tokenless: command not found`

A normal user install usually places the command in `~/.local/bin`. Check:

```bash
command -v tokenless
printf '%s\n' "$PATH"
ls -l ~/.local/bin/tokenless
```

If `~/.local/bin` is absent from `PATH`, add it according to the shell's startup-file rules and open a new terminal. Do not repeat a system install merely to solve a PATH problem.

npm users should also check:

```bash
npm prefix -g
npm list -g --depth=0 anolisa-tokenless
```

If npm logs say that optional dependencies were skipped, reinstall with:

```bash
npm install -g --include=optional anolisa-tokenless
```

Linux npm binaries support glibc only. musl systems such as Alpine require a Linux source build.

## Input and JSON errors

| Error | Cause | Resolution |
|-------|-------|------------|
| `No input provided` | No `--file` and stdin is a terminal | Use `-f <path>` or a pipe |
| `Input exceeds 64 MiB limit` | One input exceeds the cap | Split the input; do not bypass it by raising system memory limits |
| `JSON parse error` | Invalid JSON | Run `jq . < input.json` first |
| `Expected a JSON array for --batch mode` | `--batch` input is not an array | Remove `--batch` or fix the input structure |
| Output is still the original | Compression had no estimated saving | Normal behavior; inspect the stderr notice |

## No statistics appear after enabling the adapter

### 1. Verify the standalone CLI

```bash
printf '%s\n' \
  '{"status":"ok","debug":{"trace":"verbose"},"metadata":null,"data":{"items":[1,2,3]}}' \
  | tokenless compress-response

tokenless stats list --limit 5
```

If this also creates no record, check:

```bash
tokenless stats status
ls -ld ~/.tokenless
ls -l ~/.tokenless/stats.db
```

No record is written when compression has no savings. Use test input with removable or truncatable content.

### 2. Verify the adapter

```bash
anolisa adapter scan
anolisa adapter status tokenless
```

Confirm that:

- The target framework is detected.
- The Tokenless adapter is enabled.
- Adapter commands run as the user who owns the target framework configuration
  and adapter receipt.
- The agent CLI or IDE was restarted after enabling.

### 3. Verify the agent task

Run a task that actually passes through a hook, such as a shell command with visible output. Pure conversation, short responses, or a framework without the required hook may not create a record.

### 4. Check environment overrides

```bash
env | grep '^TOKENLESS_'
```

Confirm that `TOKENLESS_STATS_ENABLED=0` is not set unexpectedly and that any custom database path remains under the real user home.

## Adapter enable fails

Common causes:

- The target agent framework is not installed or detected.
- The framework version does not meet the adapter requirement.
- The adapter command ran as a different user from the one that owns the target
  framework configuration or adapter receipt.
- A directly installed Tokenless RPM has not been adopted into ANOLISA state.
- An npm installation has no anolisa component record, but `anolisa adapter enable` was used.
- OpenClaw security policy rejected the plugin's required unsafe-install override.

Start with:

```bash
anolisa adapter scan
anolisa --verbose adapter enable tokenless <framework>
```

For npm installations, use [Framework integration · Manual integration after npm installation](framework-integration.md#manual-integration-after-npm-installation).

For a directly installed RPM, create the missing state record, then rerun the
adapter command as the target framework user:

```bash
sudo yum install anolisa
sudo anolisa --install-mode system adopt tokenless
```

For an anolisa-managed installation, the first attempt does not bypass OpenClaw's safety scan. If the error specifically recommends it, review the findings and retry with:

```bash
anolisa adapter enable tokenless openclaw \
  --allow-unsafe-plugin-install
```

The npm/manual install script behaves differently: it always passes OpenClaw's `--dangerously-force-unsafe-install` because the plugin launches fixed `tokenless` and `rtk` child processes. Review the adapter and policy; do not enable it where that override is prohibited.

## A command is not rewritten

RTK does not have a rewrite rule for every command. Test it directly:

```bash
rtk rewrite "ls -la"
```

If `rtk` is missing:

```bash
command -v rtk
tokenless env-check --tool Shell
```

If RTK works directly but not in the agent, inspect the framework support matrix, adapter status, and whether the session was restarted.

`TOKENLESS_COMPRESSION_ENABLED=0` does not disable rewriting. Disable the adapter, or set OpenClaw's `rtk_enabled=false` when using that plugin, if the original shell input must be preserved.

## Tool Ready reports `NOT_READY`

View the complete checklist:

```bash
tokenless env-check --tool <name>
tokenless env-check --all --checklist
```

`NOT_READY` means that a required dependency is absent. Resolve the specific binary, configuration, permission, or network issue in the report. Review the change before automatic repair:

```bash
tokenless env-check --tool <name> --fix
```

`--fix` may invoke a package manager or create links. Do not add `sudo` without understanding the output.

## Database errors

### `Failed to open database`

```bash
ls -ld ~/.tokenless
ls -l ~/.tokenless/stats.db*
env | grep -E 'TOKENLESS_(DATA_DIR|STATS_DB|STASH_DB)='
```

Confirm that the current user can write the selected data directory and database. The `tokenless` CLI accepts `TOKENLESS_DATA_DIR`, `TOKENLESS_STATS_DB`, and `TOKENLESS_STASH_DB` only under the real user home and falls back when an override is rejected. The bundled RTK statistics writer uses `TOKENLESS_STATS_DB` directly, so remove or correct an unexpected override in the agent environment as well.

Do not share one `stats.db` between users. AgentSight and Tokenless should run so that they can access the same user's database.

### No SLS JSONL record

```bash
tokenless stats status
test -e /var/log/anolisa/sls/ops/tokenless.jsonl
```

SLS is enabled by default, but Tokenless does not create the target file. A missing file causes a silent skip. A custom path must be under `/var/log/` or `/tmp/`.

## `retrieve` is empty or fails

Check that:

1. The hash contains all 24 hexadecimal characters.
2. Compression did not use `--no-stash`.
3. Compression was active rather than dry-run.
4. The one-hour default TTL has not passed and the 10,000-entry capacity did not evict it.
5. Compression and retrieval use the same user and database path.
6. Compression stderr did not report a Stash write failure.

```bash
ls -l ~/.tokenless/stash.db*
env | grep '^TOKENLESS_STASH_DB='
```

Retry with the same database explicitly:

```bash
tokenless retrieve <hash> --stash-db ~/.tokenless/stash.db
```

Expired or never-successfully-written content cannot be recovered.

## Statistics exist but the prompt is not smaller

First check the framework's response-delivery path in the [support matrix](framework-integration.md#support-matrix). Qoder and Qwen Code emit `additionalContext`; legacy Copilot Shell appends it; Codex intentionally retains the original result and adds only analysis or a compressed alternative. These paths can record a smaller candidate without reducing the final prompt.

For Claude Code, response replacement requires version 2.1.121 or later. Older or unrecognized versions pass the original through. OpenClaw replaces persisted results, but TOON remains off unless `toon_compression_enabled=true`.

## Qoder plugin cache issue

Use this section only when an upgrade produces:

```text
python3: can't open file '/rewrite_hook.py'
```

Refresh the adapter:

```bash
anolisa adapter disable tokenless qoder
anolisa adapter enable tokenless qoder
```

Confirm that the cache has no unexpanded placeholder:

```bash
grep -R -n 'QODER_TOKENLESS_HOOKS' \
  ~/.qoder/plugins/cache/local/tokenless*/*/hooks.json 2>/dev/null
```

No output is expected. Fully exit and restart Qoder IDE afterwards.

## anolisa and RPM state disagree

If `dnf remove` or `rpm -e` was run directly:

```bash
sudo yum install anolisa
sudo anolisa --install-mode system repair tokenless
```

Follow the repair plan. Only when the RPM is still present and the output explicitly asks to recreate the record, run:

```bash
sudo anolisa --install-mode system forget tokenless
sudo anolisa --install-mode system adopt tokenless
```

`forget` deletes only anolisa state; it does not uninstall the RPM.

## Upgrade and uninstall

### anolisa installation

Upgrade:

```bash
anolisa update tokenless
anolisa adapter status tokenless
anolisa doctor tokenless
```

For system mode:

```bash
sudo anolisa update tokenless
```

Restart enabled agents after upgrading. Adapters normally do not need to be re-enabled. If status reports inconsistent resources, follow the diagnostic result before disabling and enabling again.

Before uninstalling, list and disable every adapter:

```bash
anolisa adapter status tokenless
anolisa adapter disable tokenless <framework>
anolisa uninstall tokenless
```

Use the same scope for system mode. In the current release, `--purge` only supports plan preview through `anolisa --dry-run uninstall --purge tokenless`; without `--dry-run`, it returns `NotImplemented` and does not uninstall the component or remove configuration, cache, or state. Use `anolisa uninstall tokenless` for an actual uninstall, and see [Clear data](configuration-and-privacy.md#clear-data) for local databases.

### npm installation

Upgrade:

```bash
npm install -g anolisa-tokenless@latest
```

npm refreshes adapter resources, but a plugin registered with a framework may still be an older copy. Run the target framework's `scripts/install.sh` again and restart the framework.

Uninstall in this order:

```bash
bash ~/.local/share/anolisa/adapters/tokenless/<framework>/scripts/uninstall.sh
npm uninstall -g anolisa-tokenless
```

After confirming that every npm-managed adapter was uninstalled, remove the resource copy from the user data directory:

```bash
rm -rf -- ~/.local/share/anolisa/adapters/tokenless
```

Run this only after confirming that the directory belongs to this Tokenless npm installation. A manually installed cosh Extension must be separately confirmed and removed from `~/.copilot-shell/extensions/tokenless`.

### YUM/RPM installation

Prefer management through the anolisa system scope. If anolisa does not own the installation record, disable adapters first, then run:

```bash
sudo yum update tokenless
sudo yum remove tokenless
```

Upgrade or removal does not automatically clear Tokenless runtime databases under the user home.

## If the issue remains

Before sharing the following output, inspect and remove sensitive content:

```bash
tokenless --version
anolisa --version
anolisa doctor tokenless
anolisa adapter status tokenless
tokenless stats status
tokenless env-check --all --json
```

Do not attach `stats.db`, `stash.db`, or unreviewed `tokenless stats show` output.
