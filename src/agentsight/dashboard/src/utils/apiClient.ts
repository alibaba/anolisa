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

class ApiRequestError extends Error {
  readonly status: number;
  readonly body: Record<string, unknown> | null;

  constructor(url: string, status: number, text: string, body: Record<string, unknown> | null) {
    super(`API ${url} -> ${status}: ${text}`);
    this.name = 'ApiRequestError';
    this.status = status;
    this.body = body;
  }
}

async function apiFetch<T>(url: string, init: RequestInit = {}): Promise<T> {
  const res = await fetch(url, { ...init, credentials: 'same-origin' });
  if (res.status === 401) {
    // Session expired or invalid — redirect to login
    window.location.hash = '#/login';
    throw new Error('Authentication required');
  }
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    let body: Record<string, unknown> | null = null;
    try {
      const parsed = JSON.parse(text);
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        body = parsed as Record<string, unknown>;
      }
    } catch {
      // Error responses may be plain text.
    }
    throw new ApiRequestError(url, res.status, text, body);
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

// ─── Grader APIs ─────────────────────────────────────────────────────────────

export type EvaluationVerdict = 'pass' | 'warn' | 'fail';
export type EvaluationRootCause =
  | 'none'
  | 'no_final_answer'
  | 'interrupted_main_call'
  | 'agent_crash'
  | 'runtime_error'
  | 'tool_failure'
  | 'safety_risk'
  | 'loop_detected'
  | 'excessive_cost'
  | 'partial_snapshot';

export interface EvaluationTarget {
  conversation_id: string;
  trace_id?: string | null;
  call_id?: string | null;
  step_id?: string | null;
}

export interface EvaluationRef {
  type: 'genai_event' | 'interruption' | 'security_event' | 'trace' | 'tool_call' | 'atif_step';
  id: string;
  label: string;
  severity?: string | null;
  target: EvaluationTarget;
  deeplink?: {
    route: string;
    query: Record<string, unknown>;
  } | null;
  metadata?: Record<string, unknown> | null;
}

export interface EvaluationDimension {
  name: string;
  score: number;
  verdict: EvaluationVerdict;
  reason: string;
  evidence_refs: EvaluationRef[];
}

export interface EvaluationFinding {
  code: string;
  severity: string;
  message: string;
  evidence_refs: EvaluationRef[];
}

export interface EvaluationMetadata {
  evaluated_with_pending: boolean;
  pending_call_count: number;
  input_event_count: number;
  grader_type: 'rule' | 'llm' | 'agent';
  grader_version: string;
  rubric_version: string | null;
  judge_model: string | null;
  prompt_hash: string | null;
  confidence: number | null;
}

export interface EvaluationResult {
  target_type: 'conversation';
  target_id: string;
  run_id: string;
  input_hash: string;
  verdict: EvaluationVerdict;
  score: number;
  summary: string;
  root_cause: EvaluationRootCause;
  recommended_action: string;
  dimensions: EvaluationDimension[];
  findings: EvaluationFinding[];
  metadata: EvaluationMetadata;
}

export interface EvaluationResponse {
  result: EvaluationResult;
  reused_existing_run: boolean;
}

export class EvaluationNotReadyError extends Error {
  readonly pendingCallCount: number;

  constructor(message: string, pendingCallCount: number) {
    super(message);
    this.name = 'EvaluationNotReadyError';
    this.pendingCallCount = pendingCallCount;
  }
}

/**
 * Fetch the latest persisted evaluation for a conversation.
 */
export async function fetchLatestEvaluation(conversationId: string): Promise<EvaluationResult | null> {
  const params = new URLSearchParams({
    target_type: 'conversation',
    target_id: conversationId,
  });
  return apiFetch<EvaluationResult | null>(`${API_BASE}/api/grader/latest?${params.toString()}`);
}

/**
 * Manually evaluate a conversation with the rule-based grader.
 */
