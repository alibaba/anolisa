# Session Recovery

cosh-shell can discover and resume earlier Agent conversations in the current
workspace. Recovery restores the messages visible to the model so the next
request continues the conversation instead of starting a fresh turn.

Session recovery is available with the `cosh-core` adapter. It does not restore
historical terminal output, approval prompts, questions, or other transient UI
state. `/session status` reports this boundary explicitly so old terminal
evidence is never presented as current evidence.

## Start With a Previous Session

Open the picker after the shell is ready:

```bash
cosh-shell --resume
cosh-shell raw cosh-core --resume
```

Select a known canonical session UUID directly:

```bash
cosh-shell --resume 2d711642-b726-4b04-8d2a-8a0470f4ed24
cosh-shell raw cosh-core --resume 2d711642-b726-4b04-8d2a-8a0470f4ed24
```

`--resume` and its optional value are launch options, not adapter names. The
same validation and persistence path is used whether a session is selected at
launch, selected interactively, or supplied directly to `cosh-core --resume`.

## Manage Sessions Interactively

Use these commands from the shell prompt:

| Command | Behavior |
|---------|----------|
| `/session` | Open the newest-first session picker |
| `/session list` | Print the first bounded summary page with complete, copyable session UUIDs |
| `/session new` | Detach the current provider conversation so the next Agent request starts fresh |
| `/new` | Alias for `/session new` |
| `/session status` | Show shell, selected, restoring, and active provider identities |
| `/session resume <id>` | Validate and select one provider session |
| `/resume [id]` | Alias for `/session` or `/session resume <id>` |
| `/session clear <id>...` | Ask for confirmation before clearing explicit IDs |
| `/session clear --all` | Prepare exact IDs and ask before clearing all persisted sessions |

Selecting a session does not start a model call. Its state changes from
`selected` to `restoring` on the next Agent request and becomes `active` only
after the resumed request completes successfully. A failed restore remains
recoverable and leaves the shell prompt usable. Direct and picker-based resume
both refuse to change selection while an Agent run or another interactive
decision is active.

Starting a fresh session clears only the shell adapter's active or selected
provider-session binding. It does not delete the previous persisted session,
restart the shell, or change the working directory, shell history, or settings.
The next Agent request omits the old resume ID and creates the new provider
identity when that request runs successfully. The command is idempotent when
no provider session is attached.

If automatic continuation of an active session reports that its stored record
is missing, corrupt, incompatible, or in another scope, cosh-shell releases
that stale active ID instead of retrying it forever. Authentication, budget,
model, and other ordinary provider failures keep the active session available
for a later retry. Internal one-turn fallbacks that disable provider resume do
not consume a session you selected explicitly.

## Picker Keys

| Key | Action |
|-----|--------|
| `Up` / `Down`, `j` / `k` | Move the cursor |
| `Enter` | Resume the highlighted healthy session |
| `Space` | Mark or unmark an entry for clearing |
| `d` | Open clear confirmation for marked entries, or the highlighted entry |
| `y` | Confirm the exact clear set |
| `n`, `Esc`, `Ctrl-C` | Cancel confirmation or close the picker |

Each picker row shows a compact session-ID prefix for visual disambiguation,
plus the prompt preview, relative update time, message count, model, health,
and whether the entry is protected. The prefix is not accepted by direct
resume or clear commands; use the complete canonical UUID printed by
`/session list`. The picker fetches the next bounded cursor page only when
navigation approaches the loaded edge and renders a bounded window around the
current row. Prompt previews are normalized and bounded by Core before
transport, then truncated further for the terminal row.

## Workspace Scope and Storage

Sessions belong to the canonical current workspace. A session from another
workspace cannot be resumed accidentally, even if its file is copied into the
current scope.

The default persistence root is:

```text
~/.copilot-shell/cosh-core/sessions/
```

Core stores each workspace below a deterministic hash directory and each
conversation in a versioned JSON envelope. The envelope includes the canonical
provider UUID, workspace, timestamps, model, generation, and model-visible
messages. Writes use a temporary sibling file, filesystem sync, atomic rename,
and an optimistic generation check. Listing orders file metadata first and
only reads the requested page. Session files are capped at 32 MiB; an
oversized entry is shown as `corrupt` without loading its contents.

A known UUID from a former flat layout remains resumable only when that legacy
directory can be proven to belong to the requested workspace. For the former
relative default, Core checks `<workspace>/sessions/<uuid>.json`; it does not
claim an ambiguous shared root or a file under the launcher cwd. Explicit
lookup loads the old array in memory without rewriting it. The schema-v1 copy
is created, and the old file removed, only after the resumed turn persists
successfully. Proven workspace-owned legacy sessions appear in the picker and
`/session list` beside upgraded entries, and their IDs, including corrupt
files, are included in clear-all and can be removed by an explicit clear.
Ambiguous files outside a proven workspace-owned directory are never listed
or claimed.

On Unix, workspace directories are explicitly restricted to `0700`, while
session JSON, temporary, and lock files use `0600`, independent of the process
umask. Concurrent writers use an advisory file lock, which the kernel releases
if a process exits unexpectedly.

The root can be changed with `session.persist_dir` in either the user config
or `<workspace>/.copilot-shell/config.toml`. Project settings and relative
paths are resolved from the workspace sent by cosh-shell, even when the Core
process was launched from another cwd. Setting `session.auto_persist = false`
keeps turn history in the current Core process only. Core marks that ID as not
resumable, and cosh-shell does not commit or reuse it on the next turn,
including when the current turn fails, is cancelled, or exits unexpectedly.
Only the identity actually used by that invocation is invalidated; unrelated
selected and older active IDs remain available.

## Health and Recovery Errors

The picker keeps damaged entries visible so they can be identified and
cleared:

| Health or error | Meaning | Next action |
|-----------------|---------|-------------|
| `ready` | The envelope is valid for this workspace | Resume normally |
| `corrupt` | JSON or required envelope data is malformed | Confirm the ID, then clear it |
| `incompatible` | The schema version is unsupported | Upgrade cosh-core or clear it |
| `scope_mismatch` | The recorded workspace differs | Return to the original workspace |
| `not_found` | The file disappeared after listing | Refresh the picker |
| `conflict` | Another writer holds or advanced the session | Retry after it finishes |

Malformed, missing, incompatible, scope-mismatched, and concurrent sessions do
not terminate the interactive shell. Only `ready` entries can be resumed;
unhealthy entries can still be cleared after confirmation.

For every resume attempt, cosh-shell verifies that Core returns the exact
selected or active provider ID before committing the turn. A typed persistence
failure releases only the active identity used by that invocation, preventing
stale history from being retried while preserving unrelated selections.

## Clear Protection

Clearing is always explicit and confirmed. The confirmation identifies the
exact IDs or count being removed. The selected session and the active provider
session are protected in both cosh-shell and cosh-core, so they are skipped
even if they appear in a clear-all request. Canceling confirmation leaves all
records unchanged. If every stored session is protected, the command reports
the protected count instead of claiming that the workspace is empty.
