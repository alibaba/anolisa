# Changelog

## 0.8.1

### Fixes
- Replace `lock().unwrap()` with poison-safe `unwrap_or_else` for mutex recovery.
- Correct SSL library attribution from aws-lc/BoringSSL to OpenSSL 3.x.
- Add Claude process name to BoringSSL classification.
- Preserve user config on schema migration instead of overwriting.
- Don't auto-overwrite invalid JSON configs; record process pid not thread tid in ns pid helper.
- Downgrade high-frequency event logs from debug to trace to reduce noise.

### Tests
- Expand unit tests for handlers, interruption store, and token store.
- Add poison-recovery tests for mutex `unwrap_or_else` changes.

## 0.8.0

### Features
- Add dashboard token-based authentication with file-only auth config.
- Add LAN/public IP address display and Chinese output in dashboard CLI.
- Add ECS security group guide and metadata integration to dashboard.
- Add conversation grader API and dashboard controls.
- Add `COSH_SESSION_ID` export for per-run session correlation.
- Auto-upgrade stale configs via `schema_version`.

### Fixes
- Fix Codex SSL capture and SSE token extraction.
- Fix false interruption signals.
- Persist idle streams and tool results to avoid snapshot loss.
- Detect SSE stream errors explicitly.
- Restrict `/health`, `/metrics`, and server auth to localhost/file-only config.
- Remove `hf-hub` git fork from default build dependency.
- Fix IMDSv2 token fetching, probe deadlines, and ECS metadata deduplication.
- Fix RPM build to copy `agentsight.json` into source tarball.
- Address clippy `single_match`, nested if-let, and architecture boundary issues.

### Tests
- Add dashboard mock HTTP and unit tests for coverage gate.
- Add `build_output` and `public_address` tests.
- Mark probe tests as `#[ignore]` for CI ECS runners.

## 0.7.1

### Fixes
- Improve severity labels and agent sidebar UX.
- Show all verdicts in the summary command.
- Sync component.toml version with package version.

## 0.7.0

### Features
- Add Codex CLI adaptation with three-tier SSL probe attach (symbol table → byte pattern → offset table) and cross-chunk SSE continuation buffer.
- Add security observability dashboard and server proxy for agent threat visibility.
- Add memory optimization with bounded event buffers, feature flags (`features.*`) and configurable runtime limits (`runtime_limits.*`).
- Add `container_id` to `AgentsightLLMData` for container-level attribution.
- Derive `session_id` from process environment variables and request metadata instead of message content.
- Add `call_kind` classification (chat / completion / embedding / tool_use) to GenAI semantic events.
- Add `--exclude` filter to `agentsight audit` CLI for noise reduction, and show non-streaming LLM calls in audit output.
- Add unified `agentsight summary` command for one-shot status overview.
- Enhance token savings page with baseline comparison, strategy breakdown, line-level diff highlighting and optimization tips.
- Upload skill metrics via SLS Logtail exporter.
- Improve agent health UX: role badges (P1/P2), TTL-based cleanup, process-ancestry grouping, and Session ID help tooltip.
- Filter client processes from health API to reduce dashboard noise.
- Add anolisa component contract for RPM lifecycle integration.

### Fixes
- Fix sslsniff BPF verifier rejection on kernel 5.15 and add BPF load tests.
- Fix traced_processes BPF map leak causing uprobe attach failure after long runtime.
- Prevent duplicate uprobe `Link`s by retaining inodes in `traced_files` on detach.
- Decode compressed (zstd/brotli) SSE streams so Claude Code and similar agents are fully captured.
- Harden compressed SSE decode against partial chunk boundaries.
- Extract token usage from non-streaming and HTTP/2 responses.
- Fix namespace PID usage in udpdns and tcpsniff probes.
- Strip `/proc/{pid}/root` prefix for uprobe attach in containerized environments.
- Implement tiered SSL and tcpsniff ring buffer reservations to reduce dropped events.
- Clamp before mask in filewrite/udpdns BPF probes; cap stdout payload to `MAX-1`.
- Change cgroup gate to OR semantics and add `trace_cgroup` FFI interface.
- Tighten SSE truncation detection and write pending row for deferred GenAI calls.
- Respect dynamic sysom path in SLS exporter mode selection; replace removed `sysom_logtail_path` with `logtail_path` filter.
- Validate ring buffer size is power-of-two at startup.
- Wire feature flags and runtime limits to actual runtime code paths.

