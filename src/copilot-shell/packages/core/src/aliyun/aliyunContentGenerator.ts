/**
 * @license
 * Copyright 2026 Copilot Shell
 * SPDX-License-Identifier: Apache-2.0
 */

import type {
  GenerateContentParameters,
  GenerateContentResponse,
  CountTokensParameters,
  CountTokensResponse,
  EmbedContentParameters,
  EmbedContentResponse,
  Content,
  Part,
  FunctionDeclaration,
} from '@google/genai';
import { FinishReason } from '@google/genai';
import type {
  ContentGenerator,
  ContentGeneratorConfig,
} from '../core/contentGenerator.js';
import type { Config } from '../config/config.js';
import {
  loadAliyunCredentials,
  type AliyunCredentials,
} from './aliyunCredentials.js';
import * as SysomModule from '@alicloud/sysom20231230';
import { GenerateCopilotResponseRequest } from '@alicloud/sysom20231230';
import { $OpenApiUtil } from '@alicloud/openapi-core';
import * as $Util from '@alicloud/tea-util';

// Get the actual Client class from the module (handles CJS/ESM interop)
// When ESM imports CJS with default export, it may be nested in .default.default
function getSysomClientClass(): new (
  config: $OpenApiUtil.Config,
) => SysomClientInstance {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const mod = SysomModule as any;
  // Check for double-nested default (ESM importing CJS)
  if (
    mod.default &&
    typeof mod.default === 'object' &&
    typeof mod.default.default === 'function'
  ) {
    return mod.default.default;
  }
  // Check for single default
  if (typeof mod.default === 'function') {
    return mod.default;
  }
  // Fallback to module itself
  return mod;
}

// Aliyun SysOM API endpoint
const ALIYUN_SYSOM_ENDPOINT = 'sysom.cn-hangzhou.aliyuncs.com';

// Default model
const DEFAULT_MODEL = 'qwen3-coder-plus';

/**
 * Message format for Aliyun API
 */
interface AliyunMessage {
  role: 'system' | 'user' | 'assistant' | 'tool';
  content: string;
  tool_call_id?: string;
  name?: string;
  // For assistant messages with tool calls
  tool_calls?: Array<{
    id: string;
    type: 'function';
    function: {
      name: string;
      arguments: string;
    };
  }>;
}

/**
 * Tool format for Aliyun API
 */
interface AliyunTool {
  type: 'function';
  function: {
    name: string;
    description?: string;
    parameters?: Record<string, unknown>;
  };
}

/**
 * Request parameters for Aliyun API
 */
interface AliyunRequestParams {
  messages: AliyunMessage[];
  tools?: AliyunTool[];
  model: string;
  stream: boolean;
  use_dashscope?: boolean;
}

/**
 * Tool use item from Aliyun API (array format)
 */
interface AliyunToolUseItem {
  index: number;
  id: string;
  type: 'function';
  function: {
    name: string;
    arguments: string;
  };
}

/**
 * Response choice from Aliyun API
 */
interface AliyunResponseChoice {
  message: {
    content: string;
    tool_use?: AliyunToolUseItem[];
  };
}

/**
 * Response data from Aliyun API
 */
interface AliyunResponseData {
  choices: AliyunResponseChoice[];
}

/**
 * Non-stream response data from Aliyun API (also used in SSE stream with accumulated content)
 */
interface AliyunNonStreamResponseData {
  choices: AliyunResponseChoice[];
}

/**
 * Extract text from parts array
 */
function extractTextFromParts(parts: Part[] | undefined): string {
  if (!parts) return '';
  return parts
    .filter(
      (p): p is Part & { text: string } =>
        'text' in p && typeof (p as { text?: string }).text === 'string',
    )
    .map((p) => p.text)
    .join('');
}

/**
 * Convert contents to Content array
 */
