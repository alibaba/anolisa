/**
 * Choosing which store to render an ATIF session from.
 *
 * A session can live in two places, both serving the same ATIF v1.7 schema:
 *
 *   - the eBPF export (`genai_events.db`) — reconstructed from captured wire
 *     traffic, so it carries request/response token metrics;
 *   - the log-collected trajectory (`trajectories.db`) — parsed from the
 *     agent's own JSONL session log.
 *
 * The export used to win on any HTTP 200. That premise breaks whenever only the
 * request half of the TLS traffic is captured (e.g. agents that statically link
 * BoringSSL, where responses land as `output_messages: null`): the export still
 * returns 200 with a document that has plenty of *steps* but no agent payload at
 * all — no messages, no tool calls, no observations, no metrics. Rendering it
 * shows a viewer full of "无消息内容" rows, and because each captured call
 * re-sends the whole prompt, chunks of the system prompt surface as extra
 * "rounds".
 *
 * So the *default* is chosen on content, not on HTTP status: whichever document
 * actually describes what the agent did wins, with the export keeping ties so
 * its wire-level metrics stay preferred wherever capture is complete.
 *
 * The default is only a default. The two stores answer different questions —
 * only the export sees the system prompt, the tool schemas and the interruption
 * signals; only the collected log sees the agent's own messages here — so both
 * documents are kept and the user can switch. Auto-picking alone would hide a
 * broken capture, which for an eBPF observability tool is itself the finding.
 */

import type { AtifDocument, AtifStep } from '../types';

/** The two stores a session can live in. Shared vocabulary across the app. */
export type AtifSource = 'ebpf' | 'log';

export const SOURCE_LABEL: Record<AtifSource, string> = { ebpf: 'eBPF', log: '日志' };

export const SOURCE_BADGE_CLASS: Record<AtifSource, string> = {
  ebpf: 'bg-blue-100 text-blue-700',
  log: 'bg-emerald-100 text-emerald-700',
};

/** Both sides of a session, plus the side to show when the user has not picked. */
export interface SessionAtifDocs {
  ebpf: AtifDocument | null;
  log: AtifDocument | null;
  defaultSource: AtifSource;
}

/** Fetchers are injected so the selection can be tested without a network. */
export interface SessionAtifSources {
  fetchExported: (sessionId: string) => Promise<unknown>;
  fetchCollected: (sessionId: string) => Promise<unknown>;
}

export function isAtifDocument(value: unknown): value is AtifDocument {
  return !!value
    && typeof value === 'object'
    && !Array.isArray(value)
    && typeof (value as { schema_version?: unknown }).schema_version === 'string'
    && String((value as { schema_version: string }).schema_version).startsWith('ATIF');
}

/** True when the step records something the agent actually said, did, or spent. */
function agentStepHasPayload(step: AtifStep): boolean {
  if ((step.message ?? '').trim()) return true;
  if ((step.reasoning_content ?? '').trim()) return true;
  if (Array.isArray(step.tool_calls) && step.tool_calls.length > 0) return true;
  if (Array.isArray(step.observation?.results) && step.observation!.results.length > 0) return true;
  // `metrics: {}` is what a response-less capture leaves behind — only count
  // metrics that carry a real number.
  const metrics = step.metrics;
  if (metrics && Object.values(metrics).some(v => typeof v === 'number')) return true;
  return false;
}

/**
 * How many agent steps carry real content, out of how many exist.
 *
 * Reported as a ratio because "empty" is rarely all-or-nothing: a capture that
 * recovered 1 response out of 235 is still broken, and a bare zero-check would
 * declare it healthy. Step *count* alone says nothing — a response-less export
 * inflates steps while carrying no information.
 */
export function atifAgentCoverage(doc: unknown): { withPayload: number; total: number } {
  const steps = (doc as AtifDocument | null)?.steps;
  if (!Array.isArray(steps)) return { withPayload: 0, total: 0 };
  const agentSteps = steps.filter(s => s?.source === 'agent');
  return {
    withPayload: agentSteps.filter(agentStepHasPayload).length,
    total: agentSteps.length,
  };
}

/** Number of agent steps carrying real content. */
export function atifAgentContentScore(doc: unknown): number {
  return atifAgentCoverage(doc).withPayload;
}

/**
 * Fill agent metadata the winner does not record from the doc that lost.
 *
 * The two stores see different things: only the wire export observes the tool
 * schemas the agent sent, so a collected doc that wins on step content would
 * otherwise report "工具定义 0 个". Existing values always stand — this only
 * fills gaps, and it copies rather than mutating either input.
 */
