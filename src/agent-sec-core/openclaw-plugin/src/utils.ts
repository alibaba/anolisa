import { execFile } from "node:child_process";

export type CliResult = {
  /** Raw stdout text (may be empty) */
  stdout: string;
  /** Raw stderr text (may be empty) */
  stderr: string;
  /** Process exit code (0 = success) */
  exitCode: number;
};

export type CliCallOptions = {
  timeout?: number;
  stdin?: string;
  traceContext?: TraceContext;
};

export type TraceContext = {
  agent_name?: string;
  trace_id?: string;
  session_id?: string;
  run_id?: string;
  call_id?: string;
  tool_call_id?: string;
};

type UnknownRecord = Record<string, unknown>;

type TraceFieldSpec = {
  outputKey: keyof TraceContext;
  inputKeys: string[];
};

const TRACE_FIELD_SPECS: TraceFieldSpec[] = [
  { outputKey: "trace_id", inputKeys: ["trace_id", "traceId"] },
  { outputKey: "session_id", inputKeys: ["session_id", "sessionId"] },
  { outputKey: "run_id", inputKeys: ["run_id", "runId"] },
  { outputKey: "call_id", inputKeys: ["call_id", "callId"] },
  {
    outputKey: "tool_call_id",
    inputKeys: ["tool_call_id", "toolCallId", "tool_use_id", "toolUseId"],
  },
];

function asRecord(value: unknown): UnknownRecord | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  return value as UnknownRecord;
}

function traceValue(record: UnknownRecord | undefined, keys: string[]): string | undefined {
  if (!record) {
    return undefined;
  }

  for (const key of keys) {
    const value = record[key];
    if (typeof value === "string" && value.trim()) {
      return value.trim();
    }
  }
  return undefined;
}

export function buildTraceContext(event: unknown, ctx: unknown): TraceContext {
  const eventRecord = asRecord(event);
  const ctxRecord = asRecord(ctx);
  const sources = [eventRecord, ctxRecord];
  const traceContext: TraceContext = { agent_name: "openclaw" };

  for (const spec of TRACE_FIELD_SPECS) {
    for (const source of sources) {
      const value = traceValue(source, spec.inputKeys);
      if (value !== undefined) {
        traceContext[spec.outputKey] = value;
        break;
      }
    }
  }

  return traceContext;
}

// ---------------------------------------------------------------------------
// Test-only mock support
// ---------------------------------------------------------------------------
type CliMockFn = (args: string[], opts: CliCallOptions) => Promise<CliResult>;

let _mockFn: CliMockFn | undefined;

/** Test-only: override callAgentSecCli with a mock function. */
export function _setCliMock(fn: CliMockFn): void {
  _mockFn = fn;
}

/** Test-only: remove mock and restore real CLI execution. */
export function _resetCliMock(): void {
  _mockFn = undefined;
}

/**
 * Execute an agent-sec-cli subcommand and return the raw output.
 * Each capability is responsible for parsing stdout on its own.
 */
export async function callAgentSecCli(
  args: string[],
  opts: CliCallOptions = {},
): Promise<CliResult> {
  const finalArgs =
    opts.traceContext && Object.keys(opts.traceContext).length > 0
      ? ["--trace-context", JSON.stringify(opts.traceContext), ...args]
      : args;

  // If a mock is active, delegate to it instead of spawning a real process.
  if (_mockFn) {
    return _mockFn(finalArgs, opts);
  }

  const timeout = opts.timeout ?? 5000;

  return new Promise((resolve) => {
    const child = execFile(
      "agent-sec-cli",
      finalArgs,
      { timeout, maxBuffer: 1024 * 1024, encoding: "utf8" },
      (error, stdout, stderr) => {
        // Fail-open: Never reject. Always resolve with error status.
        // Capabilities check exitCode !== 0 to handle CLI failures gracefully.

        // Timeout: execFile sets error.killed = true
        if (error && error.killed) {
          resolve({
            stdout: "",
            stderr: `agent-sec-cli timed out after ${timeout}ms`,
            exitCode: 124, // Standard timeout exit code
          });
          return;
        }

        // Return raw output — let each capability decide what to do
        resolve({
          stdout: stdout.trim(),
          stderr: stderr.trim() || error?.message || "",
          exitCode: typeof error?.code === "number" ? error.code : (error ? 1 : 0),
        });
      },
    );

    if (opts.stdin !== undefined) {
      child.stdin?.on("error", () => {
        // The CLI may fail before reading stdin; fail-open via the process callback.
      });
      try {
        child.stdin?.end(opts.stdin);
      } catch {
        // stdin write failures are reported through the process callback.
      }
    }
  });
}

