import React, { useState, useEffect, useCallback, useRef } from 'react';
import { useSearchParams } from 'react-router-dom';
import type {
  AtifDocument, AtifStep, AtifToolCall, AtifObservation, AtifStepMetrics,
  SubagentTrajectoryRef,
} from '../types';
import {
  fetchAtifBySession, fetchAtifByConversation, fetchTrajectoryAtif, fetchSessionSavings,
} from '../utils/apiClient';
import type { SessionSavingsDetail, OptimizationItem } from '../utils/apiClient';
import { SubagentGraph } from '../components/SubagentGraph';
import type { TrajNode } from '../utils/trajectoryTree';
import {
  buildTrajectoryTree, findNodeByPath, findNodeByRef, encodeNodePath, decodeNodePath,
} from '../utils/trajectoryTree';

// ─── Helpers ──────────────────────────────────────────────────────────────────

function fmtTokens(n: number): string {
  return n.toLocaleString();
}

function fmtTimestamp(iso?: string): string {
  if (!iso) return '';
  try {
    return new Date(iso).toLocaleString('zh-CN', {
      month: '2-digit', day: '2-digit',
      hour: '2-digit', minute: '2-digit', second: '2-digit',
    });
  } catch {
    return iso;
  }
}

function shortId(id: string, len = 20): string {
  return id.length > len ? id.slice(0, len) + '\u2026' : id;
}

/** Observation content is any JSON per the ATIF schema; render it as text. */
function asText(content: unknown): string {
  if (content == null) return '';
  if (typeof content === 'string') return content;
  try {
    return JSON.stringify(content, null, 2);
  } catch {
    return String(content);
  }
}

function stepsOf(doc: AtifDocument | null | undefined): AtifStep[] {
  return Array.isArray(doc?.steps) ? doc.steps : [];
}

function toolCallsOf(step: AtifStep): AtifToolCall[] {
  return Array.isArray(step.tool_calls) ? step.tool_calls : [];
}

function observationResultsOf(step: AtifStep) {
  return Array.isArray(step.observation?.results) ? step.observation.results : [];
}

function subagentRefsOf(result: { subagent_trajectory_ref?: SubagentTrajectoryRef[] }) {
  return Array.isArray(result.subagent_trajectory_ref) ? result.subagent_trajectory_ref : [];
}

function highlightedSections(doc: AtifDocument, callId: string | null): Set<string> {
  const sections = new Set<string>();
  if (!callId) return sections;

  for (const step of stepsOf(doc)) {
    if (toolCallsOf(step).some((toolCall) => toolCall.tool_call_id === callId)) {
      sections.add(`${step.step_id}-toolcalls`);
    }
    if (observationResultsOf(step).some((result) => result.source_call_id === callId)) {
      sections.add(`${step.step_id}-observation`);
    }
  }
  return sections;
}

// ─── Round grouping ───────────────────────────────────────────────────────────
// A "round" starts at each user step and spans the following agent/system steps,
// mirroring the round-based trajectory view in agentopt.

interface Round {
  key: number;
  label: string;
  userStep: AtifStep | null;
  steps: AtifStep[];
}

function groupIntoRounds(steps: AtifStep[]): Round[] {
  const rounds: Round[] = [];
  let userRoundCount = 0;
  for (const step of steps) {
    if (step.source === 'user' || rounds.length === 0) {
      const isUser = step.source === 'user';
      if (isUser) userRoundCount++;
      rounds.push({
        key: rounds.length,
        label: isUser ? `第 ${userRoundCount} 轮` : '前置',
        userStep: isUser ? step : null,
        steps: [step],
      });
    } else {
      rounds[rounds.length - 1].steps.push(step);
    }
  }
  return rounds;
}

/** Round to auto-select: prefer the highlighted round, else the first round. */
function initialRound(rounds: Round[], sections: Set<string>): number | null {
  if (rounds.length === 0) return null;
  if (sections.size > 0) {
    const stepIds = new Set<number>();
    sections.forEach(k => stepIds.add(parseInt(k, 10)));
    for (const round of rounds) {
      if (round.steps.some(s => stepIds.has(s.step_id))) return round.key;
    }
  }
  return rounds[0].key;
}

// ─── Strategy label config (shared with TokenSavingsPage) ────────────────────

const STRATEGY_LABELS: Record<string, { label: string; color: string; bg: string }> = {
  'compress-schema':   { label: 'Schema 压缩', color: 'text-blue-700',   bg: 'bg-blue-100' },
  'compress-response': { label: '响应压缩',    color: 'text-violet-700', bg: 'bg-violet-100' },
  'rewrite-command':   { label: '命令重写',    color: 'text-orange-700', bg: 'bg-orange-100' },
  'compress-toon':     { label: 'TOON 编码',   color: 'text-teal-700',  bg: 'bg-teal-100' },
};

// ─── Source styling ───────────────────────────────────────────────────────────

const SOURCE_STYLES: Record<string, { dot: string; badge: string; border: string; label: string }> = {
  system: {
    dot: 'bg-purple-500',
    badge: 'bg-purple-100 text-purple-700',
    border: 'border-l-purple-400',
    label: '系统',
  },
  user: {
    dot: 'bg-blue-500',
    badge: 'bg-blue-100 text-blue-700',
    border: 'border-l-blue-400',
    label: '用户',
  },
  agent: {
    dot: 'bg-green-500',
    badge: 'bg-green-100 text-green-700',
    border: 'border-l-green-400',
    label: 'Agent',
  },
};

