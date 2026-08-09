const assert = require('node:assert/strict');
const test = require('node:test');

const {
  atifAgentContentScore,
  atifAgentCoverage,
  pickRicherAtifDoc,
  loadSessionAtifDoc,
  loadSessionAtifSources,
  docForSource,
  SOURCE_LABEL,
} = require(process.env.AGENTSIGHT_ATIF_SOURCE_BUILD);

/** Shape of the eBPF export in an environment where only the request side is
 *  captured: system prompt + per-call user step, but every agent step is bare.
 *  Mirrors /api/export/atif/session/13e57387-… (239 steps, 0 agent payloads). */
function contentFreeExport() {
  return {
    schema_version: 'ATIF-v1.7',
    session_id: 'sess-1',
    agent: { name: 'Claude', version: '1.0.0' },
    final_metrics: { total_prompt_tokens: 272, total_completion_tokens: 1, total_steps: 5 },
    steps: [
      { step_id: 1, source: 'system', message: 'You are Claude Code…' },
      { step_id: 2, source: 'user', message: 'real question' },
      { step_id: 3, source: 'agent', message: '', metrics: {}, extra: { start_timestamp: 'x' } },
      // Later "rounds" are system-prompt fragments misread as user turns.
      { step_id: 4, source: 'user', message: '# Memory You have a persistent…' },
      { step_id: 5, source: 'agent', message: '', metrics: {} },
    ],
  };
}

/** Shape of the log-collected trajectory: real messages, tool calls, metrics. */
function contentRichCollected() {
  return {
    schema_version: 'ATIF-v1.7',
    session_id: 'sess-1',
    agent: { name: 'claude-code', version: '2.1.197' },
    final_metrics: { total_prompt_tokens: 37609, total_completion_tokens: 342187, total_steps: 3 },
    steps: [
      { step_id: 1, source: 'user', message: 'real question' },
      {
        step_id: 2,
        source: 'agent',
        message: 'let me look',
        tool_calls: [{ tool_call_id: 'c1', function_name: 'Bash', arguments: {} }],
        observation: { results: [{ source_call_id: 'c1', content: 'ok' }] },
        metrics: { prompt_tokens: 11088, completion_tokens: 234 },
      },
      { step_id: 3, source: 'agent', message: 'done', metrics: { prompt_tokens: 5, completion_tokens: 2 } },
    ],
  };
}

function notFound() {
  const err = new Error('not found');
  err.status = 404;
  return err;
}

test('atifAgentContentScore is zero when no agent step carries a payload', () => {
  assert.equal(atifAgentContentScore(contentFreeExport()), 0);
});

test('atifAgentContentScore counts agent steps that carry real payload', () => {
  assert.equal(atifAgentContentScore(contentRichCollected()), 2);
});

test('atifAgentContentScore tolerates missing/!array steps', () => {
  assert.equal(atifAgentContentScore(null), 0);
  assert.equal(atifAgentContentScore({}), 0);
  assert.equal(atifAgentContentScore({ steps: 'nope' }), 0);
});

test('pickRicherAtifDoc prefers the content-rich collected doc over an empty export', () => {
  const picked = pickRicherAtifDoc(contentFreeExport(), contentRichCollected());
  assert.equal(picked.agent.name, 'claude-code');
});

test('pickRicherAtifDoc keeps the export when it is at least as rich (wire metrics win ties)', () => {
  const rich = { ...contentFreeExport(), steps: contentRichCollected().steps };
  const picked = pickRicherAtifDoc(rich, contentRichCollected());
  assert.equal(picked.agent.name, 'Claude');
});

test('pickRicherAtifDoc falls back to whichever doc exists', () => {
  assert.equal(pickRicherAtifDoc(null, contentRichCollected()).agent.name, 'claude-code');
  assert.equal(pickRicherAtifDoc(contentFreeExport(), null).agent.name, 'Claude');
  assert.equal(pickRicherAtifDoc(null, null), null);
});

// ─── Agent metadata backfill ─────────────────────────────────────────────────
// The collector never records tool_definitions; the wire export does. Losing the
// count when the collected doc wins would regress the "Agent 信息" card.

test('pickRicherAtifDoc backfills tool_definitions the winning doc lacks', () => {
  const exported = contentFreeExport();
  exported.agent.tool_definitions = [{ name: 'Bash' }, { name: 'Read' }];
  const picked = pickRicherAtifDoc(exported, contentRichCollected());
  assert.equal(picked.agent.name, 'claude-code', 'collected still wins on content');
  assert.equal(picked.agent.tool_definitions.length, 2, 'but borrows the tool definitions');
});

