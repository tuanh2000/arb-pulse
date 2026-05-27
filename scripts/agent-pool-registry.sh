#!/usr/bin/env bash
# Compatibility shim: delegates to agent-pool-registry-all.sh.
# The monolithic pool-registry binary was split into three services:
#   pool-registry-tvl / pool-registry-price / pool-registry-metadata
# This script keeps existing callers (start-all.sh, stop-all.sh) working.
exec "$(dirname "$0")/agent-pool-registry-all.sh" "${1:-start}"
