import React from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

vi.mock('../utils/apiClient', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../utils/apiClient')>();
  return {
    ...actual,
    fetchEnforcementHealth: vi.fn(),
    fetchEnforcementBindings: vi.fn(),
    fetchEnforcementViolations: vi.fn(),
    createFileBinding: vi.fn(),
    detachEnforcementBinding: vi.fn(),
  };
});

import {
  EnforcementApiError,
  createFileBinding,
  detachEnforcementBinding,
  fetchEnforcementBindings,
  fetchEnforcementHealth,
  fetchEnforcementViolations,
  type EnforcementBinding,
  type EnforcementHealth,
  type EnforcementViolation,
} from '../utils/apiClient';
import { RiskEnforcementPage } from '../pages/RiskEnforcementPage';

const mockFetchEnforcementHealth = vi.mocked(fetchEnforcementHealth);
const mockFetchEnforcementBindings = vi.mocked(fetchEnforcementBindings);
const mockFetchEnforcementViolations = vi.mocked(fetchEnforcementViolations);
const mockCreateFileBinding = vi.mocked(createFileBinding);
const mockDetachEnforcementBinding = vi.mocked(detachEnforcementBinding);

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

const binding: EnforcementBinding = {
  request: {
    binding_id: 'binding-1',
    agent_id: 'qoder',
    session_id: null,
    root_pid: 45231,
    process_start_time: 99123,
    policy_id: 'agentsight-file-open:binding-1',
    policy_revision: 'v1',
    policy_dsl: [
      'source AGENT = exec "**"',
      'rule agentsight-file-open:',
      '  block open file "/root/.ssh/id_rsa" if AGENT',
      '  because "AgentSight sensitive file policy"',
    ].join('\n'),
  },
  state: 'enforced',
  message: null,
  domain_id: 7,
};

const violation: EnforcementViolation = {
  event_id: 'violation-1',
  binding_id: 'binding-1',
  agent_id: 'qoder',
  session_id: null,
  policy_id: 'agentsight-file-open:binding-1',
  policy_revision: 'v1',
  pid: 45244,
  ppid: 45231,
  process_start_time: 99124,
  operation: 'open',
  target: '/etc/shadow',
  effect: 'block',
  blocked: true,
  killed: false,
  rule_id: 'agentsight-file-open',
  reason: 'sensitive file policy',
  occurred_at_ns: 1_720_000_000_000_000_000,
  observed_at_ns: 1_720_000_000_100_000_000,
  actplane_revision: 'actplane-v1',
};

beforeEach(() => {
  vi.restoreAllMocks();
  mockFetchEnforcementHealth.mockReset();
  mockFetchEnforcementBindings.mockReset();
  mockFetchEnforcementViolations.mockReset();
  mockCreateFileBinding.mockReset();
  mockDetachEnforcementBinding.mockReset();

  mockFetchEnforcementHealth.mockResolvedValue({
    ready: true,
    backend: 'actplane',
    message: null,
  });
  mockFetchEnforcementBindings.mockResolvedValue({ bindings: [binding] });
  mockFetchEnforcementViolations.mockResolvedValue({ violations: [violation] });
  mockCreateFileBinding.mockResolvedValue(binding);
  mockDetachEnforcementBinding.mockResolvedValue(undefined);
});

