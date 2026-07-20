# ActPlane File-Enforcement Compatibility Profile

[中文版](actplane-file-enforcement-profile_zh.md)

## Status

Implemented and validated on Alibaba Cloud Linux with kernel
`6.6.102-6.alnx4.x86_64`. This is a temporary compatibility layer for the
official ActPlane revision `a62e5d9d96f91101cda019519053e950d532380a`; it
should be removed after equivalent behavior is accepted upstream.

## Decision

AgentSight keeps ActPlane as a revision-pinned Cargo dependency instead of
vendoring its source or adding a Git submodule. The enforcer build fetches that
exact upstream revision and applies the reviewed patch queue in
`patches/actplane/`. A patch that no longer applies or a source tree containing
any unreviewed change stops the build, making upstream drift explicit.

The systemd service selects `ACTPLANE_PINNED_PROFILE=file-enforcement`. This
profile reserves only the maps, tracepoints, and BPF-LSM program required for
the first gradual-enforcement scenario: blocking an Agent process from opening
a configured sensitive file. Startup fails if BPF LSM is inactive, the profile
name is unknown, or an existing bpffs singleton has a different hook set.
The singleton also carries a pinned metadata alias containing its ActPlane
revision, profile, and compatibility schema; missing or mismatched metadata is
treated as an explicit migration error.

## Why a Reduced Profile Is Required

The pinned upstream full profile cannot load on the validated kernel for two
independent verifier reasons:

1. The `trace_openat_exit` and `trace_rename_exit` programs require a 544-byte
   combined BPF stack, while the kernel verifier limit is 512 bytes.
2. `enforce_path_truncate` calls `bpf_d_path`, which is not permitted for that
   BPF-LSM hook on the validated kernel.

The reduced profile does not weaken the declared file-open policy. It avoids
loading unrelated data-flow tracepoints and file hooks that are outside the
current product scope. Network blocking, process execution blocking, taint
propagation, and temporal policy enforcement remain follow-up profiles and
must not be presented as enabled by this profile.

## Build Contract

`make build-enforcer` invokes `scripts/build-enforcer.sh`. The script:

1. uses the exact ActPlane revision declared by the AgentSight Cargo workspace;
2. accepts `ACTPLANE_SOURCE_DIR` for an approved mirror or pre-fetched checkout;
3. otherwise performs a shallow fetch into the Cargo target directory;
4. verifies the checkout revision and applies the compatibility patch once;
5. attests the patched loader and official prebuilt BPF object blobs, rejects
   tracked, untracked, ignored, or dirty-submodule inputs, and forbids
   `ACTPLANE_REBUILD_BPF`;
6. builds only `agentsight-enforcer` with the real ActPlane backend.

The source directory is cached for repeatable builds and protected by a build
lock. An incomplete cache, revision mismatch, source-attestation failure, or
patch mismatch fails with an actionable error instead of silently falling back
to the mock backend.

## Verified Closure

The isolated end-to-end validation used AgentSight on `127.0.0.1:17400`, a
dedicated UDS, and a dedicated bpffs root, leaving the existing service on port
7396 untouched. It verified the following sequence:

1. apply a policy binding for a live Python process and its `/proc` start time;
2. receive an `enforced` acknowledgement from ActPlane;
3. attempt to read the protected file and receive `EPERM`;
4. query a normalized violation containing the binding, session, rule, target,
   block result, and ActPlane revision;
5. detach the binding and receive HTTP 204;
6. reject invalid policy DSL and stale process identity with client-actionable
   HTTP responses.

## Upstream Exit Criteria

Submit the compatibility behavior and its regression tests to ActPlane.
After an upstream release provides a selectable minimal hook profile and
idempotent missing-map deletion, update the pinned revision, run the complete
upgrade matrix, and delete the local patch and Cargo source override.
