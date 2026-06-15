import React, { useState, useEffect, useRef, useCallback } from 'react';
import { fetchAgentHealth, deleteAgentHealth, restartAgentHealth } from '../utils/apiClient';
import type { AgentHealthStatus } from '../types';

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
  offline: '已退出',
};

/** Status tooltip / 描述，帮助用户理解状态含义 */
const STATUS_TOOLTIPS: Record<string, string> = {
  healthy: '服务监听端口且 HTTP 探活成功',
  unhealthy: '端口不接受连接，可能需要重启',
  hung: '端口可连但 HTTP 探活超时，进程可能卡死',
  unknown: '首轮健康检查未完成',
  no_port: 'TUI / 子进程，本身不提供服务端口（正常）',
  offline: '进程已退出，5 分钟后从列表自动移除',
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

/** Simple toast notification */
interface Toast {
  id: number;
  message: string;
}

const AgentCard: React.FC<{
  agent: AgentHealthStatus;
  onDelete: (pid: number) => void;
  onRestart: (pid: number) => void;
  restarting: boolean;
}> = ({ agent, onDelete, onRestart, restarting }) => {
  // 区分：真 Gateway = 本身在监听端口的服务进程（如 OpenClaw Gateway）
  //       升格 Gateway = 被升格为主卡的单进程 agent（如 Hermes Python CLI）—
  //       这种不该贴“Gateway”标签，他们业务上没有 gateway 概念。
  const hasPorts = (agent.ports?.length ?? 0) > 0;
  const isRealGateway = agent.role === 'gateway' && hasPorts;
  const isPromotedGateway = agent.role === 'gateway' && !hasPorts;

  // 状态显示：升格 Gateway + status=no_port 用“运行中”绿色，避免
  // 路用原 no_port 的“客户端进程”灰色语义与主卡身份冲突。
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

  // 背景色：只有 hung/unhealthy 才是需要告警的，offline 不再标红
  const bgClass = isHung ? 'bg-orange-50' : isUnhealthy ? 'bg-red-50' : '';
  // 名称色：offline 用灰色（类似“只读历史”），只有真问题才醒目
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
    <div
      className={`group px-3 py-2.5 border-b border-gray-100 last:border-b-0 ${bgClass}`}
      title={tooltip}
    >
      <div className="flex items-center gap-2">
        <span className={`inline-block w-2 h-2 rounded-full flex-shrink-0 ${dotColor}`} />
        <span className={`font-medium text-sm truncate ${nameColor}`}>
          {agent.agent_name}
        </span>
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
        <span className={`ml-auto text-xs flex-shrink-0 ${labelColor}`}>
          {label}
        </span>
      </div>
      {/* 鼠标悬停整张卡时展开状态说明（重点问题卡 hung/unhealthy 始终显示） */}
      {tooltip && (
        <div
          className={`mt-1 ml-4 text-[11px] leading-snug text-gray-500 italic ${
            isHung || isUnhealthy ? 'block' : 'hidden group-hover:block'
          }`}
        >
          ℹ️ {tooltip}
        </div>
      )}
      <div className="mt-1 ml-4 text-xs text-gray-500 space-y-0.5">
        <div>PID {agent.pid}</div>
        {agent.latency_ms !== null && agent.status === 'healthy' && (
          <span className="text-green-600">{agent.latency_ms}ms</span>
        )}
        {agent.error_message && !isOffline && (
          <div className={`truncate ${isHung ? 'text-orange-500' : 'text-red-500'}`} title={agent.error_message}>
            {agent.error_message}
          </div>
        )}
        <div className="text-gray-400">{relativeTime(agent.last_check_time)}</div>
        {isOffline && offlineRemainSec !== null && (
          <div className="text-gray-400 italic">
            {offlineRemainSec > 0
              ? `${offlineRemainSec >= 60 ? Math.ceil(offlineRemainSec / 60) + ' 分钟' : offlineRemainSec + ' 秒'}后自动移除`
              : '即将移除'}
          </div>
        )}
        {isOffline && (
          <button
            onClick={() => onDelete(agent.pid)}
            className="mt-1 text-xs text-gray-400 hover:text-gray-600 underline"
          >
            立即移除
          </button>
        )}
        {canRestart && (
          <button
            onClick={() => onRestart(agent.pid)}
            disabled={restarting}
            className="mt-1 text-xs text-orange-500 hover:text-orange-700 underline disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {restarting ? '重启中...' : '重启进程'}
          </button>
        )}
      </div>
    </div>
  );
};

