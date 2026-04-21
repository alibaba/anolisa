import type { SecurityCapability } from "../types.js";
import { callAgentSecCli } from "../utils.js";

/**
 * 用户输入 Prompt 注入 / 越狱检测。
 *
 * ## 当前防线：before_dispatch (priority 190)
 * 在用户消息进入系统的最早时机拦截，此时 event 只含用户原始输入，
 * 不包含工具输出或 RAG 内容，因此只覆盖用户侧的直接注入 / 越狱攻击。
 *
 * ## 后续待补：before_prompt_build（第二道防线）
 * prompt 组装完成后（用户输入 + 工具输出 + RAG 上下文全部拼入），
 * 再做一次 scan-prompt，可覆盖间接注入（tool output 投毒、RAG 投毒等）。
 * 届时独立为新能力 id="prompt-scan-full"，挂 before_prompt_build。
 *
 * CLI: agent-sec-cli scan-prompt --text <prompt> --mode standard --format json --source user_input
 */
export const promptScan: SecurityCapability = {
  id: "prompt-scan",
  name: "Prompt Injection Scanner",
  hooks: ["before_dispatch"],
  register(api) {
    api.on("before_dispatch", async (event: any, ctx: any) => {
      try {
        const text = String(event.content ?? event.body ?? "");
        if (!text.trim()) {
          return undefined;
        }

        const result = await callAgentSecCli(
          ["scan-prompt", "--text", text, "--mode", "standard", "--format", "json", "--source", "user_input"],
          { timeout: 10000 },
        );

        if (result.exitCode !== 0) {
          return undefined; // CLI 不可用 -> fail-open
        }

        const scanResult = JSON.parse(result.stdout);
        const verdict = scanResult.verdict;
        const findings: any[] = scanResult.findings ?? [];

        if (verdict === "pass" || findings.length === 0) {
          api.logger.info(`[prompt-scan] ✅ pass`);
          return undefined;
        }

        const descs = findings.map((f) => `- ${f.desc_zh ?? f.desc_en ?? ""}`);
        const msg = `[prompt-scan] Detected ${findings.length} issue(s):\n${descs.join("\n")}`;

        if (verdict === "deny") {
          api.logger.info(`[prompt-scan] 🚫 DENY — blocking user prompt`);
          return { handled: true, text: msg };
        }

        if (verdict === "warn") {
          api.logger.info(`[prompt-scan] ⚠️ WARN — requiring user approval`);
          return {
            requireApproval: {
              title: "Prompt Scanner Security Warning",
              description: msg,
              severity: "warning" as const,
            },
          };
        }

        return undefined;
      } catch {
        return undefined; // crash ≠ threat -> fail-open
      }
    }, { priority: 190 }); // 低于 inbound-filter (200)，用户消息通过通用过滤后再做注入检测
  },
};
