import React, { useCallback, useEffect, useRef, useState } from 'react';
import {
  createCredentialBinding,
  detachEnforcementBinding,
  enforcementSupportsMode,
  enforcementViolationTotal,
  fetchEnforcementBindings,
  fetchEnforcementHealth,
  fetchEnforcementViolations,
  type EnforcementBinding,
  type EnforcementHealth,
  type EnforcementViolation,
  type EnforcementPolicyMode,
} from '../utils/apiClient';
import { useI18n, useLocaleTag } from '../i18n';
import type { MessageKey } from '../i18n';

function policyFilePath(policyDsl: string): string {
  const matches = policyDsl
    .split(/\r?\n/)
    .map((line) => line.match(/^\s*source [A-Z_][A-Z0-9_]* = file "([^"]+)"\s*$/)
      ?? line.match(/^\s*block open file "([^"]+)" if AGENT\s*$/))
    .filter((match): match is RegExpMatchArray => match !== null);
  return matches.length === 1 ? matches[0][1] : '—';
}

function legacyBindingMode(policyDsl: string): EnforcementPolicyMode {
  if (/\bblock connect endpoint\b/.test(policyDsl) || /\bblock open file\b/.test(policyDsl)) return 'enforce';
  if (/\bnotify connect endpoint\b/.test(policyDsl)) return 'audit';
  return 'observe';
}

const modeLabels: Record<EnforcementPolicyMode, MessageKey> = {
  observe: 'risk.mode.observe',
  audit: 'risk.mode.audit',
  enforce: 'risk.mode.enforce',
};

function formatTimestamp(timestampNs: number, localeTag: string): string {
  return new Intl.DateTimeFormat(localeTag, {
    dateStyle: 'short',
    timeStyle: 'medium',
  }).format(timestampNs / 1_000_000);
}

const bindingStateLabels: Record<EnforcementBinding['state'], MessageKey> = {
  pending: 'risk.bindingState.pending',
  enforced: 'risk.bindingState.enforced',
  failed: 'risk.bindingState.failed',
  degraded: 'risk.bindingState.degraded',
  detaching: 'risk.bindingState.detaching',
  detached: 'risk.bindingState.detached',
};

