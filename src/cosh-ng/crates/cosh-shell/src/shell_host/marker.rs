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

_cosh_emit_marker() {
  local event="$1"
  local command="$2"
  local exit_status="$3"
  local timestamp
  timestamp="$(_cosh_now_ms)"

  printf '\033]1337;COSH;{"event":"%s","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","command":"%s","status":%s}\a' \
    "$(_cosh_json_escape "$event")" \
    "$(_cosh_json_escape "$COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "$(_cosh_json_escape "$command")" \
    "$exit_status"
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
    /agent|/allow|/answer|/approval-mode|/approve|/audit|/auth|/cancel|/clear|/config|/copy|/debug|/deny|/details|/explain|/help|/hooks|/mode|/select|/send-to-shell|/shell|/skill)
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
    /agent|/allow|/answer|/approval-mode|/approve|/audit|/auth|/cancel|/clear|/config|/copy|/debug|/deny|/details|/explain|/help|/hooks|/mode|/select|/send-to-shell|/shell|/skill)
      return 0
      ;;
  esac

  return 1
}

command_not_found_handle() {
  local command="$1"
  shift || true
  local original="$command"
  if (($# > 0)); then
    original="$original $*"
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
  trap - DEBUG
  if [[ -n "${_COSH_OLD_DEBUG_TRAP:-}" ]]; then
    eval "$_COSH_OLD_DEBUG_TRAP" 2>/dev/null || true
  fi
  if [[ "${_COSH_AT_PROMPT:-0}" == 1 ]]; then
    local history_entry
    local history_no
    local command
    history_entry="$(_cosh_history_entry)"
    history_no="$(_cosh_history_no "$history_entry")"
    command="$(_cosh_history_command_from_entry "$history_entry")"
    if [[ -n "$history_no" && "$history_no" != "${_COSH_LAST_HISTORY_NO:-0}" && -n "$command" ]]; then
      _COSH_LAST_HISTORY_NO="$history_no"
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
        trap '_cosh_preexec_marker' DEBUG
        return 1
      fi
      _cosh_emit_marker "preexec" "$command" 0
    fi
    _COSH_AT_PROMPT=0
  fi
  trap '_cosh_preexec_marker' DEBUG
  return 0
}

_cosh_precmd_marker() {
  local status=$?
  _cosh_emit_marker "precmd" "" "$status"
  _COSH_AT_PROMPT=1
}

# ── Hook setup (re-set after user rcfile may have overridden) ──
shopt -s extdebug 2>/dev/null || true
_COSH_OLD_DEBUG_TRAP="$(trap -p DEBUG 2>/dev/null | sed "s/^trap -- '\\(.*\\)' DEBUG$/\\1/" || true)"
trap '_cosh_preexec_marker' DEBUG
# Append precmd to existing PROMPT_COMMAND
if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
  PROMPT_COMMAND+=(_cosh_precmd_marker)
else
  PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND;}_cosh_precmd_marker"
fi
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
  local timestamp
  timestamp="$(_cosh_now_ms)"

  printf '\033]1337;COSH;{"event":"%s","token":"%s","session_id":"%s","timestamp_ms":%s,"cwd":"%s","command":"%s","status":%s}\a' \
    "$(_cosh_json_escape "$event")" \
    "$(_cosh_json_escape "$COSH_MARKER_TOKEN")" \
    "$(_cosh_json_escape "$COSH_SESSION_ID")" \
    "$timestamp" \
    "$(_cosh_json_escape "$PWD")" \
    "$(_cosh_json_escape "$command")" \
    "$exit_status"
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
    /agent|/allow|/answer|/approval-mode|/approve|/audit|/auth|/cancel|/clear|/config|/copy|/debug|/deny|/details|/explain|/help|/hooks|/mode|/select|/send-to-shell|/shell|/skill)
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
    /agent|/allow|/answer|/approval-mode|/approve|/audit|/auth|/cancel|/clear|/config|/copy|/debug|/deny|/details|/explain|/help|/hooks|/mode|/select|/send-to-shell|/shell|/skill)
      return 0
      ;;
  esac

  return 1
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
  _cosh_emit_marker "preexec" "$command" 0
}

_cosh_precmd_marker() {
  local exit_status=$?
  setopt NO_PROMPT_CR 2>/dev/null || true
  setopt NO_PROMPT_SP 2>/dev/null || true
  _cosh_emit_marker "precmd" "" "$exit_status"
}

# ── Hook setup (re-set after user rcfile may have overridden) ──
autoload -Uz add-zsh-hook
add-zsh-hook preexec _cosh_preexec_marker
add-zsh-hook precmd _cosh_precmd_marker
"#
}