function contentsToArray(
  contents: GenerateContentParameters['contents'],
): Content[] {
  if (!contents) return [];

  // If it's already an array of Content objects
  if (Array.isArray(contents)) {
    // Check if first element looks like Content (has role and parts)
    const first = contents[0];
    if (first && typeof first === 'object' && 'role' in first) {
      return contents as Content[];
    }
    // It might be an array of parts, wrap as single user content
    return [
      {
        role: 'user',
        parts: contents as Part[],
      },
    ];
  }

  // If it's a string, wrap as user content
  if (typeof contents === 'string') {
    return [{ role: 'user', parts: [{ text: contents }] }];
  }

  // If it's a single Content object
  if (typeof contents === 'object' && 'role' in contents) {
    return [contents as Content];
  }

  return [];
}

// Type for the Sysom client instance - using any due to SDK type export issues
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type SysomClientInstance = any;

/**
 * Aliyun Content Generator that uses @alicloud/sysom20231230 SDK
 */
export class AliyunContentGenerator implements ContentGenerator {
  private client: SysomClientInstance;
  private runtime: $Util.RuntimeOptions;
  private contentGeneratorConfig: ContentGeneratorConfig;

  constructor(
    credentials: AliyunCredentials,
    contentGeneratorConfig: ContentGeneratorConfig,
    _cliConfig: Config,
  ) {
    this.contentGeneratorConfig = contentGeneratorConfig;

    // Initialize Aliyun client using OpenAPI Config
    const config = new $OpenApiUtil.Config({
      accessKeyId: credentials.accessKeyId,
      accessKeySecret: credentials.accessKeySecret,
    });
    config.endpoint = ALIYUN_SYSOM_ENDPOINT;

    const SysomClient = getSysomClientClass();
    this.client = new SysomClient(config);

    // Setup runtime options
    this.runtime = new $Util.RuntimeOptions({});
    this.runtime.readTimeout = 180000;
    this.runtime.connectTimeout = 180000;
  }