describe('RiskEnforcementPage', () => {
  it('loads health bindings and violations independently', async () => {
    render(<RiskEnforcementPage />);

    expect(await screen.findByText('运行中')).toBeInTheDocument();
    expect(screen.getByText('/root/.ssh/id_rsa')).toBeInTheDocument();
    expect(screen.getByText('已拦截')).toBeInTheDocument();
  });

  it('keeps history visible when the enforcer is unavailable', async () => {
    mockFetchEnforcementHealth.mockRejectedValueOnce(
      new EnforcementApiError(503, 'enforcer_unavailable', 'socket unavailable', true),
    );

    render(<RiskEnforcementPage />);

    expect(await screen.findByText('socket unavailable')).toBeInTheDocument();
    expect(screen.getAllByText('不可用')).toHaveLength(2);
    expect(screen.getByText('/root/.ssh/id_rsa')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '下发策略' })).toBeDisabled();
  });

  it('shows the reason when health settles as unavailable', async () => {
    mockFetchEnforcementHealth.mockResolvedValueOnce({
      ready: false,
      backend: 'actplane',
      message: 'maintenance window',
    });

    render(<RiskEnforcementPage />);

    expect(await screen.findByText('maintenance window')).toBeInTheDocument();
    expect(screen.getAllByText('不可用')).toHaveLength(2);
    expect(screen.getByRole('button', { name: '下发策略' })).toBeDisabled();
  });

  it('creates a file binding and refreshes the console', async () => {
    render(<RiskEnforcementPage />);

    fireEvent.change(await screen.findByLabelText('Agent ID'), { target: { value: 'qoder' } });
    fireEvent.change(screen.getByLabelText('PID'), { target: { value: '45231' } });
    fireEvent.change(screen.getByLabelText('敏感文件'), {
      target: { value: '/root/.ssh/id_rsa' },
    });
    fireEvent.click(screen.getByRole('button', { name: '下发策略' }));

    await waitFor(() => expect(mockCreateFileBinding).toHaveBeenCalledWith({
      agent_id: 'qoder',
      root_pid: 45231,
      path: '/root/.ssh/id_rsa',
      session_id: undefined,
    }));
    expect(await screen.findByText('策略已生效')).toBeInTheDocument();
    expect(mockFetchEnforcementHealth).toHaveBeenCalledTimes(2);
    expect(screen.getByLabelText('Agent ID')).toHaveValue('');
  });

  it('preserves form values when binding creation fails', async () => {
    mockCreateFileBinding.mockRejectedValueOnce(
      new EnforcementApiError(400, 'invalid_file_binding', 'path must exist', false),
    );
    render(<RiskEnforcementPage />);

    fireEvent.change(await screen.findByLabelText('Agent ID'), { target: { value: 'qoder' } });
    fireEvent.change(screen.getByLabelText('PID'), { target: { value: '45231' } });
    fireEvent.change(screen.getByLabelText('敏感文件'), { target: { value: '/missing' } });
    fireEvent.click(screen.getByRole('button', { name: '下发策略' }));

    expect(await screen.findByText('path must exist')).toBeInTheDocument();
    expect(screen.getByLabelText('Agent ID')).toHaveValue('qoder');
    expect(screen.getByLabelText('敏感文件')).toHaveValue('/missing');
  });

  it('confirms before detaching a binding', async () => {
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    render(<RiskEnforcementPage />);

    fireEvent.click(await screen.findByRole('button', { name: '解除策略' }));

    await waitFor(() => expect(mockDetachEnforcementBinding).toHaveBeenCalledWith('binding-1'));
    expect(confirm).toHaveBeenCalled();
  });

  it('shows a placeholder when the file rule cannot be parsed', async () => {
    mockFetchEnforcementBindings.mockResolvedValueOnce({
      bindings: [{
        ...binding,
        request: { ...binding.request, policy_dsl: 'allow open file "**" if AGENT' },
      }],
    });

    render(<RiskEnforcementPage />);

    expect(await screen.findByLabelText('策略路径 binding-1')).toHaveTextContent('—');
  });

  it('renders nanosecond violation timestamps newest first', async () => {
    const older = {
      ...violation,
      event_id: 'violation-older',
      target: '/older',
      occurred_at_ns: 1_710_000_000_000_000_000,
    };
    const newer = {
      ...violation,
      event_id: 'violation-newer',
      target: '/newer',
      occurred_at_ns: 1_730_000_000_000_000_000,
    };
    mockFetchEnforcementViolations.mockResolvedValueOnce({ violations: [older, newer] });

    render(<RiskEnforcementPage />);

    const olderTarget = await screen.findByText('/older');
    const newerTarget = screen.getByText('/newer');
    expect(newerTarget.compareDocumentPosition(olderTarget) & Node.DOCUMENT_POSITION_FOLLOWING)
      .toBeTruthy();
    const formatted = new Intl.DateTimeFormat('zh-CN', {
      dateStyle: 'short',
      timeStyle: 'medium',
    }).format(newer.occurred_at_ns / 1_000_000);
    expect(screen.getByText(formatted)).toBeInTheDocument();
  });

  it('ignores a stale refresh that settles after a post-create reload', async () => {
    render(<RiskEnforcementPage />);
    await screen.findByText('运行中');

    const staleHealth = deferred<EnforcementHealth>();
    const staleBindings = deferred<{ bindings: EnforcementBinding[] }>();
    const staleViolations = deferred<{ violations: EnforcementViolation[] }>();
    const staleBinding = {
      ...binding,
      request: {
        ...binding.request,
        binding_id: 'binding-stale',
        policy_dsl: 'block open file "/stale" if AGENT',
      },
    };
    mockFetchEnforcementHealth
      .mockImplementationOnce(() => staleHealth.promise)
      .mockResolvedValueOnce({ ready: false, backend: 'latest-backend', message: null });
    mockFetchEnforcementBindings
      .mockImplementationOnce(() => staleBindings.promise)
      .mockResolvedValueOnce({ bindings: [] });
    mockFetchEnforcementViolations
      .mockImplementationOnce(() => staleViolations.promise)
      .mockResolvedValueOnce({ violations: [] });

    fireEvent.click(screen.getByRole('button', { name: '刷新' }));
    await waitFor(() => expect(mockFetchEnforcementHealth).toHaveBeenCalledTimes(2));
    fireEvent.change(screen.getByLabelText('Agent ID'), { target: { value: 'qoder' } });
    fireEvent.change(screen.getByLabelText('PID'), { target: { value: '45231' } });
    fireEvent.change(screen.getByLabelText('敏感文件'), { target: { value: '/root/.ssh/id_rsa' } });
    fireEvent.click(screen.getByRole('button', { name: '下发策略' }));

    expect(await screen.findByText('latest-backend')).toBeInTheDocument();
    await act(async () => {
      staleHealth.resolve({ ready: true, backend: 'stale-backend', message: null });
      staleBindings.resolve({ bindings: [staleBinding] });
      staleViolations.resolve({ violations: [violation] });
      await Promise.all([staleHealth.promise, staleBindings.promise, staleViolations.promise]);
    });

    expect(screen.getByText('latest-backend')).toBeInTheDocument();
    expect(screen.queryByText('stale-backend')).not.toBeInTheDocument();
    expect(screen.queryByText('/stale')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '下发策略' })).toBeDisabled();
  });
});
