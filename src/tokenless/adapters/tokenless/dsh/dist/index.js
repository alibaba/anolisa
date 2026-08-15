/**
 * Native Tokenless plugin for DeepSeek Harness (dsh).
 *
 * This entry intentionally has no dsh runtime imports.  dsh supplies the
 * Cordis event types at runtime, while the only process boundary is the
 * installed Tokenless CLI.  Keeping the entry dependency-free lets ANOLISA
 * install one self-contained bundle without running npm in $DSH_HOME.
 */
import { execFile } from 'node:child_process'
import { randomUUID } from 'node:crypto'

const PLUGIN_NAME = 'anolisa-tokenless'
const DEFAULT_AGENT_ID = 'dsh'
const DEFAULT_TIMEOUT_MS = 3000
const DEFAULT_MAX_BUFFER = 2 * 1024 * 1024

// Keep content-retrieval tools lossless.  These names mirror the shared
// Tokenless adapter taxonomy; callers may extend or replace the list through
// the dsh plugin config without changing this safety default.
const DEFAULT_SKIP_TOOLS = new Set([
  'Read',
  'read',
  'read_file',
  'read_many_files',
  'Glob',
  'glob',
  'search_file',
  'list_directory',
  'list_dir',
  'Grep',
  'grep',
  'grep_code',
  'grep_search',
  'search_files',
  'Lsp',
  'lsp',
  'NotebookRead',
  'notebook_read',
  'notebookread',
])

// These values are the same thresholds used by the shared hook adapter.  A
// dsh profile can override them in its plugin config; the Tokenless CLI still
// owns the actual compression algorithm and stats recording.
const DEFAULT_SHELL_TOOLS = new Set([
  'Bash',
  'bash',
  'Shell',
  'shell',
  'exec',
  'terminal',
  'run_shell_command',
  'run_in_terminal',
  'get_terminal_output',
  'execute_command',
  'process',
])

const DEFAULT_THRESHOLDS = {
  shell: { strings: 65536, arrays: 128, depth: 8 },
  api: { strings: 1048576, arrays: 65536, depth: 32 },
}

// Mirror common/hooks/hook_utils.py ENV_PATTERNS exactly.  The dsh bundle is
// independently publishable, so changes to the canonical table must update
// this dependency-free runtime copy and its tests together.
const ENV_PATTERNS = [
  [
    [
      /command not found/i,
      /not installed/i,
      /which:\s+no/i,
      /no command\s/i,
      /cannot execute/i,
      /is not recognized/i,
      /could not find/i,
      /unable to locate/i,
      /package not found/i,
      /\/bin\/sh:.*: not found/i,
      /command not found:/i,
    ],
    'ENV_DEPENDENCY_MISSING',
    'Missing dependency detected. Install it or ask the user for guidance.',
  ],
  [
    [
      /permission denied/i,
      /operation not permitted/i,
      /eacces/i,
      /access denied/i,
      /cannot open .* for writing/i,
    ],
    'ENV_PERMISSION',
    'Permission denied. Check file/directory permissions or run with appropriate access.',
  ],
  [
    [
      /no such file or directory/i,
      /enoent/i,
      /cannot find/i,
      /file not found/i,
      /does not exist/i,
    ],
    'ENV_FILE_MISSING',
    'Required file or directory not found. Verify the path or create it.',
  ],
  [
    [
      /connection refused/i,
      /could not resolve host/i,
      /network is unreachable/i,
      /curl: \(7\)/i,
      /curl: \(6\)/i,
      /failed to connect/i,
      /name or service not known/i,
      /couldn't resolve host/i,
      /temporary failure in name resolution/i,
      /econnrefused/i,
      /etimedout/i,
      /connection timed out/i,
    ],
    'ENV_NETWORK',
    'Network connectivity issue. Check DNS, proxy, and firewall settings.',
  ],
  [
    [
      /modulenotfounderror/i,
      /importerror/i,
      /no module named/i,
      /cannot import name/i,
      /npm err! 404/i,
    ],
    'ENV_PACKAGE_MISSING',
    'Required package or module is missing. Install the needed dependency.',
  ],
]

/** Return a plain config value or the supplied fallback. */
function valueOr(config, key, fallback) {
  return config && Object.prototype.hasOwnProperty.call(config, key)
    ? config[key]
    : fallback
}

/** Normalize a config tool list without allowing malformed values to widen it. */
function toolSet(value, fallback) {
  if (!Array.isArray(value)) return fallback
  return new Set(value.filter((name) => typeof name === 'string' && name.length > 0))
}

