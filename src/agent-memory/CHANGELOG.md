# Changelog

## 0.2.1

- fix vector/hybrid search panic and empty index when an embedding provider is configured: the index worker ran on a std::thread with no tokio Handle so embeddings were never produced, and memory_search mode=vector|hybrid called Handle::block_on from a worker thread; the runtime handle is now captured at spawn and threaded through to the worker, and the search path uses block_in_place
- fix memory_get_context leaking .git internals (e.g. .git/logs/HEAD) into agent context by extending the reserved-path filter to cover .git/ via a shared is_under_git predicate in safe_fs
- fix full_scan (startup and inotify-overflow recovery) only building the BM25 index and never dense embeddings, so preexisting files were invisible to vector search until modified; a paths_without_vec query plus a backfill pass now embeds them, centralised in an embed_sync helper shared with flush
- fix memory_search returning zero hits for short CJK query terms (< 3 chars, e.g. "花名"/"小云"): the trigram tokenizer emits no tokens for terms shorter than 3 characters, so such queries now fall back to a `body LIKE '%term%'` substring scan that preserves recall, agent-scope filtering, and cold/superseded exclusion
- resolve embedding dimensions from the first real response instead of hardcoding 1536 (DashScope text-embedding-v3 is 1024): dimensionality is stored in an AtomicUsize seeded with the estimate and overwritten on first embed
- add anolisa-cli adapter contract via .anolisa/component.toml so the CLI adapter manager can discover the openclaw plugin bundle through the [[adapters]] TOML schema

## 0.2.0

- add prompt-injection safety module (looksLikePromptInjection + escapeMemoryForPrompt) mirrored between Rust core and TS adapter
- add secret detection and PII redaction to the safety module
- add auto-recall before_prompt_build hook injecting relevant memories each turn
- add auto-capture agent_end hook with trigger filtering, SHA256 dedup and injection rejection
- add dense-vector semantic search via pluggable EmbeddingProvider (OpenAI /v1/embeddings, Ollama /api/embed)
- add files_vec table (schema v2) for per-file dense embeddings alongside FTS5 BM25
- add hybrid search with reciprocal rank fusion (RRF, k=60) of BM25 + vector scores
- add memory_search mode parameter (bm25/vector/hybrid) with graceful fallback to BM25
- add per-agent memory isolation via [memory].agent_scope (shared/isolated/filter), schema v5
- add memory sovereignty tools (memory_about/forget/auto_created/consent) with consent.toml preferences
- add 4-type closed memory classification (user/feedback/project/reference) to memory_observe
- add mem_export and mem_import for cross-agent memory migration (AMA archive format)
- add memory_summary tool for memory overview and source tracking
- add memory_session_context tool
- add memory_sessions and memory_timeline session history query tools
- add MEMORY.md index file and mem_index_refresh tool
- add user profile synthesis (Dreaming V3 mem_dream)
- add memory consolidation: auto-extract L1 atomic facts from session audit logs on shutdown
- add episodic memory extraction from coherent tool-call chains
- add cross-session task persistence and incremental consolidation
- add consolidation quality filters (mutual exclusion, non-derivable, date normalization)
- add time-decay ranking (exp(-λ×age_days)) applied to BM25/vector/hybrid scores
- add cold archival of old never-accessed files with mem_compact tool
- add conflict detection via BM25 similarity before writing new facts
- add category subdirectories (facts/<category>/) with memory_search category filter
- add token tracking (tokens field in AuditEntry)
- add mem_consolidate tool for manual consolidation trigger
- add corpus supplement registration for memory_search corpus=all
- add EmbeddingConfig (None/OpenAI/Ollama) with TOML parsing and env overrides
- extend memory_search signature with optional mode and category parameters
- cap memory_search query at 1024 characters to prevent FTS5 resource exhaustion
- truncate embedding error response bodies to 200 chars to prevent API key leakage
- distinguish CJK vs ASCII token estimation in ConsolidatedFact
- hold FactWriter JSONL file handle under mutex to prevent line interleaving
- derive BM25Store mount root from db path with canonicalize + starts_with traversal guard
- compute Episode duration from entry timestamps instead of chain length
- propagate session_id to extracted episodic facts
- return fact count from consolidate() for mem_consolidate reporting
- fix effectiveMode in search response to reflect actual mode used
- fix embedding API empty-response handling to return zero vector of correct dimensionality

## 0.1.0

- introduce filesystem memory MCP server for AI agents (Linux only) with 21 tools over stdio JSON-RPC 2.0 in three tiers (Tier A file ops, Tier B BM25 search, Tier C governance)
- add per-namespace mount under ~/.anolisa/memory/<ns>/ with optional user-namespace + private tmpfs isolation (auto/userland/userns strategies)
- enforce path sandbox via openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS) on every Tier A file open
- add SQLite FTS5 BM25 background index with transactional upsert, schema migrations, trigram CJK tokenizer and inotify-driven debounced flush
- add optional git versioning with auto-commit serialized under a per-handle mutex
- add tar.gz snapshots with strict id whitelist, atomic rename swap on restore and rollback entries under .anolisa/trash/
- add optional cgroup v2 memory.max self-limit applied before the tokio runtime starts
- add JSONL audit log (O_NOFOLLOW | O_CLOEXEC, Mutex<File>) with optional systemd-journald fan-out
- enforce profile gating (basic/advanced/expert) at both tools/list and tools/call with deny_unknown_fields on config structs
- add per-session scratch and log under /run/anolisa/sessions/<sid>/ (0700) with tmpfiles.d snippet
- add systemd user template anolisa-memory@.service with hardening (ProtectKernelTunables/Modules/Logs, SystemCallFilter, MemoryDenyWriteExecute, RestrictNamespaces, RestrictAddressFamilies=AF_UNIX)
- add RPM packaging with offline vendor tarball and single statically-linked binary (bundled SQLite + vendored libgit2)
- add OpenClaw plugin memory-anolisa with install/detect/uninstall lifecycle and 4 memory contract tools routed to the MCP server as a stdio child
- add single-source version sync from Cargo.toml into manifest/package/openclaw/mcp JSON and the bundle
- add mcp-harness example and 140 automated tests across 12 integration suites
