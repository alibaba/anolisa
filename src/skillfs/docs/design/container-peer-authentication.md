# SkillFS Container Peer Authentication

[中文版](container-peer-authentication_zh.md)

Development plan for [issue #2439](https://github.com/alibaba/anolisa/issues/2439).
This document defines planned behavior; it must not be read as an available
deployment contract until the implementation and acceptance items are complete.

## Current development status

The SkillFS and agent-sec-core authentication primitives, control and notify
wiring, fail-closed configuration gates, documentation, and same-language
regression tests are implemented on the development branch. Fixed proof
vectors pin the Rust/Python byte contract.

The real Rust-to-Python and Python-to-Rust separate-container fixture, the
security-integrated Pod profile, and ACK evidence remain open. This issue must
stay open until those cross-component and deployment acceptance items pass.

## Outcome

Allow SkillFS and agent-sec-core to authenticate their private Unix-socket
traffic while running in separate PID and mount namespaces. Preserve the
current executable-identity authentication as the unchanged host default.

The first container profile retains the existing `shared_path` resolver
transport. SkillFS and agent-sec-core mount the same physical source at the
same absolute path, while the workload sees only the propagated FUSE view.

## Security boundary

The trusted domain contains the SkillFS and agent-sec-core containers. A
Kubernetes Secret volume and the physical source are mounted only into this
domain. A private runtime volume carries their Unix sockets.

An untrusted workload may know the socket path and may intentionally be given
the runtime volume in a negative test. It must still fail authentication when
it lacks the Secret, including when it uses the same UID, GID, process name, or
executable basename as the trusted peer.

Node root, a compromised trusted container, and a peer that can read the
Secret are outside this boundary.

## Profiles

### Host executable profile

This remains the default. SkillFS verifies `SO_PEERCRED`, `/proc/<pid>/exe`
path and file identity, configured UID/GID, and process start time. Existing
CLI, configuration, wire format, and failure behavior stay unchanged.

### Container HMAC profile

This profile is explicit and mutually exclusive with executable identity.
SkillFS and agent-sec-core load the same secret from an absolute, no-follow,
bounded regular file with restrictive permissions. UID/GID remain optional
additional constraints; they are not treated as container identity.

Each connection completes a bounded challenge-response exchange before an
existing business request is read or dispatched:

1. The client sends a bounded `auth.init` frame.
2. The server sends a fresh cryptographically random nonce.
3. The client returns a domain-separated HMAC-SHA256 proof.
4. The server compares the proof in constant time and returns its own
   domain-separated proof.
5. The client verifies the server proof before sending the existing NDJSON
   business frame.

Control and notify traffic use distinct domains. A connection is single-use,
so reconnect and process restart always require a fresh nonce. Authentication
errors close the connection without falling back to executable identity or
plain protocol handling.

The handshake uses a total deadline, not a timeout that restarts after each
byte. This bounds shutdown latency and prevents a peer from holding the
single-request control loop indefinitely with a slow partial frame. Socket
ownership and optional UID/GID constraints remain the first availability
boundary; the shared secret authenticates the peer after connection.

The authentication frames are NDJSON and use the following fixed envelope:

```json
{"authVersion":"1","type":"auth.init"}
{"authVersion":"1","type":"auth.challenge","nonce":"<base64>"}
{"authVersion":"1","type":"auth.proof","proof":"<base64>"}
{"authVersion":"1","type":"auth.ok","proof":"<base64>"}
```

The nonce is 32 random bytes encoded with padded standard Base64. Each proof
is `HMAC-SHA256(secret, domain || NUL || raw_nonce)`, also encoded with padded
standard Base64. The domains are:

- `anolisa.skillfs.control.client.v1`
- `anolisa.skillfs.control.server.v1`
- `anolisa.skillfs.notify.client.v1`
- `anolisa.skillfs.notify.server.v1`

After `auth.ok`, the peers exchange exactly the existing business NDJSON
request and response. This avoids cross-language JSON canonicalization and
keeps existing control schema v1 and notify schema v2 unchanged.

Secret material and reusable proofs must never appear in logs, responses,
audit events, protocol events, or checked-in deployment assets.

## Implementation phases

### Phase 1: shared authentication primitive

- Add strict secret-file loading and validation in SkillFS and agent-sec-core.
- Define compatible challenge and proof frames, byte limits, timeouts, domain
  strings, and fixed cross-language test vectors.
- Add constant-time proof verification and failure redaction.

### Phase 2: control resolver

- Add a mutually exclusive HMAC peer mode to the SkillFS control server.
- Add explicit socket and secret paths to the agent-sec-core resolver client.
- Authenticate before `ping`, `status`, resolver, or activation method
  dispatch while leaving the business schema unchanged.
- Retain the existing Flat, Hermes, fd-anchored resolution, and error mapping.

### Phase 3: notify direction

- Authenticate SkillFS as a client of the agent-sec-core daemon.
- Require authentication only for `skill_ledger.skillfs_notify_change` when
  the hardened mode is configured; unrelated daemon APIs keep their existing
  compatibility behavior.
- Authenticate the daemon response so a fake listener cannot acknowledge a
  notification.
- Fail startup when container HMAC control and notify are enabled together but
  the notify key is omitted, avoiding a partially authenticated profile.

Notification retry and durable reconcile are separate work. Authentication
failure retains the current rule that normal FUSE I/O continues and the active
mapping does not change.

### Phase 4: deployment and local acceptance

- Add a separate security-integrated Pod profile rather than changing the
  standalone Sidecar example.
- Use distinct source, propagated FUSE, runtime socket, and Secret volumes.
- Do not enable `shareProcessNamespace`.
- Add positive and negative local container tests with separate namespaces.
- Verify restart, readiness, resolver, notify, activation, denied workload,
  and clean-unmount behavior.

## Local acceptance

Run from `src/skillfs`:

```sh
cargo +1.86.0 fmt --all -- --check
cargo +1.86.0 clippy --workspace --all-targets -- -D warnings
cargo +1.86.0 test --workspace
cargo +1.86.0 doc --workspace --no-deps
scripts/test.sh
```

Run the targeted agent-sec-core formatter, lint, type, and pytest checks from
`src/agent-sec-core`, followed by the separate-container fixture. The fixture
must fail rather than skip authentication or namespace cases.

Required negative cases include missing, empty, short, oversized, symlinked,
over-permissive, and incorrectly owned secret files; wrong, malformed, stale,
or replayed proofs; authentication timeouts; UID/GID mismatch; a plain request
against HMAC mode; and a same-UID untrusted peer with socket access but no
Secret.

## ACK follow-up

After local completion, run a focused one-off validation on ACK and record:

- Kubernetes version, runtime, node architecture, manifest revision, and image
  digests;
- Secret, source, runtime, and propagated-volume visibility from every
  container;
- separate PID and mount namespaces without shared process namespace;
- resolver, notify, activation, readiness, and both sidecar restart paths;
- denial of an untrusted container that can reach the runtime socket; and
- Pod termination and residual-mount cleanup.

ACK results are release evidence, not recurring CI evidence. Do not describe
the security-integrated profile as released until this validation and the
remaining release gates in issue #2012 are complete.

## Deferred work

- `SCM_RIGHTS` or directory-fd resolver transport.
- Removing the physical source from agent-sec-core.
- Shared PID namespace as a supported security dependency.
- Durable notify queues or reconnect reconciliation.
- Multi-source registration, source hot refresh, CSI, and rootless FUSE.
