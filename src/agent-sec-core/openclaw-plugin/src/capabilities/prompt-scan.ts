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
          api.logger.info(`[prompt-scan] pass`);
          return undefined;
        }

        const summary: string = scanResult.summary ?? "";
        const threatType: string = scanResult.threat_type ?? "";
        const msg = `[prompt-scan] ${summary || threatType || "Prompt rejected by security policy"}`;

        if (verdict === "deny") {
          api.logger.warn(`[prompt-scan] DENY — ${msg}`);
          // handled: true + text → text sent as final reply, LLM call skipped
          // handled: false + text → text ignored, event passes through to LLM
          // Set PROMPT_SCAN_BLOCK=1 to enable blocking mode.
          const blockEnabled = process.env["PROMPT_SCAN_BLOCK"] === "1";
          return { handled: blockEnabled, text: msg };
        }

        if (verdict === "warn") {
          api.logger.warn(`[prompt-scan] WARN — passing user prompt with warning`);
          return { handled: false, text: `[Security Warning] ${msg}` };
        }

        return undefined;
      } catch {
        return undefined; // crash ≠ threat -> fail-open
      }
    }, { priority: 190 });
  },
};
