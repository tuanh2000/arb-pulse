#!/usr/bin/env bash
# Fee-on-transfer prober. Given a pair, the target token, and whether the target
# is token0, runs the deployless FotDetector via eth_call and prints:
#   <token> <pair> CLEAN|FOT|UNKNOWN <requested> <received> <bps_taken>
set -uo pipefail

RPC="${RPC:-https://pulsechain-rpc.publicnode.com}"
export ETH_RPC_URL="$RPC"
BYTECODE="$(cat "$(dirname "$0")/detector.bytecode")"

probe() {
  local pair="$1" token="$2" is_token0="$3"

  # Reserves to size a safe amountOut (a fraction of the target token's reserve).
  local reserves
  reserves=$(cast call "$pair" "getReserves()(uint112,uint112,uint32)" 2>/dev/null)
  if [[ -z "$reserves" ]]; then echo "$token $pair UNKNOWN 0 0 0 noreserves"; return; fi
  local r0 r1
  r0=$(echo "$reserves" | sed -n '1p' | awk '{print $1}')
  r1=$(echo "$reserves" | sed -n '2p' | awk '{print $1}')

  local target_reserve bool_arg
  if [[ "$is_token0" == "true" ]]; then target_reserve="$r0"; bool_arg="true"; else target_reserve="$r1"; bool_arg="false"; fi

  # amountOut = reserve / 1000 (must be >0 and < reserve). Skip dust pools.
  local amount_out
  amount_out=$(python3 -c "print(max(1, int($target_reserve)//1000))")
  if [[ "$target_reserve" == "0" ]]; then echo "$token $pair UNKNOWN 0 0 0 zeroreserve"; return; fi

  # Build initcode = creation bytecode ++ abi.encode(pair, token, bool, amountOut)
  local args initcode raw
  args=$(cast abi-encode "f(address,address,bool,uint256)" "$pair" "$token" "$bool_arg" "$amount_out")
  initcode="${BYTECODE}${args:2}"

  # Deployless eth_call: the detector reverts with abi.encode(requested, received).
  raw=$(cast call --create "$initcode" 2>&1)

  # cast prints revert payload; extract the trailing 0x... (128 hex chars = 2 words)
  local hexdata
  hexdata=$(echo "$raw" | grep -oE '0x[0-9a-fA-F]{128}' | tail -1)
  if [[ -z "$hexdata" ]]; then
    echo "$token $pair UNKNOWN 0 0 0 revert:$(echo "$raw" | tr '\n' ' ' | cut -c1-80)"
    return
  fi

  local requested received bps verdict
  requested=$(cast --to-dec "0x${hexdata:2:64}")
  received=$(cast --to-dec "0x${hexdata:66:64}")
  bps=$(python3 -c "rq=int('$requested'); rc=int('$received'); print(0 if rq==0 else round((rq-rc)*10000/rq))")
  if [[ "$received" == "$requested" ]]; then verdict="CLEAN"; else verdict="FOT"; fi
  echo "$token $pair $verdict $requested $received $bps"
}

probe "$@"
