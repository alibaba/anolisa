import React from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';

vi.mock('../utils/apiClient', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../utils/apiClient')>();
  return {
    ...actual,
    containSecurityCase: vi.fn(),
    fetchContainmentPlan: vi.fn(),
    fetchSecurityCase: vi.fn(),
    fetchSecurityCases: vi.fn(),
    fetchSecurityEvents: vi.fn(),
    fetchSecuritySessions: vi.fn(),
    fetchSecuritySummary: vi.fn(),
    reviewSecurityCase: vi.fn(),
  };
});

import {
  containSecurityCase,
  fetchContainmentPlan,
  fetchSecurityCase,
  fetchSecurityCases,
  fetchSecurityEvents,
  fetchSecuritySessions,
  fetchSecuritySummary,
  reviewSecurityCase,
  type SecurityContainmentAction,
  type SecurityRiskCase,
  type SecurityRiskCaseDetail,
} from '../utils/apiClient';
import { SystemAuditPage } from '../pages/SystemAuditPage';

const caseSummary: SecurityRiskCase = {
  case_id: 'case-1',
  policy_id: 'credential-exfiltration',
  policy_revision: 3,
  agent_id: 'hermes-test',
  session_id: 'session-1',
  severity: 'high',
  risk_score: 85,
  status: 'open',
  blocked: false,
  opened_at_ns: 1_720_000_000_000_000_000,
  updated_at_ns: 1_720_000_001_000_000_000,
  summary: '疑似凭据外传',
};

const caseDetail: SecurityRiskCaseDetail = {
  ...caseSummary,
  evidence: [
    { event_id: 'event-1', event_type: 'file_action', occurred_at_ns: 1, identity: { pid: 42 }, event: { path: '~/.ssh/id_rsa' } },
    { event_id: 'event-2', event_type: 'taint_transition', occurred_at_ns: 2, identity: { pid: 42 }, event: { label: 'CREDENTIAL' } },
    { event_id: 'event-3', event_type: 'network_action', occurred_at_ns: 3, identity: { pid: 42 }, event: { destination: '198.51.100.10:443' } },
    { event_id: 'event-4', event_type: 'policy_decision', occurred_at_ns: 4, identity: { pid: 42 }, event: { mode: 'audit', blocked: false } },
  ],
};

const activeAction: SecurityContainmentAction = {
  action_id: 'action-1',
  case_id: 'case-1',
  binding_id: 'binding-1',
  agent_id: 'hermes-test',
  root_pid: 42,
  process_start_time: 100,
  duration_secs: 900,
  expires_at_ns: (Date.now() + 900_000) * 1_000_000,
  lifecycle_state: 'active',
  blocked_at_ns: null,
  requested_by: 'dashboard',
  failure_stage: null,
  attempt_count: 1,
  next_retry_at_ns: null,
  created_at_ns: 1_720_000_002_000_000_000,
  updated_at_ns: 1_720_000_003_000_000_000,
};

function mockPlan(existingAction: SecurityContainmentAction | null = null) {
  vi.mocked(fetchContainmentPlan).mockResolvedValue({
    state: 'found',
    data: {
      case_id: 'case-1',
      original_target: {
        agent_id: 'hermes-test',
        root_pid: 42,
        process_start_time: 100,
        display_name: 'Hermes',
      },
      original_target_valid: true,
      candidates: [],
      default_duration_secs: 900,
      min_duration_secs: 60,
      max_duration_secs: 86400,
      existing_action: existingAction,
    },
  });
}

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
  vi.mocked(fetchSecuritySessions).mockReset().mockResolvedValue({
    state: 'ok', data: { items: [], total: 0, limit: 100, offset: 0 },
  });
  vi.mocked(fetchSecurityEvents).mockReset().mockResolvedValue({
    state: 'ok', data: { items: [], total: 0, limit: 100, offset: 0 },
  });
  vi.mocked(fetchSecurityCase).mockReset().mockResolvedValue({ state: 'found', data: caseDetail });
  vi.mocked(reviewSecurityCase).mockReset().mockResolvedValue({
    state: 'updated',
    data: { ...caseSummary, status: 'confirmed' },
  });
  vi.mocked(containSecurityCase).mockReset().mockResolvedValue({
    state: 'policy_active', data: activeAction,
  });
  vi.mocked(fetchContainmentPlan).mockReset();
  mockPlan();
});

function renderPage() {
  return render(<MemoryRouter><SystemAuditPage /></MemoryRouter>);
}

async function selectCase() {
  fireEvent.click(await screen.findByText('疑似凭据外传'));
  await screen.findByText('完整证据链');
}

