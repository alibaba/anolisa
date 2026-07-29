// Owner: shell_host (bash marker script). Emitted protocol must stay
// byte-identical to the pre-split marker.rs; golden coverage lives in
// osc_tests.rs and tests/shell_host/marker.rs.
pub(in crate::shell_host) fn bash_marker_script() -> &'static str {
    r#"
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
  export HISTCONTROL=
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
_cosh_now_ms() {
  date +%s000
}
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
  printf '\033]1337;COSH;{"event":"%s","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","command":"%s","status":%s,"path":"%s","path_trusted":%s,"generation":%s}\a' \
    "$(_cosh_json_escape "$event")" \
    "$(_cosh_json_escape "$COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "$(_cosh_json_escape "$command")" \
    "$exit_status" \
    "$(_cosh_json_escape "$PATH")" \
    "$path_trusted" \
    "${_COSH_ATTEMPT_GENERATION:-0}"
}
_cosh_emit_intercept_marker() {
  local input="$1"
  local reason="$2"
  local top_level_missing="${3:-false}"
  local timestamp
  timestamp="$(_cosh_now_ms)"
  printf '\033]1337;COSH;{"event":"intercept","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","command":"%s","reason":"%s","status":0,"generation":%s,"top_level_missing":%s}\a' \
    "$(_cosh_json_escape "$COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "$(_cosh_json_escape "$input")" \
    "$(_cosh_json_escape "$reason")" \
    "${_COSH_ATTEMPT_GENERATION:-0}" \
    "$top_level_missing"
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
  case "$command" in
    /about|/agent|/allow|/answer|/approval-mode|/approve|/audit|/auth|/cancel|/clear|/config|/copy|/debug|/deny|/details|/draft|/explain|/extensions|/health|/help|/hooks|/mcp|/mode|/new|/recommendations|/resume|/select|/send-to-shell|/session|/shell|/skills|/stats|/status)
      printf '%s' "slash"
      return 0
      ;;
  esac
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
    /about|/agent|/allow|/answer|/approval-mode|/approve|/audit|/auth|/cancel|/clear|/config|/copy|/debug|/deny|/details|/draft|/explain|/extensions|/health|/help|/hooks|/mcp|/mode|/new|/recommendations|/resume|/select|/send-to-shell|/session|/shell|/skills|/stats|/status)
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
# pre-fix behavior.
_cosh_should_intercept_missing_path() {
  local first_word="$1"
  local command="$2"
  [[ "$first_word" == */* ]] || return 1
  [[ "${_COSH_AI_ENABLED:-1}" == 1 ]] || return 1
  ! _cosh_command_has_secret "$command" || return 1
  _cosh_path_provably_missing "$first_word" || return 1
  local intent
  intent="$(_cosh_classify_missing "$command" "$first_word" missing_path)"
  [[ "$intent" == "natural_language" ]]
}
_COSH_HANDOFF_PREFIX='COSH_SHELL_HANDOFF_BYPASS=1 '
_cosh_is_handoff_wrapper() {
  case "$1" in
    "$_COSH_HANDOFF_PREFIX"*)
      return 0
      ;;
  esac
  return 1
}
_cosh_unwrap_handoff_command() {
  local command="$1"
  printf '%s' "${command#$_COSH_HANDOFF_PREFIX}"
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
}
_cosh_replace_handoff_history() {
  if [[ -z "${_COSH_HANDOFF_HISTORY_NO:-}" || -z "${_COSH_HANDOFF_HISTORY_COMMAND+x}" ]]; then
    return 0
  fi
  builtin history -d "$_COSH_HANDOFF_HISTORY_NO" 2>/dev/null || true
  builtin history -s "$_COSH_HANDOFF_HISTORY_COMMAND" 2>/dev/null || true
  unset _COSH_HANDOFF_HISTORY_NO _COSH_HANDOFF_HISTORY_COMMAND 2>/dev/null || true
}
_cosh_begin_attempt() {
  local input="$1"
  local top_token="$2"
  local utf8_status
  _COSH_ATTEMPT_GENERATION=$((_COSH_ATTEMPT_GENERATION + 1))
  _COSH_ATTEMPT_ACTIVE=1
  _COSH_ATTEMPT_WRAPPER_ID="$_COSH_WRAPPER_ID"
  _COSH_ATTEMPT_SENSITIVE=0
  _COSH_ATTEMPT_UNSAFE=0
  _COSH_ATTEMPT_INPUT=
  _COSH_ATTEMPT_TOKEN=
  _COSH_ATTEMPT_TOKEN_FINGERPRINT=
  if _cosh_command_has_secret "$input"; then
    _COSH_ATTEMPT_SENSITIVE=1
    _COSH_ATTEMPT_TOKEN_FINGERPRINT="$(_cosh_token_fingerprint "$top_token")" || _COSH_ATTEMPT_ACTIVE=0
    return 0
  fi
  _cosh_utf8_han_status "$input"
  utf8_status=$?
  if (( utf8_status == 2 )); then
    _COSH_ATTEMPT_UNSAFE=1
    _COSH_ATTEMPT_TOKEN_FINGERPRINT="$(_cosh_token_fingerprint "$top_token")" || _COSH_ATTEMPT_ACTIVE=0
    return 0
  fi
  _COSH_ATTEMPT_INPUT="$input"
  _COSH_ATTEMPT_TOKEN="$top_token"
}
_cosh_token_fingerprint() {
  local result
  result="$(printf '%s\n' "$1" | command cksum 2>/dev/null)" || return 1
  printf '%s' "${result%% *}"
}
_cosh_delegate_bash_command_not_found() {
  if [[ "${_COSH_IN_USER_COMMAND_NOT_FOUND:-0}" == 1 ]]; then
    printf 'bash: %s: command not found\n' "$1" >&2
    return 127
  fi
  if [[ "${_COSH_HAS_USER_COMMAND_NOT_FOUND:-0}" == 1 ]]; then
    _COSH_IN_USER_COMMAND_NOT_FOUND=1
    _cosh_user_command_not_found_handle "$@"
    local status=$?
    _COSH_IN_USER_COMMAND_NOT_FOUND=0
    return "$status"
  fi
  printf 'bash: %s: command not found\n' "$1" >&2
  return 127
}
_cosh_user_handler_definition="$(declare -f command_not_found_handle 2>/dev/null || true)"
if [[ -n "$_cosh_user_handler_definition"
   && "$_cosh_user_handler_definition" != "$_COSH_INITIAL_COMMAND_NOT_FOUND_HANDLE" ]]; then
  eval "${_cosh_user_handler_definition/command_not_found_handle/_cosh_user_command_not_found_handle}"
  _COSH_HAS_USER_COMMAND_NOT_FOUND=1
else
  _COSH_HAS_USER_COMMAND_NOT_FOUND=0
fi
unset _cosh_user_handler_definition _COSH_INITIAL_COMMAND_NOT_FOUND_HANDLE
command_not_found_handle() {
  local command="$1"
  shift || true
  local original="${_COSH_ATTEMPT_INPUT:-}"
  if [[ "${_COSH_HANDOFF_ACTIVE:-0}" == 1 ]]; then
    _cosh_delegate_bash_command_not_found "$command" "$@"
    return $?
  fi
  if [[ "${_COSH_ATTEMPT_ACTIVE:-0}" != 1
     || "${_COSH_ATTEMPT_WRAPPER_ID:-}" != "$_COSH_WRAPPER_ID" ]]; then
    _cosh_delegate_bash_command_not_found "$command" "$@"
    return $?
  fi
  if [[ "${_COSH_ATTEMPT_SENSITIVE:-0}" == 1 || "${_COSH_ATTEMPT_UNSAFE:-0}" == 1 ]]; then
    local command_fingerprint
    command_fingerprint="$(_cosh_token_fingerprint "$command")"
    if [[ -z "$command_fingerprint"
       || "$command_fingerprint" != "${_COSH_ATTEMPT_TOKEN_FINGERPRINT:-}" ]]; then
      _cosh_delegate_bash_command_not_found "$command" "$@"
      return $?
    fi
    _COSH_ATTEMPT_ACTIVE=0
    local sensitive=false
    local unsafe=false
    [[ "${_COSH_ATTEMPT_SENSITIVE:-0}" == 1 ]] && sensitive=true
    [[ "${_COSH_ATTEMPT_UNSAFE:-0}" == 1 ]] && unsafe=true
    _cosh_emit_top_level_missing_marker "ambiguous" "$sensitive" "$unsafe"
    _cosh_delegate_bash_command_not_found "$command" "$@"
    return $?
  fi
  if [[ "${_COSH_ATTEMPT_TOKEN:-}" != "$command" || -z "$original" ]]; then
    _cosh_delegate_bash_command_not_found "$command" "$@"
    return $?
  fi
  if _cosh_is_pending_handoff_command "$original"; then
    _cosh_delegate_bash_command_not_found "$command" "$@"
    return $?
  fi
  _COSH_ATTEMPT_ACTIVE=0
  local reason
  if reason="$(_cosh_should_intercept_unknown "$command" "$original" "$(($# + 1))")"; then
    _cosh_emit_intercept_marker "$original" "$reason"
    return 0
  fi
  local intent
  intent="$(_cosh_classify_missing "$original" "$command")"
  if [[ "$intent" == "natural_language" && "${_COSH_AI_ENABLED:-1}" == 1 ]]; then
    if [[ "${_COSH_HAS_USER_COMMAND_NOT_FOUND:-0}" == 1 ]]; then
      _cosh_emit_top_level_missing_marker "$intent" false false
      _cosh_delegate_bash_command_not_found "$command" "$@"
      return $?
    fi
    _cosh_emit_intercept_marker "$original" "natural_language" true
    return 0
  fi
  _cosh_emit_top_level_missing_marker "$intent" false false
  _cosh_delegate_bash_command_not_found "$command" "$@"
  return $?
}

# Expands the leading command word of a history line following bash alias
# rules and stores the whitespace-compacted result in _COSH_EXPANDED_COMPACT
# (out-parameter form: $(...) would fork a subshell inside the DEBUG trap).
# Leaves _COSH_EXPANDED_COMPACT empty when no alias applies. Builtin-only:
# no subprocess, no fork.
#
# BASH_ALIASES requires bash 4+. On bash 3.x the associative array does not
# exist and ${BASH_ALIASES[$word]} would evaluate the subscript as an
# arithmetic expression (breaking on words like "/help"), so the capability
# is probed once at load time and the helper degrades to the pre-fix guard.
_COSH_HAS_BASH_ALIASES=0
if (( ${BASH_VERSINFO[0]:-0} >= 4 )); then
  _COSH_HAS_BASH_ALIASES=1
fi

_cosh_compact_alias_expanded() {
  local command="$1" expanded=0 guard=0 prefix rest word expansion done_prefix=""
  _COSH_EXPANDED_COMPACT=""
  if [[ "${_COSH_HAS_BASH_ALIASES:-0}" != 1 ]]; then
    return 0
  fi
  # Depth cap: deeper alias chains are vanishingly rare in practice; on
  # overflow the compact expansion stays incomplete, the stale-history guard
  # reports a mismatch, and the untracked fallback closes the handoff with
  # degraded evidence instead of deadlocking.
  while (( guard++ < 10 )); do
    prefix=""
    rest="$command"
    # Skip leading NAME=VALUE assignments (covers handoff wrapper prefixes);
    # bash still alias-expands the command word after assignments.
    while [[ "$rest" =~ ^[A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+ ]]; do
      prefix+="${BASH_REMATCH[0]}"
      rest="${rest:${#BASH_REMATCH[0]}}"
    done
    word="${rest%%[[:space:]]*}"
    expansion="${BASH_ALIASES[$word]:-}"
    if [[ -z "$expansion" ]]; then
      break
    fi
    expanded=1
    command="${prefix}${expansion}${rest:${#word}}"
    # bash stops recursive expansion when the expansion starts with the
    # word being expanded (e.g. ls='ls --color=auto'); the single-round
    # expansion must still be reported. A trailing blank in the alias
    # value makes bash alias-expand the next word as well, so freeze the
    # settled part into done_prefix and keep expanding after it.
    if [[ "${expansion%%[[:space:]]*}" == "$word" ]]; then
      if [[ "$expansion" =~ [[:space:]]$ ]]; then
        done_prefix+="${prefix}${expansion}"
        command="${rest:${#word}}"
        command="${command#"${command%%[![:space:]]*}"}"
        if [[ -z "$command" ]]; then
          break
        fi
        continue
      fi
      break
    fi
    # The same trailing-blank rule applies when the expansion changed the
    # command word: settle the expansion and continue with the next word.
    if [[ "$expansion" =~ [[:space:]]$ ]]; then
      done_prefix+="${prefix}${expansion}"
      command="${rest:${#word}}"
      command="${command#"${command%%[![:space:]]*}"}"
      if [[ -z "$command" ]]; then
        break
      fi
    fi
  done
  if (( expanded )); then
    command="${done_prefix}${command}"
    _COSH_EXPANDED_COMPACT="${command//[[:space:]]/}"
  fi
}

_cosh_preexec_marker() {
  if [[ "${_COSH_SNAPSHOT_DEBUG_TRAP:-0}" == 1 ]]; then
    return 0
  fi
  # Skip during completion — with extdebug the DEBUG trap fires for every
  # internal command bash runs during glob expansion / completion, and the
  # heavy operations below (date subprocess, file I/O) cause noticeable lag.
  # Require COMP_TYPE (only set by bash during programmable completion) in
  # addition to COMP_LINE/COMP_POINT so that residual COMP_LINE values do
  # not permanently silence preexec markers for real commands.
  if [[ -n "${COMP_TYPE:-}" && ( -n "${COMP_LINE:-}" || -n "${COMP_POINT:-}" ) ]]; then
    return 0
  fi
  local active_debug_trap="${_COSH_ACTIVE_DEBUG_TRAP:-}"
  if [[ "${_COSH_IN_PROMPT_COMMAND:-0}" != 1 && "${_COSH_DEBUG_TRAP_MAY_CHANGE:-0}" == 1 ]]; then
    local trap_snapshot_file="${COSH_RECOVERY_REQUEST_FILE:-/tmp/cosh-recovery}.debug-trap"
    trap -p DEBUG > "$trap_snapshot_file" 2>/dev/null || true
    trap - DEBUG
    IFS= read -r active_debug_trap < "$trap_snapshot_file" || true
    rm -f -- "$trap_snapshot_file" 2>/dev/null || true
    _COSH_ACTIVE_DEBUG_TRAP="$active_debug_trap"
    unset _COSH_DEBUG_TRAP_MAY_CHANGE
  fi
  trap - DEBUG
  local path_trusted=false
  if [[ "$active_debug_trap" == "trap -- '_cosh_preexec_marker' DEBUG" ]]; then
    path_trusted=true
  fi
  if [[ -n "${_COSH_OLD_DEBUG_TRAP:-}" ]]; then
    eval "$_COSH_OLD_DEBUG_TRAP" 2>/dev/null || true
  fi
  if [[ "${_COSH_IN_PROMPT_COMMAND:-0}" == 1 ]]; then
    eval "$active_debug_trap" 2>/dev/null || true
    return 0
  fi
  if [[ "${_COSH_AT_PROMPT:-0}" == 1 ]]; then
    local history_entry
    local history_no
    local command
    history_entry="$(_cosh_history_entry)"
    history_no="$(_cosh_history_no "$history_entry")"
    command="$(_cosh_history_command_from_entry "$history_entry")"
    local compact_command="${command//[[:space:]]/}"
    local compact_bash_command="${BASH_COMMAND//[[:space:]]/}"
    # Stale-history guard, alias aware: BASH_COMMAND is alias-expanded while
    # history keeps the raw text, so a raw mismatch must be re-checked against
    # the alias-expanded history line before treating history as stale
    # (otherwise every aliased command, e.g. ls='ls --color=auto', loses its
    # preexec marker and an approved shell handoff can never close).
    _COSH_EXPANDED_COMPACT=""
    if [[ -n "$compact_command" && "$compact_bash_command" != *"$compact_command"* && "$compact_command" != *"$compact_bash_command"* ]]; then
      _cosh_compact_alias_expanded "$command"
    fi
    if [[ -n "${BASH_COMMAND:-}" && ( -z "$compact_command" || ( "$compact_bash_command" != *"$compact_command"* && "$compact_command" != *"$compact_bash_command"* && ( -z "$_COSH_EXPANDED_COMPACT" || ( "$compact_bash_command" != *"$_COSH_EXPANDED_COMPACT"* && "$_COSH_EXPANDED_COMPACT" != *"$compact_bash_command"* ) ) ) ) ]]; then
      local fallback_command="$BASH_COMMAND"
      local fallback_first_word="$fallback_command"
      local fallback_argc=1
      if [[ "$fallback_command" == *[[:space:]]* ]]; then
        fallback_first_word="${fallback_command%%[[:space:]]*}"
        fallback_argc=2
      fi
      local fallback_reason
      if fallback_reason="$(_cosh_should_intercept_unknown "$fallback_first_word" "$fallback_command" "$fallback_argc")"; then
        _cosh_emit_intercept_marker "$fallback_command" "$fallback_reason"
        _COSH_AT_PROMPT=0
        eval "$active_debug_trap" 2>/dev/null || true
        return 1
      fi
      if _cosh_should_intercept_missing_path "$fallback_first_word" "$fallback_command"; then
        _cosh_emit_intercept_marker "$fallback_command" "natural_language"
        _COSH_AT_PROMPT=0
        eval "$active_debug_trap" 2>/dev/null || true
        return 1
      fi
      eval "$active_debug_trap" 2>/dev/null || true
      return 0
    fi
    if [[ -n "$history_no" && -n "$command" ]]; then
      _COSH_ATTEMPT_ACTIVE=0
      _COSH_ATTEMPT_SENSITIVE=0
      _COSH_ATTEMPT_UNSAFE=0
      local display_command="$command"
      if _cosh_is_handoff_wrapper "$command"; then
        display_command="$(_cosh_unwrap_handoff_command "$command")"
        _COSH_HANDOFF_ACTIVE=1
        _COSH_HANDOFF_HISTORY_NO="$history_no"
      elif _cosh_is_pending_handoff_command "$command"; then
        _COSH_HANDOFF_ACTIVE=1
      else
        _cosh_clear_handoff_request
        unset _COSH_HANDOFF_ACTIVE 2>/dev/null || true
        unset _COSH_HANDOFF_HISTORY_NO _COSH_HANDOFF_HISTORY_COMMAND 2>/dev/null || true
        local first_word="$command"
        local argc=1
        if [[ "$command" == *[[:space:]]* ]]; then
          first_word="${command%%[[:space:]]*}"
          argc=2
        fi
        local reason
        if reason="$(_cosh_should_intercept_unknown "$first_word" "$command" "$argc")"; then
          # Intercepted lines return 1 before the secret redaction below
          # ever runs, so scrub credential-bearing entries here or the raw
          # text would persist in native history (routed slash submissions
          # enter history via readline before the trap fires).
          if _cosh_command_has_secret "$command"; then
            builtin history -d "$history_no" 2>/dev/null || true
          fi
          _cosh_emit_intercept_marker "$command" "$reason"
          _COSH_AT_PROMPT=0
          eval "$active_debug_trap" 2>/dev/null || true
          return 1
        fi
        if _cosh_should_intercept_missing_path "$first_word" "$command"; then
          _cosh_emit_intercept_marker "$command" "natural_language"
          _COSH_AT_PROMPT=0
          eval "$active_debug_trap" 2>/dev/null || true
          return 1
        fi
        _cosh_begin_attempt "$command" "$first_word"
      fi
      if [[ "$command" == trap*DEBUG* ]]; then
        _COSH_DEBUG_TRAP_MAY_CHANGE=1
      fi
      if [[ "${_COSH_ATTEMPT_SENSITIVE:-0}" == 1
         || "${_COSH_ATTEMPT_UNSAFE:-0}" == 1 ]] \
         || _cosh_command_has_secret "$display_command"; then
        if [[ -z "${_COSH_HANDOFF_HISTORY_NO:-}" ]]; then
          builtin history -d "$history_no" 2>/dev/null || true
        fi
        display_command="<redacted sensitive command>"
      fi
      if [[ -n "${_COSH_HANDOFF_HISTORY_NO:-}" ]]; then
        _COSH_HANDOFF_HISTORY_COMMAND="$display_command"
        _cosh_replace_handoff_history
      fi
      _cosh_emit_marker "preexec" "$display_command" 0 "$path_trusted"
    fi
    _COSH_AT_PROMPT=0
  fi
  eval "$active_debug_trap" 2>/dev/null || true
  return 0
}
_cosh_precmd_marker() {
  local status="${1:-$?}"
  _cosh_apply_internal_recovery
  _cosh_replace_handoff_history
  _cosh_clear_handoff_request
  unset _COSH_HANDOFF_ACTIVE 2>/dev/null || true
  _COSH_ATTEMPT_ACTIVE=0
  _cosh_emit_marker "precmd" "" "$status" false
  _COSH_AT_PROMPT=1
}
# Helper frame so a hook containing `return` unwinds here instead of
# skipping the extdebug restore in _cosh_run_user_prompt_command.
_cosh_eval_user_prompt_hook() {
  eval "$1"
}
_cosh_run_user_prompt_command() {
  local status="$1"
  if [[ -z "${_COSH_USER_PROMPT_COMMAND+x}" ]]; then
    return "$status"
  fi
  # User prompt hooks run with extdebug off: while it is on, bash re-execs
  # shebang-less scripts with --debugger (ENOEXEC fallback), and hosts
  # without the bashdb package print debugger startup failures at every
  # prompt (Alinux points PROMPT_COMMAND at the shebang-less
  # /etc/sysconfig/bash-prompt-history audit script). extdebug is only
  # needed for DEBUG trap return-1 semantics during real command dispatch,
  # which prompt-hook eval does not exercise.
  shopt -u extdebug 2>/dev/null || true
  # shopt -u extdebug also clears the errtrace/functrace flags it implied
  # while enabled. Re-assert them so hooks keep the baseline trap
  # inheritance semantics of this session (ERR/DEBUG traps reaching hook
  # functions); neither flag triggers the debugger re-exec.
  set -E 2>/dev/null || true
  set -T 2>/dev/null || true
  if [[ "${_COSH_USER_PROMPT_COMMAND_IS_ARRAY:-0}" == 1 ]]; then
    local _cosh_prompt_command
    for _cosh_prompt_command in "${_COSH_USER_PROMPT_COMMAND[@]}"; do
      _cosh_eval_user_prompt_hook "$_cosh_prompt_command"
    done
  elif [[ -n "${_COSH_USER_PROMPT_COMMAND:-}" ]]; then
    _cosh_eval_user_prompt_hook "$_COSH_USER_PROMPT_COMMAND"
  fi
  shopt -s extdebug 2>/dev/null || true
  return "$status"
}
_cosh_prompt_command() {
  local status=$?
  _COSH_IN_PROMPT_COMMAND=1
  _cosh_maybe_emit_native_history_file_marker
  _cosh_precmd_marker "$status"
  _cosh_run_user_prompt_command "$status"
  _cosh_maybe_emit_native_history_file_marker
  if [[ -n "${_COSH_USER_PROMPT_COMMAND+x}" ]]; then
    local trap_snapshot_file="${COSH_RECOVERY_REQUEST_FILE:-/tmp/cosh-recovery}.debug-trap"
    _COSH_SNAPSHOT_DEBUG_TRAP=1
    trap -p DEBUG > "$trap_snapshot_file" 2>/dev/null || true
    unset _COSH_SNAPSHOT_DEBUG_TRAP
    IFS= read -r _COSH_ACTIVE_DEBUG_TRAP < "$trap_snapshot_file" || _COSH_ACTIVE_DEBUG_TRAP=""
    rm -f -- "$trap_snapshot_file" 2>/dev/null || true
  fi
  # The next visible shell bytes are the prompt paint. Keep this marker after
  # every user PROMPT_COMMAND so its output cannot masquerade as the prompt.
  _cosh_emit_marker "prompt_ready" "" "$status" false
  _COSH_IN_PROMPT_COMMAND=0
  return "$status"
}
# If BASHOPTS arrived exported from the login environment it stays exported
# (readonly keeps the -x attribute). Drop the export attribute *before*
# enabling extdebug: the user rcfile has already run, so its DEBUG trap is
# live and fires between these two commands — a child bash spawned there
# would otherwise inherit the exported extdebug and fail debugger startup
# (bashdb). Dropping -x only removes the export attribute; imported options
# stay effective in this shell and the guard keeps a refusing bash fail-safe.
export -n BASHOPTS 2>/dev/null || true
shopt -s extdebug 2>/dev/null || true
_COSH_OLD_DEBUG_TRAP="$(trap -p DEBUG 2>/dev/null | sed "s/^trap -- '\\(.*\\)' DEBUG$/\\1/" || true)"
_COSH_ACTIVE_DEBUG_TRAP="trap -- '_cosh_preexec_marker' DEBUG"
trap '_cosh_preexec_marker' DEBUG
if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
  _COSH_USER_PROMPT_COMMAND_IS_ARRAY=1
  _COSH_USER_PROMPT_COMMAND=("${PROMPT_COMMAND[@]}")
elif [[ -n "${PROMPT_COMMAND+x}" ]]; then
  _COSH_USER_PROMPT_COMMAND_IS_ARRAY=0
  _COSH_USER_PROMPT_COMMAND="$PROMPT_COMMAND"
else
  unset _COSH_USER_PROMPT_COMMAND
  _COSH_USER_PROMPT_COMMAND_IS_ARRAY=0
fi
# Replace wholesale: assigning over an array PROMPT_COMMAND (bash >= 5.1)
# only overwrites element 0, and surviving user elements would keep running
# natively at every prompt, outside the extdebug guard in
# _cosh_run_user_prompt_command.
#
# Deliberately no top-level extdebug re-enable here: a hook that installs a
# DEBUG trap ending in `return` unwinds every function frame, and with
# extdebug back on that trap's top-level failure status would make bash
# skip every subsequent command — bricking the session. With extdebug off
# the session degrades to native-bash behavior (marker interception idles)
# and the in-function restore self-heals on the first prompt after the
# user clears the trap.
unset PROMPT_COMMAND
PROMPT_COMMAND=_cosh_prompt_command
if [[ -n "${COSH_SHELL_ISOLATED:-}" ]]; then
  builtin history -c 2>/dev/null || true
fi
"#
}
