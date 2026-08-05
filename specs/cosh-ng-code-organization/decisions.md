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

## D15: ApprovalActionSet lives in ui/agent_render/actions.rs (owner note)

`ApprovalActionSet` (Hook / Standard / TurnConsent) is a presentation
contract: it enumerates which action labels a card renders, in which
order, and how linear indices map to actions. It is defined next to the
`*_PANEL_ACTIONS` descriptor tables it selects between, because the two
must evolve together (adding an action means adding a descriptor).

The policy decision — which request gets which set — is owned by
`approval/panel.rs::approval_action_set_for` (approval owner).
`raw_input/` and `runtime/` consume the enum only to interpret or relay
already-decided sets; they never decide. This keeps standard.md §2
satisfied: `ui/` holds no policy, and the cross-owner references are
reads of a display contract, mirroring how `ApprovalPanelAction` has
always been shared.

Revisit trigger: if a non-UI consumer ever needs to construct or branch
on sets without rendering context, move the enum to `types/` as an
explicit cross-module contract in a dedicated refactor.

## D16: turn-extension approval orchestration (bounded exception)

Registered: 2026-07-30, max-turn continuation.

Decision: `agent/turn_extension.rs` may call approval request rendering and
resolution helpers, while `approval/runtime.rs` may call the turn-extension
resolver. This bidirectional edge is limited to the lifecycle of a retained
max-turn run: record a continuation candidate, request explicit user consent,
then resume the same provider session once.

Reasons:

- The approval owner remains responsible for presenting and recording the
  decision; the agent owner remains responsible for starting the continuation.
- Moving this single flow into a new coordinator would add another runtime
  abstraction without removing state or policy from either owner.
- The edge is confined to the turn-extension request kind and does not expose
  provider protocol details or UI policy across owners.

Removal condition: when a second cross-owner approval workflow needs the same
lifecycle, introduce runtime domain commands and events for approval outcomes,
move orchestration into that coordinator, and delete this exception.

## D17: ApprovalLifecycleLedger lives in runtime/approval_ledger.rs (owner note)

`ApprovalLifecycleLedger` (#1940) is a pure accounting index keyed by
the #1939 identity contract (`run_id` + `request_id`): registered on
first sight, marked on response, swept per run. It lives in `runtime/`
because its owner is the run lifecycle, not approval policy — the
ledger is held by `ControlState` (`runtime/state.rs`), and its two
sweep triggers are runtime lifecycle events (`runtime/cancel.rs`,
`runtime/evidence_delivery.rs`). It depends only on std and holds no
policy: what a dropped request is denied *with* (message, audit
drop-site, terminal deny) is decided by `approval/runtime.rs`, which
consumes the ledger the same way it already consumes other
`ControlState` accounting. Splitting the data structure from its
`ControlState` host would recreate the cross-owner reach the note is
about; the maintenance contract (every `control_response` exit must
`mark_responded`) is documented in the module docs.

Revisit trigger: if a second consumer beyond approval drain/sweep needs
lifecycle bookkeeping, or the per-domain `ControlState` split lands,
move the ledger into that extracted state module together with its
host field.

## D18: interactive-sentinel hint card renders inside shell_host (owner note)

The #2025 interactive sentinel (`shell_host/raw_relay/interactive_sentinel.rs`)
builds and emits its one-shot hint card itself — hand-assembled ANSI
border, width clipping and i18n copy — instead of routing through the
`ui/` renderer like the sibling #2161 timeout notice
(`runtime/controller/input_wait.rs`, which goes through
`RatatuiInlineRenderer::write_notice_panel`). The exception exists
because the card must be spliced into the raw PTY byte relay at a
precise stream position: the relay owns the display buffer, the
alt-screen state and the in-stream timing (card + prompt-tail redraw
must land between two output chunks), none of which are visible to the
inline renderer, and handing the relay's byte stream to `ui/` would
reverse the allowed dependency direction. The card model (kind →
message IDs) stays declarative and the copy lives in `i18n/`.

Removal condition: if a second in-stream card appears in `shell_host`,
extract a shared presentation helper (e.g. `types/` card model +
byte-emitting formatter) and move both emitters onto it.