  /**
   * Convert GenerateContentParameters to Aliyun format
   */
  private convertToAliyunFormat(
    request: GenerateContentParameters,
  ): AliyunRequestParams {
    const messages: AliyunMessage[] = [];

    // Convert contents to messages
    const contentsList = contentsToArray(request.contents);
    for (const content of contentsList) {
      if (content.role === 'model') {
        // Gemini 'model' role maps to 'assistant'
        // Check if there are function calls in this message
        const functionCalls = content.parts?.filter(
          (p) => 'functionCall' in p && p.functionCall,
        );
        const textContent = extractTextFromParts(content.parts);

        if (functionCalls && functionCalls.length > 0) {
          // Assistant message with tool calls
          const toolCalls = functionCalls.map((p) => {
            const fc = (
              p as {
                functionCall: {
                  id?: string;
                  name: string;
                  args?: Record<string, unknown>;
                };
              }
            ).functionCall;
            return {
              id: fc.id || `call_${Math.random().toString(36).slice(2)}`,
              type: 'function' as const,
              function: {
                name: fc.name,
                arguments: JSON.stringify(fc.args || {}),
              },
            };
          });
          messages.push({
            role: 'assistant',
            content: textContent || '',
            tool_calls: toolCalls,
          });
        } else {
          messages.push({
            role: 'assistant',
            content: textContent,
          });
        }
      } else if (content.role === 'user') {
        // Check if there are function responses in this message
        const functionResponses = content.parts?.filter(
          (p) => 'functionResponse' in p && p.functionResponse,
        );

        if (functionResponses && functionResponses.length > 0) {
          // Convert function responses to tool messages
          for (const part of functionResponses) {
            const fr = (
              part as {
                functionResponse: {
                  id?: string;
                  name: string;
                  response: unknown;
                };
              }
            ).functionResponse;
            messages.push({
              role: 'tool',
              tool_call_id: fr.id || fr.name,
              name: fr.name,
              content:
                typeof fr.response === 'string'
                  ? fr.response
                  : JSON.stringify(fr.response),
            });
          }
        } else {
          messages.push({
            role: 'user',
            content: extractTextFromParts(content.parts),
          });
        }
      }
    }

    // Add system instruction if present (from config)
    const systemInstruction = request.config?.systemInstruction;
    if (systemInstruction) {
      let systemText = '';
      if (typeof systemInstruction === 'string') {
        systemText = systemInstruction;
      } else if (
        systemInstruction &&
        typeof systemInstruction === 'object' &&
        'parts' in systemInstruction
      ) {
        systemText = extractTextFromParts((systemInstruction as Content).parts);
      }
      if (systemText) {
        messages.unshift({
          role: 'system',
          content: systemText,
        });
      }
    }

    // Convert tools (from config)
    // Respect functionCallingConfig mode and allowedFunctionNames (BeforeToolSelection hook support)
    const rawConfig = request.config as Record<string, unknown> | undefined;
    const functionCallingConfig = rawConfig?.['functionCallingConfig'] as
      | Record<string, unknown>
      | undefined;
    const callingMode = functionCallingConfig?.['mode'] as string | undefined;
    const allowedFunctionNames = functionCallingConfig?.[
      'allowedFunctionNames'
    ] as string[] | undefined;
    // If mode=NONE, the tool-building block below is skipped entirely.
    // Otherwise, build allowedSet from allowedFunctionNames for name-based filtering.
    const allowedSet =
      callingMode === 'NONE'
        ? null
        : allowedFunctionNames && allowedFunctionNames.length > 0
          ? new Set(allowedFunctionNames)
          : null; // null = no name filter (pass all tools)

    let tools: AliyunTool[] | undefined;
    const requestTools = request.config?.tools;
    if (
      callingMode !== 'NONE' &&
      requestTools &&
      Array.isArray(requestTools) &&
      requestTools.length > 0
    ) {
      tools = [];
      for (const tool of requestTools) {
        if (
          tool &&
          typeof tool === 'object' &&
          'functionDeclarations' in tool
        ) {
          const funcDecls = (
            tool as {
              functionDeclarations?: Array<{
                name: string;
                description?: string;
                parameters?: unknown;
              }>;
            }
          ).functionDeclarations;
          if (funcDecls) {
            for (const func of funcDecls) {
              // Skip functions not in the allowed list (if restriction is active)
              if (allowedSet && !allowedSet.has(func.name)) continue;
              // Handle both Gemini tools (parameters) and MCP tools (parametersJsonSchema)
              let parameters: Record<string, unknown> | undefined;

              // Type assertion to access parametersJsonSchema property
              const funcWithJsonSchema = func as FunctionDeclaration & {
                parametersJsonSchema?: unknown;
              };

              if (funcWithJsonSchema.parametersJsonSchema) {
                // MCP tool format - use parametersJsonSchema directly
                parameters = funcWithJsonSchema.parametersJsonSchema as Record<
                  string,
                  unknown
                >;
              } else if (func.parameters) {
                // Gemini tool format - use parameters directly
                parameters = func.parameters as Record<string, unknown>;
              }

              tools.push({
                type: 'function',
                function: {
                  name: func.name,
                  description: func.description,
                  parameters,
                },
              });
            }
          }
        }
      }
    }

    return {
      messages,
      tools: tools && tools.length > 0 ? tools : undefined,
      model:
        request.model || this.contentGeneratorConfig.model || DEFAULT_MODEL,
      stream: false,
      use_dashscope: true,
    };
  }

