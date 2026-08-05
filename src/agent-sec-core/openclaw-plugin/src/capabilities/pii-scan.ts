import type { SecurityCapability } from "../types.js";
import {
  afterToolCallPiiScanText,
  inboundPiiScanText,
  valueToText,
} from "../helpers/pii-text.js";
import {
  buildTraceContext,
  callAgentSecCli,
  envFlagEnabled,
  envHookPolicy,
  isHookPolicyValue,
  normalizeHookPolicy,
  type HookPolicy,
} from "../utils.js";

const CLI_TIMEOUT_MS = 10_000;
const MAX_EVIDENCE_ITEMS = 3;
const MAX_EVIDENCE_CHARS = 80;
const BEFORE_DISPATCH_PRIORITY = 200;

type PiiScanConfig = {
  scanUserInput: boolean;
  includeLowConfidence: boolean;
  policy: HookPolicy;
};

function readConfig(pluginConfig: Record<string, any>, api: any): PiiScanConfig {
  const capabilityConfig =
    pluginConfig.capabilities?.["pii-scan-user-input"] ?? {};
  let policy: HookPolicy = "observe";
  if (process.env.PII_CHECKER_MODE !== undefined) {
    policy = envHookPolicy("PII_CHECKER_MODE", "observe");
    if (!isHookPolicyValue(process.env.PII_CHECKER_MODE)) {
      api.logger.warn("[pii-checker] invalid PII_CHECKER_MODE; using observe");
    }
  } else if (typeof capabilityConfig.policy === "string") {
    policy = normalizeHookPolicy(capabilityConfig.policy, "observe");
    if (!isHookPolicyValue(capabilityConfig.policy)) {
      api.logger.warn(
        `[pii-checker] invalid capability policy="${capabilityConfig.policy}"; using observe`,
      );
    }
  } else if (typeof capabilityConfig.enableBlock === "boolean") {
    policy = capabilityConfig.enableBlock ? "block" : "warn";
  }
  return {
    scanUserInput:
      process.env.PII_CHECKER_HOOK_ENABLED !== undefined
        ? envFlagEnabled("PII_CHECKER_HOOK_ENABLED", true)
        : pluginConfig.piiScanUserInput !== false,
    includeLowConfidence: pluginConfig.piiIncludeLowConfidence === true,
    policy,
  };
}

function shorten(value: string, limit = MAX_EVIDENCE_CHARS): string {
  const normalized = value.replace(/\s+/g, " ").trim();
  if (normalized.length <= limit) {
    return normalized;
  }
  return `${normalized.slice(0, limit - 1)}…`;
}

