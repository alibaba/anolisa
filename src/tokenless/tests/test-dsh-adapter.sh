#!/usr/bin/env bash
# Exercise the native DSH bundle without requiring a DSH installation.
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
const logFile = join(tmp, 'requests.jsonl')
const missingBinary = join(tmp, 'missing-tokenless')
writeFileSync(
  binary,
  '#!/usr/bin/env node\n' +
    'const { appendFileSync } = require("node:fs");\n' +
    'process.stdin.setEncoding("utf8"); let input = "";\n' +
    'process.stdin.on("data", chunk => input += chunk);\n' +
    'process.stdin.on("end", () => {\n' +
    '  const request = JSON.parse(input);\n' +
    '  appendFileSync(process.env.TOKENLESS_TEST_LOG, JSON.stringify({ argv: process.argv.slice(2), request }) + "\\n");\n' +
    '  const mode = process.env.TOKENLESS_TEST_MODE;\n' +
    '  if (mode === "fail") process.exit(7);\n' +
    '  if (mode === "timeout") { setTimeout(() => {}, 10000); return; }\n' +
    '  if (mode === "invalid") { process.stdout.write("{"); return; }\n' +
    '  const result = mode === "applied"\n' +
    '    ? { output: "{\\"ok\\":true}", disposition: "applied" }\n' +
    '    : mode === "tool-error"\n' +
    '      ? { output: request.input.content, disposition: "tool_error", additional_context: "Install the missing dependency and retry." }\n' +
    '      : { output: request.input.content, disposition: "passthrough" };\n' +
    '  process.stdout.write(JSON.stringify({ protocol_version: mode === "wrong-version" ? 1 : 2, operation: mode === "wrong-operation" ? "pre_tool" : "post_tool", attribution: request.attribution, result }));\n' +
    '});\n',
)
chmodSync(binary, 0o755)
process.env.TOKENLESS_TEST_LOG = logFile

