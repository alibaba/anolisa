import React, { useEffect, useRef, useState } from 'react';
import {
  containSecurityCase,
  fetchContainmentPlan,
  SecurityApiClientError,
  type SecurityContainmentAction,
  type SecurityContainmentPlan,
} from '../utils/apiClient';

type DurationMode = 'temporary' | 'persistent';

export interface ContainmentDialogProps {
  caseId: string;
  open: boolean;
  onClose: () => void;
  onContained: (action: SecurityContainmentAction) => void;
}

const containmentErrorMessages: Record<string, string> = {
  source_policy_unavailable: '原始策略来源已不可用，无法安全生成拦截规则。',
  root_process_stale: '目标进程已变化，请刷新后选择在线 Agent。',
  ambiguous_candidate: '目标进程身份不唯一，请刷新 Agent 状态后重试。',
  case_not_eligible: '当前案件状态不允许升级为拦截。',
  case_eligibility_changed: '案件状态已变化，请关闭弹窗并刷新案件。',
  invalid_duration: '拦截时长不在服务端允许的范围内。',
  incompatible_action: '该案件已有不同的拦截动作，请先查看现有动作。',
  action_in_progress: '拦截策略正在下发，请稍后刷新案件状态。',
  action_expiring: '现有拦截策略正在解除，请稍后重试。',
  cleanup_required: '旧策略仍需清理，请稍后重试或联系管理员。',
  enforcer_unavailable: '内核执行服务暂不可用，请稍后重试。',
  containment_disabled: '当前环境未启用风险拦截能力。',
  recovery_failed: '现有拦截动作恢复失败，请先处理该动作。',
  health_store_unavailable: '在线 Agent 状态暂不可用，请稍后重试。',
};

function safeErrorMessage(error: unknown): string {
  if (error instanceof SecurityApiClientError) {
    return containmentErrorMessages[error.code]
      ?? (error.retryable ? '请求暂时失败，请稍后重试。' : '无法完成拦截，请刷新案件状态。');
  }
  return '请求失败，请稍后重试。';
}