export async function evaluateConversation(
  conversationId: string,
  force = false,
): Promise<EvaluationResponse> {
  try {
    return await apiFetch<EvaluationResponse>(`${API_BASE}/api/grader/evaluate`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        target_type: 'conversation',
        target_id: conversationId,
        force,
      }),
    });
  } catch (error) {
    if (
      error instanceof ApiRequestError &&
      error.status === 409 &&
      error.body?.error === 'conversation_not_ready'
    ) {
      const message = error.body.message;
      throw new EvaluationNotReadyError(
        typeof message === 'string' ? message : 'Conversation still has pending LLM calls.',
        Number(error.body.pending_call_count ?? 0),
      );
    }
    throw error;
  }
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
  type: 'add' | 'remove' | 'context' | 'separator';
  content: string;
}

export interface OptimizationItem {
  id: string;
  category: 'tool_output' | 'mcp_response';
  title: string;
  strategy: string;
  strategy_label: string;
  before_tokens: number;
  after_tokens: number;
  saved_tokens: number;
  compounded_saved: number;
  compounding_turns: number;
  compression_ratio: number;
  explanation: string;
  before_summary: string;
  after_summary: string;
  optimization_reason: string;
  before_text: string | null;
  after_text: string | null;
  diff_lines: DiffLine[];
}

export interface SessionSavings {
  session_id: string;
  agent_name: string;
  total_input_tokens: number;
  total_output_tokens: number;
  total_tokens: number;
  baseline_tokens: number;
  saved_tokens: number;
  compounded_saved: number;
  savings_rate: number;
  compounded_savings_rate: number;
  request_count: number;
  tool_saved: number;
  mcp_saved: number;
  optimization_items: OptimizationItem[];
}

export interface StrategyBreakdownItem {
  strategy: string;
  label: string;
  saved: number;
  compounded_saved: number;
}

export interface SavingsSummary {
  total_input_tokens: number;
  total_output_tokens: number;
  total_tokens: number;
  baseline_tokens: number;
  total_saved_tokens: number;
  total_compounded_saved: number;
  savings_rate: number;
  compounded_savings_rate: number;
  total_tool_saved: number;
  total_mcp_saved: number;
  total_compounded_tool_saved: number;
  total_compounded_mcp_saved: number;
  strategy_breakdown: StrategyBreakdownItem[];
}

export interface OptimizationTip {
  level: 'success' | 'info' | 'warning';
  title: string;
  description: string;
}

