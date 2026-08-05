import React, { useEffect, useRef, useState } from 'react';
import {
  containmentTargetCandidates,
  containSecurityCase,
  defaultContainmentTargetPid,
  enforcementSupportsContainment,
  fetchEnforcementHealth,
  fetchContainmentPlan,
  SecurityApiClientError,
  type EnforcementHealth,
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

function existingActionNotice(action: SecurityContainmentAction): {
  title: string;
  message: string;
  live: boolean;
} {
  switch (action.lifecycle_state) {
    case 'pending':
      return {
        title: '策略正在下发',
        message: '系统正在等待内核执行器确认，请稍后在案件详情中查看。',
        live: true,
      };
    case 'active':
      return action.duration_secs === null
        ? {
            title: '持续拦截已生效',
            message: '该案件已有持续生效的内核策略，请在案件详情中查看或解除。',
            live: true,
          }
        : {
            title: '临时拦截已生效',
            message: '该案件已有临时内核策略，请在案件详情中查看到期时间。',
            live: true,
          };
    case 'expiring':
      return {
        title: '策略正在解除',
        message: '内核策略正在清理，完成前不能重复下发。',
        live: true,
      };
    case 'expired':
      return {
        title: '上次临时拦截已到期，可重新下发',
        message: '新动作仍会重新校验当前在线进程身份。',
        live: false,
      };
    case 'failed':
      return {
        title: '上次拦截失败，可重新尝试',
        message: '请确认 Agent 在线且内核执行服务已恢复。',
        live: false,
      };
  }
}

export const ContainmentDialog: React.FC<ContainmentDialogProps> = ({
  caseId,
  open,
  onClose,
  onContained,
}) => {
  const [plan, setPlan] = useState<SecurityContainmentPlan | null>(null);
  const [selectedPid, setSelectedPid] = useState<number | null>(null);
  const [enforcementHealth, setEnforcementHealth] = useState<EnforcementHealth | null>(null);
  const [durationMode, setDurationMode] = useState<DurationMode>('temporary');
  const [loading, setLoading] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState('');
  const requestVersion = useRef(0);
  const dialogRef = useRef<HTMLDivElement>(null);
  const headerCloseRef = useRef<HTMLButtonElement>(null);
  const firstDecisionRef = useRef<HTMLElement | null>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    requestVersion.current += 1;
    const version = requestVersion.current;
    setPlan(null);
    setSelectedPid(null);
    setEnforcementHealth(null);
    setDurationMode('temporary');
    setSubmitting(false);
    setError('');
    if (!open) {
      setLoading(false);
      return undefined;
    }

    setLoading(true);
    void Promise.all([fetchContainmentPlan(caseId), fetchEnforcementHealth()])
      .then(([response, health]) => {
        if (requestVersion.current !== version) return;
        const nextPlan = response.data;
        setPlan(nextPlan);
        setEnforcementHealth(health);
        setSelectedPid(defaultContainmentTargetPid(nextPlan, health));
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
    previousFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    return () => {
      previousFocusRef.current?.focus();
      previousFocusRef.current = null;
    };
  }, [open]);

  const existingNotice = plan?.existing_action
    ? existingActionNotice(plan.existing_action)
    : null;
  const liveExistingAction = Boolean(existingNotice?.live);
  const containmentAvailable = enforcementSupportsContainment(enforcementHealth);
  const containmentUnavailableMessage = enforcementHealth === null
    ? '正在读取执行器能力与就绪状态。'
    : !enforcementHealth.ready
      ? '当前执行器尚未就绪，无法安全下发内核拦截。'
      : '当前执行器未显式声明凭据拦截与策略交接能力，无法下发内核拦截。';

  useEffect(() => {
    if (!open) return;
    if (plan && !liveExistingAction) {
      firstDecisionRef.current?.focus();
    } else {
      headerCloseRef.current?.focus();
    }
  }, [liveExistingAction, open, plan]);

  if (!open) return null;

  const targetCandidates = plan
    ? containmentTargetCandidates(plan, enforcementHealth)
    : [];
  const canSubmit = Boolean(
    plan
      && selectedPid !== null
      && targetCandidates.some((candidate) => candidate.root_pid === selectedPid)
      && containmentAvailable
      && !loading
      && !submitting
      && !liveExistingAction,
  );
  const originalTarget = plan?.original_target;
  const targetStale = Boolean(plan && !plan.original_target_valid);
  const durationMinutes = Math.round((plan?.default_duration_secs ?? 900) / 60);

  const handleDialogKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      if (!submitting) onCloseRef.current();
      return;
    }
    if (event.key !== 'Tab') return;

    const focusable = Array.from(event.currentTarget.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), select:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])',
    ));
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (!first || !last) {
      event.preventDefault();
      return;
    }
    const focusOutside = !event.currentTarget.contains(document.activeElement);
    if (event.shiftKey && (document.activeElement === first || focusOutside)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (document.activeElement === last || focusOutside)) {
      event.preventDefault();
      first.focus();
    }
  };

  const submit = async () => {
    if (
      !plan
      || selectedPid === null
      || !targetCandidates.some((candidate) => candidate.root_pid === selectedPid)
      || !containmentAvailable
      || loading
      || submitting
    ) return;
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
    <div
      data-testid="containment-backdrop"
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/50 p-4"
      onClick={(event) => {
        if (event.target === event.currentTarget && !submitting) onCloseRef.current();
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="containment-dialog-title"
        tabIndex={-1}
        onKeyDown={handleDialogKeyDown}
        className="max-h-[90vh] w-full max-w-2xl overflow-y-auto rounded-2xl bg-white shadow-2xl outline-none"
      >
        <header className="flex items-start justify-between border-b border-gray-200 px-6 py-5">
          <div>
            <h2 id="containment-dialog-title" className="text-xl font-semibold text-gray-900">
              {containmentAvailable ? '确认升级为内核拦截' : '内核拦截不可用'}
            </h2>
            <p className="mt-1 text-sm text-gray-500">
              AgentSight 将从案件原始策略生成规则，Dashboard 不接收或展示策略 DSL。
            </p>
          </div>
          <button
            ref={headerCloseRef}
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
                  <dd className="mt-1 break-all font-mono text-xs font-medium text-gray-900">
                    {plan.source_path}
                  </dd>
                </div>
                <div>
                  <dt className="text-xs font-medium text-gray-500">拦截范围</dt>
                  <dd className="mt-1 text-sm font-medium text-gray-900">
                    不受信任的公网 IPv4 目标
                  </dd>
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

              {existingNotice && (
                <div
                  role="status"
                  className={`rounded-xl border px-4 py-3 ${
                    existingNotice.live
                      ? 'border-blue-200 bg-blue-50 text-blue-800'
                      : 'border-amber-200 bg-amber-50 text-amber-800'
                  }`}
                >
                  <p className="text-sm font-semibold">{existingNotice.title}</p>
                  <p className="mt-1 text-xs">{existingNotice.message}</p>
                </div>
              )}

              {!liveExistingAction && (!targetStale && originalTarget ? (
                <p className="rounded-lg bg-emerald-50 px-3 py-2 text-sm text-emerald-700">
                  进程身份有效：PID {originalTarget.root_pid}
                </p>
              ) : targetCandidates.length > 0 ? (
                <div className="space-y-2 rounded-xl border border-amber-200 bg-amber-50 p-4">
                  <label htmlFor="containment-target" className="block text-sm font-medium text-amber-900">
                    选择同一 Agent 的在线进程
                  </label>
                  <select
                    id="containment-target"
                    value={selectedPid ?? ''}
                    onChange={(event) => {
                      setSelectedPid(event.target.value === '' ? null : Number(event.target.value));
                    }}
                    className="w-full rounded-lg border border-amber-300 bg-white px-3 py-2 text-sm text-gray-900"
                  >
                    <option value="">请选择在线 Agent</option>
                    {targetCandidates.map((candidate) => (
                      <option
                        key={`${candidate.root_pid}:${candidate.process_start_time}`}
                        value={candidate.root_pid}
                      >
                        {candidate.display_name} · PID {candidate.root_pid}
                      </option>
                    ))}
                  </select>
                  <p className="text-xs text-amber-800">
                    原始 PID 已失效；服务端会在下发前再次校验所选进程的启动时间与 Agent 身份。
                  </p>
                </div>
              ) : (
                <p className="rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-800">
                  原始进程身份已失效，且当前未发现执行器允许的同一 Agent 在线进程。
                </p>
              ))}

              {!containmentAvailable && (
                <p className="rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-800">
                  {containmentUnavailableMessage}
                </p>
              )}

              {!liveExistingAction && <fieldset className="space-y-3">
                <legend className="text-sm font-medium text-gray-800">拦截时长</legend>
                <label className="flex cursor-pointer items-start gap-3 rounded-xl border border-gray-200 p-4">
                  <input
                    ref={(element) => { firstDecisionRef.current = element; }}
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
              </fieldset>}
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
            {liveExistingAction ? '关闭' : '取消'}
          </button>
          {!liveExistingAction && <button
            type="button"
            onClick={() => void submit()}
            disabled={!canSubmit}
            className="rounded-lg bg-red-600 px-4 py-2 text-sm font-medium text-white hover:bg-red-700 disabled:bg-red-300"
          >
            {submitting ? '正在下发...' : '确认并下发'}
          </button>}
        </footer>
      </div>
    </div>
  );
};