/** Resolve the executable without ever installing or mutating a dsh profile. */
function tokenlessBinary(config) {
  const configured = valueOr(config, 'tokenlessBin', undefined)
  if (typeof configured === 'string' && configured.length > 0) return configured
  return process.env.TOKENLESS_BIN || 'tokenless'
}

/** Extract text used only for error attribution; never stringify image blocks. */
function errorText(result) {
  if (!result || typeof result !== 'object') return ''
  const error = result.error
  if (error && typeof error.message === 'string') return error.message
  return result.content
    ?.filter((block) => block && block.type === 'text' && typeof block.text === 'string')
    .map((block) => block.text)
    .join('\n') || ''
}

/** Return an environment category and remediation hint for known failure text. */
function classifyEnvironmentError(text) {
  if (typeof text !== 'string') return undefined
  if (!text) return undefined
  for (const [patterns, category, hint] of ENV_PATTERNS) {
    if (patterns.some((pattern) => pattern.test(text))) return { category, hint }
  }
  return undefined
}

/** Extract attribution only when structured output explicitly reports failure. */
function classifyStructuredEnvironmentError(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined
  const exitCode = value.exit_code ?? value.exitCode
  const stringExitCode = typeof exitCode === 'string' ? exitCode.trim() : ''
  const nonzeroExit = (typeof exitCode === 'number' && exitCode !== 0)
    || (/^-?\d+$/.test(stringExitCode) && Number(stringExitCode) !== 0)
  const timedOut = value.timed_out === true || value.timedOut === true
  const failed = nonzeroExit
    || timedOut
    || value.isError === true
    || value.success === false
    || value.ok === false
  if (!failed) return undefined
  const error = value.error
  let errorValue = error
  if (error && typeof error === 'object') {
    if (typeof error.message === 'string') {
      errorValue = error.message
    } else {
      try {
        errorValue = JSON.stringify(error)
      } catch {
        errorValue = String(error)
      }
    }
  }
  const streamText = (stream) => {
    if (typeof stream === 'string') return stream
    if (stream && typeof stream === 'object' && typeof stream.text === 'string') return stream.text
    return undefined
  }
  const text = [streamText(value.stderr), errorValue]
    .filter((part) => typeof part === 'string')
    .join('\n')
  return classifyEnvironmentError(text)
}

/** Construct a valid plugin-owned user message without importing dsh modules. */
function attributionContext(text) {
  return {
    id: randomUUID(),
    role: 'user',
    content: [{ type: 'text', text }],
    source: {
      kind: 'plugin',
      plugin: PLUGIN_NAME,
      form: 'notice',
      summary: text.slice(0, 120),
    },
  }
}

/** Add an attribution context to a decision while preserving its shape. */
function withAttribution(decision, attribution) {
  if (!attribution) return decision
  return {
    ...decision,
    additionalContexts: [
      ...(Array.isArray(decision.additionalContexts) ? decision.additionalContexts : []),
      attributionContext(attribution),
    ],
  }
}

/** Safely read one text-only result projection. */
function singleTextContent(result) {
  if (!result || result.isError || !Array.isArray(result.content)) return undefined
  if (result.content.length !== 1) return undefined
  const [block] = result.content
  if (!block || block.type !== 'text' || typeof block.text !== 'string') return undefined
  return block.text
}

/** Convert one config threshold to a positive finite integer. */
function positiveInteger(value, fallback) {
  return Number.isInteger(value) && value > 0 ? value : fallback
}

/** Build the Tokenless CLI argv for one dsh execution. */
function compressionArgs(exec, config, shellTools) {
  const selected = shellTools.has(exec.name) ? DEFAULT_THRESHOLDS.shell : DEFAULT_THRESHOLDS.api
  const strings = positiveInteger(valueOr(config, 'truncateStringsAt', undefined), selected.strings)
  const arrays = positiveInteger(valueOr(config, 'truncateArraysAt', undefined), selected.arrays)
  const depth = positiveInteger(valueOr(config, 'maxDepth', undefined), selected.depth)
  const args = [
    'compress-response',
    '--agent-id',
    String(valueOr(config, 'agentId', DEFAULT_AGENT_ID)),
    '--truncate-strings-at',
    String(strings),
    '--truncate-arrays-at',
    String(arrays),
    '--max-depth',
    String(depth),
  ]
  const sessionId = exec.agent?.id
  if (typeof sessionId === 'string' && sessionId.length > 0) {
    args.push('--session-id', sessionId)
  }
  if (typeof exec.callId === 'string' && exec.callId.length > 0) {
    args.push('--tool-use-id', exec.callId)
  }
  if (valueOr(config, 'noStash', false) === true) args.push('--no-stash')
  return args
}

