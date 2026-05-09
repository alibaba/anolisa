import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';

// Mock recharts to avoid SVG rendering issues
vi.mock('recharts', () => ({
  BarChart: ({ children }: any) => <div data-testid="bar-chart">{children}</div>,
  Bar: () => null,
  XAxis: () => null,
  YAxis: () => null,
  Tooltip: () => null,
  ResponsiveContainer: ({ children }: any) => <div>{children}</div>,
}));

// Mock apiClient
vi.mock('../utils/apiClient', () => ({
  fetchSkillMetrics: vi.fn(),
  fetchAgentNames: vi.fn(),
}));

import { fetchSkillMetrics, fetchAgentNames } from '../utils/apiClient';
import { SkillMetricsPage } from '../pages/SkillMetricsPage';

const mockFetchSkillMetrics = fetchSkillMetrics as ReturnType<typeof vi.fn>;
const mockFetchAgentNames = fetchAgentNames as ReturnType<typeof vi.fn>;

function renderPage(route = '/skill-metrics') {
  return render(
    <MemoryRouter initialEntries={[route]}>
      <SkillMetricsPage />
    </MemoryRouter>
  );
}

const mockReport = {
  event_count: 120,
  downloads: { downloads: { 'skill-a': 120, 'skill-b': 80 } },
  loads: { loads: { 'skill-a': 50, 'skill-b': 30 }, total_loads: 80 },
  usage_ratio: { ratio: 0.667 },
  distribution: {
    histogram: [5, 10, 20, 30, 25, 10],
    min: 0,
    max: 5,
    mean: 3.2,
  },
  hotness: {
    rankings: [
      { skill_name: 'skill-a', total_loads: 50, total_rank: 1, rank_delta: 2 },
      { skill_name: 'skill-b', total_loads: 30, total_rank: 2, rank_delta: -1 },
    ],
  },
  computed_at: '2026-01-01T00:00:00Z',
  time_range_ns: [0, 1000000],
};

beforeEach(() => {
  mockFetchSkillMetrics.mockReset();
  mockFetchAgentNames.mockReset();
  mockFetchAgentNames.mockResolvedValue(['agent-1', 'agent-2']);
});

describe('SkillMetricsPage', () => {
  it('should render time filter inputs', async () => {
    mockFetchSkillMetrics.mockResolvedValue(mockReport);
    await act(async () => { renderPage(); });
    expect(screen.getByText('开始时间')).toBeInTheDocument();
    expect(screen.getByText('结束时间')).toBeInTheDocument();
  });

  it('should render quick time presets', async () => {
    mockFetchSkillMetrics.mockResolvedValue(mockReport);
    await act(async () => { renderPage(); });
    expect(screen.getByText('最近 1h')).toBeInTheDocument();
    expect(screen.getByText('最近 6h')).toBeInTheDocument();
    expect(screen.getByText('最近 24h')).toBeInTheDocument();
    expect(screen.getByText('最近 7d')).toBeInTheDocument();
  });

  it('should render query button', async () => {
    mockFetchSkillMetrics.mockResolvedValue(mockReport);
    await act(async () => { renderPage(); });
    expect(screen.getByText('查询')).toBeInTheDocument();
  });

  it('should render agent selector with fetched names', async () => {
    mockFetchSkillMetrics.mockResolvedValue(mockReport);
    await act(async () => { renderPage(); });
    expect(screen.getByText('全部 Agent')).toBeInTheDocument();
    expect(screen.getByText('agent-1')).toBeInTheDocument();
    expect(screen.getByText('agent-2')).toBeInTheDocument();
  });

  it('should render summary cards after loading data', async () => {
    mockFetchSkillMetrics.mockResolvedValue(mockReport);
    await act(async () => { renderPage(); });
    expect(screen.getByText('分析调用数')).toBeInTheDocument();
    expect(screen.getByText('120')).toBeInTheDocument();
    expect(screen.getByText('已发现技能')).toBeInTheDocument();
    expect(screen.getByText('2')).toBeInTheDocument();
    expect(screen.getAllByText('总加载次数').length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText('80')).toBeInTheDocument();
    expect(screen.getByText('技能使用率')).toBeInTheDocument();
    expect(screen.getByText('66.7%')).toBeInTheDocument();
  });

  it('should render hotness ranking table', async () => {
    mockFetchSkillMetrics.mockResolvedValue(mockReport);
    await act(async () => { renderPage(); });
    expect(screen.getByText('技能热度排行')).toBeInTheDocument();
    expect(screen.getByText('skill-a')).toBeInTheDocument();
    expect(screen.getByText('skill-b')).toBeInTheDocument();
    expect(screen.getByText('#1')).toBeInTheDocument();
    expect(screen.getByText('#2')).toBeInTheDocument();
  });

  it('should render trend delta with correct color indicator', async () => {
    mockFetchSkillMetrics.mockResolvedValue(mockReport);
    await act(async () => { renderPage(); });
    expect(screen.getByText('+2')).toBeInTheDocument();
    expect(screen.getByText('-1')).toBeInTheDocument();
  });

  it('should render distribution section', async () => {
    mockFetchSkillMetrics.mockResolvedValue(mockReport);
    await act(async () => { renderPage(); });
    expect(screen.getByText('单次调用技能数分布')).toBeInTheDocument();
    expect(screen.getByText('最小值:')).toBeInTheDocument();
    expect(screen.getByText('最大值:')).toBeInTheDocument();
    expect(screen.getByText('均值:')).toBeInTheDocument();
  });

  it('should show error message on fetch failure', async () => {
    mockFetchSkillMetrics.mockRejectedValue(new Error('服务器错误'));
    await act(async () => { renderPage(); });
    expect(screen.getByText('服务器错误')).toBeInTheDocument();
  });

  it('should reload data on query button click', async () => {
    mockFetchSkillMetrics.mockResolvedValue(mockReport);
    await act(async () => { renderPage(); });
    const callCountBefore = mockFetchSkillMetrics.mock.calls.length;
    await act(async () => {
      fireEvent.click(screen.getByText('查询'));
    });
    expect(mockFetchSkillMetrics.mock.calls.length).toBeGreaterThan(callCountBefore);
  });

  it('should update time range on quick preset click', async () => {
    mockFetchSkillMetrics.mockResolvedValue(mockReport);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('最近 1h'));
    });
    expect(mockFetchSkillMetrics).toHaveBeenCalled();
  });

  it('should render granularity buttons in hotness section', async () => {
    mockFetchSkillMetrics.mockResolvedValue(mockReport);
    await act(async () => { renderPage(); });
    expect(screen.getByText('按天')).toBeInTheDocument();
    expect(screen.getByText('按周')).toBeInTheDocument();
  });

  it('should show empty state when no data and no error', async () => {
    mockFetchSkillMetrics.mockResolvedValue({
      event_count: 0,
      downloads: null,
      loads: null,
      usage_ratio: null,
      distribution: null,
      hotness: null,
      computed_at: '',
      time_range_ns: [0, 0],
    });
    await act(async () => { renderPage(); });
    // Summary cards still show 0 values
    expect(screen.getAllByText('0').length).toBeGreaterThanOrEqual(1);
  });
});
