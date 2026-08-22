
if [[ -n "${COSH_OSC_MARKER_LOADED:-}" ]]; then
  return 0 2>/dev/null || exit 0
fi
COSH_OSC_MARKER_LOADED=1
if [[ $- != *i* ]]; then
  return 0 2>/dev/null || exit 0
fi
export COSH_SESSION_ID="${COSH_SESSION_ID:-cosh-osc-$$}"
export COSH_POC_PS1="${COSH_POC_PS1:-cosh-osc$ }"
_COSH_INITIAL_COMMAND_NOT_FOUND_HANDLE="$(declare -f command_not_found_handle 2>/dev/null || true)"
if [[ -z "${COSH_SHELL_ISOLATED:-}" ]]; then
  if [[ "${COSH_LOGIN_SHELL:-}" == "1" ]]; then
    [[ -f /etc/profile ]] && source /etc/profile
    if [[ -f ~/.bash_profile ]]; then source ~/.bash_profile
    elif [[ -f ~/.bash_login ]]; then source ~/.bash_login
    elif [[ -f ~/.profile ]]; then source ~/.profile
    fi
  else
    [[ -f ~/.bashrc ]] && source ~/.bashrc
  fi
fi
_COSH_AI_ENABLED="$_COSH_SESSION_AI_ENABLED"
readonly _COSH_AI_ENABLED
_cosh_load_native_bash_history_if_empty() {
  if [[ -n "${COSH_SHELL_ISOLATED:-}" ]]; then
    return 0
  fi
  if [[ -z "${HISTFILE:-}" || ! -r "$HISTFILE" ]]; then
    return 0
  fi
  if [[ -n "$(builtin history 1 2>/dev/null)" ]]; then
    return 0
  fi
  builtin history -r "$HISTFILE" 2>/dev/null || true
}
if [[ -z "${COSH_SHELL_ISOLATED:-}" ]]; then
  : # native mode: keep user PS1, HISTFILE, etc.
else
  export PS1="$COSH_POC_PS1"
  set -o history
  export HISTFILE="${COSH_HISTFILE:-/dev/null}"
  export HISTSIZE=1000
  export HISTFILESIZE=1000
  export HISTCONTROL=
  export HISTIGNORE=
  export HISTTIMEFORMAT=
