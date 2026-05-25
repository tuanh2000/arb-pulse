#!/usr/bin/env bash
# Agent: opportunity-finder — cycle enumeration + optimal sizing + Redis emitter.
# Deps: Redis pool snapshot written by listener (pool_updates channel).
# Config: config.toml  (override with CONFIG_PATH).
# Publishes profitable opportunities to the Redis channel defined in finder.output_channel.
source "$(dirname "$0")/common.sh"
NAME="opportunity-finder"
BIN="$BIN_DIR/opportunity-finder"

cmd="${1:-start}"

case "$cmd" in

start)
    require_binary "opportunity-finder"

    # Precondition: listener must have written its Redis snapshot.
    # Quick proxy: listener's HTTP API is up.
    if ! curl -sf http://127.0.0.1:3000/health >/dev/null 2>&1; then
        echo "[$NAME] ERROR: listener not reachable at :3000 — start it first" >&2
        exit 1
    fi

    start_bg "$NAME" "$BIN"
    echo "[$NAME] Running (no HTTP API — check logs for 'Initial scan complete')"
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
        echo "[$NAME] Running (PID $(cat "$(pid_file "$NAME")"))"
        # Show the last emitted opportunity count from logs if available.
        last=$(grep -m 1 'Initial scan complete\|emitted' "$(log_file "$NAME")" 2>/dev/null | tail -1) || true
        [[ -n "$last" ]] && echo "  last: $last"
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
