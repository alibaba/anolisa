# AgentSight Protocol v4 Tool Context Design

## Goal

Carry producer-owned conversation and tool-call identifiers from an AgentSight enforcement binding into every normalized system security event. This enables AgentLoop to require exact `gen_ai.tool.call.id` equality between application and system evidence without asking LoongCollector to infer identifiers.

## Architecture

The Agent adapter or validation harness obtains the real tool-call identifier before the protected action runs and submits it as optional `conversation_id` and `tool_call_id` fields on the existing file or credential binding request. AgentSight validates and stores those values in `ApplyCredentialPolicy` and `ApplyPolicy`. The enforcer copies the stored values into `EventIdentity` for file, taint, network, policy-decision, and enforcement-state events. LoongCollector continues consuming the existing versioned JSON envelope through `agentsight_read_v2()` and already maps `identity.tool_call_id` to `gen_ai.tool.call.id`.

No new FFI entry point or collector-side state is introduced. The enforcement wire protocol remains version 4 because both fields are optional and use Serde defaults, preserving decoding of older frames.

## Validation Rules

- Trim optional identity values and convert empty strings to `None`.
- Reject a non-empty identity longer than 256 bytes.
- Preserve older protocol v4 frames that omit the new fields.
- Emit the exact producer-supplied values; never synthesize or derive them in the enforcer or LoongCollector.
- A strict AgentLoop incident requires both non-empty tool-call IDs to be equal. A missing or mismatched system ID must not receive the strict corroborated status.

## Deployment and Rollback

Build the protocol v4 AgentSight producer and ActPlane-backed enforcer from the same revision. Back up the two installed binaries, stop the services, replace both binaries atomically, and verify protocol health before running a real case. Roll back both binaries together if health or binding checks fail.

## Real Case

A real Hermes tool invocation starts a paused driver. Before releasing it, read the real application session, conversation, and tool-call ID from AgentSight's captured model event and bind the driver's PID with those exact values. The driver reads one byte from a randomly generated mock credential and opens a TCP connection without sending the byte. Verify that application and system events contain the same tool-call ID and that the incident is promoted. Run a second binding with a deliberately different tool-call ID and verify that strict promotion is rejected. Clean up bindings, processes, and mock files afterward.
