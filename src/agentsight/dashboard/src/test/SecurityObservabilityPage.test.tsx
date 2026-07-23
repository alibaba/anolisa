import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';

vi.mock('../utils/apiClient', () => ({
  SecurityApiClientError: class SecurityApiClientError extends Error {
    status = 503;
    code = 'daemon_unavailable';
    retryable = true;
  },
  fetchSecurityStatus: vi.fn(),
  fetchSecuritySummary: vi.fn(),
  fetchSecurityEvents: vi.fn(),
  fetchSecurityEvent: vi.fn(),
  fetchSecurityCountBy: vi.fn(),
  fetchSecuritySessions: vi.fn(),
  fetchSecurityRuns: vi.fn(),
  fetchSecurityTimeline: vi.fn(),
}));

import {
  fetchSecurityCountBy,
  fetchSecurityEvent,
  fetchSecurityEvents,
  fetchSecurityRuns,
  fetchSecuritySessions,
  fetchSecurityStatus,
  fetchSecuritySummary,
  fetchSecurityTimeline,
} from '../utils/apiClient';
import { SecurityObservabilityPage } from '../pages/SecurityObservabilityPage';

const mockFetchSecurityStatus = fetchSecurityStatus as ReturnType<typeof vi.fn>;
const mockFetchSecuritySummary = fetchSecuritySummary as ReturnType<typeof vi.fn>;
const mockFetchSecurityEvents = fetchSecurityEvents as ReturnType<typeof vi.fn>;
const mockFetchSecurityEvent = fetchSecurityEvent as ReturnType<typeof vi.fn>;
const mockFetchSecurityCountBy = fetchSecurityCountBy as ReturnType<typeof vi.fn>;
const mockFetchSecuritySessions = fetchSecuritySessions as ReturnType<typeof vi.fn>;
const mockFetchSecurityRuns = fetchSecurityRuns as ReturnType<typeof vi.fn>;
const mockFetchSecurityTimeline = fetchSecurityTimeline as ReturnType<typeof vi.fn>;

const event = {
  event_id: 'event-1',
  event_type: 'tool_output_leak',
  category: 'tool_output',
  result: 'succeeded',
  timestamp: '2026-06-09T00:00:00+00:00',
  session_id: 'session-1',
  run_id: 'run-1',
  tool_call_id: 'tool-1',
  pid: 1234,
  details: {
    verdict: 'deny',
    error_message: 'blocked secret output',
    finding: 'secret',
  },
};

const warningEvent = {
  ...event,
  event_id: 'event-2',
  tool_call_id: 'tool-2',
  details: {
    verdict: 'warning',
    reason: 'needs review',
  },
};

const defaultSecurityCountItems = (groupBy: string) => {
  if (groupBy === 'category') return [{ value: 'tool_output', count: 8 }];
  if (groupBy === 'result') return [{ value: 'succeeded', count: 12 }];
  if (groupBy === 'verdict') return [{ value: 'deny', count: 1 }, { value: 'warning', count: 1 }];
  return [{ value: 'tool_output_leak', count: 8 }];
};

beforeEach(() => {
  vi.clearAllMocks();
  mockFetchSecurityStatus.mockResolvedValue({
    state: 'daemon_reachable',
    data: {
      socket_path: '/tmp/agent-sec.sock',
      stores: {
        security_db: { ready: true, path: '/tmp/security-events.db' },
        observability_db: { ready: true, path: '/tmp/observability.db' },
      },
    },
  });
  mockFetchSecuritySummary.mockResolvedValue({
    state: 'ok',
    data: {
      total: 12,
      by_category: { tool_output: 8, prompt_scan: 4 },
      by_event_type: { tool_output_leak: 8 },
      by_result: { succeeded: 12 },
      affected_sessions: 3,
      affected_runs: 5,
      latest_events: [event],
    },
  });
  mockFetchSecurityCountBy.mockImplementation((groupBy: string) => Promise.resolve({
    state: 'ok',
    data: {
      group_by: groupBy,
      items: defaultSecurityCountItems(groupBy),
    },
  }));
  mockFetchSecurityEvents.mockResolvedValue({
    state: 'ok',
    data: { items: [event, warningEvent], total: 2, limit: 25, offset: 0, next_offset: null },
  });
  mockFetchSecurityEvent.mockResolvedValue({
    state: 'found',
    data: { found: true, event },
  });
  mockFetchSecuritySessions.mockResolvedValue({
    state: 'ok',
    data: {
      items: [{
        session_id: 'session-1',
        first_seen_epoch: 1780963200,
        last_seen_epoch: 1780963300,
        turn_count: 2,
        observability_event_count: 2,
        security_event_count: 1,
      }],
      total: 1,
      limit: 100,
      offset: 0,
      next_offset: null,
    },
  });
  mockFetchSecurityRuns.mockResolvedValue({
    state: 'ok',
    data: {
      items: [{
        run_id: 'run-1',
        started_at_epoch: 1780963200,
        ended_at_epoch: 1780963300,
        user_input_preview: 'inspect coverage',
        observability_event_count: 2,
        security_event_count: 1,
      }],
      total: 1,
      limit: 100,
      offset: 0,
      next_offset: null,
    },
  });
  mockFetchSecurityTimeline.mockResolvedValue({
    state: 'ok',
    data: {
      session_id: 'session-1',
      run_id: 'run-1',
      items: [
        {
          kind: 'observability',
          id: 1,
          hook: 'before_tool_call',
          timestamp_epoch: 1780963200,
          session_id: 'session-1',
          run_id: 'run-1',
          tool_call_id: 'tool-1',
        },
        {
          kind: 'security',
          timestamp_epoch: 1780963201,
          observability_event_id: 1,
          event,
          match: { reason: 'tool_call_id' },
        },
      ],
    },
  });
});

