import type { DatabaseSync } from "node:sqlite";
import type {
  AgentMessage,
  AssembleResult,
  BootstrapResult,
  CompactResult,
  ContextEngine,
  ContextEngineInfo,
  IngestResult,
} from "./openclaw-bridge.js";
import type { SelectiveClawConfig } from "./types.js";
import type { SummarizeFn } from "./summarize.js";
import { runMigrations } from "./db/migration.js";
import { MessageStore } from "./store/message-store.js";
import { Assembler } from "./assembler.js";
import { estimateTokens } from "./estimate-tokens.js";

export class SelectiveContextEngine implements ContextEngine {
  readonly info: ContextEngineInfo = {
    id: "selective-claw",
    name: "Selective Context Injection",
    version: "0.4.0",
    ownsCompaction: true,
  };

  private static sharedActiveSessionId: string | null = null;
  private static sharedTurnSummaryCache = new Map<string, Map<number, string>>();
  private static sharedTurnMessagesCache = new Map<string, Map<number, AgentMessage[]>>();

  private store: MessageStore;
  private assembler: Assembler;
  private migrated = false;
  private config: SelectiveClawConfig;
  private summarizeFn: SummarizeFn | null = null;

  constructor(
    private db: DatabaseSync,
    config: SelectiveClawConfig,
  ) {
    this.config = config;
    this.ensureMigrated();
    this.store = new MessageStore(db);
    this.assembler = new Assembler(config.freshTailTurns);
  }

  private ensureMigrated(): void {
    if (this.migrated) return;
    runMigrations(this.db);
    this.migrated = true;
  }

  setSummarizeFn(fn: SummarizeFn): void {
    this.summarizeFn = fn;
  }

  getStore(): MessageStore {
    return this.store;
  }

  getActiveSessionId(): string | null {
    return SelectiveContextEngine.sharedActiveSessionId;
  }

  async bootstrap(params: {
    sessionId: string;
    sessionKey?: string;
    messages?: AgentMessage[];
  }): Promise<BootstrapResult> {
    SelectiveContextEngine.sharedActiveSessionId = params.sessionId;
    return { bootstrapped: true, importedMessages: 0 };
  }

  async ingest(params: {
    sessionId: string;
    sessionKey?: string;
    message: AgentMessage;
  }): Promise<IngestResult> {
    SelectiveContextEngine.sharedActiveSessionId = params.sessionId;
    return { ingested: false };
  }

  async assemble(params: {
    sessionId: string;
    sessionKey?: string;
    messages: AgentMessage[];
    tokenBudget?: number;
    prompt?: string;
  }): Promise<AssembleResult> {
    SelectiveContextEngine.sharedActiveSessionId = params.sessionId;

    if (!params.messages || params.messages.length === 0) {
      return { messages: [], estimatedTokens: 0 };
    }

    const tokenBudget =
      typeof params.tokenBudget === "number" && params.tokenBudget > 0
        ? params.tokenBudget
        : 128_000;

    const turns = this.deriveTurns(params.messages);

    this.cacheTurnMessages(params.sessionId, turns);

    if (turns.length > this.config.freshTailTurns && this.summarizeFn) {
      await this.generateMissingSummaries(params.sessionId, turns);
    }

    const summaries = this.getSummariesForSession(params.sessionId);

    const result = this.assembler.assemble({
      messages: params.messages,
      summaries,
      tokenBudget,
      freshTailTurns: this.config.freshTailTurns,
    });

    console.log(
      `[selective-claw] assemble: input=${params.messages.length} msgs, ` +
      `turns=${result.stats.totalTurns}, tail=${result.stats.freshTailTurnCount}, ` +
      `summaries=${result.stats.summaryCount}, dropped=${result.stats.droppedTurns}, ` +
      `returning=${result.messages.length} msgs`
    );

    return {
      messages: result.messages,
      estimatedTokens: result.estimatedTokens,
    };
  }

  async afterTurn(params: {
    sessionId: string;
    sessionKey?: string;
    messages?: AgentMessage[];
  }): Promise<void> {
    SelectiveContextEngine.sharedActiveSessionId = params.sessionId;
  }

  async compact(params: {
    sessionId: string;
    sessionKey?: string;
    tokenBudget?: number;
    force?: boolean;
  }): Promise<CompactResult> {
    return {
      ok: true,
      compacted: true,
      reason: "context managed by assemble",
    };
  }

