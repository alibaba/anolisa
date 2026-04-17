/**
 * @license
 * Copyright 2025 Google LLC
 * SPDX-License-Identifier: Apache-2.0
 */

import { act } from 'react';
import { render } from 'ink-testing-library';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { OpenAIKeyPrompt, credentialSchema } from './OpenAIKeyPrompt.js';
import type { Key } from '../hooks/useKeypress.js';
import { useKeypress } from '../hooks/useKeypress.js';

// Mock useKeypress hook
vi.mock('../hooks/useKeypress.js', () => ({
  useKeypress: vi.fn(),
}));

describe('OpenAIKeyPrompt', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  // ─── 基础渲染 ───────────────────────────────────────────────────────────────

  it('should render the prompt correctly', () => {
    const onSubmit = vi.fn();
    const onCancel = vi.fn();

    const { lastFrame } = render(
      <OpenAIKeyPrompt
        onSubmit={onSubmit}
        onCancel={onCancel}
        defaultBaseUrl="https://api.deepseek.com"
      />,
    );

    expect(lastFrame()).toContain('Custom Provider Configuration Required');
    expect(lastFrame()).toContain('DeepSeek');
    expect(lastFrame()).toContain(
      '↑↓ select provider · Enter/Tab navigate fields · Esc cancel',
    );
  });

  it('should show the component with proper styling', () => {
    const onSubmit = vi.fn();
    const onCancel = vi.fn();

    const { lastFrame } = render(
      <OpenAIKeyPrompt
        onSubmit={onSubmit}
        onCancel={onCancel}
        defaultBaseUrl="https://api.deepseek.com"
      />,
    );

    const output = lastFrame();
    expect(output).toContain('Custom Provider Configuration Required');
    expect(output).toContain('API Key:');
    expect(output).toContain('Base URL:');
    expect(output).toContain('Model:');
    expect(output).toContain(
      '↑↓ select provider · Enter/Tab navigate fields · Esc cancel',
    );
  });

  // ─── 全部 provider 列表渲染 ─────────────────────────────────────────────────

  it('should render all preset providers in the list', () => {
    const { lastFrame } = render(
      <OpenAIKeyPrompt onSubmit={vi.fn()} onCancel={vi.fn()} />,
    );
    const output = lastFrame()!;
    expect(output).toContain('DashScope');
    expect(output).toContain('DashScope Coding Plan');
    expect(output).toContain('DeepSeek');
    expect(output).toContain('GLM');
    expect(output).toContain('Kimi');
    expect(output).toContain('MiniMax');
    // providers with subProviders show '›'
    expect(output).toContain('DashScope ›');
    expect(output).toContain('DashScope Coding Plan ›');
  });

  // ─── subProviders provider 隐藏字段 ────────────────────────────────────────

  it('should hide API Key/Base URL/Model when a sub-provider parent is selected without defaultApiKey', () => {
    // DashScope 是默认选中项 (index 0) 且有 subProviders
    const { lastFrame } = render(
      <OpenAIKeyPrompt onSubmit={vi.fn()} onCancel={vi.fn()} />,
    );
    const output = lastFrame()!;
    expect(output).not.toContain('API Key:');
    expect(output).not.toContain('Base URL:');
    expect(output).not.toContain('Model:');
  });

  it('should show API Key/Base URL/Model when a sub-provider parent is selected WITH defaultApiKey', () => {
    const { lastFrame } = render(
      <OpenAIKeyPrompt
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
        defaultApiKey="sk-existing"
      />,
    );
    const output = lastFrame()!;
    expect(output).toContain('API Key:');
    expect(output).toContain('Base URL:');
    expect(output).toContain('Model:');
  });

  // ─── defaultBaseUrl 初始化 provider 选择 ───────────────────────────────────

  it('should auto-select provider matching defaultBaseUrl (Kimi)', () => {
    const { lastFrame } = render(
      <OpenAIKeyPrompt
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
        defaultBaseUrl="https://api.moonshot.cn/v1"
      />,
    );
    const output = lastFrame()!;
    // Kimi 被选中：显示 ● 标志
    expect(output).toContain('● Kimi');
    expect(output).toContain('API Key:');
  });

  it('should auto-select DashScope subProvider matching defaultBaseUrl (Singapore)', () => {
    const { lastFrame } = render(
      <OpenAIKeyPrompt
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
        defaultBaseUrl="https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
      />,
    );
    const output = lastFrame()!;
    // 顶层 DashScope 被选中
    expect(output).toContain('● DashScope ›');
  });

  it('should show API Key when DashScope Coding Plan China is configured with existing key', () => {
    // China (Aliyun) 子 provider 与顶层 Coding Plan 曾共享相同的 baseUrl，
    // 顶层不参与匹配后，正确命中 subProvider sIdx=0，有 apiKey 则字段显示。
    const { lastFrame } = render(
      <OpenAIKeyPrompt
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
        defaultBaseUrl="https://coding.dashscope.aliyuncs.com/v1"
        defaultApiKey="sk-existing-key"
      />,
    );
    const output = lastFrame()!;
    expect(output).toContain('● DashScope Coding Plan ›');
    expect(output).toContain('API Key:');
    expect(output).toContain('Base URL:');
    expect(output).toContain('Model:');
  });

  it('should hide API Key when DashScope Coding Plan China is configured without defaultApiKey', () => {
    // 有 subProviders 且 apiKey 为空时，provider 阶段隐藏字段（与 DashScope 普通版行为一致）
    const { lastFrame } = render(
      <OpenAIKeyPrompt
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
        defaultBaseUrl="https://coding.dashscope.aliyuncs.com/v1"
      />,
    );
    const output = lastFrame()!;
    expect(output).toContain('● DashScope Coding Plan ›');
    expect(output).not.toContain('API Key:');
  });

  it('should select DashScope Coding Plan International subProvider correctly', () => {
    const { lastFrame } = render(
      <OpenAIKeyPrompt
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
        defaultBaseUrl="https://coding-intl.dashscope.aliyuncs.com/v1"
        defaultApiKey="sk-intl-key"
      />,
    );
    const output = lastFrame()!;
    expect(output).toContain('● DashScope Coding Plan ›');
    expect(output).toContain('API Key:');
  });

  it('should show API Key on init when configured with International subProvider (initS=1)', () => {
    // 修复点：handleProviderChange 原先检查 initS===0，导致 initS=1（International）
    // 的用户切回 Coding Plan 时 apiKey 被清空、字段隐藏。
    // 初始渲染时 initS=1 且 apiKey 有值，字段应正常显示。
    const { lastFrame } = render(
      <OpenAIKeyPrompt
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
        defaultBaseUrl="https://coding-intl.dashscope.aliyuncs.com/v1"
        defaultApiKey="sk-intl-key"
      />,
    );
    const output = lastFrame()!;
    expect(output).toContain('● DashScope Coding Plan ›');
    expect(output).toContain('API Key:');
    expect(output).toContain('Base URL:');
    expect(output).toContain('Model:');
  });

  // ─── defaultApiKey 掩码显示 ─────────────────────────────────────────────────

  it('should mask defaultApiKey in display', () => {
    const { lastFrame } = render(
      <OpenAIKeyPrompt
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
        defaultBaseUrl="https://api.deepseek.com"
        defaultApiKey="sk-abcdef"
      />,
    );
    const output = lastFrame()!;
    expect(output).not.toContain('sk-abcdef');
    // 前3位明文 + 掩码
    expect(output).toContain('sk-');
    expect(output).toContain('****');
  });

  // ─── 输入控制字符过滤 ────────────────────────────────────────────────────────

  it('should handle paste with control characters', async () => {
    const onSubmit = vi.fn();
    const onCancel = vi.fn();

    const { stdin } = render(
      <OpenAIKeyPrompt onSubmit={onSubmit} onCancel={onCancel} />,
    );

    // Simulate paste with control characters
    const pasteWithControlChars = '\x1b[200~sk-test123\x1b[201~';
    stdin.write(pasteWithControlChars);

    // Wait a bit for processing
    await new Promise((resolve) => setTimeout(resolve, 50));

    // The component should have filtered out the control characters
    // and only kept 'sk-test123'
    expect(onSubmit).not.toHaveBeenCalled(); // Should not submit yet
  });

  // ─── credentialSchema ────────────────────────────────────────────────────────

  it('credentialSchema should reject empty apiKey', () => {
    const result = credentialSchema.safeParse({ apiKey: '' });
    expect(result.success).toBe(false);
  });

  it('credentialSchema should accept valid apiKey', () => {
    const result = credentialSchema.safeParse({
      apiKey: 'sk-abc',
      baseUrl: 'https://api.example.com',
      model: 'gpt-4',
    });
    expect(result.success).toBe(true);
  });

  // ─── API Key retention on navigation (#240) ─────────────────────────────────

  describe('API Key retention on navigation (#240)', () => {
    const makeKey = (overrides: Partial<Key> = {}): Key => ({
      name: '',
      ctrl: false,
      meta: false,
      shift: false,
      paste: false,
      sequence: '',
      ...overrides,
    });

    function getLatestHandler(): (key: Key) => void {
      const mock = vi.mocked(useKeypress);
      return mock.mock.calls[mock.mock.calls.length - 1]![0];
    }

    async function pressKey(key: Partial<Key>): Promise<void> {
      await act(() => {
        getLatestHandler()(makeKey(key));
      });
    }

    it('should retain defaultApiKey when navigating to apiKey via Enter on leaf provider', async () => {
      // DeepSeek is a leaf provider (no subProviders)
      const { lastFrame } = render(
        <OpenAIKeyPrompt
          onSubmit={vi.fn()}
          onCancel={vi.fn()}
          defaultBaseUrl="https://api.deepseek.com"
          defaultApiKey="sk-abcdef"
        />,
      );

      // Masked key visible on initial render
      expect(lastFrame()).toContain('sk-***');

      // Press Enter to navigate from provider to apiKey field
      await pressKey({ name: 'return', sequence: '\r' });

      // API key should still be displayed (not cleared)
      expect(lastFrame()).toContain('sk-***');
    });

    it('should retain defaultApiKey when navigating to apiKey via Tab on leaf provider', async () => {
      const { lastFrame } = render(
        <OpenAIKeyPrompt
          onSubmit={vi.fn()}
          onCancel={vi.fn()}
          defaultBaseUrl="https://api.deepseek.com"
          defaultApiKey="sk-abcdef"
        />,
      );

      // Press Tab to navigate from provider to apiKey field
      await pressKey({ name: 'tab', sequence: '\t' });

      // API key should still be displayed
      expect(lastFrame()).toContain('sk-***');
    });

    it('should retain defaultApiKey when navigating through subProvider to apiKey', async () => {
      // DashScope Singapore is a sub-provider
      const { lastFrame } = render(
        <OpenAIKeyPrompt
          onSubmit={vi.fn()}
          onCancel={vi.fn()}
          defaultBaseUrl="https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
          defaultApiKey="sk-abcdef"
        />,
      );

      // Press Enter to enter subProvider menu
      await pressKey({ name: 'return', sequence: '\r' });

      // Press Enter on subProvider to go to apiKey
      await pressKey({ name: 'return', sequence: '\r' });

      // API key should still be displayed
      expect(lastFrame()).toContain('sk-***');
    });

    it('should clear entire apiKey on first backspace when showing default key', async () => {
      const { lastFrame } = render(
        <OpenAIKeyPrompt
          onSubmit={vi.fn()}
          onCancel={vi.fn()}
          defaultBaseUrl="https://api.deepseek.com"
          defaultApiKey="sk-abcdef"
        />,
      );

      // Navigate to apiKey field
      await pressKey({ name: 'return', sequence: '\r' });
      expect(lastFrame()).toContain('sk-***');

      // Press backspace - should clear the entire field
      await pressKey({ name: 'backspace', sequence: '\b' });

      // API key should be completely gone
      expect(lastFrame()).not.toContain('sk-');
    });

    it('should replace default key on first character input', async () => {
      const { lastFrame } = render(
        <OpenAIKeyPrompt
          onSubmit={vi.fn()}
          onCancel={vi.fn()}
          defaultBaseUrl="https://api.deepseek.com"
          defaultApiKey="sk-abcdef"
        />,
      );

      // Navigate to apiKey field
      await pressKey({ name: 'return', sequence: '\r' });
      expect(lastFrame()).toContain('sk-***');

      // Type 'x' - should replace the entire default key, not append
      await pressKey({ sequence: 'x' });

      // Should no longer show original key prefix
      expect(lastFrame()).not.toContain('sk-');
    });

    it('should delete single char on backspace after user clears and types new key', async () => {
      const { lastFrame } = render(
        <OpenAIKeyPrompt
          onSubmit={vi.fn()}
          onCancel={vi.fn()}
          defaultBaseUrl="https://api.deepseek.com"
          defaultApiKey="sk-abcdef"
        />,
      );

      // Navigate to apiKey field and clear default
      await pressKey({ name: 'return', sequence: '\r' });
      await pressKey({ name: 'backspace', sequence: '\b' });

      // Type 'abcd' (4 chars → maskApiKey shows 'abc*')
      for (const ch of ['a', 'b', 'c', 'd']) {
        await pressKey({ sequence: ch });
      }
      expect(lastFrame()).toContain('abc*');

      // Backspace should only delete last char, leaving 'abc' (3 chars → '***')
      await pressKey({ name: 'backspace', sequence: '\b' });
      expect(lastFrame()).not.toContain('abc*');
    });
  });
});
