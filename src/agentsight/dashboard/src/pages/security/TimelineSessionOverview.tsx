import React from 'react';
import type {
  SecurityApiResponse,
  SecurityEventRecord,
  SecurityPaginated,
  SecurityRunSummary,
  SecuritySessionSummary,
} from '../../utils/apiClient';
import {
  fmtNumber,
  isPassVerdict,
  securityEventVerdict,
  shortId,
  verdictBadgeClasses,
  verdictCountItems,
  verdictTone,
} from './utils';

export const TimelineSessionOverview: React.FC<{
  session: SecuritySessionSummary | null;
  run: SecurityRunSummary | null;
  eventsResponse: SecurityApiResponse<SecurityPaginated<SecurityEventRecord>> | null;
  loading: boolean;
  error: string | null;
}> = ({ session, run, eventsResponse, loading, error }) => {
  const events = eventsResponse?.data.items ?? [];
  const totalSecurityEvents = eventsResponse?.data.total ?? session?.security_event_count ?? 0;
  const verdictItems = verdictCountItems(events);
  const verdictTotal = verdictItems.reduce((sum, item) => sum + item.count, 0);
  const nonPassCount = events.filter((event) => {
    const verdict = securityEventVerdict(event);
    return verdict !== '-' && !isPassVerdict(verdict);
  }).length;
  const hasRiskVerdict = events.some((event) => verdictTone(securityEventVerdict(event)) === 'risk');

  let statusLabel = '选择 Session 后统计 verdict';
  let statusClasses = 'bg-gray-100 text-gray-700';
  if (loading) {
    statusLabel = 'Verdict 统计中...';
  } else if (error) {
    statusLabel = 'Verdict 统计失败';
    statusClasses = 'bg-red-100 text-red-700';
  } else if (!session) {
    statusLabel = '未选择 Session';
  } else if (totalSecurityEvents === 0) {
    statusLabel = '无安全事件';
  } else if (verdictTotal === 0) {
    statusLabel = '暂无 verdict';
  } else if (nonPassCount === 0) {
    statusLabel = '全部 verdict 为 pass';
    statusClasses = 'bg-green-100 text-green-700';
  } else {
    statusLabel = `存在 ${nonPassCount} 个非 pass verdict`;
    statusClasses = hasRiskVerdict ? 'bg-red-100 text-red-700' : 'bg-amber-100 text-amber-800';
  }

  const metrics = [
    ['Session', shortId(session?.session_id, 18)],
    ['当前 Run', shortId(run?.run_id, 18)],
    ['Turns', fmtNumber(session?.turn_count)],
    ['观测事件', fmtNumber(session?.observability_event_count)],
    ['安全事件', fmtNumber(totalSecurityEvents)],
  ];

  return (
    <div className="rounded-lg border border-gray-200 bg-white p-4 shadow-sm">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h3 className="text-sm font-semibold text-gray-900">Session 总览</h3>
          <p className="mt-1 text-xs text-gray-500">按当前时间范围统计所选 session 的安全事件 verdict</p>
        </div>
        <span className={`rounded px-2 py-0.5 text-xs font-medium ${statusClasses}`}>
          {statusLabel}
        </span>
      </div>

      <div className="mt-4 grid gap-3 sm:grid-cols-2 lg:grid-cols-5">
        {metrics.map(([label, value]) => (
          <div key={label} className="rounded-lg border border-gray-100 bg-gray-50 px-3 py-2">
            <p className="text-xs text-gray-400">{label}</p>
            <p className="mt-1 break-all font-mono text-xs text-gray-800">{value}</p>
          </div>
        ))}
      </div>

      {verdictItems.length > 0 && (
        <div className="mt-3 flex flex-wrap gap-2">
          {verdictItems.map((item) => (
            <span key={String(item.value)} className={`rounded px-2 py-0.5 text-xs font-medium ${verdictBadgeClasses(String(item.value))}`}>
              {String(item.value)} {fmtNumber(item.count)}
            </span>
          ))}
        </div>
      )}
      {error && <p className="mt-3 text-xs text-red-600">{error}</p>}
    </div>
  );
};
