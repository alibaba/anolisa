# Runtime Security Boundary

[中文版](runtime-security_zh.md)

Related architecture: [COSH Gateway and ACP Architecture](README.md)

## Threat model

Agent Runtime code and its descendants are untrusted. They may be buggy,
compromised, or intentionally adversarial. A Runtime can produce protocol
frames and request capabilities, but it is not an operator and does not inherit
Gateway authority.

The security boundary must hold even when Runtime and operator software are
installed by the same user. Filesystem permissions alone are insufficient when
both processes run as the same kernel principal.

## Required isolation

A production Runtime must not be able to:

- connect to the Gateway command socket as an approving actor;
- read or modify Gateway SQLite, WAL, SHM, backup, or audit files;
- signal or debug the Gateway process;
- change Gateway configuration, executable, workspace binding, or unit state;
- escape the service lifecycle owner and leave effect-capable descendants;
- inherit ambient credentials or injection variables not required by profile.

This requires a kernel-enforced principal, sandbox, or service boundary. A
presentation-layer check or an in-process actor label is not sufficient.

## Process ownership

- Each child process has exactly one lifecycle owner.
- Runtime launch creates a dedicated process group or stronger containment.
- Normal shutdown propagates cancellation, waits for protocol grace, then
  escalates TERM/KILL and reaps exactly once.
- Daemon hard failure is owned by an external service manager or containment
  boundary that kills every descendant before restart becomes ready.
- Runtime cannot create a sibling service or cgroup outside that ownership.

Linux packaging must verify effective service-manager properties rather than
trusting only a unit template. Unsupported platforms fail production admission
or use an independently reviewed owner.

## Executable and workspace identity

Production admission pins executable and workspace authority before accepting
Tasks:

- absolute configured path;
- descriptor-backed device/inode identity;
- required file type and executable/directory mode;
- trusted installation provenance and profile identity;
- workspace identity shared with governed execution targets.

Launch uses the pinned descriptor or fails closed. A path rename, symlink
retarget, or same-name replacement must never cause a queued Task to execute a
different artifact or workspace.

Descriptor pinning does not attest an entire interpreter or package dependency
tree. Script adapters additionally require a trusted interpreter and immutable
or verified package closure.

## Environment

Runtime launch starts from a cleared environment and explicitly allows only
required values such as locale, selected proxy settings, and approved
authentication entry points. It rejects dynamic-loader, Node injection,
shell-function, and arbitrary inherited configuration variables.

Credentials are scoped to the Runtime profile and are not written to Task,
event, audit, test transcript, or PR evidence.

## Local endpoint admission

Gateway authenticates local clients from kernel-provided peer identity and an
installation-scoped policy. It does not trust a caller-supplied Actor ID.

Production admission validates configured Runtime profile, target, workspace,
containment proof, and service identity before binding a public command socket.
Test or interoperability flags cannot silently enable the durable production
scheduler.

## Profile admission and platform portability

Capability dependencies are mandatory only for the selected profile. A
`task-only-v1` daemon does not open or validate a checkpoint socket and must not
advertise a checkpoint tool. A `ws-ckpt-v1` daemon requires Linux plus the
reviewed service lifecycle, peer identity, socket, workspace binding, and
filesystem support of its `ws-ckpt` provider.

If a host cannot satisfy those requirements, `ws-ckpt-v1` fails admission
before the command socket is published. The operator may explicitly select a
smaller supported profile, but Gateway never downgrades an already selected
profile. The admitted profile identity and operation inventory remain fixed
for the Run and are checked against the Runtime handshake.

Adding another checkpoint implementation does not inherit `ws-ckpt` trust.
Its executable or service identity, data durability, audit barrier,
reconciliation, and crash-containment evidence are reviewed independently.

## Filesystem authority

Security-sensitive files are opened relative to a trusted directory descriptor
with owner, mode, type, and identity checks held across open. Validation followed
by a new pathname lookup is vulnerable to replacement races.

The same rule applies to database, WAL/SHM companions, backup destination,
audit files, adapter artifacts, and governed Unix sockets.

## Audit

Audit is append-only, bounded, redacted, and durably framed. A partial write or
sync failure poisons the writer until explicit recovery; a later record cannot
be appended to a corrupt tail and treated as durable evidence.

## Acceptance invariants

- An adversarial Runtime cannot approve its own request.
- Runtime cannot read Gateway durable state or audit evidence.
- Replacing an executable or workspace path after admission cannot change what
  launches.
- SIGKILL of Gateway cannot leave an effect-capable descendant outside the
  lifecycle owner.
- Environment injection and service-manager escape attempts fail closed.
- Security evidence comes from effective runtime properties and adversarial
  fixtures, not configuration intent alone.
- Unsupported platform dependencies reject only the profile that requires
  them; they never leave an advertised operation without an admitted target.
