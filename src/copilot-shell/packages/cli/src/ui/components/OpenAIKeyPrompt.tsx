/**
 * @license
 * Copyright 2025 Google LLC
 * SPDX-License-Identifier: Apache-2.0
 */

import type React from 'react';
import { useState } from 'react';
import { z } from 'zod';
import { Box, Text } from 'ink';
import { Colors } from '../colors.js';
import { useKeypress } from '../hooks/useKeypress.js';
import { t } from '../../i18n/index.js';

/**
 * Preset provider configurations for quick-fill.
 * "custom" means the user types their own Base URL.
 */
export interface OpenAIProvider {
  id: string;
  name: string;
  baseUrl: string;
  defaultModel: string;
  /** URL to apply for an API key; empty string for custom */
  apiKeyUrl: string;
}

export const OPENAI_PROVIDERS: OpenAIProvider[] = [
  {
    id: 'dashscope',
    name: 'DashScope',
    baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    defaultModel: 'qwen3-coder-plus',
    apiKeyUrl: 'https://bailian.console.aliyun.com/?tab=model#/api-key',
  },
  {
    id: 'dashscope-coding-plan',
    name: 'DashScope Coding Plan',
    baseUrl: 'https://coding.dashscope.aliyuncs.com/v1',
    defaultModel: 'qwen3-coder-plus',
    apiKeyUrl:
      'https://bailian.console.aliyun.com/?tab=coding-plan#/efm/coding-plan-detail',
  },
  {
    id: 'deepseek',
    name: 'DeepSeek',
    baseUrl: 'https://api.deepseek.com',
    defaultModel: 'deepseek-chat',
    apiKeyUrl: 'https://platform.deepseek.com/api_keys',
  },
  {
    id: 'glm',
    name: 'GLM',
    baseUrl: 'https://open.bigmodel.cn/api/paas/v4',
    defaultModel: 'glm-5',
    apiKeyUrl: 'https://bigmodel.cn/usercenter/proj-mgmt/apikeys',
  },
  {
    id: 'kimi',
    name: 'Kimi',
    baseUrl: 'https://api.moonshot.cn/v1',
    defaultModel: 'kimi-k2.5',
    apiKeyUrl: 'https://platform.moonshot.cn/console/api-keys',
  },
  {
    id: 'minimax',
    name: 'MiniMax',
    baseUrl: 'https://api.minimaxi.com/v1',
    defaultModel: 'MiniMax-M2.5',
    apiKeyUrl:
      'https://platform.minimaxi.com/user-center/basic-information/interface-key',
  },
  {
    id: 'custom',
    name: t('Custom (enter Base URL manually)'),
    baseUrl: '',
    defaultModel: '',
    apiKeyUrl: '',
  },
];

interface OpenAIKeyPromptProps {
  onSubmit: (apiKey: string, baseUrl: string, model: string) => void;
  onCancel: () => void;
  defaultApiKey?: string;
  defaultBaseUrl?: string;
  defaultModel?: string;
}

export const credentialSchema = z.object({
  apiKey: z.string().min(1, 'API key is required'),
  baseUrl: z
    .union([z.string().url('Base URL must be a valid URL'), z.literal('')])
    .optional(),
  model: z.string().min(1, 'Model must be a non-empty string').optional(),
});

export type OpenAICredentials = z.infer<typeof credentialSchema>;

function maskApiKey(key: string): string {
  if (!key) {
    return '';
  }
  if (key.length <= 3) {
    return '*'.repeat(key.length);
  }
  return key.slice(0, 3) + '*'.repeat(key.length - 3);
}

type FieldName = 'provider' | 'apiKey' | 'baseUrl' | 'model';

