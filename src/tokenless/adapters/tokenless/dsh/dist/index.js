/**
 * Native Tokenless plugin for DeepSeek Harness (dsh).
 *
 * This entry intentionally has no dsh runtime imports. DSH supplies the
 * Cordis event types at runtime, while the only process boundary is the
 * installed Tokenless CLI. Keeping the entry dependency-free lets ANOLISA
 * install one self-contained bundle without running npm in $DSH_HOME.
 */
import { execFile } from 'node:child_process'
import { randomUUID } from 'node:crypto'

const PLUGIN_NAME = 'anolisa-tokenless'
const DEFAULT_AGENT_ID = 'dsh'
const DEFAULT_TIMEOUT_MS = 3000
const DEFAULT_MAX_BUFFER = 2 * 1024 * 1024

// DSH does not expose content origin, so map its built-in tool names at the
// host boundary. Core owns every compression decision after this translation.
const FILE_TOOLS = new Set([
  'read',
  'read_file',
  'read_many_files',
  'glob',
  'search_file',
  'list_directory',
  'list_dir',
  'grep',
  'grep_code',
  'grep_search',
  'search_files',
  'lsp',
  'notebookread',
  'notebook_read',
])

const COMMAND_TOOLS = new Set([
  'bash',
  'pwsh',
  'shell',
  'exec',
  'terminal',
  'run_shell_command',
  'run_in_terminal',
  'get_terminal_output',
  'execute_command',
  'process',
])

const INTERRUPTED_CODES = new Set(['ABORTED', 'ABORTED_BEFORE_DISPATCH'])

/** Return a plain config value or the supplied fallback. */
function valueOr(config, key, fallback) {
  return config && Object.prototype.hasOwnProperty.call(config, key)
    ? config[key]
    : fallback
}

/** Resolve the executable without installing or mutating a DSH profile. */
function tokenlessBinary(config) {
  const configured = valueOr(config, 'tokenlessBin', undefined)
  if (typeof configured === 'string' && configured.length > 0) return configured
  return process.env.TOKENLESS_BIN || 'tokenless'
}

/** Convert one process limit to a positive finite integer. */
function positiveInteger(value, fallback) {
  return Number.isInteger(value) && value > 0 ? value : fallback
}

/** Return the only DSH content shape that can be replaced without schema loss. */
function singleText(content) {
  if (!Array.isArray(content) || content.length !== 1) return undefined
  const [block] = content
  if (!block || block.type !== 'text' || typeof block.text !== 'string') return undefined
  return block.text
}

/** Treat shell-shaped values as execution status only for known command tools. */
function commandFailed(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false
  const exitCode = value.exit_code ?? value.exitCode
  const numericExit = typeof exitCode === 'number'
    ? exitCode
    : (typeof exitCode === 'string' && /^-?\d+$/.test(exitCode.trim())
        ? Number(exitCode)
        : 0)
  return numericExit !== 0
    || (typeof value.signal === 'string' && value.signal.length > 0)
    || value.timed_out === true
    || value.timedOut === true
    || value.isError === true
    || value.success === false
    || value.ok === false
}

/** Extract the failure payload while leaving diagnostic classification to Core. */
function failureText(result, value) {
  const parts = []
  if (result?.isError === true && typeof result.error?.message === 'string') {
    parts.push(result.error.message)
  }
  if (Array.isArray(result?.content)) {
    parts.push(...result.content
      .filter((block) => block?.type === 'text' && typeof block.text === 'string')
      .map((block) => block.text))
  }
  if (value && typeof value === 'object' && !Array.isArray(value)) {
    const stderr = value.stderr
    if (typeof stderr === 'string') {
      parts.push(stderr)
    } else if (stderr && typeof stderr.text === 'string') {
      parts.push(stderr.text)
    }
    if (typeof value.error === 'string') {
      parts.push(value.error)
    } else if (value.error && typeof value.error.message === 'string') {
      parts.push(value.error.message)
    } else if (value.error && typeof value.error === 'object') {
      let serialized
      try {
        serialized = JSON.stringify(value.error)
      } catch {
        serialized = String(value.error)
      }
      if (typeof serialized === 'string') parts.push(serialized)
    }
    if (typeof value.signal === 'string' && value.signal.length > 0) {
      parts.push(`terminated by signal: ${value.signal}`)
    }
    if (!parts.some((part) => part.length > 0)) {
      const stdout = value.stdout
      if (typeof stdout === 'string') {
        parts.push(stdout)
      } else if (stdout && typeof stdout.text === 'string') {
        parts.push(stdout.text)
      }
    }
  }
  return parts.filter((part) => part.length > 0).join('\n')
}

