# Reversible Compression (Stash)

Tokenless compression is *inline lossy, end-to-end lossless*: when a compressor
truncates content, the dropped payload is stashed under a BLAKE3-derived key and
a `<<tokenless:KEY>>` marker is embedded in the compressed output. The LLM can
quote the marker back to retrieve the original payload on demand, so no
information is permanently lost even though the inline representation is
smaller.

This mirrors Headroom's CCR (Compress-Cache-Retrieve); the mechanism here is
called **stash** to avoid the proprietary abbreviation.

## How it works

1. **Compress**: `ResponseCompressor` truncates oversized arrays (default:
   keep the first 32 items). The dropped tail is serialized to JSON and
   `stash.stash(payload)` stores it, returning a 24-hex BLAKE3 key plus a
   store-wide, monotonically increasing ownership token used if the write must
   later be rolled back. Tokens are never reused after expiry, deletion, or
   eviction.
2. **Mark**: the truncation marker becomes
   `<... N items truncated, retrieve with <<tokenless:KEY>>`.
3. **Retrieve**: the LLM emits the marker (or the bare key); the agent calls
   `tokenless retrieve <KEY>` (or the future MCP `tokenless_retrieve` tool)
   to fetch the original payload from the stash.

When no stash store is attached (`Option<Arc<dyn StashStore>>` = `None`),
truncation is lossy and non-retrievable — the original pre-stash behavior.
This keeps the stash off the core compression path unless a caller explicitly
enables it.

## No-savings rollback

If compressed output is discarded (CLI no-savings fallback), call
`rollback_stash_writes()` so markers that never reached the LLM do not leave
orphan stash rows. Pending rollback is a `HashMap<key, generation>`: a key
created in this session is recorded, and a later in-session refresh of that
same payload updates the generation **only when the store reports an unbroken
ownership chain** (`previous_generation` equals the token this session last
recorded). A refresh of a key this session never created stays off the list.

That chain check is required because content-addressed keys are shared across
processes. If compressor A creates P, compressor B refreshes P and emits a
marker, then A stashes P again, re-adopting B's generation would make A's
no-savings rollback delete the row B's marker still needs. A mismatch drops
the key from the pending list instead; rollback of the stale create-time
token is a CAS no-op.

`stash_writes` counts unique keys created this session, plus refreshes of
keys this session did not create. An in-session refresh does not increment
again, so after a successful rollback the counter matches remaining live
rows from that session.

Session scope differs by compressor:

- `ResponseCompressor` resets pending keys at the start of each `compress()`.
- `SchemaCompressor` accumulates across `compress()` calls until rollback or
  `clear_stash_session()`. That matches `compress-schema --batch` (compress
  every item, then one all-or-nothing rollback). Call rollback only after
  every emit/discard decision for the session. Programmatic callers that
  emit some results and later discard others on the same instance must call
  `clear_stash_session()` after keeping output; otherwise a later rollback
  deletes those emitted markers.

## Marker format

```
<<tokenless:HASH>>
```

- `HASH` is the first 24 hex characters (12 bytes / 96 bits) of a BLAKE3 hash
  of the stashed payload. 96 bits makes a collision astronomically unlikely
  (2⁴⁸ birthday bound), so a key is treated as a unique handle.
- The `tokenless:` namespace distinguishes these markers from Headroom's
  `<<ccr:HASH>>` and from any user content.
- `tokenless_ccr::parse_marker` accepts a string that is exactly a marker;
  `tokenless_ccr::extract_hash` scans arbitrary text (e.g. a whole truncation
  line) and returns the first embedded hash. Both reject malformed input
  (wrong length, non-hex) by returning `None` rather than panicking, so
  callers can pass untrusted LLM output directly.

## Backends

| Backend | Feature | Persistence | Use when |
|---|---|---|---|
| `InMemoryStore` | default | process memory | tests, single-process CLI runs |
| `SqliteStore` | `sqlite` (on by default) | SQLite file (WAL) | **production hook path** |

The tokenless hooks fork+exec a fresh process per call, so an in-memory store
loses its contents between calls. `SqliteStore` is therefore the recommended
production backend: it persists to `~/.tokenless/stash.db` so a `retrieve` in
one process can read what a `compress` in another process wrote.

Both backends enforce:

- **TTL**: entries expire after a fixed lifetime (InMemory 5 min; SQLite 1 h).
  An hour comfortably covers a typical agent session's compress→retrieve
  round trip. Expiry is enforced **on read** — `retrieve()` filters out
  expired rows (SQLite `WHERE expires_at >= now`) and `len()` counts only
  live entries, so expired data is never returned. The rows themselves
  remain on disk until either capacity-based FIFO eviction (triggered by
  `stash()`) or an explicit `evict_expired()` call (available for bulk
  cleanup but not called automatically), so the SQLite file can grow
  beyond the capacity before a `stash()` triggers a trim.
