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
  INTERRUPTION_TYPE_CN,
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
});

describe('apiClient', () => {
  describe('INTERRUPTION_TYPE_CN', () => {
    it('should map all known interruption types to Chinese labels', () => {
      expect(INTERRUPTION_TYPE_CN.llm_error).toBe('LLM 错误');
      expect(INTERRUPTION_TYPE_CN.sse_truncated).toBe('SSE 截断');
      expect(INTERRUPTION_TYPE_CN.context_overflow).toBe('上下文溢出');
      expect(INTERRUPTION_TYPE_CN.agent_crash).toBe('Agent 崩溃');
      expect(INTERRUPTION_TYPE_CN.token_limit).toBe('Token 超限');
    });
  });

  describe('fetchSessions', () => {
    it('should fetch sessions without params', async () => {
      mockFetch.mockResolvedValueOnce(mockJsonResponse([]));
      const result = await fetchSessions();
      expect(result).toEqual([]);
      expect(mockFetch).toHaveBeenCalledWith(expect.stringContaining('/api/sessions'));
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
      expect(mockFetch.mock.calls[0][1]).toEqual({ method: 'POST' });
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
      expect(mockFetch.mock.calls[0][1]).toEqual({ method: 'DELETE' });
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
      expect(mockFetch.mock.calls[0][1]).toEqual({ method: 'POST' });
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
});
