#!/usr/bin/env bash
# Agent: contract — deploys ArbExecutor to PulseChain and patches config.toml.
# Requires: forge (Foundry), PRIVATE_KEY env var.
# Idempotent: if the address in config.toml is already non-zero, prints it and exits.
source "$(dirname "$0")/common.sh"
NAME="contract"
CONTRACTS_DIR="$ROOT/contracts"
CONFIG="$ROOT/config.toml"
CHAIN_ID=369
BROADCAST_JSON="$CONTRACTS_DIR/broadcast/Deploy.s.sol/$CHAIN_ID/run-latest.json"

cmd="${1:-status}"

_current_address() {
    grep 'contract\s*=' "$CONFIG" | awk -F'"' '{print $2}' | head -1
}

_patch_config() {
    local addr="$1"
    # Replace the contract address line in [broadcaster] section.
    sed -i.bak "s|contract = \"0x[0-9a-fA-F]*\"|contract = \"$addr\"|" "$CONFIG"
    rm -f "$CONFIG.bak"
    echo "[$NAME] config.toml updated: broadcaster.contract = $addr"
}

case "$cmd" in

deploy)
    if [[ -z "${PRIVATE_KEY:-}" ]]; then
        echo "[$NAME] ERROR: PRIVATE_KEY environment variable is not set" >&2
        exit 1
    fi

    # Check if already deployed (non-zero address).
    current=$(_current_address)
    if [[ "$current" != "0x0000000000000000000000000000000000000000" && -n "$current" ]]; then
        echo "[$NAME] ArbExecutor already deployed at $current"
        echo "  Re-deploy anyway? Run: $0 force-deploy"
        exit 0
    fi

    if ! command -v forge >/dev/null 2>&1; then
        echo "[$NAME] ERROR: forge not found — install Foundry: https://getfoundry.sh" >&2
        exit 1
    fi

    echo "[$NAME] Building contracts..."
    cd "$CONTRACTS_DIR"
    forge build

    echo "[$NAME] Deploying ArbExecutor to PulseChain (chain $CHAIN_ID)..."
    PRIVATE_KEY="$PRIVATE_KEY" forge script script/Deploy.s.sol \
        --rpc-url pulsechain \
        --broadcast \
        --legacy \
        2>&1 | tee "$ROOT/logs/deploy.log"

    # Parse deployed address from forge broadcast JSON.
    if [[ -f "$BROADCAST_JSON" ]]; then
        deployed=$(jq -r '.receipts[] | select(.contractAddress != null) | .contractAddress' \
            "$BROADCAST_JSON" 2>/dev/null | head -1)
        if [[ -n "$deployed" && "$deployed" != "null" ]]; then
            echo "[$NAME] Deployed at: $deployed"
            _patch_config "$deployed"
        else
            echo "[$NAME] WARNING: could not parse address from $BROADCAST_JSON" >&2
            echo "  Set it manually in config.toml [broadcaster].contract"
        fi
    else
        echo "[$NAME] WARNING: broadcast JSON not found at $BROADCAST_JSON" >&2
        echo "  Set the deployed address manually in config.toml [broadcaster].contract"
    fi
    ;;

force-deploy)
    # Same as deploy but skips the already-deployed guard.
    current=$(_current_address)
    echo "[$NAME] Force re-deploying (current: $current)..."
    # Temporarily zero out the address so the deploy guard doesn't fire.
    sed -i.bak 's|contract = "0x[^"]*"|contract = "0x0000000000000000000000000000000000000000"|' "$CONFIG"
    "$0" deploy
    ;;

status)
    addr=$(_current_address)
    if [[ "$addr" == "0x0000000000000000000000000000000000000000" || -z "$addr" ]]; then
        echo "[$NAME] Not deployed (zero address in config.toml)"
        echo "  Deploy with: make deploy-contract  (requires PRIVATE_KEY)"
    else
        echo "[$NAME] Deployed at: $addr"
    fi
    ;;

*)
    echo "Usage: $0 {deploy|force-deploy|status}" >&2
    exit 1
    ;;
esac
