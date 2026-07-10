import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  fetchSessions,
  fetchTraces,
  fetchTraceDetail,
  fetchConversationDetail,
  fetchAgentNames,
  fetchTimeseries,
  fetchTokenSavings,
  fetchAtifByTrace,
  fetchAtifBySession,
  fetchAtifByConversation,
  fetchInterruptionStats,
  fetchSessionInterruptions,
  fetchConversationInterruptions,
  fetchInterruptionCount,
  resolveInterruption,
  fetchInterruptionSessionCounts,
  fetchInterruptionConversationCounts,
  fetchAgentHealth,
  deleteAgentHealth,
  restartAgentHealth,
  fetchLatestEvaluation,
  evaluateConversation,
  EvaluationNotReadyError,
  INTERRUPTION_TYPE_CN,
  fetchSkillMetrics,
  fetchSecurityStatus,
  fetchSecuritySummary,
  fetchSecurityEvents,
  fetchSecurityEvent,
  fetchSecurityCountBy,
  fetchSecuritySessions,
  fetchSecurityRuns,
  fetchSecurityTimeline,
} from '../utils/apiClient';

// Mock global fetch
const mockFetch = vi.fn();
vi.stubGlobal('fetch', mockFetch);

function mockJsonResponse(data: any, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: 'OK',
    json: () => Promise.resolve(data),
    text: () => Promise.resolve(JSON.stringify(data)),
  };
}

function mockErrorResponse(status: number, text: string) {
  return {
    ok: false,
    status,
    statusText: text,
    json: () => Promise.reject(new Error('not json')),
    text: () => Promise.resolve(text),
  };
}

beforeEach(() => {
  mockFetch.mockReset();
  window.location.hash = '';
});

afterEach(() => {
  window.location.hash = '';
});

