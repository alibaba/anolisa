import React from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';

vi.mock('../utils/apiClient', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../utils/apiClient')>();
  return {
    ...actual,
    fetchSecuritySummary: vi.fn(),
    fetchSecurityCases: vi.fn(),
    fetchSecurityCase: vi.fn(),
    reviewSecurityCase: vi.fn(),
  };
});

import {
  fetchSecurityCase,
  fetchSecurityCases,
  fetchSecuritySummary,
  reviewSecurityCase,
} from '../utils/apiClient';
import { SystemAuditPage } from '../pages/SystemAuditPage';

const caseSummary = {
  case_id: 'case-1',
  policy_id: 'credential-exfiltration',
  policy_revision: 3,
  agent_id: 'hermes-test',
  session_id: 'session-1',
  severity: 'high' as const,
  risk_score: 85,
  status: 'open' as const,
  blocked: false,
  opened_at_ns: 1_720_000_000_000_000_000,
  updated_at_ns: 1_720_000_001_000_000_000,
  summary: '疑似凭据外传',
};

beforeEach(() => {
  vi.mocked(fetchSecuritySummary).mockReset().mockResolvedValue({
    state: 'ok',
    data: {
      total: 4,
      by_category: { system: 4 },
      by_event_type: { file_action: 1, taint_transition: 1, network_action: 1, policy_decision: 1 },
      by_result: { allowed: 4 },
      affected_sessions: 1,
      affected_runs: 1,
      latest_events: [],
    },
  });
  vi.mocked(fetchSecurityCases).mockReset().mockResolvedValue({
    state: 'ok',
    data: { items: [caseSummary], total: 1, limit: 100, offset: 0 },
  });
  vi.mocked(fetchSecurityCase).mockReset().mockResolvedValue({
    state: 'found',
    data: {
      ...caseSummary,
      evidence: [
        { event_id: 'event-1', event_type: 'file_action', occurred_at_ns: 1, identity: { pid: 42 }, event: { path: '~/.ssh/id_rsa' } },
        { event_id: 'event-2', event_type: 'taint_transition', occurred_at_ns: 2, identity: { pid: 42 }, event: { label: 'CREDENTIAL' } },
        { event_id: 'event-3', event_type: 'network_action', occurred_at_ns: 3, identity: { pid: 42 }, event: { destination: '198.51.100.10:443' } },
        { event_id: 'event-4', event_type: 'policy_decision', occurred_at_ns: 4, identity: { pid: 42 }, event: { mode: 'audit', blocked: false } },
      ],
    },
  });
  vi.mocked(reviewSecurityCase).mockReset().mockResolvedValue({
    state: 'updated',
    data: { ...caseSummary, status: 'confirmed' },
  });
});

describe('SystemAuditPage', () => {
  it('shows the audit overview and ordered evidence chain', async () => {
    render(<MemoryRouter><SystemAuditPage /></MemoryRouter>);

    expect(await screen.findByText('疑似凭据外传')).toBeInTheDocument();
    fireEvent.click(screen.getByText('疑似凭据外传'));

    expect(await screen.findByText('文件读取')).toBeInTheDocument();
    expect(screen.getByText('标签传递')).toBeInTheDocument();
    expect(screen.getByText('网络连接')).toBeInTheDocument();
    expect(screen.getByText('策略判定')).toBeInTheDocument();
    expect(screen.getByText(/规则命中，已放行/)).toBeInTheDocument();
  });

  it('reviews a case without publishing a policy revision', async () => {
    render(<MemoryRouter><SystemAuditPage /></MemoryRouter>);
    fireEvent.click(await screen.findByText('疑似凭据外传'));
    fireEvent.click(await screen.findByRole('button', { name: '确认风险' }));

    await waitFor(() => expect(reviewSecurityCase).toHaveBeenCalledWith('case-1', 'confirmed'));
    expect(await screen.findByText('已确认')).toBeInTheDocument();
  });
});
