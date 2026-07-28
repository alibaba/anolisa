import React from 'react';
import { NavLink, useLocation } from 'react-router-dom';
import type { AppCapability } from '../utils/apiClient';

type NavItem = {
  path: string;
  label: string;
  icon: string;
  capability: AppCapability;
};

const navItems: NavItem[] = [
  { path: '/', label: 'Agent 可观测', icon: '📊', capability: 'agent_observability' },
  { path: '/sessions', label: '会话列表', icon: '🗂️', capability: 'sessions' },
  { path: '/savings', label: 'Token 节省', icon: '⚡', capability: 'token_savings' },
  { path: '/optimization', label: '优化分析', icon: '🔬', capability: 'optimization' },
  { path: '/skills', label: 'Skill 指标', icon: '🧩', capability: 'skills' },
  { path: '/security', label: '安全可观测', icon: '🛡️', capability: 'security' },
  { path: '/atif', label: '轨迹查看', icon: '🔍', capability: 'atif' },
  { path: '/settings', label: '设置', icon: '⚙️', capability: 'settings' },
];

interface NavBarProps {
  capabilities?: AppCapability[];
}

export const NavBar: React.FC<NavBarProps> = ({ capabilities }) => {
  const location = useLocation();
  const visibleItems = Array.isArray(capabilities)
    ? navItems.filter((item) => capabilities.includes(item.capability))
    : navItems;

  return (
    <nav className="bg-white border-b border-gray-200 px-6 py-3">
      <div className="max-w-screen-xl mx-auto flex items-center justify-between">
        {/* Logo / Brand */}
        <div className="flex items-center gap-2">
          <span className="text-xl font-bold text-gray-900">AgentSight</span>
          <span className="text-xs text-gray-400 px-2 py-0.5 bg-gray-100 rounded">v1.0</span>
        </div>

        {/* Navigation Links */}
        <div className="flex items-center gap-1">
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
                {item.label}
              </NavLink>
            );
          })}
        </div>
      </div>
    </nav>
  );
};
