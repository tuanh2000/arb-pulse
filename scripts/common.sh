#!/usr/bin/env bash
# Shared helpers sourced by every agent script.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOGS_DIR="$ROOT/logs"
RUN_DIR="$ROOT/run"
BIN_DIR="$ROOT/target/debug"

mkdir -p "$LOGS_DIR" "$RUN_DIR"

# ── PID helpers ───────────────────────────────────────────────────────────────

pid_file() { printf '%s/%s.pid' "$RUN_DIR" "$1"; }
log_file()  { printf '%s/%s.log' "$LOGS_DIR" "$1"; }

is_running() {
    local f; f="$(pid_file "$1")"
    [[ -f "$f" ]] && kill -0 "$(cat "$f")" 2>/dev/null
}

# ── Process lifecycle ─────────────────────────────────────────────────────────

# start_bg <name> <cmd> [args...]
# Starts a background process, writes PID, appends stdout+stderr to log file.
start_bg() {
    local name="$1"; shift
    if is_running "$name"; then
        echo "[$name] Already running (PID $(cat "$(pid_file "$name")"))"
        return 0
    fi
    local logf; logf="$(log_file "$name")"
    nohup "$@" >>"$logf" 2>&1 &
    local pid=$!
    echo "$pid" > "$(pid_file "$name")"
    echo "[$name] Started (PID $pid)  log → $logf"
}

stop_bg() {
    local name="$1"
    local f; f="$(pid_file "$name")"
    if is_running "$name"; then
        kill "$(cat "$f")"
        rm -f "$f"
        echo "[$name] Stopped"
    else
        rm -f "$f"
        echo "[$name] Not running"
    fi
}

# ── Health-check helpers ──────────────────────────────────────────────────────

# wait_http <name> <url> [timeout_seconds=120]
wait_http() {
    local name="$1" url="$2" timeout="${3:-120}"
    local n=0
    printf '  [%s] Waiting for %s' "$name" "$url"
    until curl -sf "$url" >/dev/null 2>&1; do
        sleep 2; n=$((n + 2))
        printf '.'
        if (( n > timeout )); then
            echo " TIMEOUT (${timeout}s)" >&2
            return 1
        fi
    done
    echo " ready"
}

# ── Binary guard ──────────────────────────────────────────────────────────────

require_binary() {
    local bin="$BIN_DIR/$1"
    if [[ ! -x "$bin" ]]; then
        echo "[ERROR] Binary not found: $bin" >&2
        echo "  Build it first: cargo build -p $(echo "$1" | tr '-' '_')" >&2
        exit 1
    fi
}

# ── Colour helpers (used by status) ──────────────────────────────────────────

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
NC='\033[0m'

ok()   { printf "${GREEN}%-12s${NC}" "$1"; }
fail() { printf "${RED}%-12s${NC}"   "$1"; }
warn() { printf "${YELLOW}%-12s${NC}" "$1"; }