const effectLabels: Record<EnforcementViolation['effect'], MessageKey> = {
  notify: 'risk.effect.notify',
  block: 'risk.effect.block',
  kill: 'risk.effect.kill',
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
  const { t } = useI18n();
  const localeTag = useLocaleTag();

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
  const [mode, setMode] = useState<EnforcementPolicyMode>('audit');
  const [trustedEndpoint, setTrustedEndpoint] = useState('');
  const loadEpoch = useRef(0);

  const errorMessage = (error: unknown): string => (
    error instanceof Error ? error.message : t('risk.error.requestFailed')
  );

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
  }, [t]);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  const activeBindings = bindings.filter((binding) => binding.state === 'enforced');
  const displayedViolations = enforcementViolationTotal(violations, health);
  const supportsMode = (candidate: EnforcementPolicyMode): boolean => (
    enforcementSupportsMode(health, candidate)
  );
  const maxActiveBindings = health?.capabilities.max_active_bindings;
  const bindingLimitReached = typeof maxActiveBindings === 'number'
    && activeBindings.length >= maxActiveBindings;
  const canCreate = Boolean(
    health?.ready === true
      && !submitting
      && !detachingId
      && supportsMode(mode)
      && !bindingLimitReached,
  );
  const canDetach = !detachingId;
  const newestViolations = [...violations]
    .sort((left, right) => right.occurred_at_ns - left.occurred_at_ns);
  const handleCreate = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!canCreate) return;
    const parsedPid = Number(rootPid);
    if (!Number.isInteger(parsedPid) || parsedPid <= 0) {
      setOperationResult(t('risk.operation.invalidPid'));
      return;
    }

    setSubmitting(true);
    setOperationResult('');
    try {
      await createCredentialBinding({
        agent_id: agentId.trim(),
        root_pid: parsedPid,
        source_path: path.trim(),
        session_id: sessionId.trim() || undefined,
        trusted_endpoint: trustedEndpoint.trim() || undefined,
        revision: Math.floor(Date.now() / 1000),
        mode,
        taint_ttl_secs: 900,
        destination_scope: 'public_ipv4',
      });
      setAgentId('');
      setRootPid('');
      setPath('');
      setSessionId('');
      setTrustedEndpoint('');
      setOperationResult(t('risk.operation.createSucceeded'));
      await loadAll();
    } catch (error) {
      setOperationResult(errorMessage(error));
    } finally {
      setSubmitting(false);
    }
  };

  const handleDetach = async (bindingId: string) => {
    if (!window.confirm(t('risk.detach.confirm'))) return;
    setDetachingId(bindingId);
    setOperationResult('');
    try {
      await detachEnforcementBinding(bindingId);
      setOperationResult(t('risk.operation.detachSucceeded'));
      await loadAll();
    } catch (error) {
      setOperationResult(errorMessage(error));
    } finally {
      setDetachingId('');
    }
  };

  const healthUnavailable = health !== null || Boolean(healthError);
  const readinessLabel = health?.ready
    ? t('risk.readiness.ready')
    : healthUnavailable
      ? t('risk.readiness.unavailable')
      : t('risk.readiness.checking');
  const capabilityLabel = health === null
    ? t('risk.capability.loading')
    : health.capabilities.test_development
      ? t('risk.capability.testDev')
      : t('risk.capability.prod');
  const allowsCredentialEnforcement = health?.capabilities.credential_enforce === true;

  return (
    <div className="mx-auto w-full max-w-screen-xl space-y-6 p-6">
      <header className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <div className="flex items-center gap-3">
            <h1 className="text-2xl font-bold text-gray-900">
              {t(allowsCredentialEnforcement ? 'risk.title.enforce' : 'risk.title.observeAudit')}
            </h1>
            <span
              className={`rounded-full px-2.5 py-1 text-xs font-semibold ${
                health?.ready ? 'bg-green-100 text-green-700' : 'bg-gray-100 text-gray-600'
              }`}
            >
              {readinessLabel}
            </span>
          </div>
          <p className="mt-1 text-sm text-gray-500">
            {t('risk.subtitle')}
          </p>
        </div>
        <button
          type="button"
          onClick={() => void loadAll()}
          disabled={loading}
          className="rounded-lg border border-gray-300 bg-white px-4 py-2 text-sm font-medium text-gray-700 disabled:opacity-50"
        >
          {loading ? t('risk.refresh.loading') : t('common.refresh')}
        </button>
      </header>

      <section className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <SummaryCard
          label={t('risk.summary.enforcerStatus')}
          value={health?.ready
            ? t('risk.readiness.ready')
            : healthUnavailable
              ? t('risk.readiness.unavailable')
              : t('risk.readiness.checking')}
          error={healthError || health?.message || undefined}
        />
        <SummaryCard label={t('risk.summary.backend')} value={health?.backend || '—'} />
        <SummaryCard label={t('risk.summary.activeBindings')} value={activeBindings.length} />
        <SummaryCard
          label={t(allowsCredentialEnforcement
            ? 'risk.summary.blockedViolations'
            : 'risk.summary.auditedViolations')}
          value={displayedViolations}
        />
      </section>

      <section className="grid grid-cols-1 gap-6 xl:grid-cols-[minmax(0,2fr)_minmax(300px,1fr)]">
        <div className="overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm">
          <div className="border-b border-gray-200 px-5 py-4">
            <h2 className="font-semibold text-gray-900">{t('risk.bindings.title')}</h2>
            {bindingsError && <p className="mt-1 text-sm text-red-600">{bindingsError}</p>}
          </div>
          <div className="overflow-x-auto">
            <table className="w-full min-w-[720px] text-left text-sm">
              <thead className="bg-gray-50 text-xs uppercase text-gray-500">
                <tr>
                  <th className="px-4 py-3">{t('risk.bindings.header.agentPid')}</th>
                  <th className="px-4 py-3">{t('risk.bindings.header.sourcePath')}</th>
                  <th className="px-4 py-3">{t('risk.bindings.header.modeRevision')}</th>
                  <th className="px-4 py-3">{t('risk.bindings.header.state')}</th>
                  <th className="px-4 py-3">{t('risk.bindings.header.actions')}</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-100">
                {bindings.length === 0 ? (
                  <tr>
                    <td colSpan={5} className="px-4 py-10 text-center text-gray-400">
                      {t('risk.bindings.empty')}
                    </td>
                  </tr>
                ) : bindings.map((binding) => (
                  <tr key={binding.request.binding_id}>
                    <td className="px-4 py-3">
                      <div className="font-medium text-gray-900">{binding.request.agent_id}</div>
                      <div className="text-xs text-gray-500">PID {binding.request.root_pid}</div>
                      <div className="mt-1 break-all font-mono text-[11px] text-gray-400">
                        binding {binding.request.binding_id}
                      </div>
                      <div className="break-all font-mono text-[11px] text-gray-400">
                        policy {binding.request.policy_id}@{binding.request.policy_revision}
                      </div>
                    </td>
                    <td
                      aria-label={t('risk.bindings.sourcePathAria', {
                        bindingId: binding.request.binding_id,
                      })}
                      className="max-w-xs break-all px-4 py-3 font-mono text-xs text-gray-700"
                    >
                      {policyFilePath(binding.request.policy_dsl)}
                    </td>
                    <td className="px-4 py-3 text-gray-700">
                      <div>
                        {t(modeLabels[binding.request.policy_mode ?? legacyBindingMode(binding.request.policy_dsl)])}
                      </div>
                      <div className="text-xs text-gray-500">
                        {t('risk.bindings.revisionLabel', { revision: binding.request.policy_revision })}
                      </div>
                    </td>
                    <td className="px-4 py-3 text-gray-700">
                      <div>{t(bindingStateLabels[binding.state])}</div>
                      {binding.message && (
                        <div className="mt-1 max-w-xs break-words text-xs text-gray-500">
                          {binding.message}
                        </div>
                      )}
                    </td>
                    <td className="px-4 py-3">
                      <button
                        type="button"
                        aria-label={t('risk.bindings.detachAria', {
                          agentId: binding.request.agent_id,
                          pid: binding.request.root_pid,
                          path: policyFilePath(binding.request.policy_dsl),
                        })}
                        disabled={!canDetach || binding.state === 'detached'}
                        onClick={() => void handleDetach(binding.request.binding_id)}
                        className="text-sm font-medium text-red-600 disabled:text-gray-300"
                      >
                        {detachingId === binding.request.binding_id
                          ? t('risk.bindings.detaching')
                          : t('risk.bindings.detach')}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>

        <form onSubmit={handleCreate} className="rounded-xl border border-gray-200 bg-white p-5 shadow-sm">
          <h2 className="font-semibold text-gray-900">{t('risk.form.title')}</h2>
          <p className="mt-1 text-xs text-gray-500">{capabilityLabel}</p>
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
              {t('risk.form.sourcePathLabel')}
              <input
                required
                value={path}
                onChange={(event) => setPath(event.target.value)}
                placeholder="/root/.ssh/id_rsa"
                className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 font-normal"
              />
            </label>
            <label className="block text-sm font-medium text-gray-700">
              {t('risk.form.modeLabel')}
              <select
                value={mode}
                onChange={(event) => setMode(event.target.value as EnforcementPolicyMode)}
                className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 font-normal"
              >
                <option value="observe" disabled={!supportsMode('observe')}>
                  {t('risk.form.mode.observeOption')}
                </option>
                <option value="audit" disabled={!supportsMode('audit')}>
                  {t('risk.form.mode.auditOption')}
                </option>
                <option value="enforce" disabled={!supportsMode('enforce')}>
                  {t('risk.form.mode.enforceOption')}
                </option>
              </select>
            </label>
            <div className="block text-sm font-medium text-gray-700">
              {t('risk.form.scopeLabel')}
              <p className="mt-1 rounded-lg border border-gray-200 bg-gray-50 px-3 py-2 font-normal text-gray-600">
                {t('risk.form.scopeHelp')}
              </p>
            </div>
            <label className="block text-sm font-medium text-gray-700">
              {t('risk.form.trustedEndpointLabel')}
              <input
                value={trustedEndpoint}
                onChange={(event) => setTrustedEndpoint(event.target.value)}
                placeholder="10.0.0.8"
                className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 font-normal"
              />
            </label>
            <label className="block text-sm font-medium text-gray-700">
              {t('risk.form.sessionIdLabel')}
              <input
                value={sessionId}
                onChange={(event) => setSessionId(event.target.value)}
                className="mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 font-normal"
              />
            </label>
          </div>
          <button
            type="submit"
            disabled={!canCreate}
            className="mt-5 w-full rounded-lg bg-red-600 px-4 py-2 text-sm font-semibold text-white disabled:bg-gray-300"
          >
            {submitting ? t('risk.form.submit.loading') : t('risk.form.submit')}
          </button>
          {!supportsMode(mode) && (
            <p className="mt-2 text-xs text-amber-700">
              {health?.ready === false
                ? t('risk.form.mode.unreadyWarning')
                : t('risk.form.mode.unsupportedWarning')}
            </p>
          )}
          {bindingLimitReached && (
            <p className="mt-2 text-xs text-amber-700">
              {t('risk.form.bindingLimitWarning', { maxActiveBindings })}
            </p>
          )}
          <p aria-live="polite" className="mt-3 min-h-5 text-sm text-gray-700">
            {operationResult}
          </p>
        </form>
      </section>

      <section className="overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm">
        <div className="border-b border-gray-200 px-5 py-4">
          <div className="flex items-center justify-between gap-4">
            <h2 className="font-semibold text-gray-900">{t('risk.violations.title')}</h2>
            <a href="#/audit" className="text-sm font-medium text-blue-600">
              {t('risk.violations.linkToAudit')}
            </a>
          </div>
          {violationsError && <p className="mt-1 text-sm text-red-600">{violationsError}</p>}
        </div>
        <div className="overflow-x-auto">
          <table className="w-full min-w-[840px] text-left text-sm">
            <thead className="bg-gray-50 text-xs uppercase text-gray-500">
              <tr>
                <th className="px-4 py-3">{t('risk.violations.header.time')}</th>
                <th className="px-4 py-3">{t('risk.violations.header.agentPid')}</th>
                <th className="px-4 py-3">{t('risk.violations.header.operation')}</th>
                <th className="px-4 py-3">{t('risk.violations.header.target')}</th>
                <th className="px-4 py-3">{t('risk.violations.header.result')}</th>
                <th className="px-4 py-3">{t('risk.violations.header.reason')}</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-100">
              {newestViolations.length === 0 ? (
                <tr>
                  <td colSpan={6} className="px-4 py-10 text-center text-gray-400">
                    {t('risk.violations.empty')}
                  </td>
                </tr>
              ) : newestViolations.map((event) => (
                <tr key={event.event_id}>
                  <td className="whitespace-nowrap px-4 py-3 text-gray-600">
                    {formatTimestamp(event.occurred_at_ns, localeTag)}
                  </td>
                  <td className="px-4 py-3">
                    <div className="font-medium text-gray-900">{event.agent_id}</div>
                    <div className="text-xs text-gray-500">PID {event.pid}</div>
                    <div className="mt-1 break-all font-mono text-[11px] text-gray-400">
                      binding {event.binding_id}
                    </div>
                    <div className="break-all font-mono text-[11px] text-gray-400">
                      policy {event.policy_id}@{event.policy_revision}
                    </div>
                  </td>
                  <td className="px-4 py-3 font-mono text-xs text-gray-700">{event.operation}</td>
                  <td className="max-w-sm break-all px-4 py-3 font-mono text-xs text-gray-700">{event.target}</td>
                  <td className="px-4 py-3">
                    <span
                      className={`rounded-full px-2 py-1 text-xs font-semibold ${
                        event.blocked ? 'bg-red-100 text-red-700' : 'bg-yellow-100 text-yellow-700'
                      }`}
                    >
                      {event.blocked
                        ? t('risk.violations.result.blocked')
                        : event.killed
                          ? t('risk.violations.result.killed')
                          : t('risk.violations.result.logged')}
                    </span>
                    <div className="mt-2 font-mono text-[11px] text-gray-400">
                      effect {event.effect} ({t(effectLabels[event.effect])})
                    </div>
                  </td>
                  <td className="px-4 py-3 text-gray-600">
                    <div>{event.reason || '—'}</div>
                    <div className="mt-1 font-mono text-[11px] text-gray-400">
                      ActPlane {event.actplane_revision}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>
    </div>
  );
};
