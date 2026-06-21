/**
 * @license
 * Copyright 2025 Google LLC
 * SPDX-License-Identifier: Apache-2.0
 */

import { describe, it, expect, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import type { Config } from '@copilot-shell/core';
import { AuthType } from '@copilot-shell/core';
import type { LoadedSettings } from '../../config/settings.js';
import { useAuthCommand } from './useAuth.js';

vi.mock('../hooks/useQwenAuth.js', () => ({
  useQwenAuth: () => ({
    qwenAuthState: undefined,
    cancelQwenAuth: vi.fn(),
  }),
}));

vi.mock('../../config/modelProvidersScope.js', () => ({
  getPersistScopeForModelSelection: () => 'user',
}));

describe('useAuthCommand', () => {
  const createMockSettings = (): LoadedSettings =>
    ({
      merged: {
        security: {
          auth: {},
        },
        model: {},
      },
      setValue: vi.fn(),
      isTrusted: false,
      user: { settings: {} },
      workspace: { settings: {} },
    }) as unknown as LoadedSettings;

  const createMockConfig = (): Config =>
    ({
      getAuthType: vi.fn(() => undefined),
      getModelsConfig: vi.fn(() => ({})),
      refreshAuth: vi.fn(),
      getContentGenerator: vi.fn(() => undefined),
      getContentGeneratorConfig: vi.fn(() => undefined),
      updateCredentials: vi.fn(),
      getUsageStatisticsEnabled: vi.fn(() => false),
    }) as unknown as Config;

  it('restores bash option after canceling OpenAI auth when startup allows bash', async () => {
    const settings = createMockSettings();
    const config = createMockConfig();
    const addItem = vi.fn();

    const { result } = renderHook(() =>
      useAuthCommand(settings, config, addItem, true),
    );

    await act(async () => {
      await result.current.handleAuthSelect(AuthType.USE_OPENAI);
    });

    expect(result.current.showBashOptionInAuthDialog).toBe(false);
    expect(result.current.isAuthenticating).toBe(true);

    act(() => {
      result.current.cancelAuthentication();
    });

    expect(result.current.isAuthenticating).toBe(false);
    expect(result.current.isAuthDialogOpen).toBe(true);
    expect(result.current.showBashOptionInAuthDialog).toBe(true);
  });

  it('keeps bash option hidden after cancel when startup does not allow bash', async () => {
    const settings = createMockSettings();
    const config = createMockConfig();
    const addItem = vi.fn();

    const { result } = renderHook(() =>
      useAuthCommand(settings, config, addItem, false),
    );

    await act(async () => {
      await result.current.handleAuthSelect(AuthType.USE_OPENAI);
    });

    act(() => {
      result.current.cancelAuthentication();
    });

    expect(result.current.isAuthDialogOpen).toBe(true);
    expect(result.current.showBashOptionInAuthDialog).toBe(false);
  });

  it('should persist effective model to model.name and security.auth.openaiModel', async () => {
    const settings = createMockSettings();
    const config = createMockConfig();
    const addItem = vi.fn();
    vi.mocked(config.refreshAuth).mockResolvedValue(undefined);
    vi.mocked(config.getContentGeneratorConfig).mockReturnValue({
      model: 'my-model',
    } as ReturnType<Config['getContentGeneratorConfig']>);

    const { result } = renderHook(() =>
      useAuthCommand(settings, config, addItem, false),
    );

    // Step 1: set pendingAuthType (simulates user selecting OpenAI in AuthDialog)
    await act(async () => {
      await result.current.handleAuthSelect(AuthType.USE_OPENAI);
    });

    // Step 2: submit credentials (simulates OpenAIKeyPrompt submission)
    await act(async () => {
      await result.current.handleAuthSelect(AuthType.USE_OPENAI, {
        apiKey: 'sk-test',
        baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
        model: 'my-model',
      });
    });

    const calls = vi.mocked(settings.setValue).mock.calls;
    const modelNameCall = calls.find(([, key]) => key === 'model.name');
    const openaiModelCall = calls.find(
      ([, key]) => key === 'security.auth.openaiModel',
    );
    const openaiModelsCall = calls.find(
      ([, key]) => key === 'security.auth.openaiModels',
    );
    expect(modelNameCall).toBeDefined();
    expect(modelNameCall![2]).toBe('my-model');
    expect(openaiModelCall).toBeDefined();
    expect(openaiModelCall![2]).toBe('my-model');
    expect(openaiModelsCall).toBeDefined();
    expect(openaiModelsCall![2]).toEqual(['my-model']);
  });

  it('should persist validated fallback model over submitted model', async () => {
    const settings = createMockSettings();
    settings.merged.security!.auth!.openaiModels = [
      'qwen3.5-plus',
      'qwen3-coder-plus',
    ];
    const config = createMockConfig();
    const addItem = vi.fn();
    vi.mocked(config.refreshAuth).mockResolvedValue(undefined);
    vi.mocked(config.getContentGeneratorConfig).mockReturnValue({
      model: 'qwen3-coder-plus',
    } as ReturnType<Config['getContentGeneratorConfig']>);

    const { result } = renderHook(() =>
      useAuthCommand(settings, config, addItem, false),
    );

    await act(async () => {
      await result.current.handleAuthSelect(AuthType.USE_OPENAI);
    });

    await act(async () => {
      await result.current.handleAuthSelect(AuthType.USE_OPENAI, {
        apiKey: 'sk-test',
        baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
        model: 'my-model',
      });
    });

    const calls = vi.mocked(settings.setValue).mock.calls;
    const modelNameCall = calls.find(([, key]) => key === 'model.name');
    const openaiModelCall = calls.find(
      ([, key]) => key === 'security.auth.openaiModel',
    );
    const openaiModelsCall = calls.find(
      ([, key]) => key === 'security.auth.openaiModels',
    );
    expect(modelNameCall).toBeDefined();
    expect(modelNameCall![2]).toBe('qwen3-coder-plus');
    expect(openaiModelCall).toBeDefined();
    expect(openaiModelCall![2]).toBe('qwen3-coder-plus');
    expect(openaiModelsCall).toBeDefined();
    expect(openaiModelsCall![2]).toEqual(['qwen3-coder-plus', 'qwen3.5-plus']);
  });

  it('should set authError and keep isAuthenticating for OpenAI on failure', async () => {
    const settings = createMockSettings();
    const config = createMockConfig();
    const addItem = vi.fn();
    vi.mocked(config.refreshAuth).mockRejectedValue(
      new Error('Invalid API key'),
    );

    const { result } = renderHook(() =>
      useAuthCommand(settings, config, addItem, false),
    );

    // Step 1: set pendingAuthType
    await act(async () => {
      await result.current.handleAuthSelect(AuthType.USE_OPENAI);
    });

    // Step 2: submit credentials that will fail
    await act(async () => {
      await result.current.handleAuthSelect(AuthType.USE_OPENAI, {
        apiKey: 'sk-bad',
        baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
        model: 'test-model',
      });
    });

    expect(result.current.authError).toBeTruthy();
    expect(result.current.isAuthenticating).toBe(true);

    const errorItem = addItem.mock.calls.find(([item]) => {
      const historyItem = item as Omit<import('../types.js').HistoryItem, 'id'>;
      return historyItem.type === 'error';
    });
    expect(errorItem).toBeDefined();
  });
});
