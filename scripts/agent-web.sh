#!/usr/bin/env bash
# Agent: web — arb-api backend (:4000) + Next.js frontend (:3100).
# Deps: PostgreSQL (infra). DATABASE_URL must be set in the environment or .env.
# The frontend talks to the API via NEXT_PUBLIC_API_URL (default http://localhost:4000).
set -euo pipefail
source "$(dirname "$0")/common.sh"

WEB_DIR="$ROOT/web"
ARB_API_NAME="arb-api"
NEXT_NAME="web-frontend"
ARB_API_BIN="$BIN_DIR/arb-api"
ARB_API_PORT="${ARB_API_PORT:-4000}"
NEXT_PORT="${NEXT_PORT:-3100}"

cmd="${1:-start}"

start_arb_api() {
    require_binary "arb-api"
    if [[ -z "${DATABASE_URL:-}" ]]; then
        # Try to load from .env at the repo root.
        if [[ -f "$ROOT/.env" ]]; then
            set -a; source "$ROOT/.env"; set +a
        fi
        if [[ -z "${DATABASE_URL:-}" ]]; then
            echo "[ERROR] DATABASE_URL is not set. Export it or add it to $ROOT/.env" >&2
            exit 1
        fi
    fi
    DATABASE_URL="$DATABASE_URL" \
    ARB_API_PORT="$ARB_API_PORT" \
    start_bg "$ARB_API_NAME" "$ARB_API_BIN"
    wait_http "$ARB_API_NAME" "http://127.0.0.1:${ARB_API_PORT}/health" 30
}

start_next() {
    if [[ ! -d "$WEB_DIR/node_modules" ]]; then
        echo "[web-frontend] node_modules not found — running npm install..."
        (cd "$WEB_DIR" && npm install --silent)
    fi
    local logf; logf="$(log_file "$NEXT_NAME")"
    if is_running "$NEXT_NAME"; then
        echo "[$NEXT_NAME] Already running (PID $(cat "$(pid_file "$NEXT_NAME")"))"
        return 0
    fi
    nohup npm --prefix "$WEB_DIR" run dev -- --port "$NEXT_PORT" \
        >>"$logf" 2>&1 &
    local pid=$!
    echo "$pid" > "$(pid_file "$NEXT_NAME")"
    echo "[$NEXT_NAME] Started (PID $pid)  log → $logf"
    wait_http "$NEXT_NAME" "http://127.0.0.1:${NEXT_PORT}" 60
    echo "  Frontend: http://localhost:${NEXT_PORT}"
}

case "$cmd" in

start)
    echo "==> Starting web stack"
    start_arb_api
    start_next
    echo "==> Web stack ready"
    echo "  API:      http://localhost:${ARB_API_PORT}"
    echo "  Frontend: http://localhost:${NEXT_PORT}/tokens"
    ;;

stop)
    echo "==> Stopping web stack"
    stop_bg "$NEXT_NAME"
    stop_bg "$ARB_API_NAME"
    echo "==> Web stack stopped"
    ;;

restart)
    "$0" stop
    sleep 1
    "$0" start
    ;;

status)
    if is_running "$ARB_API_NAME"; then
        echo -n "[$ARB_API_NAME] Running (PID $(cat "$(pid_file "$ARB_API_NAME")"))  "
        curl -s "http://127.0.0.1:${ARB_API_PORT}/health" 2>/dev/null || true
        echo
    else
        echo "[$ARB_API_NAME] Stopped"
    fi
    if is_running "$NEXT_NAME"; then
        echo "[$NEXT_NAME] Running (PID $(cat "$(pid_file "$NEXT_NAME")"))  http://localhost:${NEXT_PORT}/tokens"
    else
        echo "[$NEXT_NAME] Stopped"
    fi
    ;;

logs-api)
    tail -f "$(log_file "$ARB_API_NAME")"
    ;;

logs-frontend)
    tail -f "$(log_file "$NEXT_NAME")"
    ;;

*)
    echo "Usage: $0 {start|stop|restart|status|logs-api|logs-frontend}" >&2
    exit 1
    ;;
esac
