import React from 'react';
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { EvaluationBadge } from '../components/EvaluationBadge';

describe('EvaluationBadge', () => {
  it('renders nothing when result is null', () => {
    const { container } = render(<EvaluationBadge result={null} />);
    expect(container.innerHTML).toBe('');
  });

  it('renders pass verdict with score', () => {
    render(<EvaluationBadge result={{ verdict: 'pass', score: 0.93 } as any} />);
    expect(screen.getByText('通过')).toBeInTheDocument();
    expect(screen.getByText('93')).toBeInTheDocument();
  });

  it('renders warn verdict', () => {
    render(<EvaluationBadge result={{ verdict: 'warn', score: 0.62 } as any} />);
    expect(screen.getByText('需复核')).toBeInTheDocument();
  });

  it('renders fail verdict', () => {
    render(<EvaluationBadge result={{ verdict: 'fail', score: 0.2 } as any} />);
    expect(screen.getByText('未通过')).toBeInTheDocument();
  });
});
