#!/usr/bin/env bash
# Agent: pool-registry-tvl — DEX pair seeder + TVL worker.
# Deps: PostgreSQL (infra). First run seeds all pairs (~5 min).
# Metrics: :9107/metrics
source "$(dirname "$0")/common.sh"
NAME="pool-registry-tvl"
BIN="$BIN_DIR/pool-registry-tvl"
API_PORT="${REGISTRY_TVL_API_PORT:-3003}"

cmd="${1:-start}"

case "$cmd" in

start)
    require_binary "pool-registry-tvl"
    REGISTRY_API_PORT="$API_PORT" start_bg "$NAME" "$BIN"
    wait_http "$NAME" "http://127.0.0.1:${API_PORT}/health" 30
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
        curl -s "http://127.0.0.1:${API_PORT}/health" 2>/dev/null || true
        echo
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
