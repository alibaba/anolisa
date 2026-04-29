# Changelog

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