describe('apiClient', () => {
  describe('INTERRUPTION_TYPE_CN', () => {
    it('should map all known interruption types to Chinese labels', () => {
      expect(INTERRUPTION_TYPE_CN.llm_error).toBe('LLM 错误');
      expect(INTERRUPTION_TYPE_CN.sse_truncated).toBe('SSE 截断');
      expect(INTERRUPTION_TYPE_CN.context_overflow).toBe('上下文溢出');
      expect(INTERRUPTION_TYPE_CN.agent_crash).toBe('Agent 崩溃');
      expect(INTERRUPTION_TYPE_CN.token_limit).toBe('Token 超限');
      expect(INTERRUPTION_TYPE_CN.rate_limit).toBe('速率限制');
      expect(INTERRUPTION_TYPE_CN.auth_error).toBe('鉴权错误');
      expect(INTERRUPTION_TYPE_CN.network_timeout).toBe('网络超时');
      expect(INTERRUPTION_TYPE_CN.service_unavailable).toBe('服务不可用');
      expect(INTERRUPTION_TYPE_CN.safety_filter).toBe('安全过滤');
    });
  });

  describe('fetchSessions', () => {
    it('should fetch sessions without params', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse([]));
      const result = await fetchSessions();
      expect(result).toEqual([]);
      expect(mockFetch).toHaveBeenCalledWith(
        expect.stringContaining('/api/sessions'),
        expect.objectContaining({ credentials: 'same-origin' }),
      );
    });

    it('should add start_ns and end_ns params', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse([]));
      await fetchSessions(1000, 2000);
      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('start_ns=1000');
      expect(url).toContain('end_ns=2000');
    });
  });

  describe('fetchTraces', () => {
    it('should fetch traces for a session', async () => {
      const mockTraces = [{ trace_id: 't1', conversation_id: 'c1' }];
      mockFetch.mockResolvedValueOnce(mockJsonResponse(mockTraces));
      const result = await fetchTraces('session-1');
      expect(result).toEqual(mockTraces);
      expect(mockFetch.mock.calls[0][0]).toContain('/api/sessions/session-1/traces');
    });

    it('should include time range params', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse([]));
      await fetchTraces('s1', 100, 200);
      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('start_ns=100');
      expect(url).toContain('end_ns=200');
    });

    it('should skip null params', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse([]));
      await fetchTraces('s1', null, null);
      const url = mockFetch.mock.calls[0][0];
      expect(url).not.toContain('start_ns');
      expect(url).not.toContain('end_ns');
    });
  });

  describe('fetchTraceDetail', () => {
    it('should fetch trace detail', async () => {
      const events = [{ id: 1, call_id: 'c1' }];
      mockFetch.mockResolvedValueOnce(mockJsonResponse(events));
      const result = await fetchTraceDetail('trace-abc');
      expect(result).toEqual(events);
      expect(mockFetch.mock.calls[0][0]).toContain('/api/traces/trace-abc');
    });
  });

  describe('fetchConversationDetail', () => {
    it('should fetch conversation detail', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse([]));
      await fetchConversationDetail('conv-1');
      expect(mockFetch.mock.calls[0][0]).toContain('/api/conversations/conv-1');
    });
  });

  describe('Grader APIs', () => {
    it('fetchLatestEvaluation should fetch latest conversation evaluation', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse(null));
      const result = await fetchLatestEvaluation('conv-1');

      expect(result).toBeNull();
      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('/api/grader/latest');
      expect(url).toContain('target_type=conversation');
      expect(url).toContain('target_id=conv-1');
    });

    it('evaluateConversation should post a manual evaluation request', async () => {
      const response = { result: { target_id: 'conv-1', verdict: 'pass' }, reused_existing_run: false };
      mockFetch.mockResolvedValueOnce(mockJsonResponse(response));
      const result = await evaluateConversation('conv-1', true);

      expect(result).toEqual(response);
      expect(mockFetch.mock.calls[0][0]).toContain('/api/grader/evaluate');
      expect(mockFetch.mock.calls[0][1]).toMatchObject({
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'same-origin',
      });
      expect(JSON.parse(mockFetch.mock.calls[0][1].body)).toEqual({
        target_type: 'conversation',
        target_id: 'conv-1',
        force: true,
      });
    });

    it('evaluateConversation should expose pending conflicts as EvaluationNotReadyError', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse({
        error: 'conversation_not_ready',
        pending_call_count: 2,
        message: 'pending',
      }, 409));

      let caught: unknown;
      try {
        await evaluateConversation('conv-1');
      } catch (error) {
        caught = error;
      }

      expect(caught).toMatchObject({
        name: 'EvaluationNotReadyError',
        pendingCallCount: 2,
      });
      expect(caught).toBeInstanceOf(EvaluationNotReadyError);
    });

    it('evaluateConversation should redirect to login after an unauthorized response', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse({}, 401));

      await expect(evaluateConversation('conv-1')).rejects.toThrow('Authentication required');

      expect(window.location.hash).toBe('#/login');
    });
  });

  describe('fetchAgentNames', () => {
    it('should fetch agent names', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse(['agent-a', 'agent-b']));
      const result = await fetchAgentNames();
      expect(result).toEqual(['agent-a', 'agent-b']);
    });

    it('should pass time params', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse([]));
      await fetchAgentNames(100, 200);
      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('start_ns=100');
      expect(url).toContain('end_ns=200');
    });
  });

  describe('fetchTimeseries', () => {
    it('should fetch timeseries with required params', async () => {
      const data = { token_series: [], model_series: [] };
      mockFetch.mockResolvedValueOnce(mockJsonResponse(data));
      const result = await fetchTimeseries(1000, 2000);
      expect(result).toEqual(data);
      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('start_ns=1000');
      expect(url).toContain('end_ns=2000');
      expect(url).toContain('buckets=30');
    });

    it('should include agent_name if provided', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse({ token_series: [], model_series: [] }));
      await fetchTimeseries(1000, 2000, 'my-agent', 10);
      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('agent_name=my-agent');
      expect(url).toContain('buckets=10');
    });
  });

  describe('fetchTokenSavings', () => {
    it('should fetch token savings', async () => {
      const data = { stats_available: true, summary: {}, sessions: [] };
      mockFetch.mockResolvedValueOnce(mockJsonResponse(data));
      const result = await fetchTokenSavings(100, 200);
      expect(result).toEqual(data);
    });

    it('should include agent_name', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse({ stats_available: false, summary: {}, sessions: [] }));
      await fetchTokenSavings(100, 200, 'agent-x');
      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('agent_name=agent-x');
    });
  });

  describe('ATIF export APIs', () => {
    it('fetchAtifByTrace', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse({ version: '1.6' }));
      await fetchAtifByTrace('trace-1');
      expect(mockFetch.mock.calls[0][0]).toContain('/api/export/atif/trace/trace-1');
    });

    it('fetchAtifBySession', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse({ version: '1.6' }));
      await fetchAtifBySession('session-1');
      expect(mockFetch.mock.calls[0][0]).toContain('/api/export/atif/session/session-1');
    });

    it('fetchAtifByConversation', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse({ version: '1.6' }));
      await fetchAtifByConversation('conv-1');
      expect(mockFetch.mock.calls[0][0]).toContain('/api/export/atif/conversation/conv-1');
    });
  });

  describe('Interruption APIs', () => {
    it('fetchInterruptionStats', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse([]));
      await fetchInterruptionStats(100, 200);
      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('/api/interruptions/stats');
      expect(url).toContain('start_ns=100');
    });

    it('fetchSessionInterruptions', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse([]));
      await fetchSessionInterruptions('s1');
      expect(mockFetch.mock.calls[0][0]).toContain('/api/sessions/s1/interruptions');
    });

    it('fetchConversationInterruptions', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse([]));
      await fetchConversationInterruptions('c1');
      expect(mockFetch.mock.calls[0][0]).toContain('/api/conversations/c1/interruptions');
    });

    it('fetchInterruptionCount without params', async () => {
      const data = { total: 5, by_severity: { critical: 1, high: 2, medium: 1, low: 1 } };
      mockFetch.mockResolvedValueOnce(mockJsonResponse(data));
      const result = await fetchInterruptionCount();
      expect(result.total).toBe(5);
    });

    it('fetchInterruptionCount with params', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse({ total: 0, by_severity: { critical: 0, high: 0, medium: 0, low: 0 } }));
      await fetchInterruptionCount(100, 200, 'agent-a');
      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('start_ns=100');
      expect(url).toContain('agent_name=agent-a');
    });

    it('fetchInterruptionSessionCounts', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse([]));
      await fetchInterruptionSessionCounts(100, 200);
      expect(mockFetch.mock.calls[0][0]).toContain('/api/interruptions/session-counts');
    });

    it('fetchInterruptionConversationCounts', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse([]));
      await fetchInterruptionConversationCounts(100, 200);
      expect(mockFetch.mock.calls[0][0]).toContain('/api/interruptions/conversation-counts');
    });
  });

  describe('resolveInterruption', () => {
    it('should POST to resolve endpoint', async () => {
      mockFetch.mockResolvedValueOnce({ ok: true, status: 200, text: () => Promise.resolve('') });
      await resolveInterruption('int-1');
      expect(mockFetch.mock.calls[0][0]).toContain('/api/interruptions/int-1/resolve');
      expect(mockFetch.mock.calls[0][1]).toEqual({
        method: 'POST',
        credentials: 'same-origin',
      });
    });

    it('should throw on error', async () => {
      mockFetch.mockResolvedValueOnce(mockErrorResponse(404, 'Not found'));
      await expect(resolveInterruption('bad-id')).rejects.toThrow('404');
    });
  });

  describe('Agent health APIs', () => {
    it('fetchAgentHealth', async () => {
      const data = { agents: [] };
      mockFetch.mockResolvedValueOnce(mockJsonResponse(data));
      const result = await fetchAgentHealth();
      expect(result).toEqual(data);
    });

    it('deleteAgentHealth success', async () => {
      mockFetch.mockResolvedValueOnce({ ok: true, status: 200, text: () => Promise.resolve('') });
      await deleteAgentHealth(1234);
      expect(mockFetch.mock.calls[0][0]).toContain('/api/agent-health/1234');
      expect(mockFetch.mock.calls[0][1]).toEqual({
        method: 'DELETE',
        credentials: 'same-origin',
      });
    });

    it('deleteAgentHealth error', async () => {
      mockFetch.mockResolvedValueOnce(mockErrorResponse(500, 'Internal error'));
      await expect(deleteAgentHealth(999)).rejects.toThrow('500');
    });

    it('restartAgentHealth success', async () => {
      const body = { ok: true, new_pid: 5678, cmd: ['node', 'agent.js'] };
      mockFetch.mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: () => Promise.resolve(body),
      });
      const result = await restartAgentHealth(1234);
      expect(result).toEqual(body);
      expect(mockFetch.mock.calls[0][1]).toEqual({
        method: 'POST',
        credentials: 'same-origin',
      });
    });

    it('restartAgentHealth error', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 500,
        statusText: 'Internal',
        json: () => Promise.resolve({ error: 'process not found' }),
      });
      await expect(restartAgentHealth(999)).rejects.toThrow('process not found');
    });
  });

  describe('Security Observability APIs', () => {
    it('fetchSecurityStatus returns state envelopes even on non-2xx status', async () => {
      const body = {
        state: 'daemon_unreachable',
        data: { socket_path: '/tmp/agent-sec.sock' },
        message: 'agent-sec daemon is unavailable',
      };
      mockFetch.mockResolvedValueOnce(mockJsonResponse(body, 503));

      const result = await fetchSecurityStatus();

      expect(result).toEqual(body);
      expect(mockFetch.mock.calls[0][0]).toContain('/api/security/status');
    });

    it('rejects malformed security state envelopes', async () => {
      mockFetch
        .mockResolvedValueOnce(mockJsonResponse({ state: 'daemon_reachable' }))
        .mockResolvedValueOnce(mockJsonResponse({ state: 'ok', data: null }));

      await expect(fetchSecurityStatus()).rejects.toMatchObject({
        status: 200,
        code: 'malformed_security_response',
        retryable: false,
      });
      await expect(fetchSecuritySummary()).rejects.toMatchObject({
        status: 200,
        code: 'malformed_security_response',
        retryable: false,
      });
    });

    it('fetchSecurityEvents builds query params', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse({
        state: 'ok',
        data: { items: [], total: 0, limit: 25, offset: 0, next_offset: null },
      }));

      await fetchSecurityEvents({
        start_ns: 100,
        end_ns: 200,
        category: 'code_scan',
        limit: 25,
        include_details: false,
      });

      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('/api/security/events');
      expect(url).toContain('start_ns=100');
      expect(url).toContain('end_ns=200');
      expect(url).toContain('category=code_scan');
      expect(url).toContain('include_details=false');
    });

    it('fetchSecuritySummary throws structured REST errors', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse({
        error: { code: 'daemon_timeout', message: 'request timed out', retryable: true },
      }, 504));

      await expect(fetchSecuritySummary()).rejects.toMatchObject({
        status: 504,
        code: 'daemon_timeout',
        retryable: true,
      });
    });

    it('fetches detail, count, sessions, runs, and timeline endpoints', async () => {
      mockFetch
        .mockResolvedValueOnce(mockJsonResponse({ state: 'found', data: { found: true, event: null } }))
        .mockResolvedValueOnce(mockJsonResponse({ state: 'ok', data: { group_by: 'category', items: [] } }))
        .mockResolvedValueOnce(mockJsonResponse({ state: 'ok', data: { items: [], total: 0, limit: 100, offset: 0 } }))
        .mockResolvedValueOnce(mockJsonResponse({ state: 'ok', data: { items: [], total: 0, limit: 100, offset: 0 } }))
        .mockResolvedValueOnce(mockJsonResponse({ state: 'ok', data: { session_id: 's1', run_id: 'r1', items: [] } }));

      await fetchSecurityEvent('event-1');
      await fetchSecurityCountBy('category', { start_ns: 1 });
      await fetchSecuritySessions({ limit: 100 });
      await fetchSecurityRuns('session-1', { limit: 100 });
      await fetchSecurityTimeline({
        session_id: 'session-1',
        run_id: 'run-1',
        start_ns: 1,
        end_ns: 2,
        include_security: true,
      });

      expect(mockFetch.mock.calls[0][0]).toContain('/api/security/events/event-1');
      expect(mockFetch.mock.calls[1][0]).toContain('/api/security/events/count-by');
      expect(mockFetch.mock.calls[1][0]).toContain('group_by=category');
      expect(mockFetch.mock.calls[2][0]).toContain('/api/security/observability/sessions');
      expect(mockFetch.mock.calls[3][0]).toContain('/api/security/observability/sessions/session-1/runs');
      expect(mockFetch.mock.calls[4][0]).toContain('/api/security/observability/timeline');
      expect(mockFetch.mock.calls[4][0]).toContain('start_ns=1');
      expect(mockFetch.mock.calls[4][0]).toContain('end_ns=2');
      expect(mockFetch.mock.calls[4][0]).toContain('include_security=true');
    });
  });

  describe('apiFetch error handling', () => {
    it('should throw on non-ok response', async () => {
      mockFetch.mockResolvedValueOnce(mockErrorResponse(500, 'Server Error'));
      await expect(fetchSessions()).rejects.toThrow('500');
    });

    it('should handle text() failure gracefully', async () => {
      mockFetch.mockResolvedValueOnce({
        ok: false,
        status: 502,
        statusText: 'Bad Gateway',
        text: () => Promise.reject(new Error('stream error')),
      });
      await expect(fetchSessions()).rejects.toThrow('Bad Gateway');
    });
  });

  describe('fetchSkillMetrics', () => {
    const mockReport = {
      event_count: 50,
      downloads: { downloads: { 'skill-a': 50 } },
      loads: { loads: { 'skill-a': 20 }, total_loads: 20 },
      usage_ratio: { ratio: 0.4 },
      distribution: { histogram: [0, 5, 10, 15, 10, 10], min: 0, max: 5, mean: 3.0 },
      hotness: { rankings: [{ skill_name: 'skill-a', total_loads: 20, total_rank: 1, rank_delta: null }] },
      computed_at: '2026-01-01T00:00:00Z',
      time_range_ns: [0, 1000000],
    };

    it('should call /api/skill-metrics without params', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse(mockReport));
      const result = await fetchSkillMetrics();
      expect(result).toEqual(mockReport);
      expect(mockFetch).toHaveBeenCalledWith(
        expect.stringContaining('/api/skill-metrics'),
        expect.objectContaining({ credentials: 'same-origin' }),
      );
    });

    it('should add start_ns and end_ns query params', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse(mockReport));
      await fetchSkillMetrics(1000, 2000);
      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('start_ns=1000');
      expect(url).toContain('end_ns=2000');
    });

    it('should add agent_name query param when provided', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse(mockReport));
      await fetchSkillMetrics(undefined, undefined, 'cosh');
      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('agent_name=cosh');
    });

    it('should add granularity query param when provided', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse(mockReport));
      await fetchSkillMetrics(undefined, undefined, undefined, 'week');
      const url = mockFetch.mock.calls[0][0];
      expect(url).toContain('granularity=week');
    });

    it('should throw on non-ok response', async () => {
      mockFetch.mockResolvedValueOnce(mockErrorResponse(500, 'Internal Server Error'));
      await expect(fetchSkillMetrics()).rejects.toThrow();
    });
  });
});
