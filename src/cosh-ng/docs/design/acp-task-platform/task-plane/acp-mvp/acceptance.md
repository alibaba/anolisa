# ACP v1 Local Runtime MVP Acceptance Report

[中文版](acceptance_zh.md) | [Design](design.md) |
[Planning set](../../README.md)

## Result

**PARTIAL IMPLEMENTATION / NOT ACCEPTED.** The candidate has a strict ACP v1
codec, supervised stdio bridge, bounded session driver with independent
cancellation, an installed COSH entrypoint, and built-in profile resolution
for `codex-acp` and `claude-agent-acp`. A source-tree installer pins the
official Adapter packages, a deterministic harness exercises the entrypoint,
and a local once-only permission proxy writes redacted evidence before reply.
Dirty-worktree exploratory runs observed both pinned Adapters and several Codex
scenarios, but they are not accepted real Codex/Claude conformance or manual
Terminal evidence. Reproducible evidence tied to an exact candidate commit,
with the required sanitized artifacts and complete scenario matrix, remains.

This gate is independent from complete G1 and G2 acceptance. Passing it will
prove only the narrow local interoperability outcome defined in the design.

## Status vocabulary

| Status | Meaning |
| --- | --- |
| `PASS` | Exact candidate evidence satisfies the whole MVP criterion |
| `PARTIAL` | A bounded source/test slice exists, but the user path or required proof is incomplete |
| `FAIL` | Implemented behavior was exercised and contradicted the criterion |
| `NOT IMPLEMENTED` | The required production surface does not exist |
| `NOT RUN` | The surface exists, but required evidence was not executed |

## Current evidence

| Area | Current status | Evidence and gap |
| --- | --- | --- |
| ACP v1 codec | `PARTIAL` | Exact wire v1 initialization, one session, text prompt/update/stop, bounds, and malformed-input handling have focused fixtures; no real adapter run |
| Supervised stdio | `PARTIAL` | The bridge composes one supervisor and a bounded driver with deadlines/backpressure; broader race and process-tree fixtures remain |
| Runtime profiles | `PARTIAL` | The installed entrypoint descriptor-pins the canonical executable inode and workspace; the workspace digest binds path/device/inode, with fixed args and an environment allowlist. Signed/offline distribution remains open |
| Streaming | `PASS for local slice` | The bounded driver assigns monotonic local sequence and uses RAII to release queue/byte budgets on success, parse failure, sink rejection, cancel, and terminal paths |
| Cancellation | `PARTIAL` | Independent control reaches a silent Agent, settles pending permission callbacks, and reaps the process; wider race coverage remains |
| Permission correlation | `PASS for local slice` | Local TTY presentation retains only correlated `allow_once`/`reject_once`; non-TTY, EOF, unsupported-only options, and explicit deny cancel |
| Permission evidence | `PASS for local slice` | Private append-only JSONL records bounded hashes, actor UID, and decision class before reply; raw prompt/tool/session/workspace values are excluded |
| Unsupported callbacks | `PARTIAL` | Fake fs request receives correlated method-not-found; complete fs/terminal non-advertisement matrix remains |
| Adapter distribution | `PARTIAL` | Source installer uses an exact npm lockfile, a private managed prefix, scripts disabled during install, and package/version/bin provenance checks; the repository does not yet distribute a signed offline Adapter artifact |
| Real adapter conformance | `PARTIAL` | Exploratory dirty-worktree observations exist, but real Codex/Claude conformance and manual Terminal validation remain unaccepted |
| Rollback | `PARTIAL` | Existing direct `cosh-shell raw cosh-core` path remains and raw-package routing is tested; an installed-package smoke remains |

Source presence is not user-facing acceptance. Profile resolver tests that use
temporary executable files do not prove an installed official adapter works.

## Acceptance matrix

