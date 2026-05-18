import type { SecurityCapability } from "../types.js";
import { callAgentSecCli } from "../utils.js";

const DEFAULT_WARNING_TTL_MS = 300_000;
const CLI_TIMEOUT_MS = 10_000;
const MAX_EVIDENCE_ITEMS = 3;
const MAX_EVIDENCE_CHARS = 80;

type WarningBucket = {
  warnings: string[];
  createdAt: number;
  lastTouchedAt: number;
};

type PiiScanConfig = {
  scanUserInput: boolean;
  includeLowConfidence: boolean;
  warningTtlMs: number;
};

function readConfig(pluginConfig: Record<string, any>): PiiScanConfig {
  const ttl = Number(pluginConfig.piiWarningTtlMs);
  return {
    scanUserInput: pluginConfig.piiScanUserInput !== false,
    includeLowConfidence: pluginConfig.piiIncludeLowConfidence === true,
    warningTtlMs:
      Number.isFinite(ttl) && ttl >= 0 ? ttl : DEFAULT_WARNING_TTL_MS,
  };
}

function getRunId(event: any, ctx: any): string | undefined {
  const ctxRunId = typeof ctx?.runId === "string" ? ctx.runId.trim() : "";
  if (ctxRunId) {
    return ctxRunId;
  }
  const eventRunId = typeof event?.runId === "string" ? event.runId.trim() : "";
  return eventRunId || undefined;
}

function cleanupExpired(
  warningsByRun: Map<string, WarningBucket>,
  warningTtlMs: number,
): void {
  const now = Date.now();
  for (const [runId, bucket] of warningsByRun) {
    if (now - bucket.lastTouchedAt >= warningTtlMs) {
      warningsByRun.delete(runId);
    }
  }
}

function pushWarning(
  warningsByRun: Map<string, WarningBucket>,
  runId: string,
  warning: string,
  warningTtlMs: number,
): void {
  cleanupExpired(warningsByRun, warningTtlMs);
  const now = Date.now();
  const bucket =
    warningsByRun.get(runId) ??
    {
      warnings: [],
      createdAt: now,
      lastTouchedAt: now,
    };
  if (!bucket.warnings.includes(warning)) {
    bucket.warnings.push(warning);
  }
  bucket.lastTouchedAt = now;
  warningsByRun.set(runId, bucket);
}

function readWarnings(
  warningsByRun: Map<string, WarningBucket>,
  runId: string,
  warningTtlMs: number,
): string[] {
  cleanupExpired(warningsByRun, warningTtlMs);
  const bucket = warningsByRun.get(runId);
  if (!bucket) {
    return [];
  }
  return [...bucket.warnings];
}

function deleteWarnings(
  warningsByRun: Map<string, WarningBucket>,
  runId: string,
): void {
  warningsByRun.delete(runId);
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

function formatPiiWarning(verdict: string, findings: unknown[]): string {
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
  parts.push("本轮请求将继续处理。");
  return parts.join("；");
}

function buildScanArgs(prompt: string, includeLowConfidence: boolean): string[] {
  const args = [
    "scan-pii",
    "--text",
    prompt,
    "--format",
    "json",
    "--source",
    "user_input",
  ];
  if (includeLowConfidence) {
    args.push("--include-low-confidence");
  }
  return args;
}

/**
 * 用户输入 PII / 凭据检测。
 *
 * v1 only scans event.prompt in before_prompt_build and shows non-blocking
 * warnings by queueing a same-run block reply in reply_dispatch.
 */
export const piiScan: SecurityCapability = {
  id: "pii-scan-user-input",
  name: "PII Checker",
  hooks: ["before_prompt_build", "reply_dispatch"],
  register(api) {
    const cfg = readConfig((api.pluginConfig as Record<string, any>) ?? {});
    if (!cfg.scanUserInput) {
      api.logger.info("[pii-checker] piiScanUserInput=false, capability disabled");
      return;
    }

    const warningsByRun = new Map<string, WarningBucket>();

    api.on(
      "before_prompt_build",
      async (event: any, ctx: any) => {
        try {
          cleanupExpired(warningsByRun, cfg.warningTtlMs);

          const prompt = typeof event?.prompt === "string" ? event.prompt : "";
          if (!prompt.trim()) {
            return undefined;
          }

          const result = await callAgentSecCli(buildScanArgs(prompt, cfg.includeLowConfidence), {
            timeout: CLI_TIMEOUT_MS,
          });
          if (result.exitCode !== 0) {
            api.logger.warn(`[pii-checker] CLI failed: ${result.stderr || result.exitCode}`);
            return undefined;
          }

          const scanResult = JSON.parse(result.stdout) as {
            verdict?: unknown;
            findings?: unknown;
          };
          const verdict = safeString(scanResult.verdict) || "pass";
          const findings = Array.isArray(scanResult.findings)
            ? scanResult.findings
            : [];

          if (verdict === "pass" || findings.length === 0) {
            api.logger.info("[pii-checker] pass");
            return undefined;
          }

          if (verdict !== "warn" && verdict !== "deny") {
            return undefined;
          }

          const runId = getRunId(event, ctx);
          if (!runId) {
            api.logger.warn("[pii-checker] missing runId, warning not cached");
            return undefined;
          }

          const warning = formatPiiWarning(verdict, findings);
          pushWarning(warningsByRun, runId, warning, cfg.warningTtlMs);
          api.logger.warn(`[pii-checker] ${verdict.toUpperCase()} — warning cached for runId=${runId}`);
          return undefined;
        } catch (error) {
          api.logger.warn(`[pii-checker] failed open: ${error instanceof Error ? error.message : String(error)}`);
          return undefined;
        }
      },
      { priority: 0 },
    );

    api.on(
      "reply_dispatch",
      async (event: any, ctx: any) => {
        try {
          const runId = getRunId(event, ctx);
          if (!runId) {
            cleanupExpired(warningsByRun, cfg.warningTtlMs);
            return undefined;
          }

          if (event?.sendPolicy === "deny" || event?.suppressUserDelivery === true) {
            deleteWarnings(warningsByRun, runId);
            return undefined;
          }

          const warnings = readWarnings(warningsByRun, runId, cfg.warningTtlMs);
          if (warnings.length === 0) {
            return undefined;
          }

          const queued = ctx?.dispatcher?.sendBlockReply?.({
            text: warnings.join("\n"),
          });
          if (queued) {
            deleteWarnings(warningsByRun, runId);
          }
          return undefined;
        } catch (error) {
          api.logger.warn(`[pii-checker] reply_dispatch failed open: ${error instanceof Error ? error.message : String(error)}`);
          return undefined;
        }
      },
      { priority: 0 },
    );
  },
};
