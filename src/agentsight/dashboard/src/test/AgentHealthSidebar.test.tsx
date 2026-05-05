import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';

// Mock apiClient
vi.mock('../utils/apiClient', () => ({
  fetchAgentHealth: vi.fn(),
  deleteAgentHealth: vi.fn(),
  restartAgentHealth: vi.fn(),
}));

import { fetchAgentHealth, deleteAgentHealth, restartAgentHealth } from '../utils/apiClient';
import { AgentHealthSidebar } from '../components/AgentHealthSidebar';

const mockFetchAgentHealth = fetchAgentHealth as ReturnType<typeof vi.fn>;
const mockDeleteAgentHealth = deleteAgentHealth as ReturnType<typeof vi.fn>;
const mockRestartAgentHealth = restartAgentHealth as ReturnType<typeof vi.fn>;

beforeEach(() => {
  mockFetchAgentHealth.mockReset();
  mockDeleteAgentHealth.mockReset();
  mockRestartAgentHealth.mockReset();
});

describe('AgentHealthSidebar', () => {
  it('should show loading state initially', () => {
    mockFetchAgentHealth.mockReturnValue(new Promise(() => {})); // never resolves
    render(<AgentHealthSidebar />);
    expect(screen.getByText('加载中...')).toBeInTheDocument();
  });

  it('should show empty state when no agents found', async () => {
    mockFetchAgentHealth.mockResolvedValue({ agents: [], last_scan_time: 0 });
    await act(async () => {
      render(<AgentHealthSidebar />);
    });
    expect(screen.getByText('暂无已发现的 Agent')).toBeInTheDocument();
  });

  it('should show error state', async () => {
    mockFetchAgentHealth.mockRejectedValue(new Error('Network error'));
    await act(async () => {
      render(<AgentHealthSidebar />);
    });
    expect(screen.getByText('Network error')).toBeInTheDocument();
  });

  it('should render agent list with healthy agent', async () => {
    mockFetchAgentHealth.mockResolvedValue({
      agents: [{
        pid: 1234,
        agent_name: 'TestAgent',
        category: 'ai',
        exe_path: '/usr/bin/agent',
        ports: [8080],
        status: 'healthy',
        last_check_time: Date.now() - 3000,
        latency_ms: 42,
        error_message: null,
      }],
      last_scan_time: Date.now(),
    });
    await act(async () => {
      render(<AgentHealthSidebar />);
    });
    expect(screen.getByText('TestAgent')).toBeInTheDocument();
    expect(screen.getByText('正常')).toBeInTheDocument();
    expect(screen.getByText('PID 1234')).toBeInTheDocument();
    expect(screen.getByText('42ms')).toBeInTheDocument();
  });

  it('should render offline agent with delete button', async () => {
    mockFetchAgentHealth.mockResolvedValue({
      agents: [{
        pid: 5678,
        agent_name: 'OfflineAgent',
        category: 'ai',
        exe_path: '/bin/agent',
        ports: [],
        status: 'offline',
        last_check_time: Date.now() - 60000,
        latency_ms: null,
        error_message: 'Process exited',
      }],
      last_scan_time: Date.now(),
    });
    await act(async () => {
      render(<AgentHealthSidebar />);
    });
    expect(screen.getByText('OfflineAgent')).toBeInTheDocument();
    expect(screen.getByText('已下线')).toBeInTheDocument();
    expect(screen.getByText('确认下线并删除')).toBeInTheDocument();
  });

  it('should handle delete action', async () => {
    mockFetchAgentHealth.mockResolvedValue({
      agents: [{
        pid: 5678,
        agent_name: 'OfflineAgent',
        category: 'ai',
        exe_path: '/bin/agent',
        ports: [],
        status: 'offline',
        last_check_time: Date.now(),
        latency_ms: null,
        error_message: null,
      }],
      last_scan_time: Date.now(),
    });
    mockDeleteAgentHealth.mockResolvedValue(undefined);
    await act(async () => {
      render(<AgentHealthSidebar />);
    });
    await act(async () => {
      fireEvent.click(screen.getByText('确认下线并删除'));
    });
    expect(mockDeleteAgentHealth).toHaveBeenCalledWith(5678);
  });

  it('should render hung agent with restart button', async () => {
    mockFetchAgentHealth.mockResolvedValue({
      agents: [{
        pid: 9999,
        agent_name: 'HungAgent',
        category: 'ai',
        exe_path: '/bin/agent',
        ports: [3000],
        status: 'hung',
        last_check_time: Date.now(),
        latency_ms: null,
        error_message: 'Timeout exceeded',
        restart_cmd: ['node', 'agent.js'],
      }],
      last_scan_time: Date.now(),
    });
    await act(async () => {
      render(<AgentHealthSidebar />);
    });
    expect(screen.getByText('HungAgent')).toBeInTheDocument();
    expect(screen.getByText('卡顿')).toBeInTheDocument();
    expect(screen.getByText('重启进程')).toBeInTheDocument();
  });

  it('should handle restart action', async () => {
    mockFetchAgentHealth.mockResolvedValue({
      agents: [{
        pid: 9999,
        agent_name: 'HungAgent',
        category: 'ai',
        exe_path: '/bin/agent',
        ports: [3000],
        status: 'hung',
        last_check_time: Date.now(),
        latency_ms: null,
        error_message: 'Timeout',
        restart_cmd: ['node', 'agent.js'],
      }],
      last_scan_time: Date.now(),
    });
    mockRestartAgentHealth.mockResolvedValue({ ok: true, new_pid: 10000, cmd: ['node', 'agent.js'] });
    await act(async () => {
      render(<AgentHealthSidebar />);
    });
    await act(async () => {
      fireEvent.click(screen.getByText('重启进程'));
    });
    expect(mockRestartAgentHealth).toHaveBeenCalledWith(9999);
  });

  it('should show header with counts', async () => {
    mockFetchAgentHealth.mockResolvedValue({
      agents: [
        { pid: 1, agent_name: 'A', category: 'ai', exe_path: '', ports: [], status: 'healthy', last_check_time: Date.now(), latency_ms: 10, error_message: null },
        { pid: 2, agent_name: 'B', category: 'ai', exe_path: '', ports: [], status: 'offline', last_check_time: Date.now(), latency_ms: null, error_message: null },
      ],
      last_scan_time: Date.now(),
    });
    await act(async () => {
      render(<AgentHealthSidebar />);
    });
    expect(screen.getByText('Agent 状态')).toBeInTheDocument();
    expect(screen.getByText('1/2')).toBeInTheDocument();
    expect(screen.getByText('1 下线')).toBeInTheDocument();
  });
});
