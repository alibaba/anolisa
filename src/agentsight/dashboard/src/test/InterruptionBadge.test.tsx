import React from 'react';
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { InterruptionBadge } from '../components/InterruptionBadge';

describe('InterruptionBadge', () => {
  describe('simple mode', () => {
    it('should return null when count is 0', () => {
      const { container } = render(<InterruptionBadge count={0} severity="high" />);
      expect(container.innerHTML).toBe('');
    });

    it('should return null when count is undefined', () => {
      const { container } = render(<InterruptionBadge />);
      expect(container.innerHTML).toBe('');
    });

    it('should render count and severity label', () => {
      render(<InterruptionBadge count={3} severity="high" />);
      expect(screen.getAllByText('3 高危').length).toBeGreaterThan(0);
    });

    it('should render critical severity', () => {
      render(<InterruptionBadge count={1} severity="critical" />);
      expect(screen.getAllByText('1 严重').length).toBeGreaterThan(0);
    });

    it('should render medium severity by default', () => {
      render(<InterruptionBadge count={2} />);
      expect(screen.getAllByText('2 中危').length).toBeGreaterThan(0);
    });

    it('should render low severity', () => {
      render(<InterruptionBadge count={5} severity="low" />);
      expect(screen.getAllByText('5 低危').length).toBeGreaterThan(0);
    });

    it('should call onClick when clicked', () => {
      const onClick = vi.fn();
      const { container } = render(<InterruptionBadge count={1} severity="high" onClick={onClick} />);
      const badge = container.querySelector('span.cursor-pointer')!;
      fireEvent.click(badge);
      expect(onClick).toHaveBeenCalledTimes(1);
    });
  });

  describe('detailed mode (bySeverity)', () => {
    it('should render badges for non-zero severities', () => {
      render(
        <InterruptionBadge
          bySeverity={{ critical: 1, high: 2, medium: 0, low: 0 }}
        />
      );
      expect(screen.getAllByText('1 严重').length).toBeGreaterThan(0);
      expect(screen.getAllByText('2 高危').length).toBeGreaterThan(0);
    });

    it('should return null when all severities are zero', () => {
      const { container } = render(
        <InterruptionBadge
          bySeverity={{ critical: 0, high: 0, medium: 0, low: 0 }}
        />
      );
      expect(container.innerHTML).toBe('');
    });

    it('should render with types tooltip data', () => {
      render(
        <InterruptionBadge
          bySeverity={{ critical: 0, high: 3, medium: 0, low: 0 }}
          types={[
            { interruption_type: 'llm_error', severity: 'high', count: 2 },
            { interruption_type: 'sse_truncated', severity: 'high', count: 1 },
          ]}
        />
      );
      expect(screen.getAllByText('3 高危').length).toBeGreaterThan(0);
    });

    it('should call onClick on detailed badges', () => {
      const onClick = vi.fn();
      const { container } = render(
        <InterruptionBadge
          bySeverity={{ critical: 0, high: 1, medium: 0, low: 0 }}
          onClick={onClick}
        />
      );
      const badge = container.querySelector('span.cursor-pointer')!;
      fireEvent.click(badge);
      expect(onClick).toHaveBeenCalledTimes(1);
    });
  });
});
