import React, { useState, useRef, useEffect, useCallback } from 'react';
import { useSearchParams } from 'react-router-dom';
import {
  PieChart, Pie, Cell, ResponsiveContainer,
} from 'recharts';
import { fetchTokenSavings, fetchAgentNames } from '../utils/apiClient';
import type { SessionSavings, SavingsSummary, OptimizationItem, DiffLine } from '../utils/apiClient';

// ─── Helpers ──────────────────────────────────────────────────────────────────

function fmtTokens(n: number): string {
  return n.toLocaleString();
}

function shortId(id: string, len = 16): string {
  return id.length > len ? id.slice(0, len) + '…' : id;
}

/** 复制按钮组件，点击后短暂显示「已复制」反馈 */
const CopyButton: React.FC<{ text: string }> = ({ text }) => {
  const [copied, setCopied] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const handleCopy = (e: React.MouseEvent) => {
    e.stopPropagation();
    const done = () => {
      setCopied(true);
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => setCopied(false), 1500);
    };
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(text).then(done).catch(() => fallbackCopy(text, done));
    } else {
      fallbackCopy(text, done);
    }
  };
  return (
    <button
      onClick={handleCopy}
      className={`flex-shrink-0 px-1.5 py-0.5 rounded text-xs transition-colors ${
        copied
          ? 'bg-green-100 text-green-600'
          : 'bg-gray-100 hover:bg-gray-200 text-gray-500 hover:text-gray-700'
      }`}
      title="复制完整 ID"
    >
      {copied ? '✓ 已复制' : '复制'}
    </button>
  );
};

function fallbackCopy(text: string, done: () => void) {
  const el = document.createElement('textarea');
  el.value = text;
  el.style.position = 'fixed';
  el.style.opacity = '0';
  document.body.appendChild(el);
  el.focus();
  el.select();
  try { document.execCommand('copy'); } catch {}
  document.body.removeChild(el);
  done();
}

