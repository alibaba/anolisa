import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';

vi.mock('../utils/apiClient', async () => {
  const actual = await vi.importActual<any>('../utils/apiClient');
  return {
    ...actual,
    evaluateConversation: vi.fn(),
  };
});

import { EvaluationNotReadyError, evaluateConversation, EvaluationResult } from '../utils/apiClient';
import { EvaluationPanel } from '../components/EvaluationPanel';

const mockEvaluate = evaluateConversation as ReturnType<typeof vi.fn>;

const result: EvaluationResult = {
  target_type: 'conversation',
  target_id: 'conv-1',
  run_id: 'run-1',
  input_hash: 'hash-1',
  verdict: 'warn',
  score: 0.72,
  summary: 'Conversation is usable but needs review.',
  root_cause: 'partial_snapshot',
  recommended_action: 'Wait for pending calls to complete.',
  dimensions: [
    {
      name: 'completion',
      score: 0.85,
      verdict: 'pass',
      reason: 'A usable output exists.',
      evidence_refs: [
        {
          type: 'genai_event',
          id: 'call-1',
          label: 'Assistant output',
          target: {
            conversation_id: 'conv-1',
            call_id: 'call-1',
          },
          deeplink: {
            route: '/atif',
            query: {
              type: 'conversation',
              id: 'conv-1',
              highlight_call_id: 'call-1',
            },
          },
          metadata: null,
        },
      ],
    },
  ],
  findings: [
    {
      code: 'partial_snapshot',
      severity: 'medium',
      message: 'Evaluation was forced while calls were pending.',
      evidence_refs: [],
    },
  ],
  metadata: {
    evaluated_with_pending: true,
    pending_call_count: 1,
    input_event_count: 2,
    grader_type: 'rule',
    grader_version: 'rule-v1',
    rubric_version: null,
    judge_model: null,
    prompt_hash: null,
    confidence: null,
  },
};

beforeEach(() => {
  mockEvaluate.mockReset();
});

function renderPanel(ui: React.ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

describe('EvaluationPanel', () => {
  it('renders evaluate button when no result exists', () => {
    renderPanel(<EvaluationPanel conversationId="conv-1" initialResult={null} />);
    expect(screen.getByText('开始评估')).toBeInTheDocument();
  });

  it('renders compact summary and pending warning', () => {
    renderPanel(<EvaluationPanel conversationId="conv-1" initialResult={result} />);
    expect(screen.getByText('需复核')).toBeInTheDocument();
    expect(screen.getByText('72')).toBeInTheDocument();
    expect(screen.getByText('当前会话可用，但需要复核：快照未完成。')).toBeInTheDocument();
    expect(screen.getByText('评估时仍有 1 个 LLM 调用未完成。')).toBeInTheDocument();
    expect(screen.getByText('等待 pending 调用完成，或保留强制评估标记。')).toBeInTheDocument();
  });

  it('falls back for unknown root causes from newer backends', () => {
    renderPanel(
      <EvaluationPanel
        conversationId="conv-1"
        initialResult={{
          ...result,
          root_cause: 'provider_backoff' as any,
          recommended_action: 'Apply provider-specific backoff.',
        }}
      />
    );

    expect(screen.getByText('当前会话可用，但需要复核：provider_backoff。')).toBeInTheDocument();
    expect(screen.getByText('Apply provider-specific backoff.')).toBeInTheDocument();
  });

  it('reveals dimensions and findings', () => {
    renderPanel(<EvaluationPanel conversationId="conv-1" initialResult={result} />);
    fireEvent.click(screen.getByText('查看详情'));
    expect(screen.getByText('完成度')).toBeInTheDocument();
    expect(screen.getAllByText('快照未完成').length).toBeGreaterThanOrEqual(2);
    expect(screen.getByText('助手输出')).toBeInTheDocument();
  });

  it('runs evaluation and emits the new result', async () => {
    const onResult = vi.fn();
    mockEvaluate.mockResolvedValue({ result, reused_existing_run: false });
    renderPanel(<EvaluationPanel conversationId="conv-1" initialResult={null} onResult={onResult} />);

    fireEvent.click(screen.getByText('开始评估'));

    await waitFor(() => expect(mockEvaluate).toHaveBeenCalledWith('conv-1', false));
    await waitFor(() => expect(onResult).toHaveBeenCalledWith(result));
    expect(screen.getByText('需复核')).toBeInTheDocument();
  });

  it('shows force action after pending conflict', async () => {
    mockEvaluate
      .mockRejectedValueOnce(new EvaluationNotReadyError('pending', 2))
      .mockResolvedValueOnce({ result, reused_existing_run: false });
    renderPanel(<EvaluationPanel conversationId="conv-1" initialResult={null} />);

    fireEvent.click(screen.getByText('开始评估'));
    await waitFor(() => expect(screen.getByText(/2 个 LLM 调用仍未完成/)).toBeInTheDocument());

    fireEvent.click(screen.getByText('强制评估'));
    await waitFor(() => expect(mockEvaluate).toHaveBeenLastCalledWith('conv-1', true));
    expect(await screen.findByText('需复核')).toBeInTheDocument();
  });
});
