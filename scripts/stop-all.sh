#!/usr/bin/env bash
# Graceful full-stack shutdown — stops agents in reverse dependency order.
source "$(dirname "$0")/common.sh"

SCRIPTS="$ROOT/scripts"

echo "arb-pulse: stopping all agents..."

"$SCRIPTS/agent-broadcaster.sh"       stop 2>/dev/null || true
"$SCRIPTS/agent-opportunity-finder.sh" stop 2>/dev/null || true
"$SCRIPTS/agent-listener.sh"          stop 2>/dev/null || true
"$SCRIPTS/agent-pool-registry.sh"     stop 2>/dev/null || true
"$SCRIPTS/agent-infra.sh"             stop 2>/dev/null || true

echo "Done."
