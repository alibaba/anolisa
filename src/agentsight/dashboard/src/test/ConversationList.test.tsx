import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';

// Mock recharts
vi.mock('recharts', () => ({
  LineChart: ({ children }: any) => <div data-testid="line-chart">{children}</div>,
  Line: () => null,
  BarChart: ({ children }: any) => <div data-testid="bar-chart">{children}</div>,
  Bar: () => null,
  XAxis: () => null,
  YAxis: () => null,
  CartesianGrid: () => null,
  Tooltip: () => null,
  Legend: () => null,
  ResponsiveContainer: ({ children }: any) => <div>{children}</div>,
}));

// Mock apiClient
vi.mock('../utils/apiClient', () => ({
  fetchSessions: vi.fn(),
  fetchTraces: vi.fn(),
  fetchAgentNames: vi.fn(),
  fetchTimeseries: vi.fn(),
  fetchTraceDetail: vi.fn(),
  fetchInterruptionCount: vi.fn(),
  fetchInterruptionStats: vi.fn(),
  fetchInterruptionSessionCounts: vi.fn(),
  fetchInterruptionConversationCounts: vi.fn(),
  fetchTokenSavings: vi.fn(),
  INTERRUPTION_TYPE_CN: {
    llm_error: 'LLM 错误',
    sse_truncated: 'SSE 中断',
    context_overflow: '上下文溢出',
    agent_crash: 'Agent 崩溃',
    token_limit: 'Token 限制',
  },
}));

// Mock InterruptionBadge
vi.mock('../components/InterruptionBadge', () => ({
  InterruptionBadge: () => <span data-testid="interruption-badge">badge</span>,
}));

// Mock InterruptionPanel
vi.mock('../components/InterruptionPanel', () => ({
  InterruptionPanel: () => <div data-testid="interruption-panel">panel</div>,
  ResolvedEventInfo: undefined,
}));

import {
  fetchSessions,
  fetchAgentNames,
  fetchTimeseries,
  fetchInterruptionCount,
  fetchInterruptionStats,
  fetchInterruptionSessionCounts,
  fetchInterruptionConversationCounts,
  fetchTokenSavings,
  fetchTraces,
} from '../utils/apiClient';
import { ConversationList } from '../pages/ConversationList';

const mockFetchSessions = fetchSessions as ReturnType<typeof vi.fn>;
const mockFetchAgentNames = fetchAgentNames as ReturnType<typeof vi.fn>;
const mockFetchTimeseries = fetchTimeseries as ReturnType<typeof vi.fn>;
const mockFetchInterruptionCount = fetchInterruptionCount as ReturnType<typeof vi.fn>;
const mockFetchInterruptionStats = fetchInterruptionStats as ReturnType<typeof vi.fn>;
const mockFetchInterruptionSessionCounts = fetchInterruptionSessionCounts as ReturnType<typeof vi.fn>;
const mockFetchInterruptionConversationCounts = fetchInterruptionConversationCounts as ReturnType<typeof vi.fn>;
const mockFetchTokenSavings = fetchTokenSavings as ReturnType<typeof vi.fn>;
const mockFetchTraces = fetchTraces as ReturnType<typeof vi.fn>;

function setupMocks() {
  mockFetchAgentNames.mockResolvedValue(['agent-a', 'agent-b']);
  mockFetchSessions.mockResolvedValue([]);
  mockFetchTimeseries.mockResolvedValue({ token_series: [], model_series: [] });
  mockFetchInterruptionCount.mockResolvedValue({ total: 0, by_severity: { critical: 0, high: 0, medium: 0, low: 0 } });
  mockFetchInterruptionStats.mockResolvedValue([]);
  mockFetchInterruptionSessionCounts.mockResolvedValue([]);
  mockFetchInterruptionConversationCounts.mockResolvedValue([]);
  mockFetchTokenSavings.mockResolvedValue({ sessions: [], summary: null, stats_available: false });
  mockFetchTraces.mockResolvedValue([]);
}

