#!/usr/bin/env python3
"""Fee-on-transfer prober.

Runs the FotDetector runtime via an eth_call state override: funds a scratch
address with the pair's base token, performs a normal V2 swap to pull a slice of
the target token to itself, and returns (requested, received). A standard ERC20
returns received == requested; a fee-on-transfer token returns received <
requested. The contract reads reserves and token ordering itself.

Usage: probe.py <pair> <base> <tokenOut> <baseSlot>
Prints: <tokenOut> <pair> <verdict> <requested> <received> <bps_taken> [err]
"""
import json
import subprocess
import sys
import urllib.request
from pathlib import Path

RPC = "https://pulsechain-rpc.publicnode.com"
DETECTOR = "0x00000000000000000000000000000000000d3733"
RUNTIME = Path(__file__).with_name("detector.runtime").read_text().strip()


from functools import lru_cache


@lru_cache(maxsize=None)
def keccak(b):
    out = subprocess.run(
        ["cast", "keccak", "0x" + b.hex()], capture_output=True, text=True, check=True
    ).stdout.strip()
    return bytes.fromhex(out[2:])


def h32(x):
    return int(x).to_bytes(32, "big")


def addr32(a):
    return bytes(12) + bytes.fromhex(a[2:].rjust(40, "0"))


def rpc(method, params):
    req = urllib.request.Request(
        RPC,
        data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
        headers={"content-type": "application/json", "User-Agent": "fot-probe/1.0"},
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def discover_slot(token, holder):
    """Find the ERC20 balance storage slot of `token` by matching a known
    holder's balance against candidate slots (Solidity and Vyper layouts)."""
    bal = rpc("eth_call", [
        {"to": token, "data": "0x70a08231" + addr32(holder).hex()}, "latest",
    ])
    if "error" in bal:
        return None
    target = int(bal["result"], 16)
    if target == 0:
        return None
    for slot in range(60):
        for preimage in (addr32(holder) + h32(slot), h32(slot) + addr32(holder)):
            key = "0x" + keccak(preimage).hex()
            v = rpc("eth_getStorageAt", [token, key, "latest"])
            if "result" in v and int(v["result"], 16) == target:
                return slot
    return None


SELECTOR = None  # computed once


def measure_selector():
    global SELECTOR
    if SELECTOR is None:
        SELECTOR = "0x" + keccak(b"measure(address,address,address)").hex()[:8]
    return SELECTOR


def probe(pair, base, token_out, base_slot):
    # Fund the scratch detector with a huge base balance (covers any reserve).
    fund = 2**200
    data = measure_selector() + b"".join(
        [addr32(pair), addr32(base), addr32(token_out)]
    ).hex()

    bal_key = "0x" + keccak(addr32(DETECTOR) + h32(base_slot)).hex()
    overrides = {
        DETECTOR: {"code": RUNTIME},
        base: {"stateDiff": {bal_key: "0x" + h32(fund).hex()}},
    }
    resp = rpc("eth_call", [{"to": DETECTOR, "data": data}, "latest", overrides])

    if "error" in resp:
        return ("ERROR", 0, 0, 0, resp["error"].get("message", "")[:60])
    out = resp["result"][2:]
    requested = int(out[0:64], 16)
    received = int(out[64:128], 16)
    bps = 0 if requested == 0 else round((requested - received) * 10000 / requested)
    # Flag FOT only when a meaningful fraction is taken (>=0.01%), so sub-wei
    # rounding never produces a false positive.
    verdict = "FOT" if bps >= 1 else "CLEAN"
    return (verdict, requested, received, bps, "")


if __name__ == "__main__":
    pair, base, token_out = sys.argv[1], sys.argv[2], sys.argv[3]
    base_slot = int(sys.argv[4])
    v, rq, rc, bps, err = probe(pair, base, token_out, base_slot)
    print(f"{token_out} {pair} {v} {rq} {rc} {bps} {err}")
