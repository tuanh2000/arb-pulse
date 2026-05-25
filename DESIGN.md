# PulseChain Arbitrage Bot — Research & Design

## PulseChain Fundamentals

| Parameter | Value |
|---|---|
| Chain ID | **369** (testnet: 943) |
| Native Token | **PLS** |
| Block Time | ~10 seconds |
| EVM | 1:1 Ethereum opcode compatibility |
| Consensus | PoSA (top 33 validators) |
| Gas Cost | ~59× cheaper than Ethereum |
| Fee Model | EIP-1559 (base fee burned + priority tip) |

Solidity works identically to Ethereum. Use Hardhat or Foundry with network URL `https://rpc.pulsechain.com`.

---

## Key DEXes & Contract Addresses

| DEX | Type | Factory Address | Fee |
|---|---|---|---|
| PulseX V2 | Uniswap V2 (x\*y=k) | `0x29eA7545DEf87022BAdc76323F373EA1e707C523` | 0.30% |
| PulseX StableSwap | Curve-style | `0xE3acFA6C40d53C3faf2aa62D0a715C737071511c` | low |
| PulseX V1 (deprecated) | Uniswap V2 | `0x1715a3E4A142d8b698131108995174F37aEBA10D` | 0.30% |
| 9inch V2 | Uniswap V2 | `0x3a0Fa7884dD93f3cd234bBE2A0958Ef04b05E13b` | ~0.30% |
| 9inch V3 | Concentrated liquidity | verify on scan.pulsechain.com | multi-tier |
| 9mm V2 | Uniswap V2 | verify on scan.pulsechain.com | **0.17%** |
| 9mm V3 | Concentrated liquidity | verify on scan.pulsechain.com | multi-tier |
| PHUX | Balancer V2 | verify on ph-defi.gitbook.io | varies |

**Other key addresses:**

| Contract | Address |
|---|---|
| WPLS | `0xA1077a294dDE1B09bB078844df40758a5D0f9a27` |
| Multicall3 | `0xcA11bde05977b3631167028862bE2a173976CA11` |
| PulseX Router V2 | `0x165C3410fC91EF562C50559f7d2289fEbed552d9` |
| PLSX Token | `0x95B303987A60C71504D99Aa1b13B4DA07b0790ab` |
| PulseX MasterChef | `0xB2Ca4A66d3e57a5a9A12043B6bAD28249fE302d4` |

---

## RPC Endpoints

| Provider | HTTP | WebSocket | Notes |
|---|---|---|---|
| Official | `https://rpc.pulsechain.com` | `wss://rpc.pulsechain.com` | Free, no SLA, no archive |
| G4MM4 | `https://rpc-pulsechain.g4mm4.io` | `wss://rpc-pulsechain.g4mm4.io` | Free; exposes `txpool`, `trace`, `debug` APIs |
| PublicNode | `https://pulsechain-rpc.publicnode.com` | `wss://pulsechain-rpc.publicnode.com` | Free, ~637 RPS |
| Dwellir | `https://api-pulse-mainnet.n.dwellir.com/{KEY}` | yes | Paid; archive; trace/debug |
| Testnet | `https://rpc.v4.testnet.pulsechain.com` | — | chainId 943 |

**Recommended:** G4MM4 WebSocket for event subscriptions (exposes txpool for MEV awareness); official HTTP as fallback.

---

## Data Sources

| Purpose | Source |
|---|---|
| Real-time pool events | `wss://rpc-pulsechain.g4mm4.io` via `eth_subscribe("logs")` |
| Batch state reads | Multicall3 `0xcA11bde05977b3631167028862bE2a173976CA11` |
| Pair discovery | PulseX subgraph `https://graph.pulsechain.com/subgraphs/name/pulsechain/pulsex` |
| Contract verification | `https://scan.pulsechain.com` (Blockscout API: `https://api.scan.pulsechain.com/api`) |
| Gas tracking | `owlracle.info/pulse`, `beacon.pulsechain.com/gasnow` |
| Archive / trace | Dwellir paid RPC |

---

