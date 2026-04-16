/**
 * @license
 * Copyright 2025 Qwen
 * SPDX-License-Identifier: Apache-2.0
 */

import OpenAI from 'openai';
import type {
  ContentGenerator,
  ContentGeneratorConfig,
} from '../contentGenerator.js';
import type { Config } from '../../config/config.js';
import { OpenAIContentGenerator } from './openaiContentGenerator.js';
import {
  DashScopeOpenAICompatibleProvider,
  DeepSeekOpenAICompatibleProvider,
  ModelScopeOpenAICompatibleProvider,
  OpenRouterOpenAICompatibleProvider,
  type OpenAICompatibleProvider,
  DefaultOpenAICompatibleProvider,
} from './provider/index.js';

export { OpenAIContentGenerator } from './openaiContentGenerator.js';
export { ContentGenerationPipeline, type PipelineConfig } from './pipeline.js';

export {
  type OpenAICompatibleProvider,
  DashScopeOpenAICompatibleProvider,
  DeepSeekOpenAICompatibleProvider,
  OpenRouterOpenAICompatibleProvider,
} from './provider/index.js';

export { OpenAIContentConverter } from './converter.js';

/**
 * Create an OpenAI-compatible content generator with the appropriate provider
 */
export function createOpenAIContentGenerator(
  contentGeneratorConfig: ContentGeneratorConfig,
  cliConfig: Config,
): ContentGenerator {
  const provider = determineProvider(contentGeneratorConfig, cliConfig);
  return new OpenAIContentGenerator(
    contentGeneratorConfig,
    cliConfig,
    provider,
  );
}

/**
 * Determine the appropriate provider based on configuration
 */
export function determineProvider(
  contentGeneratorConfig: ContentGeneratorConfig,
  cliConfig: Config,
): OpenAICompatibleProvider {
  const config =
    contentGeneratorConfig || cliConfig.getContentGeneratorConfig();

  // Check for DashScope provider
  if (DashScopeOpenAICompatibleProvider.isDashScopeProvider(config)) {
    return new DashScopeOpenAICompatibleProvider(
      contentGeneratorConfig,
      cliConfig,
    );
  }

  if (DeepSeekOpenAICompatibleProvider.isDeepSeekProvider(config)) {
    return new DeepSeekOpenAICompatibleProvider(
      contentGeneratorConfig,
      cliConfig,
    );
  }

  // Check for OpenRouter provider
  if (OpenRouterOpenAICompatibleProvider.isOpenRouterProvider(config)) {
    return new OpenRouterOpenAICompatibleProvider(
      contentGeneratorConfig,
      cliConfig,
    );
  }

  // Check for ModelScope provider
  if (ModelScopeOpenAICompatibleProvider.isModelScopeProvider(config)) {
    return new ModelScopeOpenAICompatibleProvider(
      contentGeneratorConfig,
      cliConfig,
    );
  }

  // Default provider for standard OpenAI-compatible APIs
  return new DefaultOpenAICompatibleProvider(contentGeneratorConfig, cliConfig);
}

export { type ErrorHandler, EnhancedErrorHandler } from './errorHandler.js';

/**
 * Validate OpenAI API credentials and model availability by calling the /models endpoint.
 *
 * - Throws if the API key is invalid (HTTP 401).
 * - Throws if the configured model is not present in the returned models list.
 * - Silently passes for all other errors (e.g. network issues, providers that do not
 *   expose a /models endpoint) so that legitimate custom providers are not blocked.
 */
export async function validateOpenAICredentials(
  contentGeneratorConfig: ContentGeneratorConfig,
  cliConfig: Config,
): Promise<void> {
  const provider = determineProvider(contentGeneratorConfig, cliConfig);
  const client = provider.buildClient();

  let models: string[] | undefined;

  try {
    const response = await client.models.list();
    models = response.data.map((m) => m.id);
  } catch (error) {
    if (error instanceof OpenAI.APIError && error.status === 401) {
      throw new Error(
        'Invalid API key. Please check your API key and try again.',
      );
    }
    // For other errors (network issues, unsupported /models endpoint, etc.),
    // skip credential validation to avoid blocking legitimate custom providers.
    return;
  }

  // Validate that the configured model is available
  const model = contentGeneratorConfig.model;
  if (models && models.length > 0 && !models.includes(model)) {
    throw new Error(
      `Model "${model}" is not available with the provided credentials. Please verify the model name and try again.`,
    );
  }
}
