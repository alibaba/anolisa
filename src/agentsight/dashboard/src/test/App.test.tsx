import React from 'react';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';

// Mock heavy page components to avoid pulling in all their deps
vi.mock('../pages/ConversationList', () => ({
  ConversationList: () => <div data-testid="page-conversations">ConversationList</div>,
}));
vi.mock('../pages/TokenSavingsPage', () => ({
  TokenSavingsPage: () => <div data-testid="page-savings">TokenSavingsPage</div>,
}));
vi.mock('../pages/AtifViewerPage', () => ({
  AtifViewerPage: () => <div data-testid="page-atif">AtifViewerPage</div>,
}));
vi.mock('../pages/SecurityObservabilityPage', () => ({
  SecurityObservabilityPage: () => <div data-testid="page-security">SecurityObservabilityPage</div>,
}));
vi.mock('../pages/RiskEnforcementPage', () => ({
  RiskEnforcementPage: () => <div data-testid="page-enforcement">RiskEnforcementPage</div>,
}));
vi.mock('../pages/SystemAuditPage', () => ({
  SystemAuditPage: () => <div data-testid="page-system-audit">SystemAuditPage</div>,
}));
vi.mock('../components/AgentHealthSidebar', () => ({
  AgentHealthSidebar: () => <div data-testid="sidebar">Sidebar</div>,
}));

import App from '../App';

afterEach(() => {
  window.location.hash = '';
});

describe('App', () => {
  it('should render NavBar with brand', () => {
    render(
      <App />
    );
    expect(screen.getByText('AgentSight')).toBeInTheDocument();
  });

  it('should render ConversationList on root path', () => {
    render(<App />);
    expect(screen.getByTestId('page-conversations')).toBeInTheDocument();
  });

  it('should render AgentHealthSidebar', () => {
    render(<App />);
    expect(screen.getByTestId('sidebar')).toBeInTheDocument();
  });

  it('should render SecurityObservabilityPage on security path', () => {
    window.location.hash = '#/security';
    render(<App />);
    expect(screen.getByTestId('page-security')).toBeInTheDocument();
  });

  it('renders RiskEnforcementPage on enforcement path', async () => {
    window.location.hash = '#/enforcement';
    render(<App />);
    expect(await screen.findByTestId('page-enforcement')).toBeInTheDocument();
  });

  it('renders SystemAuditPage on audit path', async () => {
    window.location.hash = '#/audit';
    render(<App />);
    expect(await screen.findByTestId('page-system-audit')).toBeInTheDocument();
  });
});
