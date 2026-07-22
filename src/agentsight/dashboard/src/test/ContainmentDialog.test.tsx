import React from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

vi.mock('../utils/apiClient', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../utils/apiClient')>();
  return {
    ...actual,
    fetchContainmentPlan: vi.fn(),
    containSecurityCase: vi.fn(),
  };
});

import {
  containSecurityCase,
  fetchContainmentPlan,
  SecurityApiClientError,
} from '../utils/apiClient';
import { ContainmentDialog } from '../components/ContainmentDialog';

const originalTarget = {
  agent_id: 'hermes-test',
  root_pid: 1832,
  process_start_time: 991,
  display_name: 'Hermes',
};

const plan = {
  case_id: 'case-1',
  original_target: originalTarget,
  original_target_valid: true,
  candidates: [originalTarget],
  default_duration_secs: 900,
  min_duration_secs: 60,
  max_duration_secs: 86400,
  existing_action: null,
};

const action = {
  action_id: 'action-1',
  case_id: 'case-1',
  binding_id: 'binding-1',
  agent_id: 'hermes-test',
  root_pid: 1832,
  process_start_time: 991,
  duration_secs: 900,
  expires_at_ns: 1_000_000,
  lifecycle_state: 'active' as const,
  blocked_at_ns: null,
  requested_by: 'dashboard',
  failure_stage: null,
  attempt_count: 0,
  next_retry_at_ns: null,
  created_at_ns: 1,
  updated_at_ns: 2,
};

beforeEach(() => {
  vi.mocked(fetchContainmentPlan).mockReset().mockResolvedValue({
    state: 'found',
    data: plan,
  });
  vi.mocked(containSecurityCase).mockReset().mockResolvedValue({
    state: 'policy_active',
    data: action,
  });
});

describe('ContainmentDialog', () => {
  it('defaults to a 15 minute temporary block and submits the valid PID', async () => {
    const onContained = vi.fn();
    render(
      <ContainmentDialog
        caseId="case-1"
        open
        onClose={vi.fn()}
        onContained={onContained}
      />,
    );

    expect(await screen.findByRole('dialog', { name: '确认升级为内核拦截' })).toBeInTheDocument();
    expect(screen.getByLabelText('临时拦截 15 分钟')).toBeChecked();
    expect(screen.getByText('由服务端案件策略安全恢复')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '确认并下发' }));

    await waitFor(() => expect(containSecurityCase).toHaveBeenCalledWith('case-1', {
      root_pid: 1832,
      duration_secs: 900,
    }));
    expect(onContained).toHaveBeenCalledWith(action);
  });

  it('requires a live Agent selection when the original PID is stale', async () => {
    vi.mocked(fetchContainmentPlan).mockResolvedValue({
      state: 'found',
      data: {
        ...plan,
        original_target_valid: false,
        candidates: [{ ...originalTarget, root_pid: 4242, process_start_time: 992 }],
      },
    });
    render(
      <ContainmentDialog caseId="case-1" open onClose={vi.fn()} onContained={vi.fn()} />,
    );

    const selector = await screen.findByLabelText('选择在线 Agent');
    expect(selector).toBeRequired();
    expect(screen.getByRole('button', { name: '确认并下发' })).toBeDisabled();

    fireEvent.change(selector, { target: { value: '4242' } });
    expect(screen.getByRole('button', { name: '确认并下发' })).toBeEnabled();
  });

  it('only submits a persistent block after the user selects it explicitly', async () => {
    render(
      <ContainmentDialog caseId="case-1" open onClose={vi.fn()} onContained={vi.fn()} />,
    );
    await screen.findByText('确认升级为内核拦截');

    fireEvent.click(screen.getByLabelText('持续拦截（需手动解除）'));
    fireEvent.click(screen.getByRole('button', { name: '确认并下发' }));

    await waitFor(() => expect(containSecurityCase).toHaveBeenCalledWith('case-1', {
      root_pid: 1832,
      duration_secs: null,
    }));
  });

  it.each([
    ['source_policy_unavailable', '原始策略来源已不可用，无法安全生成拦截规则。'],
    ['root_process_stale', '目标进程已变化，请刷新后选择在线 Agent。'],
  ])('maps %s to an actionable message without exposing server details', async (code, message) => {
    vi.mocked(containSecurityCase).mockRejectedValue(
      new SecurityApiClientError(409, {
        code,
        message: 'policy_dsl=/root/private should never be rendered',
        retryable: code === 'root_process_stale',
      }),
    );
    render(
      <ContainmentDialog caseId="case-1" open onClose={vi.fn()} onContained={vi.fn()} />,
    );
    await screen.findByText('确认升级为内核拦截');
    fireEvent.click(screen.getByRole('button', { name: '确认并下发' }));

    expect(await screen.findByText(message)).toBeInTheDocument();
    expect(screen.queryByText(/policy_dsl/)).not.toBeInTheDocument();
  });

  it('prevents duplicate submission while the request is pending', async () => {
    let resolveRequest: ((value: { state: string; data: typeof action }) => void) | undefined;
    vi.mocked(containSecurityCase).mockImplementation(() => new Promise((resolve) => {
      resolveRequest = resolve;
    }));
    render(
      <ContainmentDialog caseId="case-1" open onClose={vi.fn()} onContained={vi.fn()} />,
    );
    await screen.findByText('确认升级为内核拦截');

    const submit = screen.getByRole('button', { name: '确认并下发' });
    fireEvent.click(submit);
    expect(screen.getByRole('button', { name: '正在下发...' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: '正在下发...' }));
    expect(containSecurityCase).toHaveBeenCalledTimes(1);

    resolveRequest?.({ state: 'policy_active', data: action });
    await waitFor(() => expect(containSecurityCase).toHaveBeenCalledTimes(1));
  });

  it('discards stale loading results and resets choices when the case changes', async () => {
    let resolveFirst: ((value: { state: string; data: typeof plan }) => void) | undefined;
    vi.mocked(fetchContainmentPlan)
      .mockImplementationOnce(() => new Promise((resolve) => {
        resolveFirst = resolve;
      }))
      .mockResolvedValue({
        state: 'found',
        data: {
          ...plan,
          case_id: 'case-2',
          original_target: { ...originalTarget, root_pid: 4242 },
          candidates: [{ ...originalTarget, root_pid: 4242 }],
        },
      });
    const props = { open: true, onClose: vi.fn(), onContained: vi.fn() };
    const { rerender } = render(<ContainmentDialog caseId="case-1" {...props} />);
    rerender(<ContainmentDialog caseId="case-2" {...props} />);

    expect(await screen.findByText('Hermes · PID 4242')).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText('持续拦截（需手动解除）'));
    resolveFirst?.({ state: 'found', data: plan });
    await Promise.resolve();
    expect(screen.queryByText('Hermes · PID 1832')).not.toBeInTheDocument();

    rerender(<ContainmentDialog caseId="case-2" {...props} open={false} />);
    rerender(<ContainmentDialog caseId="case-2" {...props} open />);
    expect(await screen.findByLabelText('临时拦截 15 分钟')).toBeChecked();
  });
});