## Full System Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        OFF-CHAIN BOT                            │
│                                                                 │
│  ┌────────────────┐      ┌───────────────────┐                  │
│  │   LISTENER     │─────▶│   PATH FINDER     │                  │
│  │                │      │                   │                  │
│  │ • WS subscribe │      │ • Read pool state │                  │
│  │   to Sync evts │      │ • Enumerate paths │                  │
│  │ • Multicall3   │      │ • getAmountOut    │                  │
│  │   batch fetch  │      │   simulation      │                  │
│  │ • In-memory    │      │ • Filter: profit  │                  │
│  │   reserve store│      │   > gas cost      │                  │
│  └────────────────┘      └─────────┬─────────┘                  │
│                                    │ profitable path             │
│                          ┌─────────▼─────────┐                  │
│                          │   TX SENDER       │                  │
│                          │                   │                  │
│                          │ • Build calldata  │                  │
│                          │ • EIP-1559 gas    │                  │
│                          │   pricing         │                  │
│                          │ • Submit tx       │                  │
│                          └─────────┬─────────┘                  │
└────────────────────────────────────┼────────────────────────────┘
                                     │
                         ┌───────────▼────────────┐
                         │   ON-CHAIN CONTRACT    │
                         │                        │
                         │ 1. Flash borrow tokenA │
                         │    from PulseX V2 pair │
                         │ 2. Swap A→B on DEX2    │
                         │ 3. Swap B→A on DEX1    │
                         │ 4. Repay + 0.3% fee    │
                         │ 5. require(profit > 0) │
                         │    ← revert if no gain │
                         │ 6. Transfer profit out │
                         └────────────────────────┘
```

---

## Module 1: Listener

**Goal:** Maintain an always-current in-memory snapshot of pool reserves.

**Startup:**
1. Enumerate all pairs from each factory via `getAllPairsLength()` + `allPairs(i)`, or seed from a subgraph snapshot.
2. Batch-fetch initial reserves using Multicall3 (`aggregate3` calling `getReserves()` on all pairs).
3. Store in memory: `Map<pairAddress, { reserve0, reserve1, token0, token1, fee, dex }>`.

**Runtime:**
1. Open WebSocket to `wss://rpc-pulsechain.g4mm4.io`.
2. Subscribe to `eth_subscribe("logs", { topics: [SYNC_TOPIC] })` — fires on every swap and liquidity change.
3. On each `Sync(uint112 reserve0, uint112 reserve1)` event: update the map entry for `event.address`.
4. Expose a query function: `getPoolState(pairAddress) → PoolState`.

```
Sync event topic:
keccak256("Sync(uint112,uint112)") = 0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1

Swap event topic:
keccak256("Swap(address,uint256,uint256,uint256,uint256,address)") = 0xd78ad95fa46c994b6551d0da85fc275fe613ce37657fb8d5e3d130840159d822
```

**API exported:**
```typescript
getPoolState(pair: string): PoolState | undefined
getAllPools(): Map<string, PoolState>
getPairsForToken(token: string): string[]  // for path enumeration
```

---

## Module 2: PathFinder

**Goal:** On each Sync event, find whether any profitable arbitrage path exists.

**Algorithm (two-hop V2×V2):**
1. Get the updated pool's tokens `(tokenA, tokenB)`.
2. Find all other pools that contain `tokenA` or `tokenB`.
3. Simulate forward: `out1 = getAmountOut(amountIn, r0_dex1, r1_dex1)`.
4. Simulate reverse: `out2 = getAmountOut(out1, r1_dex2, r0_dex2)`.
5. Profit = `out2 - amountIn`. Net profit = profit minus estimated gas cost.
6. If net profit > `minProfitThreshold`, emit the path.

**Optimal input amount (V2×V2 closed form):**
```
optimalIn = sqrt(r_a1 * r_b1 * r_a2 * r_b2 * (1-fee)^2) - r_a1
            ─────────────────────────────────────────────────────
                              r_a1 + r_b2
```
(approximate — use binary search for precision or multi-hop paths)

**getAmountOut formula:**
```
// PulseX V2 (0.30% fee): fee_num = 997
// 9mm V2 (0.17% fee):    fee_num = 9983 (over 10000)

amountInWithFee = amountIn * fee_num
amountOut = (amountInWithFee * reserveOut) / (reserveIn * 1000 + amountInWithFee)
```

**Output:**
```typescript
interface ArbPath {
  tokenIn: string
  tokenOut: string        // intermediate token
  amountIn: bigint
  expectedProfit: bigint
  dex1Pair: string
  dex2Router: string
  dex1IsBorrow: boolean   // which side to flash-borrow from
}
```