function getSourceStyle(source: string) {
  return SOURCE_STYLES[source] ?? {
    dot: 'bg-gray-400',
    badge: 'bg-gray-100 text-gray-600',
    border: 'border-l-gray-300',
    label: source,
  };
}

// ─── Collapsible Section ──────────────────────────────────────────────────────

interface CollapsibleProps {
  icon: string;
  title: string;
  count?: number;
  isOpen: boolean;
  onToggle: () => void;
  children: React.ReactNode;
}

const Collapsible: React.FC<CollapsibleProps> = ({ icon, title, count, isOpen, onToggle, children }) => (
  <div className="mt-3">
    <button
      onClick={onToggle}
      className="w-full flex items-center justify-between px-4 py-2 bg-gray-50 hover:bg-gray-100 rounded-lg text-left text-sm transition-colors"
    >
      <span className="flex items-center gap-2">
        <span>{icon}</span>
        <span className="font-medium text-gray-700">{title}</span>
        {count !== undefined && (
          <span className="px-1.5 py-0.5 bg-gray-200 text-gray-600 rounded text-xs">{count}</span>
        )}
      </span>
      <span className="text-gray-400 text-xs">{isOpen ? '\u25b2' : '\u25bc'}</span>
    </button>
    {isOpen && <div className="mt-2 px-1">{children}</div>}
  </div>
);

// ─── ExpandableText ───────────────────────────────────────────────────────────

const TEXT_THRESHOLD = 300;

const ExpandableText: React.FC<{ text: string; className?: string }> = ({ text, className = '' }) => {
  const [expanded, setExpanded] = useState(false);
  const isLong = text.length > TEXT_THRESHOLD;
  const display = isLong && !expanded ? text.slice(0, TEXT_THRESHOLD) + '\u2026' : text;

  return (
    <div>
      <pre className={`text-sm whitespace-pre-wrap break-words rounded-lg p-3 max-h-80 overflow-y-auto ${className}`}>
        {display}
      </pre>
      {isLong && (
        <button
          onClick={() => setExpanded(!expanded)}
          className="mt-1 text-xs text-blue-600 hover:text-blue-800"
        >
          {expanded ? '← 收起' : '展开全部 →'}
        </button>
      )}
    </div>
  );
};

// ─── StepCard ─────────────────────────────────────────────────────────────────

interface StepCardProps {
  step: AtifStep;
  expandedSections: Set<string>;
  onToggleSection: (key: string) => void;
  savingsMap?: Map<string, OptimizationItem>;
  onNavigateSubagent?: (ref: SubagentTrajectoryRef) => void;
}

