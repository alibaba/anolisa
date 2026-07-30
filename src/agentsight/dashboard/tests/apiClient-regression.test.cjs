const assert = require('node:assert/strict');
const test = require('node:test');

const {
  enforcementSupportsContainment,
  enforcementSupportsMode,
  enforcementViolationTotal,
  fetchContainmentPlan,
  fetchSecurityCase,
  fetchSecurityStatus,
} = require(process.env.AGENTSIGHT_API_CLIENT_BUILD);

function securityCaseDetail() {
  return {
    case_id: 'case-1',
    policy_id: 'credential-exfiltration',
    policy_revision: 1,
    agent_id: 'agent-1',
    severity: 'high',
    risk_score: 99,
    status: 'open',
    blocked: false,
    opened_at_ns: 1,
    updated_at_ns: 1,
    summary: 'fixture',
    evidence: [],
    containment: null,
  };
}

test('fetchSecurityCase rejects a non-2xx state envelope before returning it', async () => {
  global.fetch = async () => new Response(JSON.stringify({
    state: 'missing',
    data: { case_id: 'case-1' },
  }), { status: 404, statusText: 'Not Found' });

  await assert.rejects(
    () => fetchSecurityCase('case-1'),
    (error) => error?.status === 404 && error?.code === 'security_api_error',
  );
});

test('fetchSecurityStatus preserves a non-2xx availability state envelope', async () => {
  global.fetch = async () => new Response(JSON.stringify({
    state: 'daemon_unreachable',
    data: { error: 'socket unavailable' },
    meta: { source: 'agentsight' },
  }), { status: 503, statusText: 'Service Unavailable' });

  const response = await fetchSecurityStatus();

  assert.equal(response.state, 'daemon_unreachable');
  assert.deepEqual(response.data, { error: 'socket unavailable' });
});

test('fetchSecurityCase accepts a valid system-audit detail response', async () => {
  global.fetch = async () => new Response(JSON.stringify({
    state: 'ok',
    data: securityCaseDetail(),
  }), { status: 200 });

  const response = await fetchSecurityCase('case-1');

  assert.equal(response.data.case_id, 'case-1');
  assert.deepEqual(response.data.evidence, []);
});

test('fetchContainmentPlan rejects a non-2xx state envelope', async () => {
  global.fetch = async () => new Response(JSON.stringify({
    state: 'missing',
    data: { case_id: 'case-1' },
  }), { status: 404, statusText: 'Not Found' });

  await assert.rejects(
    () => fetchContainmentPlan('case-1'),
    (error) => error?.status === 404 && error?.code === 'security_api_error',
  );
});
test('fetchSecurityCase rejects a successful response whose detail shape is malformed', async () => {
  global.fetch = async () => new Response(JSON.stringify({
    state: 'ok',
    data: { ...securityCaseDetail(), evidence: 'not-an-array' },
  }), { status: 200 });

  await assert.rejects(
    () => fetchSecurityCase('case-1'),
    (error) => error?.status === 200 && error?.code === 'malformed_security_case',
  );
});

test('enforcement capabilities fail closed while the backend is not ready', () => {
  const health = {
    ready: false,
    backend: 'mock',
    capabilities: {
      credential_observe: true,
      credential_audit: true,
      credential_enforce: true,
      policy_handoff: true,
      alternate_pid_retarget: true,
      test_development: true,
    },
    message: 'private socket /run/agentsight/enforcer.sock is unavailable',
  };

  assert.equal(enforcementSupportsMode(health, 'enforce'), false);
  assert.equal(enforcementSupportsContainment(health), false);
});

test('audit-only backends report all observed violations instead of blocked-only rows', () => {
  const health = {
    ready: true,
    backend: 'actplane',
    capabilities: {
      credential_observe: true,
      credential_audit: true,
      credential_enforce: false,
      policy_handoff: false,
      alternate_pid_retarget: false,
      test_development: false,
    },
    message: null,
  };
  const violations = [{ blocked: false }, { blocked: false }, { blocked: true }];

  assert.equal(enforcementViolationTotal(violations, health), 3);
});
