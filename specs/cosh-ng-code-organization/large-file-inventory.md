# cosh-shell Large-File Inventory

> Tracked technical debt for `crates/cosh-shell/src` production files that
> exceed the 700-line layout threshold enforced by
> `crates/cosh-shell/scripts/check-layout.sh`.

Registration in this inventory records that a large file is *known* debt with a
named owner and a split plan. It does **not** mean the file is acceptable
long-term — every entry is a standing invitation to split. The audit fails if a
new over-threshold file appears that is not listed here.

## Rules

- Only files that already exceed the threshold on `main`/`HEAD` may be
  registered (`git show HEAD:<path> | wc -l`). Files newly *created* over the
  threshold must be split instead of waived — the automatic-compaction feature
  split its new `slash/session/compact.rs` (732 lines) into
  `compact.rs` + `compact/process.rs` + `compact/runtime.rs` rather than
  registering it here.
- The `Lines` column is the current size and is advisory; the audit matches on
  the `File` path only.
- Do not raise the 700-line threshold, suppress the check, or bulk-register
  files without a baseline and a reason.

## Registered files

| Lines | File | Owner | Reason / split plan |
|------:|------|-------|---------------------|
| 1717 | `src/ui/agent_render/health.rs` | ui | Health banner rendering; pre-existing. Split per-severity renderers. |
| 1628 | `src/diagnostics/health/collectors.rs` | diagnostics | Per-subsystem collectors; pre-existing. Split by collector family. |
| 1378 | `src/auth/runtime.rs` | auth | Auth control-protocol runtime; pre-existing. Extract provider flows. |
| 992 | `src/runtime/state.rs` | runtime | Central inline state; pre-existing. Extract per-domain state modules. |
| 925 | `src/agent/heartbeat.rs` | agent | Heartbeat/status animation; pre-existing. Split timing from rendering. |
| 912 | `src/evidence/output_policy.rs` | evidence | Output excerpt policy; pre-existing. Split bounding from classification. |
| 906 | `src/adapter/fake.rs` | adapter | Test fake adapter; pre-existing. Split scripted-scenario builders. |
| 898 | `src/ui/agent_render/approval.rs` | ui | Approval card rendering; pre-existing. Split per-card-kind renderers. |
| 886 | `src/activity/runtime.rs` | activity | Activity runtime; pre-existing. Extract lifecycle from bookkeeping. |
| 859 | `src/agent/poll.rs` | agent | Agent run polling loop; pre-existing debt (main baseline 839). The compaction feature added the `compaction_recommended_v1` status capture; split event routing from rendering. |
| 856 | `src/parser/mod.rs` | parser | Event parser (legacy `mod.rs`); pre-existing. Split per-event-kind parsers and rename off `mod.rs`. |
| 855 | `src/activity/runtime_render.rs` | activity | Activity rendering; pre-existing. Split per-panel renderers. |
| 853 | `src/i18n/en.rs` | i18n | English catalog; pre-existing. Partition by message domain. |
| 848 | `src/shell_host/marker.rs` | shell_host | OSC marker state machine; pre-existing. Split parsing from state. |
| 823 | `src/i18n/zh.rs` | i18n | Chinese catalog; pre-existing. Partition by message domain. |
| 805 | `src/shell_host/osc.rs` | shell_host | OSC sequence handling; pre-existing. Split sequence kinds. |
| 788 | `src/runtime/shell_evidence.rs` | runtime | Shell evidence runtime; pre-existing. Extract capture from rendering. |
| 786 | `src/tools/display.rs` | tools | Tool display formatting; pre-existing. Split per-tool formatters. |
| 778 | `src/ui/agent_render/activity.rs` | ui | Activity card rendering; pre-existing. Split per-activity renderers. |
| 770 | `src/diagnostics/health/recommendation.rs` | diagnostics | Health recommendations; pre-existing. Split rule families. |
| 770 | `src/agent/failed_command.rs` | agent | Failed-command analysis; pre-existing debt (main baseline 760). The compaction feature added the suppression-aware start disposition handling; split request-building from gating. |
| 737 | `src/agent/approval_bridge.rs` | agent | Approval bridge; pre-existing. Split request mapping from replay. |
| 724 | `src/runtime/hooks.rs` | runtime | Runtime hook dispatch; pre-existing. Split per-hook-type handling. |
| 722 | `src/i18n/message_id_all.rs` | i18n | `MessageId::ALL` mirror of the enum; near-threshold historical file (main baseline 699, zero headroom). The compaction feature added 23 required user-facing message ids, crossing the threshold. Split plan: generate the enum + `ALL` array from a single macro so new ids do not grow either file. |
| 722 | `src/i18n/message_id.rs` | i18n | `MessageId` enum; near-threshold historical file (main baseline 699, zero headroom). The compaction feature added 23 required user-facing message ids, crossing the threshold. Split plan: partition `MessageId` into domain sub-enums (or macro-generate it) as a follow-up i18n refactor. |
| 710 | `src/slash/hooks.rs` | slash | Slash hook command; pre-existing. Split subcommands. |
