#!/bin/bash
# /etc/profile.d/anolisa-register.sh  (bash users)
# /etc/anolisa/profile.d/anolisa-register.sh  (cosh users)
#
# Login-time token collection consent check script.
# Executes before entering the actual shell; handles interactive confirmation for INIT state.
#
# Source location: src/anolisa/scripts/anolisa-register.sh
#
# Deployment notes (RPM/DEB):
#   - This script is deployed directly via %files
#   - bash users: install -m 0644 to /etc/profile.d/
#   - cosh users: install -m 0644 to /etc/anolisa/profile.d/
#
#
# ── Early exit helper (compatible with both source and direct execution) ────────────
_anolisa_exit() {
    return 0 2>/dev/null || exit 0
}

# ── Skip non-interactive sessions ─────────────────────────────────────────────
# Skip when stdin is not a terminal (scp / sftp / rsync / ssh host cmd, etc.)
if ! tty -s 2>/dev/null; then
    _anolisa_exit
fi

# ── Only check once per SSH session ──────────────────────────────────────────
if [ -n "$ANOLISA_REGISTER_DONE" ]; then
    _anolisa_exit
fi
export ANOLISA_REGISTER_DONE=1

# ── Check if `anolisa` command is available ───────────────────────────────────
if ! command -v anolisa >/dev/null 2>&1; then
    _anolisa_exit
fi

# ── Check if current user has sudo privileges ─────────────────────────────────
# Non-privileged users cannot write /etc/anolisa/register.json, skip them
HAS_SUDO=false
if [ "$(id -u)" -eq 0 ]; then
    HAS_SUDO=true
# sudo -n true also matches users within the sudo password cache window; allowed here
elif sudo -n true 2>/dev/null; then
    HAS_SUDO=true
fi

# ── Read registration state ──────────────────────────────────────────────────
REGISTER_JSON="/etc/anolisa/register.json"
STATE="init"
LATER_TS=""

if [ -f "$REGISTER_JSON" ]; then
    if command -v jq >/dev/null 2>&1; then
        STATE=$(jq -r '.state // "init"' "$REGISTER_JSON" 2>/dev/null)
        LATER_TS=$(jq -r '.later_start_time // empty' "$REGISTER_JSON" 2>/dev/null)
    else
        STATE=$(grep -o '"state"[[:space:]]*:[[:space:]]*"[^"]*"' "$REGISTER_JSON" \
            | head -1 | sed 's/.*:[[:space:]]*"//;s/".*//')
        LATER_TS=$(grep -o '"later_start_time"[[:space:]]*:[[:space:]]*"[^"]*"' "$REGISTER_JSON" \
            | head -1 | sed 's/.*:[[:space:]]*"//;s/".*//')
    fi
    [ -z "$STATE" ] && STATE="init"
fi

# ── Determine if prompt is needed ──────────────────────────────────────
# Only prompt in INIT state with sudo privileges
NEED_PROMPT=false

if [ "$STATE" = "init" ]; then
    if [ -z "$LATER_TS" ]; then
        # INIT-fresh: no decision has been made yet
        NEED_PROMPT=true
    else
        # INIT-later: check if 30 days (2592000 seconds) have elapsed
        # GNU coreutils → busybox → BSD (macOS) fallback
        LATER_EPOCH=$(date -d "$LATER_TS" +%s 2>/dev/null \
            || date -D '%Y-%m-%dT%H:%M:%SZ' -d "$LATER_TS" +%s 2>/dev/null \
            || date -jf '%Y-%m-%dT%H:%M:%SZ' "$LATER_TS" +%s 2>/dev/null)
        NOW_EPOCH=$(date +%s)
        if [ -z "$LATER_EPOCH" ]; then
            # Time parsing failed — treat as expired
            NEED_PROMPT=true
        else
            ELAPSED=$(( NOW_EPOCH - LATER_EPOCH ))
            if [ "$ELAPSED" -ge 2592000 ] || [ "$ELAPSED" -lt 0 ]; then
                NEED_PROMPT=true
            fi
        fi
    fi
