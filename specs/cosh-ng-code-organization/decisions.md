# cosh-shell Code Organization Decisions

> In-repo registry for organization-standard exceptions that reviewers must be
> able to discover next to the code. Each entry records the decision, the
> reason it is acceptable, and the conditions under which it is removed.

## D13: activity -> runtime reverse dependency (temporary exception)

Registered: 2026-07-25, shell-handoff-preexec-loss.

Decision: the `activity` owner's existing dependency on `runtime`
(`crate::runtime::prelude::*`, `crate::runtime::state::PendingInteractiveShellHandoff`,
`crate::runtime::evidence_delivery::record_shell_handoff_completion`) is
registered as a known temporary exception to the `runtime -> activity`
one-way dependency direction and is not a blocking finding for new PRs.
The identical imports in `activity/shell_handoff.rs` were migrated verbatim
from `activity/runtime.rs` during a file-level split and do not add a new
reverse edge.

Reasons:

- The reverse dependency is a pre-existing global pattern on the `main`
  baseline: every module under `activity/` (`runtime`, `runtime_render`,
  `runtime_tests`, `tool_invocation`, `tool_result_summary`) already imports
  `crate::runtime::prelude::*`, and `activity/runtime.rs` has referenced
  `record_shell_handoff_completion` and `PendingInteractiveShellHandoff`
  since before this exception was registered.
- File-level splits do not change the module dependency graph: the
  `activity -> runtime` edge exists identically before and after the split.
- Removing the edge requires decoupling the runtime prelude and injecting the
  evidence-recording responsibility through a trait or closure, touching every
  `activity` module and the runtime state model — an architecture-level change
  that must not ride along inside a bugfix PR.

Removal conditions (follow-up SDD):

- Route the activity-side state updates of `record_shell_handoff_completion`
  through a forward `runtime -> activity` call (closure or trait injection) so
  `activity` no longer holds references to runtime-internal functions.
- Move `activity` modules onto explicit narrow interfaces (`types/` contracts
  or dedicated context parameters) instead of the blanket
  `runtime::prelude` import.
- Once complete, delete this entry and restore the exception-free one-way
  dependency wording in the organization standard.

## D14: shell handoff untracked-status string contract (registered debt)

Registered: 2026-07-25, shell-handoff-preexec-loss.

Decision: `types::shell_handoff::SHELL_HANDOFF_UNTRACKED_STATUS`
(`pub(crate) const &str = "completed_untracked"`) is the cross-owner
contract token for a shell handoff closed at a prompt boundary without
preexec tracking. Consumers: `activity/shell_handoff.rs` (row status),
`runtime/evidence_delivery.rs` (provider notice gating), and
`ui/agent_render/activity.rs` (status label match arm). Following the
D11 precedent for intent string constants, the token stays a `&str`
constant rather than an enum variant because it is a persisted
projection/render value consumed by activity rows and the journal;
enum promotion would require a serialization mapping across owners
without adding compile-time safety beyond what the shared constant
already provides (all comparisons and the ui match arm reference the
constant, so typos fail to compile).

Stability: the literal value is frozen; renaming it is a
projection-format change and requires a dedicated SDD.
