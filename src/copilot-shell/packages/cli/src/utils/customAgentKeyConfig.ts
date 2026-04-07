/**
 * @license
 * Copyright 2026 Alibaba Cloud
 * SPDX-License-Identifier: Apache-2.0
 */

import { readFileSync, existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { OPENAI_PROVIDERS } from '../ui/components/OpenAIKeyPrompt.js';

export interface AgentKeyConfig {
  apiKey: string;
  baseUrl: string;
  model: string;
  providerName: string;
}

/**
 * 完成认证/返回流程的自动跳转延迟（毫秒）。
 * 由 CustomAgentKeyImportPrompt 和 CustomAgentKeyDetectFailedPrompt 共享。
 */
export const AGENT_KEY_AUTO_REDIRECT_MS = 2000;

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
 * 仅检查 OpenClaw 配置目录是否存在（~/.openclaw），不读取任何 Key。
 * 用于决定是否向用户展示 Agent 共享流程一。
 */
export function hasOpenClawConfigDir(): boolean {
  return existsSync(join(homedir(), '.openclaw'));
}

/**
 * 仅检查 Qwen Code 配置目录是否存在（~/.qwen），不读取任何 Key。
 * 用于决定是否向用户展示 Agent 共享流程一。
 */
export function hasQwenCodeConfigDir(): boolean {
  return existsSync(join(homedir(), '.qwen'));
}

/**
 * 读取 OpenClaw 的配置（~/.openclaw/openclaw.json）。
 * 仅采信 api === 'openai-completions' 的 provider。
 * 文件不存在、格式非法或无有效 provider 时返回 null。
 */
export function readOpenClawConfig(): AgentKeyConfig | null {
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

/**
 * 读取 Qwen Code 的用户配置（~/.qwen/settings.json）中的 OpenAI 兼容认证信息。
 * 仅读取 security.auth.selectedType === 'openai' 且具备 apiKey 的配置。
 * 文件不存在、格式非法或无有效配置时返回 null。
 */
export function readQwenCodeConfig(): AgentKeyConfig | null {
  try {
    const filePath = join(homedir(), '.qwen', 'settings.json');
    const json = JSON.parse(readFileSync(filePath, 'utf-8')) as Record<
      string,
      unknown
    >;
    const auth = (json['security'] as Record<string, unknown> | undefined)?.[
      'auth'
    ] as Record<string, unknown> | undefined;
    if (!auth) return null;

    const selectedType = auth['selectedType'] as string | undefined;
    if (selectedType !== 'openai') return null;

    const apiKey = auth['apiKey'] as string | undefined;
    const baseUrl = auth['baseUrl'] as string | undefined;
    // model 存储在顶层 model.name，而非 security.auth 内
    const model =
      ((json['model'] as Record<string, unknown> | undefined)?.['name'] as
        | string
        | undefined) ?? '';
    if (!apiKey || !baseUrl) return null;

    // 尝试匹配已知 provider
    const preset = OPENAI_PROVIDERS.find((p) =>
      baseUrl.includes(new URL(p.baseUrl).hostname),
    );
    return {
      apiKey,
      baseUrl,
      model,
      providerName: preset?.name ?? 'Qwen Code',
    };
  } catch {
    return null;
  }
}
