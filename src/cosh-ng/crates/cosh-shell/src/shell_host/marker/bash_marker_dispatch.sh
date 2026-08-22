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
  # Guarded call: the helper reports its enum status (1 = plain ASCII)
  # through the return value by design, so a bare call would surface an
  # internal "failure" on every ordinary dispatch — fatal under a user
  # `set -e` session (SEM-019) and noisy for user ERR traps. The
  # conditional context keeps the 0/1/2 tri-state intact.
  utf8_status=0
  _cosh_utf8_han_status "$input" || utf8_status=$?
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
  if [[ "${_COSH_ATTEMPT_SUBSHELL:-}" != "${BASH_SUBSHELL:-0}"
     || "${#FUNCNAME[@]}" != 1
     || "${_COSH_ATTEMPT_EXPANSION_DRIFT:-0}" == 1 ]]; then
    _cosh_delegate_bash_command_not_found "$command" "$@"
    return $?
  fi
  if [[ "${_COSH_ATTEMPT_UNSAFE:-0}" == 1 ]]; then
    local command_fingerprint
    # cksum failure inside the substitution must not leak a non-zero
    # status into the user's errexit context; empty already means
    # "delegate to native command_not_found" below.
    command_fingerprint="$(_cosh_token_fingerprint "$command")" || command_fingerprint=""
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
  if [[ "$intent" == "natural_language" && "${_COSH_AI_ENABLED:-1}" == 1 ]]; then
    if [[ "${_COSH_HAS_USER_COMMAND_NOT_FOUND:-0}" == 1 ]]; then
      _cosh_emit_top_level_missing_marker "$intent" "$sensitive" false
      _cosh_delegate_bash_command_not_found "$command" "$@"
      return $?
    fi
    _cosh_emit_intercept_marker "$original" "natural_language" true "$sensitive"
    return 0
  fi
  _cosh_emit_top_level_missing_marker "$intent" "$sensitive" false
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

