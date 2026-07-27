import type { SecurityCapability } from "./types.js";

export function isCapabilityEnabled(
  capability: SecurityCapability,
  config: Record<string, any>,
): boolean {
  const capabilityConfig = config[capability.id] ?? {};
  if (typeof capabilityConfig.enabled === "boolean") {
    return capabilityConfig.enabled;
  }
  return true;
}