fi
_cosh_load_native_bash_history_if_empty
_COSH_AT_PROMPT=0
_COSH_IN_PROMPT_COMMAND=0
_COSH_LAST_NATIVE_HISTORY_FILE=
_COSH_ATTEMPT_GENERATION=0
_COSH_ATTEMPT_ACTIVE=0
_COSH_ATTEMPT_INPUT=
_COSH_ATTEMPT_TOKEN=
_COSH_ATTEMPT_TOKEN_FINGERPRINT=
_COSH_ATTEMPT_SENSITIVE=0
_COSH_ATTEMPT_UNSAFE=0
_COSH_ATTEMPT_EXPANSION_DRIFT=0
_COSH_ATTEMPT_SUBSHELL=
_COSH_WRAPPER_ID="${COSH_SESSION_ID}:${COSH_MARKER_TOKEN}"
_cosh_apply_internal_recovery() {
  if [[ -z "${COSH_RECOVERY_REQUEST_FILE:-}" || ! -f "$COSH_RECOVERY_REQUEST_FILE" ]]; then
    return 0
  fi
  trap - DEBUG
  rm -f -- "$COSH_RECOVERY_REQUEST_FILE" 2>/dev/null || true
  stty echo icanon isig iexten opost 2>/dev/null || true
  trap '_cosh_preexec_marker' DEBUG
}
_cosh_json_escape() {
  local value="$1"
  value=${value//\\/\\\\}
  value=${value//\"/\\\"}
  value=${value//$'\n'/\\n}
  value=${value//$'\r'/\\r}
  value=${value//$'\t'/\\t}
  printf '%s' "$value"
}
_cosh_native_history_file_path() {
  if [[ -n "${COSH_SHELL_ISOLATED:-}" || -z "${HISTFILE:-}" ]]; then
    return 1
  fi
  local history_file="$HISTFILE"
  case "$history_file" in
    /*) ;;
    '~') history_file="$HOME" ;;
    '~/'*) history_file="$HOME/${history_file#\~/}" ;;
    *) history_file="$PWD/$history_file" ;;
  esac
  if [[ "$history_file" != /* ]]; then
    return 1
  fi
  if printf '%s' "$history_file" | LC_ALL=C grep -q '[[:cntrl:]]'; then
    return 1
  fi
  printf '%s' "$history_file"
}
_cosh_emit_native_history_file_marker() {
  local history_file="$1"
  printf '\033]1337;COSH;{"event":"history_file","token":"%s","session_id":"%s","history_file":"%s"}\a' \
    "$(_cosh_json_escape "$COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$(_cosh_json_escape "$history_file")"
}
_cosh_maybe_emit_native_history_file_marker() {
  local history_file
  history_file="$(_cosh_native_history_file_path)" || return 0
  if [[ "$history_file" == "${_COSH_LAST_NATIVE_HISTORY_FILE:-}" ]]; then
    return 0
  fi
  if _cosh_emit_native_history_file_marker "$history_file"; then
    _COSH_LAST_NATIVE_HISTORY_FILE="$history_file"
  fi
}
_cosh_maybe_emit_native_history_file_marker
# builtin strftime keeps the marker emission path free of an external
# `date` exec (NS-009 fork hygiene; the enclosing $() substitution remains
# a one-shot subshell). %(...)T needs bash >= 4.2, so probe once and keep
# the `date` fallback for older hosts (e.g. macOS /bin/bash 3.2 in dev).
if printf '%(%s)T' -1 >/dev/null 2>&1; then
  _cosh_now_ms() {
    printf '%(%s)T000\n' -1
  }
else
  _cosh_now_ms() {
    date +%s000
  }
fi
_cosh_history_entry() {
  local saved_fmt="$HISTTIMEFORMAT"
  HISTTIMEFORMAT=
  local entry
  entry="$(builtin history 1 2>/dev/null)"
  HISTTIMEFORMAT="$saved_fmt"
  printf '%s' "$entry"
}
_cosh_history_no() {
  printf '%s' "$1" | sed -E 's/^[[:space:]]*([0-9]+).*/\1/'
}
_cosh_history_command_from_entry() {
  local saved_fmt="$HISTTIMEFORMAT"
  HISTTIMEFORMAT=
  local entry
  entry="$(builtin history 1 2>/dev/null)"
  HISTTIMEFORMAT="$saved_fmt"
  printf '%s' "$entry" | sed -E 's/^[[:space:]]*[0-9]+[[:space:]]*//'
}
_cosh_command_has_secret() {
  local lower
  lower="$(printf '%s' "$1" | LC_ALL=C tr '[:upper:]' '[:lower:]')"
  case "$lower" in
    *"-----begin "*"private key-----"*|*"bearer "*|*"://"*":"*"@"*|*ghp_*|*github_pat_*|*glpat-*|*npm_*|*hf_*|*xox?-*|*aiza*)
      return 0
      ;;
    *ltai????????????*)
      return 0
      ;;
    *akia????????????????*|*asia????????????????*)
      return 0
      ;;
    sk-*|sk_live_*|sk_test_*|*" sk-"*|*"=sk-"*|*":sk-"*|*"\"sk-"*|*"'sk-"*|*" sk_live_"*|*" sk_test_"*|*"=sk_live_"*|*"=sk_test_"*)
      return 0
      ;;
  esac
  local key
  for key in password passwd passphrase token access_token access-token refresh_token refresh-token id_token id-token secret client_secret client-secret api_key api-key apikey access_key_id access-key-id access_key_secret access-key-secret security_token security-token authorization cookie set-cookie; do
    case "$lower" in
      *"$key="*|*"$key:"*|*"--$key "*|*"--$key="*)
        return 0
        ;;
    esac
  done
  return 1
}
_cosh_emit_marker() {
  local event="$1"
  local command="$2"
  local exit_status="$3"
  local path_trusted="${4:-false}"
  local timestamp
  timestamp="$(_cosh_now_ms)"
  # Optional handoff-claim fragment (#2142): only approved-handoff preexec
  # lines carry a token, every other marker stays byte-identical.
  local handoff_fragment=""
  if [[ -n "${_COSH_HANDOFF_TOKEN:-}" ]]; then
    handoff_fragment=",\"handoff\":\"$(_cosh_json_escape "$_COSH_HANDOFF_TOKEN")\""
  fi
  printf '\033]1337;COSH;{"event":"%s","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","command":"%s","status":%s,"path":"%s","path_trusted":%s,"generation":%s%s}\a' \
    "$(_cosh_json_escape "$event")" \
    "$(_cosh_json_escape "$COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "$(_cosh_json_escape "$command")" \
    "$exit_status" \
    "$(_cosh_json_escape "$PATH")" \
    "$path_trusted" \
    "${_COSH_ATTEMPT_GENERATION:-0}" \
    "$handoff_fragment"
}
_cosh_emit_intercept_marker() {
  local input="$1"
  local reason="$2"
  local top_level_missing="${3:-false}"
  local sensitive="${4:-false}"
  local timestamp
  timestamp="$(_cosh_now_ms)"
  printf '\033]1337;COSH;{"event":"intercept","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","command":"%s","reason":"%s","status":0,"generation":%s,"top_level_missing":%s,"sensitive":%s}\a' \
    "$(_cosh_json_escape "$COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "$(_cosh_json_escape "$input")" \
    "$(_cosh_json_escape "$reason")" \
    "${_COSH_ATTEMPT_GENERATION:-0}" \
    "$top_level_missing" \
    "$sensitive"
}
_cosh_emit_top_level_missing_marker() {
  local intent="$1"
  local sensitive="${2:-false}"
  local unsafe="${3:-false}"
  local timestamp
  timestamp="$(_cosh_now_ms)"
  printf '\033]1337;COSH;{"event":"top_level_missing","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","generation":%s,"proven":true,"intent":"%s","sensitive":%s,"unsafe":%s}\a' \
    "$(_cosh_json_escape "$COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "${_COSH_ATTEMPT_GENERATION:-0}" \
    "$(_cosh_json_escape "$intent")" \
    "$sensitive" \
    "$unsafe"
}
_cosh_should_intercept_unknown() {
  local command="$1"
  if _cosh_is_slash_control_candidate "$command"; then
    printf '%s' "slash"
    return 0
  fi
  if [[ "$command" == "??" || "$command" == "??"* ]]; then
    printf '%s' "agent_marker"
    return 0
  fi
  return 1
}
_cosh_is_slash_control_candidate() {
  local command="$1"
  case "$command" in
    /about|/agent|/allow|/answer|/approval-mode|/approve|/audit|/auth|/cancel|/clear|/config|/copy|/debug|/deny|/details|/explain|/extensions|/health|/help|/hooks|/mcp|/mode|/new|/recommendations|/resume|/select|/send-to-shell|/session|/shell|/skills|/stats|/status)
      return 0
      ;;
  esac
  return 1
}
# bash executes slash-bearing command words as paths without consulting
# command_not_found_handle, so the natural-language classifier never sees
# them (#1919). Reclassify here with the missing-path context; only a
# natural_language verdict on a provably-ENOENT path intercepts (dangling
# symlinks and permission-opaque paths keep their native 126/127 errors),
# everything else keeps the native bash error byte-identical to the
# pre-fix behavior. Secret-bearing lines are not vetoed here (#2138):
# both callers compute the sensitive flag, scrub history, and mark the
# intercept so durable sinks redact the whole input field.
_cosh_should_intercept_missing_path() {
  local first_word="$1"
  local command="$2"
  [[ "$first_word" == */* ]] || return 1
  [[ "${_COSH_AI_ENABLED:-1}" == 1 ]] || return 1
  _cosh_path_provably_missing "$first_word" || return 1
  local intent
  intent="$(_cosh_classify_missing "$command" "$first_word" missing_path)"
  [[ "$intent" == "natural_language" ]]
}
_COSH_HANDOFF_PREFIX='COSH_SHELL_HANDOFF_BYPASS=1 '
# Transport-only prefix for agent handoffs whose implicit pagers are disabled.
# Must stay byte-identical to NON_INTERACTIVE_PAGER_PREFIX in
# src/types/shell_handoff.rs, or the original command text would leak into
# markers, history and evidence.
_COSH_HANDOFF_PAGER_PREFIX='PAGER=cat GIT_PAGER=cat MANPAGER=cat SYSTEMD_PAGER=cat '
# Only the bypass prefix marks a transport line: handoff_pty_bytes always emits
# it first, so a line that merely starts with the pager assignments is an
# ordinary user command and must keep its full text.
_cosh_is_handoff_wrapper() {
  case "$1" in
    "$_COSH_HANDOFF_PREFIX"*)
      return 0
      ;;
  esac
  return 1
}
_cosh_unwrap_handoff_command() {
  local command="${1#$_COSH_HANDOFF_PREFIX}"
  printf '%s' "${command#$_COSH_HANDOFF_PAGER_PREFIX}"
}
_cosh_is_pending_handoff_command() {
  local command="$1"
  if [[ -z "${COSH_HANDOFF_REQUEST_FILE:-}" || ! -f "$COSH_HANDOFF_REQUEST_FILE" ]]; then
    return 1
  fi
  [[ "$(cat -- "$COSH_HANDOFF_REQUEST_FILE" 2>/dev/null)" == "$command" ]]
}
_cosh_clear_handoff_request() {
  if [[ -n "${COSH_HANDOFF_REQUEST_FILE:-}" && -f "$COSH_HANDOFF_REQUEST_FILE" ]]; then
    rm -f -- "$COSH_HANDOFF_REQUEST_FILE" 2>/dev/null || true
  fi
  if [[ -n "${COSH_HANDOFF_REQUEST_FILE:-}"
     && -f "${COSH_HANDOFF_REQUEST_FILE}.no-pager" ]]; then
    rm -f -- "${COSH_HANDOFF_REQUEST_FILE}.no-pager" 2>/dev/null || true
  fi
  if [[ -n "${COSH_HANDOFF_REQUEST_FILE:-}"
     && -f "${COSH_HANDOFF_REQUEST_FILE}.token" ]]; then
    rm -f -- "${COSH_HANDOFF_REQUEST_FILE}.token" 2>/dev/null || true
  fi
}
# One-time claim token for the approved handoff (#2142). Staged by the Rust
# transport next to the request file; carried back on the preexec/precmd
# markers so the parser can claim the command block even when the reported
# command text is redacted. Missing sidecar leaves the token empty, which
# keeps the marker JSON byte-identical to the pre-token format.
_cosh_load_handoff_token() {
  _COSH_HANDOFF_TOKEN=""
  if [[ -n "${COSH_HANDOFF_REQUEST_FILE:-}"
     && -f "${COSH_HANDOFF_REQUEST_FILE}.token" ]]; then
    _COSH_HANDOFF_TOKEN="$(cat -- "${COSH_HANDOFF_REQUEST_FILE}.token" 2>/dev/null)" || _COSH_HANDOFF_TOKEN=""
  fi
}
# Implicit-pager policy for one approved handoff. The sidecar file is written by
# the Rust transport before the command reaches the shell; the variable set must
# stay identical to NON_INTERACTIVE_PAGER_PREFIX in src/types/shell_handoff.rs.
# Scope is a single command: preexec applies it, precmd restores it, so the
# user's own commands keep their own pager configuration.
# Classifies both value visibility and readonly state. An exported readonly
# pager cannot be assigned, but its export attribute can be removed long enough
# to keep the inherited value out of the handoff command's environment.
_cosh_pager_var_state() {
  local name="$1" dump
  if [[ -z "${!name+x}" ]]; then
    printf unset
    return 0
  fi
  # One subshell per variable, and only on approved-handoff lines: the handoff
  # branch of the preexec marker already forks for _cosh_unwrap_handoff_command.
  dump="$(declare -p "$name" 2>/dev/null)"
  case "$dump" in
    "declare -"*r*" $name="*)
      case "$dump" in
        "declare -"*x*" $name="*)
          printf readonly_export
          ;;
        *)
          printf readonly_shell
          ;;
      esac
      ;;
    "declare -"*x*" $name="*)
      printf export
      ;;
    *)
      printf shell
      ;;
  esac
}
_cosh_apply_handoff_pager_policy() {
  if [[ -z "${COSH_HANDOFF_REQUEST_FILE:-}"
     || ! -f "${COSH_HANDOFF_REQUEST_FILE}.no-pager" ]]; then
    return 0
  fi
  local name state
  for name in PAGER GIT_PAGER MANPAGER SYSTEMD_PAGER; do
    state="$(_cosh_pager_var_state "$name")"
    printf -v "_COSH_${name}_STATE" '%s' "$state"
    printf -v "_COSH_${name}_SAVED" '%s' "${!name-}"
    case "$state" in
      readonly_export)
        export -n "$name"
        ;;
      readonly_shell)
        ;;
      *)
        export "$name=cat"
        ;;
    esac
  done
  _COSH_HANDOFF_PAGER_APPLIED=1
  return 0
}
# Undoes an injection only while it is still exactly what cosh left behind: an
# exported scalar holding `cat`. A handoff command that changed the value
# (export PAGER=less), removed it (unset GIT_PAGER) or only dropped the export
# attribute (export -n PAGER) keeps its own result, because reverting it would
# report success while silently discarding the effect.
_cosh_restore_one_pager_var() {
  local name="$1"
  local state_var="_COSH_${name}_STATE" saved_var="_COSH_${name}_SAVED"
  case "${!state_var-unset}" in
    readonly_export)
      if [[ "${!name-}" == "${!saved_var-}"
         && "$(_cosh_pager_var_state "$name")" == readonly_shell ]]; then
        export "$name"
      fi
      return 0
      ;;
    readonly_shell)
      return 0
      ;;
  esac
  if [[ "${!name-}" != cat
     || "$(_cosh_pager_var_state "$name")" != export ]]; then
    return 0
  fi
  unset "$name"
  case "${!state_var-unset}" in
    shell)
      printf -v "$name" '%s' "${!saved_var-}"
      ;;
    export)
      printf -v "$name" '%s' "${!saved_var-}"
      export "$name"
      ;;
  esac
  return 0
}
_cosh_restore_handoff_pager_policy() {
  if [[ "${_COSH_HANDOFF_PAGER_APPLIED:-0}" != 1 ]]; then
    return 0
  fi
  unset _COSH_HANDOFF_PAGER_APPLIED 2>/dev/null || true
  local name
  for name in PAGER GIT_PAGER MANPAGER SYSTEMD_PAGER; do
    _cosh_restore_one_pager_var "$name"
    unset "_COSH_${name}_STATE" "_COSH_${name}_SAVED" 2>/dev/null || true
  done
  return 0
}
