import React from 'react';
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { NavBar } from '../components/NavBar';

describe('NavBar', () => {
  const renderNavBar = (path = '/') => {
    return render(
      <MemoryRouter initialEntries={[path]}>
        <NavBar />
      </MemoryRouter>
    );
  };

  it('should render brand name', () => {
    renderNavBar();
    expect(screen.getByText('AgentSight')).toBeInTheDocument();
  });

  it('should render version badge', () => {
    renderNavBar();
    expect(screen.getByText('v1.0')).toBeInTheDocument();
  });

  it('should render all navigation items', () => {
    renderNavBar();
    expect(screen.getByText('Agent 可观测')).toBeInTheDocument();
    expect(screen.getByText('Token 节省')).toBeInTheDocument();
    expect(screen.getByText('ATIF 查看器')).toBeInTheDocument();
  });

  it('should highlight active link for root path', () => {
    renderNavBar('/');
    const link = screen.getByText('Agent 可观测').closest('a');
    expect(link?.className).toContain('bg-blue-100');
  });

  it('should highlight active link for savings path', () => {
    renderNavBar('/savings');
    const link = screen.getByText('Token 节省').closest('a');
    expect(link?.className).toContain('bg-blue-100');
  });

  it('should not highlight inactive links', () => {
    renderNavBar('/');
    const link = screen.getByText('Token 节省').closest('a');
    expect(link?.className).not.toContain('bg-blue-100');
  });
});
