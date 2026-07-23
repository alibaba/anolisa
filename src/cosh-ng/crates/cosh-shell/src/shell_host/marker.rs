pub(super) fn bash_marker_script() -> &'static str {
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

# ── Source user startup files (native mode) ──
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

# ── Mode-dependent shell settings ──
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
_COSH_LAST_HISTORY_NO=0
_COSH_LAST_HISTORY_COMMAND=
_COSH_IN_PROMPT_COMMAND=0
_COSH_LAST_NATIVE_HISTORY_FILE=

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

  printf '\033]1337;COSH;{"event":"%s","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","command":"%s","status":%s,"path":"%s","path_trusted":%s}\a' \
    "$(_cosh_json_escape "$event")" \
    "$(_cosh_json_escape "$COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "$(_cosh_json_escape "$command")" \
    "$exit_status" \
    "$(_cosh_json_escape "$PATH")" \
    "$path_trusted"
}

_cosh_emit_intercept_marker() {
  local input="$1"
  local reason="$2"
  local timestamp
  timestamp="$(_cosh_now_ms)"

  printf '\033]1337;COSH;{"event":"intercept","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","command":"%s","reason":"%s","status":0}\a' \
    "$(_cosh_json_escape "$COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "$(_cosh_json_escape "$input")" \
    "$(_cosh_json_escape "$reason")"
}

_cosh_has_non_ascii() {
  printf '%s' "$1" | LC_ALL=C grep -q '[^ -~]'
}

