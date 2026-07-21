# Security Audit Notify Path and Case Deduplication

## Goal

Complete the real kernel event path for `observe` and `audit` policies while grouping one
credential-exfiltration attempt into one risk case when a client retries several resolved IP
addresses.

## Constraints

- Keep AgentSight self-contained; AgentSecCore is not a runtime dependency.
- Keep the pinned ActPlane revision and reviewed patch queue.
- Preserve Linux 5.8+ compatibility and verify the BPF change on a real kernel.
- Avoid the `trace_openat_exit` program because its combined stack exceeds 512 bytes on the
  current kernel.
- Do not change the enforcement protocol or the existing Dashboard routes.

## Design

### Kernel event paths

The credential-exfiltration pinned profile uses two complementary paths:

- LSM `file_open` records credential-source reads and propagates taint.
- LSM `socket_connect` evaluates and denies `block` rules.
- `sys_enter_connect` evaluates `notify` rules without denying the operation.

ActPlane's LSM programs run in block mode and intentionally do not emit notify effects. The
syscall tracepoint runs in notify mode and therefore supplies the missing `observe` and `audit`
events. The profile continues suppressing all other dataflow tracepoints, especially file-open
exit programs, so the verifier stack limit remains respected. In enforce mode, the tracepoint
does not emit a duplicate because block effects are unsupported by its notify-only mode.

### Risk-case grouping

One case represents one short exfiltration burst, not one destination IP. Its correlation key is
derived from the binding, policy revision, source resource, source and sink process identities,
and a five-second occurrence bucket. Destination addresses are deliberately excluded. This
groups DNS fallback attempts while allowing later independent reads to form new cases.

Every immutable event remains stored. When an existing case is updated, new evidence links are
appended after the existing links, preserving each source-taint-sink-decision chain in order.
`observe` continues to store events without opening a case; `audit` opens an unblocked case;
`enforce` opens a blocked critical case.

## Failure handling

- A decision without source or sink evidence remains a typed ingestion error.
- Replayed event IDs remain idempotent.
- Loader attestation fails if the upstream input or reviewed patch result differs.
- A missing tracepoint or BPF verifier rejection prevents the enforcer from reporting healthy.

## Verification

- Integration tests prove DNS destination retries produce one case with both evidence chains.
- ActPlane profile tests prove only `trace_connect` is added to the lightweight profile.
- A real Linux test verifies `audit` allows the connection and records a case.
- A real Linux test verifies `enforce` blocks the same operation and records a critical case.

