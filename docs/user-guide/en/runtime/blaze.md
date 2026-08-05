# Blaze Firecracker Networking

[中文版](../../zh/runtime/blaze.md)

Blaze can give each Firecracker sandbox a dedicated network namespace, tap
device, veth pair, and address slot. This capability is opt-in and is disabled
by default.

## Prerequisites

The Blaze daemon must run on Linux with permission to manage host networking.
The `ip`, `sysctl`, and `iptables` commands must be installed and executable.
Firecracker and its kernel and root filesystem images must also be available.

Blaze checks these prerequisites when a loaded policy both enables networking
and selects Firecracker as an eligible backend. Policies that leave networking
disabled do not require these host capabilities.

## Configuration

Set `enable_network` in the Firecracker section of a workload policy:

```toml
[select]
backend_priority = ["firecracker"]

[backend.firecracker]
enable_network = true
```

The option applies only to Firecracker. Its default is `false`, so existing
policies retain their previous behavior until they opt in.

## Runtime Behavior

When a request selects a network-enabled Firecracker policy, sandbox creation:

1. allocates a host-wide network slot;
2. creates an owner-qualified network namespace;
3. creates the tap and veth devices and configures addresses, forwarding, and
   namespace-local NAT; and
4. starts Firecracker with the tap device attached.

Allocation and deletion use `/run/lock/blaze-network.lock`, which prevents two
Blaze daemon processes on the same host from choosing the same slot at the same
time. Blaze records the namespace owner before creating dependent devices so a
partially completed setup remains attributable to the sandbox.

Explicit sandbox destruction removes the owned namespace and devices after the
backend process has stopped. A compensated startup failure performs the same
cleanup. If cleanup cannot be confirmed, Blaze retains ownership and does not
return the slot to the allocator, allowing a later destroy attempt to retry the
operation.

After a daemon restart, a later destroy request can reconstruct a recorded
network slot. Blaze does not run a background scan or retry controller for
orphaned network resources.

## Host Integration Boundary

Blaze configures the sandbox-local network path. Routing beyond the host and DNS
configuration remain the host operator's responsibility. Before enabling the
option in production, configure the required upstream routing or translation
and verify guest connectivity for the host environment.

To disable the capability, set `enable_network = false` or remove the key, then
destroy existing network-enabled sandboxes through the normal instance API.
