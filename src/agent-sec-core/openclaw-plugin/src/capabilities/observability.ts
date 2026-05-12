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
import {
  OBSERVABILITY_HOOKS,
  type ObservabilityHookName,
} from "../helpers/observability/schema.js";
import { formatSafeError } from "../helpers/observability/helpers.js";
import { buildOpenClawObservabilityRecord } from "../helpers/observability/record.js";

export { buildOpenClawObservabilityRecord } from "../helpers/observability/record.js";

const OBSERVABILITY_PRIORITY = 1000;
const OBSERVABILITY_LATE_PRIORITY = -10_000;

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
          api.logger.debug?.(`[observability] observability record failed exit=${result.exitCode}`);
        }
      })
      .catch((error: unknown) => {
        api.logger.debug?.(`[observability] observability record error=${formatSafeError(error)}`);
      });
  } catch (error) {
    api.logger.debug?.(`[observability] failed to build ${hookName} payload: ${formatSafeError(error)}`);
  }
}
