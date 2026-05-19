import type { SecurityCapability } from "../types.js";
import { callAgentSecCli } from "../utils.js";

export const codeScan: SecurityCapability = {
  id: "scan-code",
  name: "Code Scanner",
  hooks: ["before_tool_call"],
  register(api) {
    const cfg = (api.pluginConfig as Record<string, any>) ?? {};
    const requireApprovalEnabled = cfg.codeScanRequireApproval === true;

    api.on("before_tool_call", async (event: any, ctx: any) => {
      try {

        // 只拦截 shell 类工具
        const command = extractCommand(event);
        if (!command) {
          return undefined;
        }

        const result = await callAgentSecCli(
          ["scan-code", "--code", command, "--language", "bash"],
          { timeout: 10000 },
        );

        if (result.exitCode !== 0) {
          return undefined;
        }

        const scanResult = JSON.parse(result.stdout);
        const verdict = scanResult.verdict;
        const findings = scanResult.findings ?? [];

        if (verdict === "pass" || findings.length === 0) {
          api.logger.info(`[scan-code] ✅ pass — allowing command`);
          return undefined;
        }

        // 构建提示信息（与 cosh hook 的 msg 格式一致）
        const descs = findings.map((f: any) => `- ${f.desc_zh}`);
        const msg = `[code-scanner] Detected ${findings.length} issue(s):\n${descs.join("\n")}\n\nCommand: ${command}`;

        if (verdict === "deny") {
          api.logger.warn(`[scan-code] DENY (requireApproval=${requireApprovalEnabled}) — ${msg}`);
          if (requireApprovalEnabled) {
            return {
              requireApproval: {
                title: "Code Scanner Security Warning",
                description: msg,
                severity: "warning" as const,
              },
            };
          }
          return undefined;
        }

        if (verdict === "warn") {
          api.logger.warn(`[scan-code] WARN (requireApproval=${requireApprovalEnabled}) — ${msg}`);
          if (requireApprovalEnabled) {
            return {
              requireApproval: {
                title: "Code Scanner Security Warning",
                description: msg,
                severity: "warning" as const,
              },
            };
          }
          return undefined;
        }

        return undefined;
      } catch (err) {
        return undefined; // crash ≠ threat → allow
      }
    });
  },
};

/** 从 event 中提取 shell 命令，无法提取则返回 undefined */
function extractCommand(event: { toolName: string; params: Record<string, unknown> }): string | undefined {
  // OpenClaw 唯一的 shell 执行工具是 exec，参数字段为 command
  // 参考: https://docs.openclaw.ai/tools/exec
  if (event.toolName !== "exec") return undefined;
  const cmd = event.params.command;
  if (typeof cmd !== "string" || !cmd.trim()) return undefined;
  return cmd;
}