export type OpenClawObservabilityRecord = Record<string, unknown>;

const OBSERVABILITY_SENSITIVE_KEYS = new Set([
  "prompt",
  "user_input",
  "system_prompt",
  "messages",
  "response",
  "parameters",
  "result",
  "error",
  "tool_calls",
]);
const DROP = Symbol("drop-sensitive-observability-field");

async function redactTextForObservability(text: string): Promise<string | undefined> {
  const result = await callAgentSecCli(
    [
      "scan-pii",
      "--stdin",
      "--format",
      "json",
      "--redact-output",
      "--source",
      "observability",
    ],
    { stdin: text },
  );
  if (result.exitCode !== 0) {
    return undefined;
  }
  try {
    const data = JSON.parse(result.stdout) as { redacted_text?: unknown };
    return typeof data.redacted_text === "string" ? data.redacted_text : undefined;
  } catch {
    return undefined;
  }
}

async function redactSensitiveValue(value: unknown): Promise<unknown | typeof DROP> {
  if (typeof value === "string") {
    const redacted = await redactTextForObservability(value);
    return redacted === undefined ? DROP : redacted;
  }

  let serialized: string;
  try {
    serialized = JSON.stringify(value);
  } catch {
    serialized = String(value);
  }
  const redacted = await redactTextForObservability(serialized);
  if (redacted === undefined) {
    return DROP;
  }
  try {
    return JSON.parse(redacted);
  } catch {
    return redacted;
  }
}

async function redactObservabilityValue(value: unknown): Promise<unknown | typeof DROP> {
  if (Array.isArray(value)) {
    const redactedItems = await Promise.all(value.map((item) => redactObservabilityValue(item)));
    return redactedItems.filter((item) => item !== DROP);
  }
  if (value && typeof value === "object") {
    const entries = await Promise.all(
      Object.entries(value as Record<string, unknown>).map(async ([key, item]) => {
        const safeItem = OBSERVABILITY_SENSITIVE_KEYS.has(key)
          ? await redactSensitiveValue(item)
          : await redactObservabilityValue(item);
        return [key, safeItem] as const;
      }),
    );
    const redacted: Record<string, unknown> = {};
    for (const [key, item] of entries) {
      if (item !== DROP) {
        redacted[key] = item;
      }
    }
    return redacted;
  }
  return value;
}

async function redactObservabilityRecord(
  event: OpenClawObservabilityRecord,
): Promise<OpenClawObservabilityRecord> {
  const metrics = event.metrics;
  if (!metrics || typeof metrics !== "object" || Array.isArray(metrics)) {
    return event;
  }
  const safeMetrics = await redactObservabilityValue(metrics);
  return {
    ...event,
    metrics:
      safeMetrics && typeof safeMetrics === "object" && !Array.isArray(safeMetrics)
        ? (safeMetrics as Record<string, unknown>)
        : {},
  };
}

/**
 * Emit one OpenClaw observability record to agent-sec-cli via stdin.
 * Logging is best-effort: callers must not use failures to alter OpenClaw behavior.
 */
export async function recordOpenClawObservability(
  event: OpenClawObservabilityRecord,
): Promise<CliResult> {
  const safeEvent = await redactObservabilityRecord(event);
  return callAgentSecCli(
    ["observability", "record", "--format", "json", "--stdin"],
    {
      stdin: JSON.stringify(safeEvent),
    },
  );
}
