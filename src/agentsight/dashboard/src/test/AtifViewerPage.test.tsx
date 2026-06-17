import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';

// Mock apiClient
vi.mock('../utils/apiClient', () => ({
  fetchAtifBySession: vi.fn(),
  fetchAtifByConversation: vi.fn(),
  fetchSessionSavings: vi.fn(),
}));

import { fetchAtifBySession, fetchAtifByConversation, fetchSessionSavings } from '../utils/apiClient';
import { AtifViewerPage } from '../pages/AtifViewerPage';

const mockFetchAtifBySession = fetchAtifBySession as ReturnType<typeof vi.fn>;
const mockFetchAtifByConversation = fetchAtifByConversation as ReturnType<typeof vi.fn>;
const mockFetchSessionSavings = fetchSessionSavings as ReturnType<typeof vi.fn>;

function renderPage(route = '/atif') {
  return render(
    <MemoryRouter initialEntries={[route]}>
      <AtifViewerPage />
    </MemoryRouter>
  );
}

const mockAtifDoc = {
  schema_version: 'ATIF-1.0',
  session_id: 'sess-atif-test-123456789',
  agent: {
    name: 'TestAgent',
    version: '1.0.0',
    framework: 'langchain',
  },
  steps: [
    {
      step_id: 1,
      source: 'user',
      message: 'Hello, how are you?',
      timestamp: '2024-01-01T10:00:00Z',
      model_name: null,
      reasoning_content: null,
      tool_calls: null,
      observation: null,
      metrics: null,
    },
    {
      step_id: 2,
      source: 'agent',
      message: 'I am fine, thank you!',
      timestamp: '2024-01-01T10:00:05Z',
      model_name: 'gpt-4o',
      reasoning_content: 'User is greeting, respond politely.',
      tool_calls: [
        {
          tool_call_id: 'tc-1',
          tool_name: 'search',
          arguments: { query: 'greeting' },
          result: 'found: hello',
        },
      ],
      observation: {
        results: [{ output: 'search result' }],
      },
      metrics: {
        prompt_tokens: 100,
        completion_tokens: 50,
        cached_tokens: 10,
      },
    },
  ],
  final_metrics: {
    total_steps: 2,
    total_prompt_tokens: 100,
    total_completion_tokens: 50,
    total_cached_tokens: 10,
  },
};

beforeEach(() => {
  mockFetchAtifBySession.mockReset();
  mockFetchAtifByConversation.mockReset();
  mockFetchSessionSavings.mockReset();
  mockFetchSessionSavings.mockRejectedValue(new Error('no savings'));
});