| ID | Criterion | Current result | Required proof |
| --- | --- | --- | --- |
| MVP-01 | One installed COSH entrypoint accepts a built-in profile, canonical workspace, and bounded text prompt | `PARTIAL` | Entrypoint and fake-path evidence exist; accepted installed-profile proof at an exact candidate commit remains |
| MVP-02 | Only locally installed `codex-acp` or `claude-agent-acp` is launched; native Codex/Claude, `npx`, shell, package runner, and network bootstrap are impossible | `PARTIAL` | Runtime resolver never bootstraps packages; the explicit installer pins and verifies both Adapter packages, but signed/offline distribution remains |
| MVP-03 | Profile resolution pins exact basename, canonical executable/workspace, fixed args, and allowlisted environment without logging values | `PARTIAL` | Descriptor-pinned replacement, workspace-inode identity, spoof/path/environment tests pass; signed package provenance remains a Gate |
| MVP-04 | Driver performs ACP v1 initialize, one session/new, and one active text prompt in order | `PARTIAL` | End-to-end driver fixture with wrong-order and duplicate-prompt negatives |
| MVP-05 | Text updates are delivered in receive order with bounded local sequence, queue depth, and bytes | `PASS for local slice` | Multi-chunk, sequence exhaustion, aggregate-byte, sink-failure, cancel, and terminal RAII matrix |
| MVP-06 | Every turn reports exactly one terminal result and rejects late updates | `PARTIAL` | ACP failure matrix covers completion/cancel/error/exit/timeout, late response, and cleanup; real-Adapter race evidence remains |
| MVP-07 | Cancel reaches the driver while Agent stdout is silent and settles protocol/process state within configured bounds | `PARTIAL` | Independent-control fake-Agent test passes; completion/cancel race matrix remains |
| MVP-08 | Cancel settles every pending permission and no late decision or update can authorize work | `PARTIAL` | Permission-during-cancel and late-response race fixtures |
| MVP-09 | Permission proxy offers only correlated `allow_once` and `reject_once`; `allow_always` and `reject_always` cannot create a decision or rule | `PASS for local slice` | Seven focused proxy/evidence tests plus non-interactive entrypoint cancellation |
| MVP-10 | Permission evidence is bounded, redacted, and records request correlation plus decision class | `PASS for local slice` | Private-file, symlink, mode, secret exclusion, control-injection, and entrypoint evidence tests |
| MVP-11 | fs, terminal, load, resume, rich content, additional directories, and multiple sessions remain unadvertised and fail closed | `PARTIAL` | Complete capability/request negative matrix with zero host I/O |
| MVP-12 | Malformed/oversized/invalid UTF-8/contaminated stdout, stderr flood, child exit, and timeout terminate safely with one reaped child | `PARTIAL` | Adversarial process fixtures and leak assertions |
| MVP-13 | At least one installed real adapter completes initialize, prompt, multiple streamed text updates, terminal result, active cancel, allow once, and reject once | `PARTIAL` | Dirty-worktree observations are exploratory; reproducible real-adapter and manual Terminal evidence at an exact candidate SHA remains |
| MVP-14 | Disabling or not selecting ACP preserves the current direct cosh-core path | `PARTIAL` | Installed rollback smoke test |
| MVP-15 | English/Chinese MVP and aggregate documents remain semantically equivalent and all relative links resolve | `PASS for document slice` | Documentation checks recorded below |

MVP-01 through MVP-15 are mandatory. MVP-13 may use either official adapter,
but the acceptance report must state which profile passed. The other profile
remains `NOT RUN` or records its own result.

## Required automated evidence

The implementation report must record exact commands and counts for equivalent
coverage:

```text
profile resolver unit tests
ACP codec and supervised bridge tests
session driver protocol tests
installed local entrypoint integration tests
silent-Agent cancellation race tests
permission allow/reject/cancel tests
malformed-output and process-leak tests
rollback smoke test
```

The fake-Agent corpus must include:

- normal initialization and at least two text chunks;
- wrong version, malformed JSON, invalid UTF-8, stdout log contamination,
  oversized frame, stderr flood, and early exit;
- a silent prompt that is cancelled through the independent control handle;
- allow-once, reject-once, unsupported-only options, duplicate IDs, late
  decisions, and cancellation while permission is pending;
- unadvertised filesystem, terminal, load, and resume requests with proof that
  no host callback executed;
- output saturation and cancellation/completion races.

## Required real-adapter evidence