function safeString(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function formatPiiWarning(
  verdict: string,
  findings: unknown[],
  finalMessage = "本轮请求将继续处理。",
): string {
  const typedFindings = findings.filter(
    (finding): finding is Record<string, unknown> =>
      typeof finding === "object" && finding !== null && !Array.isArray(finding),
  );
  const piiTypes = Array.from(
    new Set(
      typedFindings
        .map((finding) => safeString(finding.type))
        .filter((value) => value.length > 0),
    ),
  ).sort();
  const severities = Array.from(
    new Set(
      typedFindings
        .map((finding) => safeString(finding.severity))
        .filter((value) => value.length > 0),
    ),
  ).sort();
  const evidence = typedFindings
    .map((finding) => safeString(finding.evidence_redacted))
    .filter((value, index, arr) => value.length > 0 && arr.indexOf(value) === index)
    .slice(0, MAX_EVIDENCE_ITEMS)
    .map((value) => shorten(value));

  const risk = verdict === "deny" ? "高风险敏感信息" : "敏感信息";
  const parts = [
    `[pii-checker] 检测到 ${typedFindings.length} 项${risk}`,
    `类型：${piiTypes.length > 0 ? piiTypes.join(", ") : "unknown"}`,
  ];
  if (severities.length > 0) {
    parts.push(`严重级别：${severities.join(", ")}`);
  }
  if (evidence.length > 0) {
    parts.push(`脱敏示例：${evidence.join(", ")}`);
  }
  parts.push(finalMessage);
  return parts.join("；");
}

function buildScanArgs(source: string, includeLowConfidence: boolean): string[] {
  const args = [
    "scan-pii",
    "--stdin",
    "--format",
    "json",
    "--redact-output",
    "--source",
    source,
  ];
  if (includeLowConfidence) {
    args.push("--include-low-confidence");
  }
  return args;
}

function getInboundText(event: any): string {
  return inboundPiiScanText(event);
}

function getModelOutputText(event: any): string {
  const response = safeString(event?.response);
  if (response.trim()) {
    return response;
  }
  const lastAssistant = safeString(event?.lastAssistant ?? event?.last_assistant);
  if (lastAssistant.trim()) {
    return lastAssistant;
  }
  const assistantTexts = Array.isArray(event?.assistantTexts)
    ? event.assistantTexts
    : Array.isArray(event?.assistant_texts)
      ? event.assistant_texts
      : [];
  return assistantTexts.filter((item: unknown) => typeof item === "string").join("\n");
}

function getToolOutputText(event: any): string {
  return afterToolCallPiiScanText(event);
}

async function scanPiiText(
  api: any,
  cfg: PiiScanConfig,
  event: any,
  ctx: any,
  text: string,
  source: string,
): Promise<{ verdict: string; findings: unknown[] } | undefined> {
  const result = await callAgentSecCli(buildScanArgs(source, cfg.includeLowConfidence), {
    timeout: CLI_TIMEOUT_MS,
    stdin: text,
    traceContext: buildTraceContext(event, ctx),
  });
  if (result.exitCode !== 0) {
    api.logger.warn(`[pii-checker] CLI failed: ${result.stderr || result.exitCode}`);
    return undefined;
  }

  let scanResult: { verdict?: unknown; findings?: unknown };
  try {
    scanResult = JSON.parse(result.stdout) as {
      verdict?: unknown;
      findings?: unknown;
    };
  } catch (error) {
    api.logger.warn(
      `[pii-checker] CLI returned invalid JSON, failed open: ${
        error instanceof Error ? error.message : String(error)
      }`,
    );
    return undefined;
  }
  return {
    verdict: safeString(scanResult.verdict) || "pass",
    findings: Array.isArray(scanResult.findings) ? scanResult.findings : [],
  };
}

function logPiiWarning(
  api: any,
  verdict: string,
  findings: unknown[],
  cfg: PiiScanConfig,
  finalMessage?: string,
): string {
  const warning = formatPiiWarning(verdict, findings, finalMessage);
  api.logger.warn(
    `[pii-checker] ${verdict.toUpperCase()} (policy=${cfg.policy}) — ${warning}`,
  );
  return warning;
}

/**
 * 用户输入 PII / 凭据检测。
 *
 * Scans PII at the boundaries exposed by OpenClaw and applies the configured
 * policy. Unsupported confirmation or post-action boundaries fail open with a warning.
 */
export const piiScan: SecurityCapability = {
  id: "pii-scan-user-input",
  name: "PII Checker",
  hooks: ["before_dispatch", "before_tool_call", "after_tool_call", "llm_output"],
  register(api) {
    if (!envFlagEnabled("PII_CHECKER_HOOK_ENABLED", true)) {
      return;
    }
    const cfg = readConfig((api.pluginConfig as Record<string, any>) ?? {}, api);
    if (!cfg.scanUserInput) {
      api.logger.info("[pii-checker] piiScanUserInput=false, capability disabled");
      return;
    }

    api.on(
      "before_dispatch",
      async (event: any, ctx: any) => {
        try {
          const text = getInboundText(event);
          if (!text.trim()) {
            return undefined;
          }

          const scanResult = await scanPiiText(api, cfg, event, ctx, text, "user_input");
          if (scanResult === undefined) return undefined;
          const { verdict, findings } = scanResult;

          if (verdict === "pass" || findings.length === 0) {
            api.logger.info("[pii-checker] pass");
            return undefined;
          }

          if (verdict !== "warn" && verdict !== "deny") {
            return undefined;
          }

          if (cfg.policy === "observe") return undefined;
          const warning = logPiiWarning(
            api,
            verdict,
            findings,
            cfg,
            verdict === "deny" && cfg.policy === "block" ? "本轮请求已被阻断。" : undefined,
          );
          if (verdict === "deny" && cfg.policy === "block") {
            return {
              handled: true,
              text: warning,
            };
          }
          return undefined;
        } catch (error) {
          api.logger.warn(
            `[pii-checker] failed open: ${error instanceof Error ? error.message : String(error)}`,
          );
          return undefined;
        }
      },
      { priority: BEFORE_DISPATCH_PRIORITY },
    );

    api.on(
      "before_tool_call",
      async (event: any, ctx: any) => {
        try {
          const text = valueToText(event?.params ?? event?.parameters ?? event?.args);
          if (!text.trim()) return undefined;
          const scanResult = await scanPiiText(api, cfg, event, ctx, text, "tool_input");
          if (scanResult === undefined) return undefined;
          const { verdict, findings } = scanResult;
          if (verdict === "pass" || findings.length === 0) return undefined;
          if (verdict !== "warn" && verdict !== "deny") return undefined;
          if (cfg.policy === "observe") return undefined;
          const warning = logPiiWarning(
            api,
            verdict,
            findings,
            cfg,
            verdict === "deny" && cfg.policy === "block"
              ? "本次工具调用已被阻断。"
              : undefined,
          );
          if (verdict === "deny" && cfg.policy === "ask") {
            return {
              requireApproval: {
                title: "PII Checker Security Review",
                description: warning,
                severity: "critical",
              },
            };
          }
          if (verdict === "deny" && cfg.policy === "block") {
            return { block: true, blockReason: warning };
          }
          return undefined;
        } catch (error) {
          api.logger.warn(
            `[pii-checker] failed open: ${error instanceof Error ? error.message : String(error)}`,
          );
          return undefined;
        }
      },
      { priority: BEFORE_DISPATCH_PRIORITY },
    );

    api.on("after_tool_call", async (event: any, ctx: any) => {
      try {
        const text = getToolOutputText(event);
        if (!text.trim()) return undefined;
        const scanResult = await scanPiiText(api, cfg, event, ctx, text, "tool_output");
        if (scanResult === undefined) return undefined;
        const { verdict, findings } = scanResult;
        if (verdict === "pass" || findings.length === 0) return undefined;
        if (verdict !== "warn" && verdict !== "deny") return undefined;
        if (cfg.policy !== "observe") logPiiWarning(api, verdict, findings, cfg);
        return undefined;
      } catch (error) {
        api.logger.warn(
          `[pii-checker] failed open: ${error instanceof Error ? error.message : String(error)}`,
        );
        return undefined;
      }
    });

    api.on("llm_output", async (event: any, ctx: any) => {
      try {
        const text = getModelOutputText(event);
        if (!text.trim()) return undefined;
        const scanResult = await scanPiiText(api, cfg, event, ctx, text, "model_output");
        if (scanResult === undefined) return undefined;
        const { verdict, findings } = scanResult;
        if (verdict === "pass" || findings.length === 0) return undefined;
        if (verdict !== "warn" && verdict !== "deny") return undefined;
        if (cfg.policy !== "observe") logPiiWarning(api, verdict, findings, cfg);
        return undefined;
      } catch (error) {
        api.logger.warn(
          `[pii-checker] failed open: ${error instanceof Error ? error.message : String(error)}`,
        );
        return undefined;
      }
    });
  },
};
