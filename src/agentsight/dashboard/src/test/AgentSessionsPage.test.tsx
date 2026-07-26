import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { MemoryRouter, Routes, Route } from 'react-router-dom';

// Mock apiClient
vi.mock('../utils/apiClient', () => ({
  fetchSessions: vi.fn(),
  fetchTrajectories: vi.fn(),
}));

import { fetchSessions, fetchTrajectories } from '../utils/apiClient';
import { AgentSessionsPage, mergeSessions } from '../pages/AgentSessionsPage';

const mockFetchSessions = fetchSessions as ReturnType<typeof vi.fn>;
const mockFetchTrajectories = fetchTrajectories as ReturnType<typeof vi.fn>;

const NOW_NS = Date.now() * 1_000_000;

const ebpfSession = (id: string, agent: string | null = 'claude') => ({
  session_id: id,
  conversation_count: 3,
  first_seen_ns: NOW_NS - 3_600_000_000_000,
  last_seen_ns: NOW_NS,
  total_input_tokens: 1200,
  total_output_tokens: 400,
  model: 'gpt-4o',
  agent_name: agent,
  first_user_query: '帮我修个登录 bug',
  last_user_query: '再跑一遍回归测试',
});

const logTrajectory = (id: string, agent = 'qoder') => ({
  session_id: id,
  schema_version: 'ATIF-v1.7',
  agent_name: agent,
  model_name: 'qwen-max',
  num_steps: 12,
  total_prompt_tokens: 800,
  total_completion_tokens: 200,
  start_time: null,
  end_time: new Date().toISOString(),
  first_user_message: '重构会话列表页面',
  last_user_message: '补充单元测试',
  project: 'my-project',
  source: 'qoder',
  is_subagent: false,
  collected_at_ns: NOW_NS,
});

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/sessions']}>
      <Routes>
        <Route path="/sessions" element={<AgentSessionsPage />} />
        <Route path="/optimization/:sessionId" element={<div>优化分析页面</div>} />
        <Route path="/atif" element={<div>轨迹查看页面</div>} />
      </Routes>
    </MemoryRouter>
  );
}

beforeEach(() => {
  mockFetchSessions.mockReset();
  mockFetchTrajectories.mockReset();
});

describe('mergeSessions', () => {
  it('merges both sources and dedups by session_id', () => {
    const merged = mergeSessions(
      [ebpfSession('s-shared'), ebpfSession('s-ebpf-only')] as any,
      [logTrajectory('s-shared'), logTrajectory('s-log-only')] as any,
    );
    expect(merged).toHaveLength(3);

    const shared = merged.find((s) => s.session_id === 's-shared')!;
    expect(shared.sources).toEqual(['ebpf', 'log']);
    // eBPF side wins for tokens/model; log side supplies the project
    expect(shared.model).toBe('gpt-4o');
    expect(shared.input_tokens).toBe(1200);
    expect(shared.project).toBe('my-project');
    // The log side carries the full trajectory — its message previews win
    expect(shared.first_message).toBe('重构会话列表页面');
    expect(shared.last_message).toBe('补充单元测试');

    const logOnly = merged.find((s) => s.session_id === 's-log-only')!;
    expect(logOnly.sources).toEqual(['log']);
    expect(logOnly.count).toBe(12);
    expect(logOnly.model).toBe('qwen-max');

    const ebpfOnly = merged.find((s) => s.session_id === 's-ebpf-only')!;
    expect(ebpfOnly.first_message).toBe('帮我修个登录 bug');
    expect(ebpfOnly.last_message).toBe('再跑一遍回归测试');
  });

  it('sorts by last activity descending', () => {
    const older = { ...logTrajectory('s-old'), end_time: null, collected_at_ns: NOW_NS - 9e15 };
    const merged = mergeSessions([ebpfSession('s-new')] as any, [older] as any);
    expect(merged[0].session_id).toBe('s-new');
    expect(merged[1].session_id).toBe('s-old');
  });

  it('hides subagent rows when their parent is present', () => {
    const parent = logTrajectory('s-parent');
    const sub1 = { ...logTrajectory('s-parent:subagent:aExplore-b4b7e9'), is_subagent: true };
    const sub2 = { ...logTrajectory('s-parent:subagent:aDebug-f1a2c3'), is_subagent: true };
    const merged = mergeSessions([] as any, [parent, sub1, sub2] as any);

    // Only the parent row remains; subagents are folded into it.
    expect(merged).toHaveLength(1);
    expect(merged[0].session_id).toBe('s-parent');
    expect(merged[0].subagent_count).toBe(2);
  });

  it('keeps orphaned subagent rows when parent is absent', () => {
    const orphan = { ...logTrajectory('s-gone:subagent:aExplore-abc123'), is_subagent: true };
    const merged = mergeSessions([] as any, [orphan] as any);

    expect(merged).toHaveLength(1);
    expect(merged[0].session_id).toBe('s-gone:subagent:aExplore-abc123');
  });
});