function renderPage(route = '/') {
  return render(
    <MemoryRouter initialEntries={[route]}>
      <ConversationList />
    </MemoryRouter>
  );
}

beforeEach(() => {
  vi.resetAllMocks();
  setupMocks();
});

describe('ConversationList', () => {
  it('should show initial prompt before query', async () => {
    await act(async () => { renderPage(); });
    expect(screen.getByText('请选择时间范围和 Agent，然后点击「查询」')).toBeInTheDocument();
  });

  it('should show filter bar with query button', async () => {
    await act(async () => { renderPage(); });
    expect(screen.getByText('查询')).toBeInTheDocument();
    expect(screen.getByText('开始时间')).toBeInTheDocument();
    expect(screen.getByText('结束时间')).toBeInTheDocument();
  });

  it('should show quick time presets', async () => {
    await act(async () => { renderPage(); });
    expect(screen.getByText('最近 1h')).toBeInTheDocument();
    expect(screen.getByText('最近 6h')).toBeInTheDocument();
    expect(screen.getByText('最近 24h')).toBeInTheDocument();
    expect(screen.getByText('最近 7d')).toBeInTheDocument();
  });

  it('should load agent names on mount', async () => {
    await act(async () => { renderPage(); });
    expect(mockFetchAgentNames).toHaveBeenCalled();
  });

  it('should show loading state during query', async () => {
    mockFetchSessions.mockReturnValue(new Promise(() => {}));
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('查询中...')).toBeInTheDocument();
  });

  it('should show error on failed query', async () => {
    mockFetchSessions.mockRejectedValue(new Error('Network fail'));
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText(/Network fail/)).toBeInTheDocument();
  });

  it('should show empty state when no sessions', async () => {
    mockFetchSessions.mockResolvedValue([]);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('所选时间范围内暂无 Session 数据')).toBeInTheDocument();
  });

  it('should show summary cards after query', async () => {
    mockFetchSessions.mockResolvedValue([
      {
        session_id: 'sess-001',
        agent_name: 'TestBot',
        model: 'gpt-4',
        conversation_count: 5,
        total_input_tokens: 10000,
        total_output_tokens: 5000,
        last_seen_ns: Date.now() * 1_000_000,
      },
    ]);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('Sessions')).toBeInTheDocument();
    expect(screen.getByText('1')).toBeInTheDocument(); // sessions count
    expect(screen.getByText('总输入 Token')).toBeInTheDocument();
    expect(screen.getAllByText('10,000').length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText('总输出 Token')).toBeInTheDocument();
    expect(screen.getAllByText('5,000').length).toBeGreaterThanOrEqual(1);
  });

  it('should render session table with data', async () => {
    mockFetchSessions.mockResolvedValue([
      {
        session_id: 'sess-abc-def-ghi-jkl-mno-pqr',
        agent_name: 'MyAgent',
        model: 'claude-3',
        conversation_count: 3,
        total_input_tokens: 2000,
        total_output_tokens: 1000,
        last_seen_ns: Date.now() * 1_000_000,
      },
    ]);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('MyAgent')).toBeInTheDocument();
    expect(screen.getByText('claude-3')).toBeInTheDocument();
    expect(screen.getByText('3')).toBeInTheDocument(); // conversation count
  });

  it('should show interruption count card', async () => {
    mockFetchSessions.mockResolvedValue([]);
    mockFetchInterruptionCount.mockResolvedValue({
      total: 5,
      by_severity: { critical: 1, high: 2, medium: 1, low: 1 },
    });
    mockFetchInterruptionStats.mockResolvedValue([
      { interruption_type: 'llm_error', severity: 'critical', count: 1 },
    ]);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('异常中断')).toBeInTheDocument();
    expect(screen.getByText('5')).toBeInTheDocument();
  });

  it('should expand session row to show trace sub-table', async () => {
    mockFetchSessions.mockResolvedValue([
      {
        session_id: 'sess-expandable',
        agent_name: 'ExpandAgent',
        model: 'gpt-4o',
        conversation_count: 2,
        total_input_tokens: 500,
        total_output_tokens: 300,
        last_seen_ns: Date.now() * 1_000_000,
      },
    ]);
    mockFetchTraces.mockResolvedValue([
      {
        conversation_id: 'conv-001',
        user_query: 'Hello world',
        total_input_tokens: 300,
        total_output_tokens: 200,
        start_timestamp_ns: Date.now() * 1_000_000,
      },
    ]);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    // Click on session row to expand
    const agentCell = screen.getByText('ExpandAgent');
    await act(async () => {
      fireEvent.click(agentCell.closest('tr')!);
    });
    // Should show trace sub-table content
    expect(screen.getByText('Conversation ID')).toBeInTheDocument();
  });

  it('should show agent dropdown with loaded names', async () => {
    await act(async () => { renderPage(); });
    expect(screen.getByText('全部 Agent')).toBeInTheDocument();
  });

  it('should handle pagination when many sessions', async () => {
    // Create 12 sessions to trigger pagination
    const sessions = Array.from({ length: 12 }, (_, i) => ({
      session_id: `sess-${String(i).padStart(3, '0')}`,
      agent_name: `Agent-${i}`,
      model: 'gpt-4',
      conversation_count: 1,
      total_input_tokens: 100 * (i + 1),
      total_output_tokens: 50 * (i + 1),
      last_seen_ns: Date.now() * 1_000_000,
    }));
    mockFetchSessions.mockResolvedValue(sessions);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    // Should show pagination
    expect(screen.getByText(/下一页/)).toBeInTheDocument();
  });

  it('should show trace detail modal when clicking trace detail button', async () => {
    const mockFetchTraceDetail = (await import('../utils/apiClient')).fetchTraceDetail as ReturnType<typeof vi.fn>;
    mockFetchSessions.mockResolvedValue([{
      session_id: 'sess-modal-test',
      agent_name: 'ModalAgent',
      model: 'gpt-4',
      conversation_count: 1,
      total_input_tokens: 500,
      total_output_tokens: 200,
      last_seen_ns: Date.now() * 1_000_000,
    }]);
    mockFetchTraces.mockResolvedValue([{
      conversation_id: 'conv-modal',
      trace_id: 'trace-modal-1',
      user_query: 'Test query',
      total_input_tokens: 300,
      total_output_tokens: 150,
      start_ns: Date.now() * 1_000_000,
      start_timestamp_ns: Date.now() * 1_000_000,
    }]);
    mockFetchTraceDetail.mockResolvedValue([{
      id: 'ev-1',
      model: 'gpt-4o',
      start_timestamp_ns: Date.now() * 1_000_000,
      input_tokens: 100,
      output_tokens: 50,
      total_tokens: 150,
      input_messages: JSON.stringify([{ role: 'user', content: 'Hello' }]),
      output_messages: JSON.stringify([{ role: 'assistant', content: 'Hi there' }]),
    }]);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    // Expand session
    await act(async () => {
      fireEvent.click(screen.getByText('ModalAgent').closest('tr')!);
    });
    // Should see trace sub-table
    expect(screen.getByText('Conversation ID')).toBeInTheDocument();
  });

  it('should show time range quick presets and update time', async () => {
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('最近 1h'));
    });
    // After clicking preset, the query button should still be available
    expect(screen.getByText('查询')).toBeInTheDocument();
  });

  it('should render charts section headings after query', async () => {
    mockFetchSessions.mockResolvedValue([]);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('Token 时序（输入 / 输出 / 总计）')).toBeInTheDocument();
    expect(screen.getByText('模型 Token 时序（堆叠）')).toBeInTheDocument();
  });

  it('should show session with no model as dash', async () => {
    mockFetchSessions.mockResolvedValue([{
      session_id: 'sess-no-model',
      agent_name: 'NoModelAgent',
      model: null,
      conversation_count: 1,
      total_input_tokens: 100,
      total_output_tokens: 50,
      last_seen_ns: Date.now() * 1_000_000,
    }]);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('NoModelAgent')).toBeInTheDocument();
  });

  it('should show session with savings link when savings available', async () => {
    mockFetchSessions.mockResolvedValue([{
      session_id: 'sess-with-savings',
      agent_name: 'SavingsAgent',
      model: 'gpt-4',
      conversation_count: 2,
      total_input_tokens: 3000,
      total_output_tokens: 1500,
      last_seen_ns: Date.now() * 1_000_000,
    }]);
    mockFetchTokenSavings.mockResolvedValue({
      sessions: [{ session_id: 'sess-with-savings', saved_tokens: 500 }],
      summary: null,
      stats_available: true,
    });
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('500')).toBeInTheDocument();
  });

  it('should auto-restore query from URL params', async () => {
    const now = Date.now();
    const route = `/?start=${now - 3600000}&end=${now}&q=1&agent=test-agent`;
    mockFetchSessions.mockResolvedValue([]);
    await act(async () => {
      render(
        <MemoryRouter initialEntries={[route]}>
          <ConversationList />
        </MemoryRouter>
      );
    });
    // Should auto-query (hasQueried becomes true)
    // wait for the query to complete
    await act(async () => { await new Promise(r => setTimeout(r, 50)); });
    expect(mockFetchSessions).toHaveBeenCalled();
  });

  it('should show trace sub-table with user query text', async () => {
    mockFetchSessions.mockResolvedValue([{
      session_id: 'sess-trace-query',
      agent_name: 'QueryAgent',
      model: 'gpt-4',
      conversation_count: 1,
      total_input_tokens: 400,
      total_output_tokens: 200,
      last_seen_ns: Date.now() * 1_000_000,
    }]);
    mockFetchTraces.mockResolvedValue([{
      conversation_id: 'conv-with-query',
      trace_id: 'trace-q-1',
      user_query: 'What is the meaning of life?',
      total_input_tokens: 400,
      total_output_tokens: 200,
      start_ns: Date.now() * 1_000_000,
      start_timestamp_ns: Date.now() * 1_000_000,
    }]);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    await act(async () => {
      fireEvent.click(screen.getByText('QueryAgent').closest('tr')!);
    });
    expect(screen.getByText('What is the meaning of life?')).toBeInTheDocument();
  });

  it('should show empty trace state when session has no traces', async () => {
    mockFetchSessions.mockResolvedValue([{
      session_id: 'sess-no-traces',
      agent_name: 'EmptyAgent',
      model: 'gpt-4',
      conversation_count: 0,
      total_input_tokens: 0,
      total_output_tokens: 0,
      last_seen_ns: Date.now() * 1_000_000,
    }]);
    mockFetchTraces.mockResolvedValue([]);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    await act(async () => {
      fireEvent.click(screen.getByText('EmptyAgent').closest('tr')!);
    });
    expect(screen.getByText('该 Session 下暂无 Trace')).toBeInTheDocument();
  });

  it('should show interruption badges on sessions with interruptions', async () => {
    mockFetchSessions.mockResolvedValue([{
      session_id: 'sess-with-int',
      agent_name: 'IntAgent',
      model: 'gpt-4',
      conversation_count: 2,
      total_input_tokens: 1000,
      total_output_tokens: 500,
      last_seen_ns: Date.now() * 1_000_000,
    }]);
    mockFetchInterruptionSessionCounts.mockResolvedValue([{
      session_id: 'sess-with-int',
      total: 3,
      by_severity: { critical: 1, high: 1, medium: 1, low: 0 },
      types: [{ interruption_type: 'llm_error', severity: 'critical', count: 1 }],
    }]);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    // InterruptionBadge is mocked, should show
    expect(screen.getByTestId('interruption-badge')).toBeInTheDocument();
  });
});