/** 主卡下方按 agent_name 展示同名 client/worker 进程的折叠子列表。
 *  默认折起以免侧栏过长；点击可展开查看。 */
const RelatedProcesses: React.FC<{
  agentName: string;
  related: AgentHealthStatus[];
}> = ({ agentName: _agentName, related }) => {
  const [open, setOpen] = useState(false);
  return (
    <div className="px-3 py-1.5 border-b border-gray-100 bg-gray-50/50">
      <button
        onClick={() => setOpen(o => !o)}
        className="text-[11px] text-gray-500 hover:text-gray-700 flex items-center gap-1"
      >
        <span className={`transition-transform ${open ? 'rotate-90' : ''}`}>▶</span>
        关联进程 ({related.length})
      </button>
      {open && (
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
  );
};

export const AgentHealthSidebar: React.FC = () => {
  const [agents, setAgents] = useState<AgentHealthStatus[]>([]);
  const [clientAgents, setClientAgents] = useState<AgentHealthStatus[]>([]);
  const [showClients, setShowClients] = useState(false);
  const [lastScan, setLastScan] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);
  // 正在重启的 PID 集合，用于控制按钮 disabled 状态
  const [restartingPids, setRestartingPids] = useState<Set<number>>(new Set());
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const toastIdRef = useRef(0);
  // Track which PIDs we've already notified about going offline
  const notifiedOfflineRef = useRef<Set<number>>(new Set());

  const addToast = useCallback((message: string) => {
    const id = ++toastIdRef.current;
    setToasts(prev => [...prev, { id, message }]);
    setTimeout(() => setToasts(prev => prev.filter(t => t.id !== id)), 5000);
  }, []);

  const refresh = useCallback(async () => {
    try {
      // 一次拉全部（包含 client/worker），后续按 agent_name 分组挂到各自主卡下面
      const data = await fetchAgentHealth({ includeClients: true });

      // 检测新增离线和卡顿 agent
      data.agents.forEach(a => {
        if (a.status === 'offline' && !notifiedOfflineRef.current.has(a.pid)) {
          notifiedOfflineRef.current.add(a.pid);
          addToast(`\u26a0\ufe0f Agent "${a.agent_name}" (PID ${a.pid}) \u5df2\u9000\u51fa`);
        }
        if (a.status === 'hung' && !notifiedOfflineRef.current.has(-a.pid)) {
          notifiedOfflineRef.current.add(-a.pid); // 用负数区分 hung 通知
          addToast(`\u23f3 Agent "${a.agent_name}" (PID ${a.pid}) \u54cd\u5e94\u8d85\u65f6\uff0c\u53ef\u80fd\u5361\u987f`);
        }
      });
      // 清理不再存在的 PID
      const currentPids = new Set(data.agents.map(a => a.pid));
      notifiedOfflineRef.current.forEach(pid => {
        const absPid = Math.abs(pid);
        if (!currentPids.has(absPid)) notifiedOfflineRef.current.delete(pid);
      });
      // 如果 hung 进程恢复正常，清除其 hung 通知记录
      data.agents.forEach(a => {
        if (a.status !== 'hung') notifiedOfflineRef.current.delete(-a.pid);
      });

      // gateway = 主卡列表；others = client/worker，按 agent_name 挂到各主卡下
      setAgents(data.agents.filter(a => a.role === 'gateway'));
      setClientAgents(data.agents.filter(a => a.role !== 'gateway'));
      setLastScan(data.last_scan_time);
      setError(null);
    } catch (e: any) {
      if (agents.length === 0) {
        setError(e.message || '\u8bf7\u6c42\u5931\u8d25');
      }
    } finally {
      setLoading(false);
    }
  }, [addToast]);

  const handleDelete = async (pid: number) => {
    try {
      await deleteAgentHealth(pid);
      notifiedOfflineRef.current.delete(pid);
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
      // 清除 hung 通知记录
      notifiedOfflineRef.current.delete(-pid);
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
    refresh();
    timerRef.current = setInterval(refresh, 10_000);
    return () => {
      if (timerRef.current) clearInterval(timerRef.current);
    };
  }, [refresh]);

  // 排序：hung/unhealthy 首位（真有问题），正常中间，offline 最后（不抢眼）
  const sorted = [...agents].sort((a, b) => {
    const order: Record<string, number> = { hung: 0, unhealthy: 1, healthy: 2, no_port: 3, unknown: 4, offline: 5 };
    return (order[a.status] ?? 6) - (order[b.status] ?? 6);
  });

  const healthyCount = agents.filter(a => a.status === 'healthy').length;
  const offlineCount = agents.filter(a => a.status === 'offline').length;
  const hungCount = agents.filter(a => a.status === 'hung').length;
  const totalCount = agents.length;

  return (
    <>
      {/* Toast notifications */}
      <div className="fixed top-4 right-4 z-50 flex flex-col gap-2 pointer-events-none">
        {toasts.map(t => (
          <div key={t.id}
            className="bg-red-600 text-white text-xs px-4 py-2 rounded shadow-lg animate-pulse pointer-events-auto">
            {t.message}
          </div>
        ))}
      </div>

      <aside className="w-64 flex-shrink-0 border-l border-gray-200 bg-white overflow-y-auto"
             style={{ height: 'calc(100vh - 56px)' }}>
        {/* Header */}
        <div className="px-3 py-3 border-b border-gray-200 flex items-center justify-between">
          <span className="text-sm font-semibold text-gray-800">Agent 状态</span>
          <div className="flex items-center gap-1">
            {offlineCount > 0 && (
              <span className="text-xs px-1.5 py-0.5 rounded-full bg-red-100 text-red-600 font-semibold">
                {offlineCount} 下线
              </span>
            )}
            {hungCount > 0 && (
              <span className="text-xs px-1.5 py-0.5 rounded-full bg-orange-100 text-orange-600 font-semibold">
                {hungCount} 卡顿
              </span>
            )}
            {totalCount > 0 && (
              <span className="text-xs px-1.5 py-0.5 rounded-full bg-gray-100 text-gray-600">
                {healthyCount}/{totalCount}
              </span>
            )}
          </div>
        </div>

        {/* Content */}
        {loading ? (
          <div className="px-3 py-6 text-center text-xs text-gray-400">加载中...</div>
        ) : error ? (
          <div className="px-3 py-6 text-center text-xs text-red-400">{error}</div>
        ) : sorted.length === 0 ? (
          <div className="px-3 py-6 text-center text-xs text-gray-400">暂无已发现的 Agent</div>
        ) : (
          <div>
            {sorted.map(agent => {
              // 只把 parent_pid 与当前主卡 pid 严格匹配的 Worker 进程挂为关联进程，
              // 避免同名独立实例（两个独立终端各开一个 hermes）被错误合并。
              const related = clientAgents.filter(c => c.parent_pid === agent.pid);
              return (
                <React.Fragment key={agent.pid}>
                  <AgentCard
                    agent={agent}
                    onDelete={handleDelete}
                    onRestart={handleRestart}
                    restarting={restartingPids.has(agent.pid)}
                  />
                  {related.length > 0 && (
                    <RelatedProcesses agentName={agent.agent_name} related={related} />
                  )}
                </React.Fragment>
              );
            })}
            {/* 孤儿关联进程：Worker 但父进程不是任何主卡（不应出现，兑底）。
             *  过滤 status=offline 的进程——它们 5 分钟后会被 TTL 自动清理，
             *  不需要提前震出来干扰视线。*/}
            {(() => {
              const gatewayPids = new Set(sorted.map(a => a.pid));
              const orphans = clientAgents.filter(c =>
                c.status !== 'offline' &&
                (c.parent_pid === undefined || c.parent_pid === null || !gatewayPids.has(c.parent_pid))
              );
              if (orphans.length === 0) return null;
              return (
                <div className="px-3 py-2 border-t border-gray-100">
                  <button
                    onClick={() => setShowClients(s => !s)}
                    className="text-xs text-gray-500 hover:text-gray-700 flex items-center gap-1"
                  >
                    <span className={`transition-transform ${showClients ? 'rotate-90' : ''}`}>▶</span>
                    孤儿关联进程 ({orphans.length})
                  </button>
                  {showClients && (
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
              );
            })()}
          </div>
        )}

        {/* Footer */}
        {lastScan > 0 && (
          <div className="px-3 py-2 border-t border-gray-100 text-xs text-gray-400">
            上次扫描: {relativeTime(lastScan)}
          </div>
        )}
      </aside>
    </>
  );
};

