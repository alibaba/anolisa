# cosh-ng Architecture

[中文版](../../zh/cosh-ng/architecture.md)

cosh-ng separates the interactive terminal, Agent runtime, deterministic OS
API, and an emerging Gateway control plane so each boundary can be tested and
integrated independently. The Gateway material described below is a partial
candidate-worktree foundation, not an upstream production service.

## Upstream system view

```text
bash/zsh <--- cosh-shell
                  |
                  | JSONL
                  v
              cosh-core
                  |
                  +--> provider / tools / MCP
                  |
                  +--> cosh-platform ---> cosh-types

caller ---> cosh-cli ---> cosh-platform ---> cosh-types
```

The launcher installed as `cosh` normally executes `cosh-shell raw cosh-core`.
`cosh-shell` is compile-time independent of the other workspace crates, but it
owns a long-lived cosh-core child at runtime. The stdin/stdout protocol between
them must remain backward-aware because either side can fail or restart
independently.

The pinned upstream baseline for the Gateway plan is
`fa0c8369d300d90a6470965dc564e20b09487eb7`. It contains the five crates and
runtime path above, but no `cosh-gateway` or `cosh-gateway-contracts` crate.

## Candidate Gateway foundation

The shared candidate worktree based on that baseline adds two library crates:

```text
cosh-gateway-contracts --> TaskAggregate --> SQLite Task/event/receipt/Outbox transaction
        |
        +---------------> Capability Broker slice (in-memory, targeted tests)

cosh-gateway ----------> RuntimeSupervisor --> private COSH JSONL v1 codec
                                   `-------> official ACP wire-v1 codec/bridge
                                              + bounded session driver
                                              + fixed installed-adapter profiles

future CoshCoreBridge --> contracts public mapping + supervisor + codec

Gateway daemon/API, CoshCoreBridge, installed ACP entrypoint,
complete ACP domain/governance mapping, Shell attachment, and Web presentation
are not implemented.
```

The Task reducer and SQLite store are local control-plane foundations. The
Runtime supervisor owns a directly launched child process group, bounded
stdout/stderr, escalation/reap, and one process terminal observation. Its
cosh-core codec speaks the existing **private COSH control protocol v1**; it is
not ACP and is not yet mapped to public Runtime events.

No executable Gateway entry point or authenticated Unix/network API exists.
The current Shell path is unchanged: `cosh-shell` still owns its native PTY and
compatibility cosh-core process. The candidate pins official ACP Rust SDK 2.0.0,
raises the component baseline to Rust 1.88, and adds a supervised stable-v1
stdio slice plus built-in profiles for installed `codex-acp` and
`claude-agent-acp`. There is no package-runner or network bootstrap path. The
library still lacks an installed entrypoint, a session driver with independent
cancel, a production permission proxy, and real-adapter conformance evidence.

## Crate responsibilities

| Crate | Binary | Owns | Must not own |
|---|---|---|---|
| `cosh-types` | — | Side-effect-free response, error, config, audit, and checkpoint wire types | OS access or runtime policy |
| `cosh-platform` | — | Distro detection, package/service adapters, audit policy/store, ws-ckpt client | CLI rendering or Agent UX |
| `cosh-cli` | `cosh-cli` | Clap commands, JSON envelope, exit status | Distro-specific branching outside platform adapters |
| `cosh-core` | `cosh-core` | Providers, tool loop, hooks, Skills, MCP, extensions, registry, sessions, and compaction | Terminal ownership or foreground PTY interaction |
| `cosh-shell` | `cosh-shell` | PTY host, input routing, cards, approvals, evidence, UI, core process lifecycle | Provider implementation or direct OS API abstraction |
| `cosh-gateway-contracts` (candidate) | — | Side-effect-free Task, Runtime, Capability, identity, header, and error contracts with bounded leaf strings/digests | Storage, process ownership, transport, provider, OS execution, or aggregate admission limits not yet implemented |
| `cosh-gateway` (candidate) | — | Partial Task reducer/SQLite store, Runtime supervision/private core codec, ACP v1 codec/bridge and fixed installed-adapter profiles, and Capability integration slice | Shell PTY, installed Gateway/ACP entrypoints, provider/ACP wire types as domain contracts, OS effects outside the Broker, or ungoverned ACP callbacks |

## Interactive data flow

1. `cosh-shell` starts bash/zsh in a PTY and installs OSC lifecycle markers.
2. Input routing sends shell syntax to the PTY, slash commands to the local
   control surface, and natural language to the Agent adapter.
3. The default adapter maintains a cosh-core process and sends one JSONL user
   message per Agent turn.
4. cosh-core resolves workspace config, the provider, Skills, extensions, MCP
   tools, and session state, then streams events back.
5. cosh-shell governs those events and renders text, question cards, or approval
   cards.
6. Approved shell execution is handed back to the foreground PTY. OSC evidence
   is correlated with the Agent run and returned to core when requested.
7. Registry mutations such as extension reload use the same long-lived core
   and publish changes at a safe generation boundary.

## Deterministic CLI data flow

```text
Clap command
  → command module validates arguments
  → cosh-platform selects the backend
  → backend returns typed data or CoshError
  → cosh-cli emits CoshResponse<T>
  → exit 0 on success, exit 1 on operation failure
