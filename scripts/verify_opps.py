#!/usr/bin/env python3
"""Replay stored opportunities against on-chain reserves at their discovery block."""
import json, subprocess, urllib.request, sys

RPC = "https://rpc.pulsechain.com"
FEE_BPS_DEFAULT = 30
LOAN_FEE_BPS = 0

def db_rows(limit, order="ASC"):
    sql = (f"SELECT id, block_number, amount_in_raw, expected_out_raw, profit_raw, "
           f"profit_human, hops::text FROM opportunities ORDER BY id {order} LIMIT {limit};")
    out = subprocess.check_output([
        "docker","exec","arb-pulse-postgres-1","psql","-U","arbpulse","-d","arbpulse",
        "-t","-A","-F","\x1f","-c", sql]).decode()
    rows=[]
    for line in out.strip().splitlines():
        if not line: continue
        f=line.split("\x1f")
        rows.append(dict(id=int(f[0]), block=int(f[1]), amount_in=int(f[2]),
                         expected_out=int(f[3]), profit_raw=int(f[4]),
                         profit_human=float(f[5]), hops=json.loads(f[6])))
    return rows

_id=0
def call(to,data,block):
    global _id;_id+=1
    p={"jsonrpc":"2.0","id":_id,"method":"eth_call","params":[{"to":to,"data":data},block]}
    req=urllib.request.Request(RPC,data=json.dumps(p).encode(),headers={"Content-Type":"application/json"})
    r=json.loads(urllib.request.urlopen(req,timeout=30).read())
    if "error" in r: raise RuntimeError(r["error"])
    return r["result"]

def reserves(pool,block):
    res=call(pool,"0x0902f1ac",block)[2:]
    return int(res[0:64],16), int(res[64:128],16)

def token0(pool,block):
    return ("0x"+call(pool,"0x0dfe1681",block)[2:][24:64]).lower()

def amount_out(amt,r_in,r_out,fee_bps):
    a=amt*(10000-fee_bps)
    return (a*r_out)//(r_in*10000+a)

def verify(opp):
    blk=hex(opp["block"])
    amt=opp["amount_in"]
    for h in opp["hops"]:
        r0,r1=reserves(h["pool"],blk)
        t0=token0(h["pool"],blk)
        r_in,r_out=(r0,r1) if t0==h["token_in"].lower() else (r1,r0)
        amt=amount_out(amt,r_in,r_out,h.get("fee_bps",FEE_BPS_DEFAULT))
    repay=opp["amount_in"]*(10000+LOAN_FEE_BPS)//10000
    profit=amt-repay
    match = (amt==opp["expected_out"] and profit==opp["profit_raw"])
    print(f"opp #{opp['id']} @block {opp['block']}  hops={len(opp['hops'])}")
    print(f"   stored : out={opp['expected_out']} profit_raw={opp['profit_raw']} ({opp['profit_human']:.6f})")
    print(f"   replay : out={amt} profit_raw={profit} ({profit/1e6:.6f})")
    print(f"   => {'MATCH (faithful to on-chain reserves at block)' if match else 'MISMATCH'}")
    return match

if __name__=="__main__":
    n=int(sys.argv[1]) if len(sys.argv)>1 else 5
    order=sys.argv[2] if len(sys.argv)>2 else "ASC"
    for opp in db_rows(n,order):
        try:
            verify(opp)
        except Exception as e:
            print(f"opp #{opp['id']}: replay error: {e}")
        print()
