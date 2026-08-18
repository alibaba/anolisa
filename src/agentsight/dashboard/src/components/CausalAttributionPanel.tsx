import React, { useEffect, useMemo, useState } from 'react';
import { runCausalAttribution } from '../utils/apiClient';
import type {
  CausalAttrib,
  CausalAttributionRequest,
  CausalCase,
  CausalEdge,
  CausalNode,
  CausalNodeKind,
} from '../types/causal';

const DEFAULT_PROMPTS = [
  '这轮不符合预期，哪里有问题？',
  '引用靠谱吗？有没有未核实的？',
  '分析对象对吗？有没有跑偏？',
  '声明成功的步骤真的生效了吗？',
];

// ─── Local history (per (session, round)) ─────────────────────────────────────

interface HistoryEntry {
  complaint: string;
  ranAt: string;
  caseData: CausalCase;
}

const HISTORY_KEY_PREFIX = 'agentsight.causal.history.v1';
const HISTORY_MAX_ENTRIES = 20;

function historyKey(
  sessionId: string,
  roundIndex?: number,
  idKind?: 'session' | 'conversation',
): string {
  return `${HISTORY_KEY_PREFIX}:${idKind ?? 'session'}:${sessionId}:r${roundIndex ?? 'all'}`;
}

function readHistory(
  sessionId: string,
  roundIndex?: number,
  idKind?: 'session' | 'conversation',
): HistoryEntry[] {
  if (typeof window === 'undefined' || !window.localStorage) return [];
  try {
    const raw = window.localStorage.getItem(historyKey(sessionId, roundIndex, idKind));
    if (!raw) return [];
    const parsed = JSON.parse(raw) as HistoryEntry[];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function writeHistory(
  sessionId: string,
  roundIndex: number | undefined,
  idKind: 'session' | 'conversation' | undefined,
  entries: HistoryEntry[],
): void {
  if (typeof window === 'undefined' || !window.localStorage) return;
  try {
    const trimmed = entries.slice(-HISTORY_MAX_ENTRIES);
    window.localStorage.setItem(
      historyKey(sessionId, roundIndex, idKind),
      JSON.stringify(trimmed),
    );
  } catch {
    // localStorage quota / disabled — silently ignore.
  }
}

function appendHistory(
  sessionId: string,
  roundIndex: number | undefined,
  idKind: 'session' | 'conversation' | undefined,
  complaint: string,
  caseData: CausalCase,
): HistoryEntry[] {
  const prev = readHistory(sessionId, roundIndex, idKind);
  const next: HistoryEntry[] = [
    ...prev.filter((e) => e.complaint !== complaint),
    { complaint, ranAt: new Date().toISOString(), caseData },
  ];
  writeHistory(sessionId, roundIndex, idKind, next);
  return next;
}

interface CausalAttributionPanelProps {
  sessionId: string;
  roundIndex?: number;
  roundLabel?: string;
  /** Whether the selected round only carries the system prompt. Supplied as a
   *  flag rather than inferred from `roundLabel`, which is localized. */
  isPreambleRound?: boolean;
  /** "conversation" when the parent page is viewing a conversation_id; unset otherwise. */
  idKind?: 'session' | 'conversation';
  /** Called when the user clicks a causal node — parent scrolls the trajectory to that step. */
  onScrollToStep?: (stepId: number) => void;
}

/**
 * Human-readable legend for each node color. Displayed compactly above the
 * causal graph so users know what each color means without needing the docs.
 */
const NODE_KIND_LEGEND: Array<{
  kind: CausalNodeKind;
  label: string;
  desc: string;
}> = [
  { kind: 'root', label: '元凶', desc: '首次产生缺陷' },
  { kind: 'sym', label: '症状', desc: '忠实处理被污染输入' },
  { kind: 'seed', label: '萌芽', desc: '小问题被放大' },
  { kind: 'shipped', label: '问题交付', desc: '坏结果交给用户' },
  { kind: 'good', label: '达成', desc: '最终结论正确' },
  { kind: 'user', label: '用户', desc: '用户输入/干预' },
  { kind: 'env', label: '环境', desc: '外部触发' },
  { kind: 'cf', label: '反事实', desc: '如果…会怎样' },
  { kind: 'ok', label: '正常', desc: '无缺陷' },
];

const NODE_STYLE: Record<
  CausalNodeKind,
  { fill: string; stroke: string; text: string; tag: string }
> = {
  ok: { fill: '#e7f3ec', stroke: '#3f9a63', text: '#22303f', tag: '#3f9a63' },
  seed: { fill: '#fff1d6', stroke: '#e08a00', text: '#22303f', tag: '#e08a00' },
  root: { fill: '#fde2dd', stroke: '#c0392b', text: '#22303f', tag: '#c0392b' },
  sym: { fill: '#fff2d8', stroke: '#c98a12', text: '#22303f', tag: '#c98a12' },
  good: { fill: '#e7f3ec', stroke: '#2e7d46', text: '#22303f', tag: '#2e7d46' },
  env: { fill: '#fff2d8', stroke: '#c98a12', text: '#22303f', tag: '#c98a12' },
  shipped: { fill: '#f6d7d1', stroke: '#7b241c', text: '#22303f', tag: '#7b241c' },
  cf: { fill: '#f9e8e8', stroke: '#7b241c', text: '#22303f', tag: '#7b241c' },
  user: { fill: '#d6eaf8', stroke: '#1a5276', text: '#22303f', tag: '#1a5276' },
};

const ATTRIB_STYLE: Record<CausalAttrib, { bg: string; fg: string; label: string }> = {
  model: { bg: '#ede7fb', fg: '#5b3fb0', label: '模型 model' },
  skill: { bg: '#dcf1ee', fg: '#1f7a68', label: '工具/技能 skill' },
  prompt: { bg: '#fdecd2', fg: '#a4650a', label: '指令 prompt' },
  agent: { bg: '#dde7f2', fg: '#37618a', label: '编排 agent' },
};

const NODE_W = 220;
const NODE_H = 78;
const GAP_Y = 48;
const GAP_X = 40;
const START_Y = 20;
const MAIN_X = 20;

interface Laid {
  node: CausalNode;
  x: number;
  y: number;
  cx: number;
  cy: number;
}

function layoutNodes(caseData: CausalCase): Map<string, Laid> {
  // Vertical layout: the backend returns nodes in reverse chronological order
  // (conclusion first, root last). Reverse again so the ROOT sits at the top
  // and the conclusion at the bottom — the natural "cause → effect" reading.
  const main = caseData.nodes
    .filter((n) => n.kind !== 'cf')
    .slice()
    .reverse();
  const cfs = caseData.nodes.filter((n) => n.kind === 'cf');

  const laid = new Map<string, Laid>();
  main.forEach((node, i) => {
    const y = START_Y + i * (NODE_H + GAP_Y);
    laid.set(node.id, {
      node,
      x: MAIN_X,
      y,
      cx: MAIN_X + NODE_W / 2,
      cy: y + NODE_H / 2,
    });
  });

  // Counterfactual nodes sit to the right of their parent chain, indented
  // one column over. Anchor each CF to the closest main-chain Y so it
  // visually lines up with the step it's "what-if" about.
  cfs.forEach((node, i) => {
    const anchorY =
      main.length > 0
        ? START_Y + Math.min(i, main.length - 1) * (NODE_H + GAP_Y)
        : START_Y + i * (NODE_H + GAP_Y);
    const x = MAIN_X + NODE_W + GAP_X;
    laid.set(node.id, {
      node,
      x,
      y: anchorY,
      cx: x + NODE_W / 2,
      cy: anchorY + NODE_H / 2,
    });
  });

  return laid;
}

function ArrowDefs() {
  return (
    <defs>
      <marker id="causal-arrow-normal" markerWidth="10" markerHeight="10" refX="9" refY="3" orient="auto">
        <path d="M0,0 L9,3 L0,6 Z" fill="#9aa7b5" />
      </marker>
      <marker id="causal-arrow-bad" markerWidth="10" markerHeight="10" refX="9" refY="3" orient="auto">
        <path d="M0,0 L9,3 L0,6 Z" fill="#c0392b" />
      </marker>
      <marker id="causal-arrow-refute" markerWidth="10" markerHeight="10" refX="9" refY="3" orient="auto">
        <path d="M0,0 L9,3 L0,6 Z" fill="#2e7d46" />
      </marker>
    </defs>
  );
}

function EdgeLine({
  edge,
  laid,
}: {
  edge: CausalEdge;
  laid: Map<string, Laid>;
}) {
  const from = laid.get(edge.a);
  const to = laid.get(edge.b);
  if (!from || !to) return null;

  const isCf = from.node.kind === 'cf' || to.node.kind === 'cf';
  const sameColumn = Math.abs(from.x - to.x) < 10;

  if (edge.type === 'refute') {
    // Arc to the LEFT of the main chain so it doesn't collide with CF nodes
    // (which live to the right). Control point 60px left of the column.
    const midY = (from.cy + to.cy) / 2;
    const arcX = Math.min(from.x, to.x) - 60;
    const d = `M${from.x},${from.cy} Q${arcX},${midY} ${to.x},${to.cy}`;
    return (
      <g>
        <path
          d={d}
          fill="none"
          stroke="#2e7d46"
          strokeWidth={2}
          strokeDasharray="6 4"
          markerEnd="url(#causal-arrow-refute)"
        />
        <text x={arcX - 4} y={midY} fontSize={11} textAnchor="end" fill="#2e7d46">
          现实反证
        </text>
      </g>
    );
  }

  if (isCf || !sameColumn) {
    // Horizontal: main column → counterfactual column. Arrow points from
    // the main-chain node to the CF node.
    const main = from.x < to.x ? from : to;
    const cf = from.x < to.x ? to : from;
    return (
      <line
        x1={main.x + NODE_W}
        y1={main.cy}
        x2={cf.x}
        y2={cf.cy}
        stroke="#c0392b"
        strokeWidth={2.4}
        strokeDasharray="6 4"
        markerEnd="url(#causal-arrow-bad)"
      />
    );
  }

  // Main chain: vertical edge from parent's bottom to child's top. Parent
  // is whichever node has the smaller Y (higher on the page).
  const parent = from.y < to.y ? from : to;
  const child = from.y < to.y ? to : from;
  const stroke = edge.type === 'bad' ? '#c0392b' : '#c2ccd6';
  const marker = edge.type === 'bad' ? 'url(#causal-arrow-bad)' : 'url(#causal-arrow-normal)';
  return (
    <line
      x1={parent.cx}
      y1={parent.y + NODE_H}
      x2={child.cx}
      y2={child.y}
      stroke={stroke}
      strokeWidth={edge.type === 'bad' ? 2.4 : 1.5}
      markerEnd={marker}
    />
  );
}

function NodeRect({
  laid,
  selected,
  onClick,
}: {
  laid: Laid;
  selected: boolean;
  onClick: () => void;
}) {
  const { node, x, y } = laid;
  const style =
    NODE_STYLE[node.kind] ?? {
      fill: '#eef2f7',
      stroke: '#94a3b8',
      text: '#22303f',
      tag: '#64748b',
    };
  const isCf = node.kind === 'cf';
  const dashArray = isCf ? '5 3' : undefined;
  // Truncate the evaluator's summary so it fits within the node width. The
  // full text is shown in the detail panel below the graph.
  const shortTag = (node.tag || '').length > 14
    ? `${(node.tag || '').slice(0, 13)}…`
    : (node.tag || '');

  return (
    <g style={{ cursor: 'pointer' }} onClick={onClick}>
      <rect
        x={x}
        y={y}
        width={NODE_W}
        height={NODE_H}
        rx={12}
        fill={style.fill}
        stroke={style.stroke}
        strokeWidth={selected ? 3 : 2.2}
        strokeDasharray={dashArray}
      />
      <text x={x + NODE_W / 2} y={y + 26} fontSize={13} fontWeight={700} textAnchor="middle" fill={style.text}>
        {shortTag}
      </text>
      <text x={x + NODE_W / 2} y={y + 48} fontSize={10.5} textAnchor="middle" fill={style.tag}>
        {typeof node.step === 'number' ? `step ${node.step}` : ''}
      </text>
    </g>
  );
}

function CausalGraph({
  caseData,
  onScrollToStep,
}: {
  caseData: CausalCase;
  onScrollToStep?: (stepId: number) => void;
}) {
  const laidMap = useMemo(() => layoutNodes(caseData), [caseData]);
  // Default to the topmost node (root cause) so the user's eye starts at
  // the origin of the failure chain.
  const laidList = Array.from(laidMap.values()).sort((a, b) => a.y - b.y);
  const [selectedId, setSelectedId] = useState<string | null>(laidList[0]?.node.id ?? null);
  const [showAnalysis, setShowAnalysis] = useState(false);

  const xs = laidList.map((l) => l.x + NODE_W);
  const ys = laidList.map((l) => l.y + NODE_H);
  const width = Math.max(320, (xs.length ? Math.max(...xs) : 320) + 80);
  const height = Math.max(200, (ys.length ? Math.max(...ys) : 200) + 40);

  const selected = selectedId ? laidMap.get(selectedId)?.node : null;

  const handleSelectNode = (node: CausalNode) => {
    setSelectedId(node.id);
    setShowAnalysis(false);
    if (typeof node.step === 'number' && onScrollToStep) {
      onScrollToStep(node.step);
    }
  };

  return (
    <div>
      {/* Color legend — one row, wraps on narrow screens */}
      <div className="mb-3 flex flex-wrap gap-x-3 gap-y-1 text-[10px] text-gray-600">
        {NODE_KIND_LEGEND.map((item) => {
          const style = NODE_STYLE[item.kind];
          return (
            <span key={item.kind} className="inline-flex items-center gap-1">
              <span
                className="inline-block w-2.5 h-2.5 rounded-sm border"
                style={{ background: style.fill, borderColor: style.stroke }}
              />
              <span className="font-semibold text-gray-700">{item.label}</span>
              <span className="text-gray-400">{item.desc}</span>
            </span>
          );
        })}
      </div>

      <div className="mb-2 text-xs text-gray-500 leading-relaxed">
        <b>读法</b>：最上是根因，顺着箭头向下追问“导致了什么”，越往下越靠近最终结论；
        <b className="text-red-700"> 红边框 = 元凶</b>。点节点：左侧轨迹同步跳到该步。
      </div>

      <div className="overflow-y-auto border border-gray-200 rounded-lg bg-slate-50 p-3" style={{ maxHeight: '60vh' }}>
        <svg viewBox={`0 0 ${width} ${height}`} width={width} height={height}>
          <ArrowDefs />
          {caseData.edges.map((e, i) => (
            <EdgeLine key={i} edge={e} laid={laidMap} />
          ))}
          {Array.from(laidMap.values()).map((l) => (
            <NodeRect
              key={l.node.id}
              laid={l}
              selected={l.node.id === selectedId}
              onClick={() => handleSelectNode(l.node)}
            />
          ))}
        </svg>
      </div>

      {/* Node detail: original trajectory content first, evaluator analysis second */}
      <div className="mt-3 bg-gray-50 border border-gray-200 rounded-lg p-3">
        {selected ? (
          <div>
            <div className="flex items-center gap-2 mb-2 flex-wrap">
              <span
                className="inline-block text-xs font-semibold px-2 py-0.5 rounded-full border"
                style={{
                  background:
                    NODE_STYLE[selected.kind]?.fill ?? '#eef2f7',
                  borderColor:
                    NODE_STYLE[selected.kind]?.stroke ?? '#94a3b8',
                  color:
                    NODE_STYLE[selected.kind]?.stroke ?? '#64748b',
                }}
              >
                {selected.tag || selected.kind}
              </span>
              {typeof selected.step === 'number' && (
                <button
                  onClick={() => onScrollToStep?.(selected.step as number)}
                  className="text-xs text-indigo-600 hover:text-indigo-800 font-medium"
                  title="左侧轨迹跳转到这一步"
                >
                  step {selected.step} ↗
                </button>
              )}
            </div>

            {/* Original trajectory content — the raw observation from this step */}
            {selected.raw ? (
              <div className="mb-2">
                <div className="text-[10px] uppercase tracking-wide text-gray-400 font-semibold mb-1">
                  该步原始记录
                </div>
                <pre className="p-2 bg-white border border-gray-200 rounded text-xs text-gray-700 whitespace-pre-wrap break-words max-h-48 overflow-y-auto">
                  {selected.raw}
                </pre>
              </div>
            ) : (
              <p className="text-sm text-gray-600 italic mb-2">
                {(selected.plain || '').slice(0, 200)}
              </p>
            )}

            {/* Evaluator analysis — basis + defect_type, collapsible */}
            <details
              open={showAnalysis}
              onToggle={(e) => setShowAnalysis((e.target as HTMLDetailsElement).open)}
              className="border-t border-dashed border-gray-200 pt-2"
            >
              <summary className="cursor-pointer text-xs font-semibold text-gray-600">
                归因分析 · 展开查看
              </summary>
              <div className="mt-2 space-y-1 text-xs">
                {selected.plain && (
                  <p className="text-gray-700 leading-relaxed">{selected.plain}</p>
                )}
                {selected.foot && (
                  <p className="text-gray-500">
                    <span className="font-semibold text-gray-600">依据：</span>
                    {selected.foot}
                  </p>
                )}
              </div>
            </details>
          </div>
        ) : (
          <p className="text-sm text-gray-500">点击图中节点查看该步。</p>
        )}
      </div>
    </div>
  );
}

export const CausalAttributionPanel: React.FC<CausalAttributionPanelProps> = ({
  sessionId,
  roundIndex,
  roundLabel,
  isPreambleRound = false,
  idKind,
  onScrollToStep,
}) => {
  const [complaint, setComplaint] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [caseData, setCaseData] = useState<CausalCase | null>(null);
  const [cached, setCached] = useState(false);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [selectedAltIdx, setSelectedAltIdx] = useState<number | null>(null);
  const [stageIdx, setStageIdx] = useState(0);
  const [elapsed, setElapsed] = useState(0);

  // Stage definitions with estimated durations (seconds)
  const stages = [
    { name: '验收标准 + 逐步评估', est: 60 },
    { name: '归因判定', est: 60 },
  ];
  const totalEst = stages.reduce((a, s) => a + s.est, 0);

  // Load prior runs for this (session, round, scope) on mount. If there's at
  // least one, auto-display the most recent so the user sees their last
  // result without re-running the LLM pipeline.
  useEffect(() => {
    const past = readHistory(sessionId, roundIndex, idKind);
    setHistory(past);
    if (past.length > 0) {
      const latest = past[past.length - 1];
      setCaseData(latest.caseData);
      setComplaint(latest.complaint);
      setCached(true);
      setSelectedAltIdx(null);
    }
  }, [sessionId, roundIndex, idKind]);

  const canRun = complaint.trim().length > 0 && !loading;

  const run = async (force: boolean) => {
    if (!canRun) return;
    setLoading(true);
    setError(null);
    setStageIdx(0);
    setElapsed(0);

    // Animate stage progression based on estimated time per stage
    const timer = window.setInterval(() => {
      setElapsed((prev) => {
        const next = prev + 1;
        let cumulative = 0;
        for (let i = 0; i < stages.length; i++) {
          cumulative += stages[i].est;
          if (next < cumulative) {
            setStageIdx(i);
            break;
          }
        }
        return next;
      });
    }, 1000);

    try {
      const req: CausalAttributionRequest = {
        session_id: sessionId,
        round_index: roundIndex,
        complaint: complaint.trim(),
        force,
        id_kind: idKind,
      };
      const res = await runCausalAttribution(req);
      setCaseData(res.case);
      setCached(res.cached);
      setStageIdx(stages.length); // mark complete
      setSelectedAltIdx(null);
      const next = appendHistory(
        sessionId,
        roundIndex,
        idKind,
        complaint.trim(),
        res.case,
      );
      setHistory(next);
    } catch (e) {
      setError(e instanceof Error ? e.message : '归因失败');
    } finally {
      window.clearInterval(timer);
      setLoading(false);
    }
  };

  const loadFromHistory = (entry: HistoryEntry) => {
    setComplaint(entry.complaint);
    setCaseData(entry.caseData);
    setCached(true);
    setSelectedAltIdx(null);
  };

  const clearHistory = () => {
    if (typeof window === 'undefined' || !window.localStorage) return;
    window.localStorage.removeItem(historyKey(sessionId, roundIndex, idKind));
    setHistory([]);
    setCaseData(null);
    setCached(false);
    setComplaint('');
    setSelectedAltIdx(null);
  };

  const attrib = caseData
    ? (ATTRIB_STYLE[caseData.attrib] ?? {
        bg: '#eef2f7',
        fg: '#334155',
        label: caseData.attrib || '(未归因)',
      })
    : null;

  return (
    <div className="bg-white rounded-xl shadow-sm border border-gray-200 p-5">
      <div className="flex items-center gap-3 flex-wrap pb-4 border-b border-gray-100">
        <h3 className="text-sm font-semibold text-gray-900">
          {roundLabel ? `${roundLabel} · 因果归因` : '因果归因'}
        </h3>
        <span className="text-xs text-gray-500">
          输入一句你的不满，由果溯因定位软失败根因
        </span>
      </div>

      {isPreambleRound && (
        <div className="mt-3 px-3 py-2 rounded-lg bg-amber-50 border border-amber-200 text-xs text-amber-800 leading-snug">
          <b>提示：</b>“前置”轮只包含系统 prompt，没有 agent 决策可分析。建议切到左侧某个“第 N 轮”再发起归因，结果会更有意义。
        </div>
      )}

      <div className="pt-4 space-y-3">
        <div>
          <textarea
            value={complaint}
            onChange={(e) => setComplaint(e.target.value)}
            placeholder="如：这轮引用靠谱吗？总觉得不太对…"
            rows={2}
            className="w-full border border-gray-200 rounded-lg px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-400 focus:border-transparent"
          />
          <div className="flex flex-wrap gap-2 mt-2">
            {DEFAULT_PROMPTS.map((p) => (
              <button
                key={p}
                onClick={() => setComplaint(p)}
                className="text-xs px-2.5 py-1 bg-gray-100 hover:bg-gray-200 text-gray-700 rounded-full"
              >
                {p}
              </button>
            ))}
          </div>
        </div>

        <div className="flex items-center gap-3 flex-wrap">
          <button
            onClick={() => run(false)}
            disabled={!canRun}
            className="px-4 py-2 bg-gray-900 text-white text-sm font-semibold rounded-lg hover:bg-gray-800 disabled:bg-gray-300 disabled:cursor-not-allowed"
          >
            {loading ? '归因中…' : '发起归因'}
          </button>
          {caseData && cached && (
            <button
              onClick={() => run(true)}
              disabled={!canRun}
              className="px-4 py-2 bg-white border border-gray-300 text-gray-800 text-sm font-semibold rounded-lg hover:bg-gray-50 disabled:text-gray-400 disabled:cursor-not-allowed"
              title="用当前提示词重新跑一次 LLM 流水线，覆盖缓存"
            >
              重新归因
            </button>
          )}
          {cached && !loading && (
            <span className="text-xs px-2 py-1 bg-emerald-50 text-emerald-700 rounded-full">
              已缓存 · 秒出
            </span>
          )}
          {error && <span className="text-sm text-red-600">{error}</span>}
        </div>

        {loading && (
          <div className="pt-3 space-y-2">
            <div className="flex items-center justify-between text-xs text-gray-600">
              <span>
                阶段 {stageIdx + 1}/{stages.length}：
                <span className="font-semibold text-gray-900">
                  {stages[Math.min(stageIdx, stages.length - 1)].name}
                </span>
              </span>
              <span className="font-mono text-gray-500">
                {elapsed}s / ~{totalEst}s
              </span>
            </div>
            <div className="w-full bg-gray-100 rounded-full h-2 overflow-hidden">
              <div
                className="bg-indigo-500 h-full transition-all duration-1000 ease-linear"
                style={{ width: `${Math.min(100, (elapsed / totalEst) * 100)}%` }}
              />
            </div>
            <div className="flex items-center gap-2 text-xs text-gray-500">
              {stages.map((s, i) => (
                <span
                  key={s.name}
                  className={`flex items-center gap-1 ${
                    i < stageIdx
                      ? 'text-emerald-700'
                      : i === stageIdx
                      ? 'text-indigo-700 font-semibold'
                      : 'text-gray-400'
                  }`}
                >
                  <span
                    className={`inline-block w-2 h-2 rounded-full ${
                      i < stageIdx
                        ? 'bg-emerald-500'
                        : i === stageIdx
                        ? 'bg-indigo-500 animate-pulse'
                        : 'bg-gray-300'
                    }`}
                  />
                  {s.name}
                </span>
              ))}
            </div>
          </div>
        )}

        {caseData && (
          <div className="mt-6 space-y-4">
            {/* ① Verdict */}
            <div
              className={`border rounded-lg p-4 ${
                caseData.outcome === 'fail'
                  ? 'bg-red-50 border-red-200 border-l-4 border-l-red-600'
                  : 'bg-green-50 border-green-200 border-l-4 border-l-green-600'
              }`}
            >
              <div className="text-xs font-semibold text-gray-600 mb-2">① 结论</div>
              <div className="text-sm text-gray-800 space-y-1">
                <div>
                  <span className="text-gray-500 font-medium">犯了什么错：</span>
                  <span className="font-semibold text-red-700">{caseData.verdict}</span>
                </div>
                <div>
                  <span className="text-gray-500 font-medium">核心原因：</span>
                  <span>{caseData.root_one}</span>
                </div>
                <div>
                  <span className="text-gray-500 font-medium">最终结果：</span>
                  <span className={caseData.outcome === 'fail' ? 'text-red-700 font-semibold' : 'text-green-700 font-semibold'}>
                    {caseData.outcome === 'fail' ? '❌ 问题结论直接交付' : '✅ 最终办成了'}
                  </span>
                  {caseData.outcome_note && (
                    <span className="ml-2 text-gray-600 text-xs">· {caseData.outcome_note}</span>
                  )}
                </div>
                {caseData.turn_issue && (
                  <div className="pt-1 border-t border-dashed border-gray-200">
                    <span className="text-gray-500 font-medium">性质：</span>
                    <span className="text-gray-700">{caseData.turn_issue}</span>
                  </div>
                )}
              </div>
            </div>

            {/* ② Graph */}
            <div>
              <div className="text-xs font-semibold text-gray-600 mb-2">② 因果链</div>
              <CausalGraph caseData={caseData} onScrollToStep={onScrollToStep} />
            </div>

            {/* ③ Attribution + fix */}
            {attrib && (
              <div className="border border-gray-200 rounded-lg p-4 bg-slate-50">
                <div className="text-xs font-semibold text-gray-600 mb-2">③ 归因对象与解决方案</div>

                {/* Primary attribution */}
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="text-sm text-gray-600 font-medium">首选归属</span>
                  <span
                    className="text-xs font-bold px-2.5 py-1 rounded-full"
                    style={{
                      background: attrib.bg,
                      color: attrib.fg,
                      outline: `2px solid ${attrib.fg}`,
                      outlineOffset: 2,
                    }}
                  >
                    {attrib.label}
                  </span>
                </div>

                {/* Alternative candidates */}
                {caseData.alternative_attribs && caseData.alternative_attribs.length > 0 && (
                  <div className="mt-3">
                    <div className="text-[11px] text-gray-500 font-semibold mb-1.5">
                      次优候选（点击可切换视角）
                    </div>
                    <div className="flex flex-wrap gap-2">
                      {caseData.alternative_attribs.map((alt, i) => {
                        const altStyle = ATTRIB_STYLE[alt.attrib] ?? {
                          bg: '#eef2f7',
                          fg: '#334155',
                          label: alt.attrib,
                        };
                        const isSelected = selectedAltIdx === i;
                        return (
                          <button
                            key={`${alt.attrib}-${i}`}
                            onClick={() => setSelectedAltIdx(isSelected ? null : i)}
                            className={`text-left text-xs rounded-lg border px-2.5 py-1.5 transition ${
                              isSelected
                                ? 'border-indigo-400 bg-indigo-50 shadow-sm'
                                : 'border-gray-200 bg-white hover:border-gray-300 hover:bg-gray-50'
                            }`}
                            style={{ minWidth: 160 }}
                          >
                            <div className="flex items-center justify-between gap-2">
                              <span
                                className="font-bold px-1.5 py-0.5 rounded"
                                style={{ background: altStyle.bg, color: altStyle.fg }}
                              >
                                {altStyle.label}
                              </span>
                              <span className="text-[10px] text-gray-400 font-mono">
                                {(alt.confidence * 100).toFixed(0)}%
                              </span>
                            </div>
                            <div className="text-[11px] text-gray-600 mt-1 leading-snug">
                              {alt.rationale}
                            </div>
                          </button>
                        );
                      })}
                    </div>
                  </div>
                )}

                {/* Fix block — reactive to the selected candidate */}
                <div className="mt-3 pt-3 border-t border-dashed border-gray-200">
                  <span className="text-sm text-gray-600 font-medium">
                    {selectedAltIdx !== null ? '该候选的解决方案：' : '解决方案：'}
                  </span>
                  <p className="text-sm text-gray-800 mt-1 leading-relaxed">
                    {selectedAltIdx !== null && caseData.alternative_attribs?.[selectedAltIdx]
                      ? caseData.alternative_attribs[selectedAltIdx].fix
                      : caseData.fix}
                  </p>
                </div>
              </div>
            )}

            {/* Contra panel */}
            {caseData.contra && (
              <div className="border border-gray-200 rounded-lg p-4 bg-white">
                <div className="text-xs font-semibold text-gray-600 mb-2">⚑ 关键矛盾</div>
                <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                  <div className="bg-green-50 border border-green-200 rounded p-3">
                    <div className="text-xs font-bold text-green-800 mb-1">已观测到的证据</div>
                    <p className="text-sm text-gray-800">{caseData.contra.saw}</p>
                  </div>
                  <div className="bg-red-50 border border-red-200 rounded p-3">
                    <div className="text-xs font-bold text-red-800 mb-1">实际给出的结论</div>
                    <p className="text-sm text-gray-800">{caseData.contra.said}</p>
                  </div>
                </div>
              </div>
            )}
          </div>
        )}

        {history.length > 0 && (
          <details className="mt-6 border-t border-gray-100 pt-4">
            <summary className="cursor-pointer text-xs font-semibold text-gray-600 flex items-center justify-between">
              <span>历史归因（本地缓存 · {history.length} 条）</span>
              <button
                onClick={(e) => {
                  e.preventDefault();
                  if (window.confirm('确定清空本 (session, round) 的所有历史归因记录？')) {
                    clearHistory();
                  }
                }}
                className="text-xs text-red-500 hover:text-red-700 font-normal ml-3"
              >
                清空历史
              </button>
            </summary>
            <ul className="mt-3 space-y-2">
              {history
                .slice()
                .reverse()
                .map((entry, idx) => {
                  const isActive =
                    caseData?.id === entry.caseData.id &&
                    complaint === entry.complaint;
                  const when = new Date(entry.ranAt);
                  const whenText = `${when.getMonth() + 1}/${when.getDate()} ${String(
                    when.getHours(),
                  ).padStart(2, '0')}:${String(when.getMinutes()).padStart(2, '0')}`;
                  return (
                    <li key={`${entry.ranAt}-${idx}`}>
                      <button
                        onClick={() => loadFromHistory(entry)}
                        className={`w-full text-left px-3 py-2 rounded-lg border transition ${
                          isActive
                            ? 'border-blue-400 bg-blue-50'
                            : 'border-gray-200 bg-white hover:bg-gray-50'
                        }`}
                      >
                        <div className="flex items-center justify-between gap-2">
                          <span className="text-sm font-medium text-gray-800 truncate">
                            {entry.caseData.title || '(无标题)'}
                          </span>
                          <span className="text-xs text-gray-400 flex-shrink-0">{whenText}</span>
                        </div>
                        <div className="text-xs text-gray-500 truncate mt-0.5">
                          {entry.complaint}
                        </div>
                        <div className="text-xs text-gray-400 mt-0.5">
                          {entry.caseData.outcome === 'fail' ? '❌' : '✅'}{' '}
                          {entry.caseData.attrib} · {entry.caseData.nodes.length} 节点
                        </div>
                      </button>
                    </li>
                  );
                })}
            </ul>
          </details>
        )}
      </div>
    </div>
  );
};