```

Package and service writes support `--dry-run`. Checkpoint calls cross a Unix
socket using bincode with a four-byte little-endian length prefix.

## cosh-shell ownership map

| Owner | Responsibility |
|---|---|
| `shell_host/` | PTY lifecycle, OSC parsing, shell integration, raw relay |
| `raw_input/` and `input/` | terminal modes, multiline input, input relay |
| `slash/` | slash parser, registry, and command-specific presentation |
| `adapter/` | provider/core adapters and control protocol transport |
| `agent/` | Agent run lifecycle and governed events |
| `runtime/` | orchestration, shared state, dispatch, and startup |
| `approval/` and `question/` | user decisions and control responses |
| `hooks/` | hook policy and execution; hands mutations to runtime boundaries |
| `tools/` | command risk model, read-only rules, tool presentation |
| `ui/` | terminal rendering and card components |
| `evidence/`, `journal/`, `ledger/` | bounded evidence and decision records |

New implementation files do not belong at the `cosh-shell/src/` root. Keep
owner boundaries visible and run `crates/cosh-shell/scripts/check-layout.sh`
after structural changes.

## Compatibility and safety contracts

- `CoshResponse<T>` is the stable automation envelope.
- ws-ckpt enum order is part of the binary wire format.
- cosh-core messages are newline-delimited JSON; stdout must not contain logs or
  UI prose in headless mode.
- A running Agent turn is pinned to its registry generation. A healthy candidate
  activates immediately only when idle; otherwise it waits for a safe point.
- Session state is workspace-scoped. Recovery restores model-visible
  conversation, not historical terminal evidence.
- Core read tools are pinned to the canonical startup workspace. A later `cd`
  changes the shell directory, not the read boundary; path and mount escapes
  fail closed.
- Foreground shell handoffs are serialized. Input-wait timeouts apply only when
  kernel evidence shows a foreground process waiting for input; pipelines and
  full-screen programs are exempt.
- Linux package routing may use the first recognized `ID_LIKE` family while
  preserving the distribution's real `ID` in typed and JSON output.
- Tool auto-approval fails closed. Raw command substring matching is not a
  security boundary.

## Gateway and ACP delivery boundary

The candidate libraries do not form a durable production Gateway. They still
lack the Gateway API/daemon, Task coordinator and lease/recovery loop, complete
Capability enforcement, integrated CoshCore Bridge, installed ACP Runtime
entrypoint, production permission UI/evidence, real-adapter evidence,
Shell attachment, and Web/channel presentation. The
[ACP v1 Phase 0-2 planning set](../../../../src/cosh-ng/docs/design/acp-v1-phase-0-2/README.md)
separates the pinned upstream baseline from candidate implementation evidence
and defines the remaining module boundaries, Warp comparison, delivery
sequence, and acceptance gates. Overall Phase 0-2 status remains **NOT
ACCEPTED**.

Continue with [Developing cosh-ng](getting-started.md), [IPC protocols](ipc-protocol.md),
and [Testing](testing.md).