export const ContainmentDialog: React.FC<ContainmentDialogProps> = ({
  caseId,
  open,
  onClose,
  onContained,
}) => {
  const [plan, setPlan] = useState<SecurityContainmentPlan | null>(null);
  const [selectedPid, setSelectedPid] = useState<number | null>(null);
  const [durationMode, setDurationMode] = useState<DurationMode>('temporary');
  const [loading, setLoading] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState('');
  const requestVersion = useRef(0);
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    requestVersion.current += 1;
    const version = requestVersion.current;
    setPlan(null);
    setSelectedPid(null);
    setDurationMode('temporary');
    setSubmitting(false);
    setError('');
    if (!open) {
      setLoading(false);
      return undefined;
    }

    setLoading(true);
    void fetchContainmentPlan(caseId)
      .then((response) => {
        if (requestVersion.current !== version) return;
        const nextPlan = response.data;
        setPlan(nextPlan);
        setSelectedPid(
          nextPlan.original_target_valid && nextPlan.original_target
            ? nextPlan.original_target.root_pid
            : null,
        );
      })
      .catch((nextError) => {
        if (requestVersion.current === version) setError(safeErrorMessage(nextError));
      })
      .finally(() => {
        if (requestVersion.current === version) setLoading(false);
      });
    return () => {
      if (requestVersion.current === version) requestVersion.current += 1;
    };
  }, [caseId, open]);

  useEffect(() => {
    if (!open) return undefined;
    dialogRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !submitting) onClose();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [onClose, open, submitting]);

  if (!open) return null;

  const canSubmit = Boolean(plan && selectedPid !== null && !loading && !submitting);
  const originalTarget = plan?.original_target;
  const targetStale = Boolean(plan && !plan.original_target_valid);
  const durationMinutes = Math.round((plan?.default_duration_secs ?? 900) / 60);

  const submit = async () => {
    if (!plan || selectedPid === null || loading || submitting) return;
    const version = requestVersion.current;
    setSubmitting(true);
    setError('');
    try {
      const response = await containSecurityCase(caseId, {
        root_pid: selectedPid,
        duration_secs: durationMode === 'temporary' ? plan.default_duration_secs : null,
      });
      if (requestVersion.current === version) onContained(response.data);
    } catch (nextError) {
      if (requestVersion.current === version) setError(safeErrorMessage(nextError));
    } finally {
      if (requestVersion.current === version) setSubmitting(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/50 p-4">
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="containment-dialog-title"
        tabIndex={-1}
        className="max-h-[90vh] w-full max-w-2xl overflow-y-auto rounded-2xl bg-white shadow-2xl outline-none"
      >
        <header className="flex items-start justify-between border-b border-gray-200 px-6 py-5">
          <div>
            <h2 id="containment-dialog-title" className="text-xl font-semibold text-gray-900">
              确认升级为内核拦截
            </h2>
            <p className="mt-1 text-sm text-gray-500">
              AgentSight 将从案件原始策略生成规则，Dashboard 不接收或展示策略 DSL。
            </p>
          </div>
          <button
            type="button"
            aria-label="关闭"
            onClick={onClose}
            disabled={submitting}
            className="rounded-lg px-2 py-1 text-xl text-gray-400 hover:bg-gray-100 hover:text-gray-700 disabled:opacity-40"
          >
            ×
          </button>
        </header>

        <div className="space-y-5 px-6 py-5">
          {loading && <p role="status" className="text-sm text-gray-500">正在加载拦截方案...</p>}

          {plan && (
            <>
              <dl className="grid gap-3 rounded-xl border border-gray-200 bg-gray-50 p-4 sm:grid-cols-2">
                <div>
                  <dt className="text-xs font-medium text-gray-500">敏感文件</dt>
                  <dd className="mt-1 text-sm font-medium text-gray-900">
                    由服务端案件策略安全恢复
                  </dd>
                </div>
                <div>
                  <dt className="text-xs font-medium text-gray-500">拦截范围</dt>
                  <dd className="mt-1 text-sm font-medium text-gray-900">所有非可信网络目标</dd>
                </div>
                <div>
                  <dt className="text-xs font-medium text-gray-500">执行效果</dt>
                  <dd className="mt-1 text-sm font-medium text-gray-900">ActPlane 内核拒绝（deny）</dd>
                </div>
                <div>
                  <dt className="text-xs font-medium text-gray-500">原始 Agent</dt>
                  <dd className="mt-1 text-sm font-medium text-gray-900">
                    {originalTarget
                      ? `${originalTarget.display_name} · PID ${originalTarget.root_pid}`
                      : '原始进程不可用'}
                  </dd>
                </div>
              </dl>

              {targetStale ? (
                <div>
                  <label htmlFor="containment-agent" className="text-sm font-medium text-gray-800">
                    选择在线 Agent
                  </label>
                  <p className="mt-1 text-xs text-amber-700">
                    原始 PID 已失效，必须选择同一 Agent 的在线进程。
                  </p>
                  <select
                    id="containment-agent"
                    required
                    value={selectedPid ?? ''}
                    onChange={(event) => setSelectedPid(
                      event.target.value ? Number(event.target.value) : null,
                    )}
                    className="mt-2 w-full rounded-lg border border-gray-300 bg-white px-3 py-2 text-sm"
                  >
                    <option value="">请选择在线 Agent</option>
                    {plan.candidates.map((candidate) => (
                      <option key={`${candidate.root_pid}-${candidate.process_start_time}`} value={candidate.root_pid}>
                        {candidate.display_name}（PID {candidate.root_pid}）
                      </option>
                    ))}
                  </select>
                </div>
              ) : originalTarget && (
                <p className="rounded-lg bg-emerald-50 px-3 py-2 text-sm text-emerald-700">
                  进程身份有效：PID {originalTarget.root_pid}
                </p>
              )}

              <fieldset className="space-y-3">
                <legend className="text-sm font-medium text-gray-800">拦截时长</legend>
                <label className="flex cursor-pointer items-start gap-3 rounded-xl border border-gray-200 p-4">
                  <input
                    type="radio"
                    name="containment-duration"
                    aria-label={`临时拦截 ${durationMinutes} 分钟`}
                    checked={durationMode === 'temporary'}
                    onChange={() => setDurationMode('temporary')}
                    className="mt-1"
                  />
                  <span>
                    <span className="block text-sm font-medium text-gray-900">
                      临时拦截 {durationMinutes} 分钟
                    </span>
                    <span className="mt-1 block text-xs text-gray-500">到期后由 AgentSight 自动解除。</span>
                  </span>
                </label>
                <label className="flex cursor-pointer items-start gap-3 rounded-xl border border-gray-200 p-4">
                  <input
                    type="radio"
                    name="containment-duration"
                    aria-label="持续拦截（需手动解除）"
                    checked={durationMode === 'persistent'}
                    onChange={() => setDurationMode('persistent')}
                    className="mt-1"
                  />
                  <span>
                    <span className="block text-sm font-medium text-gray-900">
                      持续拦截（需手动解除）
                    </span>
                    <span className="mt-1 block text-xs text-gray-500">
                      仅在明确选择后启用，不会自动到期。
                    </span>
                  </span>
                </label>
              </fieldset>
            </>
          )}

          {error && (
            <p role="alert" className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
              {error}
            </p>
          )}
        </div>

        <footer className="flex justify-end gap-3 border-t border-gray-200 px-6 py-4">
          <button
            type="button"
            onClick={onClose}
            disabled={submitting}
            className="rounded-lg border border-gray-300 bg-white px-4 py-2 text-sm text-gray-700 disabled:opacity-40"
          >
            取消
          </button>
          <button
            type="button"
            onClick={() => void submit()}
            disabled={!canSubmit}
            className="rounded-lg bg-red-600 px-4 py-2 text-sm font-medium text-white hover:bg-red-700 disabled:bg-red-300"
          >
            {submitting ? '正在下发...' : '确认并下发'}
          </button>
        </footer>
      </div>
    </div>
  );
};