---

## Module 3: Transaction Sender

**Goal:** Construct and submit the arb transaction fast enough to land in the current or next block.

**Steps:**
1. Receive `ArbPath` from PathFinder.
2. Encode calldata: `arbContract.execute(flashPair, tokenBorrow, amountBorrow, dex2Router, tokenTarget, minProfit)`.
3. Estimate gas: `eth_estimateGas`.
4. Fetch current base fee from latest block header (`baseFeePerGas`).
5. Set `maxFeePerGas = baseFee * 2 + priorityFee`, `maxPriorityFeePerGas = 5–10 gwei`.
6. Sign and submit: `eth_sendRawTransaction`.
7. Wait for receipt; log result.

**No private mempool on PulseChain** — there is no Flashbots equivalent. Priority fee is the only tool for competitive block ordering.

**Gas cost estimate:**
- Simple V2→V2 two-hop arb: ~250,000–400,000 gas
- At 10 gwei base fee: ~0.004 PLS ≈ < $0.001
- Minimum profit floor should still be set (e.g., 0.01 PLS) to avoid dust transactions

---

## Module 4: Smart Contract

**Goal:** Execute the arbitrage atomically on-chain. Revert if unprofitable — no token loss possible.

**Flash loan options:**

| Provider | Mechanism | Fee | Notes |
|---|---|---|---|
| PulseX V2 pairs | Uniswap V2 flash swap | 0.30% | Built-in, no external contract needed |
| 9inch / 9mm V2 pairs | Uniswap V2 flash swap | 0.17–0.30% | Same mechanism |
| PHUX | Balancer V2 flash loan | 0% | Requires IFlashLoanRecipient callback |
| Phiat | AAVE V2 flash loan | 0.09% | Requires IFlashLoanReceiver callback |

**Recommended starting point:** PulseX V2 flash swaps — no external dependency, built into every V2 pair.

**Contract skeleton:**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

interface IUniswapV2Pair {
    function swap(uint amount0Out, uint amount1Out, address to, bytes calldata data) external;
    function token0() external view returns (address);
    function token1() external view returns (address);
}

interface IUniswapV2Router {
    function swapExactTokensForTokens(
        uint amountIn, uint amountOutMin,
        address[] calldata path, address to, uint deadline
    ) external returns (uint[] memory amounts);
}

interface IERC20 {
    function transfer(address to, uint amount) external returns (bool);
    function approve(address spender, uint amount) external returns (bool);
    function balanceOf(address account) external view returns (uint);
}

