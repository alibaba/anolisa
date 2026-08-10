# cosh-ng Architecture

[中文版](../../zh/cosh-ng/architecture.md)

cosh-ng separates the interactive terminal, Agent runtime, and deterministic OS
API so each boundary can be tested and integrated independently.

## System view

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

## Crate responsibilities

| Crate | Binary | Owns | Must not own |
|---|---|---|---|
| `cosh-types` | — | Side-effect-free response, error, config, audit, and checkpoint wire types | OS access or runtime policy |
| `cosh-platform` | — | Distro detection, package/service adapters, audit policy/store, ws-ckpt client | CLI rendering or Agent UX |
| `cosh-cli` | `cosh-cli` | Clap commands, JSON envelope, exit status | Distro-specific branching outside platform adapters |
| `cosh-core` | `cosh-core` | Providers, tool loop, hooks, Skills, MCP, extensions, registry, sessions, and compaction | Terminal ownership or foreground PTY interaction |
| `cosh-shell` | `cosh-shell` | PTY host, input routing, cards, approvals, evidence, UI, core process lifecycle | Provider implementation or direct OS API abstraction |

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

Continue with [Developing cosh-ng](getting-started.md), [IPC protocols](ipc-protocol.md),
and [Testing](testing.md).
