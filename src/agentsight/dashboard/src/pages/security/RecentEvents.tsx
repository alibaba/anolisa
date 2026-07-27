import React from 'react';
import type { SecurityEventRecord } from '../../utils/apiClient';
import { badgeClasses, fmtTime, securityEventVerdict, verdictBadgeClasses } from './utils';

export const RecentEvents: React.FC<{
  events: SecurityEventRecord[];
  onSelect: (eventId: string) => void;
}> = ({ events, onSelect }) => {
  const columns = 'grid-cols-[128px_120px_minmax(180px,1fr)_110px_110px]';
  return (
    <div className="overflow-hidden rounded-lg border border-gray-200 bg-white shadow-sm">
      <div className="border-b border-gray-200 px-4 py-3">
        <h3 className="text-sm font-semibold text-gray-900">近期安全事件</h3>
      </div>
      {events.length === 0 ? (
        <div className="px-4 py-10 text-center text-sm text-gray-400">所选范围内暂无安全事件</div>
      ) : (
        <div className="overflow-x-auto">
          <div className={`grid min-w-[760px] ${columns} items-center gap-3 border-b border-gray-200 bg-gray-50 px-4 py-2 text-xs font-medium text-gray-500`}>
            <span>时间</span>
            <span>Category</span>
            <span>Event Type</span>
            <span className="justify-self-end">Result</span>
            <span className="justify-self-end">Verdict</span>
          </div>
          <div className="min-w-[760px] divide-y divide-gray-100">
            {events.map((event) => {
              const verdict = securityEventVerdict(event);
              return (
                <button
                  key={event.event_id}
                  onClick={() => onSelect(event.event_id)}
                  className={`grid w-full ${columns} items-center gap-3 px-4 py-3 text-left hover:bg-gray-50`}
                >
                  <span className="text-xs text-gray-500">{fmtTime(event)}</span>
                  <span className={`w-fit rounded px-2 py-0.5 text-xs font-medium ${badgeClasses(event.category, 'category')}`}>
                    {event.category ?? '-'}
                  </span>
                  <span className="min-w-0 truncate text-sm text-gray-800" title={event.event_type ?? event.event_id}>
                    {event.event_type ?? event.event_id}
                  </span>
                  <span className={`w-fit justify-self-end rounded px-2 py-0.5 text-xs font-medium ${badgeClasses(event.result, 'result')}`}>
                    {event.result ?? '-'}
                  </span>
                  <span className={`w-fit justify-self-end rounded px-2 py-0.5 text-xs font-medium ${verdictBadgeClasses(verdict)}`}>
                    {verdict}
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
};