  private async generateMissingSummaries(
    sessionId: string,
    turns: Array<{ turnSeq: number; messages: AgentMessage[] }>,
  ): Promise<void> {
    const olderTurns = turns.slice(0, -this.config.freshTailTurns);
    if (olderTurns.length === 0) return;

    const summaries = this.getSummariesForSession(sessionId);
    const needSummary = olderTurns.filter((t) => !summaries.has(t.turnSeq));
    if (needSummary.length === 0) return;

    const summarizeFn = this.summarizeFn!;

    const results = await Promise.allSettled(
      needSummary.map((turn) => {
        const text = turn.messages
          .map((m) => `${m.role}: ${this.extractContent(m)}`)
          .join("\n");
        return summarizeFn(text).then((summary) => ({ turnSeq: turn.turnSeq, summary }));
      }),
    );

    if (!SelectiveContextEngine.sharedTurnSummaryCache.has(sessionId)) {
      SelectiveContextEngine.sharedTurnSummaryCache.set(sessionId, new Map());
    }
    const cache = SelectiveContextEngine.sharedTurnSummaryCache.get(sessionId)!;

    let generated = 0;
    let failed = 0;
    let fallbacks = 0;
    for (const r of results) {
      if (r.status === "fulfilled") {
        const isFallback = r.value.summary.endsWith("...");
        if (isFallback) fallbacks++;
        cache.set(r.value.turnSeq, r.value.summary);
        generated++;
        try {
          this.store.setTurnSummary(sessionId, r.value.turnSeq, r.value.summary);
        } catch {
          // best-effort
        }
      } else {
        failed++;
        console.error(`[selective-claw] summary rejected for turn:`, r.reason);
      }
    }

    if (generated > 0 || failed > 0) {
      console.log(
        `[selective-claw] summaries: generated=${generated}, fallbacks=${fallbacks}, failed=${failed}, ` +
        `cached=${cache.size}, needed=${needSummary.length}`
      );
    }
  }

  private cacheTurnMessages(
    sessionId: string,
    turns: Array<{ turnSeq: number; messages: AgentMessage[] }>,
  ): void {
    if (!SelectiveContextEngine.sharedTurnMessagesCache.has(sessionId)) {
      SelectiveContextEngine.sharedTurnMessagesCache.set(sessionId, new Map());
    }
    const cache = SelectiveContextEngine.sharedTurnMessagesCache.get(sessionId)!;
    for (const turn of turns) {
      cache.set(turn.turnSeq, turn.messages);
    }
  }

  expandTurns(sessionId: string, turnSeqs: number[]): {
    found: number;
    turns: Array<{ turnSeq: number; messages: Array<{ role: string; content: string }> }>;
  } {
    const cache = SelectiveContextEngine.sharedTurnMessagesCache.get(sessionId);
    if (!cache) return { found: 0, turns: [] };

    const result: Array<{ turnSeq: number; messages: Array<{ role: string; content: string }> }> = [];
    for (const seq of turnSeqs) {
      const msgs = cache.get(seq);
      if (msgs) {
        result.push({
          turnSeq: seq,
          messages: msgs.map((m) => ({
            role: m.role,
            content: this.extractContent(m),
          })),
        });
      }
    }
    return { found: result.length, turns: result };
  }

  private deriveTurns(messages: AgentMessage[]): Array<{ turnSeq: number; messages: AgentMessage[] }> {
    const turns: Array<{ turnSeq: number; messages: AgentMessage[] }> = [];
    let turnSeq = 0;
    let current: { turnSeq: number; messages: AgentMessage[] } | null = null;

    for (const msg of messages) {
      const role = this.normalizeRole(msg.role);
      if (role === "user" || role === "system") {
        turnSeq++;
        current = { turnSeq, messages: [] };
        turns.push(current);
      }
      if (!current) {
        current = { turnSeq: 1, messages: [] };
        turns.push(current);
      }
      current.messages.push(msg);
    }

    return turns;
  }

  private getSummariesForSession(sessionId: string): Map<number, string> {
    if (SelectiveContextEngine.sharedTurnSummaryCache.has(sessionId)) {
      return SelectiveContextEngine.sharedTurnSummaryCache.get(sessionId)!;
    }

    const map = new Map<number, string>();
    try {
      const stored = this.store.getTurnSummaries(sessionId);
      for (const s of stored) {
        map.set(s.turnSeq, s.summary);
      }
    } catch {
      // best-effort
    }
    SelectiveContextEngine.sharedTurnSummaryCache.set(sessionId, map);
    return map;
  }

  private extractContent(message: AgentMessage): string {
    if (typeof message.content === "string") return message.content;
    if (Array.isArray(message.content)) {
      return message.content
        .map((block: any) => {
          if (typeof block === "string") return block;
          if (block?.type === "text" && typeof block.text === "string") return block.text;
          if (block?.type === "tool_result" && typeof block.output === "string") return block.output;
          return JSON.stringify(block);
        })
        .join("\n");
    }
    if (message.content != null) return JSON.stringify(message.content);
    return "";
  }

  private normalizeRole(role: string): "system" | "user" | "assistant" | "tool" {
    if (role === "toolResult" || role === "tool_result") return "tool";
    if (role === "system" || role === "user" || role === "assistant" || role === "tool") {
      return role;
    }
    return "user";
  }
}
