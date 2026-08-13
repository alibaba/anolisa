# ACP v1 Local Runtime MVP Design

[中文版](design_zh.md) | [Acceptance report](acceptance.md) |
[Planning set](../../README.md)

## Status and delivery decision

This module defines a narrow delivery gate for launching one locally installed
ACP adapter and completing one text turn. It is smaller than the complete Phase
2 ACP, Shell attachment, Web, durable Gateway, and OS-governance gates.

The candidate worktree already provides:

- an official ACP Rust SDK 2.0.0 codec for stable ACP wire version 1;
- a synchronous `AcpV1RuntimeBridge` composed with one
  `RuntimeSupervisor` that owns the child process, stdio, process group, and
  reap result;
- bounded initialization, one session, one active text prompt, streamed
  `session/update`, permission correlation, cancellation frames, and
  fail-closed decoding;
- built-in `Codex` and `ClaudeCode` profile resolution for the locally
  installed `codex-acp` and `claude-agent-acp` adapters.

These are library-level foundations. No installed COSH entrypoint selects a
profile and drives a complete turn. The current synchronous bridge does not
provide an independent cancellation control path while its owner is blocked
reading Agent stdout. Permission responses are correlated ACP wire responses,
not yet a COSH approval or Capability Broker decision. No real adapter evidence
has been recorded.

## MVP outcome

An installed COSH command can select one built-in profile, pin one canonical
workspace, launch the corresponding local adapter over stdio, send one text
prompt, stream ordered text updates, resolve or reject a once-only permission
request, cancel independently, and report one terminal result.

At least one of the two real adapters must pass the complete acceptance matrix
before this MVP is accepted. Supporting both profiles in source does not imply
that both adapters passed live conformance.

## Scope

The complete MVP profile is deliberately fixed:

| Dimension | MVP contract |
| --- | --- |
| Transport | Local subprocess stdio with newline-delimited ACP v1 JSON-RPC |
| Adapter profiles | Installed `codex-acp` and `claude-agent-acp` only |
| Workspace | One canonical absolute directory fixed before launch |
| Connection | One supervised adapter process and ACP connection |
| Session | One opaque ACP session per driver |
| Concurrency | One active prompt and one ordered event stream |
| Prompt content | Non-empty bounded UTF-8 text only |
| Permission | Offered once-only allow or reject decisions |
| Cancellation | Independent control command with bounded escalation and reap |
| Presentation | Bounded text/events and safe diagnostics at the local entrypoint |

## Explicit non-goals

The MVP does not include:

- native ACP support in the Codex or Claude Code binaries;
- downloading or executing adapters through `npx`, a shell, a package runner,
  or any network bootstrap;
- filesystem callbacks, terminal callbacks, rich prompt content, additional
  directories, session load, session resume, or multi-session operation;
- `allow_always`, `reject_always`, durable trust rules, or policy mutation;
- Web, channel, Shell attachment, Gateway daemon, remote transport, or
  cross-device replay;
- durable Task recovery, Run leases, process-transparent restart, or complete
  Capability Broker governance.

Unsupported features stay unadvertised. An Agent request for an unadvertised
filesystem or terminal method receives a correlated method-not-found response
and never reaches host I/O.

## Runtime profile boundary

The built-in profile resolver is the only MVP adapter selection authority:

| Profile ID | Required executable | Launch rule |
| --- | --- | --- |
| `Codex` | `codex-acp` | Resolve an installed regular executable with the exact basename |
| `ClaudeCode` | `claude-agent-acp` | Resolve an installed regular executable with the exact basename |

An explicit executable must be absolute. Implicit resolution searches only
absolute `PATH` entries. The resolver canonicalizes the executable and
workspace, uses fixed empty argument lists, clears inherited environment, and
copies only the common and profile-specific allowlisted variables. Prompts,
ACP payloads, and adapter output cannot add process arguments, replace the
executable, or change the workspace.

The adapter executables are separate installed adapters. Documentation and UI
must not claim that the native `codex` or `claude` command implements ACP.

## Local entrypoint

The MVP requires one installed COSH-owned entrypoint with the conceptual input:

```text
RunAcpPrompt {
  profile,
  workspace,
  prompt
}
```

The final executable and flag spelling is an implementation decision, but the
entrypoint must:

1. accept only a built-in profile ID;
2. resolve the profile before spawning;
3. keep adapter installation external and return a typed missing-adapter error;
4. expose streamed updates, permission requests, cancellation, and one terminal
   result without exposing SDK objects as a public COSH contract;
5. leave `cosh-shell raw cosh-core` unchanged when the ACP entrypoint or profile
   is not selected.

The entrypoint is local process orchestration, not the Phase 1 authenticated
Gateway API or a daemon.

## Session driver ownership