fi

# ── Non-privileged user: show one-line hint in INIT state, no interactive prompt ─────
if [ "$NEED_PROMPT" = true ] && [ "$HAS_SUDO" = false ]; then
    echo "anolisa subscription is not configured. Ask an admin to run: sudo anolisa subscription register"
    _anolisa_exit
fi

# ── Detect if upload service is running: skip if sysak_meta is active (already registered) ──
if command -v systemctl >/dev/null 2>&1; then
    if systemctl is-active --quiet sysak_meta 2>/dev/null \
        && systemctl is-active --quiet sysak_agentsight 2>/dev/null; then
        _anolisa_exit
    fi
fi

# ── Has sudo + needs prompt: interactive confirmation ──────────────────────
if [ "$NEED_PROMPT" = true ] && [ "$HAS_SUDO" = true ]; then
    # Verify /dev/tty is available, otherwise skip interaction
    if [ ! -e /dev/tty ]; then
        _anolisa_exit
    fi

    echo ""
    echo "───────────────────────────────────────────────────────────────────"
    echo "  🌱 Join the Agentic OS Co-Build Program"
    echo "  Welcome to Agentic OS — the operating system for the Agent era."
    echo "  We invite you to become a co-builder and help make this OS"
    echo "  smarter and more in tune with your needs."
    echo ""
    echo "  By joining, you will get:"
    echo "    ✦ Smarter cosh — learns from real user scenarios, more accurate"
    echo "    ✦ Cross-instance Token insights — view costs & trends for all"
    echo "      instances under your account in one dashboard"
    echo "    ✦ Personalized optimization — model selection, Token savings,"
    echo "      Skill recommendations, tailored for you"
    echo "    ✦ Early access to new features — beta Skills / new model"
    echo "      adaptations delivered first"
    echo "    ✦ Product co-build vote — your pain points become our next P0"
    echo ""
    echo "  Our commitments:"
    echo "    · Only upload desensitized aggregate statistics"
    echo "      (token counts, model ID, request counts, time window)"
    echo "    · Your prompts, conversations, keys, files — never leave"
    echo "      this machine"
    echo "    · Uses Alibaba Cloud internal network, zero public network"
    echo "      cost, zero extra configuration"
    echo "    · You stay in control — run 'anolisa subscription unregister'"
    echo "      to opt out at any time"
    echo ""
    echo "  Help us make Agentic OS even better?"
    echo "───────────────────────────────────────────────────────────────────"
    echo ""
    echo "  [ Y · Join / N · Local only / L · Remind me later ]"
    printf "  Default: N (local only, no data uploaded): "

    # read failure (tty abnormal, etc.) — do not execute any action to avoid the reject path
    if ! read -r CHOICE </dev/tty; then
        echo "[anolisa] warn: could not read input, skipping registration prompt"
    else
        case "$CHOICE" in
            [Yy]*)
                if [ "$(id -u)" -eq 0 ]; then
                    anolisa subscription register --yes
                else
                    sudo anolisa subscription register --yes
                fi
                ;;
            [Ll]*)
                if [ "$(id -u)" -eq 0 ]; then
                    anolisa subscription later
                else
                    sudo anolisa subscription later
                fi
                ;;
            [Nn]*|"")
                if [ "$(id -u)" -eq 0 ]; then
                    anolisa subscription unregister --force >/dev/null 2>&1
                else
                    sudo anolisa subscription unregister --force >/dev/null 2>&1
                fi
                ;;
        esac
    fi
    echo ""
fi

# ── UNREGISTERED state: show one-line hint only, no interactive prompt ────────────────
if [ "$STATE" = "unregistered" ]; then
    echo "anolisa subscription inactive. To enable: anolisa subscription register"
fi
