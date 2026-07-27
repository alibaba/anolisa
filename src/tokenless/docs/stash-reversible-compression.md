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
   `stash.stash(payload)` stores it, returning a 24-hex BLAKE3 key.
2. **Mark**: the truncation marker becomes
   `<... N items truncated, retrieve with <<tokenless:KEY>>`.
3. **Retrieve**: the LLM emits the marker (or the bare key); the agent calls
   `tokenless retrieve <KEY>` (or the future MCP `tokenless_retrieve` tool)
   to fetch the original payload from the stash.

When no stash store is attached (`Option<Arc<dyn StashStore>>` = `None`),
truncation is lossy and non-retrievable — the original pre-stash behavior.
This keeps the stash off the core compression path unless a caller explicitly
enables it.

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

# Override the stash db path (must be under the trusted home directory).
tokenless retrieve <hash> --stash-db ~/.tokenless/alt-stash.db
```

`TOKENLESS_STASH_DB` mirrors `TOKENLESS_STATS_DB` as an env override.

## Security model

The stash db path is resolved under the **trusted home directory** — derived
from `getpwuid_r(getuid())`, never from `$HOME` (which a parent process can
spoof to redirect state into attacker-writable paths). An override
(`--stash-db` or `TOKENLESS_STASH_DB`) is validated by canonicalizing both the
home anchor and the candidate and requiring the candidate to live under the
home; a path outside home is rejected. This mirrors the stats DB trust model
exactly, so an attacker cannot redirect the stash to a system-critical
location.

`retrieve` queries are parameterized SQL; a malformed hash simply yields "no
payload" rather than an injection.

## Fail-open policy

- **Compress path**: if the stash cannot be opened (no trusted home, directory
  cannot be created, db open fails) or `stash()` errors, compression proceeds
  without stash and the marker degrades to the plain
  `<... N more items truncated>` form. Compression never fails because of the
  stash.
- **Retrieve path**: retrieve is user-initiated, so failures surface as
  errors (exit 1) rather than being swallowed.

## What is not (yet) stashed

- **String truncation**: long string values are truncated with a `… (truncated)`
  marker but the tail is not stashed. The stash marker (~65 chars) against
  small per-field limits would be proportionally large overhead; the
  high-value case is array truncation, which is covered.
- **Schema description truncation**: `SchemaCompressor::truncate_description`
  remains lossy for the same marker-overhead reason.
- **MCP `tokenless_retrieve`**: not yet implemented; retrieval is via the CLI
  today. MCP integration is tracked separately.

## Mapping to Headroom CCR

| Headroom | Tokenless | Notes |
|---|---|---|
| CCR Store | stash store (`StashStore` trait) | InMemory / SQLite(WAL) / Redis* |
| `<<ccr:HASH>>` | `<<tokenless:HASH>>` | 24-hex BLAKE3, same key length |
| `headroom_retrieve` (MCP) | `tokenless retrieve` (CLI) | MCP tool pending |
| DashMap `remove_if` TOCTOU fix | single-writer `Mutex<Connection>` | SQLite path |
| default TTL 5 min / cap 1000 | InMemory 5 min / 1000; SQLite 1 h / 10 000 | tuned for hook process model |

\* Redis backend is not yet implemented; it is tracked for the
multi-worker case (no `cfg`-gated scaffolding exists yet).
