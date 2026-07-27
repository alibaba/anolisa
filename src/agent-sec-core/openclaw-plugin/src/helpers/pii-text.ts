import { createHash } from "node:crypto";

type UnknownRecord = Record<string, unknown>;

function asRecord(value: unknown): UnknownRecord | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  return value as UnknownRecord;
}

function safeString(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function firstNonEmptyString(...values: unknown[]): string {
  for (const value of values) {
    const text = safeString(value);
    if (text.trim()) {
      return text;
    }
  }
  return "";
}

export function inboundPiiScanText(event: unknown): string {
  const record = asRecord(event);
  return firstNonEmptyString(
    record?.content,
    record?.body,
    record?.userInput,
    record?.user_input,
    record?.userPrompt,
    record?.user_prompt,
    record?.prompt,
    record?.llmInput,
    record?.llm_input,
  );
}

export function valueToText(value: unknown): string {
  if (value === undefined || value === null) {
    return "";
  }
  if (typeof value === "string") {
    return value;
  }
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

export function afterToolCallPiiScanText(event: unknown): string {
  const record = asRecord(event);
  const result = valueToText(record?.result);
  if (result.trim()) {
    return result;
  }
  return safeString(record?.error);
}

export function textSha256(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

export function piiScanInputSha256(text: string): string | undefined {
  if (!text.trim()) {
    return undefined;
  }
  return textSha256(text);
}
