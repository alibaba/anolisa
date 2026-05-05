import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';

// Mock recharts to avoid SVG rendering issues
vi.mock('recharts', () => ({
  PieChart: ({ children }: any) => <div data-testid="pie-chart">{children}</div>,
  Pie: () => null,
  Cell: () => null,
  ResponsiveContainer: ({ children }: any) => <div>{children}</div>,
}));

// Mock apiClient
vi.mock('../utils/apiClient', () => ({
  fetchTokenSavings: vi.fn(),
  fetchAgentNames: vi.fn(),
}));

import { fetchTokenSavings, fetchAgentNames } from '../utils/apiClient';
import { TokenSavingsPage } from '../pages/TokenSavingsPage';

const mockFetchTokenSavings = fetchTokenSavings as ReturnType<typeof vi.fn>;
const mockFetchAgentNames = fetchAgentNames as ReturnType<typeof vi.fn>;

function renderPage(route = '/savings') {
  return render(
    <MemoryRouter initialEntries={[route]}>
      <TokenSavingsPage />
    </MemoryRouter>
  );
}

beforeEach(() => {
  mockFetchTokenSavings.mockReset();
  mockFetchAgentNames.mockReset();
  mockFetchAgentNames.mockResolvedValue(['agent-1', 'agent-2']);
});

describe('TokenSavingsPage', () => {
  it('should show initial prompt before query', async () => {
    await act(async () => { renderPage(); });
    expect(screen.getByText('请选择时间范围，然后点击「查询」')).toBeInTheDocument();
    expect(screen.getByText('查看 Token 节省效果')).toBeInTheDocument();
  });

  it('should show query button', async () => {
    await act(async () => { renderPage(); });
    expect(screen.getByText('查询')).toBeInTheDocument();
  });

  it('should show time filter inputs', async () => {
    await act(async () => { renderPage(); });
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
    mockFetchTokenSavings.mockReturnValue(new Promise(() => {}));
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('查询中...')).toBeInTheDocument();
  });

  it('should show error on failed query', async () => {
    mockFetchTokenSavings.mockRejectedValue(new Error('Server down'));
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('Server down')).toBeInTheDocument();
  });

  it('should show stats unavailable notice', async () => {
    mockFetchTokenSavings.mockResolvedValue({
      sessions: [],
      summary: { total_input_tokens: 0, total_output_tokens: 0, total_tokens: 0, total_compounded_saved: 0, total_compounded_tool_saved: 0, total_compounded_mcp_saved: 0, compounded_savings_rate: 0 },
      stats_available: false,
    });
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('未发现优化记录')).toBeInTheDocument();
  });

  it('should render summary cards after successful query', async () => {
    mockFetchTokenSavings.mockResolvedValue({
      sessions: [],
      summary: {
        total_input_tokens: 5000,
        total_output_tokens: 3000,
        total_tokens: 8000,
        total_compounded_saved: 2000,
        total_compounded_tool_saved: 1200,
        total_compounded_mcp_saved: 800,
        compounded_savings_rate: 25.0,
      },
      stats_available: true,
    });
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('总 Token 消耗')).toBeInTheDocument();
    expect(screen.getByText('8,000')).toBeInTheDocument();
    expect(screen.getByText('已降低 Token')).toBeInTheDocument();
    expect(screen.getByText('2,000')).toBeInTheDocument();
    expect(screen.getAllByText('降低率').length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText('25.0%')).toBeInTheDocument();
    expect(screen.getByText('良好')).toBeInTheDocument();
  });

  it('should render session list after query', async () => {
    mockFetchTokenSavings.mockResolvedValue({
      sessions: [{
        session_id: 'sess-abc-123',
        agent_name: 'TestAgent',
        total_input_tokens: 1000,
        total_output_tokens: 500,
        saved_tokens: 300,
        compounded_saved: 300,
        compounded_savings_rate: 30.0,
        optimization_items: [],
      }],
      summary: {
        total_input_tokens: 1000,
        total_output_tokens: 500,
        total_tokens: 1500,
        total_compounded_saved: 300,
        total_compounded_tool_saved: 200,
        total_compounded_mcp_saved: 100,
        compounded_savings_rate: 20.0,
      },
      stats_available: true,
    });
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('Session ID')).toBeInTheDocument();
    expect(screen.getByText('TestAgent')).toBeInTheDocument();
  });

  it('should expand session row to show optimization details', async () => {
    mockFetchTokenSavings.mockResolvedValue({
      sessions: [{
        session_id: 'sess-expand-test',
        agent_name: 'Expander',
        total_input_tokens: 2000,
        total_output_tokens: 1000,
        saved_tokens: 500,
        compounded_saved: 500,
        compounded_savings_rate: 25.0,
        optimization_items: [{
          id: 'opt-1',
          category: 'tool_output',
          before_tokens: 400,
          after_tokens: 100,
          compounded_saved: 300,
          before_text: 'long original text',
          after_text: 'short text',
          diff_lines: [],
        }],
      }],
      summary: {
        total_input_tokens: 2000,
        total_output_tokens: 1000,
        total_tokens: 3000,
        total_compounded_saved: 500,
        total_compounded_tool_saved: 300,
        total_compounded_mcp_saved: 200,
        compounded_savings_rate: 16.7,
      },
      stats_available: true,
    });
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    // Click session row to expand
    const row = screen.getByText('Expander').closest('tr');
    await act(async () => {
      fireEvent.click(row!);
    });
    expect(screen.getByText('工具输出')).toBeInTheDocument();
    expect(screen.getAllByText('详情').length).toBeGreaterThanOrEqual(1);
  });

  it('should show savings rate badge as 优秀 when >= 30%', async () => {
    mockFetchTokenSavings.mockResolvedValue({
      sessions: [],
      summary: {
        total_input_tokens: 5000,
        total_output_tokens: 3000,
        total_tokens: 8000,
        total_compounded_saved: 4000,
        total_compounded_tool_saved: 3000,
        total_compounded_mcp_saved: 1000,
        compounded_savings_rate: 50.0,
      },
      stats_available: true,
    });
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('优秀')).toBeInTheDocument();
  });
});
