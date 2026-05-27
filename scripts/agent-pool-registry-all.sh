#!/usr/bin/env bash
# Starts / stops / restarts all three pool-registry services together:
#   pool-registry-tvl      (seeder + TVL worker,  API :3003, metrics :9107)
#   pool-registry-price    (price oracle,          API :3002, metrics :9106)
#   pool-registry-metadata (metadata + FoT/meme,  API :3001, metrics :9105)
#
# All three processes are launched first, then health-checked in parallel so
# the slow TVL chain-seeding (~5 min on first run) does not delay the others.
set -euo pipefail
source "$(dirname "$0")/common.sh"

SCRIPTS="$(dirname "$0")"
METADATA_PORT="${REGISTRY_METADATA_API_PORT:-3001}"
PRICE_PORT="${REGISTRY_PRICE_API_PORT:-3002}"
TVL_PORT="${REGISTRY_TVL_API_PORT:-3003}"

cmd="${1:-start}"

case "$cmd" in

start)
    echo "==> Starting all pool-registry services"

    require_binary "pool-registry-tvl"
    require_binary "pool-registry-price"
    require_binary "pool-registry-metadata"

    # Launch all three immediately — no waiting between them.
    REGISTRY_API_PORT="$TVL_PORT"      start_bg "pool-registry-tvl"      "$BIN_DIR/pool-registry-tvl"
    REGISTRY_API_PORT="$PRICE_PORT"    start_bg "pool-registry-price"     "$BIN_DIR/pool-registry-price"
    REGISTRY_API_PORT="$METADATA_PORT" start_bg "pool-registry-metadata"  "$BIN_DIR/pool-registry-metadata"

    # All three APIs start immediately (heavy work runs in background tasks),
    # so a 30-second timeout is sufficient for all of them.
    wait_http "pool-registry-price"    "http://127.0.0.1:${PRICE_PORT}/health"     30
    wait_http "pool-registry-metadata" "http://127.0.0.1:${METADATA_PORT}/health"  30
    wait_http "pool-registry-tvl"      "http://127.0.0.1:${TVL_PORT}/health"       30

    echo "==> All pool-registry services running"
    echo "  metadata  http://127.0.0.1:${METADATA_PORT}/health"
    echo "  price     http://127.0.0.1:${PRICE_PORT}/health"
    echo "  tvl       http://127.0.0.1:${TVL_PORT}/health"
    ;;

stop)
    echo "==> Stopping all pool-registry services"
    stop_bg "pool-registry-metadata"
    stop_bg "pool-registry-price"
    stop_bg "pool-registry-tvl"
    echo "==> All pool-registry services stopped"
    ;;

restart)
    "$0" stop
    sleep 1
    "$0" start
    ;;

status)
    "$SCRIPTS/agent-pool-registry-tvl.sh"      status
    "$SCRIPTS/agent-pool-registry-price.sh"    status
    "$SCRIPTS/agent-pool-registry-metadata.sh" status
    ;;

*)
    echo "Usage: $0 {start|stop|restart|status}" >&2
    exit 1
    ;;
esac
