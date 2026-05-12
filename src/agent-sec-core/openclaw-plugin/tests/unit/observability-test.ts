import { afterEach, beforeEach, describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  buildOpenClawObservabilityRecord,
  observability,
} from "../../src/capabilities/observability.js";
import {
  _resetCliMock,
  _setCliMock,
  type CliResult,
} from "../../src/utils.js";

type RegisteredHook = {
  hookName: string;
  handler: (event: unknown, ctx: unknown) => unknown;
  priority: number;
};

const OBSERVABILITY_HOOKS = [
  "llm_input",
  "model_call_started",
  "model_call_ended",
  "llm_output",
  "agent_end",
  "before_tool_call",
  "after_tool_call",
];

const AGENT_SEC_METRIC_ALLOWLIST: Record<string, readonly string[]> = {
  before_agent_run: [
    "history_messages_count",
    "images_count",
    "context_window_utilization",
    "model_id",
    "model_provider",
    "prompt",
    "system_prompt",
    "user_input",
  ],
  before_llm_call: [
    "api",
    "history_messages_count",
    "images_count",
    "context_window_utilization",
    "model_id",
    "model_provider",
    "prompt",
    "system_prompt",
    "transport",
    "user_input",
  ],
  after_llm_call: [
    "error_category",
    "failure_kind",
    "latency_ms",
    "outcome",
    "output_kind",
    "assistant_texts_count",
    "request_payload_bytes",
    "response",
    "response_stream_bytes",
    "stop_reason",
    "time_to_first_byte_ms",
    "tool_calls",
    "tool_calls_count",
    "upstream_request_id_hash",
  ],
  before_tool_call: ["parameters", "tool_name"],
  after_tool_call: ["duration_ms", "error", "exit_code", "result", "result_size_bytes", "status"],
  after_agent_run: [
    "duration_ms",
    "error",
    "final_model_id",
    "final_model_provider",
    "output_kind",
    "assistant_texts_count",
    "response",
    "stop_reason",
    "success",
    "tool_calls",
    "tool_calls_count",
    "total_api_calls",
    "total_tool_calls",
  ],
};

function createMockApi(pluginConfig: Record<string, unknown> = {}) {
  const hooks: RegisteredHook[] = [];
  const logs: string[] = [];
  const api = {
    pluginConfig,
    logger: {
      info: (msg: string) => logs.push(`[INFO] ${msg}`),
      error: (msg: string) => logs.push(`[ERROR] ${msg}`),
      warn: (msg: string) => logs.push(`[WARN] ${msg}`),
      debug: (msg: string) => logs.push(`[DEBUG] ${msg}`),
    },
    on: (hookName: string, handler: RegisteredHook["handler"], opts?: { priority?: number }) => {
      hooks.push({ hookName, handler, priority: opts?.priority ?? 0 });
    },
  };
  return { api: api as never, hooks, logs };
}

function beforeToolCallEvent() {
  return {
    toolName: "exec",
    params: {
      command:
        "OPENAI_API_KEY=sk-testsecret1234567890 curl https://example.com && rm -rf /tmp/demo",
    },
    runId: "run-001",
    toolCallId: "tool-001",
    traceId: "11111111111111111111111111111111",
    spanId: "2222222222222222",
  };
}

let capturedArgs: string[] | undefined;
let capturedStdin: string | undefined;

function mockCli(result: CliResult = { exitCode: 0, stdout: "", stderr: "" }) {
  _setCliMock(async (args, opts) => {
    capturedArgs = args;
    capturedStdin = opts.stdin;
    return result;
  });
}

function assertMetricsAllowedByAgentSecSchema(payload: { hook: string; metrics: Record<string, unknown> }): void {
  const allowed = AGENT_SEC_METRIC_ALLOWLIST[payload.hook];
  assert.ok(allowed, `unexpected agent-sec hook: ${payload.hook}`);
  assert.ok(Object.keys(payload.metrics).length > 0);
  for (const key of Object.keys(payload.metrics)) {
    assert.ok(allowed.includes(key), `${payload.hook} does not allow metric ${key}`);
  }
}