function backfillAgent(winner: AtifDocument, loser: AtifDocument): AtifDocument {
  const winnerAgent = winner.agent;
  const loserAgent = loser.agent;
  if (!winnerAgent || !loserAgent) return winner;

  const missingTools = !winnerAgent.tool_definitions?.length
    && !!loserAgent.tool_definitions?.length;
  const missingModel = !winnerAgent.model_name && !!loserAgent.model_name;
  if (!missingTools && !missingModel) return winner;

  return {
    ...winner,
    agent: {
      ...winnerAgent,
      ...(missingTools ? { tool_definitions: loserAgent.tool_definitions } : {}),
      ...(missingModel ? { model_name: loserAgent.model_name } : {}),
    },
  };
}

/** Pick the document that better describes the run; the export keeps ties. */
export function pickRicherAtifDoc(
  exported: AtifDocument | null,
  collected: AtifDocument | null,
): AtifDocument | null {
  if (!exported) return collected;
  if (!collected) return exported;
  return atifAgentContentScore(collected) > atifAgentContentScore(exported)
    ? backfillAgent(collected, exported)
    : backfillAgent(exported, collected);
}

function isNotFound(err: unknown): boolean {
  return (err as { status?: number } | null)?.status === 404;
}

/**
 * Load both sides of a session.
 *
 * Both stores are consulted every time — the export alone cannot tell us
 * whether it is content-free because capture was partial or because the run
 * really was that short, and holding both lets the viewer switch source without
 * a refetch. A missing store is not an error; a store that fails for any other
 * reason is only fatal when it was the sole candidate.
 *
 * Each side keeps its own document: the eBPF view stays visible (and visibly
 * empty) rather than being swapped out, because a content-free capture is a
 * diagnostic signal about AgentSight itself. Only agent *metadata* crosses the
 * boundary, via backfillAgent.
 */
export async function loadSessionAtifSources(
  sessionId: string,
  sources: SessionAtifSources,
): Promise<SessionAtifDocs> {
  const [exportedResult, collectedResult] = await Promise.allSettled([
    sources.fetchExported(sessionId),
    sources.fetchCollected(sessionId),
  ]);

  // A real export failure (auth, 500, network) must not be masked by falling
  // back to a possibly-stale collected trajectory.
  if (exportedResult.status === 'rejected' && !isNotFound(exportedResult.reason)) {
    throw exportedResult.reason;
  }

  const exported = exportedResult.status === 'fulfilled' && isAtifDocument(exportedResult.value)
    ? exportedResult.value
    : null;
  const collected = collectedResult.status === 'fulfilled' && isAtifDocument(collectedResult.value)
    ? collectedResult.value
    : null;

  if (!exported && !collected) {
    if (collectedResult.status === 'rejected' && !isNotFound(collectedResult.reason)) {
      throw collectedResult.reason;
    }
    throw new Error(`未找到该 Session：${sessionId}（既无 eBPF 捕获记录，也无采集轨迹）`);
  }

  // Compare scores directly rather than the identity of pickRicherAtifDoc's
  // result: backfillAgent returns a fresh object whenever metadata is borrowed,
  // so `picked === collected` silently flips the default.
  const preferLog = !!collected
    && (!exported || atifAgentContentScore(collected) > atifAgentContentScore(exported));

  return {
    ebpf: exported && collected ? backfillAgent(exported, collected) : exported,
    log: exported && collected ? backfillAgent(collected, exported) : collected,
    defaultSource: preferLog ? 'log' : 'ebpf',
  };
}

/**
 * The document to render for `source`. An explicit pick is honoured even when
 * that side is empty — that is the point of the switch — but a source the
 * session simply does not have falls back to the default.
 */
export function docForSource(
  docs: SessionAtifDocs,
  source: AtifSource | null,
): AtifDocument | null {
  if (source) {
    const picked = source === 'ebpf' ? docs.ebpf : docs.log;
    if (picked) return picked;
  }
  return docs.defaultSource === 'ebpf' ? docs.ebpf : docs.log;
}

/** Load a session from whichever store describes it best. */
export async function loadSessionAtifDoc(
  sessionId: string,
  sources: SessionAtifSources,
): Promise<AtifDocument> {
  const docs = await loadSessionAtifSources(sessionId, sources);
  // Non-null: loadSessionAtifSources throws unless at least one side exists.
  return docForSource(docs, null) as AtifDocument;
}