Acceptance requires one locally installed `codex-acp` or
`claude-agent-acp`. The evidence package records:

1. full candidate commit SHA and operating-system environment;
2. selected profile and canonical adapter path without credentials;
3. adapter executable version and installation source;
4. exact COSH entrypoint commands for normal prompt and cancellation;
5. sanitized transcript proving initialization, at least two ordered text
   updates, one terminal, allow once, reject once, and active cancellation;
6. confirmation that no `npx`, download, network bootstrap, filesystem callback,
   or terminal callback was used by COSH;
7. any unsupported or untested behavior for the other built-in profile.

Provider output, prompts, credentials, environment values, host identifiers,
and private workspace contents must be removed from evidence.

## Stage 2 permission evidence

`cosh-gateway run` defaults to local `/dev/tty` presentation and offers only
Agent-provided `allow_once` and `reject_once` choices. `--permission deny`, no
TTY, EOF, invalid input, and unsupported-only options cancel without
authorization. The callback reply is sent only after a private append-only
JSONL record is synchronized. That record contains correlation hashes, actor
UID, profile, time, and decision class; it excludes raw prompt, tool arguments,
option labels, provider session identifiers, and workspace paths.

Targeted Stage 2 checks:

```bash
cargo +1.88.0 test --locked --package cosh-gateway permission:: --lib
cargo +1.88.0 test --locked --package cosh-gateway \
  --test cli_entrypoint --bin cosh-gateway
```

## Stage 3 Adapter evidence

The source installer pins `@agentclientprotocol/codex-acp` at `1.2.0` and
`@agentclientprotocol/claude-agent-acp` at `0.66.0` through the committed
lockfile. It accepts only an explicit absolute private prefix and rejects a
symlinked, non-owned, group/world-accessible, or unrelated non-empty prefix.
After `npm ci --ignore-scripts`, it verifies each package name, version, and
canonical `bin` target. npm is never available to the COSH runtime resolver.

Fake conformance validates initialization, session creation, two ordered text
chunks, prompt completion, and exactly one terminal event. Real mode is
opt-in, requires a piped prompt plus `--acknowledge-provider-run`, verifies
exact package provenance, and reduces JSONL to event counts in memory. It does
not create an evidence file or echo prompt or Agent text.

```bash
bash src/cosh-ng/tests/test-acp-adapters.sh
src/cosh-ng/scripts/run-acp-conformance.sh fake \
  --gateway "$PWD/src/cosh-ng/target/debug/cosh-gateway" \
  --workspace "$PWD"
```

These checks use a fake npm implementation and deterministic fake Agent. They
do not install from the network or invoke a provider.

## External gates remain unaccepted

Descriptor pinning and the deterministic ACP failure matrix are local source
evidence. Production `serve` does not admit ACP profiles; ACP is exposed only
through the explicitly ungoverned `doctor` and `run` interoperability commands.
No exploratory observation may be promoted into governed-daemon evidence.

The worktree is uncommitted and has no exact candidate SHA. Reproducible real
Codex and Claude conformance, manual `/dev/tty` permission/cancellation runs,
a signed offline Adapter artifact, and retained sanitized evidence remain
separate external gates. Provider output, prompts, credentials, environment
values, host identifiers, and workspace contents are not retained here.

## Exit criteria

The ACP MVP is accepted only when:

1. MVP-01 through MVP-15 are `PASS` on one exact candidate commit.
2. The installed entrypoint and fake-Agent failure/race suite pass with exact
   counts.
3. At least one real official adapter passes the complete prompt, stream,
   cancel, allow-once, and reject-once scenario.
4. The acceptance report names every timeout, frame, queue, stderr, and shutdown
   bound used by the passing revision.
5. The report states explicitly that the result is not G1/G2, durable
   governance, filesystem/terminal, Web, Shell attachment, or daemon acceptance.

## Documentation validation

The bilingual documents must pass repository docs lint, relative-link checking,
pairing/parity review, and `git diff --check`. Real Codex/Claude, ECS, and manual
Terminal evidence remain separate unaccepted gates; exploratory observations
cannot make MVP-13 pass.
