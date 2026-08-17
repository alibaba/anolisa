# Adapter Lifecycle and Conformance

[中文版](adapter-conformance_zh.md)

Related architecture: [COSH Gateway and ACP Architecture](README.md)

## Purpose

An Agent adapter is supported only when its source, profile, protocol behavior,
failure behavior, and release evidence are reproducible. A successful prompt is
not sufficient conformance.

## Lifecycle

```text
source selection
  -> version lock
  -> staged installation
  -> provenance verification
  -> profile admission
  -> fake conformance
  -> real-agent conformance
  -> signed/offline release artifact
  -> upgrade or rollback
```

Gateway runtime does not invoke a package runner or network installer. Package
installation is a separate operator action.

## Installed profile

A profile records:

- stable profile name and Runtime kind;
- exact adapter package and version;
- canonical entry point and executable identity;
- trusted interpreter/package closure requirements;
- fixed arguments and working-directory policy;
- environment allowlist;
- ACP wire version and required capabilities;
- compatibility status for the upstream agent version.

Profile resolution occurs before production admission. A Task cannot override
the executable, arguments, environment, or workspace.

An installed Agent Runtime profile is distinct from a Gateway capability
profile. The first admits an agent process and wire contract; the second admits
a closed set of governed `ExecutionTarget` operations. Passing ACP adapter
conformance does not make `ws-ckpt`, shell, filesystem, or terminal execution
available. Release evidence records the tested combination of both profiles.

## Atomic installation

Installation uses a private managed prefix and stages a complete candidate
before publication. Verification covers package metadata, canonical binary,
file ownership/mode, and expected version. The installed marker is written last
and published atomically.

A failed or interrupted installation leaves either the previous verified
installation or no accepted installation. It must not leave a marker that
causes a partial tree to pass admission.

Release distribution should provide a signed or otherwise verifiable offline
artifact. Runtime network bootstrap is not an accepted fallback.

## Fake conformance

The deterministic fake adapter suite covers:

- initialize and required capability negotiation;
- Session creation and a bounded multi-chunk prompt;
- exactly one terminal result after buffered observations;
- batch requests and per-item errors;
- tool-use identity and monotonic revisions;
- correlated permission requests and one-time decisions;
- cancellation independent from a silent or blocked reader;
- malformed JSON, invalid UTF-8, oversized frame, stderr flood, early exit,
  timeout, and transport close;
- late update, late permission decision, and duplicate terminal rejection;
- process-group cleanup and exactly one reap.

Fake conformance is required for every change but does not prove compatibility
with a real provider.

## Real-agent conformance

At least one supported adapter runs on the exact candidate artifact with:

- version and profile identity recorded;
- initialize, Session creation, bounded text prompt, streamed updates, and one
  terminal outcome;
- independent cancel of active work;
- real `allow_once` and `reject_once` flows where supported;
- no filesystem or terminal capability outside the declared profile;
- sanitized evidence that excludes prompt, provider output, credentials,
  private paths, and proxy URLs.

Codex and Claude adapter claims are independent; success of one does not accept
the other.

## Failure and race matrix

Conformance records expected behavior for:

| Case | Expected result |
| --- | --- |
| Unsupported wire/capability | Fail before Session work |
| Silent initialization or prompt | Timeout, shutdown, and reap |
| Cancel vs completion | One terminal winner; late loser ignored or rejected |
| Permission during cancellation | No allow response after cancellation wins |
| Malformed stdout | Protocol failure and process-tree shutdown |
| Runtime exit before terminal | Deterministic failure with bounded diagnostics |
| Response loss after accepted callback | Durable replay without second write |
| Adapter path replacement | Launch pinned artifact or fail closed |

## Upgrade and rollback

An adapter upgrade is an explicit compatibility change. It reruns installer,
fake, and real conformance before publication. Rollback restores a previously
verified profile and does not rewrite Task or audit history.

Unsupported or regressed profiles are disabled. Gateway does not silently fall
back to another provider or ungoverned Runtime for an existing Task.

The same rule applies to capability providers. A missing `ws-ckpt` service can
make `ws-ckpt-v1` unavailable while leaving an explicitly selected
`task-only-v1` daemon valid. It cannot leave the checkpoint tool advertised or
redirect the operation to provider-native execution.

## Evidence package

Every accepted release records:

- candidate commit and artifact digest;
- adapter package, adapter version, and upstream agent version;
- operating environment and required Runtime capabilities;
- Gateway capability profile and exact ExecutionTarget provider versions;
- exact automated commands and result summary;
- manual steps and expected observations, when applicable;
- untested cases and rollback result.

Evidence is bounded and redacted. Secrets, raw prompts, and private provider
output are never committed to the public repository.

## Community contribution

Adapter contributions include the profile, provenance rule, fake fixture,
failure/race matrix, documentation, and real-agent evidence plan. Reviewers can
evaluate each layer independently without needing provider credentials for the
deterministic suite.