  /**
   * Convert Aliyun response to GenerateContentResponse
   */
  private convertFromAliyunFormat(
    responseData: AliyunResponseData,
  ): GenerateContentResponse {
    const choice = responseData.choices?.[0];
    if (!choice) {
      return {
        candidates: [
          {
            content: { parts: [{ text: '' }], role: 'model' },
            finishReason: FinishReason.STOP,
          },
        ],
      } as GenerateContentResponse;
    }

    const message = choice.message;
    const parts: Array<{
      text?: string;
      functionCall?: { name: string; args: Record<string, unknown> };
    }> = [];

    // Add text content
    if (message.content) {
      parts.push({ text: message.content });
    }

    // Add tool calls (tool_use is an array)
    if (message.tool_use && Array.isArray(message.tool_use)) {
      for (const toolCall of message.tool_use) {
        try {
          parts.push({
            functionCall: {
              id: toolCall.id,
              name: toolCall.function.name,
              args: JSON.parse(toolCall.function.arguments || '{}'),
            },
            // eslint-disable-next-line @typescript-eslint/no-explicit-any
          } as any);
        } catch {
          console.warn(
            'Failed to parse tool call arguments:',
            toolCall.function.arguments,
          );
        }
      }
    }

    return {
      candidates: [
        {
          content: { parts, role: 'model' },
          finishReason: FinishReason.STOP,
        },
      ],
    } as GenerateContentResponse;
  }

  async generateContent(
    request: GenerateContentParameters,
    _userPromptId: string,
  ): Promise<GenerateContentResponse> {
    const requestParams = this.convertToAliyunFormat(request);
    const headers: Record<string, string> = {
      'content-type': 'application/json',
    };
    const aliyunRequest = new GenerateCopilotResponseRequest({
      llmParamString: JSON.stringify(requestParams),
    });

    try {
      const response = await this.client.generateCopilotResponseWithOptions(
        aliyunRequest,
        headers,
        this.runtime,
      );

      if (response.body?.data) {
        const responseData = JSON.parse(
          response.body.data,
        ) as AliyunResponseData;
        return this.convertFromAliyunFormat(responseData);
      }

      return {
        candidates: [
          {
            content: {
              parts: [{ text: 'Empty response from Aliyun API' }],
              role: 'model',
            },
            finishReason: FinishReason.STOP,
          },
        ],
      } as GenerateContentResponse;
    } catch (error) {
      const errorMessage =
        error instanceof Error ? error.message : String(error);
      throw new Error(`Aliyun API error: ${errorMessage}`);
    }
  }

