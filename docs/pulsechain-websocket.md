# PulseChain WebSocket (wss) Reference

## Overview

PulseChain's WebSocket endpoint exposes the standard Ethereum JSON-RPC pub/sub API
(`eth_subscribe` / `eth_unsubscribe`). It is **not** a custom protocol — everything
below is built on the same wire format as Ethereum mainnet.

**Public endpoints:**

| Endpoint | Client | Notes |
|---|---|---|
| `wss://rpc.pulsechain.com` | Go-Pulse (geth fork) | Official, best mempool throughput |
| `wss://rpc-pulsechain.g4mm4.io` | Likely Go-Pulse | Community-operated, same API surface |
| `wss://pulsechain.publicnode.com` | Erigon-Pulse | Lower `newPendingTransactions` throughput |

Only `eth`, `net`, and `web3` namespaces are enabled on all public nodes. The
`txpool`, `debug`, and `trace` namespaces require a private node.

---

## Answer: Does the WebSocket Include Mempool Transactions?

**Yes — partially.** The subscription `eth_subscribe ["newPendingTransactions"]` does
work on `wss://rpc.pulsechain.com` and emits transaction **hashes** as they enter the
node's local txpool. PulseChain has no Flashbots-style private relay, so all
transactions flow through public p2p gossip and are visible here.

However there are important caveats:

- **Confirmed-only `logs`:** The `logs` subscription only fires after a block is
  mined. There is no pending-log stream.
- **Hash-only by default:** You get a 32-byte hash, not the full transaction.
  Append `true` to get full bodies (see format below).
- **Erigon nodes are slow:** Erigon-Pulse streams roughly 3–8 pending tx/s vs.
  much higher rates on Go-Pulse. Use the official `rpc.pulsechain.com` endpoint for
  mempool work.
- **Coverage depends on p2p connectivity:** A public RPC node sees a subset of
  mempool traffic proportional to its peer connections. A private, well-connected
  Go-Pulse node will see more.
- **`txpool_*` not available publicly:** You cannot call `txpool_content` or
  `txpool_status` on public endpoints. Those namespaces are only accessible on a
  node you run yourself with `--http.api=eth,net,web3,txpool`.

