import React, { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  EvaluationNotReadyError,
  EvaluationRef,
  EvaluationResult,
  evaluateConversation,
} from '../utils/apiClient';
import { EvaluationBadge } from './EvaluationBadge';

interface EvaluationPanelProps {
  conversationId: string;
  initialResult: EvaluationResult | null;
  onResult?: (result: EvaluationResult) => void;
}

export const EvaluationPanel: React.FC<EvaluationPanelProps> = ({
  conversationId,
  initialResult,
  onResult,
}) => {
  const navigate = useNavigate();
  const [result, setResult] = useState<EvaluationResult | null>(initialResult);
  const [expanded, setExpanded] = useState(false);
  const [loading, setLoading] = useState(false);
  const [pendingCount, setPendingCount] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const runEvaluation = async (force: boolean) => {
    setLoading(true);
    setError(null);
    try {
      const response = await evaluateConversation(conversationId, force);
      setResult(response.result);
      setPendingCount(null);
      onResult?.(response.result);
    } catch (err) {
      if (err instanceof EvaluationNotReadyError) {
        setPendingCount(err.pendingCallCount);
      } else {
        setError(err instanceof Error ? err.message : '质量评估失败');
      }
    } finally {
      setLoading(false);
    }
  };

  const renderEvidenceLinks = (refs: EvaluationRef[]) => {
    if (refs.length === 0) return null;

    return (
      <div className="mt-1 flex flex-wrap gap-1">
        {refs.slice(0, 3).map((ref) => {
          const path = evidencePath(ref);
          return (
            <button
              key={`${ref.type}-${ref.id}-${ref.label}`}
              onClick={() => path && navigate(path)}
              disabled={!path}
              className="rounded border border-blue-200 bg-white px-1.5 py-0.5 text-[11px] text-blue-700 hover:bg-blue-50 disabled:cursor-not-allowed disabled:opacity-50"
              title={ref.id}
            >
              {evidenceLabel(ref.label)}
            </button>
          );
        })}
        {refs.length > 3 && <span className="text-[11px] text-gray-400">+{refs.length - 3}</span>}
      </div>
    );
  };

  return (
    <div className="border border-gray-200 bg-white rounded-lg p-3 text-sm">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="text-xs font-semibold text-gray-500">质量评估</span>
            <EvaluationBadge result={result} />
          </div>
          {result ? (
            <div className="mt-2 space-y-1">
              <p className="text-sm text-gray-800">{summaryText(result)}</p>
              <p className="text-xs text-gray-500">
                根因：<span>{rootCauseLabel(result.root_cause)}</span>
              </p>
              <p className="text-xs text-gray-600">{recommendedActionText(result)}</p>
            </div>
          ) : (
            <p className="mt-2 text-xs text-gray-500">暂无质量评估结果。</p>
          )}
        </div>
        <button
          onClick={() => runEvaluation(false)}
          disabled={loading}
          className="px-3 py-1 rounded border border-blue-300 bg-blue-50 text-blue-700 text-xs font-medium hover:bg-blue-100 disabled:opacity-50"
        >
          {loading ? '评估中...' : '开始评估'}
        </button>
      </div>

      {pendingCount !== null && (
        <div className="mt-3 flex items-center justify-between gap-3 rounded border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800">
          <span>{pendingCount} 个 LLM 调用仍未完成。</span>
          <button
            onClick={() => runEvaluation(true)}
            disabled={loading}
            className="rounded border border-amber-300 bg-white px-2 py-0.5 font-medium hover:bg-amber-100 disabled:opacity-50"
          >
            强制评估
          </button>
        </div>
      )}

      {result?.metadata.evaluated_with_pending && (
        <div className="mt-3 rounded border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800">
          评估时仍有 {result.metadata.pending_call_count} 个 LLM 调用未完成。
        </div>
      )}

      {error && (
        <div className="mt-3 rounded border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-700">
          {error}
        </div>
      )}

      {result && (
        <div className="mt-3">
          <button
            onClick={() => setExpanded((value) => !value)}
            className="text-xs font-medium text-blue-700 hover:text-blue-900"
          >
            {expanded ? '收起详情' : '查看详情'}
          </button>
          {expanded && (
            <div className="mt-2 grid gap-3 lg:grid-cols-2">
              <div>
                <h4 className="text-xs font-semibold text-gray-500">评估维度</h4>
                <div className="mt-1 space-y-1">
                  {result.dimensions.map((dimension) => (
                    <div key={dimension.name} className="rounded bg-gray-50 px-2 py-1">
                      <div className="flex items-center justify-between gap-2">
                        <span className="text-xs text-gray-700">{dimensionLabel(dimension.name)}</span>
                        <span className="text-xs text-gray-500">
                          {Math.round(dimension.score * 100)}
                        </span>
                      </div>
                      <p className="mt-0.5 text-xs text-gray-500">{reasonText(dimension.reason)}</p>
                      {renderEvidenceLinks(dimension.evidence_refs)}
                    </div>
                  ))}
                </div>
              </div>
              <div>
                <h4 className="text-xs font-semibold text-gray-500">问题发现</h4>
                <div className="mt-1 space-y-1">
                  {result.findings.length === 0 ? (
                    <p className="text-xs text-gray-400">未发现问题。</p>
                  ) : (
                    result.findings.map((finding) => (
                      <div key={`${finding.code}-${finding.message}`} className="rounded bg-gray-50 px-2 py-1">
                        <div className="flex items-center justify-between gap-2">
                          <span className="text-xs text-gray-700" title={finding.code}>
                            {findingLabel(finding.code)}
                          </span>
                          <span className="text-xs text-gray-500">{severityLabel(finding.severity)}</span>
                        </div>
                        <p className="mt-0.5 text-xs text-gray-500">{findingMessageText(finding.message)}</p>
                        {renderEvidenceLinks(finding.evidence_refs)}
                      </div>
                    ))
                  )}
                </div>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

function evidencePath(ref: EvaluationRef): string | null {
  if (!ref.deeplink) return null;

  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(ref.deeplink.query ?? {})) {
    if (value !== null && value !== undefined) {
      params.set(key, String(value));
    }
  }
  const query = params.toString();
  return query ? `${ref.deeplink.route}?${query}` : ref.deeplink.route;
}

function summaryText(result: EvaluationResult): string {
  if (result.verdict === 'pass') {
    return '会话已完成，未发现确定性的质量问题。';
  }
  if (result.verdict === 'warn') {
    return `当前会话可用，但需要复核：${rootCauseLabel(result.root_cause)}。`;
  }
  return `质量评估未通过，主要原因：${rootCauseLabel(result.root_cause)}。`;
}

function recommendedActionText(result: EvaluationResult): string {
  if (result.verdict === 'pass') {
    return '暂无需要立即处理的动作。';
  }

  const actions: Record<string, string> = {
    none: '复核告警项和支撑证据。',
    no_final_answer: '检查最后一次 LLM 调用和服务端响应解析。',
    interrupted_main_call: '检查中断证据，修复运行稳定性后再重试会话。',
    agent_crash: '重试前先检查 Agent 健康状态和崩溃诊断。',
    runtime_error: '检查模型服务错误、网络稳定性和重试行为。',
    tool_failure: '检查失败的工具调用和工具响应解析。',
    safety_risk: '重新运行 Agent 前先复核安全相关发现。',
    loop_detected: '检查重复调用并收紧停止条件。',
    excessive_cost: '复核提示词、工具输出和 Token 节省空间。',
    partial_snapshot: '等待 pending 调用完成，或保留强制评估标记。',
  };

  return actions[result.root_cause] ?? result.recommended_action ?? result.root_cause;
}

function rootCauseLabel(value: string): string {
  const labels: Record<string, string> = {
    none: '未发现明确根因',
    no_final_answer: '未生成最终回答',
    interrupted_main_call: '主调用被中断',
    agent_crash: 'Agent 崩溃',
    runtime_error: '运行时错误',
    tool_failure: '工具调用失败',
    safety_risk: '安全风险',
    loop_detected: '疑似循环调用',
    excessive_cost: '成本过高',
    partial_snapshot: '快照未完成',
  };

  return labels[value] ?? value;
}

function dimensionLabel(value: string): string {
  const labels: Record<string, string> = {
    completion: '完成度',
    runtime_health: '运行健康',
    tool_use: '工具使用',
    efficiency: '效率',
    safety: '安全',
  };

  return labels[value] ?? value;
}

function reasonText(value: string): string {
  const labels: Record<string, string> = {
    'No usable assistant output was captured.': '未捕获到可用的助手输出。',
    'A usable output exists.': '已捕获可用输出。',
    'A usable output exists, but the snapshot still has pending calls.': '已捕获可用输出，但快照仍有未完成调用。',
    'A usable assistant output was captured.': '已捕获可用的助手输出。',
    'One or more LLM calls were interrupted.': '一个或多个 LLM 调用被中断。',
    'Unresolved interruption signals were captured for this conversation.': '当前会话存在未解决的中断信号。',
    'The snapshot contains pending calls and may still change.': '快照包含未完成调用，结果仍可能变化。',
    'No runtime interruption was detected.': '未检测到运行时中断。',
    'Tool output contains deterministic error signals.': '工具输出包含确定性错误信号。',
    'The conversation required an unusually large number of LLM calls.': '当前会话的 LLM 调用次数异常偏高。',
    'No deterministic tool failure was detected.': '未检测到确定性工具故障。',
    'Token usage or call count is unusually high for a single conversation.': '单个会话的 Token 用量或调用次数异常偏高。',
    'Token usage or call count is elevated for a single conversation.': '单个会话的 Token 用量或调用次数偏高。',
    'Token usage and call count are within normal bounds.': 'Token 用量和调用次数处于正常范围。',
    'Safety-related interruption signal was captured.': '捕获到安全相关中断信号。',
    'No safety-specific signal was available or triggered.': '未发现安全专项信号触发。',
  };

  return labels[value] ?? value;
}

function findingLabel(value: string): string {
  const labels: Record<string, string> = {
    no_final_answer: '未生成最终回答',
    interrupted_main_call: '主调用被中断',
    partial_snapshot: '快照未完成',
    tool_failure: '工具调用失败',
    loop_detected: '疑似循环调用',
    llm_error: 'LLM 错误',
    sse_truncated: 'SSE 流截断',
    network_timeout: '网络超时',
    service_unavailable: '服务不可用',
    agent_crash: 'Agent 崩溃',
  };

  return labels[value] ?? value;
}

function findingMessageText(value: string): string {
  const labels: Record<string, string> = {
    'The conversation has no usable assistant output.': '会话没有可用的助手输出。',
    'An LLM call was interrupted before normal completion.': 'LLM 调用在正常完成前被中断。',
    'Evaluation was forced while LLM calls were still pending.': '仍有 LLM 调用未完成时执行了强制评估。',
    'Evaluation was forced while calls were pending.': '仍有调用未完成时执行了强制评估。',
    'An unresolved interruption was recorded for this conversation.': '当前会话存在未解决的中断记录。',
    'Tool output contains an error-like signal.': '工具输出包含疑似错误信号。',
    'The conversation used many LLM calls and may need loop inspection.': '会话使用了较多 LLM 调用，可能需要检查循环行为。',
  };

  return labels[value] ?? value;
}

function severityLabel(value: string): string {
  const labels: Record<string, string> = {
    critical: '严重',
    high: '高',
    medium: '中',
    low: '低',
  };

  return labels[value] ?? value;
}

function evidenceLabel(value: string): string {
  const labels: Record<string, string> = {
    'Assistant output': '助手输出',
    'No output': '无输出',
    'Interrupted LLM call': '中断的 LLM 调用',
    'Interrupted call': '中断调用',
    'Pending call': '未完成调用',
    'Tool failure signal': '工具故障信号',
    'Repeated calls': '重复调用',
    'High cost': '高成本',
    'Elevated cost': '成本偏高',
    'Pending snapshot': '未完成快照',
    'Tool failure': '工具故障',
  };

  return labels[value] ?? findingLabel(value);
}