test('pickRicherAtifDoc never overwrites metadata the winning doc already has', () => {
  const exported = contentFreeExport();
  exported.agent.tool_definitions = [{ name: 'Bash' }];
  exported.agent.model_name = 'wire-model';
  const collected = contentRichCollected();
  collected.agent.tool_definitions = [{ name: 'A' }, { name: 'B' }, { name: 'C' }];
  collected.agent.model_name = 'log-model';
  const picked = pickRicherAtifDoc(exported, collected);
  assert.equal(picked.agent.tool_definitions.length, 3);
  assert.equal(picked.agent.model_name, 'log-model');
});

test('pickRicherAtifDoc leaves the winning doc untouched when nothing to backfill', () => {
  const collected = contentRichCollected();
  const picked = pickRicherAtifDoc(contentFreeExport(), collected);
  assert.equal(picked.agent.tool_definitions, undefined);
});

test('pickRicherAtifDoc does not mutate the input documents', () => {
  const exported = contentFreeExport();
  exported.agent.tool_definitions = [{ name: 'Bash' }];
  const collected = contentRichCollected();
  pickRicherAtifDoc(exported, collected);
  assert.equal(collected.agent.tool_definitions, undefined, 'original collected doc unchanged');
});

// ─── loadSessionAtifDoc ──────────────────────────────────────────────────────

test('loadSessionAtifDoc returns the collected doc when the export is content-free', async () => {
  const doc = await loadSessionAtifDoc('sess-1', {
    fetchExported: async () => contentFreeExport(),
    fetchCollected: async () => contentRichCollected(),
  });
  assert.equal(doc.agent.name, 'claude-code');
  assert.equal(doc.steps.filter(s => s.source === 'user').length, 1);
});

test('loadSessionAtifDoc uses the export when no collected trajectory exists', async () => {
  const doc = await loadSessionAtifDoc('sess-1', {
    fetchExported: async () => contentFreeExport(),
    fetchCollected: async () => { throw notFound(); },
  });
  assert.equal(doc.agent.name, 'Claude');
});

test('loadSessionAtifDoc uses the collected doc when the export 404s', async () => {
  const doc = await loadSessionAtifDoc('sess-1', {
    fetchExported: async () => { throw notFound(); },
    fetchCollected: async () => contentRichCollected(),
  });
  assert.equal(doc.agent.name, 'claude-code');
});

test('loadSessionAtifDoc ignores a non-ATIF payload from either store', async () => {
  const doc = await loadSessionAtifDoc('sess-1', {
    fetchExported: async () => ({ error: 'nope' }),
    fetchCollected: async () => contentRichCollected(),
  });
  assert.equal(doc.agent.name, 'claude-code');
});

test('loadSessionAtifDoc throws a both-stores-missing message when neither has the session', async () => {
  await assert.rejects(
    loadSessionAtifDoc('sess-1', {
      fetchExported: async () => { throw notFound(); },
      fetchCollected: async () => { throw notFound(); },
    }),
    /既无 eBPF 捕获记录，也无采集轨迹/,
  );
});

test('loadSessionAtifDoc surfaces a non-404 export error instead of masking it', async () => {
  const boom = new Error('boom');
  boom.status = 500;
  await assert.rejects(
    loadSessionAtifDoc('sess-1', {
      fetchExported: async () => { throw boom; },
      fetchCollected: async () => contentRichCollected(),
    }),
    /boom/,
  );
});

test('loadSessionAtifDoc still succeeds when the collected store errors out', async () => {
  const doc = await loadSessionAtifDoc('sess-1', {
    fetchExported: async () => contentFreeExport(),
    fetchCollected: async () => { throw new Error('store offline'); },
  });
  assert.equal(doc.agent.name, 'Claude');
});

// ─── loadSessionAtifSources / docForSource ───────────────────────────────────
// The viewer keeps BOTH documents so the user can switch source without a
// refetch, and so an empty eBPF side stays visible as a capture-health signal
// instead of being silently swapped out.

const bothStores = () => ({
  fetchExported: async () => contentFreeExport(),
  fetchCollected: async () => contentRichCollected(),
});

test('loadSessionAtifSources returns both documents plus the content-based default', async () => {
  const res = await loadSessionAtifSources('sess-1', bothStores());
  assert.equal(res.defaultSource, 'log');
  assert.equal(res.ebpf.agent.name, 'Claude');
  assert.equal(res.log.agent.name, 'claude-code');
});

test('loadSessionAtifSources defaults to ebpf when it is at least as rich', async () => {
  const res = await loadSessionAtifSources('sess-1', {
    fetchExported: async () => ({ ...contentFreeExport(), steps: contentRichCollected().steps }),
    fetchCollected: async () => contentRichCollected(),
  });
  assert.equal(res.defaultSource, 'ebpf');
});

