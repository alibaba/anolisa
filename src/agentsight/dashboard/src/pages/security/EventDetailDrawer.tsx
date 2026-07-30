import React from 'react';
import type {
  SecurityApiResponse,
  SecurityEventDetailResponse,
  SecurityEventRecord,
} from '../../utils/apiClient';
import { StatePill } from './common';
import { fmtTime, securityEventVerdict, verdictBadgeClasses } from './utils';

export const EventDetailDrawer: React.FC<{
  eventId: string;
  detail: SecurityApiResponse<SecurityEventDetailResponse> | null;
  loading: boolean;
  error: string | null;
  onClose: () => void;
  onRetry: () => void;
}> = ({ eventId, detail, loading, error, onClose, onRetry }) => {
  const event = detail?.data.event;
  return (
    <div className="fixed inset-0 z-50 flex justify-end bg-black bg-opacity-30">
      <div className="flex h-full w-full max-w-3xl flex-col bg-white shadow-2xl">
        <div className="flex items-start justify-between gap-4 border-b border-gray-200 px-5 py-4">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <h2 className="text-base font-semibold text-gray-900">安全事件详情</h2>
              {detail && <StatePill state={detail.state} />}
            </div>
            <p className="mt-1 truncate font-mono text-xs text-gray-400">{eventId}</p>
          </div>
          <button
            onClick={onClose}
            className="rounded-lg px-2 py-1 text-lg leading-none text-gray-500 hover:bg-gray-100"
            aria-label="关闭"
          >
            x
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-5">
          {loading && <div className="py-10 text-center text-sm text-gray-400">加载详情...</div>}
          {error && (
            <div className="rounded-lg border border-red-200 bg-red-50 p-4">
              <p className="text-sm text-red-700">{error}</p>
              <button
                onClick={onRetry}
                className="mt-3 rounded-lg border border-red-300 bg-white px-3 py-1.5 text-sm text-red-700 hover:bg-red-50"
              >
                重试
              </button>
            </div>
          )}
          {!loading && !error && detail?.state === 'not_found' && (
            <div className="rounded-lg border border-gray-200 bg-gray-50 p-4 text-sm text-gray-500">
              该安全事件已不存在。
            </div>
          )}
          {!loading && !error && event && (
            <div className="space-y-5">
              {(event.redacted || detail.state === 'redacted') && (
                <div className="rounded-lg border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800">
                  敏感字段已由 agent-sec daemon 脱敏。
                </div>
              )}
              {(event.truncated || detail.state === 'truncated') && (
                <div className="rounded-lg border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800">
                  部分字段因大小限制被截断。
                </div>
              )}

              <div className="grid gap-3 sm:grid-cols-2">
                {[
                  ['时间', fmtTime(event)],
                  ['类别', event.category ?? '-'],
                  ['结果', event.result ?? '-'],
                  ['Verdict', securityEventVerdict(event)],
                  ['Session', event.session_id ?? '-'],
                  ['Run', event.run_id ?? '-'],
                  ['Call', event.call_id ?? '-'],
                  ['Tool Call', event.tool_call_id ?? '-'],
                  ['Trace', event.trace_id ?? '-'],
                ].map(([label, value]) => (
                  <div key={label} className="rounded-lg border border-gray-200 bg-gray-50 px-3 py-2">
                    <p className="text-xs text-gray-400">{label}</p>
                    <p className="mt-1 break-all font-mono text-xs text-gray-800">{value}</p>
                  </div>
                ))}
              </div>

              <div>
                <h3 className="mb-2 text-sm font-semibold text-gray-900">Details</h3>
                <pre className="max-h-[52vh] overflow-auto rounded-lg border border-gray-200 bg-gray-950 p-4 text-xs text-gray-100">
                  {JSON.stringify(event.details ?? event, null, 2)}
                </pre>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
