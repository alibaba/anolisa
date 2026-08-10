import React, { useEffect, useState } from 'react';
import { HashRouter, Routes, Route, Navigate, useNavigate, useLocation } from 'react-router-dom';
import { NavBar } from './components/NavBar';
import { AgentHealthNotifier } from './components/AgentHealthNotifier';
import { ConversationList } from './pages/ConversationList';
import { AgentHealthPage } from './pages/AgentHealthPage';
import { AtifViewerPage } from './pages/AtifViewerPage';
import { TokenSavingsPage } from './pages/TokenSavingsPage';
import { SkillMetricsPage } from './pages/SkillMetricsPage';
import { SecurityObservabilityPage } from './pages/SecurityObservabilityPage';
import { OptimizationPage } from './pages/OptimizationPage';
import { AgentSessionsPage } from './pages/AgentSessionsPage';
import { SettingsPage } from './pages/SettingsPage';
import { RiskEnforcementPage } from './pages/RiskEnforcementPage';
import { SystemAuditPage } from './pages/SystemAuditPage';
import { LoginPage } from './pages/LoginPage';
import { useI18n } from './i18n';
import { fetchAuthStatus, fetchAuthVerify, login } from './utils/apiClient';
import type { AppCapability, AuthStatusResponse } from './utils/apiClient';

const DEFAULT_CAPABILITIES: AppCapability[] = [
  'agent_observability',
  'sessions',
  'token_savings',
  'optimization',
  'skills',
  'security',
  'enforcement',
  'atif',
  'settings',
  'agent_health',
];

const LOCAL_DEFAULT_PATH = '/sessions';

function capabilitiesFromStatus(status: AuthStatusResponse | null): AppCapability[] {
  return Array.isArray(status?.capabilities) ? status.capabilities : DEFAULT_CAPABILITIES;
}

function pathAllowed(pathname: string, capabilities: AppCapability[]): boolean {
  if (pathname === '/') return capabilities.includes('agent_observability');
  if (pathname.startsWith('/sessions')) return capabilities.includes('sessions');
  if (pathname.startsWith('/savings')) return capabilities.includes('token_savings');
  if (pathname.startsWith('/optimization')) return capabilities.includes('optimization');
  if (pathname.startsWith('/skills')) return capabilities.includes('skills');
  if (pathname.startsWith('/security')) return capabilities.includes('security');
  if (pathname.startsWith('/audit')) return capabilities.includes('security');
  if (pathname.startsWith('/enforcement')) return capabilities.includes('enforcement');
  if (pathname.startsWith('/atif')) return capabilities.includes('atif');
  if (pathname.startsWith('/settings')) return capabilities.includes('settings');
  if (pathname.startsWith('/health')) return capabilities.includes('agent_health');
  return true;
}

function defaultPath(capabilities: AppCapability[]): string {
  return capabilities.includes('agent_observability') ? '/' : LOCAL_DEFAULT_PATH;
}

/** Auth gate: checks auth status and renders LoginPage when needed. */
const AuthGate: React.FC<{ children: (status: AuthStatusResponse | null) => React.ReactNode }> = ({ children }) => {
  const [authState, setAuthState] = useState<'loading' | 'authenticated' | 'unauthenticated' | 'disabled'>('loading');
  const [status, setStatus] = useState<AuthStatusResponse | null>(null);
  const navigate = useNavigate();
  const location = useLocation();
  const { t } = useI18n();

  useEffect(() => {
    // Check for token in URL query parameter (e.g. ?token=xxx from CLI link)
    const urlParams = new URLSearchParams(window.location.search);
    const urlToken = urlParams.get('token');
    if (urlToken) {
      // Auto-login with the token from URL, then clean the URL
      (async () => {
        try {
          const ok = await login(urlToken);
          // Remove token from URL regardless of success
          urlParams.delete('token');
          const cleanSearch = urlParams.toString();
          const cleanUrl = window.location.pathname
            + (cleanSearch ? '?' + cleanSearch : '')
            + window.location.hash;
          window.history.replaceState(null, '', cleanUrl);
          if (ok) {
            setAuthState('authenticated');
          } else {
            setAuthState('unauthenticated');
          }
        } catch {
          setAuthState('unauthenticated');
        }
      })();
      return;
    }

    // Always allow the login page itself
    if (location.pathname === '/login') {
      setAuthState('unauthenticated');
      return;
    }

    (async () => {
      try {
        const nextStatus = await fetchAuthStatus();
        setStatus(nextStatus);
        if (!nextStatus.auth_enabled) {
          setAuthState('disabled');
          return;
        }
        const verify = await fetchAuthVerify();
        if (verify.authenticated) {
          setAuthState('authenticated');
        } else {
          setAuthState('unauthenticated');
        }
      } catch {
        // Server unreachable — skip auth check and assume full capabilities for compatibility.
        setStatus(null);
        setAuthState('disabled');
      }
    })();
  }, [location.pathname]);

  const handleAuthenticated = () => {
    setAuthState('authenticated');
    navigate(defaultPath(capabilitiesFromStatus(status)));
  };

  if (authState === 'loading') {
    return (
      <div className="min-h-screen flex items-center justify-center text-gray-400">
        {t('app.loading')}
      </div>
    );
  }

  if (authState === 'unauthenticated') {
    return <LoginPage onAuthenticated={handleAuthenticated} />;
  }

  return <>{children(status)}</>;
};

const AppShell: React.FC<{ status: AuthStatusResponse | null }> = ({ status }) => {
  const location = useLocation();
  const capabilities = capabilitiesFromStatus(status);
  const fallbackPath = defaultPath(capabilities);

  if (!pathAllowed(location.pathname, capabilities)) {
    return <Navigate to={fallbackPath} replace />;
  }

  return (
    <div className="min-h-screen bg-gray-50 flex flex-col">
      <NavBar capabilities={capabilities} />
      <div className="flex flex-1 overflow-hidden">
        <main className="flex-1 overflow-auto">
          <Routes>
            <Route
              path="/"
              element={
                capabilities.includes('agent_observability')
                  ? <ConversationList />
                  : <Navigate to={fallbackPath} replace />
              }
            />
            <Route path="/sessions" element={<AgentSessionsPage />} />
            <Route path="/savings" element={<TokenSavingsPage />} />
            <Route path="/optimization" element={<OptimizationPage />} />
            <Route path="/optimization/:sessionId" element={<OptimizationPage />} />
            <Route path="/skills" element={<SkillMetricsPage />} />
            <Route path="/security" element={<SecurityObservabilityPage />} />
            <Route path="/audit" element={<SystemAuditPage />} />
            <Route path="/enforcement" element={<RiskEnforcementPage />} />
            <Route path="/atif" element={<AtifViewerPage />} />
            <Route path="/settings" element={<SettingsPage />} />
            <Route path="/health" element={<AgentHealthPage />} />
            <Route path="*" element={<Navigate to={fallbackPath} replace />} />
          </Routes>
        </main>
        {capabilities.includes('agent_health') && <AgentHealthNotifier />}
      </div>
    </div>
  );
};

const App: React.FC = () => {
  return (
    <HashRouter>
      <Routes>
        <Route path="/login" element={<LoginPage onAuthenticated={() => { window.location.hash = '#/'; window.location.reload(); }} />} />
        <Route path="/*" element={
          <AuthGate>
            {(status) => <AppShell status={status} />}
          </AuthGate>
        } />
      </Routes>
    </HashRouter>
  );
};

export default App;
