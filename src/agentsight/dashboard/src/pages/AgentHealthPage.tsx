import React, { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import {
  fetchAgentHealth,
  deleteAgentHealth,
  restartAgentHealth,
  fetchInterruptions,
  resolveInterruption,
  fetchLatencyMetrics,
} from '../utils/apiClient';
import type { InterruptionRecord, InterruptionSeverity, LatencyMetricsSummary, MetricPercentiles } from '../utils/apiClient';
import type { AgentHealthStatus } from '../types';
import { useI18n, useLocaleTag, INTERRUPTION_TYPES, interruptionTypeKey } from '../i18n';
import type { MessageKey } from '../i18n';
import { formatNs } from '../utils/datetime';

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

/** Status display label key */
const STATUS_LABEL_KEY: Record<string, MessageKey> = {
  healthy: 'ah.status.healthy',
  unhealthy: 'ah.status.unhealthy',
  hung: 'ah.status.hung',
  unknown: 'ah.status.unknown',
  no_port: 'ah.status.noPort',
  offline: 'ah.status.offline',
};

/** Status tooltip key */
const STATUS_TOOLTIP_KEY: Record<string, MessageKey> = {
  healthy: 'ah.tooltip.healthy',
  unhealthy: 'ah.tooltip.unhealthy',
  hung: 'ah.tooltip.hung',
  unknown: 'ah.tooltip.unknown',
  no_port: 'ah.tooltip.noPort',
  offline: 'ah.tooltip.offline',
};

type AgentStatus = AgentHealthStatus['status'];

const AGGREGATE_SEVERITY_RANK: Record<AgentStatus, number> = {
  unknown: 0,
  no_port: 1,
  healthy: 2,
  offline: 3,
  unhealthy: 4,
  hung: 5,
};

const CARD_STATUS_ORDER: Record<AgentStatus, number> = {
  hung: 0,
  unhealthy: 1,
  healthy: 2,
  no_port: 3,
  unknown: 4,
  offline: 5,
};

interface AgentGroup {
  agentName: string;
  agents: AgentHealthStatus[];
  status: AgentStatus;
}

function getAggregateStatus(group: AgentHealthStatus[]): AgentStatus {
  let worst: AgentStatus = 'unknown';

  for (const agent of group) {
    if (AGGREGATE_SEVERITY_RANK[agent.status] > AGGREGATE_SEVERITY_RANK[worst]) {
      worst = agent.status;
    }
  }

  return worst;
}

function groupAgentsByName(agents: AgentHealthStatus[]): AgentGroup[] {
  const groups = new Map<string, AgentHealthStatus[]>();

  for (const agent of agents) {
    const group = groups.get(agent.agent_name);
    if (group) {
      group.push(agent);
    } else {
      groups.set(agent.agent_name, [agent]);
    }
  }

  return Array.from(groups, ([agentName, group]) => ({
    agentName,
    agents: group,
    status: getAggregateStatus(group),
  }));
}

function isPromotedGatewayRunning(agent: AgentHealthStatus): boolean {
  return (
    agent.role === 'gateway' &&
    agent.status === 'no_port' &&
    (agent.ports?.length ?? 0) === 0
  );
}

/** Format relative time */
function relativeTime(timestampMs: number, t: (key: MessageKey, params?: Record<string, string | number>) => string): string {
  if (timestampMs === 0) return '—';
  const diff = Math.floor((Date.now() - timestampMs) / 1000);
  if (diff < 5) return t('common.justNow');
  if (diff < 60) return t('common.secondsAgo', { n: diff });
  if (diff < 3600) return t('common.minutesAgo', { n: Math.floor(diff / 60) });
  return t('common.hoursAgo', { n: Math.floor(diff / 3600) });
}

interface Toast {
  id: number;
  message: string;
}

function formatMetricValue(value: number): string {
  return value.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

function formatMetricP50(metric: MetricPercentiles | null, unit: string): string {
  return metric ? formatMetricValue(metric.p50) + ' ' + unit : '—';
}

function canonicalAgentKey(agentName: string): string {
  return agentName.toLowerCase();
}

const LatencyMetricsRow: React.FC<{ metrics: LatencyMetricsSummary }> = ({ metrics }) => {
  const { t } = useI18n();
  const items = [
    { label: t('latency.ttft'), metric: metrics.ttft_ms, unit: 'ms' },
    { label: t('latency.tps'), metric: metrics.tps_tokens_per_second, unit: 'tokens/s' },
    { label: t('latency.tpot'), metric: metrics.tpot_ms_per_token, unit: 'ms/token' },
    { label: t('latency.e2e'), metric: metrics.e2e_latency_ms, unit: 'ms' },
  ];

  if (!items.some(item => item.metric !== null)) return null;

  const tooltip = items
    .map(({ label, metric, unit }) => {
      if (!metric) return label + ' —';
      return (
        label +
        ' ' +
        t('latency.p50') +
        ' ' +
        formatMetricValue(metric.p50) +
        ' ' +
        unit +
        ' · ' +
        t('latency.p95') +
        ' ' +
        formatMetricValue(metric.p95) +
        ' ' +
        unit +
        ' · ' +
        t('latency.p99') +
        ' ' +
        formatMetricValue(metric.p99) +
        ' ' +
        unit
      );
    })
    .join(' · ');

  return (
    <div
      className="mt-2 pt-2 border-t border-gray-100 flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-gray-700"
      title={tooltip}
      aria-label={tooltip}
    >
      {items.map(({ label, metric, unit }) => (
        <span key={label}>
          <span className="text-gray-400">{label}</span> {formatMetricP50(metric, unit)}
        </span>
      ))}
    </div>
  );
};

const LATENCY_TIME_PRESETS = [
  { key: 'latency.range24h', ms: 24 * 3600 * 1000 },
  { key: 'latency.range7d', ms: 7 * 24 * 3600 * 1000 },
  { key: 'latency.range30d', ms: 30 * 24 * 3600 * 1000 },
] as const;

const AgentCard: React.FC<{
  agent: AgentHealthStatus;
  related: AgentHealthStatus[];
  onDelete: (pid: number) => void;
  onRestart: (pid: number) => void;
  restarting: boolean;
  latency?: LatencyMetricsSummary;
  showProcessDetails?: boolean;
}> = ({
  agent,
  related,
  onDelete,
  onRestart,
  restarting,
  latency,
  showProcessDetails = false,
}) => {
  const { t } = useI18n();
  const [showRelated, setShowRelated] = useState(false);

  // Real Gateway = a service process that listens on a port itself (e.g. OpenClaw Gateway).
  // Promoted Gateway = a single-process agent promoted to a main card (e.g. Hermes Python CLI) —
  // these should not carry a "Gateway" label since they have no gateway concept.
  const hasPorts = (agent.ports?.length ?? 0) > 0;
  const isRealGateway = agent.role === 'gateway' && hasPorts;

  // Promoted Gateway + status=no_port uses the green "Running" label to avoid
  // the gray "client process" semantics conflicting with main-card identity.
  const useRunningStatus = isPromotedGatewayRunning(agent);
  const dotColor = useRunningStatus
    ? 'bg-green-500'
    : STATUS_COLORS[agent.status] || 'bg-gray-400';
  const labelKey = STATUS_LABEL_KEY[agent.status];
  const label = useRunningStatus
    ? t('ah.running')
    : labelKey ? t(labelKey) : agent.status;
  const tooltipKey = STATUS_TOOLTIP_KEY[agent.status];
  const tooltip = useRunningStatus
    ? t('ah.tooltip.running')
    : tooltipKey ? t(tooltipKey) : '';
  const isOffline = agent.status === 'offline';
  const isHung = agent.status === 'hung';
  const isUnhealthy = agent.status === 'unhealthy';
  const canRestart = isHung && !!agent.restart_cmd?.length;

  // Time until offline entries are auto-removed (5-minute TTL)
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

  const remainText = offlineRemainSec !== null && offlineRemainSec > 0
    ? (offlineRemainSec >= 60
        ? `${Math.ceil(offlineRemainSec / 60)} ${t('common.min')}`
        : `${offlineRemainSec} ${t('common.sec')}`)
    : null;

  return (
    <div className={`rounded-lg border shadow-sm p-4 ${borderClass}`} title={tooltip}>
      <div className="flex items-center gap-2">
        <span className={`inline-block w-2.5 h-2.5 rounded-full flex-shrink-0 ${dotColor}`} />
        <span className={`font-medium text-sm truncate ${nameColor}`}>{agent.agent_name}</span>
        {isRealGateway && (
          <span className="text-[10px] px-1 py-0.5 rounded bg-green-100 text-green-700 font-medium">
            {t('ah.gateway')}
          </span>
        )}
        {agent.role === 'client' && (
          <span className="text-[10px] px-1 py-0.5 rounded bg-gray-100 text-gray-500 font-medium">
            {t('ah.client')}
          </span>
        )}
        {agent.role === 'worker' && (
          <span className="text-[10px] px-1 py-0.5 rounded bg-gray-100 text-gray-500 font-medium">
            {t('ah.worker')}
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
          {showProcessDetails && (
            <span className="text-gray-400">
              {agent.ports?.length ? agent.ports.map(port => `:${port}`).join(', ') : '—'}
            </span>
          )}
          {showProcessDetails ? (
            <span
              className={
                agent.latency_ms !== null && agent.status === 'healthy'
                  ? 'text-green-600'
                  : 'text-gray-400'
              }
            >
              {agent.latency_ms !== null ? `${agent.latency_ms}ms` : '—'}
            </span>
          ) : (
            agent.latency_ms !== null && agent.status === 'healthy' && (
              <span className="text-green-600">{agent.latency_ms}ms</span>
            )
          )}
          <span className="text-gray-400">{relativeTime(agent.last_check_time, t)}</span>
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
            {remainText
              ? t('ah.untilAutoRemoval', { time: remainText })
              : t('ah.removingSoon')}
          </div>
        )}
      </div>
      {latency && <LatencyMetricsRow metrics={latency} />}
      {(isOffline || canRestart) && (
        <div className="mt-2 flex items-center gap-3">
          {isOffline && (
            <button
              onClick={() => onDelete(agent.pid)}
              className="text-xs text-gray-400 hover:text-gray-600 underline"
            >
              {t('ah.removeNow')}
            </button>
          )}
          {canRestart && (
            <button
              onClick={() => onRestart(agent.pid)}
              disabled={restarting}
              className="text-xs text-orange-500 hover:text-orange-700 underline disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {restarting ? t('ah.restarting') : t('ah.restartProcess')}
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
            {t('ah.relatedProcesses', { n: related.length })}
          </button>
          {showRelated && (
            <div className="mt-1 ml-2 border-l-2 border-gray-200 pl-2 space-y-1">
              {related.map(ca => (
                <div key={ca.pid} className="text-[11px] text-gray-500 flex items-center gap-1.5">
                  <span className="inline-block w-1.5 h-1.5 rounded-full bg-gray-300" />
                  <span className="text-[10px] px-1 py-0.5 rounded bg-gray-100">
                    {ca.role === 'worker' ? t('ah.worker') : t('ah.client')}
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

const AgentGroupCard: React.FC<{
  group: AgentGroup;
  clientAgents: AgentHealthStatus[];
  onDelete: (pid: number) => void;
  onRestart: (pid: number) => void;
  restartingPids: Set<number>;
  latency?: LatencyMetricsSummary;
}> = ({ group, clientAgents, onDelete, onRestart, restartingPids, latency }) => {
  const { t } = useI18n();
  const isHung = group.status === 'hung';
  const isUnhealthy = group.status === 'unhealthy';
  const isOffline = group.status === 'offline';
  const hasAbnormalProcess = isHung || isUnhealthy;
  const [showProcesses, setShowProcesses] = useState(hasAbnormalProcess);

  useEffect(() => {
    if (hasAbnormalProcess) {
      setShowProcesses(true);
    }
  }, [hasAbnormalProcess]);

  const useRunningStatus = group.agents.every(isPromotedGatewayRunning);
  const dotColor = useRunningStatus
    ? 'bg-green-500'
    : STATUS_COLORS[group.status] || 'bg-gray-400';
  const labelKey = STATUS_LABEL_KEY[group.status];
  const label = useRunningStatus
    ? t('ah.running')
    : labelKey ? t(labelKey) : group.status;
  const tooltipKey = STATUS_TOOLTIP_KEY[group.status];
  const tooltip = useRunningStatus
    ? t('ah.tooltip.running')
    : tooltipKey ? t(tooltipKey) : '';
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
        <span
          className={`inline-block w-2.5 h-2.5 rounded-full flex-shrink-0 ${dotColor}`}
        />
        <span className={`font-medium text-sm truncate ${nameColor}`}>{group.agentName}</span>
        <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-gray-100 text-gray-600 font-semibold">
          ×{group.agents.length}
        </span>
        <span className={`ml-auto text-xs flex-shrink-0 ${labelColor}`}>{label}</span>
      </div>
      {tooltip && (
        <div className="mt-1.5 text-[11px] leading-snug text-gray-500 italic">ℹ️ {tooltip}</div>
      )}
      {latency && <LatencyMetricsRow metrics={latency} />}
      <div className="mt-2 pt-2 border-t border-gray-100">
        <button
          onClick={() => setShowProcesses(open => !open)}
          className="text-[11px] text-gray-500 hover:text-gray-700 flex items-center gap-1"
        >
          <span className={`transition-transform ${showProcesses ? 'rotate-90' : ''}`}>▶</span>
          {t('ah.processList', { n: group.agents.length })}
        </button>
        {showProcesses && (
          <div className="mt-2 space-y-2">
            {group.agents.map(agent => (
              <AgentCard
                key={agent.pid}
                agent={agent}
                related={clientAgents.filter(client => client.parent_pid === agent.pid)}
                onDelete={onDelete}
                onRestart={onRestart}
                restarting={restartingPids.has(agent.pid)}
                showProcessDetails
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
};

const AgentStatusSection: React.FC<{ addToast: (msg: string) => void }> = ({ addToast }) => {
  const { t } = useI18n();
  const [agents, setAgents] = useState<AgentHealthStatus[]>([]);
  const [clientAgents, setClientAgents] = useState<AgentHealthStatus[]>([]);
  const [showOrphans, setShowOrphans] = useState(false);
  const [lastScan, setLastScan] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [restartingPids, setRestartingPids] = useState<Set<number>>(new Set());
  const hasDataRef = useRef(false);
  const [rangeMs, setRangeMs] = useState(7 * 24 * 3600 * 1000);
  const [latencyMetrics, setLatencyMetrics] = useState<LatencyMetricsSummary[]>([]);
  const [latencyLoading, setLatencyLoading] = useState(true);
  const [latencyError, setLatencyError] = useState<string | null>(null);
  const latencyRequestIdRef = useRef(0);

  const loadLatency = useCallback(async () => {
    const requestId = ++latencyRequestIdRef.current;
    setLatencyLoading(true);
    setLatencyError(null);
    try {
      const endNs = Date.now() * 1_000_000;
      const startNs = endNs - rangeMs * 1_000_000;
      const data = await fetchLatencyMetrics(startNs, endNs);
      if (requestId === latencyRequestIdRef.current) {
        setLatencyMetrics(data);
        setLatencyError(null);
      }
    } catch (e: any) {
      if (requestId === latencyRequestIdRef.current) {
        setLatencyError(e.message || '');
      }
    } finally {
      if (requestId === latencyRequestIdRef.current) {
        setLatencyLoading(false);
      }
    }
  }, [rangeMs]);

  const refresh = useCallback(async () => {
    try {
      // Fetch all at once (including client/worker), then group by parent_pid under each main card
      const data = await fetchAgentHealth({ includeClients: true });
      const agentRows = Array.isArray(data?.agents) ? data.agents : [];
      setAgents(agentRows.filter(a => a.role === 'gateway'));
      setClientAgents(agentRows.filter(a => a.role !== 'gateway'));
      setLastScan(data.last_scan_time ?? 0);
      setError(null);
      hasDataRef.current = true;
    } catch (e: any) {
      if (!hasDataRef.current) {
        setError(e.message || t('ah.requestFailed'));
      }
    } finally {
      setLoading(false);
    }
  }, [t]);

  const handleDelete = async (pid: number) => {
    try {
      await deleteAgentHealth(pid);
      setAgents(prev => prev.filter(a => a.pid !== pid));
    } catch (e: any) {
      addToast(t('ah.deleteFailed', { msg: e.message }));
    }
  };

  const handleRestart = async (pid: number) => {
    setRestartingPids(prev => new Set(prev).add(pid));
    try {
      const result = await restartAgentHealth(pid);
      addToast(t('ah.restartSucceeded', { pid: result.new_pid }));
      // Remove the old entry from the local list immediately (the new PID appears within 30s)
      setAgents(prev => prev.filter(a => a.pid !== pid));
    } catch (e: any) {
      addToast(t('ah.restartFailed', { msg: e.message }));
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

  useEffect(() => {
    void loadLatency();
  }, [loadLatency]);

  const groupedAgents = useMemo(
    () =>
      groupAgentsByName(agents).sort(
        (a, b) => CARD_STATUS_ORDER[a.status] - CARD_STATUS_ORDER[b.status],
    ),
    [agents],
  );

  const healthyCount = agents.filter(a => a.status === 'healthy').length;
  const offlineCount = agents.filter(a => a.status === 'offline').length;
  const hungCount = agents.filter(a => a.status === 'hung').length;
  const totalCount = agents.length;

  const latencyByAgent = new Map<string, LatencyMetricsSummary[]>();
  for (const metric of latencyMetrics) {
    if (metric.agent_name !== null) {
      const key = canonicalAgentKey(metric.agent_name);
      const summaries = latencyByAgent.get(key);
      if (summaries) {
        summaries.push(metric);
      } else {
        latencyByAgent.set(key, [metric]);
      }
    }
  }
  const latencyForAgent = (agentName: string): LatencyMetricsSummary | undefined => {
    const summaries = latencyByAgent.get(canonicalAgentKey(agentName));
    // Do not silently choose one when casing variants produce separate summaries.
    return summaries?.length === 1 ? summaries[0] : undefined;
  };

  const gatewayPids = new Set(agents.map(a => a.pid));
  // Orphaned related processes: workers whose parent is not any main card (should not happen, fallback).
  // Filter out status=offline processes — they are TTL-cleaned after 5 minutes.
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
            title={t('ah.agentDashboardTooltip')}
          >
            {t('ah.agentDashboard')}
          </h2>
          {offlineCount > 0 && (
            <span className="text-xs px-1.5 py-0.5 rounded-full bg-red-100 text-red-600 font-semibold">
              {t('ah.crashed', { n: offlineCount })}
            </span>
          )}
          {hungCount > 0 && (
            <span className="text-xs px-1.5 py-0.5 rounded-full bg-orange-100 text-orange-600 font-semibold">
              {t('ah.hungCount', { n: hungCount })}
            </span>
          )}
          {totalCount > 0 && (
            <span className="text-xs px-1.5 py-0.5 rounded-full bg-gray-100 text-gray-600">
              {t('ah.healthyRatio', { healthy: healthyCount, total: totalCount })}
            </span>
          )}
        </div>
        <div className="flex items-center gap-2 flex-wrap justify-end">
          <div className="flex items-center gap-1.5">
            <span className="text-[11px] text-gray-400">{t('latency.title')}</span>
            {LATENCY_TIME_PRESETS.map(({ key, ms }) => (
              <button
                key={key}
                type="button"
                onClick={() => setRangeMs(ms)}
                aria-pressed={rangeMs === ms}
                className={rangeMs === ms
                  ? 'px-2 py-1 text-[11px] rounded bg-blue-100 text-blue-700 font-medium'
                  : 'px-2 py-1 text-[11px] rounded bg-gray-100 hover:bg-gray-200 text-gray-600'}
              >
                {t(key)}
              </button>
            ))}
          </div>
          {latencyLoading && (
            <span className="text-[11px] text-gray-400">{t('latency.loading')}</span>
          )}
          {latencyError !== null && (
            <span className="text-[11px] text-red-400" title={latencyError || t('latency.error')}>
              {t('latency.error')}
            </span>
          )}
          {lastScan > 0 && (
            <span className="text-xs text-gray-400">{t('ah.lastScan', { time: relativeTime(lastScan, t) })}</span>
          )}
        </div>
      </div>

      {loading ? (
        <div className="py-8 text-center text-sm text-gray-400">{t('common.loading')}</div>
      ) : error ? (
        <div className="py-8 text-center text-sm text-red-400">{error}</div>
      ) : groupedAgents.length === 0 ? (
        <div className="py-8 text-center text-sm text-gray-400 bg-white rounded-lg border border-gray-200">
          {t('ah.noAgents')}
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
          {groupedAgents.map(group =>
            group.agents.length === 1 ? (
              <AgentCard
                key={group.agents[0].pid}
                agent={group.agents[0]}
                // Only attach processes whose parent_pid strictly matches the current main card pid,
                // avoiding merging same-named independent instances.
                related={clientAgents.filter(c => c.parent_pid === group.agents[0].pid)}
                onDelete={handleDelete}
                onRestart={handleRestart}
                restarting={restartingPids.has(group.agents[0].pid)}
                latency={latencyForAgent(group.agentName)}
              />
            ) : (
              <AgentGroupCard
                key={group.agentName}
                group={group}
                clientAgents={clientAgents}
                onDelete={handleDelete}
                onRestart={handleRestart}
                restartingPids={restartingPids}
                latency={latencyForAgent(group.agentName)}
              />
            ),
          )}
        </div>
      )}

      {orphans.length > 0 && (
        <div className="mt-3">
          <button
            onClick={() => setShowOrphans(s => !s)}
            className="text-xs text-gray-500 hover:text-gray-700 flex items-center gap-1"
          >
            <span className={`transition-transform ${showOrphans ? 'rotate-90' : ''}`}>▶</span>
            {t('ah.orphanedProcesses', { n: orphans.length })}
          </button>
          {showOrphans && (
            <div className="mt-1 ml-2 border-l-2 border-gray-100 pl-2 space-y-1">
              {orphans.map(ca => (
                <div key={ca.pid} className="text-[11px] text-gray-500 flex items-center gap-1.5">
                  <span className="inline-block w-1.5 h-1.5 rounded-full bg-gray-300" />
                  <span className="font-medium">{ca.agent_name}</span>
                  <span className="text-[10px] px-1 py-0.5 rounded bg-gray-100">
                    {ca.role === 'worker' ? t('ah.worker') : t('ah.client')}
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

const SEVERITY_LABEL_KEY: Record<InterruptionSeverity, MessageKey> = {
  critical: 'common.critical',
  high: 'common.high',
  medium: 'common.medium',
  low: 'common.low',
};

const TIME_RANGE_KEYS: { labelKey: MessageKey; hours: number }[] = [
  { labelKey: 'ah.last1Hour', hours: 1 },
  { labelKey: 'ah.last24Hours', hours: 24 },
  { labelKey: 'ah.last7Days', hours: 24 * 7 },
];

function parseDetail(raw: string | null, t: (key: MessageKey) => string): React.ReactNode {
  if (!raw) return <span className="text-xs text-gray-400">{t('ah.noDetails')}</span>;
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
  const { t } = useI18n();
  if (!value) return <span className="text-gray-400">—</span>;
  const short = value.length > 8 ? `${value.slice(0, 8)}…` : value;
  const copy = async (e: React.MouseEvent) => {
    e.stopPropagation();
    let ok = false;
    try {
      // Prefer the clipboard API (HTTPS / localhost)
      if (window.isSecureContext && navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(value);
        ok = true;
      }
    } catch {
      // fall through
    }
    if (!ok) {
      // Fallback: temporary textarea + execCommand, works over HTTP too
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
    addToast(ok ? t('ah.copiedValue', { value }) : t('ah.copyFailedValue', { value }));
  };
  return (
    <span className="inline-flex items-center gap-1 font-mono text-xs text-gray-600" title={value}>
      <span>{short}</span>
      <button
        onClick={copy}
        type="button"
        aria-label={t('common.copy')}
        className="text-gray-400 hover:text-blue-600 p-0.5 rounded hover:bg-blue-50"
        title={t('common.copyFullId')}
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
  const { t } = useI18n();
  const locale = useLocaleTag();
  const [expanded, setExpanded] = useState(false);
  const [resolving, setResolving] = useState(false);

  const dotStyle = SEVERITY_DOT[event.severity] ?? 'bg-gray-400';
  const itypeKey = interruptionTypeKey(event.interruption_type);
  const typeLabel = itypeKey ? t(itypeKey) : event.interruption_type;
  const sevKey = SEVERITY_LABEL_KEY[event.severity];

  const handleResolve = async () => {
    const confirmed = window.confirm(t('ah.markResolvedConfirm'));
    if (!confirmed) return;
    setResolving(true);
    try {
      await resolveInterruption(event.interruption_id);
      onResolved(event.interruption_id);
    } catch (e: any) {
      addToast(t('ah.markFailed', { msg: e.message ?? '' }));
    } finally {
      setResolving(false);
    }
  };

  return (
    <>
      <tr className={`border-t border-gray-100 ${event.resolved ? 'opacity-60' : ''}`}>
        <td className="px-3 py-2 whitespace-nowrap text-xs text-gray-500">
          {formatNs(event.occurred_at_ns, locale)}
        </td>
        <td className="px-3 py-2 text-xs text-gray-700 truncate max-w-[10rem]">
          {event.agent_name ?? '—'}
        </td>
        <td className="px-3 py-2 text-xs text-gray-800 whitespace-nowrap">{typeLabel}</td>
        <td className="px-3 py-2 whitespace-nowrap">
          <span className="inline-flex items-center gap-1.5 text-xs text-gray-700">
            <span className={`inline-block w-2 h-2 rounded-full ${dotStyle}`} />
            {sevKey ? t(sevKey) : event.severity}
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
            <span className="text-green-600">{t('common.resolved')}</span>
          ) : (
            <span className="text-red-500">{t('common.unresolved')}</span>
          )}
        </td>
        <td className="px-3 py-2 whitespace-nowrap text-right">
          <div className="flex items-center gap-1 justify-end">
            {!event.resolved && (
              <button
                onClick={handleResolve}
                disabled={resolving}
                title={t('ah.markResolvedTitle')}
                className="text-xs px-2 py-0.5 rounded bg-green-600 hover:bg-green-500 text-white disabled:opacity-50"
              >
                {resolving ? '…' : t('common.resolve')}
              </button>
            )}
            <button
              onClick={() => setExpanded(x => !x)}
              className="text-xs px-2 py-0.5 rounded border border-gray-300 text-gray-600 hover:bg-gray-50"
            >
              {expanded ? t('common.collapse') : t('common.details')}
            </button>
          </div>
        </td>
      </tr>
      {expanded && (
        <tr className="border-t border-gray-50 bg-gray-50/50">
          <td colSpan={8} className="px-3 py-2">
            {event.call_id && (
              <div className="mb-1 text-xs text-gray-400">{t('ah.callLabel', { id: event.call_id })}</div>
            )}
            {parseDetail(event.detail, t)}
          </td>
        </tr>
      )}
    </>
  );
};

const PAGE_SIZES = [15, 30, 50];

const InterruptionSection: React.FC<{ addToast: (msg: string) => void }> = ({ addToast }) => {
  const { t } = useI18n();
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
        setError(e.message ?? t('ah.failedToLoad'));
      }
    } finally {
      setLoading(false);
    }
  }, [hours, typeFilter, severityFilter, unresolvedOnly, t]);

  useEffect(() => {
    setLoading(true);
    void load();
    const timer = setInterval(load, 30_000);
    return () => clearInterval(timer);
  }, [load]);

  // Reset to page 1 when filters change to avoid staying on an empty page
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
          <h2 className="text-lg font-semibold text-gray-800">{t('ah.interruptionEvents')}</h2>
          {!loading && (
            <span className="text-xs text-gray-400">{t('ah.unresolvedCount', { n: unresolvedCount })}</span>
          )}
        </div>
        <div className="flex items-center gap-2 flex-wrap">
          <select value={hours} onChange={e => setHours(Number(e.target.value))} className={selectClass}>
            {TIME_RANGE_KEYS.map(r => (
              <option key={r.hours} value={r.hours}>
                {t(r.labelKey)}
              </option>
            ))}
          </select>
          <select value={typeFilter} onChange={e => setTypeFilter(e.target.value)} className={selectClass}>
            <option value="">{t('common.allTypes')}</option>
            {INTERRUPTION_TYPES.map(k => {
              const key = interruptionTypeKey(k);
              return (
                <option key={k} value={k}>
                  {key ? t(key) : k}
                </option>
              );
            })}
          </select>
          <select
            value={severityFilter}
            onChange={e => setSeverityFilter(e.target.value)}
            className={selectClass}
          >
            <option value="">{t('common.allSeverities')}</option>
            {(Object.keys(SEVERITY_LABEL_KEY) as InterruptionSeverity[]).map(s => (
              <option key={s} value={s}>
                {t(SEVERITY_LABEL_KEY[s])}
              </option>
            ))}
          </select>
          <label className="flex items-center gap-1 text-xs text-gray-600 cursor-pointer select-none">
            <input
              type="checkbox"
              checked={unresolvedOnly}
              onChange={e => setUnresolvedOnly(e.target.checked)}
            />
            {t('common.unresolvedOnly')}
          </label>
        </div>
      </div>

      <div className="bg-white rounded-lg border border-gray-200 overflow-x-auto">
        {loading ? (
          <div className="py-8 text-center text-sm text-gray-400 animate-pulse">{t('common.loading')}</div>
        ) : error ? (
          <div className="py-8 text-center text-sm text-red-400">{error}</div>
        ) : events.length === 0 ? (
          <div className="py-8 text-center text-sm text-gray-400">{t('ah.noInterruptionEvents')}</div>
        ) : (
          <>
            <table className="w-full text-left">
              <thead>
                <tr className="text-xs text-gray-400">
                  <th className="px-3 py-2 font-medium">{t('common.time')}</th>
                  <th className="px-3 py-2 font-medium">{t('common.agent')}</th>
                  <th className="px-3 py-2 font-medium">{t('common.type')}</th>
                  <th className="px-3 py-2 font-medium">{t('common.severity')}</th>
                  <th className="px-3 py-2 font-medium">{t('common.session')}</th>
                  <th className="px-3 py-2 font-medium">{t('common.conversation')}</th>
                  <th className="px-3 py-2 font-medium">{t('common.status')}</th>
                  <th className="px-3 py-2 font-medium text-right">{t('common.actions')}</th>
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
                    <span>{t('common.events', { n: events.length })}</span>
                    <span className="text-gray-400">·</span>
                    <span>
                      {t('common.page', { cur: clamped, total: totalPages })}
                    </span>
                    <span className="text-gray-400">·</span>
                    <label className="flex items-center gap-1">
                      {t('common.perPage')}
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
                    </label>
                  </div>
                  <div className="flex items-center gap-1">
                    <button
                      onClick={() => setPage(1)}
                      disabled={clamped <= 1}
                      className="px-2 py-1 rounded border border-gray-300 disabled:opacity-40 disabled:cursor-not-allowed hover:bg-gray-50"
                    >
                      {t('common.first')}
                    </button>
                    <button
                      onClick={() => setPage(p => Math.max(1, p - 1))}
                      disabled={clamped <= 1}
                      className="px-2 py-1 rounded border border-gray-300 disabled:opacity-40 disabled:cursor-not-allowed hover:bg-gray-50"
                    >
                      {t('common.prev')}
                    </button>
                    <button
                      onClick={() => setPage(p => Math.min(totalPages, p + 1))}
                      disabled={clamped >= totalPages}
                      className="px-2 py-1 rounded border border-gray-300 disabled:opacity-40 disabled:cursor-not-allowed hover:bg-gray-50"
                    >
                      {t('common.next')}
                    </button>
                    <button
                      onClick={() => setPage(totalPages)}
                      disabled={clamped >= totalPages}
                      className="px-2 py-1 rounded border border-gray-300 disabled:opacity-40 disabled:cursor-not-allowed hover:bg-gray-50"
                    >
                      {t('common.last')}
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