describe('SystemAuditPage', () => {
  it('shows the audit overview and ordered evidence chain', async () => {
    renderPage();
    await selectCase();

    expect(await screen.findByText('文件读取')).toBeInTheDocument();
    expect(screen.getByText('标签传递')).toBeInTheDocument();
    expect(screen.getByText('网络连接')).toBeInTheDocument();
    expect(screen.getByText('策略判定')).toBeInTheDocument();
    expect(screen.getByText(/规则命中，已放行/)).toBeInTheDocument();
  });

  it('opens containment from an eligible audit case', async () => {
    renderPage();
    await selectCase();

    fireEvent.click(await screen.findByRole('button', { name: '升级为拦截' }));
    expect(await screen.findByRole('dialog', { name: '确认升级为内核拦截' })).toBeInTheDocument();
  });

  it('does not offer containment for an ineligible case', async () => {
    vi.mocked(fetchSecurityCase).mockResolvedValue({
      state: 'found',
      data: { ...caseDetail, severity: 'medium' },
    });
    renderPage();
    await selectCase();

    expect(screen.queryByRole('button', { name: '升级为拦截' })).not.toBeInTheDocument();
    expect(fetchContainmentPlan).not.toHaveBeenCalled();
  });

  it('shows active as waiting for a block acknowledgement', async () => {
    mockPlan(activeAction);
    renderPage();
    await selectCase();

    expect(await screen.findByText('策略生效')).toBeInTheDocument();
    expect(screen.getByText('等待首次内核阻断')).toBeInTheDocument();
    expect(screen.queryByText('已遏制')).not.toBeInTheDocument();
    expect(screen.getByText('剩余时间')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: '标记已处置' })).toHaveLength(1);
  });

  it('shows contained only after blocked_at_ns exists', async () => {
    mockPlan({ ...activeAction, blocked_at_ns: 1_720_000_004_000_000_000 });
    renderPage();
    await selectCase();

    expect(await screen.findByText('已遏制')).toBeInTheDocument();
    expect(screen.getByText(/首次阻断/)).toBeInTheDocument();
  });

  it('shows failure stage without exposing an unavailable raw reason', async () => {
    mockPlan({
      ...activeAction,
      lifecycle_state: 'failed',
      failure_stage: 'attach',
      expires_at_ns: null,
    });
    renderPage();
    await selectCase();

    expect(await screen.findByText('执行失败')).toBeInTheDocument();
    expect(screen.getByText('策略挂载')).toBeInTheDocument();
    expect(screen.queryByText(/failure_reason/)).not.toBeInTheDocument();
  });

  it('shows expired containment as retryable', async () => {
    mockPlan({
      ...activeAction,
      lifecycle_state: 'expired',
      expires_at_ns: Date.now() * 1_000_000,
    });
    renderPage();
    await selectCase();

    expect(await screen.findByText('已到期')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '重新下发拦截' })).toBeInTheDocument();
  });

  it('uses a safe error state when the containment summary cannot load', async () => {
    vi.mocked(fetchContainmentPlan).mockRejectedValue(new Error('private backend detail'));
    renderPage();
    await selectCase();

    expect(await screen.findByText('拦截状态暂时不可用，请刷新后重试。')).toBeInTheDocument();
    expect(screen.queryByText('private backend detail')).not.toBeInTheDocument();
  });

  it('refreshes list, detail, and containment summary after success', async () => {
    renderPage();
    await selectCase();
    fireEvent.click(await screen.findByRole('button', { name: '升级为拦截' }));
    fireEvent.click(await screen.findByRole('button', { name: '确认并下发' }));

    await waitFor(() => expect(containSecurityCase).toHaveBeenCalledWith('case-1', {
      root_pid: 42,
      duration_secs: 900,
    }));
    await waitFor(() => {
      expect(vi.mocked(fetchSecurityCases).mock.calls.length).toBeGreaterThanOrEqual(2);
      expect(vi.mocked(fetchSecurityCase).mock.calls.length).toBeGreaterThanOrEqual(2);
      expect(vi.mocked(fetchContainmentPlan).mock.calls.length).toBeGreaterThanOrEqual(3);
    });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('reviews a case without publishing a policy revision', async () => {
    renderPage();
    await selectCase();
    fireEvent.click(await screen.findByRole('button', { name: '确认风险' }));

    await waitFor(() => expect(reviewSecurityCase).toHaveBeenCalledWith('case-1', 'confirmed'));
    expect(await screen.findByText('已确认')).toBeInTheDocument();
  });
});