/** Execute a child process with bounded output and explicit stdin. */
function runTokenless(binary, args, options, input) {
  return new Promise((resolve, reject) => {
    let child
    try {
      child = execFile(binary, args, options, (error, stdout, stderr) => {
        if (error) {
          error.stderr = stderr
          reject(error)
          return
        }
        resolve({ stdout, stderr })
      })
    } catch (error) {
      reject(error)
      return
    }
    child.stdin?.on('error', () => {})
    try {
      child.stdin?.end(input)
    } catch (error) {
      reject(error)
    }
  })
}

/** Run Tokenless and return a strictly smaller JSON candidate, or undefined. */
async function compressText(text, exec, config, shellTools) {
  if (exec.signal?.aborted) return undefined
  let parsed
  try {
    parsed = JSON.parse(text)
  } catch {
    return undefined
  }
  if (parsed === null || (typeof parsed !== 'object' && !Array.isArray(parsed))) return undefined
  const binary = tokenlessBinary(config)
  try {
    const { stdout } = await runTokenless(binary, compressionArgs(exec, config, shellTools), {
      timeout: positiveInteger(valueOr(config, 'timeoutMs', undefined), DEFAULT_TIMEOUT_MS),
      maxBuffer: positiveInteger(valueOr(config, 'maxBuffer', undefined), DEFAULT_MAX_BUFFER),
      encoding: 'utf8',
      windowsHide: true,
      signal: exec.signal,
    }, text)
    const candidate = typeof stdout === 'string' ? stdout.trim() : ''
    // The host's original content is authoritative unless compression proves
    // a real reduction.  This avoids duplicate payloads and preserves fail-open
    // behavior when the CLI is unavailable, malformed, or no-op.
    if (!candidate || candidate.length >= text.length) return undefined
    JSON.parse(candidate)
    return candidate
  } catch {
    return undefined
  }
}

/** Register native response compression on dsh's typed post-execute seam. */
export function apply(ctx, config = {}) {
  const skipTools = toolSet(valueOr(config, 'skipTools', undefined), DEFAULT_SKIP_TOOLS)
  const shellTools = toolSet(valueOr(config, 'shellTools', undefined), DEFAULT_SHELL_TOOLS)
  const enabled = valueOr(config, 'responseCompressionEnabled', true) !== false
  ctx.on('tools/post-execute', async (exec, result, next) => {
    const envError = result?.isError === true
      ? classifyEnvironmentError(errorText(result))
      : undefined
    const structuredError = result?.isError === false && shellTools.has(exec.name)
      ? classifyStructuredEnvironmentError(result.value)
      : undefined
    // Attribution is independent of compression so disabled, skipped, and
    // parented failures still tell the agent why blind retries are unsafe.
    const originalAttribution = structuredError || envError

    // DSH treats this seam as a waterfall.  Let downstream policies settle
    // their decision before replacing only its accepted display content.
    const decision = await next()
    const replacesValue = Object.prototype.hasOwnProperty.call(decision, 'value')
    // A downstream canonical value replaces the original result entirely, so
    // any attribution must describe that value rather than the stale result.
    const attribution = replacesValue
      ? (shellTools.has(exec.name)
          ? classifyStructuredEnvironmentError(decision.value)
          : undefined)
      : originalAttribution
    const responseContext = attribution
      ? `[tokenless:env] ${exec.name} failed: ${attribution.category} (${attribution.hint}). Skip retry.`
      : undefined
    const canCompress = enabled
      && !skipTools.has(exec.name)
      && exec.parent === undefined
      && decision.kind === 'accept'
      && !replacesValue
    if (!canCompress) return withAttribution(decision, responseContext)

    // Only a single text block is safe to replace.  Images, tool-call blocks,
    // nested tool results, and mixed content remain untouched by design.
    const contentResult = decision.content === undefined
      ? result
      : { ...result, content: decision.content }
    const original = singleTextContent(contentResult)
    if (original === undefined) return withAttribution(decision, responseContext)

    const candidate = await compressText(original, exec, config, shellTools)
    if (!candidate) return withAttribution(decision, responseContext)

    return {
      ...withAttribution(decision, responseContext),
      content: [{ type: 'text', text: candidate }],
    }
  })
}

export const name = PLUGIN_NAME
export const inject = ['tools']
