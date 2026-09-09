#!/usr/bin/env bash
# install.sh — Deploy the agent-memory OpenClaw plugin via the openclaw CLI.
#
# This script ONLY deploys an already-built plugin.
# Compilation is the Makefile's job:
#     make -C src/agent-memory build-openclaw-plugin
# If dist/index.js is missing, exit with a clear error.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-openclaw}"
COMPONENT="${ANOLISA_COMPONENT:-agent-memory}"
# ANOLISA_ADAPTER_DIR is injected by anolisa-adapter-ctl (FHS spec §2.4).
# Fall back to the directory containing manifest.json.
PLUGIN_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}/openclaw"

OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$OPENCLAW_HOME}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR%/}"
OPENCLAW_HOME="${OPENCLAW_HOME%/}"
OPENCLAW_BIN="${OPENCLAW_BIN:-openclaw}"

# Normalize the consent switch with the same systemd-style token table as
# the Rust side's env_bool() (src/config.rs): only leading/trailing
# whitespace is trimmed — deleting interior whitespace would normalize
# 't rue' into a grant — and matching is case-insensitive. Unset or empty
# accepts (the documented default); every other value, including
# whitespace-only, aborts with rc=2 before any CLI call — the operator
# must never believe they have withheld consent while the script accepts
# it. This runs before every early exit (missing CLI, missing dist) so a
# bad value is reported unconditionally.
ACCEPT_CAPABILITIES=1
_consent="${AGENT_MEMORY_ACCEPT_CAPABILITIES-}"
if [ -n "$_consent" ]; then
    _consent="${_consent#"${_consent%%[![:space:]]*}"}"
    _consent="${_consent%"${_consent##*[![:space:]]}"}"
    case "$(printf '%s' "$_consent" | tr '[:upper:]' '[:lower:]')" in
        1|true|yes|on) ACCEPT_CAPABILITIES=1 ;;
        0|false|no|off) ACCEPT_CAPABILITIES=0 ;;
        *)
            echo "[${COMPONENT}] ERROR: AGENT_MEMORY_ACCEPT_CAPABILITIES='${AGENT_MEMORY_ACCEPT_CAPABILITIES}' is not a boolean (use 1/true/yes/on or 0/false/no/off)." >&2
            exit 2 ;;
    esac
fi

echo "[${COMPONENT}] Installing ${AGENT} plugin..."

if ! command -v "$OPENCLAW_BIN" &>/dev/null; then
    echo "[${COMPONENT}] openclaw CLI not found (OPENCLAW_BIN=${OPENCLAW_BIN}) — skipping plugin installation."
    echo "[${COMPONENT}] Install OpenClaw first, then run this script again."
    exit 0
fi

if [ ! -f "$PLUGIN_DIR/dist/index.js" ]; then
    echo "[${COMPONENT}] ERROR: $PLUGIN_DIR/dist/index.js is missing." >&2
    echo "[${COMPONENT}]        Build the plugin first:" >&2
    echo "[${COMPONENT}]            cd $PLUGIN_DIR && npm run build" >&2
    exit 1
fi

# Base argv only. Both optional flags — capability consent and the legacy
# unsafe-install bypass — are negotiated from the installer help probed below,
# so nothing is appended before that probe has run.
INSTALL_ARGS=("--force")

# OpenClaw gates capability-declaring plugins behind install-time consent:
# a noninteractive `plugins install` is rejected unless --accept-capabilities
# is passed. Version boundaries here are unreliable — the flag shipped in
# 2026.8.1 but the CLI reference documents it only since 2026.9.1, and older
# releases abort on the unknown option outright — so probe the host's
# installer help instead of version-gating. Whole-token match (near-miss
# options such as --no-accept-capabilities stay out), mirroring tokenless's
# installer and anolisa-core's help_lists_flag().
PROBE_RC=0
INSTALL_HELP="$(env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" \
    "$OPENCLAW_BIN" plugins install --help 2>&1)" || PROBE_RC=$?
