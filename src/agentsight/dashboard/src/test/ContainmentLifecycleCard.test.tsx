import React from 'react';
import { act, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ContainmentLifecycleCard } from '../components/ContainmentLifecycleCard';
import type { SecurityContainmentAction } from '../utils/apiClient';

const action: SecurityContainmentAction = {
  action_id: 'action-1',
  case_id: 'case-1',
  binding_id: 'binding-1',
  agent_id: 'hermes-test',
  root_pid: 42,
  process_start_time: 100,
  duration_secs: 2,
  expires_at_ns: 2_000_000_000,
  lifecycle_state: 'active',
  blocked_at_ns: null,
  requested_by: 'dashboard',
  failure_stage: null,
  failure_summary: null,
  attempt_count: 1,
  next_retry_at_ns: null,
  created_at_ns: 1,
  updated_at_ns: 1,
};

afterEach(() => {
  vi.useRealTimers();
});

describe('ContainmentLifecycleCard countdown', () => {
  it('stops scheduling updates when expiry is reached', () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    render(
      <ContainmentLifecycleCard
        action={action}
        loading={false}
        error={false}
        canUpgrade
        reviewing={false}
        onUpgrade={vi.fn()}
        onResolve={vi.fn()}
      />,
    );

    expect(vi.getTimerCount()).toBe(1);
    act(() => vi.advanceTimersByTime(2_000));
    expect(screen.getByText('等待状态刷新')).toBeInTheDocument();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('cleans up a pending countdown when replaced', () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const view = render(
      <ContainmentLifecycleCard
        action={action}
        loading={false}
        error={false}
        canUpgrade
        reviewing={false}
        onUpgrade={vi.fn()}
        onResolve={vi.fn()}
      />,
    );
    expect(vi.getTimerCount()).toBe(1);

    view.rerender(
      <ContainmentLifecycleCard
        action={{ ...action, expires_at_ns: null }}
        loading={false}
        error={false}
        canUpgrade
        reviewing={false}
        onUpgrade={vi.fn()}
        onResolve={vi.fn()}
      />,
    );
    expect(vi.getTimerCount()).toBe(0);
  });
});
