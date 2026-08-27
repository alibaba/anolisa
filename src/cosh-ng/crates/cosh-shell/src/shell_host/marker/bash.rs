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
readonly _COSH_MARKER_TOKEN="$COSH_MARKER_TOKEN"
unset COSH_MARKER_TOKEN 2>/dev/null || true
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
trap -p DEBUG > "${COSH_RECOVERY_REQUEST_FILE:-/tmp/cosh-recovery}.user-debug-trap" 2>/dev/null || true
trap - DEBUG
_COSH_AI_ENABLED="$_COSH_SESSION_AI_ENABLED"
readonly _COSH_AI_ENABLED
_cosh_assistance_enabled() {
  local status restore_xtrace=0
  if [[ $- == *x* ]]; then
    restore_xtrace=1
    set +x
  fi
  [[ -n "${COSH_ASSISTANCE_STATE_FILE:-}"
     && -f "$COSH_ASSISTANCE_STATE_FILE" ]]
  status=$?
  (( restore_xtrace == 1 )) && set -x
  return "$status"
}
_cosh_ai_enabled() {
  [[ "${_COSH_AI_ENABLED:-1}" == 1 ]] && _cosh_assistance_enabled
}
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
case ":${HISTCONTROL:-}:" in
  *:ignorespace:*|*:ignoreboth:*) ;;
  *) HISTCONTROL="${HISTCONTROL:+$HISTCONTROL:}ignorespace" ;;