if [ "$PROBE_RC" -ne 0 ]; then
    # Degrade instead of failing closed: the script must keep installing on
    # every host the manifest claims to support. Clearing INSTALL_HELP keeps
    # probe error text from being mistaken for an advertised option, and marks
    # the unsafe-install decision below as unclassified rather than absent.
    printf '[%s] WARNING: cannot inspect OpenClaw installer options (rc=%s): %s\n' \
        "$COMPONENT" "$PROBE_RC" "$INSTALL_HELP" >&2
    printf '[%s]          Installing without --accept-capabilities; the unsafe-install\n' "$COMPONENT" >&2
    printf '[%s]          bypass cannot be classified either (see its own note below).\n' "$COMPONENT" >&2
    INSTALL_HELP=""
fi
CONSENT_WITHHELD=0
if [[ "$INSTALL_HELP" =~ (^|[^[:alnum:]_.-])--accept-capabilities([^[:alnum:]_.-]|$) ]]; then
    if [ "$ACCEPT_CAPABILITIES" = "0" ]; then
        # Deliberate refusal, not a silent skip: policies that forbid
        # non-interactive consent grants must see the install fail loudly
        # on gating hosts instead of succeeding with an implicit grant.
        CONSENT_WITHHELD=1
        echo "[${COMPONENT}] AGENT_MEMORY_ACCEPT_CAPABILITIES=0: withholding --accept-capabilities — capability consent is not granted by this script." >&2
        echo "[${COMPONENT}]          Hosts that gate consent will reject this install until consent is granted interactively." >&2
    else
        INSTALL_ARGS+=("--accept-capabilities")
        echo "[${COMPONENT}] Passing --accept-capabilities: granting install-time consent to memory-anolisa's declared capabilities."
    fi
elif [ -n "$INSTALL_HELP" ]; then
    echo "[${COMPONENT}] OpenClaw did not advertise --accept-capabilities (pre-2026.8.1 host); installing with the base flags."
elif [ "$ACCEPT_CAPABILITIES" = "0" ]; then
    # Unreachable unless the probe failed: the host's gating status is
    # unknown, so surface that the opt-out is shaping the install.
    echo "[${COMPONENT}] AGENT_MEMORY_ACCEPT_CAPABILITIES=0 is active; if this host gates consent, the install will fail." >&2
fi

# Hosts whose installer still runs the install-time safety scan need
# --dangerously-force-unsafe-install as their only non-interactive bypass, and
# this plugin trips that scan because it spawns the agent-memory MCP server as a
# stdio subprocess. Later hosts dropped install-time dangerous-code blocking and
# keep the token as a deprecated no-op, so passing it there accomplishes nothing
# and fails outright once the token is removed. The boundary is not the
# capability-consent one above: in the published npm packages 2026.6.1 still
# advertises "Bypass built-in dangerous-code install blocking", while
# 2026.6.2-beta.1 and every later release (stable from 2026.6.5) advertise
# "Deprecated no-op; security.installPolicy may still block". Classify the option
# from the help already captured above rather than from a version — the same
# line-scoped, whole-token read anolisa-core's unsafe_install_support() performs
# — and pass the flag only while it still has effect. `tr` rather than ${var,,}:
# bash 3.2 has no case-conversion expansion.
UNSAFE_SUPPORT=absent
[ "$PROBE_RC" -eq 0 ] || UNSAFE_SUPPORT=unknown
while IFS= read -r help_line; do
    if [[ "$help_line" =~ (^|[^[:alnum:]_.-])--dangerously-force-unsafe-install([^[:alnum:]_.-]|$) ]]; then
        help_line_lc="$(printf '%s' "$help_line" | tr '[:upper:]' '[:lower:]')"
        case "$help_line_lc" in
            *"no op"*|*"no-op"*) UNSAFE_SUPPORT=noop ;;
            *) UNSAFE_SUPPORT=effective ;;
        esac
        break
    fi
done <<< "$INSTALL_HELP"