describe('SecurityObservabilityPage', () => {
  it('shows unavailable status without loading data APIs', async () => {
    mockFetchSecurityStatus.mockResolvedValueOnce({
      state: 'daemon_unreachable',
      data: {},
      message: 'agent-sec daemon is unavailable',
    });

    render(<SecurityObservabilityPage />);

    expect((await screen.findAllByText('daemon 不可达')).length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText('agent-sec daemon is unavailable')).toBeInTheDocument();
    expect(mockFetchSecuritySummary).not.toHaveBeenCalled();
    expect(mockFetchSecurityEvents).not.toHaveBeenCalled();
  });

  it('loads overview data when daemon status is reachable', async () => {
    render(<SecurityObservabilityPage />);

    await waitFor(() => expect(mockFetchSecuritySummary).toHaveBeenCalled());
    expect(mockFetchSecurityEvents).toHaveBeenCalledWith(expect.objectContaining({
      include_details: true,
      limit: 500,
    }));
    expect(screen.getAllByText('12').length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText('安全能力结论')).toBeInTheDocument();
    expect(screen.getByText('风险操作占比')).toBeInTheDocument();
    expect(screen.getByText('按 Verdict 聚类')).toBeInTheDocument();
    expect(screen.getByText('安全能力做了什么')).toBeInTheDocument();
    expect(screen.getByText('2 / 2 个 verdict 非 pass')).toBeInTheDocument();
    expect(screen.getByText('存在 1 个风险 verdict')).toBeInTheDocument();
    expect(screen.getAllByText('tool_output').length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText('tool_output_leak').length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText('时间')).toBeInTheDocument();
    expect(screen.getByText('Category')).toBeInTheDocument();
    expect(screen.getByText('Event Type')).toBeInTheDocument();
    expect(screen.getByText('Result')).toBeInTheDocument();
    expect(screen.getByText('Verdict')).toBeInTheDocument();
    expect(screen.getAllByText('succeeded').length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText('deny').length).toBeGreaterThanOrEqual(1);
    expect(screen.queryByText('正常')).not.toBeInTheDocument();
    expect(screen.queryByText('近期事件')).not.toBeInTheDocument();
    expect(screen.queryByText('latest events')).not.toBeInTheDocument();
  });

  it('uses summary verdicts when the detailed event sample is unavailable', async () => {
    const summaryEvent = {
      event_id: 'skill-ledger-show',
      event_type: 'skill_ledger_show',
      category: 'skill_ledger',
      result: 'succeeded',
      verdict: 'tampered',
      command: 'show',
      skill_name: 'demo-skill',
      timestamp: '2026-06-09T00:00:00+00:00',
    };
    mockFetchSecuritySummary.mockResolvedValueOnce({
      state: 'ok',
      data: {
        total: 3,
        by_category: { skill_ledger: 3 },
        by_event_type: { skill_ledger_show: 3 },
        by_result: { succeeded: 3 },
        affected_sessions: 0,
        affected_runs: 0,
        latest_events: [summaryEvent],
      },
    });
    mockFetchSecurityCountBy.mockImplementation((groupBy: string) => Promise.resolve({
      state: 'ok',
      data: {
        group_by: groupBy,
        items: groupBy === 'verdict'
          ? [
            { value: 'tampered', count: 1 },
            { value: 'drifted', count: 1 },
            { value: 'unmanaged', count: 1 },
          ]
          : defaultSecurityCountItems(groupBy),
      },
    }));
    mockFetchSecurityEvents.mockRejectedValueOnce(new Error('recent events unavailable'));

    render(<SecurityObservabilityPage />);

    const recentEventsHeading = await screen.findByText('近期安全事件');
    const recentEventsCard = recentEventsHeading.closest<HTMLElement>('.rounded-lg');
    expect(recentEventsCard).not.toBeNull();
    await waitFor(() => {
      expect(within(recentEventsCard!).getByText('tampered')).toHaveClass('bg-red-100');
    });
    expect(await screen.findByText('存在 1 个风险 verdict')).toBeInTheDocument();
    expect(screen.getByText('风险 1 / Warning 2')).toBeInTheDocument();
    for (const verdict of ['drifted', 'unmanaged']) {
      const warningBadge = screen.getAllByText(verdict)
        .find((element) => element.classList.contains('bg-amber-100'));
      expect(warningBadge).toBeDefined();
    }
  });

  it('uses exact verdict counts instead of the overview event sample', async () => {
    mockFetchSecurityCountBy.mockImplementation((groupBy: string) => Promise.resolve({
      state: 'ok',
      data: {
        group_by: groupBy,
        items: groupBy === 'verdict'
          ? [{ value: 'deny', count: 58 }, { value: 'pass', count: 120 }]
          : defaultSecurityCountItems(groupBy),
      },
    }));
    mockFetchSecurityEvents.mockResolvedValueOnce({
      state: 'ok',
      data: { items: [event], total: 500, limit: 500, offset: 0, next_offset: null },
    });

    render(<SecurityObservabilityPage />);

    expect(await screen.findByText('58 / 178 个 verdict 非 pass')).toBeInTheDocument();
    expect(screen.getByText('存在 58 个风险 verdict')).toBeInTheDocument();
    expect(screen.getByText('基于当前时间范围内 178 条含 verdict 事件统计')).toBeInTheDocument();
  });

  it('opens event detail drawer from the events tab', async () => {
    render(<SecurityObservabilityPage />);

    fireEvent.click(await screen.findByRole('button', { name: '安全事件' }));
    await waitFor(() => expect(mockFetchSecurityEvents).toHaveBeenCalledTimes(2));
    expect(screen.getByLabelText('Category')).toBeInTheDocument();
    expect(screen.getByLabelText('Result')).toBeInTheDocument();
    expect(screen.queryByLabelText('Event Type')).not.toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'tool_output' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'succeeded' })).toBeInTheDocument();
    expect(screen.getAllByText('Verdict').length).toBeGreaterThanOrEqual(2);
    expect(screen.queryByText('Event Type')).not.toBeInTheDocument();
    expect(screen.queryByText('PID')).not.toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'deny' })).toBeInTheDocument();
    expect(screen.getAllByText('succeeded').length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText('deny').length).toBeGreaterThanOrEqual(1);
    const warningBadge = screen.getAllByText('warning')
      .find((element) => element.tagName.toLowerCase() === 'span');
    expect(warningBadge).toBeDefined();
    expect(warningBadge!).toHaveClass('bg-amber-100');

    fireEvent.change(screen.getByLabelText('Category'), { target: { value: 'tool_output' } });
    fireEvent.change(screen.getByLabelText('Result'), { target: { value: 'succeeded' } });
    fireEvent.click(screen.getByRole('button', { name: '查询' }));
    await waitFor(() => expect(mockFetchSecurityEvents).toHaveBeenCalledTimes(3));
    const latestCall = mockFetchSecurityEvents.mock.calls[mockFetchSecurityEvents.mock.calls.length - 1]?.[0];
    expect(latestCall).toEqual(expect.objectContaining({
      category: 'tool_output',
      result: 'succeeded',
      include_details: true,
    }));
    expect(latestCall).not.toHaveProperty('event_type');

    fireEvent.click(screen.getByText('tool-1'));

    expect(await screen.findByText('安全事件详情')).toBeInTheDocument();
    expect(mockFetchSecurityEvent).toHaveBeenCalledWith('event-1');
    expect(screen.getByText(/secret/)).toBeInTheDocument();
  });

  it('renders timeline with correlated security event items', async () => {
    render(<SecurityObservabilityPage />);

    fireEvent.click(await screen.findByRole('button', { name: '全链路事件' }));

    expect(await screen.findByText('before_tool_call')).toBeInTheDocument();
    expect(await screen.findByText('Session 总览')).toBeInTheDocument();
    expect(await screen.findByText('存在 2 个非 pass verdict')).toBeInTheDocument();
    expect(screen.getByText('tool_output_leak @ before_tool_call')).toBeInTheDocument();
    expect(screen.getByText('observability before_tool_call')).toBeInTheDocument();
    expect(await screen.findByText('match tool_call_id')).toBeInTheDocument();
    expect(screen.getByText('verdict')).toBeInTheDocument();
    expect(screen.getAllByText('deny').length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText('error')).toBeInTheDocument();
    expect(screen.getByText('blocked secret output')).toBeInTheDocument();
    expect(screen.getByText('warning 1')).toHaveClass('bg-amber-100');
    expect(screen.getAllByText('session session-1').length).toBeGreaterThanOrEqual(2);
    expect(screen.getAllByText('run run-1').length).toBeGreaterThanOrEqual(2);
    expect(screen.getAllByText('tool tool-1').length).toBeGreaterThanOrEqual(2);
    fireEvent.click(screen.getByRole('button', { name: /查看安全事件详情 event-1/ }));
    expect(await screen.findByText('安全事件详情')).toBeInTheDocument();
    expect(mockFetchSecurityEvent).toHaveBeenCalledWith('event-1');
    expect(mockFetchSecurityRuns).toHaveBeenCalledWith('session-1', expect.objectContaining({ limit: 100 }));
    expect(mockFetchSecurityTimeline).toHaveBeenCalledWith(expect.objectContaining({
      session_id: 'session-1',
      run_id: 'run-1',
      include_security: true,
    }));
    expect(mockFetchSecurityEvents).toHaveBeenCalledWith(expect.objectContaining({
      session_id: 'session-1',
      include_details: true,
      limit: 500,
    }));
  });

  it('reloads the selected timeline when the date range changes', async () => {
    const { container } = render(<SecurityObservabilityPage />);

    fireEvent.click(await screen.findByRole('button', { name: '全链路事件' }));

    await waitFor(() => expect(mockFetchSecurityTimeline).toHaveBeenCalledTimes(1));
    const [startInput] = Array.from(container.querySelectorAll<HTMLInputElement>('input[type="datetime-local"]'));
    const nextStart = '2026-06-10T12:34';
    fireEvent.change(startInput, { target: { value: nextStart } });

    await waitFor(() => expect(mockFetchSecurityTimeline).toHaveBeenCalledTimes(2));
    expect(mockFetchSecurityTimeline).toHaveBeenLastCalledWith(expect.objectContaining({
      session_id: 'session-1',
      run_id: 'run-1',
      start_ns: new Date(nextStart).getTime() * 1_000_000,
      include_security: true,
    }));
  });

  it('refreshes timeline details when selected session and run remain selected', async () => {
    const sessionEventRequestCount = () => mockFetchSecurityEvents.mock.calls
      .filter(([params]) => params?.session_id === 'session-1')
      .length;

    render(<SecurityObservabilityPage />);

    fireEvent.click(await screen.findByRole('button', { name: '全链路事件' }));

    await waitFor(() => expect(mockFetchSecurityRuns).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(mockFetchSecurityTimeline).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(sessionEventRequestCount()).toBe(1));

    fireEvent.click(screen.getByRole('button', { name: '刷新' }));

    await waitFor(() => expect(mockFetchSecurityRuns).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(mockFetchSecurityTimeline).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(sessionEventRequestCount()).toBe(2));
  });

  it('displays error when status fetch fails', async () => {
    mockFetchSecurityStatus.mockRejectedValueOnce(new Error('Network error'));

    render(<SecurityObservabilityPage />);

    await waitFor(() => expect(screen.getByText('Network error')).toBeInTheDocument());
    expect(mockFetchSecuritySummary).not.toHaveBeenCalled();
  });

  it('shows partial error when some overview requests fail', async () => {
    mockFetchSecuritySummary.mockRejectedValueOnce(new Error('summary timeout'));
    mockFetchSecurityCountBy.mockRejectedValue(new Error('count-by failed'));

    render(<SecurityObservabilityPage />);

    await waitFor(() => expect(mockFetchSecuritySessions).toHaveBeenCalled());
    const errorElements = await screen.findAllByText(/summary timeout|count-by failed/);
    expect(errorElements.length).toBeGreaterThanOrEqual(1);
  });

  it('displays error when timeline runs fetch fails', async () => {
    mockFetchSecurityRuns.mockRejectedValueOnce(new Error('runs endpoint unavailable'));

    render(<SecurityObservabilityPage />);

    fireEvent.click(await screen.findByRole('button', { name: '全链路事件' }));

    await waitFor(() => expect(mockFetchSecurityRuns).toHaveBeenCalledTimes(1));
    expect(await screen.findByText('runs endpoint unavailable')).toBeInTheDocument();
    expect(mockFetchSecurityTimeline).not.toHaveBeenCalled();
  });
});