- **Capacity** (FIFO): once the live entry count exceeds the limit (InMemory
  1000; SQLite 10 000), the oldest entries are evicted. This prevents
  unbounded growth from runaway compression.

SQLite allocates ownership tokens and performs the live-row check, `created`
decision, upsert, and capacity enforcement in one `BEGIN IMMEDIATE`
transaction. A singleton `stash_metadata` row persists the generation
high-water mark across row deletion, expiry, lazy purge, and eviction; opening
older databases migrates the generation column and repairs that high-water
mark from the existing rows. InMemory keeps the equivalent high-water counter
under its store lock. Both backends fail without changing stash state when the
signed SQLite generation limit is exhausted.

## CLI

```bash
# Compress with stash on by default — dropped array items become retrievable.
echo '[1,2,...,200]' | tokenless compress-response --truncate-arrays-at 5
# -> [1,2,3,4,5,"<... 195 items truncated, retrieve with <<tokenless:c30c…>>"]

# Retrieve the original dropped items (same stash db, separate process).
tokenless retrieve c30ccf5ed1125e0ed871ba8e
# -> [6,7,8,…,200]

# Pass the whole truncation line; the hash is extracted automatically.
tokenless retrieve "<... 195 items truncated, retrieve with <<tokenless:c30c…>>"

# Opt out of stash (lossy truncation, the pre-stash behavior).
echo '[...]' | tokenless compress-response --no-stash

# Override the stash db path under the home or selected data directory.
tokenless retrieve <hash> --stash-db ~/.tokenless/alt-stash.db
```

`TOKENLESS_DATA_DIR` relocates both SQLite databases, producing
`$TOKENLESS_DATA_DIR/stash.db` and `$TOKENLESS_DATA_DIR/stats.db`.
`TOKENLESS_STASH_DB` mirrors `TOKENLESS_STATS_DB` as a higher-priority
single-file override.

## Security model

`TOKENLESS_DATA_DIR` is an explicit directory-level trust decision and may
point outside the real home, including to a managed service directory. It must
be absolute, cannot be filesystem root or contain parent traversal, and its
nearest existing ancestor is canonicalized before use. An invalid explicit
directory disables SQLite state for that operation instead of silently moving
it back under home.

File-level overrides (`--stash-db`, `TOKENLESS_STASH_DB`, and
`TOKENLESS_STATS_DB`) remain confined to the canonical real home — derived
from `getpwuid_r(getuid())`, never `$HOME` — or the selected data directory.
Existing database files must be regular files rather than symlinks. The CLI
and bundled RTK writer use the same path policy.

`retrieve` queries are parameterized SQL; a malformed hash simply yields "no
payload" rather than an injection.

## Fail-open policy

- **Compress path**: if the stash cannot be opened (invalid data directory,
  directory cannot be created, db open fails) or `stash()` errors, compression
  proceeds without stash and the marker degrades to the plain
  `<... N more items truncated>` form. Compression never fails because of the
  stash.
- **Retrieve path**: retrieve is user-initiated, so failures surface as
  errors (exit 1) rather than being swallowed.

## What is not (yet) stashed

- **String truncation**: long string values are truncated with a `… (truncated)`
  marker but the tail is not stashed. The stash marker (~65 chars) against
  small per-field limits would be proportionally large overhead; the
  high-value case is array truncation, which is covered.
- **MCP `tokenless_retrieve`**: not yet implemented; retrieval is via the CLI
  today. MCP integration is tracked separately.

Schema description truncation **is** stashed when a store is attached (CLI
default): `SchemaCompressor::truncate_description` writes the verbatim
original and appends a `<<tokenless:KEY>>` marker. It stays lossy only when
stash is off or the stash write fails.

## Mapping to Headroom CCR

| Headroom | Tokenless | Notes |
|---|---|---|
| CCR Store | stash store (`StashStore` trait) | InMemory / SQLite(WAL) / Redis* |
| `<<ccr:HASH>>` | `<<tokenless:HASH>>` | 24-hex BLAKE3, same key length |
| `headroom_retrieve` (MCP) | `tokenless retrieve` (CLI) | MCP tool pending |
| DashMap `remove_if` TOCTOU fix | `BEGIN IMMEDIATE` ownership transaction | SQLite path |
| default TTL 5 min / cap 1000 | InMemory 5 min / 1000; SQLite 1 h / 10 000 | tuned for hook process model |

\* Redis backend is not yet implemented; it is tracked for the
multi-worker case (no `cfg`-gated scaffolding exists yet).