# AGENT_MEMORY_SAFE_INSTALL=1 declines the bypass. That is a real choice only
# where the bypass still does something; on a no-op host both paths are
# identical, so say so instead of silently ignoring the variable.
SAFE_INSTALL="${AGENT_MEMORY_SAFE_INSTALL:-0}"
UNSAFE_DECLINED=0
case "$UNSAFE_SUPPORT" in
    effective)
        if [ "$SAFE_INSTALL" = "1" ]; then
            UNSAFE_DECLINED=1
            echo "[${COMPONENT}] AGENT_MEMORY_SAFE_INSTALL=1: declining --dangerously-force-unsafe-install;" >&2
            echo "[${COMPONENT}]       this host still scans plugin sources at install time and may block" >&2
            echo "[${COMPONENT}]       the plugin for its child_process.spawn MCP transport." >&2
        else
            INSTALL_ARGS+=("--dangerously-force-unsafe-install")
            echo "[${COMPONENT}] Passing --dangerously-force-unsafe-install: this host still scans plugin"
            echo "[${COMPONENT}]       sources at install time, and that scan flags the child_process.spawn"
            echo "[${COMPONENT}]       this plugin uses to launch the agent-memory MCP server over stdio."
        fi
        ;;
    unknown)
        # Probe failed: the host cannot be classified, so keep the bypass that
        # every release still accepting the token needs, and name the opt-out.
        if [ "$SAFE_INSTALL" = "1" ]; then
            UNSAFE_DECLINED=1
            echo "[${COMPONENT}] AGENT_MEMORY_SAFE_INSTALL=1: declining --dangerously-force-unsafe-install." >&2
        else
            INSTALL_ARGS+=("--dangerously-force-unsafe-install")
            echo "[${COMPONENT}] WARNING: installer options are unclassified, so the legacy bypass is kept." >&2
            echo "[${COMPONENT}]          Set AGENT_MEMORY_SAFE_INSTALL=1 to decline it on hosts that no" >&2
            echo "[${COMPONENT}]          longer accept the option." >&2
        fi
        ;;
    noop)
        echo "[${COMPONENT}] Not passing --dangerously-force-unsafe-install: this OpenClaw advertises it"
        echo "[${COMPONENT}]       as a deprecated no-op, so install-time safety follows the operator-owned"
        echo "[${COMPONENT}]       security.installPolicy instead."
        if [ "$SAFE_INSTALL" = "1" ]; then
            echo "[${COMPONENT}]       AGENT_MEMORY_SAFE_INSTALL=1 changes nothing on this host: the flag is"
            echo "[${COMPONENT}]       omitted either way."
        fi
        ;;
    *)
        echo "[${COMPONENT}] Not passing --dangerously-force-unsafe-install: this OpenClaw does not"
        echo "[${COMPONENT}]       advertise the option."
        ;;
esac