### Refactoring
- Split `genai/builder.rs` into 4 focused modules and `genai.rs` into 5 submodules.
- Bundle shared BPF maps into `SharedMaps` for reduced duplication.
- Extract background threads module with stop-signal support.
- Replace remaining `unwrap()` calls with `if-let` / `?` patterns.

### CI & Quality
- Add fmt, clippy, unit test coverage, and architecture boundary check CI gates.
- Add `clippy.toml` + `cargo-deny` for lint and supply-chain auditing.
- Add architecture boundary check script (`check-arch-boundary.py`).
- Add scoped AGENTS.md for FFI, unified orchestrator, and storage modules.
- Define Footprint Ladder for code surface growth control.
- Add `agentsight-code-review` and `pr-body` develop-skills.

## 0.6.1

- Add real-time agent_crash detection in trace mode.
- Add OOM crash detection.
- Add cgroup-level event filtering with v1/v2 compatibility.
- Support QwenCode skill discovery via per-user home scanning.
- Support SLS Logtail activation reversible via dynamic path.
- Support bridging ilogtail `SLS_LOG_PATH` into config via token-collector switch.
- Default `traceEnabled` to false to drop conversation content from SLS by default.
- Drop `gen_ai.system_instructions` from SLS uploads when `traceEnabled=false`.
- Refactor session_id and conversation_id derivation from response_id instead of message content.
- Fix CJK deadloop detection, `kill()` error check, and SIGKILL escalation.
- Fix SQLite read/write contention via VACUUM optimization.
- Fix rpm-build.sh agentsight build failures.
- Fix allow log path re-init on repeated new+start.

## 0.6.0

- Add deadloop detection and auto-kill mechanism for runaway agent processes.
- Add retry storm detection and `/metrics` interruption counters.
- Add BPF-layer HTTP protocol filter and wildcard capture (`*`) for unknown IP/port targets.
- Add client-side hybrid encryption for sensitive message fields.
- Add `traceEnabled` configuration toggle with SLS upload layer enforcement.
- Add HTTP domain rules resolved to tcpsniff BPF map via DNS.
- Add default DashScope HTTPS rule and `anolisa_release` module.
- Add FFI interface for `tcp_targets` and `input_delta` config.
- Add CO-RE compatibility to UDP DNS probe for kernel 6.0+.
- Support runtime SLS logtail path via config hot-reload.
- Expand interruption types and add logtail export.
- Restructure config to `https`/`http` rules.
- Refactor query `stats.db` by `tool_use_id` and unify savings display.
- Refactor load encryption public key from `agentsight.json`.
- Fix decode HPACK Huffman headers.
- Fix BoringSSL probe attachment, FFI event delivery, and chunked-body panic.
- Fix preserve initial SSE chunk in event-stream responses.
- Fix `c_char` / BPF comm portability (i8 vs u8).
- Remove dead code and deprecated APIs.

## 0.5.0