# NS-009 aggregate exit invariant: after the first DEBUG firing of a line
# has finished the preexec work, the trap stays DISARMED for the rest of
# the line — per-command trap execution is what opens bash's mid-line job
# reap/notify/cleanup window that serializes background jobs. Re-arm only
# when (a) the active spec is not the marker's own (user ownership must
# never be dropped), (b) a user OLD DEBUG chain exists (native bash with a
# user DEBUG trap fires per command too — parity mode), or (c) the line
# dispatch is still pending (stale-history retry loop). The prompt
# boundary (_cosh_rearm_debug_trap) is the sole other re-arm authority.
_cosh_debug_trap_exit() {
  if [[ "$2" != true || -n "${_COSH_OLD_DEBUG_TRAP:-}" \
        || "${_COSH_AT_PROMPT:-0}" == 1 ]]; then
    eval "$1" 2>/dev/null || true
  else
    # Ledger for the prompt-boundary re-arm: an empty DEBUG trap at the
    # prompt is only *our* dormant state if we disarmed it here; without
    # this record an empty trap is treated as user-cleared and respected.
    _COSH_DEBUG_TRAP_DORMANT=1
  fi
}
# Prompt-boundary re-arm: snapshot whatever DEBUG trap is live (a user
# hook may have installed one during this line) and decide ownership:
#   - empty + user touched traps this line (MAY_CHANGE): the user cleared
#     the trap on purpose — respect it, marker idles (self-heal semantics);
#   - empty otherwise: our own dormant state — re-install the marker trap;
#   - a user trap embedding the marker (combined form): the user owns the
#     spec — keep it armed as-is so path generation stays fails-closed;
#   - any other user trap: absorb into the OLD chain, then re-install.
_cosh_rearm_debug_trap() {
  local snapshot_file="${COSH_RECOVERY_REQUEST_FILE:-/tmp/cosh-recovery}.debug-trap"
  local current=""
  local may_change="${_COSH_DEBUG_TRAP_MAY_CHANGE:-0}"
  unset _COSH_DEBUG_TRAP_MAY_CHANGE
  _COSH_SNAPSHOT_DEBUG_TRAP=1
  trap -p DEBUG > "$snapshot_file" 2>/dev/null || true
  unset _COSH_SNAPSHOT_DEBUG_TRAP
  IFS= read -r current < "$snapshot_file" || current=""
  rm -f -- "$snapshot_file" 2>/dev/null || true
  if [[ -z "$current" ]]; then
    # Empty trap at the prompt: three-way ownership decision. A line that
    # touched traps (MAY_CHANGE) means the user cleared it - respect it.
    # Our own dormant ledger means the line-execution exit disarmed it -
    # reinstall. Neither (unexpected) fails safe to the native state.
    if [[ "$may_change" == 1 ]]; then
      unset _COSH_DEBUG_TRAP_DORMANT
      _COSH_ACTIVE_DEBUG_TRAP=""
      return 0
    fi
    if [[ "${_COSH_DEBUG_TRAP_DORMANT:-0}" != 1 ]]; then
      _COSH_ACTIVE_DEBUG_TRAP=""
      return 0
    fi
  fi
  unset _COSH_DEBUG_TRAP_DORMANT
  if [[ -n "$current" && "$current" != "trap -- '_cosh_preexec_marker' DEBUG" ]]; then
    local user_cmd="${current#trap -- \'}"
    user_cmd="${user_cmd%\' DEBUG}"
    user_cmd="${user_cmd//\'\\\'\'/\'}"
    if [[ "$user_cmd" != *_cosh_preexec_marker* ]]; then
      _COSH_OLD_DEBUG_TRAP="$user_cmd"
    else
      _COSH_ACTIVE_DEBUG_TRAP="$current"
      return 0
    fi
  fi
  _COSH_ACTIVE_DEBUG_TRAP="trap -- '_cosh_preexec_marker' DEBUG"
  trap '_cosh_preexec_marker' DEBUG
}
# Frame-level errexit protection (SEM-019): the dispatch/prompt frames run
# dozens of internal commands whose transient non-zero statuses must never
# reach a user `set -e` session. The wrapper suspends errexit on entry and
# restores it on exit; the veto path (non-zero return, extdebug skip) defers
# restoration to the next frame entry so the trap's own non-zero return
# cannot re-trigger errexit mid-veto (2541-D4; prompt-side unified restore
# proven safe by the #2598 T1 probe).
_cosh_preexec_marker() {
  local _cosh_had_errexit=0
  if [[ "${_COSH_RESTORE_ERREXIT:-0}" == 1 ]]; then
    _cosh_had_errexit=1
    unset _COSH_RESTORE_ERREXIT
  fi
  case $- in *e*) _cosh_had_errexit=1; set +e ;; esac
  _cosh_preexec_marker_impl "$@"
  local _cosh_ret=$?
  if (( _cosh_had_errexit )); then
    if (( _cosh_ret == 0 )); then
      set -e
    else
      _COSH_RESTORE_ERREXIT=1
    fi
  fi
  return "$_cosh_ret"
}
_cosh_preexec_marker_impl() {
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
  local path_trusted=false
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
  if [[ "$active_debug_trap" == "trap -- '_cosh_preexec_marker' DEBUG" ]]; then
    path_trusted=true
  fi
  if [[ -n "${_COSH_OLD_DEBUG_TRAP:-}" ]]; then
    eval "$_COSH_OLD_DEBUG_TRAP" 2>/dev/null || true
  fi
  if [[ "${_COSH_IN_PROMPT_COMMAND:-0}" == 1 ]]; then
    _cosh_debug_trap_exit "$active_debug_trap" "$path_trusted"
    return 0
  fi
  # Internal-namespace ownership guard: statements from the marker's own
  # frames (e.g. the errexit wrapper tail running under the freshly
  # re-armed trap) must never enter the user dispatch path. Without this,
  # the stale-history containment check can false-match an internal
  # statement against the history entry (probed: `_cosh_debug_trap_exit`
  # contains "exit", the preloaded history tail was `exit`, and the
  # resulting begin_attempt poisoned the attempt state so the next real
  # command's command_not_found dispatch delegated natively). `_cosh_`/
  # `_COSH_` is the marker's reserved namespace; a user command carrying
  # it degrades to native execution (fail-safe, same as a stale miss).
  # One obligation survives the early exit: a trap-mutating line (e.g. a
  # user installing a combined trap that embeds the marker) must still
  # flag MAY_CHANGE so the next snapshot re-reads ownership — otherwise
  # path generation would stay trusted under a user-owned trap.
  if [[ "${BASH_COMMAND:-}" == *_cosh_* || "${BASH_COMMAND:-}" == *_COSH_* ]]; then
    if [[ "${BASH_COMMAND:-}" == *trap*DEBUG* ]]; then
      _COSH_DEBUG_TRAP_MAY_CHANGE=1
    fi
    _cosh_debug_trap_exit "$active_debug_trap" "$path_trusted"
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
    local attempt_expansion_drift=0
    _cosh_has_leading_alias "$command" && attempt_expansion_drift=1
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
      local fallback_sensitive=false
      _cosh_command_has_secret "$fallback_command" && fallback_sensitive=true
      local fallback_reason
      if fallback_reason="$(_cosh_should_intercept_unknown "$fallback_first_word" "$fallback_command" "$fallback_argc")"; then
        _cosh_emit_intercept_marker "$fallback_command" "$fallback_reason" false "$fallback_sensitive"
        _COSH_AT_PROMPT=0
        _cosh_debug_trap_exit "$active_debug_trap" "$path_trusted"
        return 1
      fi
      if _cosh_should_intercept_missing_path "$fallback_first_word" "$fallback_command"; then
        _cosh_emit_intercept_marker "$fallback_command" "natural_language" false "$fallback_sensitive"
        _COSH_AT_PROMPT=0
        _cosh_debug_trap_exit "$active_debug_trap" "$path_trusted"
        return 1
      fi
      _cosh_debug_trap_exit "$active_debug_trap" "$path_trusted"
      return 0
    fi
    if [[ -n "$history_no" && -n "$command" ]]; then
      _COSH_ATTEMPT_ACTIVE=0
      _COSH_ATTEMPT_SENSITIVE=0
      _COSH_ATTEMPT_UNSAFE=0
      local display_command="$command"
      if _cosh_is_handoff_wrapper "$command"; then
        display_command="$(_cosh_unwrap_handoff_command "$command")"
        _COSH_HANDOFF_HISTORY_NO="$history_no"
        # Handoff treatment (active flag, pager policy, token) applies only
        # when the unwrapped text matches the staged request: a user-typed
        # bypass-prefixed line racing ahead must not steal the claim, and its
        # precmd must not see the active flag and clear the staged sidecars
        # the real handoff line is about to consume (#2142 review).
        if _cosh_is_pending_handoff_command "$display_command"; then
          _COSH_HANDOFF_ACTIVE=1
          _cosh_apply_handoff_pager_policy
          _cosh_load_handoff_token
          _cosh_clear_handoff_request
        fi
      elif _cosh_is_pending_handoff_command "$command"; then
        _COSH_HANDOFF_ACTIVE=1
        _cosh_load_handoff_token
        _cosh_apply_handoff_pager_policy
        # Consume-then-clear: the claim is single-shot, and clearing here
        # (not in unrelated branches) is what keeps it alive across
        # command-ahead races.
        _cosh_clear_handoff_request
      else
        # Deliberately no _cosh_clear_handoff_request here: an unrelated
        # command racing ahead of an approved handoff must leave the staged
        # request/token sidecars for the handoff line that follows; the Rust
        # transport owns cleanup for abandoned handoffs (#2142 review).
        unset _COSH_HANDOFF_ACTIVE 2>/dev/null || true
        unset _COSH_HANDOFF_TOKEN 2>/dev/null || true
        unset _COSH_HANDOFF_HISTORY_NO _COSH_HANDOFF_HISTORY_COMMAND 2>/dev/null || true
        local first_word="$command"
        local argc=1
        if [[ "$command" == *[[:space:]]* ]]; then
          first_word="${command%%[[:space:]]*}"
          argc=2
        fi
        local intercept_sensitive=false
        _cosh_command_has_secret "$command" && intercept_sensitive=true
        local reason
        if reason="$(_cosh_should_intercept_unknown "$first_word" "$command" "$argc")"; then
          # Intercepted lines return 1 before the secret redaction below
          # ever runs, so scrub credential-bearing entries here or the raw
          # text would persist in native history (routed slash submissions
          # enter history via readline before the trap fires).
          if [[ "$intercept_sensitive" == true ]]; then
            builtin history -d "$history_no" 2>/dev/null || true
          fi
          _cosh_emit_intercept_marker "$command" "$reason" false "$intercept_sensitive"
          _COSH_AT_PROMPT=0
          _cosh_debug_trap_exit "$active_debug_trap" "$path_trusted"
          return 1
        fi
        if _cosh_should_intercept_missing_path "$first_word" "$command"; then
          if [[ "$intercept_sensitive" == true ]]; then
            builtin history -d "$history_no" 2>/dev/null || true
          fi
          _cosh_emit_intercept_marker "$command" "natural_language" false "$intercept_sensitive"
          _COSH_AT_PROMPT=0
          _cosh_debug_trap_exit "$active_debug_trap" "$path_trusted"
          return 1
        fi
        _cosh_begin_attempt "$command" "$first_word" "$attempt_expansion_drift"
      fi
      # Containment form: a trap mutation may hide inside a compound on
      # the same line (e.g. `f(){ trap - DEBUG; }; f`), not only at the
      # line head. Cross-line indirection (function defined earlier,
      # called later) stays undetectable and is a recorded deviation.
      if [[ "$command" == *trap*DEBUG* ]]; then
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
  _cosh_debug_trap_exit "$active_debug_trap" "$path_trusted"
  return 0
}
