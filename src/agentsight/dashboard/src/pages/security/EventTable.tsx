import React from 'react';
import type {
  SecurityApiResponse,
  SecurityEventRecord,
  SecurityPaginated,
} from '../../utils/apiClient';
import { StatePill } from './common';
import { EVENT_PAGE_SIZE } from './types';
import {
  badgeClasses,
  fmtNumber,
  fmtTime,
  securityEventVerdict,
  shortId,
  verdictBadgeClasses,
} from './utils';

export const EventTable: React.FC<{
  response: SecurityApiResponse<SecurityPaginated<SecurityEventRecord>> | null;
  loading: boolean;
  error: string | null;
  onSelect: (eventId: string) => void;
  onPage: (offset: number) => void;
  onViewTimeline?: (sessionId: string, runId: string) => void;
}> = ({ response, loading, error, onSelect, onPage, onViewTimeline }) => {
  const data = response?.data;
  const items = data?.items ?? [];
  const previousOffset = Math.max(0, (data?.offset ?? 0) - (data?.limit ?? EVENT_PAGE_SIZE));
  const hasPrevious = (data?.offset ?? 0) > 0;
  const hasNext = data?.next_offset != null;

  return (
    <div className="rounded-lg border border-gray-200 bg-white shadow-sm">
      <div className="flex items-center justify-between gap-3 border-b border-gray-200 px-4 py-3">
        <div className="flex items-center gap-2">
          <h3 className="text-sm font-semibold text-gray-900">安全事件</h3>
          {response && <StatePill state={response.state} />}
        </div>
        <span className="text-xs text-gray-400">Total {fmtNumber(data?.total)}</span>
      </div>

      {error && (
        <div className="border-b border-red-100 bg-red-50 px-4 py-3 text-sm text-red-700">{error}</div>
      )}
      {loading && items.length === 0 && (
        <div className="px-4 py-12 text-center text-sm text-gray-400">加载安全事件...</div>
      )}
      {!loading && !error && items.length === 0 && (
        <div className="px-4 py-12 text-center text-sm text-gray-400">所选过滤条件下暂无安全事件</div>
      )}
      {items.length > 0 && (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[980px]">
            <thead className="bg-gray-50">
              <tr>
                <th className="px-4 py-3 text-left text-xs font-semibold uppercase text-gray-500">时间</th>
                <th className="px-4 py-3 text-left text-xs font-semibold uppercase text-gray-500">Category</th>
                <th className="px-4 py-3 text-left text-xs font-semibold uppercase text-gray-500">Result</th>
                <th className="px-4 py-3 text-left text-xs font-semibold uppercase text-gray-500">Verdict</th>
                <th className="px-4 py-3 text-left text-xs font-semibold uppercase text-gray-500">Session</th>
                <th className="px-4 py-3 text-left text-xs font-semibold uppercase text-gray-500">Run</th>
                <th className="px-4 py-3 text-left text-xs font-semibold uppercase text-gray-500">Tool Call</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-100">
              {items.map((event) => {
                const verdict = securityEventVerdict(event);
                return (
                  <tr
                    key={event.event_id}
                    onClick={() => onSelect(event.event_id)}
                    className="cursor-pointer hover:bg-gray-50"
                  >
                    <td className="whitespace-nowrap px-4 py-3 text-xs text-gray-500">{fmtTime(event)}</td>
                    <td className="px-4 py-3">
                      <span className={`rounded px-2 py-0.5 text-xs font-medium ${badgeClasses(event.category, 'category')}`}>
                        {event.category ?? '-'}
                      </span>
                    </td>
                    <td className="px-4 py-3">
                      <span className={`rounded px-2 py-0.5 text-xs font-medium ${badgeClasses(event.result, 'result')}`}>
                        {event.result ?? '-'}
                      </span>
                    </td>
                    <td className="px-4 py-3">
                      <span className={`rounded px-2 py-0.5 text-xs font-medium ${verdictBadgeClasses(verdict)}`}>
                        {verdict}
                      </span>
                    </td>
                    <td className="px-4 py-3 font-mono text-xs text-gray-600" title={event.session_id ?? ''}>
                      {event.session_id ? shortId(event.session_id) : '-'}
                    </td>
                    <td className="px-4 py-3 font-mono text-xs text-gray-600" title={event.run_id ?? ''}>
                      {event.session_id && event.run_id && onViewTimeline ? (
                        <button
                          onClick={(e) => { e.stopPropagation(); onViewTimeline(event.session_id!, event.run_id!); }}
                          className="text-blue-600 hover:text-blue-800 hover:underline"
                        >
                          详情
                        </button>
                      ) : (
                        shortId(event.run_id)
                      )}
                    </td>
                    <td className="px-4 py-3 font-mono text-xs text-gray-600" title={event.tool_call_id ?? ''}>
                      {shortId(event.tool_call_id)}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {data && data.total > 0 && (
        <div className="flex items-center justify-end gap-2 border-t border-gray-200 px-4 py-3">
          <span className="text-xs text-gray-500">
            {data.offset + 1}-{Math.min(data.offset + data.items.length, data.total)} / {data.total}
          </span>
          <button
            onClick={() => onPage(previousOffset)}
            disabled={!hasPrevious || loading}
            className="rounded border border-gray-300 px-3 py-1 text-xs text-gray-700 hover:bg-gray-50 disabled:opacity-40"
          >
            上一页
          </button>
          <button
            onClick={() => onPage(data.next_offset ?? data.offset)}
            disabled={!hasNext || loading}
            className="rounded border border-gray-300 px-3 py-1 text-xs text-gray-700 hover:bg-gray-50 disabled:opacity-40"
          >
            下一页
          </button>
        </div>
      )}
    </div>
  );
};
