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
      optimization_tips: [{ level: 'warning', title: '未检测到 Tokenless 组件', description: '未发现 stats.db，请确认 tokenless 组件已安装并启用。' }],
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
        strategy_breakdown: [
          { strategy: 'rewrite-command', label: '命令重写', saved: 600, compounded_saved: 1200 },
          { strategy: 'compress-response', label: '响应压缩', saved: 400, compounded_saved: 800 },
        ],
      },
      stats_available: true,
      optimization_tips: [{ level: 'success', title: '节省效果良好', description: '当前复合节省率 25.0%，已达到良好水平。' }],
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
        strategy_breakdown: [],
      },
      stats_available: true,
      optimization_tips: [],
    });
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('Session ID')).toBeInTheDocument();
    expect(screen.getByText('TestAgent')).toBeInTheDocument();
  });

  it('should expand session row to show optimization details with strategy badge', async () => {
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
          strategy: 'compress-schema',
          strategy_label: 'Schema 压缩',
          before_tokens: 400,
          after_tokens: 100,
          compounded_saved: 300,
          compression_ratio: 75.0,
          explanation: '工具输出优化: 原始 400 tokens → 100 tokens，压缩率 75.0%。后续 1 轮LLM调用均受益，复合节省 300 tokens。',
          compounding_turns: 1,
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
        strategy_breakdown: [
          { strategy: 'compress-schema', label: 'Schema 压缩', saved: 300, compounded_saved: 300 },
        ],
      },
      stats_available: true,
      optimization_tips: [],
    });
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    // Click session row to expand - find the one inside the table (has <tr> ancestor)
    const allExpander = screen.getAllByText('Expander');
    const row = allExpander.map(el => el.closest('tr')).find(tr => tr !== null);
    await act(async () => {
      fireEvent.click(row!);
    });
    expect(screen.getAllByText('工具输出').length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText('Schema 压缩').length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText('详情').length).toBeGreaterThanOrEqual(1);
    // Click "详情" to expand DiffView and verify compression_ratio renders
    const detailBtns = screen.getAllByText('详情');
    await act(async () => {
      fireEvent.click(detailBtns[0]);
    });
    // The explanation banner should show compression ratio text
    expect(screen.getByText(/75\.0%/)).toBeInTheDocument();
    // The explanation text itself
    expect(screen.getByText(/压缩率/)).toBeInTheDocument();
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
        strategy_breakdown: [],
      },
      stats_available: true,
      optimization_tips: [{ level: 'success', title: '节省效果优秀', description: '当前复合节省率 50.0%，表现优秀！继续保持当前配置。' }],
    });
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('优秀')).toBeInTheDocument();
  });

  it('should render optimization tips panel', async () => {
    mockFetchTokenSavings.mockResolvedValue({
      sessions: [],
      summary: {
        total_input_tokens: 5000,
        total_output_tokens: 3000,
        total_tokens: 8000,
        total_compounded_saved: 200,
        total_compounded_tool_saved: 200,
        total_compounded_mcp_saved: 0,
        compounded_savings_rate: 2.5,
      },
      stats_available: true,
      optimization_tips: [
        { level: 'warning', title: '节省率较低', description: '当前复合节省率不足 5%，建议检查 tokenless 配置。' },
        { level: 'info', title: '建议开启 MCP 响应压缩', description: '当前仅有工具输出优化，未检测到 MCP 响应压缩。开启后可进一步降低 Token 消耗。' },
      ],
    });
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('优化建议')).toBeInTheDocument();
    expect(screen.getByText('节省率较低')).toBeInTheDocument();
    expect(screen.getByText('建议开启 MCP 响应压缩')).toBeInTheDocument();
  });

  it('should render savings breakdown panel with top items', async () => {
    mockFetchTokenSavings.mockResolvedValue({
      sessions: [{
        session_id: 'sess-breakdown',
        agent_name: 'BreakdownAgent',
        total_input_tokens: 3000,
        total_output_tokens: 1500,
        saved_tokens: 800,
        compounded_saved: 800,
        compounded_savings_rate: 17.8,
        optimization_items: [
          {
            id: 'item-1',
            category: 'tool_output',
            before_tokens: 500,
            after_tokens: 100,
            saved_tokens: 400,
            compounded_saved: 800,
            compounding_turns: 2,
            compression_ratio: 80.0,
            explanation: '工具输出优化: 原始 500 tokens → 100 tokens，压缩率 80.0%。',
            before_text: 'original',
            after_text: 'compressed',
            diff_lines: [],
          },
        ],
      }],
      summary: {
        total_input_tokens: 3000,
        total_output_tokens: 1500,
        total_tokens: 4500,
        total_compounded_saved: 800,
        total_compounded_tool_saved: 800,
        total_compounded_mcp_saved: 0,
        compounded_savings_rate: 17.8,
      },
      stats_available: true,
      optimization_tips: [],
    });
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(screen.getByText('节省排行 Top 5（按复合节省量）')).toBeInTheDocument();
    expect(screen.getAllByText('BreakdownAgent').length).toBeGreaterThanOrEqual(1);
  });
});
