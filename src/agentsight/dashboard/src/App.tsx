import React, { useEffect, useState } from 'react';
import { HashRouter, Routes, Route, useNavigate, useLocation } from 'react-router-dom';
import { NavBar } from './components/NavBar';
import { AgentHealthSidebar } from './components/AgentHealthSidebar';
import { ConversationList } from './pages/ConversationList';
import { AtifViewerPage } from './pages/AtifViewerPage';
import { TokenSavingsPage } from './pages/TokenSavingsPage';
import { SkillMetricsPage } from './pages/SkillMetricsPage';
import { SecurityObservabilityPage } from './pages/SecurityObservabilityPage';
import { OptimizationPage } from './pages/OptimizationPage';
import { LoginPage } from './pages/LoginPage';
import { fetchAuthStatus, fetchAuthVerify, login } from './utils/apiClient';

/** Auth gate: checks auth status and renders LoginPage when needed. */
const AuthGate: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [authState, setAuthState] = useState<'loading' | 'authenticated' | 'unauthenticated' | 'disabled'>('loading');
  const navigate = useNavigate();
  const location = useLocation();

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
        const status = await fetchAuthStatus();
        if (!status.auth_enabled) {
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
        // Server unreachable — skip auth check
        setAuthState('disabled');
      }
    })();
  }, [location.pathname]);

  const handleAuthenticated = () => {
    setAuthState('authenticated');
    navigate('/');
  };

  if (authState === 'loading') {
    return (
      <div className="min-h-screen flex items-center justify-center text-gray-400">
        Loading...
      </div>
    );
  }

  if (authState === 'unauthenticated') {
    return <LoginPage onAuthenticated={handleAuthenticated} />;
  }

  return <>{children}</>;
};

const App: React.FC = () => {
  return (
    <HashRouter>
      <Routes>
        <Route path="/login" element={<LoginPage onAuthenticated={() => { window.location.hash = '#/'; window.location.reload(); }} />} />
        <Route path="/*" element={
          <AuthGate>
            <div className="min-h-screen bg-gray-50 flex flex-col">
              <NavBar />
              <div className="flex flex-1 overflow-hidden">
                <main className="flex-1 overflow-auto">
                  <Routes>
                    <Route path="/" element={<ConversationList />} />
                    <Route path="/savings" element={<TokenSavingsPage />} />
                    <Route path="/optimization" element={<OptimizationPage />} />
                    <Route path="/optimization/:sessionId" element={<OptimizationPage />} />
                    <Route path="/skills" element={<SkillMetricsPage />} />
                    <Route path="/security" element={<SecurityObservabilityPage />} />
                    <Route path="/atif" element={<AtifViewerPage />} />
                  </Routes>
                </main>
                <AgentHealthSidebar />
              </div>
            </div>
          </AuthGate>
        } />
      </Routes>
    </HashRouter>
  );
};

export default App;
