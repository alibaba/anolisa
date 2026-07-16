# SkillFS Notify v2 (S2)

Design record for the one-step switch of
`skill_ledger.skillfs_notify_change` from notify v1 to v2. S2 keeps the
existing daemon method and transport, but replaces the Skill identity with a
canonical path plus a complete Skill id.

## Scope

Notify v2 covers the request model, response validation, canonical/live root
separation, and all existing notification producers. It does not add runtime
negotiation, v1 compatibility, dual delivery, registration, multi-root
aggregation, or deletion-state semantics.

## Request

SkillFS sends one NDJSON daemon request per debounced Skill change:

```json
{
  "id": "skillfs-42",
  "method": "skill_ledger.skillfs_notify_change",
  "params": {
    "schemaVersion": 2,
    "canonicalSkillDir": "/canonical/skills/category/skill",
    "skillId": "category/skill",
    "eventKind": "write",
    "paths": ["SKILL.md", "scripts/run.sh"]
  },
  "trace_context": {},
  "timeout_ms": 5000
}
```

`schemaVersion` is the protocol version. The business payload has exactly four
fields: `canonicalSkillDir`, `skillId`, `eventKind`, and `paths`. Notify v2
never sends `skillDir`, `skillName`, `mountId`, `generation`, `resolverSocket`,
or `sourceId`.

There is no v1 fallback or runtime version negotiation. SkillFS and the daemon
must switch directly to v2.

## Response

SkillFS accepts a response only when the daemon envelope has `ok=true` and its
data contains both values below:

```json
{
  "ok": true,
  "data": {
    "schemaVersion": 2,
    "accepted": true
  }
}
```

A missing or different schema version is an invalid response. `ok=false` or
`accepted=false` is a rejection. Connection, timeout, invalid-response, and
rejection failures are diagnostic only: FUSE I/O succeeds independently and
the current activation mapping stays in place.

## Canonical Root and Live Root

Notify and activation use two explicit roots:

- **Canonical root** (`source_canon` in the CLI) is the user-visible root that
  callers and the ledger address. Notify derives
  `canonicalSkillDir = canonical_root.join(skillId)`.
- **Live root** (`daemon_root` in the CLI) is the physical source that remains
  reachable after an in-place FUSE over-mount. It is the configured backing
  root when present and otherwise the source root. Activation bootstrap,
  activation reload, and pending-install completeness checks continue to use
  this root.

The live/backing path, including any deployment-private or `PrivateTmp`
detail, never appears in the notify v2 payload. The daemon uses
`canonicalSkillDir` with the S1 `skill.resolveLiveSource` resolver when it needs
the live directory.

## Skill Identity and Paths

`skillId` is always the complete id relative to the canonical root:

- flat layout: `weather`;
- Hermes layout: `category/weather`.

`paths` are relative to `canonicalSkillDir`. SkillFS sorts and deduplicates
them. A rename within one Skill includes both the old and new relative paths.
When more than `MAX_NOTIFY_PATHS` unique paths accumulate, SkillFS sends an
empty array to request a whole-Skill rescan.

The same construction applies to ordinary FUSE mutations, startup reconcile,
quiet-timeout delivery, pending-install completion, staging publish/rename,
and Hermes nested Skill mutations. Startup reconcile and root-level publish
events may intentionally use an empty `paths` array.

## Daemon Integration

The sec-core consumer switches directly to v2 and must:

1. require notify `schemaVersion=2` and the four business fields;
2. preserve the complete `skillId`;
3. interpret every path relative to `canonicalSkillDir`;
4. resolve the live source through the S1 resolver before each saturating
   scan/resolve pass; and
5. return `schemaVersion=2` with `accepted=true` only after accepting the
   event.

## Deferred Work

S2 does not add `register`, `unregister`, `mountId`, `generation`, `sourceId`,
`resolverSocket`, tombstones, or multi-directory runtime aggregation. A future
SkillFS instance may route several canonical roots internally by longest
prefix match; the external notify and resolver protocols already carry the
canonical path needed for that routing and do not need another version.