/** datetime-local input value from a timestamp (ms) — uses local timezone */
function toDatetimeLocal(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Parse a datetime-local value back to ms timestamp */
function fromDatetimeLocal(val: string): number {
  return new Date(val).getTime();
}

// ─── Types ────────────────────────────────────────────────────────────────────

type OptimizationCategory = 'tool_output' | 'mcp_response';

// ─── Category config ──────────────────────────────────────────────────────────

const CATEGORY_CONFIG: Record<OptimizationCategory, { label: string; color: string; bg: string }> = {
  tool_output: { label: '工具输出', color: 'text-orange-700', bg: 'bg-orange-100' },
  mcp_response: { label: 'MCP输出', color: 'text-violet-700', bg: 'bg-violet-100' },
};

// ─── Pie chart data ───────────────────────────────────────────────────────────

const PIE_COLORS = ['#3b82f6', '#10b981']; // 输入蓝, 输出绿
const SAVED_PIE_COLORS = ['#f59e0b', '#8b5cf6']; // 工具橙, MCP紫

// ─── Diff line component ──────────────────────────────────────────────────────

const DiffLineView: React.FC<{ line: DiffLine }> = ({ line }) => {
  if (line.type === 'context') {
    return <div className="h-2" />;
  }
  const isRemove = line.type === 'remove';
  return (
    <div
      className={`font-mono text-xs px-2 py-0.5 ${
        isRemove ? 'bg-red-50 text-red-700' : 'bg-green-50 text-green-700'
      }`}
    >
      <span className="inline-block w-4 text-center opacity-60">
        {isRemove ? '-' : '+'}
      </span>
      {line.content}
    </div>
  );
};

// ─── Optimization table row ───────────────────────────────────────────────────

const OptimizationTableRow: React.FC<{ item: OptimizationItem }> = ({ item }) => {
  const [expanded, setExpanded] = useState(false);
  const cfg = CATEGORY_CONFIG[item.category];

  return (
    <>
      <tr className="hover:bg-gray-50 transition-colors">
        <td className="px-4 py-3">
          <span className={`px-2 py-0.5 rounded text-xs font-medium ${cfg.bg} ${cfg.color}`}>
            {cfg.label}
          </span>
        </td>
        <td className="px-4 py-3 text-sm text-gray-600 text-right">
          {fmtTokens(item.before_tokens)}
        </td>
        <td className="px-4 py-3 text-sm text-gray-600 text-right">
          {fmtTokens(item.after_tokens)}
        </td>
        <td className="px-4 py-3 text-sm font-semibold text-green-600 text-right">
          {fmtTokens(item.saved_tokens)}
        </td>
        <td className="px-4 py-3 text-center">
          <button
            onClick={() => setExpanded(!expanded)}
            className="text-xs text-blue-600 hover:text-blue-800 transition-colors"
          >
            {expanded ? '收起' : '详情'}
          </button>
        </td>
      </tr>
      {expanded && (
        <tr className="bg-gray-50">
          <td colSpan={5} className="px-4 py-3">
            <div className="bg-white rounded-lg border border-gray-200 overflow-hidden">
              {item.diff_lines.map((dl, i) => (
                <DiffLineView key={i} line={dl} />
              ))}
            </div>
          </td>
        </tr>
      )}
    </>
  );
};

// ─── Session row with expand ──────────────────────────────────────────────────

const SessionRow: React.FC<{
  session: SessionSavings;
  initialExpanded?: boolean;
  rowRef?: React.Ref<HTMLTableRowElement>;
}> = ({ session, initialExpanded = false, rowRef }) => {
  const [expanded, setExpanded] = useState(initialExpanded);

  return (
    <>
      <tr
        ref={rowRef}
        className={`hover:bg-gray-50 transition-colors cursor-pointer ${
          expanded ? 'bg-blue-50' : ''
        }`}
        onClick={() => setExpanded(!expanded)}
      >
        <td className="px-4 lg:px-6 py-4">
          <div className="flex items-center gap-2">
            <span className="text-gray-400 text-xs flex-shrink-0">
              {expanded ? '▼' : '▶'}
            </span>
            <span
              className="font-mono text-sm text-gray-800 truncate"
              title={session.session_id}
            >
              {shortId(session.session_id, 20)}
            </span>
            <CopyButton text={session.session_id} />
          </div>
        </td>
        <td className="px-4 lg:px-6 py-4 text-sm text-gray-700">
          <span className="truncate block" title={session.agent_name}>
            {session.agent_name}
          </span>
        </td>
        <td className="px-4 lg:px-6 py-4 text-sm text-gray-900 text-right">
          {fmtTokens(session.total_input_tokens)}
        </td>
        <td className="px-4 lg:px-6 py-4 text-sm text-gray-900 text-right">
          {fmtTokens(session.total_output_tokens)}
        </td>
        <td className="px-4 lg:px-6 py-4 text-sm font-semibold text-green-600 text-right">
          {fmtTokens(session.saved_tokens)}
        </td>
        <td className="px-4 lg:px-6 py-4">
          <div className="flex items-center gap-2">
            <div className="flex-1 h-2 bg-gray-200 rounded-full overflow-hidden max-w-[80px]">
              <div
                className="h-full bg-green-500 rounded-full"
                style={{ width: `${Math.min(session.savings_rate, 100)}%` }}
              />
            </div>
            <span className="text-xs font-semibold text-green-600">
              {session.savings_rate.toFixed(1)}%
            </span>
          </div>
        </td>
      </tr>

      {/* Expanded detail */}
      {expanded && (
        <tr className="bg-blue-50">
          <td colSpan={6} className="px-4 lg:px-8 py-4">
            {/* Optimization items table */}
            <div className="bg-white rounded-lg border border-gray-200 overflow-hidden">
              <table className="w-full min-w-[600px]">
                <thead className="bg-gray-50 border-b border-gray-200">
                  <tr>
                    <th className="px-4 py-2 text-left text-xs font-semibold text-gray-600 uppercase tracking-wide w-[90px]">
                      分类
                    </th>
                    <th className="px-4 py-2 text-right text-xs font-semibold text-gray-600 uppercase tracking-wide w-[100px]">
                      优化前
                    </th>
                    <th className="px-4 py-2 text-right text-xs font-semibold text-gray-600 uppercase tracking-wide w-[100px]">
                      优化后
                    </th>
                    <th className="px-4 py-2 text-right text-xs font-semibold text-gray-600 uppercase tracking-wide w-[100px]">
                      节省
                    </th>
                    <th className="px-4 py-2 text-center text-xs font-semibold text-gray-600 uppercase tracking-wide w-[60px]">
                      详情
                    </th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-gray-100">
                  {session.optimization_items.map((item) => (
                    <OptimizationTableRow key={item.id} item={item} />
                  ))}
                </tbody>
              </table>
            </div>
          </td>
        </tr>
      )}
    </>
  );
};

// ─── Main page ────────────────────────────────────────────────────────────────

export const TokenSavingsPage: React.FC = () => {
  const [searchParams] = useSearchParams();
  const now = Date.now();

  // Read URL params for deep-link from homepage
  const targetSessionId = searchParams.get('session_id');
  const paramStart = Number(searchParams.get('start'));
  const paramEnd = Number(searchParams.get('end'));
  const paramAgent = searchParams.get('agent') ?? '';

  const [startMs, setStartMs] = useState(paramStart || (now - 24 * 3600 * 1000));
  const [endMs, setEndMs] = useState(paramEnd || now);
  const [hasQueried, setHasQueried] = useState(false);
  const [selectedAgent, setSelectedAgent] = useState(paramAgent);

  // Track which session to auto-expand (from URL deep-link)
  const [expandedSessionId] = useState<string | null>(targetSessionId);

  // API data state
  const [sessions, setSessions] = useState<SessionSavings[]>([]);
  const [summary, setSummary] = useState<SavingsSummary | null>(null);
  const [statsAvailable, setStatsAvailable] = useState(true);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [agentNames, setAgentNames] = useState<string[]>([]);

  // Ref for scrolling to the target session row
  const targetRowRef = useRef<HTMLTableRowElement>(null);

  // Load agent names on mount
  useEffect(() => {
    const startNs = (Date.now() - 7 * 24 * 3600 * 1000) * 1_000_000;
    const endNs = Date.now() * 1_000_000;
    fetchAgentNames(startNs, endNs).then(setAgentNames).catch(() => {});
  }, []);

  const handleQuery = useCallback(async () => {
    setLoading(true);
    setError(null);
    setHasQueried(true);
    try {
      const startNs = startMs * 1_000_000;
      const endNs = endMs * 1_000_000;
      const resp = await fetchTokenSavings(startNs, endNs, selectedAgent || undefined);
      setSessions(resp.sessions);
      setSummary(resp.summary);
      setStatsAvailable(resp.stats_available);
    } catch (e: any) {
      setError(e.message || 'Failed to fetch token savings');
    } finally {
      setLoading(false);
    }
  }, [startMs, endMs, selectedAgent]);

  // Auto-query on mount when navigated from homepage with URL params
  const hasAutoQueriedRef = useRef(false);
  useEffect(() => {
    if (targetSessionId && !hasAutoQueriedRef.current) {
      hasAutoQueriedRef.current = true;
      handleQuery();
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Auto-scroll to the target session row after data loads
  useEffect(() => {
    if (expandedSessionId && sessions.length > 0 && targetRowRef.current) {
      targetRowRef.current.scrollIntoView({ behavior: 'smooth', block: 'center' });
    }
  }, [expandedSessionId, sessions]);

  const totalInput = summary?.total_input_tokens ?? 0;
  const totalOutput = summary?.total_output_tokens ?? 0;
  const totalTokens = summary?.total_tokens ?? 0;
  const totalSaved = summary?.total_saved_tokens ?? 0;
  const totalToolSaved = summary?.total_tool_saved ?? 0;
  const totalMcpSaved = summary?.total_mcp_saved ?? 0;
  const savingsRate = summary?.savings_rate ?? 0;

  return (
    <main className="max-w-screen-xl mx-auto px-6 py-6 space-y-6">
      {/* ── Filter bar ── */}
      <div className="bg-white rounded-xl shadow-sm border border-gray-200 p-4 flex flex-wrap items-end gap-4">
        {/* Start time */}
        <div className="flex items-center gap-2">
          <label className="text-sm text-gray-600 whitespace-nowrap">开始时间</label>
          <input
            type="datetime-local"
            className="border border-gray-300 rounded-lg px-3 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-blue-400"
            value={toDatetimeLocal(startMs)}
            onChange={(e) => setStartMs(fromDatetimeLocal(e.target.value))}
          />
        </div>
        {/* End time */}
        <div className="flex items-center gap-2">
          <label className="text-sm text-gray-600 whitespace-nowrap">结束时间</label>
          <input
            type="datetime-local"
            className="border border-gray-300 rounded-lg px-3 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-blue-400"
            value={toDatetimeLocal(endMs)}
            onChange={(e) => setEndMs(fromDatetimeLocal(e.target.value))}
          />
        </div>

        {/* Quick presets */}
        <div className="flex gap-2 flex-wrap">
          {[
            { label: '最近 1h', ms: 3600 * 1000 },
            { label: '最近 6h', ms: 6 * 3600 * 1000 },
            { label: '最近 24h', ms: 24 * 3600 * 1000 },
            { label: '最近 7d', ms: 7 * 24 * 3600 * 1000 },
          ].map(({ label, ms }) => (
            <button
              key={label}
              onClick={() => {
                const n = Date.now();
                setEndMs(n);
                setStartMs(n - ms);
              }}
              className="px-3 py-1.5 text-xs bg-gray-100 hover:bg-gray-200 rounded-lg text-gray-600 transition-colors"
            >
              {label}
            </button>
          ))}
        </div>

        {/* Agent selector */}
        <div className="flex items-center gap-2">
          <label className="text-sm text-gray-600 whitespace-nowrap">Agent</label>
          <select
            className="border border-gray-300 rounded-lg px-3 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-blue-400 min-w-[160px]"
            value={selectedAgent}
            onChange={(e) => setSelectedAgent(e.target.value)}
          >
            <option value="">全部 Agent</option>
            {agentNames.map((n) => (
              <option key={n} value={n}>{n}</option>
            ))}
          </select>
        </div>

        {/* Query button */}
        <button
          onClick={handleQuery}
          disabled={loading}
          className="ml-auto px-5 py-2 bg-blue-600 text-white rounded-lg text-sm font-medium hover:bg-blue-700 transition-colors disabled:opacity-50"
        >
          {loading ? '查询中...' : '查询'}
        </button>
      </div>

      {/* ── Error message ── */}
      {error && (
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded-lg text-sm">
          {error}
        </div>
      )}

      {/* ── Stats unavailable notice ── */}
      {hasQueried && !statsAvailable && (
        <div className="bg-yellow-50 border border-yellow-200 text-yellow-700 px-4 py-3 rounded-lg text-sm">
          未发现优化记录
        </div>
      )}

      {/* ── Content shown only after first query ── */}
      {hasQueried && !loading ? (
      <>
      {/* ── Summary cards ── */}
      <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
        {/* Card 1: Total consumption */}
        <div className="bg-white rounded-xl shadow-sm border border-gray-200 p-5">
          <p className="text-sm text-gray-500">总 Token 消耗</p>
          <p className="text-3xl font-bold text-gray-900 mt-1">{fmtTokens(totalTokens)}</p>
          <div className="mt-3">
            <ResponsiveContainer width="100%" height={60}>
              <PieChart>
                <Pie
                  data={[
                    { name: '输入', value: totalInput },
                    { name: '输出', value: totalOutput },
                  ]}
                  cx="50%"
                  cy="50%"
                  innerRadius={14}
                  outerRadius={26}
                  paddingAngle={2}
                  dataKey="value"
                  stroke="none"
                >
                  {PIE_COLORS.map((c, i) => (
                    <Cell key={i} fill={c} />
                  ))}
                </Pie>
              </PieChart>
            </ResponsiveContainer>
            <div className="flex justify-center gap-4 -mt-1">
              <span className="flex items-center gap-1 text-xs text-gray-500">
                <span className="w-2 h-2 rounded-full bg-blue-500" />
                输入 {fmtTokens(totalInput)}
              </span>
              <span className="flex items-center gap-1 text-xs text-gray-500">
                <span className="w-2 h-2 rounded-full bg-green-500" />
                输出 {fmtTokens(totalOutput)}
              </span>
            </div>
          </div>
        </div>

        {/* Card 2: Saved tokens */}
        <div className="bg-white rounded-xl shadow-sm border border-gray-200 p-5">
          <p className="text-sm text-gray-500">已降低 Token</p>
          <p className="text-3xl font-bold text-green-600 mt-1">
            {fmtTokens(totalSaved)}
          </p>
          <div className="mt-3">
            <ResponsiveContainer width="100%" height={60}>
              <PieChart>
                <Pie
                  data={[
                    { name: '工具', value: totalToolSaved },
                    { name: 'MCP', value: totalMcpSaved },
                  ]}
                  cx="50%"
                  cy="50%"
                  innerRadius={14}
                  outerRadius={26}
                  paddingAngle={2}
                  dataKey="value"
                  stroke="none"
                >
                  {SAVED_PIE_COLORS.map((c, i) => (
                    <Cell key={i} fill={c} />
                  ))}
                </Pie>
              </PieChart>
            </ResponsiveContainer>
            <div className="flex justify-center gap-4 -mt-1">
              <span className="flex items-center gap-1 text-xs text-gray-500">
                <span className="w-2 h-2 rounded-full bg-orange-500" />
                工具 {fmtTokens(totalToolSaved)}
              </span>
              <span className="flex items-center gap-1 text-xs text-gray-500">
                <span className="w-2 h-2 rounded-full bg-violet-500" />
                MCP {fmtTokens(totalMcpSaved)}
              </span>
            </div>
          </div>
        </div>

        {/* Card 3: Savings rate */}
        <div className="bg-white rounded-xl shadow-sm border border-gray-200 p-5">
          <p className="text-sm text-gray-500">降低率</p>
          <div className="flex items-center gap-4 mt-1">
            <div className="relative w-20 h-20 flex-shrink-0">
              <svg viewBox="0 0 80 80" className="w-full h-full -rotate-90">
                <circle cx="40" cy="40" r="34" fill="none" stroke="#e5e7eb" strokeWidth="6" />
                <circle
                  cx="40"
                  cy="40"
                  r="34"
                  fill="none"
                  stroke={savingsRate >= 30 ? '#10b981' : savingsRate >= 15 ? '#3b82f6' : '#f59e0b'}
                  strokeWidth="6"
                  strokeDasharray={`${(savingsRate / 100) * 213.6} 213.6`}
                  strokeLinecap="round"
                />
              </svg>
              <div className="absolute inset-0 flex items-center justify-center">
                <span className="text-lg font-bold text-gray-900">
                  {savingsRate.toFixed(1)}%
                </span>
              </div>
            </div>
            <div>
              <span
                className={`px-2 py-0.5 rounded text-xs font-medium ${
                  savingsRate >= 30
                    ? 'bg-green-100 text-green-700'
                    : savingsRate >= 15
                    ? 'bg-blue-100 text-blue-700'
                    : 'bg-orange-100 text-orange-700'
                }`}
              >
                {savingsRate >= 30 ? '优秀' : savingsRate >= 15 ? '良好' : '待优化'}
              </span>
              <p className="text-xs text-gray-400 mt-1">
                基于总消耗计算
              </p>
            </div>
          </div>
        </div>
      </div>

      {/* ── Session table ── */}
      <div className="bg-white rounded-xl shadow-sm border border-gray-200 overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full min-w-[800px]">
            <thead className="bg-gray-50 border-b border-gray-200">
              <tr>
                <th className="px-4 lg:px-6 py-3 text-left text-xs font-semibold text-gray-600 uppercase tracking-wide">
                  Session ID
                </th>
                <th className="px-4 lg:px-6 py-3 text-left text-xs font-semibold text-gray-600 uppercase tracking-wide">
                  Agent
                </th>
                <th className="px-4 lg:px-6 py-3 text-right text-xs font-semibold text-gray-600 uppercase tracking-wide">
                  输入 Token
                </th>
                <th className="px-4 lg:px-6 py-3 text-right text-xs font-semibold text-gray-600 uppercase tracking-wide">
                  输出 Token
                </th>
                <th className="px-4 lg:px-6 py-3 text-right text-xs font-semibold text-gray-600 uppercase tracking-wide">
                  已降低
                </th>
                <th className="px-4 lg:px-6 py-3 text-left text-xs font-semibold text-gray-600 uppercase tracking-wide">
                  降低率
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-100">
              {sessions.map((sess) => (
                <SessionRow
                  key={sess.session_id}
                  session={sess}
                  initialExpanded={sess.session_id === expandedSessionId}
                  rowRef={sess.session_id === expandedSessionId ? targetRowRef : undefined}
                />
              ))}
            </tbody>
          </table>
        </div>
      </div>

      </>
      ) : (
        /* Prompt before first query */
        <div className="flex flex-col items-center justify-center py-20 text-gray-400">
          <div className="text-5xl mb-4">⚡</div>
          <p className="text-base">请选择时间范围，然后点击「查询」</p>
          <p className="text-xs mt-2">查看 Token 节省效果</p>
        </div>
      )}
    </main>
  );
};