describe("observability", () => {
  beforeEach(() => {
    capturedArgs = undefined;
    capturedStdin = undefined;
  });

  afterEach(() => {
    _resetCliMock();
  });

  it("registers the configured observability hooks when enabled by default", () => {
    const { api, hooks } = createMockApi();

    observability.register(api);

    assert.deepEqual(hooks.map((hook) => hook.hookName), OBSERVABILITY_HOOKS);
    assert.equal(hooks.some((hook) => hook.hookName === "before_model_resolve"), false);
    assert.equal(hooks.find((hook) => hook.hookName === "before_tool_call")?.priority, -10_000);
    assert.equal(hooks.find((hook) => hook.hookName === "llm_input")?.priority, 1000);
  });

  it("emits the expected CLI payload for before_tool_call", () => {
    mockCli();
    const { api, hooks } = createMockApi();
    observability.register(api);
    const hook = hooks.find((item) => item.hookName === "before_tool_call");
    assert.ok(hook);

    const result = hook.handler(beforeToolCallEvent(), {
      sessionId: "session-001",
      sessionKey: "session-key-001",
      runId: "run-ctx",
    });

    assert.equal(result, undefined);
    assert.deepEqual(capturedArgs, ["observability", "record", "--format", "json", "--stdin"]);
    assert.ok(capturedStdin);
    const payload = JSON.parse(capturedStdin);
    assert.equal("schemaVersion" in payload, false);
    assert.equal(payload.hook, "before_tool_call");
    assert.match(payload.observedAt, /^\d{4}-\d{2}-\d{2}T/);
    assert.equal(payload.metadata.traceId, "11111111111111111111111111111111");
    assert.equal(payload.metadata.toolCallId, "tool-001");
    assert.equal(payload.metadata.sessionId, "session-001");
    assert.equal(payload.metadata.runId, "run-001");
    assert.deepEqual(payload.metrics, {
      tool_name: "exec",
      parameters: beforeToolCallEvent().params,
    });
  });

  it("keeps correlation metadata out of metrics", () => {
    const payload = buildOpenClawObservabilityRecord(
      "before_tool_call",
      beforeToolCallEvent(),
      { sessionId: "session-001", sessionKey: "session-key-001" },
    );

    assert.ok(payload);
    const metricsJson = JSON.stringify(payload.metrics);
    assert.equal(metricsJson.includes("11111111111111111111111111111111"), false);
    assert.equal(metricsJson.includes("2222222222222222"), false);
    assert.equal(metricsJson.includes("sk-testsecret1234567890"), true);
    assert.equal(payload.metadata.traceId, "11111111111111111111111111111111");
  });

  it("builds llm_input directly as before_agent_run without callId", () => {
    const payload = buildOpenClawObservabilityRecord(
      "llm_input",
      {
        provider: "dashscope",
        model: "qwen3.6-plus",
        sessionId: "session-llm",
        runId: "run-llm",
        systemPrompt: "System after prompt-build hooks",
        prompt: "帮我创建testfolder，在里面创建a.txt",
        historyMessagesCount: 22,
        imagesCount: 1,
        contextWindowUtilization: 0.5,
      },
      { sessionId: "session-llm", runId: "run-llm" },
    );

    assert.ok(payload);
    assert.equal(payload.hook, "before_agent_run");
    assert.equal("callId" in payload.metadata, false);
    assert.equal(payload.metrics.prompt, "帮我创建testfolder，在里面创建a.txt");
    assert.equal(payload.metrics.user_input, "帮我创建testfolder，在里面创建a.txt");
    assert.equal(payload.metrics.system_prompt, "System after prompt-build hooks");
    assert.equal(payload.metrics.history_messages_count, 22);
    assert.equal(payload.metrics.images_count, 1);
    assert.equal(payload.metrics.context_window_utilization, 0.5);
    assertMetricsAllowedByAgentSecSchema(payload);
  });

  it("emits llm_input as before_agent_run and model_call_started as before_llm_call", () => {
    mockCli();
    const { api, hooks } = createMockApi();
    observability.register(api);
    hooks.find((item) => item.hookName === "llm_input")?.handler(
      {
        provider: "openai",
        model: "gpt-5.4",
        systemPrompt: "System after prompt-build hooks",
        prompt: "Effective LLM prompt",
        userInput: "Original user input",
        historyMessages: [{ role: "user", content: "hello" }, { role: "assistant", content: "hi" }],
        imagesCount: 2,
        contextWindowUtilization: 0.42,
      },
      { sessionId: "session-model", runId: "run-model" },
    );
    assert.ok(capturedStdin);
    const inputPayload = JSON.parse(capturedStdin);
    assert.equal(inputPayload.hook, "before_agent_run");
    assert.deepEqual(inputPayload.metadata, {
      runId: "run-model",
      sessionId: "session-model",
    });
    assert.deepEqual(inputPayload.metrics, {
      model_id: "gpt-5.4",
      model_provider: "openai",
      prompt: "Effective LLM prompt",
      system_prompt: "System after prompt-build hooks",
      user_input: "Original user input",
      history_messages_count: 2,
      images_count: 2,
      context_window_utilization: 0.42,
    });

    hooks.find((item) => item.hookName === "model_call_started")?.handler(
      {
        runId: "run-model",
        sessionId: "session-model",
        callId: "call-001",
        provider: "openai",
        model: "gpt-5.4",
        api: "responses",
        transport: "http",
      },
      {},
    );

    assert.ok(capturedStdin);
    const startedPayload = JSON.parse(capturedStdin);
    assert.ok(startedPayload);
    assert.equal(startedPayload.hook, "before_llm_call");
    assert.deepEqual(startedPayload.metadata, {
      runId: "run-model",
      sessionId: "session-model",
      callId: "call-001",
    });
    assert.deepEqual(startedPayload.metrics, {
      model_id: "gpt-5.4",
      model_provider: "openai",
      api: "responses",
      transport: "http",
    });
    assertMetricsAllowedByAgentSecSchema(inputPayload);
    assertMetricsAllowedByAgentSecSchema(startedPayload);
  });

  it("builds model_call_ended metrics accepted by agent-sec-cli", () => {
    const payload = buildOpenClawObservabilityRecord(
      "model_call_ended",
      {
        runId: "run-model",
        sessionId: "session-model",
        callId: "call-001",
        provider: "openai",
        model: "gpt-5.4",
        durationMs: 1234,
        outcome: "error",
        errorCategory: "Error",
        failureKind: "timeout",
        requestPayloadBytes: 2048,
        responseStreamBytes: 512,
        timeToFirstByteMs: 300,
        upstreamRequestIdHash: "hash-001",
      },
      {},
    );

    assert.ok(payload);
    assert.equal(payload.hook, "after_llm_call");
    assert.deepEqual(payload.metadata, {
      runId: "run-model",
      sessionId: "session-model",
      callId: "call-001",
    });
    assert.deepEqual(payload.metrics, {
      latency_ms: 1234,
      outcome: "error",
      error_category: "Error",
      failure_kind: "timeout",
      request_payload_bytes: 2048,
      response_stream_bytes: 512,
      time_to_first_byte_ms: 300,
      upstream_request_id_hash: "hash-001",
    });
    assertMetricsAllowedByAgentSecSchema(payload);
  });

  it("builds llm_output directly as after_agent_run without callId", () => {
    const payload = buildOpenClawObservabilityRecord(
      "llm_output",
      {
        provider: "dashscope",
        model: "qwen3.6-plus",
        sessionId: "session-llm",
        runId: "run-llm",
        resolvedRef: "dashscope/qwen3.6-plus",
        harnessId: "pi-embedded",
        assistantTexts: ["我先检查目录。", "`testfolder2` 已经不存在。"],
        lastAssistant: "`testfolder2` 已经不存在。",
        usage: {
          input: 100,
          output: 20,
          cacheRead: 5,
          cacheWrite: 3,
          total: 128,
        },
      },
      { sessionId: "session-llm", runId: "run-llm" },
    );

    assert.ok(payload);
    assert.equal(payload.hook, "after_agent_run");
    assert.equal("callId" in payload.metadata, false);
    assert.deepEqual(payload.metadata, {
      runId: "run-llm",
      sessionId: "session-llm",
    });
    assert.deepEqual(payload.metrics, {
      response: "`testfolder2` 已经不存在。",
      output_kind: "text",
      assistant_texts_count: 2,
      stop_reason: "stop",
    });
    assertMetricsAllowedByAgentSecSchema(payload);
  });

  it("builds llm_output tool-use summaries as after_agent_run metrics", () => {
    const payload = buildOpenClawObservabilityRecord(
      "llm_output",
      {
        provider: "dashscope",
        model: "qwen3.6-plus",
        sessionId: "session-llm",
        runId: "run-llm",
        assistantTexts: [],
        lastAssistant: {
          role: "assistant",
          stopReason: "toolUse",
          content: [
            {
              type: "toolCall",
              name: "exec",
              input: {
                command: 'find /home/xingdong -name "testfolder2" -maxdepth 3 2>/dev/null',
              },
            },
          ],
        },
      },
      { sessionId: "session-llm", runId: "run-llm" },
    );

    assert.ok(payload);
    assert.equal(payload.hook, "after_agent_run");
    assert.equal("callId" in payload.metadata, false);
    assert.deepEqual(payload.metadata, {
      runId: "run-llm",
      sessionId: "session-llm",
    });
    assert.deepEqual(payload.metrics, {
      output_kind: "tool_use",
      stop_reason: "toolUse",
      assistant_texts_count: 0,
      tool_calls_count: 1,
      tool_calls: [
        {
          toolName: "exec",
          parameters: {
            command: 'find /home/xingdong -name "testfolder2" -maxdepth 3 2>/dev/null',
          },
        },
      ],
    });
    assertMetricsAllowedByAgentSecSchema(payload);
  });

  it("builds after_tool_call metrics accepted by agent-sec-cli", () => {
    const payload = buildOpenClawObservabilityRecord(
      "after_tool_call",
      {
        runId: "run-tool",
        sessionId: "session-tool",
        toolCallId: "tool-call-001",
        result: { content: "token=secret-value-1234567890" },
        error: "failed with password=hunter2",
        durationMs: 50,
      },
      { sessionId: "session-tool", runId: "run-tool" },
    );

    assert.ok(payload);
    assert.equal(payload.hook, "after_tool_call");
    assert.deepEqual(payload.metrics.result, { content: "token=secret-value-1234567890" });
    assert.equal(payload.metrics.error, "failed with password=hunter2");
    assert.equal(payload.metrics.duration_ms, 50);
    assert.equal(typeof payload.metrics.result_size_bytes, "number");
    assert.deepEqual(Object.keys(payload.metrics).sort(), [
      "duration_ms",
      "error",
      "result",
      "result_size_bytes",
    ]);
    assertMetricsAllowedByAgentSecSchema(payload);
  });

  it("derives after_tool_call error from non-zero tool result details", () => {
    const payload = buildOpenClawObservabilityRecord(
      "after_tool_call",
      {
        runId: "run-tool",
        sessionId: "session-tool",
        toolCallId: "tool-call-001",
        result: {
          content: [
            {
              type: "text",
              text: "ls: cannot access 'testfolder/': No such file or directory\n\n(Command exited with code 2)",
            },
          ],
          details: {
            status: "completed",
            exitCode: 2,
            durationMs: 16,
            aggregated: "ls: cannot access 'testfolder/': No such file or directory",
          },
        },
        durationMs: 489,
      },
      { sessionId: "session-tool", runId: "run-tool" },
    );

    assert.ok(payload);
    assert.equal(payload.hook, "after_tool_call");
    assert.equal(payload.metrics.error, "ls: cannot access 'testfolder/': No such file or directory");
    assert.equal(payload.metrics.duration_ms, 489);
    assert.equal(payload.metrics.status, "completed");
    assert.equal(payload.metrics.exit_code, 2);
    assert.equal(typeof payload.metrics.result_size_bytes, "number");
    assertMetricsAllowedByAgentSecSchema(payload);
  });

  it("does not derive response from agent_end messages", () => {
    const payload = buildOpenClawObservabilityRecord(
      "agent_end",
      {
        runId: "run-agent-end",
        success: true,
        durationMs: 321,
        messages: [
          { role: "user", content: [{ type: "text", text: "old request" }] },
          { role: "assistant", content: [{ type: "text", text: "old response" }] },
          { role: "user", content: [{ type: "text", text: "create file" }] },
          {
            role: "assistant",
            content: [
              { type: "thinking", thinking: "private reasoning" },
              { type: "text", text: "确认一下是否还在：" },
              { type: "toolCall", name: "exec" },
            ],
          },
          { role: "toolResult", content: [{ type: "text", text: "done" }] },
          {
            role: "assistant",
            content: [{ type: "text", text: "搞定了\n\n- testfolder/a.txt" }],
          },
        ],
      },
      { sessionId: "session-001", runId: "run-agent-end" },
    );

    assert.ok(payload);
    assert.equal(payload.hook, "after_agent_run");
    assert.deepEqual(payload.metrics, {
      success: true,
      duration_ms: 321,
    });
    assertMetricsAllowedByAgentSecSchema(payload);
  });

  it("uses run-level aggregate metrics provided by agent_end", () => {
    mockCli();
    const { api, hooks } = createMockApi();
    observability.register(api);
    hooks.find((item) => item.hookName === "model_call_started")?.handler(
      {
        runId: "run-aggregate",
        sessionId: "session-aggregate",
        callId: "call-1",
        provider: "openai",
        model: "gpt-5.4",
      },
      {},
    );
    hooks.find((item) => item.hookName === "before_tool_call")?.handler(
      {
        runId: "run-aggregate",
        sessionId: "session-aggregate",
        toolCallId: "tool-1",
        toolName: "exec",
        params: { command: "true" },
      },
      {},
    );

    hooks.find((item) => item.hookName === "agent_end")?.handler(
      {
        runId: "run-aggregate",
        sessionId: "session-aggregate",
        success: true,
        totalApiCalls: 3,
        totalToolCalls: 2,
        finalModelId: "gpt-5.4",
        finalModelProvider: "openai",
        messages: [{ role: "assistant", content: [{ type: "text", text: "done" }] }],
      },
      {},
    );

    assert.ok(capturedStdin);
    const payload = JSON.parse(capturedStdin);
    assert.equal(payload.hook, "after_agent_run");
    assert.equal(payload.metrics.total_api_calls, 3);
    assert.equal(payload.metrics.total_tool_calls, 2);
    assert.equal(payload.metrics.final_model_id, "gpt-5.4");
    assert.equal(payload.metrics.final_model_provider, "openai");
    assertMetricsAllowedByAgentSecSchema(payload);
  });

  it("does not derive after_agent_run aggregate metrics from volatile process state", () => {
    mockCli();
    const { api, hooks } = createMockApi();
    observability.register(api);
    hooks.find((item) => item.hookName === "model_call_started")?.handler(
      {
        runId: "run-volatile",
        sessionId: "session-volatile",
        callId: "call-1",
        provider: "openai",
        model: "gpt-5.4",
      },
      {},
    );
    hooks.find((item) => item.hookName === "before_tool_call")?.handler(
      {
        runId: "run-volatile",
        sessionId: "session-volatile",
        toolCallId: "tool-1",
        toolName: "exec",
        params: { command: "true" },
      },
      {},
    );

    hooks.find((item) => item.hookName === "agent_end")?.handler(
      {
        runId: "run-volatile",
        sessionId: "session-volatile",
        success: true,
        messages: [{ role: "assistant", content: [{ type: "text", text: "done" }] }],
      },
      {},
    );

    assert.ok(capturedStdin);
    const payload = JSON.parse(capturedStdin);
    assert.equal(payload.hook, "after_agent_run");
    assert.deepEqual(payload.metrics, { success: true });
    assertMetricsAllowedByAgentSecSchema(payload);
  });

  it("skips CLI when required metadata is missing", () => {
    mockCli();
    const { api, hooks } = createMockApi();
    observability.register(api);
    const hook = hooks.find((item) => item.hookName === "before_tool_call");
    assert.ok(hook);

    hook.handler({ toolName: "exec", params: { command: "true" } }, {});

    assert.equal(capturedArgs, undefined);
    assert.equal(capturedStdin, undefined);
  });

  it("handles CLI failure and timeout results without throwing", () => {
    mockCli({ exitCode: 124, stdout: "", stderr: "timeout" });
    const { api, hooks } = createMockApi();
    observability.register(api);
    const hook = hooks.find((item) => item.hookName === "before_tool_call");
    assert.ok(hook);

    assert.doesNotThrow(() => {
      hook.handler(beforeToolCallEvent(), { sessionId: "session-001" });
    });
  });

});
