#!/usr/bin/env bash
# arb-pulse system status — prints a one-screen overview of all agents.
source "$(dirname "$0")/common.sh"

_svc_line() {
    local name="$1" health_url="${2:-}"
    if is_running "$name"; then
        local pid; pid=$(cat "$(pid_file "$name")")
        if [[ -n "$health_url" ]]; then
            local resp
            resp=$(curl -sf "$health_url" 2>/dev/null) || resp="(API unreachable)"
            printf "  $(ok "RUNNING")  %-26s PID=%-7s %s\n" "$name" "$pid" "$resp"
        else
            printf "  $(ok "RUNNING")  %-26s PID=%s\n" "$name" "$pid"
        fi
    else
        printf "  $(fail "STOPPED")  %s\n" "$name"
    fi
}

echo ""
echo "═══════════════════════════════════════════════════════"
echo "  arb-pulse  status  $(date '+%Y-%m-%d %H:%M:%S')"
echo "═══════════════════════════════════════════════════════"

# ── Infrastructure ────────────────────────────────────────────────────────────
echo ""
echo "Infrastructure (docker-compose):"
cd "$ROOT"
if docker-compose ps 2>/dev/null | grep -qE 'postgres|redis'; then
    docker-compose ps 2>/dev/null \
        | grep -E 'postgres|redis' \
        | awk '{printf "  %-30s %s\n", $1, $NF}'
else
    echo "  $(fail "DOWN")  docker services not running"
fi

# ── Rust agents ───────────────────────────────────────────────────────────────
echo ""
echo "Rust agents:"
_svc_line "pool-registry-metadata"  "http://127.0.0.1:3001/health"
_svc_line "pool-registry-price"     "http://127.0.0.1:3002/health"
_svc_line "pool-registry-tvl"       "http://127.0.0.1:3003/health"
_svc_line "listener"                "http://127.0.0.1:3000/health"
_svc_line "opportunity-finder"
_svc_line "broadcaster"
_svc_line "arb-api"                 "http://127.0.0.1:4000/health"
_svc_line "web-frontend"

# ── Contract ──────────────────────────────────────────────────────────────────
echo ""
echo "Contract:"
addr=$(grep 'contract\s*=' "$ROOT/config.toml" 2>/dev/null | awk -F'"' '{print $2}' | head -1)
if [[ "$addr" == "0x0000000000000000000000000000000000000000" || -z "$addr" ]]; then
    printf "  $(warn "PENDING")  ArbExecutor not deployed  (run: make deploy-contract)\n"
else
    printf "  $(ok "DEPLOYED") ArbExecutor = %s\n" "$addr"
fi

# ── Redis quick-check ─────────────────────────────────────────────────────────
echo ""
echo "Redis:"
if redis-cli -u redis://127.0.0.1:6379 ping 2>/dev/null | grep -q PONG; then
    pool_keys=$(redis-cli -u redis://127.0.0.1:6379 keys 'pool:*' 2>/dev/null | wc -l | tr -d ' ')
    printf "  $(ok "REACHABLE") pool keys=%s\n" "$pool_keys"
else
    printf "  $(fail "UNREACHABLE")\n"
fi

echo ""
