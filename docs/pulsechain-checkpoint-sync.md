# PulseChain Checkpoint Sync — checkpoint.pulsechain.com

## Overview

**checkpoint.pulsechain.com** is the official PulseChain beacon chain checkpoint sync provider, running the open-source [Checkpointz](https://github.com/ethpandaops/checkpointz) software (v0.24.0, operating in **full** mode). It allows beacon node operators to sync from a recent finalized checkpoint instead of syncing from genesis, cutting sync time from days down to minutes.

- **Public URL:** https://checkpoint.pulsechain.com
- **Brand:** PulseChain Mainnet
- **Software:** Checkpointz v0.24.0-49ddba9 (linux)
- **Mode:** Full (serves both state and block data needed for checkpoint sync)

---

## Network & Chain Config

| Parameter | Value |
|---|---|
| Network name | `pulsechain` |
| Chain ID | `369` |
| Preset base | `pulsechain` |
| Deposit contract | `0x3693693693693693693693693693693693693693` |
| Seconds per slot | `10` |
| Slots per epoch | `32` |
| Min genesis validators | `4096` |
| Genesis delay | `300 s` |
| Max effective balance | `32,000,000,000,000,000` (Gwei) |
| Effective balance increment | `1,000,000,000,000,000` |
| Base reward factor | `64000` |
| Sync committee size | `512` |
| Epochs per sync committee period | `256` |
| ETH1 follow distance | `2048` |

---

## Current Finality (as of 2026-05-27)

| Checkpoint | Epoch | Root |
|---|---|---|
| Finalized | 300215 | `0x689b50190e1967d625c1ac729152d365b80e678e39f509c50fda3c316278085d` |
| Current justified | 300216 | `0x826ab56dbf6f574bc15e658009eca29da1414b21f3ede0a8f658d524d442f4db` |
| Previous justified | 300215 | `0x689b50190e1967d625c1ac729152d365b80e678e39f509c50fda3c316278085d` |

---

## Upstream Beacon Nodes

The checkpoint service aggregates data from 13 healthy upstream beacon nodes hosted in Hetzner Helsinki datacenters (`hel1-dc3`, `hel1-dc5`, `hel1-dc7`):

| Node | Datacenter |
|---|---|
| rpc-core-001-4gjguk | hel1-dc3 |
| rpc-core-002-hurumo | hel1-dc3 |
| rpc-core-003-hzjb3o | hel1-dc3 |
| rpc-core-004-0ecccz | hel1-dc3 |
| rpc-core-011-32wwnd | hel1-dc5 |
| rpc-core-012-n8jhm2 | hel1-dc5 |
| rpc-core-013-m59ac5 | hel1-dc5 |
| rpc-core-014-yvjsdr | hel1-dc5 |
| rpc-core-017-wak558 | hel1-dc7 |
| rpc-core-018-gtshku | hel1-dc7 |
| rpc-core-019-0whp3r | hel1-dc7 |
| rpc-core-020-l2gjgf | hel1-dc7 |
| rpc-core-021-kqac73 | hel1-dc7 |

All 13 nodes were healthy and in consensus at the time of snapshot.

---

## API Endpoints

Base URL: `https://checkpoint.pulsechain.com`

| Endpoint | Description |
|---|---|
| `GET /checkpointz/v1/status` | Service health, upstream list, finality, version |
| `GET /eth/v1/beacon/states/finalized/finality_checkpoints` | Current finality checkpoints |
| `GET /eth/v1/config/spec` | Full beacon chain spec/config |

The service exposes the standard Ethereum Beacon API, meaning all standard `/eth/v1/...` beacon endpoints work for checkpoint sync purposes.

---

## How to Use for Checkpoint Sync

Pass `https://checkpoint.pulsechain.com` as the checkpoint sync URL to your consensus client. Commands per client:

### Lighthouse
```
--checkpoint-sync-url=https://checkpoint.pulsechain.com
```

### Prysm
```
--checkpoint-sync-url=https://checkpoint.pulsechain.com
--genesis-beacon-api-url=https://checkpoint.pulsechain.com
```

### Teku
```
--checkpoint-sync-url=https://checkpoint.pulsechain.com
```

### Lodestar
```
--checkpointSyncUrl=https://checkpoint.pulsechain.com
```

### Nimbus
Requires a separate step to import the checkpoint state before starting. See Nimbus docs for `--state-root` and block import instructions.

---

## What Happens During Checkpoint Sync

When a beacon node checkpoint-syncs, it:

1. Downloads the finalized beacon state and block from the checkpoint provider.
2. Verifies the state root and block root match the checkpoint hashes.
3. Starts syncing forward from that point rather than from genesis.

Example log output from a Lighthouse sync:
```
checkpointSlot=529024
Writing checkpoint state    stateRoot=91e4f512
Writing checkpoint block    blockRoot=13b4cc5f
```

---

## Why Use a Checkpoint Provider?

- Syncing from genesis on PulseChain can take days.
- Checkpoint sync reduces this to minutes by skipping historical state reconstruction.
- Checkpointz runs in **full mode**, meaning it serves both the finalized state and the finalized block — required for a complete checkpoint sync (light mode only serves state roots and is informational only).

---

## References

- Checkpointz GitHub: https://github.com/ethpandaops/checkpointz
- Community checkpoint sync endpoints list: https://github.com/eth-clients/checkpoint-sync-endpoints