describe('AtifViewerPage', () => {
  it('should show empty state with instructions', async () => {
    await act(async () => { renderPage(); });
    expect(screen.getByText('ATIF 轨迹查看器')).toBeInTheDocument();
    expect(screen.getByText('请输入 Session 或 Conversation ID，然后点击「加载」')).toBeInTheDocument();
    expect(screen.getByText('或导入本地 ATIF JSON 文件')).toBeInTheDocument();
  });

  it('should show type toggle buttons', async () => {
    await act(async () => { renderPage(); });
    expect(screen.getByText('按 Session')).toBeInTheDocument();
    expect(screen.getByText('按 Conversation')).toBeInTheDocument();
  });

  it('should have load button disabled when no id entered', async () => {
    await act(async () => { renderPage(); });
    const loadBtn = screen.getByText('加载');
    expect(loadBtn).toBeDisabled();
  });

  it('should show loading state when fetching', async () => {
    mockFetchAtifBySession.mockReturnValue(new Promise(() => {}));
    await act(async () => { renderPage(); });
    const input = screen.getByPlaceholderText('输入 Session ID...');
    await act(async () => {
      fireEvent.change(input, { target: { value: 'sess-123' } });
    });
    await act(async () => {
      fireEvent.click(screen.getByText('加载'));
    });
    expect(screen.getAllByText('加载中...').length).toBeGreaterThanOrEqual(1);
  });

  it('should show error on fetch failure', async () => {
    mockFetchAtifBySession.mockRejectedValue(new Error('Not found'));
    await act(async () => { renderPage(); });
    const input = screen.getByPlaceholderText('输入 Session ID...');
    await act(async () => {
      fireEvent.change(input, { target: { value: 'sess-fail' } });
    });
    await act(async () => {
      fireEvent.click(screen.getByText('加载'));
    });
    expect(screen.getByText(/Not found/)).toBeInTheDocument();
  });

  it('should render document after successful load', async () => {
    mockFetchAtifBySession.mockResolvedValue(mockAtifDoc);
    await act(async () => { renderPage(); });
    const input = screen.getByPlaceholderText('输入 Session ID...');
    await act(async () => {
      fireEvent.change(input, { target: { value: 'sess-atif-test' } });
    });
    await act(async () => {
      fireEvent.click(screen.getByText('加载'));
    });
    // Check document is rendered
    expect(screen.getByText('ATIF-1.0')).toBeInTheDocument();
    expect(screen.getByText('交互轨迹')).toBeInTheDocument();
    expect(screen.getByText('共 2 步')).toBeInTheDocument();
  });

  it('should show agent info card', async () => {
    mockFetchAtifBySession.mockResolvedValue(mockAtifDoc);
    await act(async () => { renderPage(); });
    const input = screen.getByPlaceholderText('输入 Session ID...');
    await act(async () => {
      fireEvent.change(input, { target: { value: 'sess-123' } });
    });
    await act(async () => {
      fireEvent.click(screen.getByText('加载'));
    });
    expect(screen.getByText('TestAgent')).toBeInTheDocument();
  });

  it('should show metrics cards', async () => {
    mockFetchAtifBySession.mockResolvedValue(mockAtifDoc);
    await act(async () => { renderPage(); });
    const input = screen.getByPlaceholderText('输入 Session ID...');
    await act(async () => {
      fireEvent.change(input, { target: { value: 'sess-metrics' } });
    });
    await act(async () => {
      fireEvent.click(screen.getByText('加载'));
    });
    expect(screen.getByText('总步骤数')).toBeInTheDocument();
    expect(screen.getByText('总输入 Token')).toBeInTheDocument();
    expect(screen.getByText('总输出 Token')).toBeInTheDocument();
  });

  it('should render step cards with correct source labels', async () => {
    mockFetchAtifBySession.mockResolvedValue(mockAtifDoc);
    await act(async () => { renderPage(); });
    const input = screen.getByPlaceholderText('输入 Session ID...');
    await act(async () => {
      fireEvent.change(input, { target: { value: 'sess-steps' } });
    });
    await act(async () => {
      fireEvent.click(screen.getByText('加载'));
    });
    expect(screen.getByText('用户')).toBeInTheDocument();
    expect(screen.getByText('Agent')).toBeInTheDocument();
    expect(screen.getByText('Step 1')).toBeInTheDocument();
    expect(screen.getByText('Step 2')).toBeInTheDocument();
  });

  it('should switch to conversation mode', async () => {
    mockFetchAtifByConversation.mockResolvedValue(mockAtifDoc);
    await act(async () => { renderPage(); });
    await act(async () => {
      fireEvent.click(screen.getByText('按 Conversation'));
    });
    const input = screen.getByPlaceholderText('输入 Conversation ID...');
    await act(async () => {
      fireEvent.change(input, { target: { value: 'conv-123' } });
    });
    await act(async () => {
      fireEvent.click(screen.getByText('加载'));
    });
    expect(mockFetchAtifByConversation).toHaveBeenCalledWith('conv-123');
  });

  it('should show back button', async () => {
    await act(async () => { renderPage(); });
    expect(screen.getByText('← 返回')).toBeInTheDocument();
  });

  it('should show download button when doc is loaded', async () => {
    mockFetchAtifBySession.mockResolvedValue(mockAtifDoc);
    await act(async () => { renderPage(); });
    const input = screen.getByPlaceholderText('输入 Session ID...');
    await act(async () => {
      fireEvent.change(input, { target: { value: 'sess-dl' } });
    });
    await act(async () => {
      fireEvent.click(screen.getByText('加载'));
    });
    expect(screen.getByText('⬇️ 下载 JSON')).toBeInTheDocument();
  });

  it('should show model name on agent steps', async () => {
    mockFetchAtifBySession.mockResolvedValue(mockAtifDoc);
    await act(async () => { renderPage(); });
    const input = screen.getByPlaceholderText('输入 Session ID...');
    await act(async () => {
      fireEvent.change(input, { target: { value: 'sess-model' } });
    });
    await act(async () => {
      fireEvent.click(screen.getByText('加载'));
    });
    expect(screen.getByText('gpt-4o')).toBeInTheDocument();
  });

  it('should show expandable text for long messages', async () => {
    const longMessage = 'A'.repeat(400); // over threshold
    const docWithLong = {
      ...mockAtifDoc,
      steps: [{
        step_id: 1,
        source: 'user',
        message: longMessage,
        timestamp: '2024-01-01T10:00:00Z',
        model_name: null,
        reasoning_content: null,
        tool_calls: null,
        observation: null,
        metrics: null,
      }],
    };
    mockFetchAtifBySession.mockResolvedValue(docWithLong);
    await act(async () => { renderPage(); });
    const input = screen.getByPlaceholderText('输入 Session ID...');
    await act(async () => {
      fireEvent.change(input, { target: { value: 'sess-long' } });
    });
    await act(async () => {
      fireEvent.click(screen.getByText('加载'));
    });
    expect(screen.getByText('展开全部 →')).toBeInTheDocument();
  });

  it('should auto-load from URL params', async () => {
    mockFetchAtifBySession.mockResolvedValue(mockAtifDoc);
    await act(async () => {
      renderPage('/atif?type=session&id=sess-from-url');
    });
    expect(mockFetchAtifBySession).toHaveBeenCalledWith('sess-from-url');
  });

  it('should show Token savings comparison card when savings data exists', async () => {
    mockFetchAtifBySession.mockResolvedValue(mockAtifDoc);
    mockFetchSessionSavings.mockResolvedValue({
      session_id: 'sess-atif-test-123456789',
      stats_available: true,
      total_actual_tokens: 8000,
      total_compounded_saved: 2000,
      total_original_tokens: 10000,
      savings_rate: 20.0,
      items: [{
        id: 'tc-1',
        category: 'tool_output',
        strategy: 'compress-schema',
        strategy_label: 'Schema 压缩',
        title: 'Schema 压缩',
        before_tokens: 500,
        after_tokens: 200,
        saved_tokens: 300,
        compounded_saved: 600,
        compounding_turns: 2,
        before_summary: '原始内容 500 tokens',
        after_summary: '优化后 200 tokens',
        before_text: null,
        after_text: null,
        diff_lines: [],
      }],
    });

    await act(async () => {
      renderPage('/atif?type=session&id=sess-atif-test-123456789');
    });

    // Wait for savings data to load
    await act(async () => {
      await new Promise(r => setTimeout(r, 50));
    });

    expect(screen.getByText('Token 节省对比')).toBeInTheDocument();
    expect(screen.getByText('原始 Token（未优化）')).toBeInTheDocument();
    expect(screen.getByText('实际 Token（优化后）')).toBeInTheDocument();
    expect(screen.getByText('10,000')).toBeInTheDocument();
    expect(screen.getByText('8,000')).toBeInTheDocument();
  });
});