_cosh_is_shell_command_prefix() {
  local command="$1"

  case "$command" in
    /*|./*|../*|~/*)
      return 0
      ;;
    [A-Za-z_]*=*)
      return 0
      ;;
    awk|bash|bat|brew|bun|cargo|cat|cd|chmod|chown|cp|curl|docker|du|echo|env|fd|find|git|grep|head|less|ls|make|mkdir|mv|node|npm|npx|nvim|pnpm|printf|ps|pwd|python|python3|rg|rm|sed|sh|sudo|tail|top|touch|tree|vi|vim|yarn)
      return 0
      ;;
  esac

  return 1
}

_cosh_should_intercept_unknown() {
  local command="$1"
  local original="$2"
  local argc="$3"

  case "$command" in
    /agent|/allow|/answer|/approval-mode|/approve|/audit|/auth|/cancel|/clear|/config|/copy|/debug|/deny|/details|/explain|/extensions|/help|/hooks|/mode|/recommendations|/resume|/select|/send-to-shell|/session|/shell|/skills)
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

  if _cosh_has_non_ascii "$original" && ! _cosh_is_shell_command_prefix "$command"; then
    printf '%s' "natural_language"
    return 0
  fi

  if (( argc > 1 )); then
    case "$command" in
      [Pp][Ll][Ee][Aa][Ss][Ee]|[Ee][Xx][Pp][Ll][Aa][Ii][Nn]|[Ww][Hh][Yy]|[Hh][Oo][Ww]|[Ww][Hh][Aa][Tt]|[Ff][Ii][Xx])
        printf '%s' "natural_language"
        return 0
        ;;
    esac
  fi

  return 1
}

_cosh_is_slash_control_candidate() {
  local command="$1"

  case "$command" in
    /agent|/allow|/answer|/approval-mode|/approve|/audit|/auth|/cancel|/clear|/config|/copy|/debug|/deny|/details|/explain|/extensions|/help|/hooks|/mode|/recommendations|/resume|/select|/send-to-shell|/session|/shell|/skills)
      return 0
      ;;
  esac

  return 1
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

command_not_found_handle() {
  local command="$1"
  shift || true
  local original="$command"
  if (($# > 0)); then
    original="$original $*"
  fi

  if [[ "${_COSH_HANDOFF_ACTIVE:-0}" == 1 ]] || _cosh_is_pending_handoff_command "$original"; then
    printf 'bash: %s: command not found\n' "$command" >&2
    return 127
  fi

  local reason
  if reason="$(_cosh_should_intercept_unknown "$command" "$original" "$(($# + 1))")"; then
    _cosh_emit_intercept_marker "$original" "$reason"
    return 0
  fi

  printf 'bash: %s: command not found\n' "$command" >&2
  return 127
}

_cosh_preexec_marker() {
  if [[ "${_COSH_SNAPSHOT_DEBUG_TRAP:-0}" == 1 ]]; then
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
    if [[ -n "${BASH_COMMAND:-}" && ( -z "$compact_command" || ( "$compact_bash_command" != *"$compact_command"* && "$compact_command" != *"$compact_bash_command"* ) ) ]]; then
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
      eval "$active_debug_trap" 2>/dev/null || true
      return 0
    fi
    if [[ -n "$history_no" && -n "$command" && ( "$history_no" != "${_COSH_LAST_HISTORY_NO:-0}" || "$command" != "${_COSH_LAST_HISTORY_COMMAND:-}" ) ]]; then
      _COSH_LAST_HISTORY_NO="$history_no"
      _COSH_LAST_HISTORY_COMMAND="$command"
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
          _cosh_emit_intercept_marker "$command" "$reason"
          _COSH_AT_PROMPT=0
          eval "$active_debug_trap" 2>/dev/null || true
          return 1
        fi
      fi
      if [[ "$command" == trap*DEBUG* ]]; then
        _COSH_DEBUG_TRAP_MAY_CHANGE=1
      fi
      if _cosh_command_has_secret "$display_command"; then
        if [[ -z "${_COSH_HANDOFF_HISTORY_NO:-}" ]]; then
          builtin history -d "$history_no" 2>/dev/null || true
        fi
        display_command="<redacted sensitive command>"
        _COSH_LAST_HISTORY_COMMAND="$display_command"
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
  _cosh_emit_marker "precmd" "" "$status" false
  _COSH_AT_PROMPT=1
}

_cosh_run_user_prompt_command() {
  local status="$1"
  if [[ -z "${_COSH_USER_PROMPT_COMMAND+x}" ]]; then
    return "$status"
  fi

  if [[ "${_COSH_USER_PROMPT_COMMAND_IS_ARRAY:-0}" == 1 ]]; then
    local _cosh_prompt_command
    for _cosh_prompt_command in "${_COSH_USER_PROMPT_COMMAND[@]}"; do
      eval "$_cosh_prompt_command"
    done
  elif [[ -n "${_COSH_USER_PROMPT_COMMAND:-}" ]]; then
    eval "$_COSH_USER_PROMPT_COMMAND"
  fi
  return "$status"
}

_cosh_prompt_command() {
  local status=$?
  _COSH_IN_PROMPT_COMMAND=1
  _cosh_maybe_emit_native_history_file_marker
  _cosh_precmd_marker "$status"
  _cosh_run_user_prompt_command "$status"
  if [[ -n "${_COSH_USER_PROMPT_COMMAND+x}" ]]; then
    local trap_snapshot_file="${COSH_RECOVERY_REQUEST_FILE:-/tmp/cosh-recovery}.debug-trap"
    _COSH_SNAPSHOT_DEBUG_TRAP=1
    trap -p DEBUG > "$trap_snapshot_file" 2>/dev/null || true
    unset _COSH_SNAPSHOT_DEBUG_TRAP
    IFS= read -r _COSH_ACTIVE_DEBUG_TRAP < "$trap_snapshot_file" || _COSH_ACTIVE_DEBUG_TRAP=""
    rm -f -- "$trap_snapshot_file" 2>/dev/null || true
  fi
  _COSH_IN_PROMPT_COMMAND=0
  return "$status"
}

# ── Hook setup (re-set after user rcfile may have overridden) ──
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
PROMPT_COMMAND=_cosh_prompt_command
if [[ -n "${COSH_SHELL_ISOLATED:-}" ]]; then
  builtin history -c 2>/dev/null || true
fi
"#
}

pub(super) fn zsh_marker_script() -> &'static str {
    r#"
if [[ -n "${COSH_OSC_MARKER_LOADED:-}" ]]; then
  return 0 2>/dev/null || exit 0
fi
COSH_OSC_MARKER_LOADED=1

[[ -o interactive ]] || return 0 2>/dev/null || exit 0

export COSH_SESSION_ID="${COSH_SESSION_ID:-cosh-osc-$$}"
export COSH_POC_PS1="${COSH_POC_PS1:-cosh-osc$ }"

# ── Source user startup files (native mode) ──
if [[ -z "${COSH_SHELL_ISOLATED:-}" ]]; then
  if [[ -n "${COSH_ZDOTDIR_ORIG:-}" ]]; then
    _cosh_marker_zdotdir="${ZDOTDIR:-}"
    if [[ -n "$_cosh_marker_zdotdir" && "${HISTFILE:-}" == "$_cosh_marker_zdotdir/.zsh_history" ]]; then
      HISTFILE="${COSH_ZDOTDIR_ORIG}/.zsh_history"
    fi
    export ZDOTDIR="${COSH_ZDOTDIR_ORIG}"
    [[ -f "${COSH_ZDOTDIR_ORIG}/.zshenv" ]] && source "${COSH_ZDOTDIR_ORIG}/.zshenv"
    if [[ "${COSH_LOGIN_SHELL:-}" == "1" ]]; then
      [[ -f "${COSH_ZDOTDIR_ORIG}/.zprofile" ]] && source "${COSH_ZDOTDIR_ORIG}/.zprofile"
      [[ -f "${COSH_ZDOTDIR_ORIG}/.zlogin" ]] && source "${COSH_ZDOTDIR_ORIG}/.zlogin"
    fi
    [[ -f "${COSH_ZDOTDIR_ORIG}/.zshrc" ]] && source "${COSH_ZDOTDIR_ORIG}/.zshrc"
    unset _cosh_marker_zdotdir
  else
    [[ -f ~/.zshenv ]] && source ~/.zshenv
    if [[ "${COSH_LOGIN_SHELL:-}" == "1" ]]; then
      [[ -f ~/.zprofile ]] && source ~/.zprofile
      [[ -f ~/.zlogin ]] && source ~/.zlogin
    fi
    [[ -f ~/.zshrc ]] && source ~/.zshrc
  fi
fi

_cosh_load_native_zsh_history_if_empty() {
  if [[ -n "${COSH_SHELL_ISOLATED:-}" ]]; then
    return 0
  fi
  if [[ -z "${HISTFILE:-}" && -n "${COSH_ZDOTDIR_ORIG:-}" && -r "${COSH_ZDOTDIR_ORIG}/.zsh_history" ]]; then
    HISTFILE="${COSH_ZDOTDIR_ORIG}/.zsh_history"
  fi
  if [[ -z "${HISTFILE:-}" || ! -r "$HISTFILE" ]]; then
    return 0
  fi
  if fc -l 1 >/dev/null 2>&1; then
    return 0
  fi
  fc -R "$HISTFILE" 2>/dev/null || true
}

# ── Mode-dependent shell settings ──
if [[ -z "${COSH_SHELL_ISOLATED:-}" ]]; then
  : # native mode: keep user PS1/PROMPT, HISTFILE, etc.
else
  export PS1="$COSH_POC_PS1"
  export PROMPT="$COSH_POC_PS1"
  export HISTFILE="${COSH_HISTFILE:-/dev/null}"
  HISTSIZE="${COSH_HISTSIZE:-1000}"
  SAVEHIST=0
fi
_cosh_load_native_zsh_history_if_empty
setopt NO_BEEP 2>/dev/null || true
setopt NO_PROMPT_CR 2>/dev/null || true
setopt NO_PROMPT_SP 2>/dev/null || true
unsetopt NOMATCH 2>/dev/null || true

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

_cosh_now_ms() {
  date +%s000
}

_cosh_emit_marker() {
  local event="$1"
  local command="$2"
  local exit_status="$3"
  local path_trusted="${4:-false}"
  local timestamp
  timestamp="$(_cosh_now_ms)"

  printf '\033]1337;COSH;{"event":"%s","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","command":"%s","status":%s,"path":"%s","path_trusted":%s}\a' \
    "$(_cosh_json_escape "$event")" \
    "$(_cosh_json_escape "$COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "$(_cosh_json_escape "$command")" \
    "$exit_status" \
    "$(_cosh_json_escape "$PATH")" \
    "$path_trusted"
}

_cosh_emit_intercept_marker() {
  local input="$1"
  local reason="$2"
  local timestamp
  timestamp="$(_cosh_now_ms)"

  printf '\033]1337;COSH;{"event":"intercept","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","command":"%s","reason":"%s","status":0}\a' \
    "$(_cosh_json_escape "$COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "$(_cosh_json_escape "$input")" \
    "$(_cosh_json_escape "$reason")"
}

_cosh_has_non_ascii() {
  printf '%s' "$1" | LC_ALL=C grep -q '[^ -~]'
}

_cosh_is_shell_command_prefix() {
  local command="$1"

  case "$command" in
    /*|./*|../*|~/*)
      return 0
      ;;
    [A-Za-z_]*=*)
      return 0
      ;;
    awk|bash|bat|brew|bun|cargo|cat|cd|chmod|chown|cp|curl|docker|du|echo|env|fd|find|git|grep|head|less|ls|make|mkdir|mv|node|npm|npx|nvim|pnpm|printf|ps|pwd|python|python3|rg|rm|sed|sh|sudo|tail|top|touch|tree|vi|vim|yarn|zsh)
      return 0
      ;;
  esac

  return 1
}

_cosh_should_intercept_unknown() {
  local command="$1"
  local original="$2"
  local argc="$3"

  case "$command" in
    /agent|/allow|/answer|/approval-mode|/approve|/audit|/auth|/cancel|/clear|/config|/copy|/debug|/deny|/details|/explain|/extensions|/help|/hooks|/mode|/recommendations|/resume|/select|/send-to-shell|/session|/shell|/skills)
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

  if _cosh_has_non_ascii "$original" && ! _cosh_is_shell_command_prefix "$command"; then
    printf '%s' "natural_language"
    return 0
  fi

  if (( argc > 1 )); then
    case "$command" in
      [Pp][Ll][Ee][Aa][Ss][Ee]|[Ee][Xx][Pp][Ll][Aa][Ii][Nn]|[Ww][Hh][Yy]|[Hh][Oo][Ww]|[Ww][Hh][Aa][Tt]|[Ff][Ii][Xx])
        printf '%s' "natural_language"
        return 0
        ;;
    esac
  fi

  return 1
}

_cosh_is_slash_control_candidate() {
  local command="$1"

  case "$command" in
    /agent|/allow|/answer|/approval-mode|/approve|/audit|/auth|/cancel|/clear|/config|/copy|/debug|/deny|/details|/explain|/extensions|/help|/hooks|/mode|/recommendations|/resume|/select|/send-to-shell|/session|/shell|/skills)
      return 0
      ;;
  esac

  return 1
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

_cosh_command_has_secret() {
  local lower="${(L)1}"

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

_cosh_zshaddhistory_marker() {
  local command="${1%$'\n'}"
  if _cosh_is_handoff_wrapper "$command"; then
    local history_command="$(_cosh_unwrap_handoff_command "$command")"
    if _cosh_command_has_secret "$history_command"; then
      history_command="<redacted sensitive command>"
    fi
    _COSH_HANDOFF_HISTORY_COMMAND="$history_command"
    return 1
  fi
  if _cosh_command_has_secret "$command"; then
    return 1
  fi
  return 0
}

_cosh_add_handoff_history() {
  if [[ -z "${_COSH_HANDOFF_HISTORY_COMMAND+x}" ]]; then
    return 0
  fi
  print -sr -- "$_COSH_HANDOFF_HISTORY_COMMAND" 2>/dev/null || true
  unset _COSH_HANDOFF_HISTORY_COMMAND 2>/dev/null || true
}

command_not_found_handler() {
  if [[ "${_COSH_PREEXEC_INTERCEPTED:-0}" == 1 ]]; then
    _COSH_PREEXEC_INTERCEPTED=0
    return 0
  fi

  local command="$1"
  shift || true
  local original="$command"
  if (($# > 0)); then
    original="$original $*"
  fi

  if [[ "${_COSH_HANDOFF_ACTIVE:-0}" == 1 ]] || _cosh_is_pending_handoff_command "$original"; then
    printf 'zsh: command not found: %s\n' "$command" >&2
    return 127
  fi

  local reason
  if reason="$(_cosh_should_intercept_unknown "$command" "$original" "$(($# + 1))")"; then
    _cosh_emit_intercept_marker "$original" "$reason"
    return 0
  fi

  printf 'zsh: command not found: %s\n' "$command" >&2
  return 127
}

_cosh_preexec_marker() {
  _COSH_PREEXEC_INTERCEPTED=0
  local command="$1"
  local display_command="$command"
  local path_trusted=false
  if [[ "${preexec_functions[-1]:-}" == "_cosh_preexec_marker" ]]; then
    path_trusted=true
  fi
  if _cosh_is_handoff_wrapper "$command"; then
    display_command="$(_cosh_unwrap_handoff_command "$command")"
    _COSH_HANDOFF_ACTIVE=1
    if _cosh_command_has_secret "$display_command"; then
      display_command="<redacted sensitive command>"
    fi
    _COSH_HANDOFF_HISTORY_COMMAND="$display_command"
  elif _cosh_is_pending_handoff_command "$command"; then
    _COSH_HANDOFF_ACTIVE=1
  else
    _cosh_clear_handoff_request
    unset _COSH_HANDOFF_ACTIVE 2>/dev/null || true
    unset _COSH_HANDOFF_HISTORY_COMMAND 2>/dev/null || true
    local first_word="$command"
    local argc=1
    if [[ "$command" == *[[:space:]]* ]]; then
      first_word="${command%%[[:space:]]*}"
      argc=2
    fi
    local reason
    if reason="$(_cosh_should_intercept_unknown "$first_word" "$command" "$argc")"; then
      _cosh_emit_intercept_marker "$command" "$reason"
      _COSH_PREEXEC_INTERCEPTED=1
      return 1
    fi
  fi
  if _cosh_command_has_secret "$display_command"; then
    display_command="<redacted sensitive command>"
  fi
  _cosh_emit_marker "preexec" "$display_command" 0 "$path_trusted"
}

_cosh_precmd_marker() {
  local exit_status=$?
  setopt NO_PROMPT_CR 2>/dev/null || true
  setopt NO_PROMPT_SP 2>/dev/null || true
  _cosh_apply_internal_recovery
  _cosh_add_handoff_history
  _cosh_clear_handoff_request
  unset _COSH_HANDOFF_ACTIVE 2>/dev/null || true
  _cosh_emit_marker "precmd" "" "$exit_status" false
}

# ── Hook setup (re-set after user rcfile may have overridden) ──
# Slash command function stubs — prevent "zsh: no such file or directory" for
# commands starting with / that zsh would try to exec as an absolute path.
# The actual interception and marker emission happens in _cosh_preexec_marker.
for _cosh_sc in agent allow answer approval-mode approve audit auth cancel clear config copy debug deny details explain extensions help hooks mode recommendations select send-to-shell shell skills; do
  functions[/$_cosh_sc]=':'
done
unset _cosh_sc

autoload -Uz add-zsh-hook
add-zsh-hook zshaddhistory _cosh_zshaddhistory_marker
add-zsh-hook preexec _cosh_preexec_marker
add-zsh-hook precmd _cosh_precmd_marker
"#
}