# The transcript (tee into a log file, then grep the file — never a
# short-circuit pipe, whose early match would SIGPIPE the producer under
# pipefail) exists only for the consent-withheld path, the sole consumer of
# the consent-rejection signal; every other install runs the CLI exactly as
# before — stderr stays stderr, stdout stays the operator's terminal, so
# TTY-gated colour/prompts survive on the SAFE_INSTALL blocking-scan path.
# The merged 2>&1 on the withheld path is deliberate for a non-interactive
# refusal transcript: the CLI's stderr surfaces on this script's stdout.
INSTALL_CMD=(env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" \
    "$OPENCLAW_BIN" plugins install "$PLUGIN_DIR" "${INSTALL_ARGS[@]}")
INSTALL_RC=0
if [ "$CONSENT_WITHHELD" = "1" ]; then
    INSTALL_LOG="$(mktemp -t agent-memory-openclaw-install.XXXXXX)"
    trap 'rm -f "$INSTALL_LOG"' EXIT
    "${INSTALL_CMD[@]}" 2>&1 | tee "$INSTALL_LOG" || INSTALL_RC=${PIPESTATUS[0]}
else
    INSTALL_LOG=""
    "${INSTALL_CMD[@]}" || INSTALL_RC=$?
fi
if [ "$INSTALL_RC" -ne 0 ]; then
    # Attribute the failure to the withheld consent only when OpenClaw's
    # output carries the documented consent-rejection phrase ("Plugin "…"
    # requires capability consent. …"); permissions, a broken manifest, or
    # a security scan must keep the generic failure code so callers do not
    # mistake a real install failure for a policy refusal. If OpenClaw
    # rewords the message, this degrades to rc=1 with the opt-out note.
    if [ "$CONSENT_WITHHELD" = "1" ] && grep -qi 'requires capability consent' "$INSTALL_LOG"; then
        echo "[${COMPONENT}] install failed with consent withheld by AGENT_MEMORY_ACCEPT_CAPABILITIES=0 — grant consent interactively or unset the variable to let this script grant it." >&2
        echo "[${COMPONENT}]   openclaw plugins install <plugin-dir> --force --accept-capabilities" >&2
        echo "[${COMPONENT}]   plugin-dir: $PLUGIN_DIR" >&2
        exit 3
    fi
    if [ "$CONSENT_WITHHELD" = "1" ]; then
        echo "[${COMPONENT}] Note: AGENT_MEMORY_ACCEPT_CAPABILITIES=0 is active, but this failure does not look like a consent rejection." >&2
    fi
    echo "[${COMPONENT}] openclaw CLI install failed — check OpenClaw version >= 5.0.0" >&2
    # What the script itself did is knowable here; why OpenClaw rejected the
    # install is not. Neither note may assert a cause: 2388b370 attributes the
    # consent opt-out only when the captured output carries the documented
    # rejection phrase, and no equivalent phrase is documented for
    # security.installPolicy — so both stay conditional on the CLI output,
    # which this default path leaves on the operator's own terminal.
    if [ "$UNSAFE_SUPPORT" = "noop" ]; then
        echo "[${COMPONENT}]       note: this OpenClaw advertises --dangerously-force-unsafe-install as a" >&2
        echo "[${COMPONENT}]       deprecated no-op, so the script sent no bypass and cannot shape install-time" >&2
        echo "[${COMPONENT}]       safety on this host. Read the CLI output above for the actual cause; only" >&2
        echo "[${COMPONENT}]       if it names security.installPolicy is that operator-owned policy what to" >&2
        echo "[${COMPONENT}]       relax — not this script." >&2
    elif [ "$UNSAFE_DECLINED" = "1" ]; then
        echo "[${COMPONENT}]       note: AGENT_MEMORY_SAFE_INSTALL=1 declined the unsafe-install bypass. If the" >&2
        echo "[${COMPONENT}]       CLI output above blames the install-time safety scan, unset the variable to" >&2
        echo "[${COMPONENT}]       let the script pass the bypass this host still honors." >&2
    fi
    exit 1
fi

# OpenClaw 2026.6.11 requires non-bundled plugins to explicitly opt-in
# to conversation hooks (agent_end, before_prompt_build, etc.). Without
# this setting the hooks are silently blocked and auto-capture /
# auto-recall never fire (#1460).
env -u OPENCLAW_HOME OPENCLAW_STATE_DIR="$OPENCLAW_STATE_DIR" "$OPENCLAW_BIN" config set \
    "plugins.entries.memory-anolisa.hooks.allowConversationAccess" true || {
    echo "[${COMPONENT}] WARNING: failed to set allowConversationAccess — auto-capture/auto-recall hooks may be blocked." >&2
}

echo "[${COMPONENT}] ${AGENT} plugin installed via openclaw CLI."
echo "[${COMPONENT}] Run '${OPENCLAW_BIN} gateway restart' to activate."
echo "[${COMPONENT}] NOTE: allowConversationAccess and plugin hooks only take effect after gateway restart."
echo "[${COMPONENT}]       Without restart, auto-capture and auto-recall hooks remain silently blocked."
