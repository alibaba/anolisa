import type { SecurityCapability } from "./types.js";
import { envFlagEnabled } from "./utils.js";

const CAPABILITY_ENABLED_ENV: Record<string, string> = {
  "pii-scan-user-input": "PII_CHECKER_HOOK_ENABLED",
  "skill-ledger": "SKILL_LEDGER_HOOK_ENABLED",
};

export function isCapabilityEnabled(
  capability: SecurityCapability,
  config: Record<string, any>,
): boolean {
  const enabledEnv = CAPABILITY_ENABLED_ENV[capability.id];
  if (enabledEnv && process.env[enabledEnv] !== undefined) {
    return envFlagEnabled(enabledEnv, true);
  }
  const capabilityConfig = config[capability.id] ?? {};
  if (typeof capabilityConfig.enabled === "boolean") {
    return capabilityConfig.enabled;
  }
  return true;
}