esac
_COSH_USER_HISTORY_ENABLED=0
[[ -o history ]] && _COSH_USER_HISTORY_ENABLED=1
set +o history
_COSH_AT_PROMPT=0
_COSH_ATTEMPT_GENERATION=0
_COSH_ATTEMPT_ACTIVE=0
_COSH_ATTEMPT_INPUT=
_COSH_ATTEMPT_TOKEN=
_COSH_ATTEMPT_TOKEN_FINGERPRINT=
_COSH_ATTEMPT_SENSITIVE=0
_COSH_ATTEMPT_UNSAFE=0
_COSH_ATTEMPT_EXPANSION_DRIFT=0
_COSH_ATTEMPT_SUBSHELL=
_COSH_WRAPPER_ID="${COSH_SESSION_ID}:${_COSH_MARKER_TOKEN}"
_cosh_apply_internal_recovery() {
  if [[ -z "${COSH_RECOVERY_REQUEST_FILE:-}" || ! -f "$COSH_RECOVERY_REQUEST_FILE" ]]; then
    return 0
  fi
  rm -f -- "$COSH_RECOVERY_REQUEST_FILE" 2>/dev/null || true
  stty echo icanon isig iexten opost 2>/dev/null || true
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
  local restore_xtrace=0
  if [[ $- == *x* ]]; then
    restore_xtrace=1
    set +x
  fi
  printf '\033]1337;COSH;{"event":"history_file","token":"%s","session_id":"%s","history_file":"%s"}\a' \
    "$(_cosh_json_escape "$_COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$(_cosh_json_escape "$history_file")"
  local marker_status=$?
  (( restore_xtrace == 1 )) && set -x
  return "$marker_status"
}
_cosh_maybe_emit_native_history_file_marker() {
  local history_file last_history_file=""
  history_file="$(_cosh_native_history_file_path)" || return 0
  local state_file="${COSH_RECOVERY_REQUEST_FILE:-/tmp/cosh-recovery}.history-file"
  IFS= read -r last_history_file < "$state_file" 2>/dev/null || true
  if [[ "$history_file" == "$last_history_file" ]]; then
    return 0
  fi
  if _cosh_emit_native_history_file_marker "$history_file"; then
    printf '%s\n' "$history_file" > "$state_file" 2>/dev/null || true
  fi
}
_cosh_maybe_emit_native_history_file_marker
_cosh_native_history_file_fragment() {
  local history_file last_history_file=""
  history_file="$(_cosh_native_history_file_path)" || return 0
  local state_file="${COSH_RECOVERY_REQUEST_FILE:-/tmp/cosh-recovery}.history-file"
  IFS= read -r last_history_file < "$state_file" 2>/dev/null || true
  if [[ "$history_file" == "$last_history_file" ]]; then
    return 0
  fi
  printf '%s\n' "$history_file" > "$state_file" 2>/dev/null || true
  printf ',\"h\":\"%s\"' "$(_cosh_json_escape "$history_file")"
}
_cosh_now_ms() {
  date +%s000
}
_cosh_history_entry() {
  local saved_fmt="${HISTTIMEFORMAT-}"
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
  local saved_fmt="${HISTTIMEFORMAT-}"
  HISTTIMEFORMAT=
  local entry
  entry="$(builtin history 1 2>/dev/null)"
  HISTTIMEFORMAT="$saved_fmt"
  printf '%s' "$entry" | sed -E 's/^[[:space:]]*[0-9]+[[:space:]]*//'
}
_cosh_last_preexec_history_no() {
  local history_no="${_COSH_LAST_PREEXEC_HISTORY_NO:-}"
  local state_file="${COSH_RECOVERY_REQUEST_FILE:-/tmp/cosh-recovery}.preexec-history-no"
  if [[ -f "$state_file" ]]; then
    IFS= read -r history_no < "$state_file" 2>/dev/null || true
  fi
  [[ "$history_no" =~ ^[0-9]+$ ]] && printf '%s' "$history_no"
}
_cosh_remember_preexec_history_no() {
  local history_no="$1"
  local state_file="${COSH_RECOVERY_REQUEST_FILE:-/tmp/cosh-recovery}.preexec-history-no"
  local state_dir="${state_file%/*}"
  [[ "$history_no" =~ ^[0-9]+$ ]] || return 0
  _COSH_LAST_PREEXEC_HISTORY_NO="$history_no"
  # The relay may remove its private directory before Bash processes `exit`.
  # Avoid letting that teardown race surface as user-visible shell output.
  [[ "$state_dir" != "$state_file" && -d "$state_dir" ]] || return 0
  printf '%s\n' "$history_no" > "$state_file" 2>/dev/null || true
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
_cosh_scrub_sensitive_native_history() {
  [[ -o history ]] || return 0
  local history_entry history_no command
  history_entry="$(_cosh_history_entry)"
  history_no="$(_cosh_history_no "$history_entry")"
  command="$(_cosh_history_command_from_entry "$history_entry")"
  [[ -n "$history_no" && -n "$command" ]] || return 0
  if _cosh_command_has_secret "$command"; then
    builtin history -d "$history_no" 2>/dev/null || true
  fi
}
_cosh_emit_marker() {
  local event="$1"
  local command="$2"
  local exit_status="$3"
  local path_trusted="${4:-false}"
  local restore_xtrace=0
  if [[ $- == *x* ]]; then
    restore_xtrace=1
    set +x
  fi
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
    "$(_cosh_json_escape "$_COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "$(_cosh_json_escape "$command")" \
    "$exit_status" \
    "$(_cosh_json_escape "$PATH")" \
    "$path_trusted" \
    "${_COSH_ATTEMPT_GENERATION:-0}" \
    "$handoff_fragment"
  local marker_status=$?
  (( restore_xtrace == 1 )) && set -x
  return "$marker_status"
}
_cosh_emit_boundary_marker() {
  local exit_status="$1"
  local restore_xtrace=0
  if [[ $- == *x* ]]; then
    restore_xtrace=1
    set +x
  fi
  local handoff_fragment=""
  local history_fragment=""
  if [[ -n "${_COSH_HANDOFF_TOKEN:-}" ]]; then
    handoff_fragment=",\"x\":\"$(_cosh_json_escape "$_COSH_HANDOFF_TOKEN")\""
  fi
  history_fragment="$(_cosh_native_history_file_fragment)"
  # The authenticated compact event is equivalent to precmd + prompt_ready;
  # omitted time and generation default in the parser. It is written directly
  # to the controlling terminal and never participates in Readline's PS1 width.
  printf '\033]1337;COSH;{"e":"p","t":"%s","c":"%s","s":%s%s%s}\a' \
    "$(_cosh_json_escape "$_COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$PWD")" \
    "$exit_status" \
    "$history_fragment" \
    "$handoff_fragment"
  local marker_status=$?
  (( restore_xtrace == 1 )) && set -x
  return "$marker_status"
}
_cosh_emit_intercept_marker() {
  local input="$1"
  local reason="$2"
  local top_level_missing="${3:-false}"
  local sensitive="${4:-false}"
  local restore_xtrace=0
  if [[ $- == *x* ]]; then
    restore_xtrace=1
    set +x
  fi
  local timestamp
  timestamp="$(_cosh_now_ms)"
  printf '\033]1337;COSH;{"event":"intercept","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","command":"%s","reason":"%s","status":0,"generation":%s,"top_level_missing":%s,"sensitive":%s}\a' \
    "$(_cosh_json_escape "$_COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "$(_cosh_json_escape "$input")" \
    "$(_cosh_json_escape "$reason")" \
    "${_COSH_ATTEMPT_GENERATION:-0}" \
    "$top_level_missing" \
    "$sensitive"
  local marker_status=$?
  (( restore_xtrace == 1 )) && set -x
  return "$marker_status"
}
_cosh_emit_top_level_missing_marker() {
  local intent="$1"
  local sensitive="${2:-false}"
  local unsafe="${3:-false}"
  local restore_xtrace=0
  if [[ $- == *x* ]]; then
    restore_xtrace=1
    set +x
  fi
  local timestamp
  timestamp="$(_cosh_now_ms)"
  printf '\033]1337;COSH;{"event":"top_level_missing","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","generation":%s,"proven":true,"intent":"%s","sensitive":%s,"unsafe":%s}\a' \
    "$(_cosh_json_escape "$_COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "${_COSH_ATTEMPT_GENERATION:-0}" \
    "$(_cosh_json_escape "$intent")" \
    "$sensitive" \
    "$unsafe"
  local marker_status=$?
  (( restore_xtrace == 1 )) && set -x
  return "$marker_status"
}
_cosh_should_intercept_unknown() {
  local command="$1"
  _cosh_assistance_enabled || return 1
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
  _cosh_ai_enabled || return 1
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
_COSH_BOUNDED_HANDOFF_COMMAND='_COSH_HANDOFF_DEBUG_TRAP="$(trap -p DEBUG 2>/dev/null)"; _COSH_HANDOFF_RETURN_TRAP="$(trap -p RETURN 2>/dev/null)"; _COSH_HANDOFF_ERR_TRAP="$(trap -p ERR 2>/dev/null)"; trap - DEBUG RETURN ERR 2>/dev/null; _cosh_prepare_staged_handoff && eval -- "$(<"$COSH_HANDOFF_REQUEST_FILE")"; _COSH_HANDOFF_STATUS=$?; eval "unset _COSH_HANDOFF_STATUS _COSH_HANDOFF_DEBUG_TRAP _COSH_HANDOFF_RETURN_TRAP _COSH_HANDOFF_ERR_TRAP ${_COSH_HANDOFF_RETURN_TRAP:+; ${_COSH_HANDOFF_RETURN_TRAP}} ${_COSH_HANDOFF_ERR_TRAP:+; ${_COSH_HANDOFF_ERR_TRAP}} ${_COSH_HANDOFF_DEBUG_TRAP:+; ${_COSH_HANDOFF_DEBUG_TRAP}}; (exit ${_COSH_HANDOFF_STATUS})"'
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
_cosh_replace_handoff_history() {
  if [[ -z "${_COSH_HANDOFF_HISTORY_NO:-}" || -z "${_COSH_HANDOFF_HISTORY_COMMAND+x}" ]]; then
    return 0
  fi
  builtin history -d "$_COSH_HANDOFF_HISTORY_NO" 2>/dev/null || true
  builtin history -s "$_COSH_HANDOFF_HISTORY_COMMAND" 2>/dev/null || true
  unset _COSH_HANDOFF_HISTORY_NO _COSH_HANDOFF_HISTORY_COMMAND 2>/dev/null || true
}
_cosh_prepare_staged_handoff() {
  trap - DEBUG RETURN ERR 2>/dev/null || true
  local command history_entry history_no history_command display_command
  if [[ -z "${COSH_HANDOFF_REQUEST_FILE:-}" || ! -r "$COSH_HANDOFF_REQUEST_FILE" ]]; then
    return 127
  fi
  command="$(cat -- "$COSH_HANDOFF_REQUEST_FILE" 2>/dev/null)" || return 127
  [[ -n "$command" ]] || return 127

  _COSH_HANDOFF_TOKEN=""
  _COSH_HANDOFF_ACTIVE=1
  _cosh_load_handoff_token
  _cosh_apply_handoff_pager_policy
  history_entry="$(_cosh_history_entry)"
  history_no="$(_cosh_history_no "$history_entry")"
  history_command="$(_cosh_history_command_from_entry "$history_entry")"
  if [[ "$history_command" == "$_COSH_BOUNDED_HANDOFF_COMMAND" ]]; then
    builtin history -d "$history_no" 2>/dev/null || true
  fi
  display_command="$command"
  if _cosh_command_has_secret "$command"; then
    display_command="<redacted sensitive command>"
  elif [[ -o history ]]; then
    builtin history -s "$command" 2>/dev/null || true
    history_entry="$(_cosh_history_entry)"
    _cosh_remember_preexec_history_no "$(_cosh_history_no "$history_entry")"
  fi
  _COSH_ATTEMPT_GENERATION=$((_COSH_ATTEMPT_GENERATION + 1))
  _cosh_emit_marker "preexec" "$display_command" 0 false
  return 0
}
_cosh_begin_attempt() {
  local input="$1"
  local top_token="$2"
  local expansion_drift="${3:-0}"
  local utf8_status
  _COSH_ATTEMPT_GENERATION=$((_COSH_ATTEMPT_GENERATION + 1))
  _COSH_ATTEMPT_ACTIVE=1
  _COSH_ATTEMPT_WRAPPER_ID="$_COSH_WRAPPER_ID"
  _COSH_ATTEMPT_SENSITIVE=0
  _COSH_ATTEMPT_UNSAFE=0
  _COSH_ATTEMPT_EXPANSION_DRIFT="$expansion_drift"
  _COSH_ATTEMPT_SUBSHELL="${BASH_SUBSHELL:-0}"
  _COSH_ATTEMPT_INPUT=
  _COSH_ATTEMPT_TOKEN=
  _COSH_ATTEMPT_TOKEN_FINGERPRINT=
  if _cosh_command_has_secret "$input"; then
    _COSH_ATTEMPT_SENSITIVE=1
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
    local final_xtrace=0
    if [[ $- == *x* ]]; then
      final_xtrace=1
      set +x
    fi
    _COSH_COMMAND_NOT_FOUND_FINAL_XTRACE="$final_xtrace"
    _COSH_IN_USER_COMMAND_NOT_FOUND=0
    return "$status"
  fi
  printf 'bash: %s: command not found\n' "$1" >&2
  return 127
}
_cosh_user_handler_definition="$(declare -f command_not_found_handle 2>/dev/null || true)"
if [[ -n "$_cosh_user_handler_definition"
   && "$_cosh_user_handler_definition" != "$_COSH_INITIAL_COMMAND_NOT_FOUND_HANDLE" ]]; then
  _cosh_user_handler_definition="${_cosh_user_handler_definition/command_not_found_handle/_cosh_user_command_not_found_handle}"
  # Resume at the start of user code so the private wrapper invocation cannot
  # expose dynamic handler arguments before the user's own trace begins.
  _cosh_user_handler_xtrace_prefix=$'{ \n  if [[ "${_COSH_COMMAND_NOT_FOUND_XTRACE:-0}" == 1 ]]; then set -x; fi\n'
  _cosh_user_handler_definition="${_cosh_user_handler_definition/\{ /$_cosh_user_handler_xtrace_prefix}"
  eval "$_cosh_user_handler_definition"
  _COSH_HAS_USER_COMMAND_NOT_FOUND=1
else
  _COSH_HAS_USER_COMMAND_NOT_FOUND=0
fi
unset _cosh_user_handler_definition _cosh_user_handler_xtrace_prefix \
  _COSH_INITIAL_COMMAND_NOT_FOUND_HANDLE
_cosh_command_not_found_handle() {
  local command="$1"
  shift || true
  if [[ "${_COSH_ATTEMPT_ACTIVE:-0}" != 1 ]]; then
    local history_entry history_command first_word
    history_entry="$(_cosh_history_entry)"
    history_command="$(_cosh_history_command_from_entry "$history_entry")"
    first_word="${history_command%%[[:space:]]*}"
    if [[ -n "$history_command" ]] \
       && ! _cosh_has_leading_alias "$history_command" \
       && _cosh_literal_first_word_matches "$history_command" "$first_word" "$command"; then
      _cosh_begin_attempt "$history_command" "$first_word" 0
    fi
  fi
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
  if [[ "${_COSH_ATTEMPT_SUBSHELL:-}" != "${BASH_SUBSHELL:-0}"
     || "${#FUNCNAME[@]}" != 2
     || "${_COSH_ATTEMPT_EXPANSION_DRIFT:-0}" == 1 ]]; then
    _cosh_delegate_bash_command_not_found "$command" "$@"
    return $?
  fi
  if [[ "${_COSH_ATTEMPT_UNSAFE:-0}" == 1 ]]; then
    local command_fingerprint
    command_fingerprint="$(_cosh_token_fingerprint "$command")"
    if [[ -z "$command_fingerprint"
       || "$command_fingerprint" != "${_COSH_ATTEMPT_TOKEN_FINGERPRINT:-}" ]]; then
      _cosh_delegate_bash_command_not_found "$command" "$@"
      return $?
    fi
    _COSH_ATTEMPT_ACTIVE=0
    local sensitive=false
    [[ "${_COSH_ATTEMPT_SENSITIVE:-0}" == 1 ]] && sensitive=true
    _cosh_emit_top_level_missing_marker "ambiguous" "$sensitive" true
    _cosh_delegate_bash_command_not_found "$command" "$@"
    return $?
  fi
  if [[ -z "$original" ]] \
     || ! _cosh_literal_first_word_matches "$original" "${_COSH_ATTEMPT_TOKEN:-}" "$command" \
     || ! _cosh_arguments_have_no_unquoted_expansion "$original"; then
    _cosh_delegate_bash_command_not_found "$command" "$@"
    return $?
  fi
  if _cosh_is_pending_handoff_command "$original"; then
    _cosh_delegate_bash_command_not_found "$command" "$@"
    return $?
  fi
  _COSH_ATTEMPT_ACTIVE=0
  local sensitive=false
  [[ "${_COSH_ATTEMPT_SENSITIVE:-0}" == 1 ]] && sensitive=true
  local reason
  if reason="$(_cosh_should_intercept_unknown "$command" "$original" "$(($# + 1))")"; then
    _cosh_emit_intercept_marker "$original" "$reason" false "$sensitive"
    return 0
  fi
  local intent
  intent="$(_cosh_classify_missing "$original" "$command")"
  if [[ "$intent" == "natural_language" ]] && _cosh_ai_enabled; then
    if [[ "${_COSH_HAS_USER_COMMAND_NOT_FOUND:-0}" == 1 ]]; then
      _cosh_emit_top_level_missing_marker "$intent" "$sensitive" false
      _cosh_delegate_bash_command_not_found "$command" "$@"
      return $?
    fi
    _cosh_emit_intercept_marker "$original" "natural_language" false "$sensitive"
    return 0
  fi
  _cosh_emit_top_level_missing_marker "$intent" "$sensitive" false
  _cosh_delegate_bash_command_not_found "$command" "$@"
  return $?
}
command_not_found_handle() {
  local _COSH_COMMAND_NOT_FOUND_XTRACE=0
  local _COSH_COMMAND_NOT_FOUND_FINAL_XTRACE=0
  if [[ $- == *x* ]]; then
    _COSH_COMMAND_NOT_FOUND_XTRACE=1
    _COSH_COMMAND_NOT_FOUND_FINAL_XTRACE=1
    set +x
  fi
  _cosh_command_not_found_handle "$@"
  local status=$?
  (( _COSH_COMMAND_NOT_FOUND_FINAL_XTRACE == 1 )) && set -x
  return "$status"
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

_cosh_has_leading_alias() {
  local command="$1"
  local rest="$command"
  local word
  [[ "${_COSH_HAS_BASH_ALIASES:-0}" == 1 ]] || return 1
  while [[ "$rest" =~ ^[A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+ ]]; do
    rest="${rest:${#BASH_REMATCH[0]}}"
  done
  word="${rest%%[[:space:]]*}"
  [[ -n "$word" && -n "${BASH_ALIASES[$word]:-}" ]]
}

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

_cosh_bounded_preexec_marker() {
  local history_entry history_no command first_word argc=1 display_command sensitive=false
  history_entry="$(_cosh_history_entry)"
  history_no="$(_cosh_history_no "$history_entry")"
  command="$(_cosh_history_command_from_entry "$history_entry")"
  if [[ -n "${COSH_HANDOFF_REQUEST_FILE:-}" && -f "$COSH_HANDOFF_REQUEST_FILE" ]]; then
    return 0
  fi
  if [[ -z "$history_no" || -z "$command"
     || "$history_no" == "$(_cosh_last_preexec_history_no)" ]]; then
    # PS0 still proves a top-level execution boundary when Bash deliberately
    # omitted the accepted line from history. Keep the ledger complete but do
    # not guess command text from a stale entry or route it to Agent.
    _COSH_ATTEMPT_GENERATION=$((_COSH_ATTEMPT_GENERATION + 1))
    _cosh_emit_marker "preexec" "<redacted untracked command>" 0 false
    return 0
  fi
  _cosh_remember_preexec_history_no "$history_no"
  first_word="${command%%[[:space:]]*}"
  [[ "$command" == *[[:space:]]* ]] && argc=2

  # PS0 is observation-only and cannot suppress execution. Missing commands
  # route through command_not_found_handle; ordinary slash controls stay in
  # the Rust input relay. A slash line that reaches PS0 came from Readline
  # history or editing and is deliberately Shell-owned, so it still needs an
  # observed command boundary.
  local reason
  if [[ "${_COSH_HAS_USER_COMMAND_NOT_FOUND:-0}" != 1 ]] \
     && reason="$(_cosh_should_intercept_unknown "$first_word" "$command" "$argc")"; then
    if [[ "$reason" != slash ]]; then
      return 0
    fi
  fi
  if ! builtin type -t -- "$first_word" >/dev/null 2>&1; then
    local intent
    intent="$(_cosh_classify_missing "$command" "$first_word")"
    if [[ "${_COSH_HAS_USER_COMMAND_NOT_FOUND:-0}" != 1 \
       && "$intent" == natural_language ]] && _cosh_ai_enabled; then
      return 0
    fi
  fi

  display_command="$command"
  if _cosh_is_handoff_wrapper "$command"; then
    display_command="$(_cosh_unwrap_handoff_command "$command")"
  fi
  _cosh_command_has_secret "$display_command" && sensitive=true
  if [[ "$sensitive" == true ]]; then
    display_command="<redacted sensitive command>"
  fi
  _COSH_ATTEMPT_GENERATION="$history_no"
  _cosh_emit_marker "preexec" "$display_command" 0 false
}
_cosh_bounded_prompt_marker() {
  _cosh_precmd_marker "${1:-$?}"
  return 0
}
_cosh_precmd_marker() {
  local status="${1:-$?}"
  _cosh_apply_internal_recovery
  _cosh_scrub_sensitive_native_history
  _cosh_replace_handoff_history
  # Only the handoff's own prompt boundary may clear the staged files: an
  # unrelated command finishing while a handoff is still pending must not
  # destroy the request/token sidecars it is about to consume (#2142 review).
  if [[ "${_COSH_HANDOFF_ACTIVE:-0}" == 1 ]]; then
    _cosh_clear_handoff_request
  fi
  _cosh_restore_handoff_pager_policy
  unset _COSH_HANDOFF_ACTIVE 2>/dev/null || true
  _COSH_ATTEMPT_ACTIVE=0
  # The precmd marker still carries the handoff token (#2142): it closes the
  # same command the preexec claimed. Cleared right after so the following
  # prompt_ready and ordinary markers stay token-free.
  _cosh_emit_boundary_marker "$status"
  unset _COSH_HANDOFF_TOKEN 2>/dev/null || true
  _COSH_AT_PROMPT=1
}
{
  if [[ -n "${COSH_SHELL_ISOLATED:-}" ]]; then
    unset PROMPT_COMMAND
    builtin history -c 2>/dev/null || true
  fi
  _COSH_USER_PS0="${PS0-}"
  _cosh_initial_history_entry="$(_cosh_history_entry)"
  _cosh_remember_preexec_history_no "$(_cosh_history_no "$_cosh_initial_history_entry")"
  unset _cosh_initial_history_entry
  PS0='$(set +x; trap - DEBUG RETURN ERR 2>/dev/null; _cosh_bounded_preexec_marker > /dev/tty)'"$_COSH_USER_PS0"
  _COSH_USER_PROMPT_COMMAND=()
  if [[ -z "${COSH_SHELL_ISOLATED:-}" ]]; then
    if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
      _COSH_USER_PROMPT_COMMAND=("${PROMPT_COMMAND[@]}")
    elif [[ -n "${PROMPT_COMMAND+x}" ]]; then
      _COSH_USER_PROMPT_COMMAND=("$PROMPT_COMMAND")
    fi
  fi
  # Keep user prompt hooks at top level so their shell-state changes persist.
  # Scratch state uses Cosh's reserved namespace and is cleared before traps
  # resume. Bash before 5.1 executes only element zero of an array
  # PROMPT_COMMAND, so those versions receive the same sequence as one scalar
  # command.
  PROMPT_COMMAND=('_COSH_PROMPT_STATUS="$?"; _COSH_PROMPT_XTRACE=0; case "$-" in *x*) _COSH_PROMPT_XTRACE=1; set +x;; esac; _COSH_PROMPT_DEBUG_TRAP="$(trap -p DEBUG 2>/dev/null)"; _COSH_PROMPT_RETURN_TRAP="$(trap -p RETURN 2>/dev/null)"; _COSH_PROMPT_ERR_TRAP="$(trap -p ERR 2>/dev/null)"; trap - DEBUG RETURN ERR 2>/dev/null; eval "unset _COSH_PROMPT_DEBUG_TRAP _COSH_PROMPT_RETURN_TRAP _COSH_PROMPT_ERR_TRAP ${_COSH_PROMPT_RETURN_TRAP:+; ${_COSH_PROMPT_RETURN_TRAP}} ${_COSH_PROMPT_ERR_TRAP:+; ${_COSH_PROMPT_ERR_TRAP}} ${_COSH_PROMPT_DEBUG_TRAP:+; ${_COSH_PROMPT_DEBUG_TRAP}}"; if (( _COSH_PROMPT_XTRACE == 1 )); then unset _COSH_PROMPT_XTRACE; set -x; else unset _COSH_PROMPT_XTRACE; fi')
  PROMPT_COMMAND+=("${_COSH_USER_PROMPT_COMMAND[@]}")
  PROMPT_COMMAND+=('_COSH_PROMPT_XTRACE=0; case "$-" in *x*) _COSH_PROMPT_XTRACE=1; set +x;; esac; _COSH_PROMPT_DEBUG_TRAP="$(trap -p DEBUG 2>/dev/null)"; _COSH_PROMPT_RETURN_TRAP="$(trap -p RETURN 2>/dev/null)"; _COSH_PROMPT_ERR_TRAP="$(trap -p ERR 2>/dev/null)"; trap - DEBUG RETURN ERR 2>/dev/null; _cosh_bounded_prompt_marker "$_COSH_PROMPT_STATUS" > /dev/tty; eval "unset _COSH_PROMPT_STATUS _COSH_PROMPT_DEBUG_TRAP _COSH_PROMPT_RETURN_TRAP _COSH_PROMPT_ERR_TRAP ${_COSH_PROMPT_RETURN_TRAP:+; ${_COSH_PROMPT_RETURN_TRAP}} ${_COSH_PROMPT_ERR_TRAP:+; ${_COSH_PROMPT_ERR_TRAP}} ${_COSH_PROMPT_DEBUG_TRAP:+; ${_COSH_PROMPT_DEBUG_TRAP}}"; if (( _COSH_PROMPT_XTRACE == 1 )); then unset _COSH_PROMPT_XTRACE; set -x; else unset _COSH_PROMPT_XTRACE; fi')
  if (( BASH_VERSINFO[0] < 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] < 1) )); then
    _COSH_PROMPT_COMMAND_SCALAR="${PROMPT_COMMAND[0]}"
    if [[ -n "${_COSH_USER_PROMPT_COMMAND[0]-}" ]]; then
      _COSH_PROMPT_COMMAND_SCALAR+=$'\n'"${_COSH_USER_PROMPT_COMMAND[0]}"
    fi
    _COSH_PROMPT_COMMAND_SCALAR+=$'\n'"${PROMPT_COMMAND[${#PROMPT_COMMAND[@]}-1]}"
    unset PROMPT_COMMAND
    PROMPT_COMMAND="$_COSH_PROMPT_COMMAND_SCALAR"
    unset _COSH_PROMPT_COMMAND_SCALAR
  fi
  if [[ "${_COSH_USER_HISTORY_ENABLED:-0}" == 1 ]]; then
    set -o history
  fi
  IFS= read -r _COSH_USER_DEBUG_TRAP < "${COSH_RECOVERY_REQUEST_FILE:-/tmp/cosh-recovery}.user-debug-trap" || true
  rm -f -- "${COSH_RECOVERY_REQUEST_FILE:-/tmp/cosh-recovery}.user-debug-trap" 2>/dev/null || true
  if [[ -n "${_COSH_USER_DEBUG_TRAP:-}" ]]; then
    eval "$_COSH_USER_DEBUG_TRAP"
  fi
}
"#
}
