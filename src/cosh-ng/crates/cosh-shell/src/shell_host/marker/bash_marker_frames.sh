_cosh_precmd_marker() {
  local status="${1:-$?}"
  _cosh_apply_internal_recovery
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
  _cosh_emit_marker "precmd" "" "$status" false
  unset _COSH_HANDOFF_TOKEN 2>/dev/null || true
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
# Companion wrapper to _cosh_preexec_marker: captures the user's $? (as
# staged by the PROMPT_COMMAND guard value) before anything else runs,
# hands it to the impl explicitly, and uses the same deferred errexit
# restore as the preexec wrapper: a non-zero passthrough status would
# otherwise fail the PROMPT_COMMAND list itself and errexit kills the
# session (bash 3.2 and 5.2 both, probed in the #2598 T7 loop — the
# earlier T1 "unified restore is safe" reading was an artifact of a
# clobbered $? that pinned the status to 0). Restoration lands at the
# next frame entry, before the next user command executes.
_cosh_prompt_command() {
  # Prefer the status captured by the PROMPT_COMMAND guard value (set
  # before `declare -F` overwrote $?); fall back to $? for direct callers.
  local _cosh_status="${_COSH_PROMPT_STATUS-$?}"
  unset _COSH_PROMPT_STATUS
  local _cosh_had_errexit=0
  if [[ "${_COSH_RESTORE_ERREXIT:-0}" == 1 ]]; then
    _cosh_had_errexit=1
    unset _COSH_RESTORE_ERREXIT
  fi
  case $- in *e*) _cosh_had_errexit=1; set +e ;; esac
  _cosh_prompt_command_impl "$_cosh_status"
  local _cosh_ret=$?
  if (( _cosh_had_errexit )); then
    set -e
    # A non-zero passthrough would fail the PROMPT_COMMAND list itself and
    # errexit kills the session (probed on bash 3.2 and 5.2). Returning 0
    # is safe: bash restores the user's $? at the prompt boundary on its
    # own (probed: PROMPT_COMMAND='false' leaves `echo $?` = 7 after a
    # status-7 command, bash 3.2 and 5.2), so the passthrough below is a
    # defensive redundancy, not the contract carrier.
    return 0
  fi
  return "$_cosh_ret"
}
_cosh_prompt_command_impl() {
  local status="$1"
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
  _cosh_rearm_debug_trap
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
# Record the export attribute before the wholesale replacement below:
# `unset PROMPT_COMMAND` drops an env-inherited -x together with the
# value, so a user reassignment (scalar or array) would show `declare -a`
# where native bash keeps `declare -ax` (NS-005). Only the flags token of
# `declare -p` is parsed — an "x" inside the value cannot false-positive.
_COSH_USER_PROMPT_COMMAND_WAS_EXPORTED=0
_cosh_pc_decl="$(declare -p PROMPT_COMMAND 2>/dev/null)" || _cosh_pc_decl=""
_cosh_pc_flags="${_cosh_pc_decl#declare -}"
_cosh_pc_flags="${_cosh_pc_flags%% *}"
if [[ "$_cosh_pc_flags" == *x* ]]; then
  _COSH_USER_PROMPT_COMMAND_WAS_EXPORTED=1
fi
unset _cosh_pc_decl _cosh_pc_flags
if [[ -n "${COSH_SHELL_ISOLATED:-}" ]]; then
  unset _COSH_USER_PROMPT_COMMAND
  _COSH_USER_PROMPT_COMMAND_IS_ARRAY=0
elif [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
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
# Guard-form hijack value: an env-leaked copy evaluated by a nested bash
# (the -x restore below re-opens that path) must stay a silent no-op
# instead of "command not found" noise at every prompt. `declare -F` only
# matches shell functions — a PATH executable of the same name cannot be
# injected through it. The leading status capture is part of the contract:
# `declare -F` would otherwise clobber the user's $? before the prompt
# chain reads it (ledger exit codes depend on it); in a nested bash the
# assignment is equally silent. The re-arm stays inside the prompt frame
# (not appended here): the frame's own tail statements firing the freshly
# re-armed trap are absorbed by the dispatch guards, while a trailing list
# member would run under a still-armed user trap before the frame marks
# itself in-prompt, exploding into full junk dispatches per member.
PROMPT_COMMAND='_COSH_PROMPT_STATUS=$?; declare -F _cosh_prompt_command >/dev/null && _cosh_prompt_command'
if [[ "${_COSH_USER_PROMPT_COMMAND_WAS_EXPORTED:-0}" == 1 ]]; then
  export PROMPT_COMMAND
fi
if [[ -n "${COSH_SHELL_ISOLATED:-}" ]]; then
  builtin history -c 2>/dev/null || true
fi
