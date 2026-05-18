import type { OpenClawPluginApi } from "openclaw/plugin-sdk/plugin-entry";
import type {
  PluginHookAgentContext,
  PluginHookAgentEndEvent,
  PluginHookAfterToolCallEvent,
  PluginHookBeforeToolCallEvent,
  PluginHookLlmInputEvent,
  PluginHookLlmOutputEvent,
  PluginHookModelCallEndedEvent,
  PluginHookModelCallStartedEvent,
  PluginHookToolContext,
} from "openclaw/plugin-sdk/plugin-runtime";
import type { SecurityCapability } from "../types.js";
import { recordOpenClawObservability } from "../utils.js";
import type { CliResult } from "../utils.js";
import {
  OBSERVABILITY_HOOKS,
  type ObservabilityHookName,
} from "../helpers/observability/schema.js";
import { formatSafeError } from "../helpers/observability/helpers.js";
import { buildOpenClawObservabilityRecord } from "../helpers/observability/record.js";

export { buildOpenClawObservabilityRecord } from "../helpers/observability/record.js";

const OBSERVABILITY_PRIORITY = 1000;
const OBSERVABILITY_LATE_PRIORITY = -10_000;
const LOG_DETAIL_MAX_CHARS = 1000;

type ObservabilityHookEvent =
  | PluginHookLlmInputEvent
  | PluginHookLlmOutputEvent
  | PluginHookModelCallStartedEvent
  | PluginHookModelCallEndedEvent
  | PluginHookAgentEndEvent
  | PluginHookBeforeToolCallEvent
  | PluginHookAfterToolCallEvent;

type ObservabilityHookContext = PluginHookAgentContext | PluginHookToolContext;

export const observability: SecurityCapability = {
  id: "observability",
  name: "OpenClaw Observability",
  hooks: [...OBSERVABILITY_HOOKS],
  register(api) {
    api.on(
      "llm_input",
      (
        event: PluginHookLlmInputEvent,
        ctx: PluginHookAgentContext,
      ) => observeHook(api, "llm_input", event, ctx),
      { priority: OBSERVABILITY_PRIORITY },
    );
    api.on(
      "model_call_started",
      (
        event: PluginHookModelCallStartedEvent,
        ctx: PluginHookAgentContext,
      ) => observeHook(api, "model_call_started", event, ctx),
      { priority: OBSERVABILITY_PRIORITY },
    );
    api.on(
      "model_call_ended",
      (
        event: PluginHookModelCallEndedEvent,
        ctx: PluginHookAgentContext,
      ) => observeHook(api, "model_call_ended", event, ctx),
      { priority: OBSERVABILITY_PRIORITY },
    );
    api.on(
      "llm_output",
      (
        event: PluginHookLlmOutputEvent,
        ctx: PluginHookAgentContext,
      ) => observeHook(api, "llm_output", event, ctx),
      { priority: OBSERVABILITY_PRIORITY },
    );
    api.on(
      "agent_end",
      (
        event: PluginHookAgentEndEvent,
        ctx: PluginHookAgentContext,
      ) => observeHook(api, "agent_end", event, ctx),
      { priority: OBSERVABILITY_PRIORITY },
    );
    api.on(
      "before_tool_call",
      (
        event: PluginHookBeforeToolCallEvent,
        ctx: PluginHookToolContext,
      ) => observeHook(api, "before_tool_call", event, ctx),
      { priority: OBSERVABILITY_LATE_PRIORITY },
    );
    api.on(
      "after_tool_call",
      (
        event: PluginHookAfterToolCallEvent,
        ctx: PluginHookToolContext,
      ) => observeHook(api, "after_tool_call", event, ctx),
      { priority: OBSERVABILITY_PRIORITY },
    );
  },
};

function observeHook(
  api: OpenClawPluginApi,
  hookName: ObservabilityHookName,
  event: ObservabilityHookEvent,
  ctx: ObservabilityHookContext,
): void {
  try {
    const payload = buildOpenClawObservabilityRecord(hookName, event, ctx);
    if (payload === undefined) {
      return;
    }
    void recordOpenClawObservability(payload)
      .then((result) => {
        if (result.exitCode !== 0) {
          api.logger.warn?.(formatRecordFailure(hookName, payload.hook, result));
        }
      })
      .catch((error: unknown) => {
        api.logger.warn?.(
          `[observability] record error source_hook=${hookName} record_hook=${formatLogValue(payload.hook)} error=${formatLogError(error)}`,
        );
      });
  } catch (error) {
    api.logger.warn?.(`[observability] failed to build ${hookName} payload: ${formatSafeError(error)}`);
  }
}

function formatRecordFailure(
  sourceHook: ObservabilityHookName,
  recordHook: unknown,
  result: CliResult,
): string {
  const fields = [
    "[observability] record failed",
    `source_hook=${sourceHook}`,
    `record_hook=${formatLogValue(recordHook)}`,
    `exit=${result.exitCode}`,
  ];
  const stderr = formatLogValue(result.stderr);
  if (stderr) {
    fields.push(`stderr=${stderr}`);
  }
  const stdout = formatLogValue(result.stdout);
  if (stdout) {
    fields.push(`stdout=${stdout}`);
  }
  return fields.join(" ");
}

function formatLogError(error: unknown): string {
  if (error instanceof Error) {
    const message = formatLogValue(error.message);
    return message ? `${error.name}: ${message}` : error.name;
  }
  return `${typeof error}: ${formatLogValue(error)}`;
}

function formatLogValue(value: unknown): string {
  if (value === undefined || value === null) {
    return "";
  }
  const text = String(value).trim().replace(/\s+/g, " ");
  if (text.length <= LOG_DETAIL_MAX_CHARS) {
    return text;
  }
  return `${text.slice(0, LOG_DETAIL_MAX_CHARS)}...<truncated>`;
}