- Add Claude Code support including SSL probe attach for BoringSSL, Anthropic SSE thinking/tool_use content blocks, and `message.id`-based session correlation.
- Add tcpsniff probe for plain HTTP traffic capture with configurable IP/port filtering (disabled by default with empty `tcp_targets`).
- Add User-Agent based agent detection with `comm` fallback for simplified agent matching.
- Add UDP DNS probe for agent discovery (replacing TLS SNI probe) with QNAME parsing moved to userspace.
- Add TLS SNI probe module and refactor discovery to config-driven rules.
- Add connection scanner for pre-established LLM API connections.
- Add `tools` field to `AgentsightLLMData` FFI struct, passed through as raw JSON.
- Add container PID namespace support in BPF traced process filtering and event emission.
- Add agent matching rules and reduce BPF ring buffer to 32MB.
- Add `uid` field to SLS logs with `OnceLock` cache and startup validation.
- Support profile-based installs.
- Fix `duration_ns` calculation in LLM data.
- Fix SSL probe cleanup of stale inodes on process exit.
- Fix BPF verifier `-E2BIG` issues by removing nested `#pragma unroll` in `udpdns.bpf.c` and masking `payload_len` on older kernels.
- Fix skill extraction for Hermes agent architecture.
- Fix Node.js `process.title` change handling in OpenClaw matcher.

## 0.4.0

- Add HTTP/1.1 request body reassembly for fragmented SSL writes.
- Add skill metrics analysis with cosh filesystem-based discovery.
- Add SLS upload and Logtail file exporter for GenAI events.
- Add hermes agent matcher for LLM process discovery.
- Detect uv Python static OpenSSL in SSL sniffer.
- Remove AK/SK-based SLS direct upload, keep Logtail file export.

## 0.3.1

- Fix simplify agent_crash detection and fix multi-process dedup. (#411)
- Fix use SqliteConfig for audit CLI db path. (#399)
- Fix hide Cosh from agent health UI and remove keepalive support. (#401)
- Fix API endpoint table in AGENTS.md. (#397)

## 0.3.0

- Add interruption detection system with drain mechanism and dashboard integration. (#315)
- Add token savings page and API endpoint for optimization visualization. (#310)
- Add compounded token savings and request count tracking. (#320)
- Add C FFI API with cbindgen header generation. (#306)
- Add filewatch and filewrite eBPF probes for file access monitoring. (#308, #309)
- Support SysOM AK/SK GenAI capture for cosh. (#305)
- Use LLM API response_id as trace_id and add conversation_id field. (#304)
- Resolve session_id from agent's own session via ResponseSessionMapper. (#303)
- Fix interruption CLI and align conversation_id naming. (#318)
- Fix cosh session_id recognition by supporting snake_case response_id. (#307)
- Fix wrong tool call id in token savings compounding. (#316, #317)
- Fix standardize call_id, add tool_call_ids column. (#319)
- Fix session_id and response_id mapping in genai builder and storage. (#321)
- Fix token savings display in conversation list. (#322)
- Fix cache agent name by pid for dead process resolution. (#358)
- Fix remove custom db path and use default paths. (#359)
- Support nightly docker image build in CI. (#302)

## 0.2.2

- Support starting backend-server for dashboard with AgentSight service.
- Fix dashboard frontend dynamic width for multiple display-size.

## 0.2.1

- Add `/usr/lib/copilot-shell` path to CoshMatcher for agent discovery. (#190)
- Add 200MB size limit for `genai_events.db` to prevent unbounded growth. (#211)
- Remove `/api/stats` endpoint returning incorrect data. (#197)
- Extract audit from HttpRecord and filter non-LLM calls. (#196)
- Always show comparison data when `--compare` flag is used in token queries. (#194)
- Fix incorrect `discover` command in README documentation. (#191)
- Remove breakdown command and keep token consumption commented. (#193)
- Replace deprecated `MemoryLimit` with `MemoryMax` in systemd service file. (#181)

## 0.2.0

- AgentSight Dashboard web UI with real-time monitoring interface. (#74)
- Agent health monitoring with offline alerting and hung process dashboard restart. (#158)
- One-click navigation from dashboard to ATIF trace analysis page. (#116)
- /metrics endpoint to expose standard Prometheus-format data. (#134)
- Support for HTTP 2.0 protocol. (#147)
- Support to build RPM package. (#166)
