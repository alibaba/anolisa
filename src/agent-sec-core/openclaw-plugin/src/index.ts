// index.ts
import { definePluginEntry } from "openclaw/plugin-sdk/plugin-entry";
import type { SecurityCapability } from "./types.js";
import { toolGate } from "./capabilities/tool-gate.js";
import { codeScan } from "./capabilities/code-scan.js";
import { inboundFilter } from "./capabilities/inbound-filter.js";
import { promptScan } from "./capabilities/prompt-scan.js";
import { llmAudit } from "./capabilities/llm-audit.js";

const capabilities: SecurityCapability[] = [
  toolGate,
  codeScan,
  inboundFilter,
  promptScan,
  llmAudit,
];

export default definePluginEntry({
  id: "agent-sec",
  name: "Agent Security",
  description: "Security hooks powered by agent-sec-cli",
  register(api) {
    const cfg = (api.pluginConfig as Record<string, any>)?.capabilities ?? {};
    let count = 0;
    for (const cap of capabilities) {
      if (cfg[cap.id]?.enabled === false) {
        api.logger.info(`[agent-sec] skipped (disabled): ${cap.id}`);
        continue;
      }
      cap.register(api);
      count++;
      api.logger.info(`[agent-sec] registered: ${cap.id} -> [${cap.hooks.join(", ")}]`);
    }
    api.logger.info(`[agent-sec] ${count}/${capabilities.length} capabilities active`);
  },
});