/** Construct a valid plugin-owned user message without importing DSH modules. */
function diagnosticContext(text) {
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

/** Add Core's diagnostic to a decision while preserving waterfall output. */
function withDiagnostic(decision, diagnostic) {
  if (typeof diagnostic !== 'string' || diagnostic.length === 0) return decision
  return {
    ...decision,
    additionalContexts: [
      ...(Array.isArray(decision.additionalContexts) ? decision.additionalContexts : []),
      diagnosticContext(diagnostic),
    ],
  }
}

/** Map a DSH tool name to the explicit PostTool content origin. */
function contentOrigin(toolName) {
  const normalized = toolName.toLowerCase()
  if (FILE_TOOLS.has(normalized)) return 'file_content'
  if (COMMAND_TOOLS.has(normalized)) return 'command_output'
  return 'api_response'
}

/** Execute a child process with bounded output and explicit stdin. */
function runTokenless(binary, request, options) {
  return new Promise((resolve, reject) => {
    let child
    try {
      child = execFile(binary, ['compress'], options, (error, stdout) => {
        if (error) {
          reject(error)
          return
        }
        resolve(stdout)
      })
    } catch (error) {
      reject(error)
      return
    }
    child.stdin?.on('error', () => {})
    child.stdin?.end(JSON.stringify(request))
  })
}

/** Run one PostTool operation and reject malformed transport responses. */
async function runPostTool(request, exec, config) {
  try {
    const stdout = await runTokenless(tokenlessBinary(config), request, {
      timeout: positiveInteger(valueOr(config, 'timeoutMs', undefined), DEFAULT_TIMEOUT_MS),
      maxBuffer: positiveInteger(valueOr(config, 'maxBuffer', undefined), DEFAULT_MAX_BUFFER),
      encoding: 'utf8',
      windowsHide: true,
      signal: exec.signal,
    })
    const response = JSON.parse(stdout)
    if (!response || typeof response !== 'object' || Array.isArray(response)) return undefined
    if (response.protocol_version !== 2 || response.operation !== 'post_tool') return undefined
    const result = response.result
    if (!result || typeof result !== 'object' || Array.isArray(result)) return undefined
    return result
  } catch {
    return undefined
  }
}

/** Build one operation request from the DSH execution boundary. */
function postToolRequest(exec, config, content, status, origin, replaceOutput) {
  const attribution = {
    agent_id: String(valueOr(config, 'agentId', DEFAULT_AGENT_ID)),
  }
  if (typeof exec.agent?.id === 'string' && exec.agent.id.length > 0) {
    attribution.session_id = exec.agent.id
  }
  if (typeof exec.callId === 'string' && exec.callId.length > 0) {
    attribution.tool_use_id = exec.callId
  }
  return {
    protocol_version: 2,
    operation: 'post_tool',
    attribution,
    input: {
      result_kind: 'tool',
      tool_name: exec.name,
      content,
      status,
      content_origin: origin,
      output_optimization: 'none',
      capabilities: {
        replace_output: replaceOutput,
        retrieval_available: false,
        replace_with_text: replaceOutput,
      },
    },
  }
}

/** Register DSH's PostTool seam as a thin Tokenless lifecycle adapter. */
export function apply(ctx, config = {}) {
  const compressionEnabled = valueOr(config, 'responseCompressionEnabled', true) !== false

  ctx.on('tools/post-execute', async (exec, result, next) => {
    // DSH post-execute is a waterfall; later policies own the final projection.
    const decision = await next()
    if (decision.kind !== 'accept' || exec.signal?.aborted) return decision

    const toolName = exec.name.toLowerCase()
    const isCommand = COMMAND_TOOLS.has(toolName)
    const replacesValue = Object.prototype.hasOwnProperty.call(decision, 'value')
    const effectiveValue = replacesValue ? decision.value : result.value
    const structuredFailure = isCommand && commandFailed(effectiveValue)

    if (replacesValue) {
      if (!structuredFailure) return decision
      const content = failureText(undefined, effectiveValue)
      const request = postToolRequest(
        exec,
        config,
        content,
        'error',
        'command_output',
        false,
      )
      const response = await runPostTool(request, exec, config)
      const diagnostic = response?.disposition === 'tool_error'
        ? response.additional_context
        : undefined
      return withDiagnostic(decision, diagnostic)
    }

    const abortCode = result?.isError === true ? result.error?.info?.code : undefined
    const interrupted = INTERRUPTED_CODES.has(abortCode)
    const failed = result?.isError === true || structuredFailure
    const status = interrupted ? 'interrupted' : (failed ? 'error' : 'success')

    if (status === 'success') {
      if (!compressionEnabled || exec.parent !== undefined) return decision
      const effectiveContent = decision.content ?? result.content
      const content = singleText(effectiveContent)
      if (content === undefined) return decision
      const request = postToolRequest(
        exec,
        config,
        content,
        status,
        contentOrigin(exec.name),
        true,
      )
      const response = await runPostTool(request, exec, config)
      if (response?.disposition !== 'applied' || typeof response.output !== 'string') {
        return decision
      }
      return {
        ...decision,
        content: [{ type: 'text', text: response.output }],
      }
    }

    const effectiveResult = decision.content === undefined
      ? result
      : { ...result, content: decision.content }
    const content = structuredFailure
      ? failureText(undefined, effectiveValue)
      : failureText(effectiveResult)
    const request = postToolRequest(
      exec,
      config,
      content,
      status,
      contentOrigin(exec.name),
      false,
    )
    const response = await runPostTool(request, exec, config)
    const diagnostic = response?.disposition === 'tool_error'
      ? response.additional_context
      : undefined
    return withDiagnostic(decision, diagnostic)
  })
}

export const name = PLUGIN_NAME
export const inject = ['tools']
