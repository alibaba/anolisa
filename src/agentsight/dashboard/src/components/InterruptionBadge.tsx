/**
 * InterruptionBadge — compact severity indicator shown next to session/trace rows.
 *
 * Supports two usage modes:
 *   1. Simple (legacy): <InterruptionBadge count={3} severity="high" />
 *   2. Detailed: <InterruptionBadge bySeverity={{ critical: 1, high: 2 }} types={[...]} />
 *
 * Tooltip uses CSS group-hover for immediate, reliable display (no native title delay).
 */

import React from 'react';
import type { InterruptionSeverity, InterruptionTypeDetail } from '../utils/apiClient';
import { INTERRUPTION_TYPE_CN } from '../utils/apiClient';

const SEVERITY_STYLES: Record<InterruptionSeverity, string> = {
  critical: 'bg-red-600 text-white',
  high:     'bg-orange-500 text-white',
  medium:   'bg-yellow-400 text-gray-900',
  low:      'bg-blue-400 text-white',
};

const SEVERITY_LABEL: Record<InterruptionSeverity, string> = {
  critical: '严重',
  high:     '高危',
  medium:   '中危',
  low:      '低危',
};

const SEVERITY_ORDER: InterruptionSeverity[] = ['critical', 'high', 'medium', 'low'];

/** Build tooltip lines from type details for a given severity. */
function buildTypeTooltipLines(types: InterruptionTypeDetail[], severity: string): string[] {
  return types
    .filter((t) => t.severity === severity)
    .sort((a, b) => b.count - a.count)
    .map((t) => `${INTERRUPTION_TYPE_CN[t.interruption_type] ?? t.interruption_type}: ${t.count} 次`);
}

/** CSS tooltip positioned above the badge. */
const CssTooltip: React.FC<{ lines: string[] }> = ({ lines }) => {
  if (lines.length === 0) return null;
  return (
    <span className="absolute bottom-full left-1/2 -translate-x-1/2 mb-1.5 hidden group-hover:flex flex-col items-start px-2 py-1.5 rounded bg-gray-800 text-white text-xs whitespace-nowrap shadow-lg z-50 pointer-events-none">
      {lines.map((line, i) => (
        <span key={i}>{line}</span>
      ))}
      {/* arrow */}
      <span className="absolute top-full left-1/2 -translate-x-1/2 border-4 border-transparent border-t-gray-800" />
    </span>
  );
};

interface Props {
  /** Total count (used for simple/legacy mode). */
  count?: number;
  /** Single severity (used for simple/legacy mode). */
  severity?: InterruptionSeverity;
  /** Per-severity breakdown (detailed mode). */
  bySeverity?: { critical: number; high: number; medium: number; low: number };
  /** Type details for tooltip (detailed mode). */
  types?: InterruptionTypeDetail[];
  /** Optional tooltip / title text */
  title?: string;
  onClick?: () => void;
}

export const InterruptionBadge: React.FC<Props> = ({ count, severity, bySeverity, types, title, onClick }) => {
  // Detailed mode: render one badge per non-zero severity
  if (bySeverity) {
    const badges = SEVERITY_ORDER
      .filter((sev) => (bySeverity[sev] ?? 0) > 0)
      .map((sev) => {
        const cnt = bySeverity[sev];
        const style = SEVERITY_STYLES[sev];
        const label = SEVERITY_LABEL[sev];
        const lines = types ? buildTypeTooltipLines(types, sev) : [`${cnt} ${label}`];
        return (
          <span
            key={sev}
            className={`relative group inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-xs font-semibold cursor-pointer select-none ${style}`}
            onClick={onClick}
          >
            {cnt} {label}
            <CssTooltip lines={lines} />
          </span>
        );
      });
    if (badges.length === 0) return null;
    return <span className="inline-flex items-center gap-1">{badges}</span>;
  }

  // Simple/legacy mode
  if (!count || count === 0) return null;
  const sev = severity ?? 'medium';
  const style = SEVERITY_STYLES[sev] ?? SEVERITY_STYLES.medium;
  const label = SEVERITY_LABEL[sev] ?? sev.toUpperCase();
  const lines = title ? [title] : [`${count} ${label}`];

  return (
    <span
      className={`relative group inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-xs font-semibold cursor-pointer select-none ${style}`}
      onClick={onClick}
    >
      {count} {label}
      <CssTooltip lines={lines} />
    </span>
  );
};

export default InterruptionBadge;
