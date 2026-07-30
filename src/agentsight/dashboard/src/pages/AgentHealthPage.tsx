import React, { useState, useEffect, useRef, useCallback } from 'react';
import {
  fetchAgentHealth,
  deleteAgentHealth,
  restartAgentHealth,
  fetchInterruptions,
  resolveInterruption,
  INTERRUPTION_TYPE_CN,
} from '../utils/apiClient';
import type { InterruptionRecord, InterruptionSeverity } from '../utils/apiClient';
import type { AgentHealthStatus } from '../types';

// ─── Agent status section ─────────────────────────────────────────────────────

/** Status dot color mapping */
const STATUS_COLORS: Record<string, string> = {
  healthy: 'bg-green-500',
  unhealthy: 'bg-red-500',
  hung: 'bg-orange-500',
  unknown: 'bg-yellow-400',
  no_port: 'bg-gray-400',
  offline: 'bg-gray-500',
};

/** Status display label */
const STATUS_LABELS: Record<string, string> = {
  healthy: '正常',
  unhealthy: '端口无响应',
  hung: '响应卡住',
  unknown: '待检测',
  no_port: '客户端进程',
  offline: '异常退出',
};

/** Status tooltip / 描述，帮助用户理解状态含义 */
const STATUS_TOOLTIPS: Record<string, string> = {
  healthy: '服务监听端口且 HTTP 探活成功',
  unhealthy: '端口不接受连接，可能需要重启',
  hung: '端口可连但 HTTP 探活超时，进程可能卡死',
  unknown: '首轮健康检查未完成',
  no_port: 'TUI / 子进程，本身不提供服务端口（正常）',
  offline: '进程异常退出，影响了进行中的 LLM 对话，5 分钟后自动移除',
};

/** Format relative time in Chinese */
function relativeTime(timestampMs: number): string {
  if (timestampMs === 0) return '—';
  const diff = Math.floor((Date.now() - timestampMs) / 1000);
  if (diff < 5) return '刚刚';
  if (diff < 60) return `${diff} 秒前`;
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  return `${Math.floor(diff / 3600)} 小时前`;
}

interface Toast {
  id: number;
  message: string;
}