describe('AgentSessionsPage', () => {
  it('renders merged rows from both sources', async () => {
    mockFetchSessions.mockResolvedValue([ebpfSession('s-ebpf-1')]);
    mockFetchTrajectories.mockResolvedValue([logTrajectory('s-log-1')]);
    await act(async () => { renderPage(); });

    expect(screen.getByText('s-ebpf-1')).toBeInTheDocument();
    expect(screen.getByText('s-log-1')).toBeInTheDocument();
    expect(screen.getByText('my-project')).toBeInTheDocument();
    // Message preview columns from both sources
    expect(screen.getByText('帮我修个登录 bug')).toBeInTheDocument();
    expect(screen.getByText('重构会话列表页面')).toBeInTheDocument();
    expect(screen.getByText('补充单元测试')).toBeInTheDocument();
    // Total count = 2
    expect(screen.getByText('2')).toBeInTheDocument();
  });

  it('degrades gracefully when trajectory API is empty', async () => {
    mockFetchSessions.mockResolvedValue([ebpfSession('s-ebpf-1')]);
    mockFetchTrajectories.mockResolvedValue([]);
    await act(async () => { renderPage(); });

    expect(screen.getByText('s-ebpf-1')).toBeInTheDocument();
  });

  it('filters rows by source tab', async () => {
    mockFetchSessions.mockResolvedValue([ebpfSession('s-ebpf-1')]);
    mockFetchTrajectories.mockResolvedValue([logTrajectory('s-log-1')]);
    await act(async () => { renderPage(); });

    await act(async () => { fireEvent.click(screen.getByText('日志采集')); });
    expect(screen.queryByText('s-ebpf-1')).not.toBeInTheDocument();
    expect(screen.getByText('s-log-1')).toBeInTheDocument();

    await act(async () => { fireEvent.click(screen.getByText('eBPF 采集')); });
    expect(screen.getByText('s-ebpf-1')).toBeInTheDocument();
    expect(screen.queryByText('s-log-1')).not.toBeInTheDocument();
  });

  it('filters rows by search text', async () => {
    mockFetchSessions.mockResolvedValue([ebpfSession('s-ebpf-1')]);
    mockFetchTrajectories.mockResolvedValue([logTrajectory('s-log-1')]);
    await act(async () => { renderPage(); });

    const input = screen.getByPlaceholderText('搜索会话 ID / 项目 / 消息内容...');
    await act(async () => { fireEvent.change(input, { target: { value: 'my-project' } }); });
    expect(screen.getByText('s-log-1')).toBeInTheDocument();
    expect(screen.queryByText('s-ebpf-1')).not.toBeInTheDocument();

    // Message content is searchable too
    await act(async () => { fireEvent.change(input, { target: { value: '登录 bug' } }); });
    expect(screen.getByText('s-ebpf-1')).toBeInTheDocument();
    expect(screen.queryByText('s-log-1')).not.toBeInTheDocument();

    await act(async () => { fireEvent.change(input, { target: { value: 'no-such-thing' } }); });
    expect(screen.getByText('没有匹配的会话')).toBeInTheDocument();
  });

  it('filters rows by agent selector', async () => {
    mockFetchSessions.mockResolvedValue([ebpfSession('s-ebpf-1', 'claude')]);
    mockFetchTrajectories.mockResolvedValue([logTrajectory('s-log-1', 'qoder')]);
    await act(async () => { renderPage(); });

    const select = screen.getByDisplayValue('全部 Agent');
    await act(async () => { fireEvent.change(select, { target: { value: 'qoder' } }); });
    expect(screen.getByText('s-log-1')).toBeInTheDocument();
    expect(screen.queryByText('s-ebpf-1')).not.toBeInTheDocument();
  });

  it('dedups agent filter options case-insensitively and matches both casings', async () => {
    // eBPF reports "Qoder", the log collector writes "qoder" — same agent.
    mockFetchSessions.mockResolvedValue([ebpfSession('s-ebpf-1', 'Qoder')]);
    mockFetchTrajectories.mockResolvedValue([logTrajectory('s-log-1', 'qoder')]);
    await act(async () => { renderPage(); });

    // Only one dropdown option for the two casings
    const qoderOptions = screen
      .getAllByRole('option')
      .filter((o) => (o.textContent ?? '').toLowerCase() === 'qoder');
    expect(qoderOptions).toHaveLength(1);

    // Filtering by it matches rows from both sources regardless of casing
    const select = screen.getByDisplayValue('全部 Agent');
    const optionValue = (qoderOptions[0] as HTMLOptionElement).value;
    await act(async () => { fireEvent.change(select, { target: { value: optionValue } }); });
    expect(screen.getByText('s-ebpf-1')).toBeInTheDocument();
    expect(screen.getByText('s-log-1')).toBeInTheDocument();
  });

  it('navigates to optimization page on row action click', async () => {
    mockFetchSessions.mockResolvedValue([ebpfSession('s-ebpf-1')]);
    mockFetchTrajectories.mockResolvedValue([]);
    await act(async () => { renderPage(); });

    await act(async () => { fireEvent.click(screen.getByText('🔬 分析')); });
    expect(screen.getByText('优化分析页面')).toBeInTheDocument();
  });

  it('opens trajectory viewer in new tab on row click', async () => {
    const openSpy = vi.spyOn(window, 'open').mockImplementation(() => null);
    mockFetchSessions.mockResolvedValue([ebpfSession('s-ebpf-1')]);
    mockFetchTrajectories.mockResolvedValue([]);
    await act(async () => { renderPage(); });

    // Click on the session ID cell (part of the row) to trigger row navigation
    await act(async () => { fireEvent.click(screen.getByText('s-ebpf-1')); });
    expect(openSpy).toHaveBeenCalledWith('#/atif?type=session&id=s-ebpf-1', '_blank');
    openSpy.mockRestore();
  });

  it('shows error banner when eBPF session fetch fails', async () => {
    mockFetchSessions.mockRejectedValue(new Error('服务器错误'));
    mockFetchTrajectories.mockResolvedValue([]);
    await act(async () => { renderPage(); });

    expect(screen.getByText('服务器错误')).toBeInTheDocument();
  });

  it('shows empty state when both sources are empty', async () => {
    mockFetchSessions.mockResolvedValue([]);
    mockFetchTrajectories.mockResolvedValue([]);
    await act(async () => { renderPage(); });

    expect(screen.getByText('当前时间范围内没有会话数据')).toBeInTheDocument();
  });
});
