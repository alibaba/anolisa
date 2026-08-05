import React, { useEffect, useMemo, useState } from 'react';
import type { SecurityContainmentAction } from '../utils/apiClient';
import { containmentLifecyclePresentation } from '../utils/containmentLifecycle';

interface ContainmentLifecycleCardProps {
  action: SecurityContainmentAction | null;
  loading: boolean;
  error: boolean;
  canUpgrade: boolean;
  reviewing: boolean;
  onUpgrade: () => void;
  onResolve: () => void;
}

const failureStageLabel: Record<NonNullable<SecurityContainmentAction['failure_stage']>, string> = {
  attach: '策略挂载',
  detach: '策略解除',
  reconcile: '状态恢复',
};

function formatNs(timestampNs: number | null): string {
  if (!timestampNs) return '—';
  return new Intl.DateTimeFormat('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).format(timestampNs / 1_000_000);
}

function formatRemaining(expiresAtNs: number, nowMs: number): string {
  const seconds = Math.max(0, Math.ceil(expiresAtNs / 1_000_000 - nowMs) / 1_000);
  if (seconds === 0) return '等待状态刷新';
  const total = Math.ceil(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const remainder = total % 60;
  return `${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}:${String(remainder).padStart(2, '0')}`;
}

export const ContainmentLifecycleCard: React.FC<ContainmentLifecycleCardProps> = ({
  action,
  loading,
  error,
  canUpgrade,
  reviewing,
  onUpgrade,
  onResolve,
}) => {
  const [nowMs, setNowMs] = useState(Date.now());
  const expiryMs = action?.expires_at_ns ? action.expires_at_ns / 1_000_000 : null;

  useEffect(() => {
    let timer: number | undefined;
    const tick = () => {
      const current = Date.now();
      setNowMs(current);
      const remaining = expiryMs === null ? 0 : expiryMs - current;
      if (remaining > 0) timer = window.setTimeout(tick, Math.min(1_000, remaining));
    };
    tick();
    return () => {
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [expiryMs]);

  const presentation = useMemo(() => (
    action ? containmentLifecyclePresentation(action) : null
  ), [action]);
  const mayRetry = action?.lifecycle_state === 'failed' || action?.lifecycle_state === 'expired';

  return (
    <section className="mt-5 rounded-xl border border-gray-200 bg-slate-50 p-4" aria-label="风险拦截状态">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <p className="text-xs font-medium text-gray-500">风险拦截</p>
          {loading ? (
            <p role="status" className="mt-1 text-sm text-gray-600">正在加载拦截状态...</p>
          ) : error ? (
            <p role="alert" className="mt-1 text-sm text-red-700">拦截状态暂时不可用，请刷新后重试。</p>
          ) : presentation ? (
            <div className="mt-1 flex items-center gap-2">
              <span className={`rounded-full px-2.5 py-1 text-xs font-semibold ${presentation.style}`}>
                {presentation.label}
              </span>
              <span className="text-sm text-gray-600">{presentation.detail}</span>
            </div>
          ) : (
            <p className="mt-1 text-sm text-gray-600">待升级：当前仅审计，不阻断系统行为。</p>
          )}
        </div>
        {canUpgrade && (!action || mayRetry) && !loading && !error && (
          <button
            type="button"
            onClick={onUpgrade}
            className="rounded-lg bg-red-600 px-4 py-2 text-sm font-medium text-white hover:bg-red-700"
          >
            {mayRetry ? '重新下发拦截' : '升级为拦截'}
          </button>
        )}
      </div>

      {action && (
        <dl className="mt-4 grid gap-3 border-t border-gray-200 pt-4 text-sm sm:grid-cols-2 lg:grid-cols-3">
          <div><dt className="text-xs text-gray-500">目标进程</dt><dd className="mt-1 text-gray-900">PID {action.root_pid}</dd></div>
          <div><dt className="text-xs text-gray-500">策略绑定</dt><dd className="mt-1 break-all font-mono text-xs text-gray-900">{action.binding_id}</dd></div>
          <div><dt className="text-xs text-gray-500">到期时间</dt><dd className="mt-1 text-gray-900">{action.expires_at_ns ? formatNs(action.expires_at_ns) : '持续生效'}</dd></div>
          <div><dt className="text-xs text-gray-500">剩余时间</dt><dd className="mt-1 text-gray-900">{action.expires_at_ns ? formatRemaining(action.expires_at_ns, nowMs) : '需手动解除'}</dd></div>
          <div><dt className="text-xs text-gray-500">首次阻断</dt><dd className="mt-1 text-gray-900">{formatNs(action.blocked_at_ns)}</dd></div>
          <div><dt className="text-xs text-gray-500">失败阶段</dt><dd className="mt-1 text-gray-900">{action.failure_stage ? failureStageLabel[action.failure_stage] : '—'}</dd></div>
          {action.failure_summary && (
            <div className="sm:col-span-2 lg:col-span-3">
              <dt className="text-xs text-gray-500">失败说明</dt>
              <dd className="mt-1 text-gray-900">{action.failure_summary}</dd>
            </div>
          )}
        </dl>
      )}

      {action && (
        <div className="mt-4 flex justify-end border-t border-gray-200 pt-4">
          <button
            type="button"
            onClick={onResolve}
            disabled={reviewing}
            className="rounded border border-gray-300 bg-white px-3 py-1.5 text-xs text-gray-600 disabled:opacity-40"
          >
            标记已处置
          </button>
        </div>
      )}
    </section>
  );
};