export function OpenAIKeyPrompt({
  onSubmit,
  onCancel,
  defaultApiKey,
  defaultBaseUrl,
  defaultModel,
}: OpenAIKeyPromptProps): React.JSX.Element {
  // Detect initial provider from defaultBaseUrl
  const detectInitialProvider = (): number => {
    if (!defaultBaseUrl) return 0; // custom
    const idx = OPENAI_PROVIDERS.findIndex(
      (p) => p.id !== 'custom' && p.baseUrl === defaultBaseUrl,
    );
    return idx >= 0 ? idx : 0;
  };

  const [providerIndex, setProviderIndex] = useState(detectInitialProvider);
  const [apiKey, setApiKey] = useState(defaultApiKey || '');
  const initialProviderIndex = detectInitialProvider();
  const initialProvider = OPENAI_PROVIDERS[initialProviderIndex];
  const [baseUrl, setBaseUrl] = useState(
    defaultBaseUrl || initialProvider?.baseUrl || '',
  );
  const [model, setModel] = useState(
    defaultModel ||
      (initialProvider?.id !== 'custom' ? initialProvider?.defaultModel : '') ||
      '',
  );
  const [currentField, setCurrentField] = useState<FieldName>('provider');
  const [validationError, setValidationError] = useState<string | null>(null);

  const selectedProvider = OPENAI_PROVIDERS[providerIndex];
  const isCustom = selectedProvider?.id === 'custom';

  const validateAndSubmit = () => {
    setValidationError(null);
    const effectiveBaseUrl = isCustom
      ? baseUrl.trim()
      : (selectedProvider?.baseUrl ?? '');
    const effectiveModel = model.trim();

    try {
      const validated = credentialSchema.parse({
        apiKey: apiKey.trim(),
        baseUrl: effectiveBaseUrl || undefined,
        model: effectiveModel || undefined,
      });

      onSubmit(
        validated.apiKey,
        validated.baseUrl === '' ? '' : validated.baseUrl || '',
        validated.model || '',
      );
    } catch (error) {
      if (error instanceof z.ZodError) {
        const errorMessage = error.errors
          .map((e) => `${e.path.join('.')}: ${e.message}`)
          .join(', ');
        setValidationError(
          t('Invalid credentials: {{errorMessage}}', { errorMessage }),
        );
      } else {
        setValidationError(t('Failed to validate credentials'));
      }
    }
  };

  const handleProviderChange = (newIndex: number) => {
    setProviderIndex(newIndex);
    // Clear API ke and modely when switching providers
    setApiKey('');
    setModel('');
    const p = OPENAI_PROVIDERS[newIndex];
    if (p && p.id !== 'custom') {
      setBaseUrl(p.baseUrl);
      setModel(p.defaultModel);
    } else {
      setBaseUrl('');
    }
  };

  useKeypress(
    (key) => {
      // Handle escape or Ctrl+C
      if (key.name === 'escape' || (key.ctrl && key.name === 'c')) {
        onCancel();
        return;
      }

      // Handle Enter key
      if (key.name === 'return') {
        if (currentField === 'provider') {
          // Clear API key when entering from provider selection
          setApiKey('');
          setCurrentField('apiKey');
          return;
        } else if (currentField === 'apiKey') {
          setCurrentField(isCustom ? 'baseUrl' : 'model');
          return;
        } else if (currentField === 'baseUrl') {
          setCurrentField('model');
          return;
        } else if (currentField === 'model') {
          // 只有在提交时才检查 API key 是否为空
          if (apiKey.trim()) {
            validateAndSubmit();
          } else {
            // 如果 API key 为空，回到 API key 字段
            setCurrentField('apiKey');
          }
        }
        return;
      }

      // Handle Tab key for field navigation
      if (key.name === 'tab') {
        if (currentField === 'provider') {
          // Clear API key when leaving provider selection
          setApiKey('');
          setCurrentField('apiKey');
        } else if (currentField === 'apiKey') {
          setCurrentField(isCustom ? 'baseUrl' : 'model');
        } else if (currentField === 'baseUrl') {
          setCurrentField('model');
        } else if (currentField === 'model') {
          setCurrentField('provider');
        }
        return;
      }

      // Handle arrow keys
      if (key.name === 'up') {
        if (currentField === 'provider') {
          const newIndex =
            (providerIndex - 1 + OPENAI_PROVIDERS.length) %
            OPENAI_PROVIDERS.length;
          handleProviderChange(newIndex);
        } else if (currentField === 'apiKey') {
          setCurrentField('provider');
        } else if (currentField === 'baseUrl') {
          setCurrentField('apiKey');
        } else if (currentField === 'model') {
          setCurrentField(isCustom ? 'baseUrl' : 'apiKey');
        }
        return;
      }

      if (key.name === 'down') {
        if (currentField === 'provider') {
          const newIndex = (providerIndex + 1) % OPENAI_PROVIDERS.length;
          handleProviderChange(newIndex);
        } else if (currentField === 'apiKey') {
          setCurrentField(isCustom ? 'baseUrl' : 'model');
        } else if (currentField === 'baseUrl') {
          setCurrentField('model');
        }
        return;
      }

      // Handle backspace/delete
      if (key.name === 'backspace' || key.name === 'delete') {
        if (currentField === 'apiKey') {
          setApiKey((prev) => prev.slice(0, -1));
        } else if (currentField === 'baseUrl') {
          setBaseUrl((prev) => prev.slice(0, -1));
        } else if (currentField === 'model') {
          setModel((prev) => prev.slice(0, -1));
        }
        return;
      }

      // Handle paste mode - if it's a paste event with content
      if (key.paste && key.sequence) {
        // 过滤粘贴相关的控制序列
        let cleanInput = key.sequence
          // 过滤 ESC 开头的控制序列（如 \u001b[200~、\u001b[201~ 等）
          .replace(/\u001b\[[0-9;]*[a-zA-Z]/g, '') // eslint-disable-line no-control-regex
          // 过滤粘贴开始标记 [200~
          .replace(/\[200~/g, '')
          // 过滤粘贴结束标记 [201~
          .replace(/\[201~/g, '')
          // 过滤单独的 [ 和 ~ 字符（可能是粘贴标记的残留）
          .replace(/^\[|~$/g, '');

        // 再过滤所有不可见字符（ASCII < 32，除了回车换行）
        cleanInput = cleanInput
          .split('')
          .filter((ch) => ch.charCodeAt(0) >= 32)
          .join('');

        if (cleanInput.length > 0) {
          if (currentField === 'apiKey') {
            setApiKey((prev) => prev + cleanInput);
          } else if (currentField === 'baseUrl') {
            setBaseUrl((prev) => prev + cleanInput);
          } else if (currentField === 'model') {
            setModel((prev) => prev + cleanInput);
          }
        }
        return;
      }

      // Handle regular character input
      if (key.sequence && !key.ctrl && !key.meta) {
        // Filter control characters
        const cleanInput = key.sequence
          .split('')
          .filter((ch) => ch.charCodeAt(0) >= 32)
          .join('');

        if (cleanInput.length > 0) {
          if (currentField === 'apiKey') {
            setApiKey((prev) => prev + cleanInput);
          } else if (currentField === 'baseUrl') {
            setBaseUrl((prev) => prev + cleanInput);
          } else if (currentField === 'model') {
            setModel((prev) => prev + cleanInput);
          }
        }
      }
    },
    { isActive: true },
  );

  return (
    <Box
      borderStyle="round"
      borderColor={Colors.AccentBlue}
      flexDirection="column"
      padding={1}
      width="100%"
    >
      <Text bold color={Colors.AccentBlue}>
        {t('Custom Provider Configuration Required')}
      </Text>
      {validationError && (
        <Box marginTop={1}>
          <Text color={Colors.AccentRed}>{validationError}</Text>
        </Box>
      )}

      {/* Provider selector */}
      <Box marginTop={1} flexDirection="column">
        <Text
          color={currentField === 'provider' ? Colors.AccentBlue : Colors.Gray}
        >
          {t('Provider:')}
        </Text>
        <Box marginLeft={2} flexDirection="column">
          {OPENAI_PROVIDERS.map((provider, idx) => (
            <Text
              key={provider.id}
              color={idx === providerIndex ? Colors.AccentBlue : Colors.Gray}
            >
              {idx === providerIndex ? '● ' : '○ '}
              {provider.name}
            </Text>
          ))}
        </Box>
      </Box>

      {/* API key URL hint for preset providers */}
      {selectedProvider && !isCustom && (
        <Box marginTop={1} flexDirection="row">
          <Text color={Colors.Gray}>{t('Get API key from: ')}</Text>
          <Text color={Colors.AccentBlue}>{selectedProvider.apiKeyUrl}</Text>
        </Box>
      )}

      {/* API Key field */}
      <Box marginTop={1} flexDirection="row">
        <Box width={12}>
          <Text
            color={currentField === 'apiKey' ? Colors.AccentBlue : Colors.Gray}
          >
            {t('API Key:')}
          </Text>
        </Box>
        <Box flexGrow={1}>
          <Text>
            {currentField === 'apiKey' ? '> ' : '  '}
            {maskApiKey(apiKey) || ' '}
          </Text>
        </Box>
      </Box>

      {/* Base URL: editable for custom, read-only for presets */}
      <Box marginTop={1} flexDirection="row">
        <Box width={12}>
          <Text
            color={
              currentField === 'baseUrl' && isCustom
                ? Colors.AccentBlue
                : Colors.Gray
            }
          >
            {t('Base URL:')}
          </Text>
        </Box>
        <Box flexGrow={1}>
          {isCustom ? (
            <Text>
              {currentField === 'baseUrl' ? '> ' : '  '}
              {baseUrl}
            </Text>
          ) : (
            <Text color={Colors.Gray}>
              {'  '}
              {selectedProvider?.baseUrl}
            </Text>
          )}
        </Box>
      </Box>

      {/* Model field */}
      <Box marginTop={1} flexDirection="row">
        <Box width={12}>
          <Text
            color={currentField === 'model' ? Colors.AccentBlue : Colors.Gray}
          >
            {t('Model:')}
          </Text>
        </Box>
        <Box flexGrow={1}>
          <Text>
            {currentField === 'model' ? '> ' : '  '}
            {model}
          </Text>
        </Box>
      </Box>
      <Box marginTop={1}>
        <Text color={Colors.Gray}>
          {t('↑↓ select provider · Enter/Tab navigate fields · Esc cancel')}
        </Text>
      </Box>
    </Box>
  );
}
