import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Routes, Route } from 'react-router-dom';

vi.mock('../utils/apiClient', () => ({
  fetchOptimizeHistory: vi.fn(),
  fetchOptimizeResults: vi.fn(),
  runOptimizeDimension: vi.fn(),
  // 构造签名与真实 ApiRequestError 保持一致（url, status, text, body）
  ApiRequestError: class ApiRequestError extends Error {
    status: number;
    body: Record<string, unknown> | null;
    constructor(
      url: string,
      status: number,
      text: string,
      body: Record<string, unknown> | null
    ) {
      super(`API ${url} -> ${status}: ${text}`);
      this.name = 'ApiRequestError';
      this.status = status;
      this.body = body;
    }
  },
}));

import {
  fetchOptimizeHistory,
  fetchOptimizeResults,
  runOptimizeDimension,
  ApiRequestError,
} from '../utils/apiClient';
import { OptimizationPage } from '../pages/OptimizationPage';

const mockFetchHistory = fetchOptimizeHistory as ReturnType<typeof vi.fn>;
const mockFetchResults = fetchOptimizeResults as ReturnType<typeof vi.fn>;
const mockRunDimension = runOptimizeDimension as ReturnType<typeof vi.fn>;

const NOW_NS = Date.now() * 1_000_000;

const entry = (id: string, dimensions: string[] = ['perf', 'accuracy']) => ({
  session_id: id,
  dimensions,
  created_at_ns: NOW_NS - 3_600_000_000_000,
  updated_at_ns: NOW_NS,
});

/** 渲染 /optimization 入口页，跳转目标用可断言的占位路由暴露 */
function renderEntry() {
  return render(
    <MemoryRouter initialEntries={['/optimization']}>
      <Routes>
        <Route path="/optimization" element={<OptimizationPage />} />
        <Route path="/optimization/:sessionId" element={<div>ANALYSIS ROUTE</div>} />
      </Routes>
    </MemoryRouter>
  );
}

