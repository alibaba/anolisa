import type { SecurityCapability } from "../types.js";
import { callAgentSecCli } from "../utils.js";

const CLI_TIMEOUT_MS = 10_000;
const MAX_EVIDENCE_ITEMS = 3;
const MAX_EVIDENCE_CHARS = 80;
const BEFORE_DISPATCH_PRIORITY = 200;

type PiiScanConfig = {
  scanUserInput: boolean;
  includeLowConfidence: boolean;
  enableBlock: boolean;
};

function readConfig(pluginConfig: Record<string, any>): PiiScanConfig {
  const capabilityConfig =
    pluginConfig.capabilities?.["pii-scan-user-input"] ?? {};
  return {
    scanUserInput: pluginConfig.piiScanUserInput !== false,
    includeLowConfidence: pluginConfig.piiIncludeLowConfidence === true,
    enableBlock: capabilityConfig.enableBlock === true,
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

function buildScanArgs(includeLowConfidence: boolean): string[] {
  const args = [
    "scan-pii",
    "--stdin",
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

function getInboundText(event: any): string {
  const content = typeof event?.content === "string" ? event.content : "";
  if (content.trim()) {
    return content;
  }
  return typeof event?.body === "string" ? event.body : "";
}

/**
 * 用户输入 PII / 凭据检测。
 *
 * Scans the current inbound user text before dispatch. When enableBlock is
 * true, a deny verdict handles the turn with a user-visible block message.
 */
export const piiScan: SecurityCapability = {
  id: "pii-scan-user-input",
  name: "PII Checker",
  hooks: ["before_dispatch"],
  register(api) {
    const cfg = readConfig((api.pluginConfig as Record<string, any>) ?? {});
    if (!cfg.scanUserInput) {
      api.logger.info("[pii-checker] piiScanUserInput=false, capability disabled");
      return;
    }

    api.on(
      "before_dispatch",
      async (event: any) => {
        try {
          const text = getInboundText(event);
          if (!text.trim()) {
            return undefined;
          }

          const result = await callAgentSecCli(buildScanArgs(cfg.includeLowConfidence), {
            timeout: CLI_TIMEOUT_MS,
            stdin: text,
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

          const warning = formatPiiWarning(
            verdict,
            findings,
            verdict === "deny" && cfg.enableBlock ? "本轮请求已被阻断。" : undefined,
          );
          api.logger.warn(
            `[pii-checker] ${verdict.toUpperCase()} (enableBlock=${cfg.enableBlock}) — ${warning}`,
          );
          if (verdict === "deny" && cfg.enableBlock) {
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
  },
};
