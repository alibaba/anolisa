import React from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';

// Mock apiClient
vi.mock('../utils/apiClient', () => ({
  fetchAtifBySession: vi.fn(),
  fetchAtifByConversation: vi.fn(),
}));

import { fetchAtifBySession, fetchAtifByConversation } from '../utils/apiClient';
import { AtifViewerPage } from '../pages/AtifViewerPage';

const mockFetchAtifBySession = fetchAtifBySession as ReturnType<typeof vi.fn>;
const mockFetchAtifByConversation = fetchAtifByConversation as ReturnType<typeof vi.fn>;

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
});
