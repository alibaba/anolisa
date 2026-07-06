import React from 'react';
import { EvaluationResult } from '../utils/apiClient';

interface EvaluationBadgeProps {
  result: Pick<EvaluationResult, 'verdict' | 'score'> | null;
}

const STYLE_BY_VERDICT = {
  pass: 'bg-green-50 text-green-700 border-green-200',
  warn: 'bg-amber-50 text-amber-700 border-amber-200',
  fail: 'bg-red-50 text-red-700 border-red-200',
} as const;

const LABEL_BY_VERDICT = {
  pass: '通过',
  warn: '需复核',
  fail: '未通过',
} as const;

export const EvaluationBadge: React.FC<EvaluationBadgeProps> = ({ result }) => {
  if (!result) return null;

  return (
    <span
      className={`inline-flex items-center gap-1 rounded border px-2 py-0.5 text-xs font-semibold ${STYLE_BY_VERDICT[result.verdict]}`}
      title={`质量分 ${Math.round(result.score * 100)}`}
    >
      <span>{LABEL_BY_VERDICT[result.verdict]}</span>
      <span className="font-mono">{Math.round(result.score * 100)}</span>
    </span>
  );
};
