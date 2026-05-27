#!/usr/bin/env bash
# Full-stack startup: starts all agents in dependency order.
# Usage:
#   ./scripts/start-all.sh              # start everything (broadcaster skipped if no PRIVATE_KEY)
#   PRIVATE_KEY=0x... ./scripts/start-all.sh   # start including broadcaster
source "$(dirname "$0")/common.sh"

SCRIPTS="$ROOT/scripts"

# Load .env if PRIVATE_KEY not already in environment.
if [[ -z "${PRIVATE_KEY:-}" && -f "$ROOT/.env" ]]; then
    set -o allexport; source "$ROOT/.env"; set +o allexport
fi
[[ -n "${PRIVATE_KEY:-}" && "$PRIVATE_KEY" != 0x* ]] && PRIVATE_KEY="0x$PRIVATE_KEY"
export PRIVATE_KEY

echo ""
echo "════════════════════════════════════════════════"
echo "  arb-pulse  starting all agents"
echo "════════════════════════════════════════════════"
echo ""

# 1. Infrastructure (postgres + redis)
echo "── Step 1/5  Infrastructure ─────────────────────"
"$SCRIPTS/agent-infra.sh" start
echo ""

# 2. Pool registry — all three services (tvl + price + metadata)
echo "── Step 2/5  Pool Registry (all modes) ──────────"
"$SCRIPTS/agent-pool-registry-all.sh" start
echo ""

# 3. Listener (needs redis + pool-registry)
echo "── Step 3/5  Listener ───────────────────────────"
"$SCRIPTS/agent-listener.sh" start
echo ""

# 4. Opportunity finder (needs listener's Redis snapshot)
echo "── Step 4/5  Opportunity Finder ─────────────────"
"$SCRIPTS/agent-opportunity-finder.sh" start
echo ""

# 5. Transaction broadcaster (needs PRIVATE_KEY + deployed contract)
echo "── Step 5/5  Transaction Broadcaster ────────────"
if [[ -z "${PRIVATE_KEY:-}" ]]; then
    echo "  PRIVATE_KEY not set — skipping broadcaster"
    echo "  When ready: export PRIVATE_KEY=0x... && make broadcaster"
else
    "$SCRIPTS/agent-broadcaster.sh" start
fi
echo ""

# Final status snapshot
"$SCRIPTS/status.sh"