**Bottom line for DEX arbitrage:** Pending transaction monitoring is **critical for
competitive arbitrage**. Bots that only react to confirmed `Sync` events are always
one full block (~10s) behind. A bot watching the mempool can see a pending swap,
simulate the resulting reserves, and submit its arb tx in the **same block** as the
swap. By the time you see the `Sync` event, those bots have already captured the
opportunity. See the [Back-Running Strategy](#back-running-strategy) section below.

---

## Subscription Types

### Wire Protocol

All subscriptions follow the same pattern:

```json
// Subscribe request
{"jsonrpc":"2.0","id":1,"method":"eth_subscribe","params":["<type>", <optional-params>]}

// Response — contains the subscription ID
{"jsonrpc":"2.0","id":1,"result":"0x9cef478923ff08bf67fde6c64013158d"}

// Subsequent notifications (pushed by the server)
{
  "jsonrpc": "2.0",
  "method": "eth_subscription",
  "params": {
    "subscription": "0x9cef478923ff08bf67fde6c64013158d",
    "result": { ... }
  }
}

// Unsubscribe
{"jsonrpc":"2.0","id":2,"method":"eth_unsubscribe","params":["0x9cef478923ff08bf67fde6c64013158d"]}
```

---

### 1. `newHeads` — New Block Headers

Fires every time a new block is imported (including reorgs — orphaned headers are
re-sent with the same block number but a different hash).

**Subscribe:**
```json
{"jsonrpc":"2.0","id":1,"method":"eth_subscribe","params":["newHeads"]}
```

**Notification payload:**
```json
{
  "jsonrpc": "2.0",
  "method": "eth_subscription",
  "params": {
    "subscription": "0x...",
    "result": {
      "parentHash":       "0x...",
      "sha3Uncles":       "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
      "miner":            "0x...",
      "stateRoot":        "0x...",
      "transactionsRoot": "0x...",
      "receiptsRoot":     "0x...",
      "logsBloom":        "0x...",
      "difficulty":       "0x0",
      "number":           "0x12a4f0",
      "gasLimit":         "0x...",
      "gasUsed":          "0x...",
      "timestamp":        "0x...",
      "extraData":        "0x...",
      "nonce":            "0x0000000000000000",
      "hash":             "0x..."
    }
  }
}
```

**Notes:**
- `difficulty` is `"0x0"` on PoSA consensus.
- `transactions` array and `size` are present in Go-Pulse responses but may be
  absent in Erigon-Pulse responses.
- `totalDifficulty` may be absent in Erigon-Pulse responses.

**Use in this bot:** Not currently subscribed. Could be used to drive a "block
heartbeat" — on each new head, re-sync any pools whose reserves have not been
updated in the last N blocks (guard against missed `Sync` events).

---

### 2. `logs` — Event Logs (Confirmed Only)

Fires when a matching log is included in a mined block. On chain reorganisation,
already-emitted logs are re-sent with `"removed": true`.

**Subscribe (with filter):**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "eth_subscribe",
  "params": [
    "logs",
    {
      "address": ["0xPairAddr1", "0xPairAddr2"],
      "topics":  ["0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"]
    }
  ]
}
```

Omitting `address` returns all matching events chain-wide (very high volume —
always use an address filter or the `topics` filter alone for global subscriptions).

**Notification payload:**
```json
{
  "jsonrpc": "2.0",
  "method": "eth_subscription",
  "params": {
    "subscription": "0x...",
    "result": {
      "address":          "0xPairAddress",
      "topics":           ["0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"],
      "data":             "0x<32-byte reserve0><32-byte reserve1>",
      "blockNumber":      "0x12a4f0",
      "transactionHash":  "0x...",
      "transactionIndex": "0x0",
      "blockHash":        "0x...",
      "logIndex":         "0x3",
      "removed":          false
    }
  }
}
```

**Decoding `Sync(uint112 reserve0, uint112 reserve1)`:**
- `data` is exactly 64 bytes: first 32 = `reserve0` (big-endian), last 32 = `reserve1`.
- The topic `0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1` is
  the keccak256 of `"Sync(uint112,uint112)"`.

This is the subscription type currently used in `crates/listener/src/listener/ws.rs`.

---

### 3. `newPendingTransactions` — Mempool Transactions

Fires when a transaction enters the node's local txpool (before confirmation).
`blockHash`, `blockNumber`, and `transactionIndex` are always `null` in these
notifications.

**Subscribe (hashes only):**
```json
{"jsonrpc":"2.0","id":1,"method":"eth_subscribe","params":["newPendingTransactions"]}
```

**Notification — hash only:**
```json
{
  "jsonrpc": "2.0",
  "method": "eth_subscription",
  "params": {
    "subscription": "0x...",
    "result": "0xd6fdc5cc41a9959e922f30cb772a9aef46f4daea279307bc5f7024edc4ccd7fa"
  }
}
```

**Subscribe (full transaction body):**
```json
{"jsonrpc":"2.0","id":1,"method":"eth_subscribe","params":["newPendingTransactions", true]}
```

**Notification — full body:**
```json
{
  "jsonrpc": "2.0",
  "method": "eth_subscription",
  "params": {
    "subscription": "0x...",
    "result": {
      "blockHash":              null,
      "blockNumber":            null,
      "transactionIndex":       null,
      "from":                   "0x...",
      "to":                     "0x...",
      "hash":                   "0x...",
      "nonce":                  "0x5",
      "value":                  "0x0",
      "gas":                    "0x30d40",
      "gasPrice":               "0x...",
      "maxFeePerGas":           "0x...",
      "maxPriorityFeePerGas":   "0x...",
      "input":                  "0x...",
      "type":                   "0x2",
      "v": "0x...", "r": "0x...", "s": "0x..."
    }
  }
}
```

**Caveat:** On Erigon-Pulse nodes, the `"from"` field may be absent (known bug,
fix status unconfirmed in Erigon-Pulse fork). On Go-Pulse, `"from"` is always present.

---

### 4. `syncing` — Sync Status

Returns `false` when the node is fully synced, or a progress object when actively
syncing. Rarely useful after the node is caught up.

```json
{"jsonrpc":"2.0","id":1,"method":"eth_subscribe","params":["syncing"]}
```

---

## Maintaining Local Pool State Without On-Chain Queries

The existing architecture in this bot already demonstrates the correct pattern.
Here is the full two-phase approach:

### Phase 1 — Bootstrap (One-Time On-Chain Scan)

On startup, enumerate all pairs from factory contracts and fetch initial reserves via
Multicall3. This is what `crates/listener/src/listener/init.rs` does:

1. Call `factory.allPairsLength()` → get total pair count.
2. Batch `factory.allPairs(i)` for all `i` in chunks of 300 via Multicall3.
3. For each pair address, batch `token0()`, `token1()`, `getReserves()` (then a
   second multicall for `decimals()`), in chunks of 60 pairs.
4. Populate `PoolStore` with `PoolState { pair_address, token0, token1, reserve0,
   reserve1, ... }`.

After this, your in-memory store is an exact mirror of on-chain state at block N.

### Phase 2 — Continuous Updates via WebSocket

Subscribe to global `Sync` events and apply each one as a delta update:

```
eth_subscribe ["logs", {"topics": ["0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"]}]
```

On each notification:
- `log.address` is the pair contract that fired `Sync`.
- `log.data[0..32]` = new `reserve0`, `log.data[32..64]` = new `reserve1`.
- `log.blockNumber` = which block this came from.
- Update `store.update_reserves(pair_addr, r0, r1, block_number)`.

This is what `crates/listener/src/listener/ws.rs` does. After startup you never need
to call `getReserves()` again for any pair that has emitted at least one `Sync` event.

### Handling Edge Cases

**Chain reorganisations:** When a reorg happens, the WebSocket re-sends the same logs
with `"removed": true`. You must handle this:

```
if log.removed {
    // The block containing this Sync was orphaned.
    // The correct response is to re-query getReserves() for this pair,
    // or to wait for the replacement block's Sync event.
    // For arbitrage, the simplest safe approach is to mark the pool as
    // stale and skip it until a non-removed Sync arrives.
}
```

The current implementation in `ws.rs` does not handle `removed: true` — it applies
all updates unconditionally. This is safe for most cases because the replacement
canonical block will fire another `Sync` within the next block time (~10s), but
it can briefly cause you to use stale reserves.

**New pairs (created after startup):** Factory contracts emit a `PairCreated` event
when a new pair is deployed. To catch new pairs without a full rescan:

```
// Subscribe to PairCreated as well:
// topic = keccak256("PairCreated(address,address,address,uint256)")
//       = 0x0d3648bd0f6ba80134a33ba9275ac585d9d315f0ad8355cddefde31afa28d0e9
eth_subscribe ["logs", {
  "address": ["<factory_address>"],
  "topics": ["0x0d3648bd0f6ba80134a33ba9275ac585d9d315f0ad8355cddefde31afa28d0e9"]
}]
```

On each `PairCreated` event, call `getReserves()` + `token0()` + `token1()` once
to bootstrap that single pair, then let future `Sync` events keep it current.

**Missed events (connection drop):** When the WebSocket reconnects, you must
re-sync any pools that may have changed while disconnected. The safest approach:

1. Note the `blockNumber` from the last event you processed before the drop.
2. After reconnecting, call `eth_getLogs` with `fromBlock = <last_known_block>` to
   fetch all `Sync` events you missed.
3. Apply them in order, then start the live subscription from `"latest"`.

The current `ws.rs` reconnects (up to 10 retries with 2s delay) but does not
perform a gap-fill. This means after a reconnect, reserve values for pools that
traded during the outage are stale until those pools trade again.

### Architecture Diagram

```
                 ┌─────────────────────────────────┐
                 │          Startup (once)          │
                 │                                  │
                 │  HTTP RPC → Multicall3           │
                 │  → enumerate all pairs           │
                 │  → fetch token0/token1/reserves  │
                 │  → populate PoolStore            │
                 └──────────────┬──────────────────┘
                                │
                                ▼
                 ┌─────────────────────────────────┐
                 │     WebSocket Subscription       │
                 │                                  │
                 │  wss://rpc.pulsechain.com        │
                 │  eth_subscribe ["logs", {        │
                 │    topics: [Sync topic]           │
                 │  }]                              │
                 │                                  │
                 │  On each Sync log:               │
                 │    parse reserve0, reserve1      │
                 │    store.update_reserves(...)    │
                 └──────────────┬──────────────────┘
                                │
                                ▼
                 ┌─────────────────────────────────┐
                 │          PoolStore               │
                 │  DashMap<Address, PoolState>     │
                 │  Always current — never stale    │
                 │  (modulo reorgs and gap on drop) │
                 └─────────────────────────────────┘
                                │
                                ▼
                 ┌─────────────────────────────────┐
                 │         PathFinder               │
                 │  Reads PoolStore directly        │
                 │  No on-chain queries at runtime  │
                 └─────────────────────────────────┘