const StepCard: React.FC<StepCardProps> = ({ step, expandedSections, onToggleSection, savingsMap, onNavigateSubagent }) => {
  const style = getSourceStyle(step.source);
  const sectionKey = (name: string) => `${step.step_id}-${name}`;
  const isOpen = (name: string) => expandedSections.has(sectionKey(name));
  const toggle = (name: string) => onToggleSection(sectionKey(name));

  const hasReasoning = !!step.reasoning_content;
  const toolCalls = toolCallsOf(step);
  const observationResults = observationResultsOf(step);
  const hasToolCalls = toolCalls.length > 0;
  const hasObservation = observationResults.length > 0;
  const hasMetrics = !!step.metrics && (
    step.metrics.prompt_tokens != null ||
    step.metrics.completion_tokens != null
  );

  return (
    <div className="relative pl-8 mb-4">
      {/* Timeline dot */}
      <div className={`absolute left-0 top-4 w-3 h-3 rounded-full ring-2 ring-white ${style.dot}`} />

      {/* Card */}
      <div className={`bg-white rounded-xl shadow-sm border border-gray-200 border-l-4 ${style.border} overflow-hidden`}>
        {/* Header */}
        <div className="px-5 py-3 flex items-center gap-3 flex-wrap">
          <span className={`px-2 py-0.5 rounded-full text-xs font-medium ${style.badge}`}>
            {style.label}
          </span>
          <span className="text-sm font-medium text-gray-900">Step {step.step_id}</span>
          {step.timestamp && (
            <span className="text-xs text-gray-400">{fmtTimestamp(step.timestamp)}</span>
          )}
          {step.model_name && (
            <span className="px-2 py-0.5 bg-indigo-100 text-indigo-700 rounded-full text-xs">
              {step.model_name}
            </span>
          )}
        </div>

        {/* Body */}
        <div className="px-5 pb-4">
          {/* Message */}
          {step.message ? (
            <ExpandableText text={step.message} className="text-gray-700 bg-gray-50" />
          ) : (
            <span className="text-xs text-gray-400 italic">无消息内容</span>
          )}

          {/* Agent-only sections */}
          {step.source === 'agent' && (
            <>
              {/* Reasoning */}
              {hasReasoning && (
                <Collapsible
                  icon="💭"
                  title="推理过程"
                  isOpen={isOpen('reasoning')}
                  onToggle={() => toggle('reasoning')}
                >
                  <pre className="text-xs text-purple-700 whitespace-pre-wrap break-words bg-purple-50 rounded-lg p-3 border border-purple-100 max-h-64 overflow-y-auto">
                    {step.reasoning_content}
                  </pre>
                </Collapsible>
              )}

              {/* Tool Calls */}
              {hasToolCalls && (
                <Collapsible
                  icon="🔧"
                  title="工具调用"
                  count={toolCalls.length}
                  isOpen={isOpen('toolcalls')}
                  onToggle={() => toggle('toolcalls')}
                >
                  <div className="space-y-2">
                    {toolCalls.map((tc, i) => (
                      <ToolCallItem key={tc.tool_call_id || i} tc={tc} savingsMap={savingsMap} />
                    ))}
                  </div>
                </Collapsible>
              )}

              {/* Observation */}
              {hasObservation && (
                <Collapsible
                  icon="📋"
                  title="观察结果"
                  count={observationResults.length}
                  isOpen={isOpen('observation')}
                  onToggle={() => toggle('observation')}
                >
                  <div className="space-y-2">
                    {observationResults.map((r, i) => {
                      const content = asText(r.content);
                      const subagentRefs = subagentRefsOf(r);
                      const hasSubagentRef = subagentRefs.length > 0;
                      return (
                        <div key={i} className="border border-teal-100 rounded-lg overflow-hidden">
                          {r.source_call_id && (
                            <div className="px-3 py-1 bg-teal-50 border-b border-teal-100">
                              <span className="text-xs text-gray-400 font-mono">call: {shortId(r.source_call_id, 16)}</span>
                            </div>
                          )}
                          {hasSubagentRef && (
                            <div className="px-3 py-2 bg-indigo-50 border-b border-indigo-100 flex flex-wrap gap-2">
                              {subagentRefs.map((ref, ri) => (
                                <button
                                  key={ri}
                                  onClick={() => onNavigateSubagent?.(ref)}
                                  className="inline-flex items-center gap-1.5 px-2.5 py-1 bg-indigo-100 hover:bg-indigo-200 text-indigo-700 rounded-lg text-xs font-medium transition-colors"
                                  title="在上方拓扑图中选中该子代理并查看其轨迹"
                                >
                                  🤖 子代理轨迹
                                  {ref.trajectory_id && (
                                    <span className="font-mono text-indigo-400">{shortId(ref.trajectory_id, 12)}</span>
                                  )}
                                </button>
                              ))}
                            </div>
                          )}
                          {content ? (
                            <div className="p-2">
                              <ExpandableText text={content} className="text-xs text-gray-700 bg-teal-50 font-mono" />
                            </div>
                          ) : !hasSubagentRef ? (
                            <div className="px-3 py-2 text-xs text-gray-400 italic">无输出内容</div>
                          ) : null}
                        </div>
                      );
                    })}
                  </div>
                </Collapsible>
              )}

              {/* Metrics */}
              {hasMetrics && (
                <div className="flex flex-wrap gap-2 mt-3 pt-3 border-t border-gray-100">
                  {step.metrics!.prompt_tokens != null && (
                    <span className="px-2 py-1 bg-blue-50 text-blue-700 rounded text-xs">
                      输入: {fmtTokens(step.metrics!.prompt_tokens!)}
                    </span>
                  )}
                  {step.metrics!.completion_tokens != null && (
                    <span className="px-2 py-1 bg-green-50 text-green-700 rounded text-xs">
                      输出: {fmtTokens(step.metrics!.completion_tokens!)}
                    </span>
                  )}
                  {step.metrics!.cached_tokens != null && step.metrics!.cached_tokens! > 0 && (
                    <span className="px-2 py-1 bg-yellow-50 text-yellow-700 rounded text-xs">
                      缓存: {fmtTokens(step.metrics!.cached_tokens!)}
                    </span>
                  )}
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
};

// ─── ToolCallItem ─────────────────────────────────────────────────────────────

const ToolCallItem: React.FC<{ tc: AtifToolCall; savingsMap?: Map<string, OptimizationItem> }> = ({ tc, savingsMap }) => {
  const [showArgs, setShowArgs] = useState(false);
  const argsStr = typeof tc.arguments === 'string'
    ? tc.arguments
    : JSON.stringify(tc.arguments, null, 2);
  const isLongArgs = argsStr.length > 200;
  const savings = savingsMap?.get(tc.tool_call_id);
  const stratStyle = savings ? (STRATEGY_LABELS[savings.strategy] ?? { label: savings.strategy_label, color: 'text-gray-700', bg: 'bg-gray-100' }) : null;

  return (
    <div className="border border-orange-100 rounded-lg overflow-hidden">
      <div className="px-3 py-2 bg-orange-50 flex items-center gap-2 flex-wrap">
        <span className="px-2 py-0.5 bg-orange-100 text-orange-700 rounded text-xs font-mono font-medium">
          {tc.function_name}
        </span>
        <span className="text-xs text-gray-400 font-mono">{shortId(tc.tool_call_id, 16)}</span>
        {savings && stratStyle && (
          <span className={`px-2 py-0.5 rounded text-xs font-medium ${stratStyle.bg} ${stratStyle.color}`}>
            已优化 -{fmtTokens(savings.compounded_saved)} tokens ({stratStyle.label})
          </span>
        )}
        {isLongArgs && (
          <button
            onClick={() => setShowArgs(!showArgs)}
            className="ml-auto text-xs text-blue-600 hover:text-blue-800"
          >
            {showArgs ? '收起参数' : '展开参数'}
          </button>
        )}
      </div>
      {(!isLongArgs || showArgs) && (
        <pre className="text-xs text-gray-700 whitespace-pre-wrap break-words bg-white p-3 max-h-48 overflow-y-auto font-mono">
          {argsStr}
        </pre>
      )}
    </div>
  );
};

// ─── Round list item (left column) ──────────────────────────────────────────

interface RoundStats {
  toolCallCount: number;
  promptSum: number;
  completionSum: number;
  firstTs?: string;
  preview: string;
}

function roundStats(round: Round): RoundStats {
  let toolCallCount = 0, promptSum = 0, completionSum = 0;
  for (const s of round.steps) {
    toolCallCount += toolCallsOf(s).length;
    promptSum += s.metrics?.prompt_tokens ?? 0;
    completionSum += s.metrics?.completion_tokens ?? 0;
  }
  const preview = (round.userStep?.message ?? round.steps.find(s => s.message)?.message ?? '')
    .replace(/\s+/g, ' ')
    .trim();
  return { toolCallCount, promptSum, completionSum, firstTs: round.steps.find(s => s.timestamp)?.timestamp, preview };
}

interface RoundListItemProps {
  round: Round;
  isActive: boolean;
  onSelect: () => void;
}

const RoundListItem: React.FC<RoundListItemProps> = ({ round, isActive, onSelect }) => {
  const stats = roundStats(round);

  return (
    <button
      onClick={onSelect}
      className={`w-full text-left px-4 py-3 rounded-xl border transition-colors ${
        isActive
          ? 'bg-blue-50 border-blue-300 shadow-sm'
          : 'bg-white border-gray-200 hover:bg-gray-50'
      }`}
    >
      <div className="flex items-center gap-2 mb-1">
        <span className={`flex-shrink-0 px-2 py-0.5 rounded-full text-xs font-medium ${
          round.userStep ? 'bg-blue-100 text-blue-700' : 'bg-purple-100 text-purple-700'
        }`}>
          {round.label}
        </span>
        {stats.firstTs && <span className="text-xs text-gray-400">{fmtTimestamp(stats.firstTs)}</span>}
      </div>
      <p
        className="text-sm text-gray-800"
        style={{ display: '-webkit-box', WebkitLineClamp: 2, WebkitBoxOrient: 'vertical', overflow: 'hidden' }}
      >
        {stats.preview || <span className="text-gray-400 italic">无消息内容</span>}
      </p>
      <div className="flex items-center gap-1.5 mt-1.5 text-xs flex-wrap">
        <span className="px-1.5 py-0.5 bg-gray-100 text-gray-500 rounded">{round.steps.length} 步</span>
        {stats.toolCallCount > 0 && (
          <span className="px-1.5 py-0.5 bg-orange-50 text-orange-600 rounded">🔧 {stats.toolCallCount}</span>
        )}
        {(stats.promptSum > 0 || stats.completionSum > 0) && (
          <span className="px-1.5 py-0.5 bg-blue-50 text-blue-600 rounded">
            {fmtTokens(stats.promptSum)} in / {fmtTokens(stats.completionSum)} out
          </span>
        )}
      </div>
    </button>
  );
};

// ─── Round detail (right column) ─────────────────────────────────────────────

interface RoundDetailProps {
  round: Round;
  expandedSections: Set<string>;
  onToggleSection: (key: string) => void;
  savingsMap?: Map<string, OptimizationItem>;
  onNavigateSubagent?: (ref: SubagentTrajectoryRef) => void;
}

const RoundDetail: React.FC<RoundDetailProps> = ({
  round, expandedSections, onToggleSection, savingsMap, onNavigateSubagent,
}) => {
  const stats = roundStats(round);

  return (
    <div className="bg-white rounded-xl shadow-sm border border-gray-200 p-5">
      {/* Detail header */}
      <div className="flex items-center gap-3 flex-wrap pb-4 border-b border-gray-100">
        <h3 className="text-sm font-semibold text-gray-900">{round.label} · 对话详情</h3>
        <span className="px-1.5 py-0.5 bg-gray-100 text-gray-500 rounded text-xs">{round.steps.length} 步</span>
        {stats.toolCallCount > 0 && (
          <span className="px-1.5 py-0.5 bg-orange-50 text-orange-600 rounded text-xs">🔧 {stats.toolCallCount} 次工具调用</span>
        )}
        {(stats.promptSum > 0 || stats.completionSum > 0) && (
          <span className="px-1.5 py-0.5 bg-blue-50 text-blue-600 rounded text-xs">
            {fmtTokens(stats.promptSum)} in / {fmtTokens(stats.completionSum)} out
          </span>
        )}
      </div>

      {/* Step timeline within the round */}
      <div className="relative pl-4 pt-4">
        <div className="absolute left-[5px] top-8 bottom-4 w-0.5 bg-gray-200" />
        {round.steps.map(step => (
          <StepCard
            key={step.step_id}
            step={step}
            expandedSections={expandedSections}
            onToggleSection={onToggleSection}
            savingsMap={savingsMap}
            onNavigateSubagent={onNavigateSubagent}
          />
        ))}
      </div>
    </div>
  );
};

// ─── AgentInfoCard ────────────────────────────────────────────────────────────

const AgentInfoCard: React.FC<{ doc: AtifDocument }> = ({ doc }) => {
  const agent = doc.agent ?? { name: 'unknown', version: '—', model_name: undefined, tool_definitions: [] };
  const toolCount = Array.isArray(agent.tool_definitions) ? agent.tool_definitions.length : 0;

  return (
    <div className="bg-white rounded-xl shadow-sm border border-gray-200 p-5 lg:col-span-2">
      <h3 className="text-sm font-semibold text-gray-900 mb-3">Agent 信息</h3>
      <div className="space-y-2 text-sm">
        {[
          { label: '名称', value: agent.name },
          { label: '版本', value: agent.version },
          { label: '模型', value: agent.model_name ?? '—' },
          { label: '工具定义', value: `${toolCount} 个` },
        ].map(({ label, value }) => (
          <div key={label} className="flex items-center justify-between">
            <span className="text-gray-500">{label}</span>
            <span className="text-gray-900 font-medium font-mono">{value}</span>
          </div>
        ))}
      </div>
    </div>
  );
};

// ─── MetricCard ───────────────────────────────────────────────────────────────

const MetricCard: React.FC<{ label: string; value: string; color: string; sub?: string }> = ({ label, value, color, sub }) => (
  <div className="bg-white rounded-xl shadow-sm border border-gray-200 p-5 flex flex-col justify-center">
    <span className="text-sm text-gray-500 mb-1">{label}</span>
    <span className={`text-2xl font-bold ${color}`}>{value}</span>
    {sub && <span className="text-xs text-gray-400 mt-1">{sub}</span>}
  </div>
);

// ─── Session loading (two stores) ─────────────────────────────────────────────
// A session lives in either store: eBPF-captured genai events (genai_events.db)
// or a collector-ingested log trajectory (trajectories.db). Both now serve the
// same ATIF schema, so only the lookup differs — try the export first, since it
// carries token metrics, then fall back for sessions never seen on the wire.

function isAtifDocument(value: unknown): value is AtifDocument {
  return !!value
    && typeof value === 'object'
    && !Array.isArray(value)
    && typeof (value as { schema_version?: unknown }).schema_version === 'string'
    && String((value as { schema_version: string }).schema_version).startsWith('ATIF');
}

async function loadSessionDoc(sessionId: string): Promise<AtifDocument> {
  try {
    const exported = await fetchAtifBySession(sessionId);
    if (isAtifDocument(exported)) return exported;
  } catch (e: any) {
    if (e?.status !== 404) throw e;
  }

  try {
    const collected = await fetchTrajectoryAtif(sessionId);
    if (isAtifDocument(collected)) return collected;
    throw new Error(`采集轨迹格式异常：${sessionId}`);
  } catch (fallbackErr: any) {
    if (fallbackErr?.status === 404) {
      throw new Error(`未找到该 Session：${sessionId}（既无 eBPF 捕获记录，也无采集轨迹）`);
    }
    throw fallbackErr;
  }
}

// ─── Main Page ────────────────────────────────────────────────────────────────

export const AtifViewerPage: React.FC = () => {
  const [searchParams, setSearchParams] = useSearchParams();
  const searchParamsRef = useRef(searchParams);

  useEffect(() => {
    searchParamsRef.current = searchParams;
  }, [searchParams]);

  // Input state
  const [queryType, setQueryType] = useState<'session' | 'conversation'>(
    (searchParams.get('type') as 'session' | 'conversation') || 'session'
  );
  const [queryId, setQueryId] = useState(searchParams.get('id') || '');

  // Data state
  const [doc, setDoc] = useState<AtifDocument | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savingsDetail, setSavingsDetail] = useState<SessionSavingsDetail | null>(null);

  // Build tool_call_id → OptimizationItem map for StepCard badges
  const savingsMap = React.useMemo(() => {
    if (!savingsDetail?.items?.length) return new Map<string, OptimizationItem>();
    return new Map(savingsDetail.items.map(item => [item.id, item]));
  }, [savingsDetail]);

  // UI state
  const [expandedSections, setExpandedSections] = useState<Set<string>>(new Set());
  const [selectedRound, setSelectedRound] = useState<number | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Subagent navigation: a trajectory-id path from the root, mirrored in the URL
  // so refreshing or sharing the link lands on the same node.
  const [nodePath, setNodePath] = useState<string[]>([]);
  const graphRef = useRef<HTMLDivElement>(null);

  const tree = React.useMemo(() => buildTrajectoryTree(doc), [doc]);
  const selectedNode = tree ? findNodeByPath(tree, nodePath) : null;
  const activeDoc = selectedNode?.doc ?? doc;

  const selectNode = useCallback((node: TrajNode) => {
    // Subagents referenced by session id only (never embedded) still live on
    // their own page — the sole remaining case where a new tab is warranted.
    if (node.externalSessionId) {
      window.open(`#/atif?type=session&id=${encodeURIComponent(node.externalSessionId)}`, '_blank');
      return;
    }
    setNodePath(node.path);
    setExpandedSections(new Set());
    setSelectedRound(initialRound(groupIntoRounds(stepsOf(node.doc)), new Set()));
    const next = new URLSearchParams(searchParamsRef.current);
    if (node.path.length > 0) next.set('node', encodeNodePath(node.path));
    else next.delete('node');
    setSearchParams(next);
  }, [setSearchParams]);

  /** Step-level "🤖 子代理轨迹" button: select the node in the graph above. */
  const navigateToSubagent = useCallback((ref: SubagentTrajectoryRef) => {
    const target = tree ? findNodeByRef(tree, ref) : null;
    if (target) {
      selectNode(target);
      graphRef.current?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
      return;
    }
    if (ref.session_id) {
      window.open(`#/atif?type=session&id=${encodeURIComponent(ref.session_id)}`, '_blank');
    } else if (ref.trajectory_path) {
      setError(`外部子轨迹引用暂不支持: ${ref.trajectory_path}`);
    } else {
      setError('无法解析子轨迹引用：缺少 trajectory_id 或 trajectory_path');
    }
  }, [tree, selectNode]);

  const toggleSection = useCallback((key: string) => {
    setExpandedSections(prev => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  // Load data
  const handleLoad = useCallback(async (type?: 'session' | 'conversation', id?: string) => {
    const t = type ?? queryType;
    const i = id ?? queryId;
    if (!i.trim()) return;

    const nextParams: Record<string, string> = { type: t, id: i.trim() };
    const currentSearchParams = searchParamsRef.current;
    // Node selection and highlights only carry over when the target is unchanged
    // (an explicit reload of a different id starts at the root trajectory).
    let initialPath: string[] = [];
    if (currentSearchParams.get('id') === i.trim()) {
      const highlightCallId = currentSearchParams.get('highlight_call_id');
      const interruptionId = currentSearchParams.get('interruption_id');
      const node = currentSearchParams.get('node');
      if (highlightCallId) nextParams.highlight_call_id = highlightCallId;
      if (interruptionId) nextParams.interruption_id = interruptionId;
      if (node) {
        nextParams.node = node;
        initialPath = decodeNodePath(node);
      }
    }
    setSearchParams(nextParams, { replace: true });
    setLoading(true);
    setError(null);
    setDoc(null);
    setNodePath(initialPath);
    setExpandedSections(new Set());
    setSelectedRound(null);

    try {
      let data: AtifDocument;
      if (t === 'conversation') {
        data = await fetchAtifByConversation(i.trim());
      } else {
        data = await loadSessionDoc(i.trim());
      }
      setDoc(data);
      const sections = highlightedSections(data, nextParams.highlight_call_id ?? null);
      setExpandedSections(sections);
      // Round selection follows the node the URL restored, not always the root.
      const restoredTree = buildTrajectoryTree(data);
      const restoredDoc = restoredTree
        ? (findNodeByPath(restoredTree, initialPath).doc ?? data)
        : data;
      setSelectedRound(initialRound(groupIntoRounds(stepsOf(restoredDoc)), sections));
      // Fetch savings data for the session
      if (data.session_id) {
        fetchSessionSavings(data.session_id)
          .then(setSavingsDetail)
          .catch(() => setSavingsDetail(null));
      }
    } catch (e: any) {
      setError(e.message ?? '加载失败');
    } finally {
      setLoading(false);
    }
  }, [queryType, queryId, setSearchParams]);

  // Back/forward navigation changes the URL without going through selectNode,
  // so mirror the `node` param back into state when they diverge.
  useEffect(() => {
    if (!tree) return;
    const urlPath = decodeNodePath(searchParams.get('node'));
    if (encodeNodePath(urlPath) === encodeNodePath(nodePath)) return;
    setNodePath(urlPath);
    setExpandedSections(new Set());
    setSelectedRound(initialRound(groupIntoRounds(stepsOf(findNodeByPath(tree, urlPath).doc)), new Set()));
  }, [searchParams, tree, nodePath]);

  // Auto-load from URL on mount
  useEffect(() => {
    const urlType = searchParams.get('type') as 'session' | 'conversation' | null;
    const urlId = searchParams.get('id');
    if (urlType && urlId) {
      setQueryType(urlType);
      setQueryId(urlId);
      handleLoad(urlType, urlId);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // JSON file import
  const handleFileImport = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = (ev) => {
      try {
        const parsed = JSON.parse(ev.target?.result as string);
        if (!parsed.schema_version || !String(parsed.schema_version).startsWith('ATIF')) {
          setError('JSON 解析失败：缺少 schema_version 字段或非 ATIF 格式');
          return;
        }
        setDoc(parsed as AtifDocument);
        setNodePath([]);
        setError(null);
        setQueryId(parsed.session_id ?? '');
        setExpandedSections(new Set());
        setSelectedRound(initialRound(groupIntoRounds(stepsOf(parsed as AtifDocument)), new Set()));
      } catch {
        setError('JSON 解析失败，请检查文件格式');
      }
    };
    reader.readAsText(file);
    e.target.value = '';
  }, []);

  // JSON download
  const handleDownload = useCallback(() => {
    if (!doc) return;
    const blob = new Blob([JSON.stringify(doc, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `atif-${(doc.session_id ?? 'trajectory').slice(0, 16)}.json`;
    a.click();
    URL.revokeObjectURL(url);
  }, [doc]);

  // Compute metrics (fallback when final_metrics is partial)
  const steps = activeDoc?.steps ?? [];
  const rounds = React.useMemo(() => groupIntoRounds(activeDoc?.steps ?? []), [activeDoc]);
  const activeRound = rounds.find(r => r.key === selectedRound) ?? null;
  const computedMetrics = activeDoc ? (() => {
    const fm = activeDoc.final_metrics;
    let promptSum = 0, completionSum = 0, cachedSum = 0;
    for (const s of steps) {
      if (s.metrics) {
        promptSum += s.metrics.prompt_tokens ?? 0;
        completionSum += s.metrics.completion_tokens ?? 0;
        cachedSum += s.metrics.cached_tokens ?? 0;
      }
    }
    return {
      steps: fm?.total_steps ?? steps.length,
      prompt: fm?.total_prompt_tokens ?? promptSum,
      completion: fm?.total_completion_tokens ?? completionSum,
      cached: fm?.total_cached_tokens ?? cachedSum,
    };
  })() : null;

  return (
    <>
      {/* Header */}
      <header className="bg-white border-b border-gray-200 px-6 py-4">
        <div className="max-w-screen-xl mx-auto flex items-center gap-4">
          <div className="flex-1 min-w-0">
            <h1 className="text-lg font-bold text-gray-900">轨迹查看</h1>
            {doc && (
              <div className="flex items-center gap-2 mt-0.5">
                <span className="px-2 py-0.5 bg-gray-100 text-gray-600 rounded text-xs">
                  {doc.schema_version}
                </span>
                <span className="text-xs text-gray-400 font-mono truncate">{doc.session_id}</span>
              </div>
            )}
          </div>
          {doc && (
            <button onClick={handleDownload}
              className="flex-shrink-0 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 text-gray-700 rounded-lg text-sm transition-colors">
              ⬇️ 下载 JSON
            </button>
          )}
        </div>
      </header>

      <main className="max-w-screen-xl mx-auto px-6 py-6 space-y-6">
        {/* Input Controls */}
        <div className="bg-white rounded-xl shadow-sm border border-gray-200 p-4 flex flex-wrap items-end gap-4">
          {/* Type toggle */}
          <div className="flex gap-1">
            {(['session', 'conversation'] as const).map(t => (
              <button
                key={t}
                onClick={() => setQueryType(t)}
                className={`px-3 py-1.5 text-sm rounded-lg transition-colors ${
                  queryType === t
                    ? 'bg-blue-600 text-white'
                    : 'bg-gray-100 text-gray-600 hover:bg-gray-200'
                }`}
              >
                按 {t === 'conversation' ? 'Conversation' : 'Session'}
              </button>
            ))}
          </div>

          {/* ID input */}
          <div className="flex-1 min-w-[240px]">
            <input
              type="text"
              value={queryId}
              onChange={e => setQueryId(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter') handleLoad(); }}
              placeholder={queryType === 'conversation' ? '输入 Conversation ID...' : '输入 Session ID...'}
              className="w-full border border-gray-300 rounded-lg px-3 py-1.5 text-sm font-mono focus:outline-none focus:ring-2 focus:ring-blue-400"
            />
          </div>

          {/* Load button */}
          <button
            onClick={() => handleLoad()}
            disabled={loading || !queryId.trim()}
            className="px-4 py-1.5 bg-blue-600 text-white text-sm rounded-lg hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
          >
            {loading ? '加载中...' : '加载'}
          </button>

          {/* File import */}
          <input
            ref={fileInputRef}
            type="file"
            accept=".json"
            onChange={handleFileImport}
            className="hidden"
          />
          <button
            onClick={() => fileInputRef.current?.click()}
            className="px-3 py-1.5 bg-gray-100 hover:bg-gray-200 text-gray-700 text-sm rounded-lg transition-colors"
          >
            📁 导入 JSON
          </button>
        </div>

        {/* Error */}
        {error && (
          <div className="bg-red-50 border border-red-200 rounded-xl p-4 text-red-600 text-sm">
            ⚠️ {error}
          </div>
        )}

        {/* Loading */}
        {loading && (
          <div className="flex items-center justify-center py-16">
            <div className="text-center">
              <div className="w-8 h-8 border-4 border-blue-200 border-t-blue-600 rounded-full animate-spin mx-auto mb-4" />
              <p className="text-gray-600">加载中...</p>
            </div>
          </div>
        )}

        {/* Empty state */}
        {!loading && !doc && !error && (
          <div className="flex items-center justify-center py-24">
            <div className="text-center">
              <p className="text-3xl text-gray-300 mb-4">ATIF</p>
              <p className="text-gray-500">请输入 Session 或 Conversation ID，然后点击「加载」</p>
              <p className="text-gray-400 text-sm mt-1">或导入本地 ATIF JSON 文件</p>
            </div>
          </div>
        )}

        {/* Loaded content */}
        {doc && !loading && (
          <>
            {/* Agent info + Metrics */}
            <div className="grid grid-cols-1 lg:grid-cols-5 gap-4">
              <AgentInfoCard doc={activeDoc!} />
              {computedMetrics && (
                <>
                  <MetricCard
                    label="总步骤数"
                    value={String(computedMetrics.steps)}
                    color="text-indigo-600"
                  />
                  <MetricCard
                    label="总输入 Token"
                    value={fmtTokens(computedMetrics.prompt)}
                    color="text-blue-600"
                    sub={computedMetrics.cached > 0 ? `其中缓存: ${fmtTokens(computedMetrics.cached)}` : undefined}
                  />
                  <MetricCard
                    label="总输出 Token"
                    value={fmtTokens(computedMetrics.completion)}
                    color="text-green-600"
                  />
                </>
              )}
            </div>

            {/* Token Savings Comparison Card */}
            {savingsDetail && savingsDetail.total_compounded_saved > 0 && (
              <div className="bg-white rounded-xl shadow-sm border border-gray-200 p-5">
                <h3 className="text-sm font-semibold text-gray-900 mb-3">Token 节省对比</h3>
                <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 mb-4">
                  <div>
                    <span className="text-xs text-gray-500">原始 Token（未优化）</span>
                    <p className="text-xl font-bold text-gray-700">{fmtTokens(savingsDetail.total_original_tokens)}</p>
                  </div>
                  <div>
                    <span className="text-xs text-gray-500">实际 Token（优化后）</span>
                    <p className="text-xl font-bold text-blue-600">{fmtTokens(savingsDetail.total_actual_tokens)}</p>
                  </div>
                  <div>
                    <span className="text-xs text-gray-500">节省</span>
                    <p className="text-xl font-bold text-green-600">
                      -{fmtTokens(savingsDetail.total_compounded_saved)}
                      <span className="text-sm font-normal text-gray-400 ml-1">
                        ({savingsDetail.savings_rate.toFixed(1)}%)
                      </span>
                    </p>
                  </div>
                </div>
                {/* Comparison bar */}
                <div className="space-y-1">
                  <div className="flex items-center gap-2">
                    <span className="text-xs text-gray-400 w-10">原始</span>
                    <div className="flex-1 h-3 bg-gray-100 rounded-full overflow-hidden">
                      <div className="h-full bg-gray-400 rounded-full" style={{ width: '100%' }} />
                    </div>
                  </div>
                  <div className="flex items-center gap-2">
                    <span className="text-xs text-gray-400 w-10">实际</span>
                    <div className="flex-1 h-3 bg-gray-100 rounded-full overflow-hidden">
                      <div
                        className="h-full bg-green-500 rounded-full"
                        style={{
                          width: savingsDetail.total_original_tokens > 0
                            ? `${(savingsDetail.total_actual_tokens / savingsDetail.total_original_tokens) * 100}%`
                            : '100%',
                        }}
                      />
                    </div>
                  </div>
                </div>
              </div>
            )}

            {/* Subagent topology graph — the single navigation surface */}
            {tree && (
              <div ref={graphRef}>
                <SubagentGraph root={tree} selectedPath={nodePath} onSelect={selectNode} />
              </div>
            )}

            {/* Round master-detail: left = round list, right = round detail */}
            <div>
              <h2 className="text-lg font-semibold text-gray-900 mb-4">
                {selectedNode && selectedNode.depth > 0 && (
                  <span className="text-indigo-600">{selectedNode.label} · </span>
                )}
                交互轨迹
                <span className="ml-2 text-sm font-normal text-gray-400">
                  共 {rounds.length} 轮对话 · {steps.length} 步
                </span>
              </h2>

              {steps.length === 0 ? (
                <div className="bg-white rounded-xl border border-gray-200 p-8 text-center">
                  <p className="text-4xl text-gray-300 mb-2">--</p>
                  <p className="text-gray-400">该轨迹暂无步骤数据</p>
                </div>
              ) : (
                <div className="grid grid-cols-1 lg:grid-cols-[minmax(280px,1fr)_2fr] gap-4 items-start">
                  {/* Left: round list */}
                  <div className="space-y-2 lg:max-h-[calc(100vh-200px)] lg:overflow-y-auto lg:sticky lg:top-4 pr-1">
                    {rounds.map(round => (
                      <RoundListItem
                        key={round.key}
                        round={round}
                        isActive={round.key === selectedRound}
                        onSelect={() => setSelectedRound(round.key)}
                      />
                    ))}
                  </div>

                  {/* Right: selected round detail */}
                  <div>
                    {activeRound ? (
                      <RoundDetail
                        round={activeRound}
                        expandedSections={expandedSections}
                        onToggleSection={toggleSection}
                        savingsMap={savingsMap}
                        onNavigateSubagent={navigateToSubagent}
                      />
                    ) : (
                      <div className="bg-white rounded-xl border border-gray-200 p-12 text-center">
                        <p className="text-gray-400">点击左侧轮次查看详情</p>
                      </div>
                    )}
                  </div>
                </div>
              )}
            </div>
          </>
        )}
      </main>
    </>
  );
};
