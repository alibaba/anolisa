/**
 * AgentSight backend API client
 *
 * When the frontend is served by agentsight itself (embedded mode), the API
 * is on the same host/port as the page — use window.location.origin.
 * Otherwise fall back to REACT_APP_API_BASE or localhost:7396 for local dev.
 */

const API_BASE: string = (() => {
  // Explicit override via env var (set at build time for non-embedded deployments)
  if (typeof process !== 'undefined' && (process.env as any).REACT_APP_API_BASE) {
    return (process.env as any).REACT_APP_API_BASE as string;
  }
  // In a real browser context, use the same origin so it works on any host/port
  if (typeof window !== 'undefined' && window.location && window.location.origin) {
    return window.location.origin;
  }
  // Fallback for local dev (webpack-dev-server on :3000 → backend on :7396)
  return 'http://localhost:7396';
})();

// ─── Types mirroring backend response structs ────────────────────────────────

export interface SessionSummary {
  session_id: string;
  conversation_count: number;
  first_seen_ns: number;
  last_seen_ns: number;
  total_input_tokens: number;
  total_output_tokens: number;
  model: string | null;
  agent_name: string | null;
}

export interface TraceSummary {
  trace_id: string;
  conversation_id: string;
  call_count: number;
  total_input_tokens: number;
  total_output_tokens: number;
  start_ns: number;
  end_ns: number | null;
  model: string | null;
  /** First user_query recorded in this conversation (best-effort) */
  user_query: string | null;
}

export interface TraceEventDetail {
  id: number;
  call_id: string | null;
  start_timestamp_ns: number;
  end_timestamp_ns: number | null;
  model: string | null;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  /** Raw JSON string — parse before use */
  input_messages: string | null;
  /** Raw JSON string — parse before use */
  output_messages: string | null;
  /** Raw JSON string — parse before use */
  system_instructions: string | null;
  agent_name: string | null;
  process_name: string | null;
  pid: number | null;
  /** The user query that triggered this LLM call */
  user_query: string | null;
  /** Raw full event JSON — fallback when output_messages is null */
  event_json: string | null;
  /** Conversation ID (user query fingerprint) */
  conversation_id: string | null;
}

// ─── Internal helpers ────────────────────────────────────────────────────────

async function apiFetch<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(`API ${url} -> ${res.status}: ${text}`);
  }
  return res.json() as Promise<T>;
}

// ─── Public API ──────────────────────────────────────────────────────────────

/**
 * List sessions within a nanosecond time range.
 * Defaults to the last 24 h when omitted.
 */
export async function fetchSessions(
  startNs?: number,
  endNs?: number
): Promise<SessionSummary[]> {
  const params = new URLSearchParams();
  if (startNs !== undefined) params.set('start_ns', String(startNs));
  if (endNs !== undefined) params.set('end_ns', String(endNs));
  const qs = params.toString() ? `?${params.toString()}` : '';
  return apiFetch<SessionSummary[]>(`${API_BASE}/api/sessions${qs}`);
}

/**
 * List all trace IDs within a session, with per-trace token stats.
 * Optional startNs/endNs are forwarded as query parameters for future
 * backend-side time-range filtering.
 */
export async function fetchTraces(
  sessionId: string,
  startNs?: number | null,
  endNs?: number | null,
): Promise<TraceSummary[]> {
  const params = new URLSearchParams();
  if (startNs != null) params.set('start_ns', String(startNs));
  if (endNs != null) params.set('end_ns', String(endNs));
  const qs = params.toString();
  const suffix = qs ? `?${qs}` : '';
  return apiFetch<TraceSummary[]>(
    `${API_BASE}/api/sessions/${encodeURIComponent(sessionId)}/traces${suffix}`
  );
}

/**
 * Fetch detailed LLM call events for a single trace.
 */
export async function fetchTraceDetail(traceId: string): Promise<TraceEventDetail[]> {
  return apiFetch<TraceEventDetail[]>(
    `${API_BASE}/api/traces/${encodeURIComponent(traceId)}`
  );
}

/**
 * Fetch detailed LLM call events for a conversation (user query).
 */
export async function fetchConversationDetail(conversationId: string): Promise<TraceEventDetail[]> {
  return apiFetch<TraceEventDetail[]>(
    `${API_BASE}/api/conversations/${encodeURIComponent(conversationId)}`
  );
}

// ─── Agent-name & time-series APIs ───────────────────────────────────────────

/**
 * Fetch distinct agent names observed within the given time range.
 */
export async function fetchAgentNames(
  startNs?: number,
  endNs?: number
): Promise<string[]> {
  const params = new URLSearchParams();
  if (startNs !== undefined) params.set('start_ns', String(startNs));
  if (endNs !== undefined) params.set('end_ns', String(endNs));
  const qs = params.toString() ? `?${params.toString()}` : '';
  return apiFetch<string[]>(`${API_BASE}/api/agent-names${qs}`);
}

