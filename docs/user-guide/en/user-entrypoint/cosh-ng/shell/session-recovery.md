# Session Recovery

[中文版](../../../../zh/user-entrypoint/cosh-ng/shell/session-recovery.md)

cosh can resume earlier Agent conversations from the current workspace. The
next request continues with the messages the model saw before.

Session recovery is available with the `cosh-core` adapter. It does not restore
historical terminal output, approval prompts, questions, or other transient UI
state. `/session status` reports this boundary explicitly so old terminal
evidence is never presented as current evidence.

## Start With a Previous Session

Open the picker after the shell is ready:

```bash
cosh --resume
```

Select a known canonical session UUID directly:

```bash
cosh --resume 2d711642-b726-4b04-8d2a-8a0470f4ed24
```

Both commands validate that the conversation belongs to the current workspace
before selecting it.

## Manage Sessions Interactively

Use these commands from the shell prompt:

| Command | Behavior |
|---------|----------|
| `/session` | Open the newest-first session picker |
| `/session list` | Print the first bounded summary page with complete, copyable session UUIDs |
| `/session list --all` | Print sessions from every workspace, grouped by workspace path |
| `/session new` | Detach the current provider conversation so the next Agent request starts fresh |
| `/new` | Alias for `/session new` |
| `/session status` | Show shell, selected, restoring, and active provider identities |
| `/session resume <id>` | Validate and select one provider session |
| `/resume [id]` | Alias for `/session` or `/session resume <id>` |
| `/session clear <id>...` | Ask for confirmation before clearing explicit IDs |
| `/session clear --all` | Prepare exact IDs and ask before clearing all persisted sessions |

Selecting a conversation does not call the model. Recovery starts with your
next Agent request. If it fails, the Shell remains usable and you can retry or
start a new conversation.

`/session list --all` lists persisted sessions across every workspace under the
same storage root. Output is grouped by canonical workspace path, with each
group sorted newest-first. The group for the current workspace is labelled with
`(current)` so you can quickly locate resumable sessions. Sessions that belong to
a workspace other than the current one are shown with `scope_mismatch`; they are
visible so you can identify them, but `/session resume <id>` still refuses to
restore them and does not change the working directory. The interactive picker
opened by `/session` remains scoped to the current workspace and does not offer
an `--all` mode.

`/session new` detaches the current Agent conversation. It does not delete the
old record, restart the Shell, or change your working directory and history.

## Picker Keys

| Key | Action |
|-----|--------|
| `Up` / `Down`, `j` / `k` | Move the cursor |
| `Enter` | Resume the highlighted healthy session |
| `Space` | Mark or unmark an entry for clearing |
| `d` | Open clear confirmation for marked entries, or the highlighted entry |
| `y` | Confirm the exact clear set |
| `n`, `Esc`, `Ctrl-C` | Cancel confirmation or close the picker |

Each row shows a short ID, prompt preview, update time, message count, model,
health, and protection state. Direct resume and clear commands require the
complete UUID printed by `/session list`.

## Workspace Scope and Storage

Sessions belong to the canonical current workspace. A session from another
workspace cannot be resumed accidentally, even if its file is copied into the
current scope.

The default persistence root is:

```text
~/.copilot-shell/cosh-core/sessions/
```

Each workspace has an isolated subdirectory under this root. A conversation
from another workspace cannot be resumed until you start cosh from its original
workspace. Change the root with `session.persist_dir`. Set
`session.auto_persist = false` when conversations should last only for the
current cosh process.

Storage permissions, migration, locking, and wire-level behavior are documented
in the [session management protocol](../../../../../developer-guide/en/cosh-ng/ipc-protocol.md#cosh-core-session-management-json-protocol).

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

## Clear Protection

Clearing is always explicit and confirmed. The confirmation identifies the
exact IDs or count being removed. The selected session and the active provider
session are protected in both cosh-shell and cosh-core, so they are skipped
even if they appear in a clear-all request. Canceling confirmation leaves all
records unchanged. If every stored session is protected, the command reports
the protected count instead of claiming that the workspace is empty.
