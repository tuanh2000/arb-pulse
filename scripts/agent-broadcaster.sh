#!/usr/bin/env bash
# Agent: transaction-broadcaster — consumes opportunities channel, builds + signs + sends txs.
# Deps: Redis (infra), deployed ArbExecutor contract (agent-contract), opportunity-finder.
# Config: config.toml  (override with CONFIG_PATH).
# REQUIRED env var: PRIVATE_KEY=0x<hex>  (never stored in config files).
source "$(dirname "$0")/common.sh"
NAME="broadcaster"
BIN="$BIN_DIR/transaction-broadcaster"

cmd="${1:-start}"

# Load .env if PRIVATE_KEY not already in environment.
if [[ -z "${PRIVATE_KEY:-}" && -f "$ROOT/.env" ]]; then
    set -o allexport
    source "$ROOT/.env"
    set +o allexport
fi
# Normalise to 0x-prefixed hex.
if [[ -n "${PRIVATE_KEY:-}" && "$PRIVATE_KEY" != 0x* ]]; then
    PRIVATE_KEY="0x$PRIVATE_KEY"
fi

case "$cmd" in

start)
    require_binary "transaction-broadcaster"

    if [[ -z "${PRIVATE_KEY:-}" ]]; then
        echo "[$NAME] ERROR: PRIVATE_KEY not found (tried env + $ROOT/.env)" >&2
        exit 1
    fi

    # Verify contract address is not the zero placeholder.
    contract_addr=$(grep -A1 '^\[broadcaster\]' "$ROOT/config.toml" | grep 'contract\s*=' | awk -F'"' '{print $2}')
    if [[ "$contract_addr" == "0x0000000000000000000000000000000000000000" || -z "$contract_addr" ]]; then
        echo "[$NAME] ERROR: ArbExecutor contract not yet deployed" >&2
        echo "  Run: make deploy-contract  then set the address in config.toml [broadcaster].contract" >&2
        exit 1
    fi

    start_bg "$NAME" "$BIN"
    echo "[$NAME] Listening for opportunities on Redis channel..."
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
        # Show recent arb tx info from logs.
        last_tx=$(grep 'ArbExecuted\|sending\|tx hash' "$(log_file "$NAME")" 2>/dev/null | tail -3) || true
        [[ -n "$last_tx" ]] && echo "$last_tx" | sed 's/^/  /'
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
