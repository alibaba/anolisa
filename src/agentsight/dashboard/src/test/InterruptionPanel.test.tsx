import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';

// Mock apiClient
vi.mock('../utils/apiClient', () => ({
  fetchSessionInterruptions: vi.fn(),
  fetchConversationInterruptions: vi.fn(),
  resolveInterruption: vi.fn(),
}));

import { fetchSessionInterruptions, fetchConversationInterruptions, resolveInterruption } from '../utils/apiClient';
import { InterruptionPanel } from '../components/InterruptionPanel';

const mockFetchSession = fetchSessionInterruptions as ReturnType<typeof vi.fn>;
const mockFetchConversation = fetchConversationInterruptions as ReturnType<typeof vi.fn>;
const mockResolve = resolveInterruption as ReturnType<typeof vi.fn>;

beforeEach(() => {
  mockFetchSession.mockReset();
  mockFetchConversation.mockReset();
  mockResolve.mockReset();
});

const sampleEvent = {
  interruption_id: 'int-1',
  session_id: 'sess-1',
  trace_id: 'trace-1',
  conversation_id: 'conv-1',
  call_id: 'call-1',
  pid: 1234,
  agent_name: 'TestAgent',
  interruption_type: 'llm_error',
  severity: 'high' as const,
  occurred_at_ns: Date.now() * 1_000_000,
  detail: '{"error": "rate_limited"}',
  resolved: false,
};

describe('InterruptionPanel', () => {
  it('should show loading state', () => {
    mockFetchSession.mockReturnValue(new Promise(() => {}));
    render(<InterruptionPanel sessionId="sess-1" />);
    expect(screen.getByText('Loading…')).toBeInTheDocument();
  });

  it('should show empty state', async () => {
    mockFetchSession.mockResolvedValue([]);
    render(<InterruptionPanel sessionId="sess-1" />);
    await waitFor(() => {
      expect(screen.getByText('No interruption events recorded for this session.')).toBeInTheDocument();
    });
  });

  it('should show error state', async () => {
    mockFetchSession.mockRejectedValue(new Error('API down'));
    render(<InterruptionPanel sessionId="sess-1" />);
    await waitFor(() => {
      expect(screen.getByText('API down')).toBeInTheDocument();
    });
  });

  it('should render events from session', async () => {
    mockFetchSession.mockResolvedValue([sampleEvent]);
    render(<InterruptionPanel sessionId="sess-1" />);
    await waitFor(() => {
      expect(screen.getByText('LLM Error')).toBeInTheDocument();
      expect(screen.getByText('1 条未处理')).toBeInTheDocument();
    });
  });

  it('should fetch by conversationId when provided', async () => {
    mockFetchConversation.mockResolvedValue([sampleEvent]);
    render(<InterruptionPanel conversationId="conv-1" />);
    await waitFor(() => {
      expect(mockFetchConversation).toHaveBeenCalledWith('conv-1');
      expect(screen.getByText('LLM Error')).toBeInTheDocument();
    });
  });

  it('should show close button when onClose provided', async () => {
    const onClose = vi.fn();
    mockFetchSession.mockResolvedValue([]);
    render(<InterruptionPanel sessionId="s1" onClose={onClose} />);
    await waitFor(() => screen.getByTitle('Close'));
    fireEvent.click(screen.getByTitle('Close'));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('should toggle detail view', async () => {
    mockFetchSession.mockResolvedValue([sampleEvent]);
    render(<InterruptionPanel sessionId="sess-1" />);
    await waitFor(() => screen.getByText('Detail'));
    fireEvent.click(screen.getByText('Detail'));
    // The detail JSON should be visible
    await waitFor(() => {
      expect(screen.getByText(/"error": "rate_limited"/)).toBeInTheDocument();
    });
  });

  it('should show call_id when present', async () => {
    mockFetchSession.mockResolvedValue([sampleEvent]);
    render(<InterruptionPanel sessionId="sess-1" />);
    await waitFor(() => {
      expect(screen.getByText('call: call-1')).toBeInTheDocument();
    });
  });

  it('should resolve event on confirm', async () => {
    mockFetchSession.mockResolvedValue([sampleEvent]);
    mockResolve.mockResolvedValue(undefined);
    window.confirm = vi.fn(() => true);
    const onResolved = vi.fn();
    render(<InterruptionPanel sessionId="sess-1" onResolvedEvent={onResolved} />);
    await waitFor(() => screen.getByText('Resolve'));
    await act(async () => {
      fireEvent.click(screen.getByText('Resolve'));
    });
    expect(mockResolve).toHaveBeenCalledWith('int-1');
    expect(onResolved).toHaveBeenCalled();
  });

  it('should not resolve when confirm is cancelled', async () => {
    mockFetchSession.mockResolvedValue([sampleEvent]);
    window.confirm = vi.fn(() => false);
    render(<InterruptionPanel sessionId="sess-1" />);
    await waitFor(() => screen.getByText('Resolve'));
    fireEvent.click(screen.getByText('Resolve'));
    expect(mockResolve).not.toHaveBeenCalled();
  });

  it('should show header title', async () => {
    mockFetchSession.mockResolvedValue([]);
    render(<InterruptionPanel sessionId="sess-1" />);
    expect(screen.getByText('Interruptions')).toBeInTheDocument();
  });
});
