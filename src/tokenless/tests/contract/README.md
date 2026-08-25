# Adapter contract suite

Shared corpus and harness verifying the adapter contract of roadmap §5.6
for every hook that speaks the protocol-v1 `tokenless compress` entry
point. Two suites consume it:

- `tests/test_hook_contract.py` — behavior-class contract against the mock
  protocol binary (`mock_tokenless.py`). Tests the adapters' envelope
  translation and the one-subprocess gate, not compression itself.
- `tests/test_hook_parity.py` — golden parity against the real debug
  binary (`make test-hook-parity`). The goldens under `goldens/` were
  captured from the pre-PR-6 two-subprocess hooks; regenerate them with
  `generate_goldens.py` only when re-baselining an intentional behavior
  change.

## Behavior classes (§5.6)

Every migrated adapter must handle all five classes, plus the
binary-missing case, and start at most one Tokenless subprocess:

| class            | mock behavior                          | expected hook outcome                 |
|------------------|----------------------------------------|---------------------------------------|
| passthrough      | `passthrough` disposition              | `{}` (schema hook: wrap the original) |
| replacement      | `applied` disposition                  | host replacement envelope             |
| no-savings       | `no_savings` disposition               | `{}` (schema hook: wrap the original) |
| timeout          | mock sleeps past the hook timeout      | subprocess killed, `{}`               |
| malformed input  | garbage hook stdin / garbage mock stdout / non-zero exit / `error` disposition | `{}` |

The mock validates the request it receives (protocol version, seam,
capabilities), so a hook that builds a malformed request fails these tests
through the resulting fail-open envelope.

## Adding an adapter

1. Add the agent's env declaration to `RESPONSE_AGENTS` / `SCHEMA_AGENTS`
   in `corpus.py` (and `FIXTURE_AGENTS` when a fixture models one host's
   private wire shape).
2. Extend the agent matrices in `tests/test_hook_contract.py` with the
   host's replacement-envelope expectation.
3. Regenerate nothing: goldens only change when behavior is intentionally
   re-baselined.

`PARITY_ALLOWLIST` in `corpus.py` enumerates the sanctioned envelope
differences from the pre-unified-entry hooks (currently: additive hosts
pass through instead of receiving duplicated compressed copies).
