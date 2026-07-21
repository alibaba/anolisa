import React, { useCallback, useEffect, useRef, useState } from 'react';
import {
  createFileBinding,
  detachEnforcementBinding,
  fetchEnforcementBindings,
  fetchEnforcementHealth,
  fetchEnforcementViolations,
  type EnforcementBinding,
  type EnforcementHealth,
  type EnforcementViolation,
} from '../utils/apiClient';

const timestampFormatter = new Intl.DateTimeFormat('zh-CN', {
  dateStyle: 'short',
  timeStyle: 'medium',
});

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : '请求失败';
}

function policyFilePath(policyDsl: string): string {
  const matches = policyDsl
    .split(/\r?\n/)
    .map((line) => line.match(/^\s*block open file "([^"]+)" if AGENT\s*$/))
    .filter((match): match is RegExpMatchArray => match !== null);
  return matches.length === 1 ? matches[0][1] : '—';
}

function formatTimestamp(timestampNs: number): string {
  return timestampFormatter.format(timestampNs / 1_000_000);
}

const bindingStateLabels: Record<EnforcementBinding['state'], string> = {
  pending: '等待下发',
  enforced: '执行中',
  failed: '失败',
  degraded: '降级',
  detaching: '解除中',
  detached: '已解除',
};

const SummaryCard: React.FC<{ label: string; value: React.ReactNode; error?: string }> = ({
  label,
  value,
  error,
}) => (
  <div className="rounded-xl border border-gray-200 bg-white p-4 shadow-sm">
    <p className="text-xs font-medium uppercase tracking-wide text-gray-500">{label}</p>
    <div className="mt-2 text-2xl font-semibold text-gray-900">{value}</div>
    {error && <p className="mt-2 text-sm text-red-600">{error}</p>}
  </div>
);

