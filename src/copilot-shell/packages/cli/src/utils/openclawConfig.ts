/**
 * @license
 * Copyright 2026 Alibaba Cloud
 * SPDX-License-Identifier: Apache-2.0
 */

import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { OPENAI_PROVIDERS } from '../ui/components/OpenAIKeyPrompt.js';

export interface OpenClawConfig {
  apiKey: string;
  baseUrl: string;
  model: string;
  providerName: string;
}

/**
 * 匹配顺序：子域名必须在父域名前，否则 coding.dashscope... 会被 dashscope 提前命中。
 * 此顺序独立于 OPENAI_PROVIDERS 的数组顺序。
 */
const PROVIDER_MATCH_ORDER = [
  'dashscope-coding-plan',
  'dashscope',
  'deepseek',
  'kimi',
  'glm',
  'minimax',
  'claude',
  'chatgpt',
];
/**
 * 读取 ~/.openclaw/openclaw.json，按 provider 优先级返回第一个可用配置。
 * 仅采信 api === 'openai-completions' 的 provider。
 * 文件不存在、格式非法或无有效 provider 时返回 null。
 */
export function readOpenClawConfig(): OpenClawConfig | null {
  try {
    const filePath = join(homedir(), '.openclaw', 'openclaw.json');
    const json = JSON.parse(readFileSync(filePath, 'utf-8')) as Record<
      string,
      unknown
    >;
    const providers = (json['models'] as Record<string, unknown> | undefined)?.[
      'providers'
    ] as Record<string, unknown> | undefined;
    if (!providers) return null;

    const candidates = Object.entries(providers)
      .map(([providerName, p]) => {
        const entry = p as Record<string, unknown> | null | undefined;
        const models = entry?.['models'] as
          | Array<Record<string, unknown>>
          | undefined;
        return {
          providerName,
          api: entry?.['api'] as string | undefined,
          apiKey: entry?.['apiKey'] as string | undefined,
          baseUrl: entry?.['baseUrl'] as string | undefined,
          model: (models?.[0]?.['id'] as string | undefined) ?? '',
        };
      })
      .filter(
        (c): c is typeof c & { apiKey: string; baseUrl: string } =>
          c.api === 'openai-completions' &&
          Boolean(c.apiKey) &&
          Boolean(c.baseUrl),
      );

    if (candidates.length === 0) return null;

    // 按指定顺序（子域名先于父域名）匹配 OPENAI_PROVIDERS
    for (const id of PROVIDER_MATCH_ORDER) {
      const preset = OPENAI_PROVIDERS.find((p) => p.id === id);
      if (!preset) continue;
      const found = candidates.find((c) =>
        c.baseUrl.includes(new URL(preset.baseUrl).hostname),
      );
      if (found) {
        return {
          apiKey: found.apiKey,
          baseUrl: found.baseUrl,
          model: found.model,
          providerName: preset.name,
        };
      }
    }

    // 兜底：取第一个有效 provider
    const { apiKey, baseUrl, model, providerName } = candidates[0];
    return { apiKey, baseUrl, model, providerName };
  } catch {
    return null;
  }
}
