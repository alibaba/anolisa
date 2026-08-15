#!/usr/bin/env bash
# Exercise the native dsh bundle without requiring a dsh installation.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d /tmp/tokenless-dsh-adapter-test.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

make -C "$ROOT" stamp-adapter-templates >/dev/null
node --input-type=module - "$ROOT" "$TMP" <<'NODE'
import assert from 'node:assert/strict'
import {
  chmodSync,
  existsSync,
  readFileSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs'
import { join } from 'node:path'

const [root, tmp] = process.argv.slice(2)
const binary = join(tmp, 'tokenless')
const argsFile = join(tmp, 'argv.json')
const missingBinary = join(tmp, 'missing-tokenless')
writeFileSync(
  binary,
  '#!/usr/bin/env node\n' +
    'const { writeFileSync } = require("node:fs");\n' +
    'process.stdin.setEncoding("utf8"); let input = "";\n' +
    'process.stdin.on("data", chunk => input += chunk);\n' +
    'process.stdin.on("end", () => {\n' +
    '  if (process.env.TOKENLESS_TEST_ARGS) writeFileSync(process.env.TOKENLESS_TEST_ARGS, JSON.stringify(process.argv.slice(2)));\n' +
    '  if (process.env.TOKENLESS_TEST_MODE === "fail") process.exit(7);\n' +
    '  if (process.env.TOKENLESS_TEST_MODE === "timeout") { setTimeout(() => {}, 10000); return; }\n' +
    '  if (process.env.TOKENLESS_TEST_MODE === "same") process.stdout.write(input);\n' +
    '  else if (process.env.TOKENLESS_TEST_MODE === "invalid") process.stdout.write("{");\n' +
    '  else process.stdout.write("{\\"ok\\":true}");\n' +
    '});\n',
)
chmodSync(binary, 0o755)
process.env.TOKENLESS_TEST_ARGS = argsFile

const pluginPath = join(root, 'adapters/tokenless/dsh/dist/index.js')
const plugin = await import(`file://${pluginPath}`)
const cordisPatch = readFileSync(
  join(root, 'adapters/tokenless/dsh/cordis.patch.yml'),
  'utf8',
)
assert.match(cordisPatch, /name:\s+['"]@anolisa\/dsh-tokenless['"]/)
assert.doesNotMatch(cordisPatch, /name:\s+\.\/dist\/index\.js/)
function register(config) {
  let callback
  const ctx = {
    on(event, listener) {
      assert.equal(event, 'tools/post-execute')
      callback = listener
    },
  }
  plugin.apply(ctx, config)
  assert.equal(typeof callback, 'function')
  return callback
}

const listener = register({ tokenlessBin: binary })
assert.equal(plugin.name, 'anolisa-tokenless')
assert.deepEqual(plugin.inject, ['tools'])

const exec = {
  name: 'api_call',
  callId: 'call-1',
  signal: new AbortController().signal,
  agent: { id: 'session-1' },
}
const longText = '{"long":"this payload is intentionally long"}'
const result = (text = longText, value) => ({
  isError: false,
  ...(value === undefined ? {} : { value }),
  content: [{ type: 'text', text }],
})
const clearArgs = () => {
  if (existsSync(argsFile)) unlinkSync(argsFile)
}
const args = () => JSON.parse(readFileSync(argsFile, 'utf8'))
const downstreamContext = {
  id: 'downstream-context',
  role: 'user',
  content: [{ type: 'text', text: 'downstream policy context' }],
}

// A compression win must still run and compose the downstream waterfall.
process.env.TOKENLESS_TEST_MODE = 'compress'
clearArgs()
let nextCalled = false
const compressed = await listener(exec, result(), async () => {
  nextCalled = true
  return { kind: 'accept', additionalContexts: [downstreamContext] }
})
assert.equal(nextCalled, true)
assert.equal(compressed.kind, 'accept')
assert.deepEqual(compressed.content, [{ type: 'text', text: '{"ok":true}' }])
assert.deepEqual(compressed.additionalContexts, [downstreamContext])
assert.deepEqual(args(), [
  'compress-response',
  '--agent-id', 'dsh',
  '--truncate-strings-at', '1048576',
  '--truncate-arrays-at', '65536',
  '--max-depth', '32',
  '--session-id', 'session-1',
  '--tool-use-id', 'call-1',
])

// Downstream blocks and canonical-value replacements must pass through intact.
clearArgs()
const block = await listener(exec, result(), async () => ({
  kind: 'block',
  feedback: [{ type: 'text', text: 'policy blocked this result' }],
  additionalContexts: [downstreamContext],
}))
assert.deepEqual(block, {
  kind: 'block',
  feedback: [{ type: 'text', text: 'policy blocked this result' }],
  additionalContexts: [downstreamContext],
})
assert.equal(existsSync(argsFile), false)

const valueDecision = { kind: 'accept', value: { canonical: true }, additionalContexts: [downstreamContext] }
const valueResult = await listener(exec, result(), async () => valueDecision)
assert.strictEqual(valueResult, valueDecision)
assert.equal(existsSync(argsFile), false)

let mixedNextCalled = false
const mixed = await listener(exec, {
  isError: false,
  content: [
    { type: 'text', text: longText },
    { type: 'image', attachment: { id: 'image-1' } },
  ],
}, async () => {
  mixedNextCalled = true
  return { kind: 'accept' }
})
assert.equal(mixedNextCalled, true)
assert.deepEqual(mixed, { kind: 'accept' })

let abortedNextCalled = false
const aborted = await listener({
  ...exec,
  signal: AbortSignal.abort(),
}, result(), async () => {
  abortedNextCalled = true
  return { kind: 'accept' }
})
assert.equal(abortedNextCalled, true)
assert.deepEqual(aborted, { kind: 'accept' })

// DSH bash exposes failures in canonical result.value, not display content.
const bashExec = { ...exec, name: 'Bash' }
const canonicalFailureResult = {
  isError: false,
  value: {
    kind: 'foreground',
    exitCode: 1,
    timedOut: false,
    stdout: { text: '', truncated: false },
    stderr: { text: 'permission denied while opening protected file', truncated: false },
  },
  content: [{ type: 'text', text: '```console\ncat protected\n[exit code: 1]\n```' }],
}
const canonicalFailure = await listener(bashExec, canonicalFailureResult, async () => ({ kind: 'accept' }))
assert.equal(canonicalFailure.additionalContexts?.length, 1)
assert.match(canonicalFailure.additionalContexts[0].content[0].text, /ENV_PERMISSION/)

// A downstream canonical replacement is the final result. Attribution must
// describe that replacement, never the stale result seen before next().
clearArgs()
const recoveredDecision = {
  kind: 'accept',
  value: {
    kind: 'foreground',
    exitCode: 0,
    timedOut: false,
    stdout: { text: 'recovered', truncated: false },
    stderr: { text: '', truncated: false },
  },
  additionalContexts: [downstreamContext],
}
const recovered = await listener(bashExec, canonicalFailureResult, async () => recoveredDecision)
assert.strictEqual(recovered, recoveredDecision)
assert.equal(existsSync(argsFile), false)

const replacementFailureDecision = {
  kind: 'accept',
  value: {
    kind: 'foreground',
    exitCode: 1,
    timedOut: false,
    stdout: { text: '', truncated: false },
    stderr: { text: 'permission denied after replacement', truncated: false },
  },
  additionalContexts: [downstreamContext],
}
const replacementFailure = await listener(bashExec, result(), async () => replacementFailureDecision)
assert.strictEqual(replacementFailure.value, replacementFailureDecision.value)
assert.equal(replacementFailure.additionalContexts.length, 2)
assert.strictEqual(replacementFailure.additionalContexts[0], downstreamContext)
assert.match(replacementFailure.additionalContexts[1].content[0].text, /ENV_PERMISSION/)
assert.equal(existsSync(argsFile), false)

const zeroExit = await listener(bashExec, {
  isError: false,
  value: {
    kind: 'foreground',
    exitCode: 0,
    timedOut: false,
    stdout: { text: 'permission denied in searched documentation', truncated: false },
    stderr: { text: '', truncated: false },
  },
  content: [{ type: 'text', text: 'permission denied in searched documentation' }],
}, async () => ({ kind: 'accept' }))
assert.equal(zeroExit.additionalContexts, undefined)

const timedOut = await listener(bashExec, {
  isError: false,
  value: {
    kind: 'foreground',
    exitCode: 124,
    timedOut: true,
    stdout: { text: '', truncated: false },
    stderr: { text: 'connection timed out', truncated: false },
  },
  content: [{ type: 'text', text: '```console\nnetwork call\n```' }],
}, async () => ({ kind: 'accept' }))
assert.equal(timedOut.additionalContexts?.length, 1)
assert.match(timedOut.additionalContexts[0].content[0].text, /ENV_NETWORK/)

const unclassifiedTimeout = await listener(bashExec, {
  isError: false,
  value: {
    kind: 'foreground',
    exitCode: 124,
    timedOut: true,
    stdout: { text: '', truncated: false },
    stderr: { text: 'command stopped after timeout', truncated: false },
  },
  content: [{ type: 'text', text: 'command stopped after timeout' }],
}, async () => ({ kind: 'accept' }))
assert.equal(unclassifiedTimeout.additionalContexts, undefined)

const nonnumericExit = await listener(bashExec, {
  isError: false,
  value: {
    kind: 'foreground',
    exitCode: 'N/A',
    timedOut: false,
    stdout: { text: '', truncated: false },
    stderr: { text: 'permission denied appears in ordinary data', truncated: false },
  },
  content: [{ type: 'text', text: 'ordinary successful result' }],
}, async () => ({ kind: 'accept' }))
assert.equal(nonnumericExit.additionalContexts, undefined)

const errorObject = await listener(bashExec, {
  isError: false,
  value: {
    kind: 'foreground',
    exitCode: 1,
    timedOut: false,
    stderr: { text: '', truncated: false },
    error: { code: 'EACCES', errno: 13 },
  },
  content: [{ type: 'text', text: 'command failed' }],
}, async () => ({ kind: 'accept' }))
assert.match(errorObject.additionalContexts[0].content[0].text, /ENV_PERMISSION/)

const stdoutOnly = await listener(bashExec, {
  isError: false,
  value: {
    kind: 'foreground',
    exitCode: 1,
    timedOut: false,
    stdout: { text: 'permission denied appears in command output', truncated: false },
    stderr: { text: '', truncated: false },
  },
  content: [{ type: 'text', text: 'command failed' }],
}, async () => ({ kind: 'accept' }))
assert.equal(stdoutOnly.additionalContexts, undefined)

// Successful non-shell tools own their value schema. Shell-shaped business
// data must never be reinterpreted as a host-level execution failure.
for (const value of [
  {
    kind: 'audit-record',
    exitCode: 1,
    stderr: { text: 'permission denied was captured in the historical record' },
  },
  {
    kind: 'latency-record',
    timedOut: true,
    stderr: { text: 'connection timed out was the recorded outcome' },
  },
  {
    kind: 'archived-result',
    success: false,
    error: { message: 'ENOENT: archived log no longer present' },
  },
]) {
  const businessResult = await listener(exec, result(longText, value), async () => ({ kind: 'accept' }))
  assert.equal(businessResult.additionalContexts, undefined)
}

const customShellListener = register({
  tokenlessBin: binary,
  shellTools: ['custom_process'],
})
const customShellFailure = await customShellListener({ ...exec, name: 'custom_process' }, {
  isError: false,
  value: {
    exitCode: 1,
    stderr: { text: 'permission denied', truncated: false },
  },
  content: [{ type: 'text', text: 'custom process failed' }],
}, async () => ({ kind: 'accept' }))
assert.match(customShellFailure.additionalContexts[0].content[0].text, /ENV_PERMISSION/)

const successfulMatch = await listener(exec, {
  isError: false,
  content: [{ type: 'text', text: 'search result: permission denied' }],
}, async () => ({ kind: 'accept' }))
assert.equal(successfulMatch.additionalContexts, undefined)

const env = await listener(exec, {
  isError: true,
  error: { message: 'command not found: jq' },
  content: [{ type: 'text', text: 'command not found: jq' }],
}, async () => ({ kind: 'accept' }))
assert.equal(env.additionalContexts?.length, 1)
assert.equal(env.additionalContexts[0].role, 'user')
assert.equal(env.additionalContexts[0].source.kind, 'plugin')
assert.match(env.additionalContexts[0].content[0].text, /ENV_DEPENDENCY_MISSING/)
assert.equal(typeof env.additionalContexts[0].id, 'string')

const dashMissing = await listener(bashExec, {
  isError: false,
  value: {
    kind: 'foreground',
    exitCode: 127,
    timedOut: false,
    stderr: { text: '/bin/sh: 1: jq: not found', truncated: false },
    stdout: { text: '', truncated: false },
  },
  content: [{ type: 'text', text: '/bin/sh: 1: jq: not found' }],
}, async () => ({ kind: 'accept' }))
assert.match(dashMissing.additionalContexts[0].content[0].text, /ENV_DEPENDENCY_MISSING/)

// Compression controls never suppress failure attribution.
clearArgs()
const disabledListener = register({
  tokenlessBin: binary,
  responseCompressionEnabled: false,
})
const disabledFailure = await disabledListener({ ...exec, name: 'Grep' }, {
  isError: true,
  error: { message: 'cannot open output for writing' },
  content: [{ type: 'text', text: 'cannot open output for writing' }],
}, async () => ({ kind: 'accept' }))
assert.match(disabledFailure.additionalContexts[0].content[0].text, /ENV_PERMISSION/)
assert.equal(existsSync(argsFile), false)

// Code Mode sub-calls keep attribution but skip compression and stash writes.
clearArgs()
const parentFailure = await listener({ ...bashExec, parent: {} }, {
  isError: false,
  value: {
    kind: 'foreground',
    exitCode: 1,
    timedOut: false,
    stderr: { text: 'permission denied', truncated: false },
    stdout: { text: '', truncated: false },
  },
  content: [{ type: 'text', text: 'permission denied' }],
}, async () => ({ kind: 'accept' }))
assert.equal(parentFailure.additionalContexts?.length, 1)
assert.equal(existsSync(argsFile), false)

// Missing binaries, non-zero exits, timeouts, and invalid/no-op output fail open.
async function failOpen(mode, callback = listener) {
  process.env.TOKENLESS_TEST_MODE = mode
  clearArgs()
  let called = false
  const original = result()
  const decision = await callback(exec, original, async () => {
    called = true
    return { kind: 'accept', content: original.content }
  })
  assert.equal(called, true)
  assert.deepEqual(decision.content, original.content)
  if (mode !== 'timeout') assert.equal(existsSync(argsFile), mode !== 'missing')
}
process.env.TOKENLESS_TEST_MODE = 'fail'
await failOpen('fail')
process.env.TOKENLESS_TEST_MODE = 'same'
await failOpen('same')
process.env.TOKENLESS_TEST_MODE = 'invalid'
await failOpen('invalid')
const timeoutListener = register({ tokenlessBin: binary, timeoutMs: 20 })
process.env.TOKENLESS_TEST_MODE = 'timeout'
await failOpen('timeout', timeoutListener)
const missingListener = register({ tokenlessBin: missingBinary })
await failOpen('missing', missingListener)

// Keep the dsh taxonomy and thresholds in parity with the shared source.
process.env.TOKENLESS_TEST_MODE = 'compress'
const categories = JSON.parse(readFileSync(
  join(root, 'adapters/tokenless/common/hooks/tool_categories.json'),
  'utf8',
))
for (const name of categories.layer_1_skip.tools) {
  clearArgs()
  await listener({ ...exec, name }, result(), async () => ({ kind: 'accept', content: result().content }))
  assert.equal(existsSync(argsFile), false, `skip tool ${name} must not invoke tokenless`)
}
const shellArgs = [
  'compress-response',
  '--agent-id', 'dsh',
  '--truncate-strings-at', String(categories.layer_2_shell.thresholds.truncate_strings_at),
  '--truncate-arrays-at', String(categories.layer_2_shell.thresholds.truncate_arrays_at),
  '--max-depth', String(categories.layer_2_shell.thresholds.max_depth),
  '--session-id', 'session-1',
  '--tool-use-id', 'call-1',
]
for (const name of categories.layer_2_shell.tools) {
  clearArgs()
  await listener({ ...exec, name }, result(), async () => ({ kind: 'accept', content: result().content }))
  assert.deepEqual(args(), shellArgs, `shell tool ${name} must use shared thresholds`)
}
clearArgs()
await listener(exec, result(), async () => ({ kind: 'accept', content: result().content }))
assert.deepEqual(args().slice(0, 9), [
  'compress-response',
  '--agent-id', 'dsh',
  '--truncate-strings-at', String(categories.layer_3_api.thresholds.truncate_strings_at),
  '--truncate-arrays-at', String(categories.layer_3_api.thresholds.truncate_arrays_at),
  '--max-depth', String(categories.layer_3_api.thresholds.max_depth),
])

console.log('native dsh adapter tests passed')
NODE
