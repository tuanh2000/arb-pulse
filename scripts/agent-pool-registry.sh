#!/usr/bin/env bash
# Agent: pool-registry — pool database (PostgreSQL) + HTTP API on :3001.
# Deps: PostgreSQL (infra).
# Config: pool-registry-config.toml  (override with REGISTRY_CONFIG_PATH).
# First-run seeds all DEX pairs from chain (~5 min); subsequent starts skip rows already present.
source "$(dirname "$0")/common.sh"
NAME="pool-registry"
BIN="$BIN_DIR/pool-registry"

cmd="${1:-start}"

case "$cmd" in

start)
    require_binary "pool-registry"
    start_bg "$NAME" "$BIN"
    # First-run seeding can take several minutes; give it a generous timeout.
    wait_http "$NAME" "http://127.0.0.1:3001/health" 600
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
        resp=$(curl -s http://127.0.0.1:3001/health 2>/dev/null) || true
        echo "$resp"
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