const AgentCard: React.FC<{
  agent: AgentHealthStatus;
  related: AgentHealthStatus[];
  onDelete: (pid: number) => void;
  onRestart: (pid: number) => void;
  restarting: boolean;
}> = ({ agent, related, onDelete, onRestart, restarting }) => {
  const [showRelated, setShowRelated] = useState(false);

  // 区分：真 Gateway = 本身在监听端口的服务进程（如 OpenClaw Gateway）
  //       升格 Gateway = 被升格为主卡的单进程 agent（如 Hermes Python CLI）—
  //       这种不该贴“Gateway”标签，他们业务上没有 gateway 概念。
  const hasPorts = (agent.ports?.length ?? 0) > 0;
  const isRealGateway = agent.role === 'gateway' && hasPorts;
  const isPromotedGateway = agent.role === 'gateway' && !hasPorts;

  // 升格 Gateway + status=no_port 用“运行中”绿色，避免原 no_port 的
  // “客户端进程”灰色语义与主卡身份冲突。
  const useRunningStatus = isPromotedGateway && agent.status === 'no_port';
  const dotColor = useRunningStatus
    ? 'bg-green-500'
    : STATUS_COLORS[agent.status] || 'bg-gray-400';
  const label = useRunningStatus ? '运行中' : STATUS_LABELS[agent.status] || agent.status;
  const tooltip = useRunningStatus
    ? '单进程 agent，本身不提供服务端口，运行正常'
    : STATUS_TOOLTIPS[agent.status] || '';
  const isOffline = agent.status === 'offline';
  const isHung = agent.status === 'hung';
  const isUnhealthy = agent.status === 'unhealthy';
  const canRestart = isHung && !!agent.restart_cmd?.length;

  // 计算 offline 项距离自动移除还有多久（5 分钟 TTL）
  const OFFLINE_TTL_MS = 5 * 60 * 1000;
  const offlineRemainSec =
    isOffline && agent.offline_since
      ? Math.max(0, Math.ceil((OFFLINE_TTL_MS - (Date.now() - agent.offline_since)) / 1000))
      : null;

  const borderClass = isHung
    ? 'border-orange-300 bg-orange-50'
    : isUnhealthy
    ? 'border-red-300 bg-red-50'
    : 'border-gray-200 bg-white';
  const nameColor = isOffline
    ? 'text-gray-500'
    : isHung
    ? 'text-orange-700'
    : isUnhealthy
    ? 'text-red-700'
    : 'text-gray-900';
  const labelColor = isOffline
    ? 'text-gray-400'
    : isHung
    ? 'text-orange-500 font-semibold'
    : isUnhealthy
    ? 'text-red-500 font-semibold'
    : 'text-gray-400';

  return (
    <div className={`rounded-lg border shadow-sm p-4 ${borderClass}`} title={tooltip}>
      <div className="flex items-center gap-2">
        <span className={`inline-block w-2.5 h-2.5 rounded-full flex-shrink-0 ${dotColor}`} />
        <span className={`font-medium text-sm truncate ${nameColor}`}>{agent.agent_name}</span>
        {isRealGateway && (
          <span className="text-[10px] px-1 py-0.5 rounded bg-green-100 text-green-700 font-medium">
            Gateway
          </span>
        )}
        {agent.role === 'client' && (
          <span className="text-[10px] px-1 py-0.5 rounded bg-gray-100 text-gray-500 font-medium">
            客户端
          </span>
        )}
        {agent.role === 'worker' && (
          <span className="text-[10px] px-1 py-0.5 rounded bg-gray-100 text-gray-500 font-medium">
            Worker
          </span>
        )}
        <span className={`ml-auto text-xs flex-shrink-0 ${labelColor}`}>{label}</span>
      </div>
      {tooltip && (
        <div className="mt-1.5 text-[11px] leading-snug text-gray-500 italic">ℹ️ {tooltip}</div>
      )}
      <div className="mt-2 text-xs text-gray-500 space-y-0.5">
        <div className="flex items-center gap-3">
          <span>PID {agent.pid}</span>
          {agent.latency_ms !== null && agent.status === 'healthy' && (
            <span className="text-green-600">{agent.latency_ms}ms</span>
          )}
          <span className="text-gray-400">{relativeTime(agent.last_check_time)}</span>
        </div>
        {agent.error_message && !isOffline && (
          <div
            className={`truncate ${isHung ? 'text-orange-500' : 'text-red-500'}`}
            title={agent.error_message}
          >
            {agent.error_message}
          </div>
        )}
        {isOffline && offlineRemainSec !== null && (
          <div className="text-gray-400 italic">
            {offlineRemainSec > 0
              ? `${
                  offlineRemainSec >= 60
                    ? Math.ceil(offlineRemainSec / 60) + ' 分钟'
                    : offlineRemainSec + ' 秒'
                }后自动移除`
              : '即将移除'}
          </div>
        )}
      </div>
      {(isOffline || canRestart) && (
        <div className="mt-2 flex items-center gap-3">
          {isOffline && (
            <button
              onClick={() => onDelete(agent.pid)}
              className="text-xs text-gray-400 hover:text-gray-600 underline"
            >
              立即移除
            </button>
          )}
          {canRestart && (
            <button
              onClick={() => onRestart(agent.pid)}
              disabled={restarting}
              className="text-xs text-orange-500 hover:text-orange-700 underline disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {restarting ? '重启中...' : '重启进程'}
            </button>
          )}
        </div>
      )}
      {related.length > 0 && (
        <div className="mt-2 pt-2 border-t border-gray-100">
          <button
            onClick={() => setShowRelated(o => !o)}
            className="text-[11px] text-gray-500 hover:text-gray-700 flex items-center gap-1"
          >
            <span className={`transition-transform ${showRelated ? 'rotate-90' : ''}`}>▶</span>
            关联进程 ({related.length})
          </button>
          {showRelated && (
            <div className="mt-1 ml-2 border-l-2 border-gray-200 pl-2 space-y-1">
              {related.map(ca => (
                <div key={ca.pid} className="text-[11px] text-gray-500 flex items-center gap-1.5">
                  <span className="inline-block w-1.5 h-1.5 rounded-full bg-gray-300" />
                  <span className="text-[10px] px-1 py-0.5 rounded bg-gray-100">
                    {ca.role === 'worker' ? 'Worker' : '客户端'}
                  </span>
                  <span className="text-gray-400">PID {ca.pid}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
};

const AgentStatusSection: React.FC<{ addToast: (msg: string) => void }> = ({ addToast }) => {
  const [agents, setAgents] = useState<AgentHealthStatus[]>([]);
  const [clientAgents, setClientAgents] = useState<AgentHealthStatus[]>([]);
  const [showOrphans, setShowOrphans] = useState(false);
  const [lastScan, setLastScan] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [restartingPids, setRestartingPids] = useState<Set<number>>(new Set());
  const hasDataRef = useRef(false);

  const refresh = useCallback(async () => {
    try {
      // 一次拉全部（包含 client/worker），后续按 parent_pid 分组挂到各自主卡下面
      const data = await fetchAgentHealth({ includeClients: true });
      const agentRows = Array.isArray(data?.agents) ? data.agents : [];
      setAgents(agentRows.filter(a => a.role === 'gateway'));
      setClientAgents(agentRows.filter(a => a.role !== 'gateway'));
      setLastScan(data.last_scan_time ?? 0);
      setError(null);
      hasDataRef.current = true;
    } catch (e: any) {
      if (!hasDataRef.current) {
        setError(e.message || '请求失败');
      }
    } finally {
      setLoading(false);
    }
  }, []);

  const handleDelete = async (pid: number) => {
    try {
      await deleteAgentHealth(pid);
      setAgents(prev => prev.filter(a => a.pid !== pid));
    } catch (e: any) {
      addToast(`删除失败: ${e.message}`);
    }
  };

  const handleRestart = async (pid: number) => {
    setRestartingPids(prev => new Set(prev).add(pid));
    try {
      const result = await restartAgentHealth(pid);
      addToast(`✅ 重启成功，新 PID: ${result.new_pid}，等待进程上线...`);
      // 立即从本地列表删除旧条目，不等下次扫描（新 PID 会在 30s 内自动出现）
      setAgents(prev => prev.filter(a => a.pid !== pid));
    } catch (e: any) {
      addToast(`重启失败: ${e.message}`);
    } finally {
      setRestartingPids(prev => {
        const next = new Set(prev);
        next.delete(pid);
        return next;
      });
    }
  };

  useEffect(() => {
    void refresh();
    const timer = setInterval(refresh, 10_000);
    return () => clearInterval(timer);
  }, [refresh]);

  // 排序：hung/unhealthy 首位（真有问题），正常中间，offline 最后（不抢眼）
  const sorted = [...agents].sort((a, b) => {
    const order: Record<string, number> = {
      hung: 0,
      unhealthy: 1,
      healthy: 2,
      no_port: 3,
      unknown: 4,
      offline: 5,
    };
    return (order[a.status] ?? 6) - (order[b.status] ?? 6);
  });

  const healthyCount = agents.filter(a => a.status === 'healthy').length;
  const offlineCount = agents.filter(a => a.status === 'offline').length;
  const hungCount = agents.filter(a => a.status === 'hung').length;
  const totalCount = agents.length;

  const gatewayPids = new Set(sorted.map(a => a.pid));
  // 孤儿关联进程：Worker 但父进程不是任何主卡（不应出现，兜底）。
  // 过滤 status=offline 的进程——它们 5 分钟后会被 TTL 自动清理。
  const orphans = clientAgents.filter(
    c =>
      c.status !== 'offline' &&
      (c.parent_pid === undefined || c.parent_pid === null || !gatewayPids.has(c.parent_pid))
  );

  return (
    <section>
      <div className="flex items-center justify-between mb-3">
        <div className="flex items-center gap-3">
          <h2
            className="text-lg font-semibold text-gray-800 cursor-help"
            title="监控本机 AI Agent 进程健康状态。仅当进程异常退出并影响了进行中的 LLM 对话时，才会展示崩溃记录。正常退出的进程不会显示。"
          >
            Agent 看板
          </h2>
          {offlineCount > 0 && (
            <span className="text-xs px-1.5 py-0.5 rounded-full bg-red-100 text-red-600 font-semibold">
              {offlineCount} 崩溃
            </span>
          )}
          {hungCount > 0 && (
            <span className="text-xs px-1.5 py-0.5 rounded-full bg-orange-100 text-orange-600 font-semibold">
              {hungCount} 卡顿
            </span>
          )}
          {totalCount > 0 && (
            <span className="text-xs px-1.5 py-0.5 rounded-full bg-gray-100 text-gray-600">
              {healthyCount}/{totalCount} 正常
            </span>
          )}
        </div>
        {lastScan > 0 && (
          <span className="text-xs text-gray-400">上次扫描: {relativeTime(lastScan)}</span>
        )}
      </div>

      {loading ? (
        <div className="py-8 text-center text-sm text-gray-400">加载中...</div>
      ) : error ? (
        <div className="py-8 text-center text-sm text-red-400">{error}</div>
      ) : sorted.length === 0 ? (
        <div className="py-8 text-center text-sm text-gray-400 bg-white rounded-lg border border-gray-200">
          暂无已发现的 Agent
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
          {sorted.map(agent => (
            <AgentCard
              key={agent.pid}
              agent={agent}
              // 只把 parent_pid 与当前主卡 pid 严格匹配的进程挂为关联进程，
              // 避免同名独立实例（两个独立终端各开一个 hermes）被错误合并。
              related={clientAgents.filter(c => c.parent_pid === agent.pid)}
              onDelete={handleDelete}
              onRestart={handleRestart}
              restarting={restartingPids.has(agent.pid)}
            />
          ))}
        </div>
      )}

      {orphans.length > 0 && (
        <div className="mt-3">
          <button
            onClick={() => setShowOrphans(s => !s)}
            className="text-xs text-gray-500 hover:text-gray-700 flex items-center gap-1"
          >
            <span className={`transition-transform ${showOrphans ? 'rotate-90' : ''}`}>▶</span>
            孤儿关联进程 ({orphans.length})
          </button>
          {showOrphans && (
            <div className="mt-1 ml-2 border-l-2 border-gray-100 pl-2 space-y-1">
              {orphans.map(ca => (
                <div key={ca.pid} className="text-[11px] text-gray-500 flex items-center gap-1.5">
                  <span className="inline-block w-1.5 h-1.5 rounded-full bg-gray-300" />
                  <span className="font-medium">{ca.agent_name}</span>
                  <span className="text-[10px] px-1 py-0.5 rounded bg-gray-100">
                    {ca.role === 'worker' ? 'Worker' : '客户端'}
                  </span>
                  <span className="text-gray-400">PID {ca.pid}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </section>
  );
};

// ─── Interruption events section ──────────────────────────────────────────────

const SEVERITY_DOT: Record<InterruptionSeverity, string> = {
  critical: 'bg-red-600',
  high: 'bg-orange-500',
  medium: 'bg-yellow-400',
  low: 'bg-blue-400',
};

const SEVERITY_LABELS: Record<InterruptionSeverity, string> = {
  critical: '致命',
  high: '严重',
  medium: '中等',
  low: '轻微',
};

const TIME_RANGES: { label: string; hours: number }[] = [
  { label: '最近 1 小时', hours: 1 },
  { label: '最近 24 小时', hours: 24 },
  { label: '最近 7 天', hours: 24 * 7 },
];

function formatNs(ns: number): string {
  return new Date(ns / 1_000_000).toLocaleString();
}

function parseDetail(raw: string | null): React.ReactNode {
  if (!raw) return <span className="text-xs text-gray-400">无详情</span>;
  try {
    const obj = JSON.parse(raw);
    return (
      <pre className="text-xs bg-gray-100 text-gray-700 rounded p-2 whitespace-pre-wrap break-all border border-gray-200">
        {JSON.stringify(obj, null, 2)}
      </pre>
    );
  } catch {
    return <span className="text-xs text-gray-500">{raw}</span>;
  }
}

/** Truncated monospace ID with a small copy button. */
const IDWithCopy: React.FC<{ value: string | null; addToast: (msg: string) => void }> = ({
  value,
  addToast,
}) => {
  if (!value) return <span className="text-gray-400">—</span>;
  const short = value.length > 8 ? `${value.slice(0, 8)}…` : value;
  const copy = async (e: React.MouseEvent) => {
    e.stopPropagation();
    let ok = false;
    try {
      // 优先 clipboard API（HTTPS / localhost）
      if (window.isSecureContext && navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(value);
        ok = true;
      }
    } catch {
      // fall through
    }
    if (!ok) {
      // 降级：临时 textarea + execCommand，HTTP 环境也能用
      try {
        const ta = document.createElement('textarea');
        ta.value = value;
        ta.setAttribute('readonly', '');
        ta.style.position = 'fixed';
        ta.style.left = '-9999px';
        ta.style.opacity = '0';
        document.body.appendChild(ta);
        ta.select();
        ok = document.execCommand('copy');
        document.body.removeChild(ta);
      } catch {
        ok = false;
      }
    }
    addToast(ok ? `已复制: ${value}` : `复制失败: ${value}`);
  };
  return (
    <span className="inline-flex items-center gap-1 font-mono text-xs text-gray-600" title={value}>
      <span>{short}</span>
      <button
        onClick={copy}
        type="button"
        aria-label="复制"
        className="text-gray-400 hover:text-blue-600 p-0.5 rounded hover:bg-blue-50"
        title="复制完整 ID"
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          viewBox="0 0 16 16"
          fill="currentColor"
          className="w-3 h-3"
        >
          <path d="M4 4v-1a2 2 0 0 1 2-2h6a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2h-1v1a2 2 0 0 1-2 2h-5a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2z" />
        </svg>
      </button>
    </span>
  );
};

const InterruptionEventRow: React.FC<{
  event: InterruptionRecord;
  onResolved: (id: string) => void;
  addToast: (msg: string) => void;
}> = ({ event, onResolved, addToast }) => {
  const [expanded, setExpanded] = useState(false);
  const [resolving, setResolving] = useState(false);

  const dotStyle = SEVERITY_DOT[event.severity] ?? 'bg-gray-400';
  const typeLabel = INTERRUPTION_TYPE_CN[event.interruption_type] ?? event.interruption_type;

  const handleResolve = async () => {
    const confirmed = window.confirm(
      '标记为已处理后，此中断事件将不再计入未处理统计。\n\n确认标记为已处理吗？'
    );
    if (!confirmed) return;
    setResolving(true);
    try {
      await resolveInterruption(event.interruption_id);
      onResolved(event.interruption_id);
    } catch (e: any) {
      addToast(`标记失败: ${e.message ?? '请稍后重试'}`);
    } finally {
      setResolving(false);
    }
  };

  return (
    <>
      <tr className={`border-t border-gray-100 ${event.resolved ? 'opacity-60' : ''}`}>
        <td className="px-3 py-2 whitespace-nowrap text-xs text-gray-500">
          {formatNs(event.occurred_at_ns)}
        </td>
        <td className="px-3 py-2 text-xs text-gray-700 truncate max-w-[10rem]">
          {event.agent_name ?? '—'}
        </td>
        <td className="px-3 py-2 text-xs text-gray-800 whitespace-nowrap">{typeLabel}</td>
        <td className="px-3 py-2 whitespace-nowrap">
          <span className="inline-flex items-center gap-1.5 text-xs text-gray-700">
            <span className={`inline-block w-2 h-2 rounded-full ${dotStyle}`} />
            {SEVERITY_LABELS[event.severity] ?? event.severity}
          </span>
        </td>
        <td className="px-3 py-2">
          <IDWithCopy value={event.session_id} addToast={addToast} />
        </td>
        <td className="px-3 py-2">
          <IDWithCopy value={event.conversation_id} addToast={addToast} />
        </td>
        <td className="px-3 py-2 whitespace-nowrap text-xs">
          {event.resolved ? (
            <span className="text-green-600">已处理</span>
          ) : (
            <span className="text-red-500">未处理</span>
          )}
        </td>
        <td className="px-3 py-2 whitespace-nowrap text-right">
          <div className="flex items-center gap-1 justify-end">
            {!event.resolved && (
              <button
                onClick={handleResolve}
                disabled={resolving}
                title="标记为已处理，不再计入未处理统计"
                className="text-xs px-2 py-0.5 rounded bg-green-600 hover:bg-green-500 text-white disabled:opacity-50"
              >
                {resolving ? '…' : 'Resolve'}
              </button>
            )}
            <button
              onClick={() => setExpanded(x => !x)}
              className="text-xs px-2 py-0.5 rounded border border-gray-300 text-gray-600 hover:bg-gray-50"
            >
              {expanded ? '收起' : '详情'}
            </button>
          </div>
        </td>
      </tr>
      {expanded && (
        <tr className="border-t border-gray-50 bg-gray-50/50">
          <td colSpan={8} className="px-3 py-2">
            {event.call_id && (
              <div className="mb-1 text-xs text-gray-400">call: {event.call_id}</div>
            )}
            {parseDetail(event.detail)}
          </td>
        </tr>
      )}
    </>
  );
};

const PAGE_SIZES = [15, 30, 50];

const InterruptionSection: React.FC<{ addToast: (msg: string) => void }> = ({ addToast }) => {
  const [events, setEvents] = useState<InterruptionRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [hours, setHours] = useState(24);
  const [typeFilter, setTypeFilter] = useState('');
  const [severityFilter, setSeverityFilter] = useState('');
  const [unresolvedOnly, setUnresolvedOnly] = useState(true);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(15);
  const hasDataRef = useRef(false);

  const load = useCallback(async () => {
    try {
      const endNs = Date.now() * 1_000_000;
      const startNs = endNs - hours * 3600 * 1_000_000_000;
      const data = await fetchInterruptions({
        startNs,
        endNs,
        interruptionType: typeFilter || undefined,
        severity: severityFilter || undefined,
        resolved: unresolvedOnly ? false : undefined,
        limit: 200,
      });
      setEvents(data);
      setError(null);
      hasDataRef.current = true;
    } catch (e: any) {
      if (!hasDataRef.current) {
        setError(e.message ?? '加载中断事件失败');
      }
    } finally {
      setLoading(false);
    }
  }, [hours, typeFilter, severityFilter, unresolvedOnly]);

  useEffect(() => {
    setLoading(true);
    void load();
    const timer = setInterval(load, 30_000);
    return () => clearInterval(timer);
  }, [load]);

  // 过滤条件变化时重置到第 1 页，避免筛选后停留在空页
  useEffect(() => {
    setPage(1);
  }, [hours, typeFilter, severityFilter, unresolvedOnly]);

  const handleResolved = (id: string) => {
    if (unresolvedOnly) {
      setEvents(prev => prev.filter(e => e.interruption_id !== id));
    } else {
      setEvents(prev =>
        prev.map(e => (e.interruption_id === id ? { ...e, resolved: true } : e))
      );
    }
  };

  const unresolvedCount = events.filter(e => !e.resolved).length;

  const selectClass =
    'text-xs border border-gray-300 rounded px-2 py-1 bg-white text-gray-700 focus:outline-none focus:ring-1 focus:ring-blue-400';

  return (
    <section className="mt-8">
      <div className="flex items-center justify-between mb-3 flex-wrap gap-2">
        <div className="flex items-center gap-3">
          <h2 className="text-lg font-semibold text-gray-800">中断事件</h2>
          {!loading && (
            <span className="text-xs text-gray-400">{unresolvedCount} 条未处理</span>
          )}
        </div>
        <div className="flex items-center gap-2 flex-wrap">
          <select value={hours} onChange={e => setHours(Number(e.target.value))} className={selectClass}>
            {TIME_RANGES.map(r => (
              <option key={r.hours} value={r.hours}>
                {r.label}
              </option>
            ))}
          </select>
          <select value={typeFilter} onChange={e => setTypeFilter(e.target.value)} className={selectClass}>
            <option value="">全部类型</option>
            {Object.entries(INTERRUPTION_TYPE_CN).map(([k, v]) => (
              <option key={k} value={k}>
                {v}
              </option>
            ))}
          </select>
          <select
            value={severityFilter}
            onChange={e => setSeverityFilter(e.target.value)}
            className={selectClass}
          >
            <option value="">全部严重度</option>
            {(Object.keys(SEVERITY_LABELS) as InterruptionSeverity[]).map(s => (
              <option key={s} value={s}>
                {SEVERITY_LABELS[s]}
              </option>
            ))}
          </select>
          <label className="flex items-center gap-1 text-xs text-gray-600 cursor-pointer select-none">
            <input
              type="checkbox"
              checked={unresolvedOnly}
              onChange={e => setUnresolvedOnly(e.target.checked)}
            />
            仅未处理
          </label>
        </div>
      </div>

      <div className="bg-white rounded-lg border border-gray-200 overflow-x-auto">
        {loading ? (
          <div className="py-8 text-center text-sm text-gray-400 animate-pulse">加载中...</div>
        ) : error ? (
          <div className="py-8 text-center text-sm text-red-400">{error}</div>
        ) : events.length === 0 ? (
          <div className="py-8 text-center text-sm text-gray-400">当前筛选条件下暂无中断事件</div>
        ) : (
          <>
            <table className="w-full text-left">
              <thead>
                <tr className="text-xs text-gray-400">
                  <th className="px-3 py-2 font-medium">时间</th>
                  <th className="px-3 py-2 font-medium">Agent</th>
                  <th className="px-3 py-2 font-medium">类型</th>
                  <th className="px-3 py-2 font-medium">严重度</th>
                  <th className="px-3 py-2 font-medium">Session</th>
                  <th className="px-3 py-2 font-medium">Conversation</th>
                  <th className="px-3 py-2 font-medium">状态</th>
                  <th className="px-3 py-2 font-medium text-right">操作</th>
                </tr>
              </thead>
              <tbody>
                {(() => {
                  const totalPages = Math.max(1, Math.ceil(events.length / pageSize));
                  const cur = Math.min(page, totalPages);
                  return events.slice((cur - 1) * pageSize, cur * pageSize).map(e => (
                    <InterruptionEventRow
                      key={e.interruption_id}
                      event={e}
                      onResolved={handleResolved}
                      addToast={addToast}
                    />
                  ));
                })()}
              </tbody>
            </table>
            {(() => {
              const totalPages = Math.max(1, Math.ceil(events.length / pageSize));
              const clamped = Math.min(page, totalPages);
              return (
                <div className="flex items-center justify-between px-3 py-2 border-t border-gray-200 text-xs text-gray-600">
                  <div className="flex items-center gap-2">
                    <span>共 {events.length} 条</span>
                    <span className="text-gray-400">·</span>
                    <span>
                      第 {clamped}/{totalPages} 页
                    </span>
                    <span className="text-gray-400">·</span>
                    <label className="flex items-center gap-1">
                      每页
                      <select
                        value={pageSize}
                        onChange={e => {
                          setPageSize(Number(e.target.value));
                          setPage(1);
                        }}
                        className="text-xs border border-gray-300 rounded px-1 py-0.5 bg-white"
                      >
                        {PAGE_SIZES.map(s => (
                          <option key={s} value={s}>
                            {s}
                          </option>
                        ))}
                      </select>
                      条
                    </label>
                  </div>
                  <div className="flex items-center gap-1">
                    <button
                      onClick={() => setPage(1)}
                      disabled={clamped <= 1}
                      className="px-2 py-1 rounded border border-gray-300 disabled:opacity-40 disabled:cursor-not-allowed hover:bg-gray-50"
                    >
                      首页
                    </button>
                    <button
                      onClick={() => setPage(p => Math.max(1, p - 1))}
                      disabled={clamped <= 1}
                      className="px-2 py-1 rounded border border-gray-300 disabled:opacity-40 disabled:cursor-not-allowed hover:bg-gray-50"
                    >
                      上一页
                    </button>
                    <button
                      onClick={() => setPage(p => Math.min(totalPages, p + 1))}
                      disabled={clamped >= totalPages}
                      className="px-2 py-1 rounded border border-gray-300 disabled:opacity-40 disabled:cursor-not-allowed hover:bg-gray-50"
                    >
                      下一页
                    </button>
                    <button
                      onClick={() => setPage(totalPages)}
                      disabled={clamped >= totalPages}
                      className="px-2 py-1 rounded border border-gray-300 disabled:opacity-40 disabled:cursor-not-allowed hover:bg-gray-50"
                    >
                      末页
                    </button>
                  </div>
                </div>
              );
            })()}
          </>
        )}
      </div>
    </section>
  );
};

// ─── Page ─────────────────────────────────────────────────────────────────────

export const AgentHealthPage: React.FC = () => {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const toastIdRef = useRef(0);

  const addToast = useCallback((message: string) => {
    const id = ++toastIdRef.current;
    setToasts(prev => [...prev, { id, message }]);
    setTimeout(() => setToasts(prev => prev.filter(t => t.id !== id)), 5000);
  }, []);

  return (
    <div className="max-w-screen-xl mx-auto px-6 py-6">
      <div className="fixed top-4 right-4 z-50 flex flex-col gap-2 pointer-events-none">
        {toasts.map(t => (
          <div
            key={t.id}
            className="bg-gray-800 text-white text-xs px-4 py-2 rounded shadow-lg pointer-events-auto"
          >
            {t.message}
          </div>
        ))}
      </div>

      <AgentStatusSection addToast={addToast} />
      <InterruptionSection addToast={addToast} />
    </div>
  );
};

export default AgentHealthPage;