```

---

## Back-Running Strategy

### Why Confirmed Sync Events Are Not Enough

When a user submits a swap, two things happen in sequence:

```
Mempool: user swap tx appears
  ↓
Block N mined: swap executes → Sync event fires
  ↓
Our bot sees Sync → submits arb tx
  ↓
Block N+1: arb tx lands (~10s later)
```

A mempool-watching bot compresses this to:

```
Mempool: user swap tx appears
  ↓
Bot decodes calldata → simulates resulting reserves → submits arb tx
  ↓
Block N: swap tx mines, then arb tx mines IN THE SAME BLOCK
```

By the time we see the `Sync` event, the opportunity is already gone.

### How to Implement Back-Running (Without Flashbots)

PulseChain has no private relay. Transaction ordering within a block is determined
by the validator — typically by gas price (descending). The approach:

1. Subscribe to `newPendingTransactions` with full bodies:
   ```json
   {"method":"eth_subscribe","params":["newPendingTransactions", true]}
   ```

2. For each pending tx, check if `to` matches a known DEX router or pair address.
   Decode the `input` calldata to identify swap function selectors:
   - `0x38ed1739` — `swapExactTokensForTokens`
   - `0x8803dbee` — `swapTokensForExactTokens`
   - `0x022c0d9f` — `swap` (low-level V2 pair call)
   - `0x414bf389` — `exactInputSingle` (V3)

3. Simulate the post-swap reserves locally using the AMM formula and your current
   `PoolStore` state. For a V2 swap of `amountIn` of `token0` → `token1`:
   ```
   amountInWithFee = amountIn * (10000 - fee_bps)
   amountOut = (amountInWithFee * reserve1) / (reserve0 * 10000 + amountInWithFee)
   new_reserve0 = reserve0 + amountIn
   new_reserve1 = reserve1 - amountOut
   ```

4. Run PathFinder against the simulated reserves. If an arb path is profitable,
   construct and submit the arb tx immediately.

5. Set the arb tx gas price **equal to the pending swap tx's gas price**. This
   places them in the same priority tier — the validator will likely include both in
   the same block. The smart contract atomically reverts if unprofitable, so a failed
   back-run only costs gas.

### Gas Price Coordination (Without a Bundle Relay)

| Arb gas price vs. swap gas price | Result |
|---|---|
| Higher | Arb tx mines **before** the swap → reserves unchanged → arb reverts |
| Equal | Both in same priority tier → validator ordering determines outcome |
| Lower | Arb tx likely misses the block → lands in next block → opportunity gone |

Setting gas price equal is the standard no-relay back-run approach. The atomic
revert in the smart contract makes failed attempts safe — you lose only the gas cost
of a reverted tx.

A more aggressive approach: set gas price slightly **above** the swap and accept that
your tx may land before the swap in some cases (it will revert harmlessly), while in
other cases the validator orders them correctly and you capture the profit.

### What Needs to Change in the Listener

The current listener only subscribes to confirmed `Sync` logs. To support
back-running, it needs a second subscription path:

```
Current flow:
  Sync (confirmed log) → update PoolStore → PathFinder reads next block