export const RiskEnforcementPage: React.FC = () => {
  const [health, setHealth] = useState<EnforcementHealth | null>(null);
  const [bindings, setBindings] = useState<EnforcementBinding[]>([]);
  const [violations, setViolations] = useState<EnforcementViolation[]>([]);
  const [healthError, setHealthError] = useState('');
  const [bindingsError, setBindingsError] = useState('');
  const [violationsError, setViolationsError] = useState('');
  const [loading, setLoading] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [detachingId, setDetachingId] = useState('');
  const [operationResult, setOperationResult] = useState('');
  const [agentId, setAgentId] = useState('');
  const [rootPid, setRootPid] = useState('');
  const [path, setPath] = useState('');
  const [sessionId, setSessionId] = useState('');
  const loadEpoch = useRef(0);

  const loadAll = useCallback(async () => {
    const epoch = ++loadEpoch.current;
    setLoading(true);
    const [healthResult, bindingsResult, violationsResult] = await Promise.allSettled([
      fetchEnforcementHealth(),
      fetchEnforcementBindings(),
      fetchEnforcementViolations(),
    ]);
    if (epoch !== loadEpoch.current) return;

    if (healthResult.status === 'fulfilled') {
      setHealth(healthResult.value);
      setHealthError('');
    } else {
      setHealth(null);
      setHealthError(errorMessage(healthResult.reason));
    }
    if (bindingsResult.status === 'fulfilled') {
      setBindings(bindingsResult.value.bindings);
      setBindingsError('');
    } else {
      setBindingsError(errorMessage(bindingsResult.reason));
    }
    if (violationsResult.status === 'fulfilled') {
      setViolations(violationsResult.value.violations);
      setViolationsError('');
    } else {
      setViolationsError(errorMessage(violationsResult.reason));
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  const activeBindings = bindings.filter((binding) => binding.state !== 'detached');
  const blockedViolations = violations.filter((event) => event.blocked).length;
  const canMutate = health?.ready === true && !submitting && !detachingId;
  const newestViolations = [...violations]
    .sort((left, right) => right.occurred_at_ns - left.occurred_at_ns);
  const handleCreate = async (event: React.FormEvent) => {
    event.preventDefault();
    const parsedPid = Number(rootPid);
    if (!Number.isInteger(parsedPid) || parsedPid <= 0) {
      setOperationResult('PID 必须是正整数');
      return;
    }

    setSubmitting(true);
    setOperationResult('');
    try {
      await createFileBinding({
        agent_id: agentId.trim(),
        root_pid: parsedPid,
        path: path.trim(),
        session_id: sessionId.trim() || undefined,
      });
      setAgentId('');
      setRootPid('');
      setPath('');
      setSessionId('');
      setOperationResult('策略已生效');
      await loadAll();
    } catch (error) {
      setOperationResult(errorMessage(error));
    } finally {
      setSubmitting(false);
    }
  };

  const handleDetach = async (bindingId: string) => {
    if (!window.confirm('确认解除这条风险拦截策略？')) return;
    setDetachingId(bindingId);
    setOperationResult('');
    try {
      await detachEnforcementBinding(bindingId);
      setOperationResult('策略已解除');
      await loadAll();
    } catch (error) {
      setOperationResult(errorMessage(error));
    } finally {
      setDetachingId('');
    }
  };

  const healthUnavailable = health !== null || Boolean(healthError);
  const readinessLabel = health?.ready ? '运行中' : healthUnavailable ? '不可用' : '检测中';

  return (
    <div className="mx-auto w-full max-w-screen-xl space-y-6 p-6">
      <header className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <div className="flex items-center gap-3">
            <h1 className="text-2xl font-bold text-gray-900">风险拦截</h1>
            <span className={`rounded-full px-2.5 py-1 text-xs font-semibold ${
              health?.ready ? 'bg-green-100 text-green-700' : 'bg-gray-100 text-gray-600'
            }`}>
              {readinessLabel}
            </span>
          </div>
          <p className="mt-1 text-sm text-gray-500">
            为 Agent 进程树下发敏感文件访问策略，并追踪实际拦截记录。
          </p>
        </div>
        <button
          type="button"
          onClick={() => void loadAll()}
          disabled={loading}
          className="rounded-lg border border-gray-300 bg-white px-4 py-2 text-sm font-medium text-gray-700 disabled:opacity-50"
        >
          {loading ? '刷新中...' : '刷新'}
        </button>
      </header>

      <section className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <SummaryCard
          label="执行器状态"
          value={health?.ready ? '就绪' : healthUnavailable ? '不可用' : '检测中'}
          error={healthError || health?.message || undefined}
        />
        <SummaryCard label="执行后端" value={health?.backend || '—'} />
        <SummaryCard label="生效策略" value={activeBindings.length} />
        <SummaryCard label="拦截违规" value={blockedViolations} />
      </section>

      <section className="grid grid-cols-1 gap-6 xl:grid-cols-[minmax(0,2fr)_minmax(300px,1fr)]">
        <div className="overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm">
          <div className="border-b border-gray-200 px-5 py-4">
            <h2 className="font-semibold text-gray-900">策略绑定</h2>
            {bindingsError && <p className="mt-1 text-sm text-red-600">{bindingsError}</p>}
          </div>
          <div className="overflow-x-auto">
            <table className="w-full min-w-[720px] text-left text-sm">
              <thead className="bg-gray-50 text-xs uppercase text-gray-500">
                <tr>
                  <th className="px-4 py-3">Agent / PID</th>
                  <th className="px-4 py-3">敏感文件</th>
                  <th className="px-4 py-3">状态</th>
                  <th className="px-4 py-3">操作</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-100">
                {bindings.length === 0 ? (
                  <tr><td colSpan={4} className="px-4 py-10 text-center text-gray-400">暂无策略绑定</td></tr>
                ) : bindings.map((binding) => (
                  <tr key={binding.request.binding_id}>
                    <td className="px-4 py-3">
                      <div className="font-medium text-gray-900">{binding.request.agent_id}</div>
                      <div className="text-xs text-gray-500">PID {binding.request.root_pid}</div>
                    </td>
                    <td
                      aria-label={`策略路径 ${binding.request.binding_id}`}
                      className="max-w-xs break-all px-4 py-3 font-mono text-xs text-gray-700"
                    >
                      {policyFilePath(binding.request.policy_dsl)}
                    </td>
                    <td className="px-4 py-3 text-gray-700">{bindingStateLabels[binding.state]}</td>
                    <td className="px-4 py-3">
                      <button
                        type="button"
                        aria-label={`解除 ${binding.request.agent_id}（PID ${binding.request.root_pid}）对 ${policyFilePath(binding.request.policy_dsl)} 的策略`}
                        disabled={!canMutate || binding.state === 'detached'}
                        onClick={() => void handleDetach(binding.request.binding_id)}
                        className="text-sm font-medium text-red-600 disabled:text-gray-300"
                      >
                        {detachingId === binding.request.binding_id ? '解除中...' : '解除策略'}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>

        <form onSubmit={handleCreate} className="rounded-xl border border-gray-200 bg-white p-5 shadow-sm">
          <h2 className="font-semibold text-gray-900">下发文件策略</h2>
          <p className="mt-1 text-xs text-gray-500">目标必须是现存的绝对文件路径和存活的根进程。</p>
          <div className="mt-4 space-y-4">
            <label className="block text-sm font-medium text-gray-700">
              Agent ID
              <input
                required
                value={agentId}
                onChange={(event) => setAgentId(event.target.value)}
                className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 font-normal"
              />
            </label>
            <label className="block text-sm font-medium text-gray-700">
              PID
              <input
                required
                inputMode="numeric"
                value={rootPid}
                onChange={(event) => setRootPid(event.target.value)}
                className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 font-normal"
              />
            </label>
            <label className="block text-sm font-medium text-gray-700">
              敏感文件
              <input
                required
                value={path}
                onChange={(event) => setPath(event.target.value)}
                placeholder="/root/.ssh/id_rsa"
                className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 font-normal"
              />
            </label>
            <label className="block text-sm font-medium text-gray-700">
              Session ID（可选）
              <input
                value={sessionId}
                onChange={(event) => setSessionId(event.target.value)}
                className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 font-normal"
              />
            </label>
          </div>
          <button
            type="submit"
            disabled={!canMutate}
            className="mt-5 w-full rounded-lg bg-red-600 px-4 py-2 text-sm font-semibold text-white disabled:bg-gray-300"
          >
            {submitting ? '下发中...' : '下发策略'}
          </button>
          <p aria-live="polite" className="mt-3 min-h-5 text-sm text-gray-700">{operationResult}</p>
        </form>
      </section>

      <section className="overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm">
        <div className="border-b border-gray-200 px-5 py-4">
          <h2 className="font-semibold text-gray-900">拦截记录</h2>
          {violationsError && <p className="mt-1 text-sm text-red-600">{violationsError}</p>}
        </div>
        <div className="overflow-x-auto">
          <table className="w-full min-w-[840px] text-left text-sm">
            <thead className="bg-gray-50 text-xs uppercase text-gray-500">
              <tr>
                <th className="px-4 py-3">时间</th>
                <th className="px-4 py-3">Agent / PID</th>
                <th className="px-4 py-3">操作</th>
                <th className="px-4 py-3">目标</th>
                <th className="px-4 py-3">结果</th>
                <th className="px-4 py-3">原因</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-100">
              {newestViolations.length === 0 ? (
                <tr><td colSpan={6} className="px-4 py-10 text-center text-gray-400">暂无拦截记录</td></tr>
              ) : newestViolations.map((event) => (
                <tr key={event.event_id}>
                  <td className="whitespace-nowrap px-4 py-3 text-gray-600">
                    {formatTimestamp(event.occurred_at_ns)}
                  </td>
                  <td className="px-4 py-3">
                    <div className="font-medium text-gray-900">{event.agent_id}</div>
                    <div className="text-xs text-gray-500">PID {event.pid}</div>
                  </td>
                  <td className="px-4 py-3 font-mono text-xs text-gray-700">{event.operation}</td>
                  <td className="max-w-sm break-all px-4 py-3 font-mono text-xs text-gray-700">{event.target}</td>
                  <td className="px-4 py-3">
                    <span className={`rounded-full px-2 py-1 text-xs font-semibold ${
                      event.blocked ? 'bg-red-100 text-red-700' : 'bg-yellow-100 text-yellow-700'
                    }`}>
                      {event.blocked ? '已拦截' : event.killed ? '已终止' : '已记录'}
                    </span>
                  </td>
                  <td className="px-4 py-3 text-gray-600">{event.reason || '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>
    </div>
  );
};
