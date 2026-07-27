import React from 'react';
import type {
  SecurityApiResponse,
  SecurityCountItem,
  SecurityEventRecord,
  SecurityPaginated,
  SecuritySummary,
} from '../../utils/apiClient';
import {
  badgeClasses,
  fmtNumber,
  fmtPercent,
  isPassVerdict,
  verdictBadgeClasses,
  verdictBarClasses,
  verdictTone,
} from './utils';

export const OverviewRiskSummary: React.FC<{
  summary: SecuritySummary | undefined;
  eventsResponse: SecurityApiResponse<SecurityPaginated<SecurityEventRecord>> | null;
  categoryItems: SecurityCountItem[];
  resultItems: SecurityCountItem[];
  verdictItems: SecurityCountItem[];
  onViewVerdict?: (verdict: string) => void;
}> = ({ summary, eventsResponse, categoryItems, resultItems, verdictItems, onViewVerdict }) => {
  const totalEvents = eventsResponse?.data.total ?? summary?.total ?? 0;
  const verdictTotal = verdictItems.reduce((sum, item) => sum + item.count, 0);
  const maxVerdictCount = Math.max(1, ...verdictItems.map((item) => item.count));
  const nonPassCount = verdictItems
    .filter((item) => {
      const verdict = String(item.value);
      return verdict !== '-' && !isPassVerdict(verdict);
    })
    .reduce((sum, item) => sum + item.count, 0);
  const riskCount = verdictItems
    .filter((item) => verdictTone(String(item.value)) === 'risk')
    .reduce((sum, item) => sum + item.count, 0);
  const warningCount = verdictItems
    .filter((item) => verdictTone(String(item.value)) === 'warning')
    .reduce((sum, item) => sum + item.count, 0);
  const nonPassRatio = fmtPercent(nonPassCount, verdictTotal);
  const coverageText = `基于当前时间范围内 ${fmtNumber(verdictTotal)} 条含 verdict 事件统计`;

  let statusLabel = '暂无 verdict';
  let statusClasses = 'bg-gray-100 text-gray-700';
  let summaryText = totalEvents === 0
    ? '当前时间范围内未记录安全事件，安全能力没有发现需要展示的检查结果。'
    : `安全能力在当前时间范围内执行了 ${fmtNumber(totalEvents)} 次检查。`;
  if (verdictTotal > 0 && nonPassCount === 0) {
    statusLabel = '未发现非 pass verdict';
    statusClasses = 'bg-green-100 text-green-700';
    summaryText = `安全能力在当前时间范围内执行了 ${fmtNumber(totalEvents)} 次检查，已记录 verdict 的事件均为 pass。`;
  } else if (riskCount > 0) {
    statusLabel = `存在 ${fmtNumber(riskCount)} 个风险 verdict`;
    statusClasses = 'bg-red-100 text-red-700';
    summaryText = `安全能力在当前时间范围内执行了 ${fmtNumber(totalEvents)} 次检查，其中 ${fmtNumber(nonPassCount)} 个 verdict 不是 pass，需要关注风险操作。`;
  } else if (warningCount > 0 || nonPassCount > 0) {
    statusLabel = `存在 ${fmtNumber(nonPassCount)} 个待关注 verdict`;
    statusClasses = 'bg-amber-100 text-amber-800';
    summaryText = `安全能力在当前时间范围内执行了 ${fmtNumber(totalEvents)} 次检查，其中 ${fmtNumber(nonPassCount)} 个 verdict 不是 pass，建议复核。`;
  } else if (totalEvents > 0) {
    summaryText = `安全能力在当前时间范围内执行了 ${fmtNumber(totalEvents)} 次检查，但当前样本中暂无 verdict 明细。`;
  }

  return (
    <div className="rounded-lg border border-gray-200 bg-white p-4 shadow-sm">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h3 className="text-sm font-semibold text-gray-900">安全能力结论</h3>
          <p className="mt-1 text-sm text-gray-600">{summaryText}</p>
          <p className="mt-1 text-xs text-gray-400">{coverageText}</p>
        </div>
        <span className={`rounded px-2 py-0.5 text-xs font-medium ${statusClasses}`}>{statusLabel}</span>
      </div>

      <div className="mt-4 grid gap-3 sm:grid-cols-3">
        <div className="rounded-lg border border-gray-100 bg-gray-50 px-3 py-2">
          <p className="text-xs text-gray-400">安全能力执行</p>
          <p className="mt-1 text-xl font-semibold text-gray-900">{fmtNumber(totalEvents)}</p>
          <p className="mt-1 text-xs text-gray-500">
            覆盖 {fmtNumber(summary?.affected_sessions)} Session / {fmtNumber(summary?.affected_runs)} Run
          </p>
        </div>
        <div className="rounded-lg border border-gray-100 bg-gray-50 px-3 py-2">
          <p className="text-xs text-gray-400">风险操作占比</p>
          <p className={`mt-1 text-xl font-semibold ${nonPassCount > 0 ? 'text-red-700' : 'text-green-700'}`}>{nonPassRatio}</p>
          <p className="mt-1 text-xs text-gray-500">
            {fmtNumber(nonPassCount)} / {fmtNumber(verdictTotal)} 个 verdict 非 pass
          </p>
        </div>
        <div className="rounded-lg border border-gray-100 bg-gray-50 px-3 py-2">
          <p className="text-xs text-gray-400">需要关注</p>
          <p className="mt-1 text-xl font-semibold text-gray-900">{fmtNumber(riskCount + warningCount)}</p>
          <p className="mt-1 text-xs text-gray-500">
            风险 {fmtNumber(riskCount)} / Warning {fmtNumber(warningCount)}
          </p>
        </div>
      </div>

      <div className="mt-4 grid gap-4 lg:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)]">
        <div>
          <div className="flex items-center justify-between gap-3">
            <h4 className="text-xs font-semibold uppercase text-gray-500">按 Verdict 聚类</h4>
            {verdictTotal > 0 && <span className="text-xs text-gray-400">{fmtNumber(verdictTotal)} with verdict</span>}
          </div>
          {verdictItems.length === 0 ? (
            <div className="mt-3 rounded-lg border border-gray-100 bg-gray-50 px-3 py-6 text-center text-sm text-gray-400">
              暂无 verdict 聚类数据
            </div>
          ) : (
            <div className="mt-3 space-y-3">
              {verdictItems.map((item) => {
                const verdictValue = String(item.value);
                const isNonPass = !isPassVerdict(verdictValue) && verdictValue !== '-';
                return (
                <div key={verdictValue} className="grid grid-cols-[auto_auto_1fr_88px] items-center gap-3">
                  <span className={`w-fit rounded px-2 py-0.5 text-xs font-medium ${verdictBadgeClasses(verdictValue)}`}>
                    {verdictValue}
                  </span>
                  {isNonPass && onViewVerdict ? (
                    <button
                      onClick={() => onViewVerdict(verdictValue)}
                      className="whitespace-nowrap text-xs text-blue-600 hover:text-blue-800 hover:underline"
                    >
                      详情
                    </button>
                  ) : (
                    <span />
                  )}
                  <div className="h-2 overflow-hidden rounded-full bg-gray-100">
                    <div
                      className={`h-full rounded-full ${verdictBarClasses(verdictValue)}`}
                      style={{ width: `${Math.max(4, (item.count / maxVerdictCount) * 100)}%` }}
                    />
                  </div>
                  <span className="text-right text-xs font-semibold text-gray-700">
                    {fmtNumber(item.count)} · {fmtPercent(item.count, verdictTotal)}
                  </span>
                </div>
                );
              })}
            </div>
          )}
        </div>

        <div className="space-y-3">
          <div>
            <h4 className="text-xs font-semibold uppercase text-gray-500">安全能力做了什么</h4>
            <div className="mt-2 flex flex-wrap gap-2">
              {categoryItems.slice(0, 6).map((item) => (
                <span key={String(item.value)} className={`rounded px-2 py-0.5 text-xs font-medium ${badgeClasses(String(item.value), 'category')}`}>
                  {String(item.value)} {fmtNumber(item.count)}
                </span>
              ))}
              {categoryItems.length === 0 && <span className="text-xs text-gray-400">暂无检查动作数据</span>}
            </div>
          </div>
          <div>
            <h4 className="text-xs font-semibold uppercase text-gray-500">执行状态</h4>
            <div className="mt-2 flex flex-wrap gap-2">
              {resultItems.slice(0, 6).map((item) => (
                <span key={String(item.value)} className={`rounded px-2 py-0.5 text-xs font-medium ${badgeClasses(String(item.value), 'result')}`}>
                  {String(item.value)} {fmtNumber(item.count)}
                </span>
              ))}
              {resultItems.length === 0 && <span className="text-xs text-gray-400">暂无执行状态数据</span>}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};
