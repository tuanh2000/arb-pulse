#!/usr/bin/env bash
# Agent: listener — WebSocket reserve tracker + Redis publisher + HTTP API on :3000.
# Deps: Redis (infra), pool-registry (:3001).
# Config: config.toml  (override with CONFIG_PATH).
# Initialises pool state via Multicall3, writes Redis snapshot, then tracks WS Sync events.
source "$(dirname "$0")/common.sh"
NAME="listener"
BIN="$BIN_DIR/listener"

cmd="${1:-start}"

case "$cmd" in

start)
    require_binary "listener"

    # Precondition: pool-registry must be up so the listener can load its pool list.
    if ! curl -sf http://127.0.0.1:3001/health >/dev/null 2>&1; then
        echo "[$NAME] ERROR: pool-registry not reachable at :3001 — start it first" >&2
        exit 1
    fi

    start_bg "$NAME" "$BIN"
    wait_http "$NAME" "http://127.0.0.1:3000/health" 180
    ;;

stop)
    stop_bg "$NAME"
    ;;

restart)
    stop_bg "$NAME"
    sleep 1
    "$0" start
    ;;

status)
    if is_running "$NAME"; then
        echo -n "[$NAME] Running (PID $(cat "$(pid_file "$NAME")"))  "
        curl -s http://127.0.0.1:3000/health 2>/dev/null || true
        echo ""
    else
        echo "[$NAME] Stopped"
    fi
    ;;

logs)
    tail -f "$(log_file "$NAME")"
    ;;

*)
    echo "Usage: $0 {start|stop|restart|status|logs}" >&2
    exit 1
    ;;
esac
