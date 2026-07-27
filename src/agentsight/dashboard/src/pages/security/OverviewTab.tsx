import React from 'react';
import type {
  SecurityApiResponse,
  SecurityCountItem,
  SecurityEventRecord,
  SecurityPaginated,
  SecuritySummary,
} from '../../utils/apiClient';
import { DistributionList, MetricCard } from './common';
import { OverviewRiskSummary } from './OverviewRiskSummary';
import { RecentEvents } from './RecentEvents';

export const OverviewTab: React.FC<{
  overviewError: string | null;
  overviewLoading: boolean;
  summary: SecurityApiResponse<SecuritySummary> | null;
  summaryData?: SecuritySummary;
  recentEvents: SecurityApiResponse<SecurityPaginated<SecurityEventRecord>> | null;
  categoryItems: SecurityCountItem[];
  eventTypeItems: SecurityCountItem[];
  resultItems: SecurityCountItem[];
  verdictItems: SecurityCountItem[];
  latestEvents: SecurityEventRecord[];
  onSelectEvent: (eventId: string) => void;
  onViewVerdict: (verdict: string) => void;
}> = ({
  overviewError,
  overviewLoading,
  summary,
  summaryData,
  recentEvents,
  categoryItems,
  eventTypeItems,
  resultItems,
  verdictItems,
  latestEvents,
  onSelectEvent,
  onViewVerdict,
}) => (
  <section className="space-y-5">
    {overviewError && (
      <div className="rounded-lg border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
        {overviewError}
      </div>
    )}
    {overviewLoading && !summary && (
      <div className="rounded-lg border border-gray-200 bg-white p-6 text-sm text-gray-400">
        加载安全汇总...
      </div>
    )}
    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
      <MetricCard label="安全事件" value={summaryData?.total ?? 0} />
      <MetricCard label="影响 Session" value={summaryData?.affected_sessions ?? 0} />
      <MetricCard label="影响 Run" value={summaryData?.affected_runs ?? 0} />
    </div>
    <OverviewRiskSummary
      summary={summaryData}
      eventsResponse={recentEvents}
      categoryItems={categoryItems}
      resultItems={resultItems}
      verdictItems={verdictItems}
      onViewVerdict={onViewVerdict}
    />
    {summary?.state === 'empty' && (
      <div className="rounded-lg border border-gray-200 bg-gray-50 px-4 py-8 text-center text-sm text-gray-500">
        所选范围内暂无安全事件。
      </div>
    )}
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
      <DistributionList title="按 Category" items={categoryItems} emptyText="暂无类别数据" />
      <DistributionList title="按 Event Type" items={eventTypeItems} emptyText="暂无事件类型数据" />
      <DistributionList title="按 Result" items={resultItems} emptyText="暂无结果数据" />
    </div>
    <RecentEvents events={latestEvents} onSelect={onSelectEvent} />
  </section>
);
