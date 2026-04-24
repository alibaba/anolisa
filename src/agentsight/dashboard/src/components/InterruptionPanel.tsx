/**
 * InterruptionPanel — sidebar / detail panel showing interruption events
 * for a selected session or trace.
 *
 * Usage:
 *   <InterruptionPanel sessionId="abc123" onClose={() => setPanelOpen(false)} />
 */

import React, { useEffect, useState, useCallback } from 'react';
import type { InterruptionRecord, InterruptionSeverity } from '../utils/apiClient';
import {
  fetchSessionInterruptions,
  fetchConversationInterruptions,
  resolveInterruption,
} from '../utils/apiClient';

// ─── Helpers ──────────────────────────────────────────────────────────────────

const SEVERITY_DOT: Record<InterruptionSeverity, string> = {
  critical: 'bg-red-600',
  high:     'bg-orange-500',
  medium:   'bg-yellow-400',
  low:      'bg-blue-400',
};

const TYPE_LABELS: Record<string, string> = {
  llm_error:        'LLM Error',
  sse_truncated:    'SSE Truncated',
  agent_crash:      'Agent Crash',
  token_limit:      'Token Limit',
  context_overflow: 'Context Overflow',
};

function formatNs(ns: number): string {
  return new Date(ns / 1_000_000).toLocaleString();
}

function parseDetail(raw: string | null): React.ReactNode {
  if (!raw) return null;
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

// ─── Single Row ───────────────────────────────────────────────────────────────

interface RowProps {
  event: InterruptionRecord;
  onResolved: (event: InterruptionRecord) => void;
}

const InterruptionRow: React.FC<RowProps> = ({ event, onResolved }) => {
  const [expanded, setExpanded] = useState(false);
  const [resolving, setResolving] = useState(false);
  const [resolveErr, setResolveErr] = useState<string | null>(null);

  const dotStyle = SEVERITY_DOT[event.severity as InterruptionSeverity] ?? 'bg-gray-400';
  const typeLabel = TYPE_LABELS[event.interruption_type] ?? event.interruption_type;

  const handleResolve = async () => {
    const confirmed = window.confirm(
      '标记为已处理后，此中断事件将不再计入未处理统计（badge 数字将减少）。\n\n确认标记为已处理吗？'
    );
    if (!confirmed) return;
    setResolving(true);
    setResolveErr(null);
    try {
      await resolveInterruption(event.interruption_id);
      onResolved(event);
    } catch (e: any) {
      setResolveErr(e.message ?? '操作失败，请稍后重试');
    } finally {
      setResolving(false);
    }
  };

  return (
    <div className="border rounded-lg p-3 mb-2 border-gray-200 bg-white shadow-sm">
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2 min-w-0">
          <span className={`inline-block w-2.5 h-2.5 rounded-full flex-shrink-0 ${dotStyle}`} />
          <span className="font-medium text-sm text-gray-800 truncate">{typeLabel}</span>
          <span className="text-xs text-gray-400">{formatNs(event.occurred_at_ns)}</span>
        </div>
        <div className="flex items-center gap-1 flex-shrink-0">
          <button
            onClick={handleResolve}
            disabled={resolving}
            title="标记为已处理，不再计入未处理统计"
            className="text-xs px-2 py-0.5 rounded bg-green-600 hover:bg-green-500 text-white disabled:opacity-50"
          >
            {resolving ? '…' : 'Resolve'}
          </button>
          <button
            onClick={() => setExpanded(x => !x)}
            className="text-xs px-2 py-0.5 rounded border border-gray-300 text-gray-600 hover:bg-gray-50"
          >
            {expanded ? 'Hide' : 'Detail'}
          </button>
        </div>
      </div>

      {resolveErr && (
        <p className="mt-1 text-xs text-red-500">{resolveErr}</p>
      )}

      {event.call_id && (
        <div className="mt-1 text-xs text-gray-400">call: {event.call_id}</div>
      )}

      {expanded && (
        <div className="mt-2">
          {parseDetail(event.detail)}
        </div>
      )}
    </div>
  );
};

// ─── Main Panel ───────────────────────────────────────────────────────────────

export interface ResolvedEventInfo {
  interruption_id: string;
  severity: string;
  interruption_type: string;
  session_id: string | null;
  trace_id: string | null;
  conversation_id: string | null;
}

interface Props {
  sessionId?: string;
  conversationId?: string;
  onClose?: () => void;
  onResolvedEvent?: (info: ResolvedEventInfo) => void;
}

export const InterruptionPanel: React.FC<Props> = ({ sessionId, conversationId, onClose, onResolvedEvent }) => {
  const [events, setEvents] = useState<InterruptionRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      let data: InterruptionRecord[];
      if (conversationId) {
        data = await fetchConversationInterruptions(conversationId);
      } else if (sessionId) {
        data = await fetchSessionInterruptions(sessionId);
      } else {
        data = [];
      }
      setEvents(data);
    } catch (e: any) {
      setError(e.message ?? 'Failed to load interruptions');
    } finally {
      setLoading(false);
    }
  }, [sessionId, conversationId]);

  useEffect(() => { void load(); }, [load]);

  const handleResolved = (resolved: InterruptionRecord) => {
    setEvents(prev => prev.filter(e => e.interruption_id !== resolved.interruption_id));
    onResolvedEvent?.({
      interruption_id: resolved.interruption_id,
      severity: resolved.severity,
      interruption_type: resolved.interruption_type,
      session_id: resolved.session_id,
      trace_id: resolved.trace_id,
      conversation_id: resolved.conversation_id,
    });
  };

  const unresolvedCount = events.length;

  return (
    <div className="flex flex-col h-full bg-white text-gray-800">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-gray-200">
        <div>
          <h3 className="font-semibold text-base text-gray-800">Interruptions</h3>
          {!loading && (
            <p className="text-xs text-gray-400">
              {unresolvedCount} 条未处理
            </p>
          )}
        </div>
        {onClose && (
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-700 text-xl leading-none"
            title="Close"
          >
            ×
          </button>
        )}
      </div>

      {/* Body */}
      <div className="flex-1 px-4 py-3">
        {loading && (
          <p className="text-sm text-gray-400 animate-pulse">Loading…</p>
        )}
        {error && (
          <p className="text-sm text-red-500">{error}</p>
        )}
        {!loading && !error && events.length === 0 && (
          <p className="text-sm text-gray-400">No interruption events recorded for this session.</p>
        )}
        {events.map(e => (
          <InterruptionRow key={e.interruption_id} event={e} onResolved={handleResolved} />
        ))}
      </div>
    </div>
  );
};

export default InterruptionPanel;
