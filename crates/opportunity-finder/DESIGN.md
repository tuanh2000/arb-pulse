# Opportunity Finder — Design Notes

> Status: **discussion / pre-implementation**. This document captures how the
> service should work and the math behind it. No code yet.

### MVP scope (decided 2026-05-25)
- **V2-only pools** — PulseX V2, 9inch V2, 9mm V2. Closed-form optimal size
  applies. StableSwap / V3 / PHUX deferred (see §8.2).
- **Single base token** — one configured `token_in` (USDC). Multi-base deferred.
- **Flash-loan funded** — borrow via PulseX flash swap / PHUX (0%) / Phiat, so
  `x*` is capped by available pool liquidity, not our own capital. The borrow
  repayment fee must be subtracted from profit (see §4.3).

## 1. Purpose & place in the system

The Opportunity Finder (a.k.a. PathFinder, module #2 of the bot) consumes live
pool state produced by the **Listener** and emits **profitable arbitrage
opportunities** for the **Transaction Sender** to execute on-chain.

```
Listener ──(Redis pool state + pool_updates)──▶ Opportunity Finder ──(opportunity)──▶ Sender ──▶ Contract
```

A single "opportunity" is a **cyclic trade**: start with N units of a base
token (e.g. USDC), swap through a sequence of pools across one or more DEXes,
and end up with **more** of the same base token than you started with. Because
the trade is a cycle that begins and ends in the same token, profit is
denominated directly in that token and the on-chain contract can guarantee
atomicity (revert if not profitable).

## 2. Input contract (from the Listener)

The Listener writes each pool to Redis. We consume it; we do not own the format.

- Key `pool:{address}` (hash) with fields:
  - `dex`, `token0`, `token1`
  - `reserve0`, `reserve1` (raw integer reserves, as strings)
  - `fee_bps` (e.g. `30` = 0.30%, `17` = 0.17%)
  - `token0_decimals`, `token1_decimals`
  - `block`
- Pub/sub channel `pool_updates`: `{ address, reserve0, reserve1, block }`
  published on every reserve change.

**Implication:** we hold an in-memory mirror of all pools, seed it from the
`pool:*` snapshot on startup, and keep it fresh by subscribing to
`pool_updates`. We recompute opportunities reactively when reserves change.

## 3. Configuration (the knobs we expose)

- `token_in` — the base token we start and end the cycle in (e.g. USDC).
  *(Open question: single base token, or a set? See §8.)*
- `max_hops` — maximum number of swaps in a cycle (path length). Typical values
  2–4. A "hop" = one pool swap. A 2-hop cycle is `USDC→X→USDC` across two
  different pools; 3-hop is `USDC→X→Y→USDC`; etc.
- `min_profit` — minimum net profit (in `token_in`, after gas) to emit.
- `max_trade_size` / capital limit — cap on input so we don't size beyond what
  we can fund (or beyond available flash-loan liquidity).

## 4. The core AMM math (Uniswap V2 / constant product)

Every pool here is `x · y = k`. For a swap, the output for a given input,
including the pool's fee, is:

```
γ = (10_000 − fee_bps) / 10_000          // fee multiplier, e.g. 0.9970 for 30 bps
amountOut = (γ · amountIn · reserveOut) / (reserveIn + γ · amountIn)
```

This is exactly the `getAmountOut` the Listener reference already documents
(PulseX uses 9971/10000 for 0.29%; 9mm 9983/10000 for 0.17%). We must use each
pool's own `fee_bps`.

> **Decimals:** reserves are raw integers; USDC has 6 decimals, WPLS 18, etc.
> Profit comparison and the `min_profit` threshold must normalize by
> `token_in` decimals. Internal optimal-input math is decimal-agnostic (it's
> all ratios), but the *reported* numbers must be human/USD-correct.

### 4.1 Composing a multi-hop path into one virtual pool

A chain of V2 swaps is itself equivalent to a **single** fee-less constant-product
swap of the form `out(x) = E_out · x / (E_in + x)`, with all per-hop fees folded
into `(E_in, E_out)`. We build it **iteratively**, one hop at a time (this is what
`amm.rs::VirtualPool` implements and what was verified by re-derivation):

```
// First hop (reserves r_in, r_out, fee multiplier γ):
E_in  = r_in / γ
E_out = r_out

// Extend with the next hop (r_in, r_out, γ):
denom = r_in + γ · E_out
E_out' = γ · r_out · E_out / denom
E_in'  = r_in · E_in / denom
```

Apply `extend` left to right to collapse an entire N-hop path into a single
`(E_in, E_out)`. This is what makes the optimal-sizing math below work for any
number of hops, not just two.

> Note: an earlier draft of this doc gave a one-shot two-pool formula with the
> fee factors misplaced; the iterative form above is the corrected, verified
> version.

### 4.2 Optimal input amount (closed form)

For the collapsed virtual pool `(E_in, E_out)`, profit as a function of input x
is:

```
profit(x) = amountOut(x) − x = (x · E_out) / (E_in + x) − x
```

(Here the path's fees are already baked into `E_in/E_out`, so we treat the
virtual swap as fee-less; if you keep a residual γ, carry it through.)

Setting `d/dx profit(x) = 0` gives the **profit-maximizing input**:

```
x* = sqrt(E_in · E_out) − E_in
```

and the path is profitable **iff `E_out > E_in`** (equivalently `x* > 0`, i.e.
the product of effective rates around the cycle exceeds 1). Max profit is
`profit(x*)`. This is a true closed form — no iterative solver needed for pure
V2 cycles. (Concentrated-liquidity / StableSwap pools break the closed form and
need numerical optimization — see §8.)

> Sanity: with `max_hops` collapsing to `(E_in, E_out)`, `x*` tells us the exact
> trade size; we then clamp by `max_trade_size` / flash-loan liquidity and
> recompute profit at the clamped size.

### 4.3 Flash-loan cost (MVP is flash-funded)

We borrow `x` units of `token_in`, run the cycle, repay the loan, keep the rest.
Net profit must subtract the borrow fee:

```
net_profit(x) = amountOut(x) − x − loan_fee(x)
```

- **PHUX (Balancer-style): 0% fee** → `loan_fee = 0`, so `x*` and profit are
  exactly §4.2. Preferred source when the asset is available there.
- **PulseX V2 flash swap: ~0.30%** → `loan_fee = x · (1/γ_pool − 1)`. This is a
  linear-in-x cost, so the optimum shifts: maximize `(x·E_out)/(E_in+x) − c·x`
  with `c = 1 + loan_fee_rate`, giving `x* = sqrt(E_in·E_out/c) − E_in`.
- **Phiat (AAVE V2 fork): 0.09%** → same linear-cost form with its own rate.

**Liquidity cap:** `x*` is bounded by the borrowable amount at the chosen
provider; clamp and recompute net profit at the cap.

## 5. Finding candidate cycles

Two complementary techniques:

### 5.1 Screening — Bellman–Ford negative cycle (cheap, approximate)
Model tokens as graph nodes, pools as edges. Weight each edge
`w = −log(effective_rate)` where the effective rate uses the **spot price**
(`γ · reserveOut / reserveIn`). A **negative-weight cycle** ⇒ the rate product
around the loop > 1 ⇒ arbitrage *exists at infinitesimal size*.

- Pro: fast, finds whether opportunities exist and roughly where.
- Con: spot-price based — ignores slippage / trade size, so it tells us a cycle
  *could* be profitable but not by how much, and can flag cycles that vanish at
  realistic size. Use it as a **filter**, not the final answer.

### 5.2 Enumeration — bounded cycle search (exact)
Because `max_hops` is small (2–4) and we anchor at `token_in`, we can directly
enumerate simple cycles `token_in → … → token_in` of length ≤ `max_hops` via
DFS over the token graph. For each enumerated path:
1. collapse to `(E_in, E_out)` (§4.1),
2. check `E_out > E_in`,
3. compute `x*` and exact profit (§4.2),
4. subtract gas estimate, keep if `≥ min_profit`.

This is exact (accounts for slippage at the chosen size) and naturally bounded
by `max_hops`. The graph is large, so we prune aggressively:
- restrict intermediate tokens to a liquid allow-list (WPLS, major stables, etc.),
- drop pools below a TVL/reserve floor (Listener already filters by min TVL),
- use the §5.1 spot-price product as a quick reject before doing the full
  optimal-sizing math.

**Likely plan:** enumeration (§5.2) as the source of truth for MVP, with the
spot-price product as the pruning heuristic. Revisit Bellman–Ford if the graph
grows too big to enumerate within our latency budget.

## 6. Execution / latency model

- React to `pool_updates`: when a pool changes, only cycles touching that pool
  can change profitability → maintain a **pool → cycles** index and re-evaluate
  just the affected cycles instead of the whole graph.
- Pre-compute the candidate cycle set (the structural enumeration) once, refresh
  it only when the pool *set* changes (pools added/removed), not on every
  reserve tick.
- Emit ranked opportunities (by net profit) to the Sender.

## 7. Output contract (to the Sender) — draft

Per opportunity, the Sender needs enough to build the atomic tx:
- ordered list of pools (addresses) + token in/out direction per hop,
- `token_in`, optimal `amount_in` (`x*`, clamped),
- expected `amount_out` and expected net profit,
- min-out / slippage bound for the on-chain revert guard,
- the block number the calc was based on (staleness check).

*(Exact serialization TBD — Redis channel? direct call? matches whatever the
Sender wants.)*

## 8. Open questions / decisions to make

### Resolved (2026-05-25)
- **Base token:** single `token_in` (USDC) for MVP.
- **Pool types:** V2-only for MVP.
- **Funding:** flash-loan / flash-swap (see §4.3).

### 8.2 Still open
1. **Non-V2 pools (later)** — PulseX StableSwap, 9inch/9mm V3 (concentrated
   liquidity), PHUX (Balancer) break the closed-form `x*`; will need per-pool
   `amountOut` + numerical optimization when we add them.
2. **Gas / profit threshold** — how to price gas in `token_in` terms; PulseChain
   gas is cheap but not free, and we compete on priority fee (no Flashbots).
3. **Flash-loan provider selection** — prefer PHUX (0%) when available, fall back
   to PulseX flash swap / Phiat; needs a per-asset liquidity/availability check.
4. **Phase-2 mempool** — Listener will publish *predicted* reserves; do we run a
   second speculative pass on predicted state? (Mirrors Listener's 2-phase.)
5. **Latency budget** — ~10s block time gives slack, but mempool competition is
   the real clock. Need a target (e.g. recompute affected cycles in < X ms).

## 9. References (math sources)
- Uniswap V2 arbitrage optimization (closed-form optimal input).
- "Profit Maximization in Arbitrage Loops" (arXiv 2406.16600) — loop
  profitability condition, multi-hop convex formulation.
- "An Improved Algorithm to Identify More Arbitrage Opportunities on DEXs"
  (arXiv 2406.16573) — line-graph / negative-cycle detection from a source token.
- Bellman–Ford negative-cycle arbitrage with `−log(rate)` edge weights.
</content>
</invoke>
