import React from 'react';
import { NavLink, useLocation } from 'react-router-dom';
import { LanguageSwitcher, useI18n } from '../i18n';
import type { MessageKey } from '../i18n';
import type { AppCapability } from '../utils/apiClient';

type NavItem = {
  path: string;
  labelKey: MessageKey;
  icon: string;
  capability: AppCapability;
};

const navItems: NavItem[] = [
  { path: '/health', labelKey: 'nav.agentHealth', icon: '🩺', capability: 'agent_health' },
  { path: '/', labelKey: 'nav.agentObservability', icon: '📊', capability: 'agent_observability' },
  { path: '/sessions', labelKey: 'nav.sessions', icon: '🗂️', capability: 'sessions' },
  { path: '/savings', labelKey: 'nav.tokenSavings', icon: '⚡', capability: 'token_savings' },
  { path: '/optimization', labelKey: 'nav.optimization', icon: '🔬', capability: 'optimization' },
  { path: '/skills', labelKey: 'nav.skillMetrics', icon: '🧩', capability: 'skills' },
  { path: '/security', labelKey: 'nav.securityObservability', icon: '🛡️', capability: 'security' },
  { path: '/audit', labelKey: 'nav.systemAudit', icon: '📋', capability: 'system_audit' },
  { path: '/enforcement', labelKey: 'nav.riskEnforcement', icon: '⛔', capability: 'enforcement' },
  { path: '/atif', labelKey: 'nav.trajectoryViewer', icon: '🔍', capability: 'atif' },
  { path: '/settings', labelKey: 'nav.settings', icon: '⚙️', capability: 'settings' },
];

interface NavBarProps {
  capabilities?: AppCapability[];
}

export const NavBar: React.FC<NavBarProps> = ({ capabilities }) => {
  const location = useLocation();
  const { t } = useI18n();
  const visibleItems = Array.isArray(capabilities)
    ? navItems.filter((item) => capabilities.includes(item.capability))
    : navItems;

  return (
    <nav className="bg-white border-b border-gray-200 px-6 py-3">
      <div className="max-w-screen-2xl mx-auto flex flex-wrap items-center gap-3">
        {/* Logo / Brand */}
        <div className="flex items-center gap-2">
          <span className="text-xl font-bold text-gray-900">AgentSight</span>
          <span className="text-xs text-gray-400 px-2 py-0.5 bg-gray-100 rounded">v1.0</span>
        </div>

        {/* Navigation Links */}
        <div className="flex flex-1 flex-wrap items-center justify-end gap-1">
          {visibleItems.map((item) => {
            const isActive = item.path === '/' 
              ? location.pathname === '/' 
              : location.pathname.startsWith(item.path);
            
            return (
              <NavLink
                key={item.path}
                to={item.path}
                className={`px-4 py-2 rounded-lg text-sm font-medium transition-colors ${
                  isActive
                    ? 'bg-blue-100 text-blue-700'
                    : 'text-gray-600 hover:bg-gray-100 hover:text-gray-900'
                }`}
              >
                <span className="mr-1.5">{item.icon}</span>
                {t(item.labelKey)}
              </NavLink>
            );
          })}
          <LanguageSwitcher id="navbar-language" className="ml-2 shrink-0" />
        </div>
      </div>
    </nav>
  );
};
