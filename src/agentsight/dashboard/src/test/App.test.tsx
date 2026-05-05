import React from 'react';
import { describe, it, expect, vi } from 'vitest';
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
vi.mock('../components/AgentHealthSidebar', () => ({
  AgentHealthSidebar: () => <div data-testid="sidebar">Sidebar</div>,
}));

import App from '../App';

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
});