describe('OptimizationPage 入口视图', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockFetchHistory.mockResolvedValue([]);
  });

  it('渲染会话 ID 输入框，空输入时禁用「开始分析」', async () => {
    renderEntry();
    expect(screen.getByLabelText('会话 ID')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '开始分析' })).toBeDisabled();
    await waitFor(() => expect(mockFetchHistory).toHaveBeenCalled());
  });

  it('输入 ID 后点击「开始分析」跳到分析路由', async () => {
    renderEntry();
    await screen.findByText('还没有分析记录');
    fireEvent.change(screen.getByLabelText('会话 ID'), { target: { value: 'sess-abc' } });
    fireEvent.click(screen.getByRole('button', { name: '开始分析' }));
    expect(screen.getByText('ANALYSIS ROUTE')).toBeInTheDocument();
  });

  it('回车同样触发分析，且首尾空格被裁掉', async () => {
    renderEntry();
    await screen.findByText('还没有分析记录');
    const input = screen.getByLabelText('会话 ID');
    fireEvent.change(input, { target: { value: '  sess-xyz  ' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(screen.getByText('ANALYSIS ROUTE')).toBeInTheDocument();
  });

  it('纯空格输入不触发跳转', async () => {
    renderEntry();
    await screen.findByText('还没有分析记录');
    const input = screen.getByLabelText('会话 ID');
    fireEvent.change(input, { target: { value: '   ' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(screen.queryByText('ANALYSIS ROUTE')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '开始分析' })).toBeDisabled();
  });

  it('无历史记录时展示空态', async () => {
    renderEntry();
    expect(await screen.findByText('还没有分析记录')).toBeInTheDocument();
  });

  it('渲染历史分析记录，维度显示为中文标签', async () => {
    mockFetchHistory.mockResolvedValue([
      entry('sess-1', ['perf', 'perf_issues']),
      entry('sess-2', ['cost_waste', 'accuracy']),
    ]);
    renderEntry();
    expect(await screen.findByText('sess-1')).toBeInTheDocument();
    expect(screen.getByText('sess-2')).toBeInTheDocument();
    expect(screen.getByText('共 2 条')).toBeInTheDocument();
    expect(screen.getByText('性能')).toBeInTheDocument();
    expect(screen.getByText('性能策略')).toBeInTheDocument();
    expect(screen.getByText('成本浪费')).toBeInTheDocument();
    expect(screen.getByText('准确性')).toBeInTheDocument();
  });

  it('点击历史记录行进入该会话的分析页', async () => {
    mockFetchHistory.mockResolvedValue([entry('sess-click')]);
    renderEntry();
    fireEvent.click(await screen.findByText('sess-click'));
    expect(screen.getByText('ANALYSIS ROUTE')).toBeInTheDocument();
  });

  it('历史记录加载失败时展示错误，不影响输入框可用', async () => {
    mockFetchHistory.mockRejectedValue(new Error('boom'));
    renderEntry();
    expect(await screen.findByText(/加载历史记录失败/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('会话 ID'), { target: { value: 'sess-ok' } });
    expect(screen.getByRole('button', { name: '开始分析' })).toBeEnabled();
  });
});

// ── 分析详情页：轨迹摘要面板 ──────────────────────────────────────────────────

const emptyResults = {
  session_id: 's',
  perf: null,
  perf_issues: null,
  cost: null,
  cost_waste: null,
  accuracy: null,
  summary: null,
};

/** 渲染 /optimization/:sessionId 分析详情页 */
function renderAnalysis() {
  return render(
    <MemoryRouter initialEntries={['/optimization/sess-1']}>
      <Routes>
        <Route path="/optimization/:sessionId" element={<OptimizationPage />} />
      </Routes>
    </MemoryRouter>
  );
}

describe('OptimizationPage 轨迹摘要面板', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockFetchHistory.mockResolvedValue([]);
    // 默认所有维度都挂起，避免 auto-trigger 打到真实实现
    mockRunDimension.mockImplementation(() => new Promise(() => {}));
  });

  it('缓存中已有摘要时直接渲染目标/过程/结果', async () => {
    mockFetchResults.mockResolvedValue({
      ...emptyResults,
      summary: { goal: '修复登录缺陷', process: ['定位中间件', '改配置'], outcome: '回归通过' },
    });
    renderAnalysis();
    expect(await screen.findByText('轨迹摘要')).toBeInTheDocument();
    expect(screen.getByText('修复登录缺陷')).toBeInTheDocument();
    expect(screen.getByText('定位中间件')).toBeInTheDocument();
    expect(screen.getByText('改配置')).toBeInTheDocument();
    expect(screen.getByText('回归通过')).toBeInTheDocument();
  });

  it('摘要维度进行中时显示生成提示', async () => {
    mockFetchResults.mockResolvedValue(emptyResults);
    renderAnalysis();
    expect(await screen.findByText('正在生成轨迹摘要…')).toBeInTheDocument();
  });

  it('LLM 未配置时整块面板不渲染', async () => {
    mockFetchResults.mockResolvedValue(emptyResults);
    const err = new ApiRequestError('/api/optimize', 400, 'bad request', {
      error: 'llm_not_configured',
    });
    mockRunDimension.mockRejectedValue(err);
    renderAnalysis();
    expect(await screen.findByText(/LLM 尚未配置/)).toBeInTheDocument();
    expect(screen.queryByText('轨迹摘要')).not.toBeInTheDocument();
    expect(screen.queryByText('正在生成轨迹摘要…')).not.toBeInTheDocument();
  });

  it('摘要返回空内容时不渲染空壳卡片', async () => {
    mockFetchResults.mockResolvedValue({
      ...emptyResults,
      summary: { goal: '', process: [], outcome: '' },
    });
    renderAnalysis();
    // 等缓存结果落地：此时 summary 维度已是 done，不会再触发生成
    await waitFor(() => expect(mockFetchResults).toHaveBeenCalled());
    await waitFor(() =>
      expect(screen.queryByText('正在生成轨迹摘要…')).not.toBeInTheDocument()
    );
    expect(screen.queryByText('轨迹摘要')).not.toBeInTheDocument();
  });
});
