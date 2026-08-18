import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useI18n, useLocaleTag } from '../i18n';
import type { MessageKey } from '../i18n';
import { ContainmentDialog } from '../components/ContainmentDialog';
import { ContainmentLifecycleCard } from '../components/ContainmentLifecycleCard';
import {
  fetchSecurityCase,
  fetchSecurityCases,
  fetchAuditEvents,
  fetchAuditSessions,
  fetchAuditSummary,
  reviewSecurityCase,
  type SecurityEventRecord,
  type SecurityEvidenceEvent,
  type SecurityRiskCase,
  type SecurityRiskCaseDetail,
  type SecuritySessionSummary,
  type SecuritySummary,
} from '../utils/apiClient';

type AuditTab = 'overview' | 'sessions' | 'cases' | 'events';
const CASE_PAGE_SIZE = 10;
const SESSION_PAGE_SIZE = 20;
const EVENT_PAGE_SIZE = 50;

const tabs: Array<{ key: AuditTab; labelKey: MessageKey }> = [
  { key: 'overview', labelKey: 'audit.tab.overview' },
  { key: 'sessions', labelKey: 'audit.tab.sessions' },
  { key: 'cases', labelKey: 'audit.tab.cases' },
  { key: 'events', labelKey: 'audit.tab.events' },
];

const severityStyle: Record<SecurityRiskCase['severity'], string> = {
  low: 'bg-slate-100 text-slate-700',
  medium: 'bg-amber-100 text-amber-700',
  high: 'bg-orange-100 text-orange-700',
  critical: 'bg-red-100 text-red-700',
};

const statusLabel: Record<SecurityRiskCase['status'], MessageKey> = {
  open: 'audit.status.open',
  confirmed: 'audit.status.confirmed',
  false_positive: 'audit.status.falsePositive',
  accepted_risk: 'audit.status.acceptedRisk',
  resolved: 'audit.status.resolved',
};

const eventLabel: Record<string, MessageKey> = {
  file_action: 'audit.event.fileAction',
  taint_transition: 'audit.event.taintTransition',
  network_action: 'audit.event.networkAction',
  policy_decision: 'audit.event.policyDecision',
  enforcement_state: 'audit.event.enforcementState',
};