export interface TimeseriesBucket {
  bucket_start_ns: number;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
}

export interface ModelTimeseriesBucket {
  bucket_start_ns: number;
  model: string;
  total_tokens: number;
}

export interface TimeseriesResponse {
  token_series: TimeseriesBucket[];
  model_series: ModelTimeseriesBucket[];
}

/**
 * Fetch time-bucketed token stats and per-model breakdowns.
 */
export async function fetchTimeseries(
  startNs: number,
  endNs: number,
  agentName?: string,
  buckets = 30
): Promise<TimeseriesResponse> {
  const params = new URLSearchParams({
    start_ns: String(startNs),
    end_ns: String(endNs),
    buckets: String(buckets),
  });
  if (agentName) params.set('agent_name', agentName);
  return apiFetch<TimeseriesResponse>(`${API_BASE}/api/timeseries?${params.toString()}`);
}

// ─── ATIF export APIs ────────────────────────────────────────────────────────

import type { AtifDocument, AgentHealthResponse } from '../types';

// ─── Token Savings types ─────────────────────────────────────────────────────

export interface DiffLine {
  type: 'add' | 'remove' | 'context';
  content: string;
}

export interface OptimizationItem {
  id: string;
  category: 'tool_output' | 'mcp_response';
  title: string;
  before_tokens: number;
  after_tokens: number;
  saved_tokens: number;
  before_summary: string;
  after_summary: string;
  diff_lines: DiffLine[];
}

export interface SessionSavings {
  session_id: string;
  agent_name: string;
  total_input_tokens: number;
  total_output_tokens: number;
  total_tokens: number;
  saved_tokens: number;
  savings_rate: number;
  tool_saved: number;
  mcp_saved: number;
  optimization_items: OptimizationItem[];
}

export interface SavingsSummary {
  total_input_tokens: number;
  total_output_tokens: number;
  total_tokens: number;
  total_saved_tokens: number;
  savings_rate: number;
  total_tool_saved: number;
  total_mcp_saved: number;
}

export interface TokenSavingsResponse {
  stats_available: boolean;
  summary: SavingsSummary;
  sessions: SessionSavings[];
}

/**
 * Fetch token savings data within a nanosecond time range.
 */
export async function fetchTokenSavings(
  startNs: number,
  endNs: number,
  agentName?: string,
): Promise<TokenSavingsResponse> {
  const params = new URLSearchParams({
    start_ns: String(startNs),
    end_ns: String(endNs),
  });
  if (agentName) params.set('agent_name', agentName);
  return apiFetch<TokenSavingsResponse>(`${API_BASE}/api/token-savings?${params.toString()}`);
}

/**
 * Export a single trace as an ATIF v1.6 trajectory document.
 */
export async function fetchAtifByTrace(traceId: string): Promise<AtifDocument> {
  return apiFetch<AtifDocument>(
    `${API_BASE}/api/export/atif/trace/${encodeURIComponent(traceId)}`
  );
}

/**
 * Export a full session (all traces) as an ATIF v1.6 trajectory document.
 */
export async function fetchAtifBySession(sessionId: string): Promise<AtifDocument> {
  return apiFetch<AtifDocument>(
    `${API_BASE}/api/export/atif/session/${encodeURIComponent(sessionId)}`
  );
}

/**
 * Export a conversation (all LLM calls for a user query) as an ATIF v1.6 trajectory document.
 */
export async function fetchAtifByConversation(conversationId: string): Promise<AtifDocument> {
  return apiFetch<AtifDocument>(
    `${API_BASE}/api/export/atif/conversation/${encodeURIComponent(conversationId)}`
  );
}

// ─── Interruption APIs ───────────────────────────────────────────────────────

export type InterruptionSeverity = 'critical' | 'high' | 'medium' | 'low';

export interface InterruptionRecord {
  interruption_id: string;
  session_id: string | null;
  trace_id: string | null;
  call_id: string | null;
  pid: number | null;
  agent_name: string | null;
  interruption_type: string;
  severity: InterruptionSeverity;
  occurred_at_ns: number;
  detail: string | null;
  resolved: boolean;
}

export interface InterruptionCountResponse {
  total: number;
  by_severity: {
    critical: number;
    high: number;
    medium: number;
    low: number;
  };
}

export interface InterruptionTypeStat {
  interruption_type: string;
  severity: string;
  count: number;
}

/**
 * Fetch per-type interruption stats within a time range.
 */
export async function fetchInterruptionStats(
  startNs: number,
  endNs: number
): Promise<InterruptionTypeStat[]> {
  const params = new URLSearchParams();
  params.set('start_ns', String(startNs));
  params.set('end_ns', String(endNs));
  return apiFetch<InterruptionTypeStat[]>(
    `${API_BASE}/api/interruptions/stats?${params.toString()}`
  );
}