Additional flow needed:
  Pending swap tx → decode calldata → simulate reserves → PathFinder fires NOW
                                                        → arb tx targets same block
```

The `PoolStore` already holds live reserves (updated by confirmed Sync events) which
serve as the baseline for simulation. The simulation does not mutate the store —
it passes projected values directly to PathFinder.

---

## Known Limitations and Quirks

| Issue | Impact | Mitigation |
|---|---|---|
| `logs` subscription is confirmed-only | ~10s delay vs. mempool | Accept it; only confirmed reserves can be traded against anyway |
| Erigon `newPendingTransactions` max ~8 tx/s | Can't use PublicNode for mempool monitoring | Use `wss://rpc.pulsechain.com` (Go-Pulse) |
| Erigon `newHeads` may omit `size`/`transactions`/`totalDifficulty` | Parsing errors if you expect those fields | Check for nil/absent before reading |
| Erigon `newPendingTransactions` full body may omit `from` | Need to decode from `v,r,s` sig if required | Only call with `true` on Go-Pulse endpoints |
| No `txpool_*` on public endpoints | Cannot snapshot the full mempool | Run a private Go-Pulse node |
| No Flashbots on PulseChain | Full mempool is public — sandwich bots have same visibility | Plan accordingly; no protection |
| G4MM4 higher latency for US users | Slower event delivery during peak hours | Use official endpoint as primary |
| Silent WebSocket disconnects | Missed events, stale reserves | Implement ping every 25s; reconnect + gap-fill |
| Reorg logs arrive with `removed: true` | Stale reserves for ~10s | Mark pool stale; skip until next confirmed Sync |
| No pending logs | Cannot front-run based on mempool swap intent | Accepted limitation |

---

## Connection Keep-Alive

Public WebSocket servers will drop idle connections. Send a JSON-RPC ping every ~25
seconds to keep the connection alive:

```json
{"jsonrpc":"2.0","id":999,"method":"eth_blockNumber","params":[]}
```

The response keeps the TCP session alive. If no response arrives within a timeout
(e.g. 30s), treat the connection as dead and reconnect from scratch.

The current `ws.rs` reconnect loop handles crashes but not silent idle drops. For
production, add a keepalive task that fires `eth_blockNumber` on a 25s timer and
resets a watchdog; trigger a reconnect if the watchdog expires without a response.