export interface TokenSavingsResponse {
  stats_available: boolean;
  summary: SavingsSummary;
  sessions: SessionSavings[];
  optimization_tips: OptimizationTip[];
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

// ─── Session-scoped Token Savings ─────────────────────────────────────────────

export interface SessionSavingsDetail {
  session_id: string;
  stats_available: boolean;
  total_actual_tokens: number;
  total_compounded_saved: number;
  total_original_tokens: number;
  savings_rate: number;
  items: OptimizationItem[];
}

/**
 * Fetch token savings detail for a single session.
 */
export async function fetchSessionSavings(
  sessionId: string,
): Promise<SessionSavingsDetail> {
  return apiFetch<SessionSavingsDetail>(
    `${API_BASE}/api/token-savings/session/${encodeURIComponent(sessionId)}`
  );
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
  conversation_id: string | null;
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

export interface ConversationInterruptionCount {
  conversation_id: string;
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
  rate_limit: '速率限制',
  auth_error: '鉴权错误',
  network_timeout: '网络超时',
  service_unavailable: '服务不可用',
  safety_filter: '安全过滤',
  retry_storm: '重试风暴',
  dead_loop: '死循环',
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
 * Fetch all unresolved interruptions for a conversation.
 */
export async function fetchConversationInterruptions(conversationId: string): Promise<InterruptionRecord[]> {
  return apiFetch<InterruptionRecord[]>(
    `${API_BASE}/api/conversations/${encodeURIComponent(conversationId)}/interruptions`
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
    { method: 'POST', credentials: 'same-origin' }
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
 * Fetch unresolved interruption count + max severity per conversation_id.
 */
export async function fetchInterruptionConversationCounts(
  startNs: number,
  endNs: number
): Promise<ConversationInterruptionCount[]> {
  const params = new URLSearchParams();
  params.set('start_ns', String(startNs));
  params.set('end_ns', String(endNs));
  return apiFetch<ConversationInterruptionCount[]>(
    `${API_BASE}/api/interruptions/conversation-counts?${params.toString()}`
  );
}

// ─── Agent health API ─────────────────────────────────────────────────────────

/**
 * Fetch the current health status of all discovered agent processes.
 */
export async function fetchAgentHealth(opts?: { includeClients?: boolean }): Promise<AgentHealthResponse> {
  const qs = opts?.includeClients ? '?include_clients=true' : '';
  return apiFetch<AgentHealthResponse>(`${API_BASE}/api/agent-health${qs}`);
}

/**
 * Acknowledge and remove an offline agent by PID.
 */
export async function deleteAgentHealth(pid: number): Promise<void> {
  const res = await fetch(`${API_BASE}/api/agent-health/${pid}`, { method: 'DELETE', credentials: 'same-origin' });
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
  const res = await fetch(`${API_BASE}/api/agent-health/${pid}/restart`, { method: 'POST', credentials: 'same-origin' });
  const body = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(`POST /api/agent-health/${pid}/restart -> ${res.status}: ${body.error ?? res.statusText}`);
  }
  return body;
}

// ─── Security Observability API ──────────────────────────────────────────────

export type SecurityApiState =
  | 'daemon_reachable'
  | 'disabled'
  | 'daemon_unreachable'
  | 'store_unavailable'
  | 'schema_mismatch'
  | 'ok'
  | 'empty'
  | 'partial'
  | 'found'
  | 'not_found'
  | 'redacted'
  | 'truncated'
  | 'no_correlation'
  | 'low_confidence'
  | 'bad_request'
  | 'payload_too_large'
  | 'timeout'
  | 'busy'
  | 'unavailable'
  | 'error'
  | string;

export interface SecurityApiResponse<T> {
  state: SecurityApiState;
  data: T;
  message?: string;
  meta?: Record<string, unknown>;
}

export interface SecurityRestError {
  code: string;
  message: string;
  retryable?: boolean;
  daemon_code?: string;
}

export class SecurityApiClientError extends Error {
  readonly status: number;
  readonly code: string;
  readonly retryable: boolean;
  readonly daemonCode?: string;

  constructor(status: number, error: SecurityRestError) {
    super(error.message);
    this.name = 'SecurityApiClientError';
    this.status = status;
    this.code = error.code;
    this.retryable = Boolean(error.retryable);
    this.daemonCode = error.daemon_code;
  }
}

export interface SecurityStoreStatus {
  path?: string;
  exists?: boolean;
  ready?: boolean;
  schema_version?: number | null;
  expected_schema_version?: number | null;
  [key: string]: unknown;
}

export interface SecurityStatusData {
  daemon?: Record<string, unknown>;
  stores?: {
    available?: boolean;
    security_db?: SecurityStoreStatus;
    observability_db?: SecurityStoreStatus;
    [key: string]: unknown;
  };
  socket_path?: string | null;
  [key: string]: unknown;
}

export interface SecurityEventRecord {
  event_id: string;
  event_type?: string | null;
  category?: string | null;
  result?: string | null;
  verdict?: string | null;
  command?: string | null;
  skill_name?: string | null;
  timestamp?: string | null;
  timestamp_ns?: number | null;
  timestamp_epoch?: number | null;
  trace_id?: string | null;
  session_id?: string | null;
  run_id?: string | null;
  call_id?: string | null;
  tool_call_id?: string | null;
  pid?: number | null;
  uid?: number | null;
  details?: unknown;
  details_preview?: unknown;
  truncated?: boolean;
  redacted?: boolean;
  [key: string]: unknown;
}

export interface SecuritySummary {
  total: number;
  by_category: Record<string, number>;
  by_event_type: Record<string, number>;
  by_result: Record<string, number>;
  affected_sessions: number;
  affected_runs: number;
  latest_events: SecurityEventRecord[];
  [key: string]: unknown;
}

export interface SecurityCountItem {
  value: string | number;
  count: number;
}

export interface SecurityCountByResponse {
  group_by: string;
  items: SecurityCountItem[];
  [key: string]: unknown;
}

export interface SecurityPaginated<T> {
  items: T[];
  total: number;
  limit: number;
  offset: number;
  next_offset?: number | null;
  [key: string]: unknown;
}

export interface SecurityEventDetailResponse {
  found: boolean;
  event: SecurityEventRecord | null;
  [key: string]: unknown;
}

export interface SecuritySessionSummary {
  session_id: string;
  first_seen_ns?: number | null;
  last_seen_ns?: number | null;
  first_seen_epoch?: number | null;
  last_seen_epoch?: number | null;
  turn_count?: number | null;
  observability_event_count?: number | null;
  security_event_count?: number | null;
  highest_severity?: string | null;
  [key: string]: unknown;
}

export interface SecurityRunSummary {
  run_id: string;
  started_at_ns?: number | null;
  ended_at_ns?: number | null;
  started_at_epoch?: number | null;
  ended_at_epoch?: number | null;
  user_input_preview?: string | null;
  observability_event_count?: number | null;
  security_event_count?: number | null;
  [key: string]: unknown;
}

export interface SecurityTimelineObservabilityContext {
  id?: string | number;
  hook?: string | null;
  timestamp?: string | null;
  timestamp_epoch?: number | null;
  session_id?: string | null;
  run_id?: string | null;
  call_id?: string | null;
  tool_call_id?: string | null;
  metadata?: Record<string, unknown>;
  metrics?: Record<string, unknown>;
  [key: string]: unknown;
}

export interface SecurityTimelineItem {
  kind: string;
  id?: string | number;
  hook?: string | null;
  timestamp?: string | null;
  timestamp_ns?: number | null;
  timestamp_epoch?: number | null;
  title?: string | null;
  summary?: string | null;
  session_id?: string | null;
  run_id?: string | null;
  call_id?: string | null;
  tool_call_id?: string | null;
  metadata?: Record<string, unknown>;
  metrics?: Record<string, unknown>;
  observability_event_id?: string | number | null;
  observability?: SecurityTimelineObservabilityContext;
  event?: SecurityEventRecord;
  match?: Record<string, unknown>;
  correlated_security_events?: unknown[];
  truncated?: boolean;
  redacted?: boolean;
  [key: string]: unknown;
}

export interface SecurityTimelineResponse {
  session_id: string;
  run_id: string;
  items: SecurityTimelineItem[];
  [key: string]: unknown;
}

export type SecurityQueryValue = string | number | boolean | null | undefined;

export interface SecurityTimeRangeParams {
  start_ns?: number;
  end_ns?: number;
}

export interface SecurityEventListParams extends SecurityTimeRangeParams {
  event_type?: string;
  category?: string;
  result?: string;
  verdict?: string;
  trace_id?: string;
  session_id?: string;
  run_id?: string;
  call_id?: string;
  tool_call_id?: string;
  limit?: number;
  offset?: number;
  include_details?: boolean;
}

export interface SecuritySessionListParams extends SecurityTimeRangeParams {
  workspace_id?: string;
  limit?: number;
  offset?: number;
}

export interface SecurityRunListParams extends SecurityTimeRangeParams {
  limit?: number;
  offset?: number;
}

export interface SecurityCountByParams extends SecurityTimeRangeParams {
  event_type?: string;
  category?: string;
  result?: string;
  verdict?: string;
  trace_id?: string;
  session_id?: string;
  run_id?: string;
  call_id?: string;
  tool_call_id?: string;
}

export interface SecurityTimelineParams extends SecurityTimeRangeParams {
  session_id: string;
  run_id: string;
  limit?: number;
  include_security?: boolean;
}

function buildQuery(params?: object): string {
  if (!params) return '';
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(params) as [string, SecurityQueryValue][]) {
    if (value === undefined || value === null || value === '') continue;
    query.set(key, String(value));
  }
  const qs = query.toString();
  return qs ? `?${qs}` : '';
}

async function securityFetch<T>(url: string): Promise<SecurityApiResponse<T>> {
  const res = await fetch(url, { credentials: 'same-origin' });
  if (res.status === 401) {
    window.location.hash = '#/login';
    throw new Error('Authentication required');
  }
  const text = await res.text().catch(() => '');
  let body: unknown = null;
  if (text) {
    try {
      body = JSON.parse(text);
    } catch {
      body = text;
    }
  }

  if (body && typeof body === 'object' && 'state' in body) {
    const envelope = body as Record<string, unknown>;
    if (typeof envelope.state !== 'string' || !isObjectRecord(envelope.data)) {
      throw new SecurityApiClientError(res.status, {
        code: 'malformed_security_response',
        message: 'Security API returned a malformed state response',
        retryable: false,
      });
    }
    return body as SecurityApiResponse<T>;
  }

  if (!res.ok) {
    const errorBody = body && typeof body === 'object' && 'error' in body
      ? (body as { error: SecurityRestError }).error
      : {
          code: 'security_api_error',
          message: typeof body === 'string' && body ? body : res.statusText,
          retryable: false,
        };
    throw new SecurityApiClientError(res.status, errorBody);
  }

  return {
    state: 'ok',
    data: body as T,
    meta: { source: 'agentsight' },
  };
}

function isObjectRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export async function fetchSecurityStatus(): Promise<SecurityApiResponse<SecurityStatusData>> {
  return securityFetch<SecurityStatusData>(`${API_BASE}/api/security/status`);
}

export async function fetchSecuritySummary(
  params?: SecurityTimeRangeParams & { latest_limit?: number },
): Promise<SecurityApiResponse<SecuritySummary>> {
  return securityFetch<SecuritySummary>(
    `${API_BASE}/api/security/summary${buildQuery(params)}`
  );
}

export async function fetchSecurityEvents(
  params?: SecurityEventListParams,
): Promise<SecurityApiResponse<SecurityPaginated<SecurityEventRecord>>> {
  return securityFetch<SecurityPaginated<SecurityEventRecord>>(
    `${API_BASE}/api/security/events${buildQuery(params)}`
  );
}

export async function fetchSecurityEvent(
  eventId: string,
): Promise<SecurityApiResponse<SecurityEventDetailResponse>> {
  return securityFetch<SecurityEventDetailResponse>(
    `${API_BASE}/api/security/events/${encodeURIComponent(eventId)}`
  );
}

export async function fetchSecurityCountBy(
  groupBy: string,
  params?: SecurityCountByParams,
): Promise<SecurityApiResponse<SecurityCountByResponse>> {
  return securityFetch<SecurityCountByResponse>(
    `${API_BASE}/api/security/events/count-by${buildQuery({ ...params, group_by: groupBy })}`
  );
}

export async function fetchSecuritySessions(
  params?: SecuritySessionListParams,
): Promise<SecurityApiResponse<SecurityPaginated<SecuritySessionSummary>>> {
  return securityFetch<SecurityPaginated<SecuritySessionSummary>>(
    `${API_BASE}/api/security/observability/sessions${buildQuery(params)}`
  );
}

export async function fetchSecurityRuns(
  sessionId: string,
  params?: SecurityRunListParams,
): Promise<SecurityApiResponse<SecurityPaginated<SecurityRunSummary>>> {
  return securityFetch<SecurityPaginated<SecurityRunSummary>>(
    `${API_BASE}/api/security/observability/sessions/${encodeURIComponent(sessionId)}/runs${buildQuery(params)}`
  );
}

export async function fetchSecurityTimeline(
  params: SecurityTimelineParams,
): Promise<SecurityApiResponse<SecurityTimelineResponse>> {
  const { session_id, run_id, ...queryParams } = params;
  return securityFetch<SecurityTimelineResponse>(
    `${API_BASE}/api/security/observability/timeline${buildQuery({
      ...queryParams,
      session_id,
      run_id,
    })}`
  );
}

// ─── Skill Metrics types ──────────────────────────────────────────────────────

export interface SkillFirstSeen {
  first_seen_session_id: string;
  first_seen_timestamp_ns: number;
  total_sessions: number;
}

export interface SkillDownloadMetrics {
  downloads: Record<string, SkillFirstSeen>;
}

export interface SkillLoadMetrics {
  loads: Record<string, number>;
  total_loads: number;
}

export interface SkillUsageRatio {
  ratio: number;
  with_skill_count: number;
  without_skill_count: number;
  total_sessions: number;
}

export interface SkillCountDistribution {
  min: number;
  max: number;
  mean: number;
  median: number;
  p90: number;
  histogram: [number, number, number, number, number, number];
}

export interface WeeklyRank {
  iso_week: string;
  load_count: number;
  rank: number;
}

export interface SkillRankEntry {
  skill_name: string;
  total_loads: number;
  total_rank: number;
  weekly_ranks: WeeklyRank[];
  rank_delta: number | null;
}

export interface SkillHotnessRanking {
  rankings: SkillRankEntry[];
}

export interface SkillMetricsReport {
  downloads: SkillDownloadMetrics | null;
  loads: SkillLoadMetrics | null;
  usage_ratio: SkillUsageRatio | null;
  distribution: SkillCountDistribution | null;
  hotness: SkillHotnessRanking | null;
  computed_at: string;
  time_range_ns: [number, number];
  event_count: number;
}

// ─── Skill Metrics API ────────────────────────────────────────────────────────

function buildSkillMetricsParams(startNs?: number, endNs?: number, agentName?: string, granularity?: string): string {
  const params = new URLSearchParams();
  if (startNs !== undefined) params.set('start_ns', String(startNs));
  if (endNs !== undefined) params.set('end_ns', String(endNs));
  if (agentName) params.set('agent_name', agentName);
  if (granularity) params.set('granularity', granularity);
  const qs = params.toString();
  return qs ? `?${qs}` : '';
}

export async function fetchSkillMetrics(
  startNs?: number,
  endNs?: number,
  agentName?: string,
  granularity?: string,
): Promise<SkillMetricsReport> {
  return apiFetch<SkillMetricsReport>(
    `${API_BASE}/api/skill-metrics${buildSkillMetricsParams(startNs, endNs, agentName, granularity)}`
  );
}

// ─── Authentication API ──────────────────────────────────────────────────────

export interface AuthStatusResponse {
  auth_enabled: boolean;
}

export interface AuthVerifyResponse {
  authenticated: boolean;
}

/**
 * Check whether dashboard authentication is enabled.
 */
export async function fetchAuthStatus(): Promise<AuthStatusResponse> {
  const res = await fetch(`${API_BASE}/api/auth/status`);
  if (!res.ok) throw new Error(`GET /api/auth/status -> ${res.status}`);
  return res.json();
}

/**
 * Verify the current session (cookie-based).
 */
export async function fetchAuthVerify(): Promise<AuthVerifyResponse> {
  const res = await fetch(`${API_BASE}/api/auth/verify`, { credentials: 'same-origin' });
  if (!res.ok) throw new Error(`GET /api/auth/verify -> ${res.status}`);
  return res.json();
}

/**
 * Log in with a token string. Sets a session cookie on success.
 */
export async function login(token: string): Promise<boolean> {
  const res = await fetch(`${API_BASE}/api/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ token }),
  });
  return res.ok;
}
