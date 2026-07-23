import { describe, expect, it } from 'vitest';

import {
  securityEventVerdict,
  verdictBadgeClasses,
  verdictTone,
} from '../pages/security/utils';
import type { SecurityEventRecord } from '../utils/apiClient';

describe('security dashboard verdict utilities', () => {
  it('prefers a top-level verdict while preserving nested fallback support', () => {
    const projectedEvent: SecurityEventRecord = {
      event_id: 'skill-ledger-show',
      category: 'skill_ledger',
      command: 'show',
      skill_name: 'demo-skill',
      verdict: 'tampered',
      details: { verdict: 'pass' },
    };
    const legacyEvent: SecurityEventRecord = {
      event_id: 'legacy-event',
      details_preview: { verdict: 'warn' },
    };

    expect(securityEventVerdict(projectedEvent)).toBe('tampered');
    expect(securityEventVerdict(legacyEvent)).toBe('warn');
    expect(securityEventVerdict({ event_id: 'missing-verdict' })).toBe('-');
  });

  it.each([
    ['tampered', 'risk', 'bg-red-100'],
    ['drifted', 'warning', 'bg-amber-100'],
    ['unmanaged', 'warning', 'bg-amber-100'],
  ] as const)('classifies %s as %s', (verdict, tone, badgeClass) => {
    expect(verdictTone(verdict)).toBe(tone);
    expect(verdictBadgeClasses(verdict)).toContain(badgeClass);
  });
});