const pluginPath = join(root, 'adapters/tokenless/dsh/dist/index.js')
const plugin = await import(`file://${pluginPath}`)
const cordisPatch = readFileSync(
  join(root, 'adapters/tokenless/dsh/cordis.patch.yml'),
  'utf8',
)
assert.match(cordisPatch, /name:\s+['"]@anolisa\/dsh-tokenless['"]/)
assert.doesNotMatch(cordisPatch, /name:\s+\.\/dist\/index\.js/)

function register(config = {}) {
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

function clearRequests() {
  if (existsSync(logFile)) unlinkSync(logFile)
}

function requests() {
  if (!existsSync(logFile)) return []
  return readFileSync(logFile, 'utf8')
    .trim()
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line))
}

function success(text = '{"long":"this payload is intentionally long"}', value = { ok: true }) {
  return {
    isError: false,
    value,
    content: [{ type: 'text', text }],
  }
}

const downstreamContext = {
  id: 'downstream-context',
  role: 'user',
  content: [{ type: 'text', text: 'downstream policy context' }],
}
const exec = {
  name: 'api_call',
  callId: 'call-1',
  signal: new AbortController().signal,
  agent: { id: 'session-1' },
}
const listener = register({ tokenlessBin: binary })
assert.equal(plugin.name, 'anolisa-tokenless')
assert.deepEqual(plugin.inject, ['tools'])

// DSH contributes host facts; Core owns detection, compression, and selection.
process.env.TOKENLESS_TEST_MODE = 'applied'
clearRequests()
let nextCalled = false
const applied = await listener(exec, success(), async () => {
  nextCalled = true
  return { kind: 'accept', additionalContexts: [downstreamContext] }
})
assert.equal(nextCalled, true)
assert.deepEqual(applied.content, [{ type: 'text', text: '{"ok":true}' }])
assert.deepEqual(applied.additionalContexts, [downstreamContext])
assert.deepEqual(requests(), [{
  argv: ['compress'],
  request: {
    protocol_version: 2,
    operation: 'post_tool',
    attribution: {
      agent_id: 'dsh',
      session_id: 'session-1',
      tool_use_id: 'call-1',
    },
    input: {
      result_kind: 'tool',
      tool_name: 'api_call',
      content: '{"long":"this payload is intentionally long"}',
      status: 'success',
      content_origin: 'api_response',
      output_optimization: 'none',
      capabilities: {
        replace_output: true,
        retrieval_available: false,
        replace_with_text: true,
      },
    },
  },
}])

// Plain text still reaches Core instead of being classified by the adapter.
process.env.TOKENLESS_TEST_MODE = 'passthrough'
clearRequests()
const plainDecision = { kind: 'accept' }
const plain = await listener(exec, success('ordinary plain text'), async () => plainDecision)
assert.strictEqual(plain, plainDecision)
assert.equal(requests()[0].request.input.content, 'ordinary plain text')

// A downstream content projection is the model-visible input sent to Core.
clearRequests()
await listener(exec, success('original'), async () => ({
  kind: 'accept',
  content: [{ type: 'text', text: 'downstream replacement' }],
}))
assert.equal(requests()[0].request.input.content, 'downstream replacement')

// Content origin is an explicit host translation, not an adapter policy knob.
const categories = JSON.parse(readFileSync(
  join(root, 'adapters/tokenless/common/hooks/tool_categories.json'),
  'utf8',
))
for (const name of categories.layer_1_skip.tools) {
  clearRequests()
  await listener({ ...exec, name }, success(), async () => ({ kind: 'accept' }))
  assert.equal(requests()[0].request.input.content_origin, 'file_content', name)
}
for (const name of categories.layer_2_shell.tools) {
  clearRequests()
  await listener({ ...exec, name }, success(undefined, { exitCode: 0 }), async () => ({ kind: 'accept' }))
  assert.equal(requests()[0].request.input.content_origin, 'command_output', name)
}
clearRequests()
await listener(exec, success(), async () => ({ kind: 'accept' }))
assert.equal(requests()[0].request.input.content_origin, 'api_response')

// Unsafe replacement shapes and a blocking downstream policy remain untouched.
clearRequests()
const block = {
  kind: 'block',
  feedback: [{ type: 'text', text: 'policy blocked this result' }],
  additionalContexts: [downstreamContext],
}
assert.strictEqual(await listener(exec, success(), async () => block), block)
assert.equal(requests().length, 0)

const mixedDecision = { kind: 'accept' }
assert.strictEqual(await listener(exec, {
  isError: false,
  value: { ok: true },
  content: [
    { type: 'text', text: 'text' },
    { type: 'image', attachment: { id: 'image-1' } },
  ],
}, async () => mixedDecision), mixedDecision)
assert.equal(requests().length, 0)

const valueDecision = {
  kind: 'accept',
  value: { canonical: true },
  additionalContexts: [downstreamContext],
}
assert.strictEqual(await listener(exec, success(), async () => valueDecision), valueDecision)
assert.equal(requests().length, 0)

// Raw and structured command failures delegate diagnosis to Core.
process.env.TOKENLESS_TEST_MODE = 'tool-error'
clearRequests()
const rawFailure = await listener(exec, {
  isError: true,
  error: { message: 'command not found: jq', info: { name: 'Error', code: 'FAILED' } },
  content: [{ type: 'text', text: 'jq could not run' }],
}, async () => ({ kind: 'accept', additionalContexts: [downstreamContext] }))
assert.equal(rawFailure.additionalContexts.length, 2)
assert.strictEqual(rawFailure.additionalContexts[0], downstreamContext)
assert.match(rawFailure.additionalContexts[1].content[0].text, /Install the missing dependency/)
assert.equal(rawFailure.additionalContexts[1].source.plugin, 'anolisa-tokenless')
let request = requests()[0].request
assert.equal(request.input.status, 'error')
assert.equal(request.input.capabilities.replace_output, false)
assert.equal(request.input.capabilities.replace_with_text, false)
assert.match(request.input.content, /command not found: jq/)

clearRequests()
await listener(exec, {
  isError: true,
  error: { message: 'generic failure' },
  content: [{ type: 'text', text: 'stale display content' }],
}, async () => ({
  kind: 'accept',
  content: [{ type: 'text', text: 'permission denied after downstream replacement' }],
}))
assert.match(requests()[0].request.input.content, /permission denied after downstream replacement/)
assert.doesNotMatch(requests()[0].request.input.content, /stale display content/)

const bashExec = { ...exec, name: 'Bash' }
clearRequests()
const commandFailure = await listener(bashExec, success('rendered command output', {
  kind: 'foreground',
  exitCode: 1,
  timedOut: false,
  stdout: { text: 'command stdout must not drive diagnosis', truncated: false },
  stderr: { text: 'permission denied', truncated: false },
}), async () => ({ kind: 'accept' }))
assert.equal(commandFailure.additionalContexts.length, 1)
request = requests()[0].request
assert.equal(request.input.status, 'error')
assert.equal(request.input.content_origin, 'command_output')
assert.equal(request.input.content, 'permission denied')

clearRequests()
await listener(bashExec, success('permission denied\n[exit code: 1]', {
  kind: 'foreground',
  exitCode: 1,
  timedOut: false,
  stdout: { text: 'permission denied', truncated: false },
  stderr: { text: '', truncated: false },
}), async () => ({ kind: 'accept' }))
request = requests()[0].request
assert.equal(request.input.status, 'error')
assert.equal(request.input.content_origin, 'command_output')
assert.equal(request.input.capabilities.replace_output, false)
assert.equal(request.input.content, 'permission denied')

clearRequests()
await listener(bashExec, success('(no output)\n[exit code: 1]', {
  kind: 'foreground',
  exitCode: 1,
  timedOut: false,
  stdout: { text: '', truncated: false },
  stderr: { text: '', truncated: false },
}), async () => ({ kind: 'accept' }))
request = requests()[0].request
assert.equal(request.input.status, 'error')
assert.equal(request.input.content, '')

const pwshExec = { ...exec, name: 'pwsh' }
clearRequests()
await listener(pwshExec, success('rendered PowerShell output', {
  kind: 'foreground',
  exitCode: 1,
  signal: null,
  timedOut: false,
  stdout: { text: '', truncated: false },
  stderr: { text: 'access denied', truncated: false },
}), async () => ({ kind: 'accept' }))
request = requests()[0].request
assert.equal(request.input.status, 'error')
assert.equal(request.input.content_origin, 'command_output')
assert.equal(request.input.capabilities.replace_output, false)

clearRequests()
await listener(bashExec, success('[killed by signal: SIGTERM]', {
  kind: 'foreground',
  exitCode: null,
  signal: 'SIGTERM',
  timedOut: false,
  stdout: { text: '', truncated: false },
  stderr: { text: '', truncated: false },
}), async () => ({ kind: 'accept' }))
request = requests()[0].request
assert.equal(request.input.status, 'error')
assert.equal(request.input.content_origin, 'command_output')
assert.equal(request.input.capabilities.replace_output, false)
assert.equal(request.input.content, 'terminated by signal: SIGTERM')

// Shell-shaped business data from an API tool remains a successful result.
process.env.TOKENLESS_TEST_MODE = 'applied'
clearRequests()
const businessResult = await listener(exec, success('business record', {
  exitCode: 1,
  timedOut: true,
  stderr: { text: 'archived failure text' },
}), async () => ({ kind: 'accept' }))
assert.deepEqual(businessResult.content, [{ type: 'text', text: '{"ok":true}' }])
assert.equal(requests()[0].request.input.status, 'success')

// A downstream command value is authoritative and can still receive guidance.
process.env.TOKENLESS_TEST_MODE = 'tool-error'
clearRequests()
const replacementValue = {
  kind: 'foreground',
  exitCode: 1,
  stderr: { text: 'connection refused', truncated: false },
}
const replacementFailure = await listener(bashExec, success(), async () => ({
  kind: 'accept',
  value: replacementValue,
  additionalContexts: [downstreamContext],
}))
assert.strictEqual(replacementFailure.value, replacementValue)
assert.equal(replacementFailure.additionalContexts.length, 2)
request = requests()[0].request
assert.equal(request.input.status, 'error')
assert.equal(request.input.capabilities.replace_output, false)
assert.equal(request.input.content, 'connection refused')

// Let DSH validate an invalid downstream value instead of rejecting in Tokenless.
const circularError = {}
circularError.self = circularError
const circularValue = {
  kind: 'foreground',
  exitCode: 1,
  stderr: { text: 'permission denied', truncated: false },
  error: circularError,
}
clearRequests()
const circularFailure = await listener(bashExec, success(), async () => ({
  kind: 'accept',
  value: circularValue,
}))
assert.strictEqual(circularFailure.value, circularValue)
assert.match(requests()[0].request.input.content, /permission denied/)

// DSH cancellation is distinguished from a tool error when it reaches the seam.
process.env.TOKENLESS_TEST_MODE = 'passthrough'
clearRequests()
await listener(exec, {
  isError: true,
  error: {
    message: 'operation aborted',
    info: { name: 'AbortError', code: 'ABORTED_BEFORE_DISPATCH' },
  },
  content: [{ type: 'text', text: 'operation aborted' }],
}, async () => ({ kind: 'accept' }))
assert.equal(requests()[0].request.input.status, 'interrupted')

clearRequests()
let abortedNextCalled = false
const abortedDecision = { kind: 'accept' }
const aborted = await listener({
  ...exec,
  signal: AbortSignal.abort(),
}, success(), async () => {
  abortedNextCalled = true
  return abortedDecision
})
assert.equal(abortedNextCalled, true)
assert.strictEqual(aborted, abortedDecision)
assert.equal(requests().length, 0)

// Code Mode child success is not replaceable, but failures still get guidance.
clearRequests()
const parentDecision = { kind: 'accept' }
assert.strictEqual(await listener(
  { ...exec, parent: {} },
  success(),
  async () => parentDecision,
), parentDecision)
assert.equal(requests().length, 0)

process.env.TOKENLESS_TEST_MODE = 'tool-error'
const parentFailure = await listener({ ...bashExec, parent: {} }, success('failed', {
  exitCode: 1,
  stderr: { text: 'permission denied', truncated: false },
}), async () => ({ kind: 'accept' }))
assert.equal(parentFailure.additionalContexts.length, 1)

// Disabling compression does not disable Core-owned error diagnostics.
const disabledListener = register({
  tokenlessBin: binary,
  responseCompressionEnabled: false,
})
clearRequests()
assert.strictEqual(await disabledListener(exec, success(), async () => plainDecision), plainDecision)
assert.equal(requests().length, 0)
const disabledFailure = await disabledListener(exec, {
  isError: true,
  error: { message: 'permission denied' },
  content: [{ type: 'text', text: 'permission denied' }],
}, async () => ({ kind: 'accept' }))
assert.equal(disabledFailure.additionalContexts.length, 1)

// Missing binaries, non-zero exits, timeouts, and malformed responses fail open.
async function failOpen(mode, callback = listener) {
  process.env.TOKENLESS_TEST_MODE = mode
  clearRequests()
  const decision = { kind: 'accept', content: success().content }
  const actual = await callback(exec, success(), async () => decision)
  assert.strictEqual(actual, decision)
}
await failOpen('fail')
await failOpen('invalid')
await failOpen('wrong-version')
await failOpen('wrong-operation')
const timeoutListener = register({ tokenlessBin: binary, timeoutMs: 20 })
await failOpen('timeout', timeoutListener)
const missingListener = register({ tokenlessBin: missingBinary })
await failOpen('missing', missingListener)
assert.equal(requests().length, 0)

// Attribution remains configurable without restoring compression policy knobs.
process.env.TOKENLESS_TEST_MODE = 'passthrough'
clearRequests()
const attributedListener = register({ tokenlessBin: binary, agentId: 'custom-dsh' })
await attributedListener({
  name: 'api_call',
  signal: new AbortController().signal,
}, success(), async () => ({ kind: 'accept' }))
assert.deepEqual(requests()[0].request.attribution, { agent_id: 'custom-dsh' })

console.log('native DSH adapter tests passed')
NODE