The MVP adds one driver above the current bridge:

```text
local entrypoint
  -> profile resolver
  -> ACP session driver
       -> AcpV1RuntimeBridge
            owns AcpV1Codec + RuntimeSupervisor
                 owns child + stdio + process group + reap
```

This composition is the current ownership model. The bridge owns its embedded
supervisor; it does not borrow a channel from a separate daemon supervisor.
There is exactly one process owner and one codec owner.

The session driver owns command serialization, event sequencing, deadlines,
and the independent cancellation handle. It performs:

```text
resolve profile
  -> launch adapter
  -> initialize(protocolVersion = 1)
  -> session/new(canonical workspace)
  -> session/prompt(text)
  -> zero or more ordered updates/permission requests
  -> prompt terminal, cancellation settlement, or transport failure
  -> shutdown and reap
```

Only the driver task/thread mutates the bridge. A separate control handle sends
cancel into the driver command queue, so cancellation remains available while
the driver is waiting for Agent stdout. A design that requires acquiring the
same blocked `&mut` bridge directly does not meet the MVP.

## Streaming and terminal semantics

- Each accepted `session/update` receives one monotonically increasing local
  sequence before delivery.
- The MVP presents only text agent-message chunks. Other valid updates are
  bounded diagnostic events or explicit unsupported events; they are not
  silently converted to text success.
- Queue depth and byte limits are explicit. Saturation cancels and fails the
  turn instead of buffering without a bound.
- Exactly one terminal result is delivered for the prompt: completed,
  cancelled, Agent error, protocol failure, process exit, or timeout.
- Updates received after terminal settlement are rejected and cannot change the
  reported result.
- Raw prompts, environment values, unrestricted stderr, and adapter payloads
  are absent from retained evidence.

## Independent cancellation

Cancellation is accepted from the local entrypoint even when no Agent update is
arriving. The driver:

1. records a local cancellation request;
2. sends ACP `session/cancel` and cancelled outcomes for every pending
   permission callback;
3. waits a bounded protocol grace for the prompt to settle;
4. closes or stops protocol input when retiring the connection;
5. escalates to process-group termination and kill through the embedded
   `RuntimeSupervisor`;
6. reaps the child and reader state;
7. emits one cancelled or explicit cleanup-failure terminal result.

Cancellation races with prompt completion use first-terminal-wins semantics.
No permission response or Agent update may authorize work after cancellation
has won.

## Permission proxy

The MVP permission boundary is a local, once-only permission proxy. It is not a
claim of durable Task approval or complete Capability Broker governance.

For each `session/request_permission`:

1. validate the active session, prompt, JSON-RPC request ID, tool call, and
   unique option IDs;
2. retain only offered `allow_once` and `reject_once` choices;
3. present untrusted labels as display data;
4. accept one local user decision correlated to that request;
5. return the selected offered option or a cancelled/rejected outcome;
6. reject duplicates, unknown options, late decisions, and cross-session IDs.

`allow_always` and `reject_always` are not offered by COSH in the MVP and cannot
create a durable rule. If the Agent supplies only unsupported choices, the
request fails closed. The evidence record contains bounded correlation and the
decision class, not raw tool input or credentials.

## Failure boundaries

| Failure | Required result |
| --- | --- |
| Adapter missing or wrong basename | Fail before spawn with a typed profile error |
| Workspace missing or not a directory | Fail before spawn |
| Wrong ACP version or initialization timeout | Terminate and reap; compatibility failure |
| Malformed, oversized, invalid UTF-8, or contaminated stdout | Fail closed and terminate the process group |
| Stderr flood | Retain only a bounded safe tail; never parse it as ACP |
| Agent exits during prompt | One transport/process terminal; never infer success |
| Permission has no supported once option | Reject or cancel without authorization |
| Cancel arrives while stdout is silent | Driver receives it independently and settles within bounds |
| Output queue saturates | Cancel/fail with a stable overload result |
| Unsupported callback | Correlated method-not-found; no host side effect |

## Delivery sequence

1. Freeze this MVP contract and the entrypoint/event/error vocabulary.
2. Keep the existing resolver and bridge composition; document exact ownership.
3. Add the session driver and independent control channel.
4. Add the installed local entrypoint and safe presentation.
5. Add the once-only permission proxy and evidence record.
6. Complete deterministic fake-Agent failure/race coverage.
7. Run exact-revision conformance against at least one installed official
   adapter and record sanitized evidence.

## Relationship to later gates

Passing this MVP proves local ACP prompt, stream, cancel, and once-only
permission interoperability. It does not pass G1 or G2. Durable Task mapping,
Capability Broker authorization, filesystem/terminal callbacks, restart,
Shell/Web attachment, and remote presentation retain their existing module
acceptance criteria.
