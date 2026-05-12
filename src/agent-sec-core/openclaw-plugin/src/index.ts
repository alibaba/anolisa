// index.ts
import { definePluginEntry } from "openclaw/plugin-sdk/plugin-entry";
import type { SecurityCapability } from "./types.js";
import { codeScan } from "./capabilities/code-scan.js";
import { observability } from "./capabilities/observability.js";
import { promptScan } from "./capabilities/prompt-scan.js";
import { skillLedger } from "./capabilities/skill-ledger.js";

const capabilities: SecurityCapability[] = [
  codeScan,
  promptScan,
  skillLedger,
  observability,
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