  async generateContentStream(
    request: GenerateContentParameters,
    _userPromptId: string,
  ): Promise<AsyncGenerator<GenerateContentResponse>> {
    const requestParams = this.convertToAliyunFormat(request);
    // Enable streaming in request params
    requestParams.stream = true;

    const headers: Record<string, string> = {
      'content-type': 'application/json',
    };

    const client = this.client;
    const runtime = this.runtime;

    // Build request for low-level SSE API call
    // We bypass generateCopilotStreamResponseWithSSE because SDK's $dara.cast
    // filters out the 'choices' field (only keeps code/data/message/requestId)
    const req = new $OpenApiUtil.OpenApiRequest({
      headers,
      body: { llmParamString: JSON.stringify(requestParams) },
    });
    const params = new $OpenApiUtil.Params({
      action: 'GenerateCopilotStreamResponse',
      version: '2023-12-30',
      protocol: 'HTTPS',
      pathname: '/api/v1/copilot/generate_copilot_stream_response',
      method: 'POST',
      authType: 'AK',
      style: 'ROA',
      reqBodyType: 'json',
      bodyType: 'json',
    });

    async function* streamGenerator(): AsyncGenerator<GenerateContentResponse> {
      let hasYieldedFinishReason = false;
      let lastContent = ''; // Track accumulated content to compute delta
      // Track tool calls: id -> last arguments length (to detect when complete)
      const yieldedToolCalls = new Set<string>();
      let lastToolUse: AliyunToolUseItem[] = [];

      try {
        // Use callSSEApi directly to get raw SSE events
        const sseStream = await client.callSSEApi(params, req, runtime);

        for await (const resp of sseStream) {
          // resp.event contains the raw SSE event data
          const eventData = resp.event?.data;
          if (!eventData) continue;

          try {
            // Parse the SSE event data
            // Format: {"choices": [{"message": {"content": "累积内容", "tool_use": [...]}}]}
            const streamData = JSON.parse(
              eventData,
            ) as AliyunNonStreamResponseData;
            const choice = streamData.choices?.[0];
            if (!choice?.message) continue;

            const parts: Array<{
              text?: string;
              functionCall?: {
                name: string;
                args: Record<string, unknown>;
              };
            }> = [];

            // API returns accumulated content, compute delta
            const fullContent = choice.message.content || '';
            if (fullContent.length > lastContent.length) {
              const deltaContent = fullContent.slice(lastContent.length);
              if (deltaContent) {
                parts.push({ text: deltaContent });
              }
              lastContent = fullContent;
            }

            // Store latest tool_use for final processing
            if (
              choice.message.tool_use &&
              Array.isArray(choice.message.tool_use)
            ) {
              lastToolUse = choice.message.tool_use;
            }

            // Yield text parts immediately
            if (parts.length > 0) {
              yield {
                candidates: [
                  {
                    content: { parts, role: 'model' },
                    finishReason: undefined,
                  },
                ],
              } as GenerateContentResponse;
            }
          } catch {
            // Skip chunks that can't be parsed
          }
        }

        // Stream ended, process tool calls from the final state
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const toolParts: Array<{ functionCall: any }> = [];

        for (const toolCall of lastToolUse) {
          if (yieldedToolCalls.has(toolCall.id)) continue;
          try {
            const args = JSON.parse(toolCall.function.arguments || '{}');
            toolParts.push({
              functionCall: {
                id: toolCall.id,
                name: toolCall.function.name,
                args,
              },
            });
            yieldedToolCalls.add(toolCall.id);
          } catch {
            // Skip invalid tool call arguments
          }
        }

        // Yield tool calls if any
        if (toolParts.length > 0) {
          // Extract functionCall objects for the functionCalls property
          const functionCallsArray = toolParts.map((p) => p.functionCall);
          yield {
            candidates: [
              {
                content: { parts: toolParts, role: 'model' },
                finishReason: undefined,
              },
            ],
            // Add functionCalls property directly since we're not using the real GenerateContentResponse class
            functionCalls: functionCallsArray,
          } as GenerateContentResponse;
        }

        // Yield final chunk with finishReason
        if (!hasYieldedFinishReason) {
          hasYieldedFinishReason = true;
          yield {
            candidates: [
              {
                content: { parts: [] as Part[], role: 'model' },
                finishReason: FinishReason.STOP,
              },
            ],
          } as GenerateContentResponse;
        }
      } catch (error) {
        const errorMessage =
          error instanceof Error ? error.message : String(error);
        throw new Error(`Aliyun streaming API error: ${errorMessage}`);
      }
    }

    return streamGenerator();
  }

  async countTokens(
    _request: CountTokensParameters,
  ): Promise<CountTokensResponse> {
    // Aliyun doesn't have a direct token counting API
    // Return an estimate based on character count
    return {
      totalTokens: 0,
    } as CountTokensResponse;
  }

  async embedContent(
    _request: EmbedContentParameters,
  ): Promise<EmbedContentResponse> {
    // Aliyun embedding is not implemented in this version
    throw new Error('Embedding is not supported by Aliyun provider');
  }

  useSummarizedThinking(): boolean {
    return false;
  }
}

/**
 * Create an Aliyun Content Generator
 */
export async function createAliyunContentGenerator(
  contentGeneratorConfig: ContentGeneratorConfig,
  config: Config,
): Promise<AliyunContentGenerator> {
  const credentials = await loadAliyunCredentials();
  if (!credentials) {
    throw new Error(
      'Aliyun credentials not found. Please use /auth to configure your Access Key ID and Secret.',
    );
  }

  return new AliyunContentGenerator(
    credentials,
    contentGeneratorConfig,
    config,
  );
}