test('loadSessionAtifSources keeps the missing side null and defaults to the present one', async () => {
  const noLog = await loadSessionAtifSources('sess-1', {
    fetchExported: async () => contentFreeExport(),
    fetchCollected: async () => { throw notFound(); },
  });
  assert.equal(noLog.log, null);
  assert.equal(noLog.defaultSource, 'ebpf');

  const noEbpf = await loadSessionAtifSources('sess-1', {
    fetchExported: async () => { throw notFound(); },
    fetchCollected: async () => contentRichCollected(),
  });
  assert.equal(noEbpf.ebpf, null);
  assert.equal(noEbpf.defaultSource, 'log');
});

test('loadSessionAtifSources backfills tool_definitions onto the collected side', async () => {
  const exported = contentFreeExport();
  exported.agent.tool_definitions = [{ name: 'Bash' }, { name: 'Read' }];
  const res = await loadSessionAtifSources('sess-1', {
    fetchExported: async () => exported,
    fetchCollected: async () => contentRichCollected(),
  });
  assert.equal(res.log.agent.tool_definitions.length, 2);
});

test('loadSessionAtifSources throws when neither store has the session', async () => {
  await assert.rejects(
    loadSessionAtifSources('sess-1', {
      fetchExported: async () => { throw notFound(); },
      fetchCollected: async () => { throw notFound(); },
    }),
    /既无 eBPF 捕获记录，也无采集轨迹/,
  );
});

test('loadSessionAtifSources surfaces a non-404 export error', async () => {
  const boom = new Error('boom');
  boom.status = 500;
  await assert.rejects(
    loadSessionAtifSources('sess-1', {
      fetchExported: async () => { throw boom; },
      fetchCollected: async () => contentRichCollected(),
    }),
    /boom/,
  );
});

test('docForSource honours an explicit pick even when that side is content-free', async () => {
  const res = await loadSessionAtifSources('sess-1', bothStores());
  // The whole point of the switch: asking for eBPF shows eBPF, empty or not.
  assert.equal(docForSource(res, 'ebpf').agent.name, 'Claude');
  assert.equal(docForSource(res, 'log').agent.name, 'claude-code');
});

test('docForSource falls back to the default when the asked-for side is absent', async () => {
  const res = await loadSessionAtifSources('sess-1', {
    fetchExported: async () => { throw notFound(); },
    fetchCollected: async () => contentRichCollected(),
  });
  assert.equal(docForSource(res, 'ebpf').agent.name, 'claude-code');
  assert.equal(docForSource(res, null).agent.name, 'claude-code');
});

test('SOURCE_LABEL carries the shared eBPF/日志 vocabulary', () => {
  assert.equal(SOURCE_LABEL.ebpf, 'eBPF');
  assert.equal(SOURCE_LABEL.log, '日志');
});

test('loadSessionAtifSources default is unaffected by metadata backfill', async () => {
  // Regression: defaultSource used to be derived from pickRicherAtifDoc's object
  // identity, which backfillAgent breaks by returning a fresh object — so a
  // session whose collected side merely borrowed tool_definitions defaulted to
  // the empty eBPF side.
  const exported = contentFreeExport();
  exported.agent.tool_definitions = [{ name: 'Bash' }];
  const res = await loadSessionAtifSources('sess-1', {
    fetchExported: async () => exported,
    fetchCollected: async () => contentRichCollected(),
  });
  assert.equal(res.defaultSource, 'log');
  assert.equal(res.log.agent.tool_definitions.length, 1, 'still backfilled');
});

// ─── atifAgentCoverage ───────────────────────────────────────────────────────
// Drives the switcher's capture-health note. A near-empty capture must not read
// as healthy just because one step happened to carry metrics.

test('atifAgentCoverage reports payload-carrying agent steps out of the total', () => {
  assert.deepEqual(atifAgentCoverage(contentRichCollected()), { withPayload: 2, total: 2 });
  assert.deepEqual(atifAgentCoverage(contentFreeExport()), { withPayload: 0, total: 2 });
});

test('atifAgentCoverage exposes partial capture rather than rounding it to healthy', () => {
  // Mirrors session 13e57387: 235 agent steps, exactly 1 with real metrics.
  const doc = contentFreeExport();
  doc.steps.push({ step_id: 6, source: 'agent', message: '', metrics: { prompt_tokens: 272 } });
  const cov = atifAgentCoverage(doc);
  assert.deepEqual(cov, { withPayload: 1, total: 3 });
  assert.ok(cov.withPayload < cov.total, 'the gap is what the UI must surface');
});

test('atifAgentCoverage tolerates malformed documents', () => {
  assert.deepEqual(atifAgentCoverage(null), { withPayload: 0, total: 0 });
  assert.deepEqual(atifAgentCoverage({ steps: 'nope' }), { withPayload: 0, total: 0 });
});