function formatTime(timestampNs: number, localeTag: string): string {
  if (!timestampNs) return '—';
  return new Intl.DateTimeFormat(localeTag, {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).format(timestampNs / 1_000_000);
}

function errorText(error: unknown, t: (key: MessageKey) => string): string {
  return error instanceof Error ? error.message : t('audit.error.loadFailed');
}

function decisionText(detail: SecurityRiskCaseDetail, t: (key: MessageKey) => string): string {
  const decision = detail.evidence.find((item) => item.event_type === 'policy_decision');
  const blocked = decision?.event.blocked === true || detail.blocked;
  return blocked ? t('audit.case.blocked') : t('audit.case.notBlocked');
}

function evidenceSummary(item: SecurityEvidenceEvent, t: (key: MessageKey) => string): string {
  switch (item.event_type) {
    case 'file_action':
      return String(item.event.path ?? item.event.operation ?? t('audit.evidence.fileFallback'));
    case 'taint_transition':
      return `${String(item.event.label ?? 'SENSITIVE')} · ${String(item.event.transition ?? 'add')}`;
    case 'network_action':
      return String(item.event.destination ?? t('audit.evidence.networkFallback'));
    case 'policy_decision':
      return String(item.event.reason ?? item.event.mode ?? t('audit.evidence.policyFallback'));
    default:
      return String(item.event.message ?? t('audit.evidence.systemEventFallback'));
  }
}

function containmentEligible(riskCase: SecurityRiskCase): boolean {
  return (riskCase.severity === 'high' || riskCase.severity === 'critical')
    && riskCase.status !== 'false_positive'
    && riskCase.status !== 'accepted_risk'
    && riskCase.status !== 'resolved';
}

const StatCard: React.FC<{ labelKey: MessageKey; value: React.ReactNode; hintKey: MessageKey }> = ({
  labelKey,
  value,
  hintKey,
}) => {
  const { t } = useI18n();
  return (
    <div className="rounded-xl border border-gray-200 bg-white p-4 shadow-sm">
      <p className="text-sm text-gray-500">{t(labelKey)}</p>
      <p className="mt-2 text-2xl font-semibold text-gray-900">{value}</p>
      <p className="mt-1 text-xs text-gray-400">{t(hintKey)}</p>
    </div>
  );
};

const Pagination: React.FC<{
  offset: number;
  pageSize: number;
  total: number;
  loading: boolean;
  onChange: (offset: number) => void;
}> = ({ offset, pageSize, total, loading, onChange }) => {
  const page = Math.floor(offset / pageSize) + 1;
  const pageCount = Math.max(1, Math.ceil(total / pageSize));
  const { t } = useI18n();
  return (
    <div className="flex items-center justify-between border-t border-gray-200 px-5 py-3 text-xs text-gray-500">
      <span>{offset + 1}–{Math.min(offset + pageSize, total)} / {total}</span>
      <div className="flex items-center gap-2">
        <button
          type="button"
          disabled={offset === 0 || loading}
          onClick={() => onChange(Math.max(0, offset - pageSize))}
          className="rounded border px-2 py-1 disabled:text-gray-300"
        >
          {t('common.prev')}
        </button>
        <span>{page}/{pageCount}</span>
        <button
          type="button"
          disabled={offset + pageSize >= total || loading}
          onClick={() => onChange(offset + pageSize)}
          className="rounded border px-2 py-1 disabled:text-gray-300"
        >
          {t('common.next')}
        </button>
      </div>
    </div>
  );
};

export const SystemAuditPage: React.FC = () => {
  const navigate = useNavigate();
  const { t } = useI18n();
  const localeTag = useLocaleTag();
  const [activeTab, setActiveTab] = useState<AuditTab>('overview');
  const [summary, setSummary] = useState<SecuritySummary | null>(null);
  const [cases, setCases] = useState<SecurityRiskCase[]>([]);
  const [caseTotal, setCaseTotal] = useState(0);
  const [caseOffset, setCaseOffset] = useState(0);
  const [sessions, setSessions] = useState<SecuritySessionSummary[]>([]);
  const [sessionTotal, setSessionTotal] = useState(0);
  const [sessionOffset, setSessionOffset] = useState(0);
  const [events, setEvents] = useState<SecurityEventRecord[]>([]);
  const [eventTotal, setEventTotal] = useState(0);
  const [eventOffset, setEventOffset] = useState(0);
  const [selectedCase, setSelectedCase] = useState<SecurityRiskCaseDetail | null>(null);
  const [loading, setLoading] = useState(false);
  const [detailLoading, setDetailLoading] = useState(false);
  const [error, setError] = useState('');
  const [reviewing, setReviewing] = useState(false);
  const [containmentDialogOpen, setContainmentDialogOpen] = useState(false);
  const caseRequestVersion = useRef(0);
  const reviewRequestVersion = useRef(0);
  const loadRequestVersion = useRef(0);

  const load = useCallback(async () => {
    loadRequestVersion.current += 1;
    const version = loadRequestVersion.current;
    setLoading(true);
    const results = await Promise.allSettled([
      fetchAuditSummary({ limit: 10 }),
      fetchSecurityCases({ limit: CASE_PAGE_SIZE, offset: caseOffset }),
      fetchAuditSessions({ limit: SESSION_PAGE_SIZE, offset: sessionOffset }),
      fetchAuditEvents({ limit: EVENT_PAGE_SIZE, offset: eventOffset, include_details: true }),
    ]);
    if (loadRequestVersion.current !== version) return;
    const failures = results.filter((result) => result.status === 'rejected');
    if (results[0].status === 'fulfilled') setSummary(results[0].value.data);
    if (results[1].status === 'fulfilled') {
      setCases(results[1].value.data.items);
      setCaseTotal(results[1].value.data.total);
    }
    if (results[2].status === 'fulfilled') {
      setSessions(results[2].value.data.items);
      setSessionTotal(results[2].value.data.total);
    }
    if (results[3].status === 'fulfilled') {
      setEvents(results[3].value.data.items);
      setEventTotal(results[3].value.data.total);
    }
    setError(failures.length ? errorText((failures[0] as PromiseRejectedResult).reason, t) : '');
    setLoading(false);
  }, [caseOffset, eventOffset, sessionOffset, t]);

  useEffect(() => {
    void load();
  }, [load]);

  const openCase = async (caseId: string) => {
    caseRequestVersion.current += 1;
    reviewRequestVersion.current += 1;
    const version = caseRequestVersion.current;
    setContainmentDialogOpen(false);
    setSelectedCase(null);
    setDetailLoading(true);
    setError('');
    setReviewing(false);
    try {
      const response = await fetchSecurityCase(caseId);
      if (caseRequestVersion.current !== version) return;
      setSelectedCase(response.data);
    } catch (nextError) {
      if (caseRequestVersion.current === version) setError(errorText(nextError, t));
    } finally {
      if (caseRequestVersion.current === version) setDetailLoading(false);
    }
  };

  const refresh = async () => {
    const selectedCaseId = selectedCase?.case_id;
    const version = caseRequestVersion.current;
    await load();
    if (selectedCaseId && caseRequestVersion.current === version) await openCase(selectedCaseId);
  };

  const handleContained = async (caseId: string) => {
    const version = caseRequestVersion.current;
    setContainmentDialogOpen(false);
    await load();
    if (caseRequestVersion.current === version) await openCase(caseId);
  };

  const review = async (
    status: 'confirmed' | 'false_positive' | 'accepted_risk' | 'resolved',
  ) => {
    if (!selectedCase) return;
    reviewRequestVersion.current += 1;
    const reviewVersion = reviewRequestVersion.current;
    const caseVersion = caseRequestVersion.current;
    const caseId = selectedCase.case_id;
    setReviewing(true);
    try {
      await reviewSecurityCase(caseId, status);
      if (
        reviewRequestVersion.current !== reviewVersion
        || caseRequestVersion.current !== caseVersion
      ) return;
      await load();
      if (caseRequestVersion.current === caseVersion) await openCase(caseId);
    } catch (nextError) {
      if (
        reviewRequestVersion.current === reviewVersion
        && caseRequestVersion.current === caseVersion
      ) setError(errorText(nextError, t));
    } finally {
      if (
        reviewRequestVersion.current === reviewVersion
        && caseRequestVersion.current === caseVersion
      ) setReviewing(false);
    }
  };

  const totalCases = summary?.risk_cases_total ?? caseTotal;
  const openCases = summary?.risk_cases_open ?? 0;
  const blockedCases = summary?.risk_cases_blocked ?? 0;
  const casePage = Math.floor(caseOffset / CASE_PAGE_SIZE) + 1;
  const casePageCount = Math.max(1, Math.ceil(caseTotal / CASE_PAGE_SIZE));
  const sortedEvidence = useMemo(() => (
    selectedCase ? [...selectedCase.evidence].sort((left, right) => (
      left.occurred_at_ns - right.occurred_at_ns
    )) : []
  ), [selectedCase]);

  return (
    <div className="mx-auto w-full max-w-screen-2xl space-y-5 p-6">
      <header className="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
        <div>
          <div className="flex items-center gap-3">
            <h1 className="text-2xl font-bold text-gray-900">{t('audit.title')}</h1>
            <span className="rounded-full bg-blue-100 px-2.5 py-1 text-xs font-semibold text-blue-700">
              {t('audit.badge.localData')}
            </span>
          </div>
          <p className="mt-1 text-sm text-gray-500">
            {t('audit.description')}
          </p>
        </div>
        <div className="flex gap-2">
          <button
            type="button"
            onClick={() => navigate('/security')}
            className="rounded-lg border border-gray-300 bg-white px-4 py-2 text-sm text-gray-700"
          >
            {t('audit.button.viewRawEvents')}
          </button>
          <button
            type="button"
            onClick={() => void refresh()}
            disabled={loading}
            className="rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white disabled:bg-blue-300"
          >
            {loading ? t('common.loading') : t('common.refresh')}
          </button>
        </div>
      </header>

      {error && (
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      <section className="grid grid-cols-2 gap-4 lg:grid-cols-5">
        <StatCard labelKey="audit.stats.totalEvents.label" value={summary?.total ?? 0} hintKey="audit.stats.totalEvents.hint" />
        <StatCard labelKey="audit.stats.sessions.label" value={summary?.affected_sessions ?? sessions.length} hintKey="audit.stats.sessions.hint" />
        <StatCard labelKey="audit.stats.cases.label" value={totalCases} hintKey="audit.stats.cases.hint" />
        <StatCard labelKey="audit.stats.open.label" value={openCases} hintKey="audit.stats.open.hint" />
        <StatCard labelKey="audit.stats.blocked.label" value={blockedCases} hintKey="audit.stats.blocked.hint" />
      </section>

      <div className="flex gap-1 rounded-xl border border-gray-200 bg-white p-1 shadow-sm">
        {tabs.map((tab) => (
          <button
            key={tab.key}
            type="button"
            onClick={() => setActiveTab(tab.key)}
            className={`rounded-lg px-4 py-2 text-sm font-medium ${
              activeTab === tab.key ? 'bg-blue-600 text-white' : 'text-gray-600 hover:bg-gray-50'
            }`}
          >
            {tab.labelKey ? t(tab.labelKey) : tab.key}
          </button>
        ))}
      </div>

      {(activeTab === 'overview' || activeTab === 'cases') && (
        <section className="grid min-h-[520px] grid-cols-1 gap-5 xl:grid-cols-[minmax(360px,0.85fr)_minmax(0,1.65fr)]">
          <div className="overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm">
            <div className="border-b border-gray-200 px-5 py-4">
              <h2 className="font-semibold text-gray-900">{t('audit.tab.cases')}</h2>
              <p className="mt-1 text-xs text-gray-500">{t('audit.cases.description')}</p>
            </div>
            <div className="divide-y divide-gray-100">
              {cases.length === 0 ? (
                <p className="px-5 py-12 text-center text-sm text-gray-400">{t('audit.cases.empty')}</p>
              ) : cases.map((item) => (
                <button
                  key={item.case_id}
                  type="button"
                  onClick={() => void openCase(item.case_id)}
                  className={`block w-full px-5 py-4 text-left hover:bg-gray-50 ${
                    selectedCase?.case_id === item.case_id ? 'bg-blue-50' : ''
                  }`}
                >
                  <div className="flex items-start justify-between gap-3">
                    <div>
                      <p className="font-medium text-gray-900">{item.summary}</p>
                      <p className="mt-1 text-xs text-gray-500">
                        {item.agent_id} · {item.session_id || t('audit.case.noSession')} · {formatTime(item.updated_at_ns, localeTag)}
                      </p>
                    </div>
                    <span className={`rounded-full px-2 py-1 text-xs font-semibold ${severityStyle[item.severity]}`}>
                      {item.risk_score}
                    </span>
                  </div>
                  <div className="mt-3 flex items-center gap-2 text-xs">
                    <span className="rounded bg-gray-100 px-2 py-1 text-gray-600">{statusLabel[item.status] ? t(statusLabel[item.status]) : item.status}</span>
                    <span className={item.blocked ? 'text-red-600' : 'text-amber-600'}>
                      {item.blocked ? t('audit.case.blocked') : t('audit.case.notBlocked')}
                    </span>
                  </div>
                </button>
              ))}
            </div>
            {caseTotal > CASE_PAGE_SIZE && (
              <div className="flex items-center justify-between border-t border-gray-200 px-5 py-3 text-xs text-gray-500">
                <span>{caseOffset + 1}–{Math.min(caseOffset + cases.length, caseTotal)} / {caseTotal}</span>
                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    disabled={caseOffset === 0 || loading}
                    onClick={() => setCaseOffset((current) => Math.max(0, current - CASE_PAGE_SIZE))}
                    className="rounded border px-2 py-1 disabled:text-gray-300"
                  >
                    {t('common.prev')}
                  </button>
                  <span>{casePage}/{casePageCount}</span>
                  <button
                    type="button"
                    disabled={caseOffset + CASE_PAGE_SIZE >= caseTotal || loading}
                    onClick={() => setCaseOffset((current) => current + CASE_PAGE_SIZE)}
                    className="rounded border px-2 py-1 disabled:text-gray-300"
                  >
                    {t('common.next')}
                  </button>
                </div>
              </div>
            )}
          </div>

          <div className="rounded-xl border border-gray-200 bg-white shadow-sm">
            {!selectedCase ? (
              <div className="flex h-full min-h-[420px] items-center justify-center text-sm text-gray-400">
                {detailLoading ? t('audit.evidence.loading') : t('audit.evidence.selectCasePlaceholder')}
              </div>
            ) : (
              <div>
                <div className="border-b border-gray-200 p-5">
                  <div className="flex flex-wrap items-start justify-between gap-4">
                    <div>
                      <p className="text-xs font-medium text-gray-500">{t('audit.case.summaryTitle')}</p>
                      <h2 className="mt-1 text-xl font-semibold text-gray-900">{selectedCase.summary}</h2>
                      <p className="mt-2 text-sm text-gray-600">
                        {t('audit.case.summaryLine', { decision: decisionText(selectedCase, t), riskScore: selectedCase.risk_score, policyRevision: selectedCase.policy_revision })}
                      </p>
                    </div>
                    <button
                      type="button"
                      disabled={reviewing || selectedCase.status === 'confirmed'}
                      onClick={() => void review('confirmed')}
                      className="rounded-lg bg-red-600 px-4 py-2 text-sm font-medium text-white disabled:bg-gray-300"
                    >
                      {t('audit.button.confirmRisk')}
                    </button>
                  </div>
                  <div className="mt-4 flex flex-wrap gap-2 text-xs">
                    <button type="button" disabled={reviewing} onClick={() => void review('false_positive')} className="rounded border px-3 py-1.5 text-gray-600 disabled:opacity-40">{t('audit.button.markFalsePositive')}</button>
                    <button type="button" disabled={reviewing} onClick={() => void review('accepted_risk')} className="rounded border px-3 py-1.5 text-gray-600 disabled:opacity-40">{t('audit.button.acceptRisk')}</button>
                    {!selectedCase.containment && (
                      <button type="button" disabled={reviewing} onClick={() => void review('resolved')} className="rounded border px-3 py-1.5 text-gray-600 disabled:opacity-40">{t('audit.button.markResolved')}</button>
                    )}
                    <button type="button" onClick={() => navigate('/enforcement')} className="rounded border border-blue-200 px-3 py-1.5 text-blue-700">{t('nav.riskEnforcement')}</button>
                  </div>
                  {(containmentEligible(selectedCase) || selectedCase.containment) && (
                    <ContainmentLifecycleCard
                      action={selectedCase.containment}
                      loading={false}
                      error={false}
                      canUpgrade={containmentEligible(selectedCase)}
                      reviewing={reviewing}
                      onUpgrade={() => setContainmentDialogOpen(true)}
                      onResolve={() => void review('resolved')}
                    />
                  )}
                </div>
                <div className="p-5">
                  <h3 className="font-semibold text-gray-900">{t('audit.evidence.fullChainTitle')}</h3>
                  <div className="mt-4 space-y-1">
                    {sortedEvidence.map((item, index) => (
                      <div key={item.event_id} className="relative flex gap-4 pb-5">
                        {index < sortedEvidence.length - 1 && <span className="absolute left-[11px] top-6 h-full w-px bg-gray-200" />}
                        <span className={`relative mt-1 h-6 w-6 shrink-0 rounded-full border-4 border-white ${
                          item.event_type === 'policy_decision' ? 'bg-red-500' : 'bg-blue-500'
                        }`} />
                        <div className="min-w-0 flex-1 rounded-lg border border-gray-200 px-4 py-3">
                          <div className="flex items-center justify-between gap-3">
                            <p className="font-medium text-gray-900">{eventLabel[item.event_type] ? t(eventLabel[item.event_type]) : item.event_type}</p>
                            <span className="text-xs text-gray-400">PID {item.identity.pid}</span>
                          </div>
                          <p className="mt-1 break-all text-sm text-gray-600">{evidenceSummary(item, t)}</p>
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            )}
          </div>
        </section>
      )}

      {activeTab === 'sessions' && (
        <section className="overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm">
          <div className="border-b px-5 py-4">
            <h2 className="font-semibold">{t('audit.tab.sessions')}</h2>
          </div>
          <table className="w-full text-left text-sm">
            <thead className="bg-gray-50 text-xs text-gray-500">
              <tr>
                <th className="px-5 py-3">{t('common.session')}</th>
                <th>{t('audit.sessions.systemEvents')}</th>
                <th>{t('audit.sessions.timeRange')}</th>
                <th>{t('audit.sessions.entryPoint')}</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {sessions.map((session) => (
                <tr key={session.session_id}>
                  <td className="px-5 py-4 font-mono text-xs">{session.session_id}</td>
                  <td>{session.security_event_count ?? 0}</td>
                  <td>{formatTime(session.first_seen_ns ?? 0, localeTag)} ～ {formatTime(session.last_seen_ns ?? 0, localeTag)}</td>
                  <td><button type="button" onClick={() => navigate('/security')} className="text-blue-600">{t('audit.sessions.viewTimeline')}</button></td>
                </tr>
              ))}
            </tbody>
          </table>
          {sessionTotal > SESSION_PAGE_SIZE && (
            <Pagination
              offset={sessionOffset}
              pageSize={SESSION_PAGE_SIZE}
              total={sessionTotal}
              loading={loading}
              onChange={setSessionOffset}
            />
          )}
        </section>
      )}

      {activeTab === 'events' && (
        <section className="overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm">
          <div className="flex items-center justify-between border-b px-5 py-4">
            <h2 className="font-semibold">{t('audit.tab.events')}</h2>
            <button type="button" onClick={() => navigate('/security')} className="text-sm text-blue-600">{t('nav.securityObservability')}</button>
          </div>
          <table className="w-full text-left text-sm">
            <thead className="bg-gray-50 text-xs text-gray-500">
              <tr>
                <th className="px-5 py-3">{t('sec.eventType')}</th>
                <th>{t('common.session')}</th>
                <th>PID</th>
                <th>{t('audit.events.result')}</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {events.map((event) => (
                <tr key={event.event_id}>
                  <td className="px-5 py-4">{eventLabel[String(event.event_type)] ? t(eventLabel[String(event.event_type)]) : (event.event_type || t('audit.event.fallback'))}</td>
                  <td className="font-mono text-xs">{event.session_id || '—'}</td>
                  <td>{event.pid ?? '—'}</td>
                  <td>{event.result || '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {eventTotal > EVENT_PAGE_SIZE && (
            <Pagination
              offset={eventOffset}
              pageSize={EVENT_PAGE_SIZE}
              total={eventTotal}
              loading={loading}
              onChange={setEventOffset}
            />
          )}
        </section>
      )}

      {selectedCase && (
        <ContainmentDialog
          caseId={selectedCase.case_id}
          open={containmentDialogOpen}
          onClose={() => setContainmentDialogOpen(false)}
          onContained={() => void handleContained(selectedCase.case_id)}
        />
      )}
    </div>
  );
};