contract ArbExecutor {
    address public immutable owner;

    constructor() { owner = msg.sender; }

    modifier onlyOwner() { require(msg.sender == owner, "!owner"); _; }

    /// Entry point called by the off-chain bot
    function execute(
        address flashPair,
        address tokenBorrow,
        uint256 amountBorrow,
        address dex2Router,
        address tokenTarget,
        uint256 minProfit
    ) external onlyOwner {
        bytes memory data = abi.encode(dex2Router, tokenTarget, minProfit, amountBorrow);
        bool isToken0 = IUniswapV2Pair(flashPair).token0() == tokenBorrow;
        IUniswapV2Pair(flashPair).swap(
            isToken0 ? amountBorrow : 0,
            isToken0 ? 0 : amountBorrow,
            address(this),
            data
        );
    }

    /// Called back by PulseX V2 pair after sending borrowed tokens
    function uniswapV2Call(
        address /*sender*/,
        uint amount0,
        uint amount1,
        bytes calldata data
    ) external {
        (address dex2Router, address tokenTarget, uint minProfit, uint borrowed) =
            abi.decode(data, (address, address, uint, uint));

        uint amountIn = amount0 > 0 ? amount0 : amount1;
        address tokenIn = amount0 > 0
            ? IUniswapV2Pair(msg.sender).token0()
            : IUniswapV2Pair(msg.sender).token1();

        // Swap tokenIn → tokenTarget on DEX2
        IERC20(tokenIn).approve(dex2Router, amountIn);
        address[] memory path = new address[](2);
        path[0] = tokenIn;
        path[1] = tokenTarget;
        uint[] memory out1 = IUniswapV2Router(dex2Router).swapExactTokensForTokens(
            amountIn, 0, path, address(this), block.timestamp
        );

        // Swap tokenTarget → tokenIn on DEX2 (or DEX1 via another router)
        path[0] = tokenTarget;
        path[1] = tokenIn;
        IERC20(tokenTarget).approve(dex2Router, out1[1]);
        uint[] memory out2 = IUniswapV2Router(dex2Router).swapExactTokensForTokens(
            out1[1], 0, path, address(this), block.timestamp
        );

        // Repay flash loan (borrowed + 0.3% fee)
        uint repay = borrowed * 1003 / 1000;
        require(out2[1] >= repay + minProfit, "not profitable");
        IERC20(tokenIn).transfer(msg.sender, repay);
        // Remaining profit stays in contract
    }

    /// Owner withdraws accumulated profit
    function withdraw(address token, uint amount) external onlyOwner {
        IERC20(token).transfer(owner, amount);
    }

    function withdrawPLS() external onlyOwner {
        payable(owner).transfer(address(this).balance);
    }
}
```

**"Never lose money" guarantee:**
The `require(out2[1] >= repay + minProfit, "not profitable")` check means if the trade is unprofitable, the entire transaction reverts. The only cost of a failed attempt is the gas fee for the reverted tx (~$0.0001 on PulseChain).

---

## Recommended Tech Stack

| Layer | Choice | Reason |
|---|---|---|
| Off-chain bot | TypeScript + ethers.js v6 | Best async/WS support, typed ABIs |
| Smart contract | Solidity 0.8.20 + Foundry | Fast fork testing against PulseChain mainnet |
| RPC (events) | G4MM4 WebSocket | Exposes `txpool`, `trace` |
| RPC (fallback) | `rpc.pulsechain.com` | Free, no auth |
| Pool state store | In-memory `Map` (Node.js) | Sub-millisecond lookup |
| Path search | Graph traversal (BFS/Bellman-Ford on token graph) | Handles multi-hop; negative cycle = arb opportunity |

---

## Suggested Project Structure

```
arb-pulse/
├── contracts/
│   ├── src/
│   │   └── ArbExecutor.sol
│   ├── test/
│   │   └── ArbExecutor.t.sol
│   └── foundry.toml
├── bot/
│   ├── src/
│   │   ├── listener/
│   │   │   ├── index.ts          # WebSocket subscription + Multicall init
│   │   │   └── poolStore.ts      # In-memory reserve map
│   │   ├── pathfinder/
│   │   │   ├── index.ts          # Sync event handler → path search
│   │   │   ├── graph.ts          # Token graph with getAmountOut edges
│   │   │   └── optimizer.ts      # Optimal amountIn computation
│   │   ├── sender/
│   │   │   ├── index.ts          # Tx construction + submission
│   │   │   └── gasStrategy.ts    # EIP-1559 fee calculation
│   │   ├── config.ts             # DEX addresses, WPLS, Multicall3
│   │   └── main.ts               # Wires all modules together
│   ├── package.json
│   └── tsconfig.json
└── DESIGN.md                     # This file
```

---

## Known Gaps to Resolve Before Building

| Gap | How to Resolve |
|---|---|
| 9inch V3 / 9mm V3 factory + router addresses | Search `scan.pulsechain.com` or project Discord/GitHub |
| 9mm V2 factory address confirmation | Verify `0x3a0Fa7884...` deploys 9mm pairs on-chain |
| PHUX Vault address | Check `ph-defi.gitbook.io` or scan for Balancer V2 Vault |
| Phiat LendingPool address | Call `ILendingPoolAddressesProvider.getLendingPool()` |
| Actual block time | Verify on `beacon.pulsechain.com` |
| No Flashbots / private mempool | Accept: high priority fee is the only tool |

---

## MEV & Frontrunning Notes

- PulseChain has **no Flashbots equivalent** — all transactions are public in the mempool.
- MEV competition is significantly less than Ethereum due to smaller searcher ecosystem.
- Defense strategy: submit with high `maxPriorityFeePerGas` (5–10 gwei above baseline) to land in the next block before competitors.
- Contract-level defense: check reserves inside the tx (slippage guard via `amountOutMin`) so a sandwiched tx simply reverts.
- G4MM4's `txpool_content` API can be monitored to detect competing arb transactions.