/** Per-(severity, type) detail returned by session/trace-counts endpoints. */
export interface InterruptionTypeDetail {
  interruption_type: string;
  severity: string;
  count: number;
}

export interface SessionInterruptionCount {
  session_id: string;
  total: number;
  by_severity: {
    critical: number;
    high: number;
    medium: number;
    low: number;
  };
  types: InterruptionTypeDetail[];
}

export interface TraceInterruptionCount {
  trace_id: string;
  total: number;
  by_severity: {
    critical: number;
    high: number;
    medium: number;
    low: number;
  };
  types: InterruptionTypeDetail[];
}

/** Map English interruption_type keys to Chinese labels. */
export const INTERRUPTION_TYPE_CN: Record<string, string> = {
  llm_error: 'LLM 错误',
  sse_truncated: 'SSE 截断',
  context_overflow: '上下文溢出',
  agent_crash: 'Agent 崩溃',
  token_limit: 'Token 超限',
};

/**
 * Fetch all unresolved interruptions for a session.
 */
export async function fetchSessionInterruptions(sessionId: string): Promise<InterruptionRecord[]> {
  return apiFetch<InterruptionRecord[]>(
    `${API_BASE}/api/sessions/${encodeURIComponent(sessionId)}/interruptions`
  );
}

/**
 * Fetch all unresolved interruptions for a trace.
 */
export async function fetchTraceInterruptions(traceId: string): Promise<InterruptionRecord[]> {
  return apiFetch<InterruptionRecord[]>(
    `${API_BASE}/api/traces/${encodeURIComponent(traceId)}/interruptions`
  );
}

/**
 * Fetch interruption counts (total + by severity) for the last 24 h.
 */
export async function fetchInterruptionCount(
  startNs?: number,
  endNs?: number,
  agentName?: string
): Promise<InterruptionCountResponse> {
  const params = new URLSearchParams();
  if (startNs !== undefined) params.set('start_ns', String(startNs));
  if (endNs !== undefined) params.set('end_ns', String(endNs));
  if (agentName) params.set('agent_name', agentName);
  const qs = params.toString() ? `?${params.toString()}` : '';
  return apiFetch<InterruptionCountResponse>(`${API_BASE}/api/interruptions/count${qs}`);
}

/**
 * Mark an interruption event as resolved.
 */
export async function resolveInterruption(interruptionId: string): Promise<void> {
  const res = await fetch(
    `${API_BASE}/api/interruptions/${encodeURIComponent(interruptionId)}/resolve`,
    { method: 'POST' }
  );
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(`POST /api/interruptions/${interruptionId}/resolve -> ${res.status}: ${text}`);
  }
}

/**
 * Fetch unresolved interruption count + max severity per session_id.
 */
export async function fetchInterruptionSessionCounts(
  startNs: number,
  endNs: number
): Promise<SessionInterruptionCount[]> {
  const params = new URLSearchParams();
  params.set('start_ns', String(startNs));
  params.set('end_ns', String(endNs));
  return apiFetch<SessionInterruptionCount[]>(
    `${API_BASE}/api/interruptions/session-counts?${params.toString()}`
  );
}

/**
 * Fetch unresolved interruption count + max severity per trace_id.
 */
export async function fetchInterruptionTraceCounts(
  startNs: number,
  endNs: number
): Promise<TraceInterruptionCount[]> {
  const params = new URLSearchParams();
  params.set('start_ns', String(startNs));
  params.set('end_ns', String(endNs));
  return apiFetch<TraceInterruptionCount[]>(
    `${API_BASE}/api/interruptions/trace-counts?${params.toString()}`
  );
}

// ─── Agent health API ─────────────────────────────────────────────────────────

/**
 * Fetch the current health status of all discovered agent processes.
 */
export async function fetchAgentHealth(): Promise<AgentHealthResponse> {
  return apiFetch<AgentHealthResponse>(`${API_BASE}/api/agent-health`);
}

/**
 * Acknowledge and remove an offline agent by PID.
 */
export async function deleteAgentHealth(pid: number): Promise<void> {
  const res = await fetch(`${API_BASE}/api/agent-health/${pid}`, { method: 'DELETE' });
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(`DELETE /api/agent-health/${pid} -> ${res.status}: ${text}`);
  }
}

/**
 * Kill and re-launch a hung agent process.
 * Returns the new PID on success.
 */
export async function restartAgentHealth(pid: number): Promise<{ ok: boolean; new_pid: number; cmd: string[] }> {
  const res = await fetch(`${API_BASE}/api/agent-health/${pid}/restart`, { method: 'POST' });
  const body = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(`POST /api/agent-health/${pid}/restart -> ${res.status}: ${body.error ?? res.statusText}`);
  }
  return body;
}
