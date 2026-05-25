#!/usr/bin/env python3
import json, urllib.request

RPC = "https://rpc.pulsechain.com"
BLOCK = hex(26619737)  # 0x1962f59

USDC = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
AMOUNT_IN = 10_000_000_000  # 1e10 raw, 6 decimals -> 10,000 USDC
FEE_BPS = 30
LOAN_FEE_BPS = 0

HOPS = [
    ("0xf4597648e3c7124ea68faf5cc18a80e970502aee", "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "0x037645963aece8c5beb947da5621362c9f7db5a6"),
    ("0x7a0b9916d620d54fadfd4139180deea7e3fb4119", "0x037645963aece8c5beb947da5621362c9f7db5a6", "0xa1077a294dde1b09bb078844df40758a5d0f9a27"),
    ("0x6444456960c3f95b5b408f4d9e00220643f06f94", "0xa1077a294dde1b09bb078844df40758a5d0f9a27", "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"),
]

_id = 0
def call(to, data, block):
    global _id; _id += 1
    payload = {"jsonrpc":"2.0","id":_id,"method":"eth_call",
               "params":[{"to":to,"data":data}, block]}
    req = urllib.request.Request(RPC, data=json.dumps(payload).encode(),
                                 headers={"Content-Type":"application/json"})
    r = json.loads(urllib.request.urlopen(req, timeout=30).read())
    if "error" in r: raise RuntimeError(r["error"])
    return r["result"]

def get_reserves(pool, block):
    res = call(pool, "0x0902f1ac", block)[2:]
    r0 = int(res[0:64], 16)
    r1 = int(res[64:128], 16)
    return r0, r1

def token0(pool, block):
    return "0x" + call(pool, "0x0dfe1681", block)[2:][24:64]

def amount_out(amt_in, r_in, r_out, fee_bps):
    fee_mult = 10000 - fee_bps
    a = amt_in * fee_mult
    return (a * r_out) // (r_in * 10000 + a)

def run(block, label):
    print(f"\n=== {label} (block {block}) ===")
    amt = AMOUNT_IN
    for i, (pool, tin, tout) in enumerate(HOPS):
        r0, r1 = get_reserves(pool, block)
        t0 = token0(pool, block).lower()
        if t0 == tin.lower():
            r_in, r_out = r0, r1
        else:
            r_in, r_out = r1, r0
        out = amount_out(amt, r_in, r_out, FEE_BPS)
        print(f"hop{i+1} {pool[:10]} r_in={r_in} r_out={r_out}  {amt} -> {out}")
        amt = out
    repay = AMOUNT_IN * (10000 + LOAN_FEE_BPS) // 10000
    profit = amt - repay
    print(f"final_out={amt}  repay={repay}  profit_raw={profit}  profit_human={profit/1e6:.6f}")
    return amt, profit

if __name__ == "__main__":
    print("Stored opp #297: expected_out=10189549882 profit_raw=189549882 profit_human=189.549882")
    try:
        run(BLOCK, "AT DISCOVERY BLOCK 26619737")
    except Exception as e:
        print("archival call failed:", e)
    run("latest", "CURRENT/LATEST")
