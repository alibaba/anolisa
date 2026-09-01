# Policy fixtures

The fixture names identify three different contract layers:

- `template-*.json` is a complete daemon `PutPolicyParams` create object. The
  samples omit `policyId`, so the daemon generates a new UUID and revision 1;
  `policyName` is a non-unique user name. An update adds the returned
  `policyId` to the same complete desired object. These files can be passed directly to
  `asc-cli policy template put --file` and are exercised through the real
  UDS daemon by the CLI E2E test.
- `high-sensitivity-read.json`, `prevent-file-deletion.json` and
  `low-sensitivity-egress.json` contain only the product `PolicyTemplate`
  payload used as lowering inputs by policy-engine golden tests.
- `canonical-policy-*.json` contains the expected backend-independent Canonical
  Policy IR after daemon-owned lowering; the golden test compares all three
  outputs structurally against these files.
- `daemon/policy-methods.json` is the registered daemon method inventory and
  request-shape fixture.

After starting the daemon, run one complete input example with:

```bash
target/debug/asc-cli \
  --socket "$AGENTSEC_V2_RUN_DIR/daemon.sock" \
  --token-file "$AGENTSEC_V2_RUN_DIR/policy-admin.token" \
  policy template put --file fixtures/template-prevent-file-deletion.json
```

The current production composition stops at `UnavailablePolicyAdapter`.
Putting these templates validates, lowers and stores them, but does not yet
make a policy effective at a PEP.
