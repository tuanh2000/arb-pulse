# PulseChain Node & Validator Hardware Requirements

## Background

PulseChain is a full-state fork of Ethereum, using the same two-layer client architecture:

- **Execution layer (EL):** go-pulse (fork of go-ethereum / geth)
- **Consensus layer (CL):** prysm-pulse, lighthouse-pulse, or other Ethereum consensus clients ported to PulseChain

Both clients must run simultaneously and communicate over the Engine API. Hardware requirements below reflect running **both on a single machine**, which is the standard setup.

---

## Full Node Requirements

A full node participates in the network, validates transactions, and maintains chain state. It does **not** require staked PLS or validator keys.

### Minimum

| Component | Spec |
|---|---|
| CPU | 4 cores @ 2.0+ GHz (x86-64) |
| RAM | 16 GB |
| Storage | 2 TB SSD (SATA or NVMe) |
| Network | 25 Mbps download / 10 Mbps upload, stable |
| OS | 64-bit Linux, macOS 11+, or Windows 10+ 64-bit |

### Recommended

| Component | Spec |
|---|---|
| CPU | 4–8 cores @ 2.8+ GHz (Intel Core i7 / AMD Ryzen 7 or better) |
| RAM | 32 GB |
| Storage | 2 TB NVMe SSD (PCIe Gen 3 or Gen 4) |
| Network | 100+ Mbps, unlimited data plan, wired connection |
| OS | 64-bit Linux (Ubuntu 22.04 LTS recommended) |

> **SSD is mandatory.** Spinning HDDs cannot keep up with the random I/O demands of the execution client. Budget/DRAM-less SSDs may also struggle — use a mainstream NVMe drive (e.g., Samsung 870/980, WD Black, Seagate FireCuda).

---

## Validator Requirements

A validator proposes and attests to blocks, earning staking rewards. It requires:
- A running full node (execution + consensus clients)
- A validator client process (lightweight, runs alongside the consensus client)
- Deposited PLS locked in the deposit contract

Hardware for the validator client itself adds minimal overhead (~100 MB RAM, negligible CPU). The bottleneck is always the full node underneath it.

### Minimum (Validator)

Same as full node minimum, with the following additions:

| Component | Spec |
|---|---|
| RAM | 16 GB (32 GB strongly recommended for reliability) |
| Storage | 2 TB SSD — validators should avoid disk-full conditions |
| Power | UPS (uninterruptible power supply) recommended |
| Uptime | 24/7 — downtime causes inactivity penalties ("leaking") |

### Recommended (Validator)

| Component | Spec |
|---|---|
| CPU | 4–8 cores @ 2.8+ GHz |
| RAM | 32 GB |
| Storage | 2–4 TB NVMe SSD |
| Network | 100+ Mbps, unlimited data, wired, low-latency |
| Power | UPS to protect against power cuts |
| Uptime | Near 100% target — co-locate with reliable power and ISP |

> **Key generation security:** It is strongly recommended to generate validator keys and mnemonics on an air-gapped machine (never connected to the internet). Transfer only the signed deposit data and keystores to the online validator machine.

---

## Storage Breakdown

Storage grows over time. Plan ahead:

| Component | Sync-from-checkpoint | Full genesis sync | Notes |
|---|---|---|---|
| go-pulse (EL) | ~650 GB | ~650 GB | Snap sync; grows ~14 GB/week |
| prysm-pulse / Lighthouse (CL) | ~200 GB | ~1 TB | Checkpoint sync recommended |
| **Total (recommended)** | **~850 GB** | **~1.7 TB** | Start with 2 TB; 4 TB gives headroom |
| Archive node (EL) | 12+ TB | 12+ TB | Not needed for normal node/validator |

- **Use checkpoint sync** for the consensus layer (see `docs/pulsechain-checkpoint-sync.md`). This reduces CL sync time to minutes and initial disk usage to ~200 GB.
- go-pulse supports `--gcmode=full` (default, pruned) or `--gcmode=archive`. Use archive only for data analysis, not for validators.
- Pruning go-pulse occasionally brings disk back to ~650 GB.

---

## Network Ports

Open these on your router and firewall for good peer connectivity:

| Client | Protocol | Port | Purpose |
|---|---|---|---|
| go-pulse | TCP + UDP | 30303 | Execution P2P |
| prysm-pulse | TCP | 13000 | Consensus P2P |
| prysm-pulse | UDP | 12000 | Consensus discovery |
| Lighthouse | TCP + UDP | 9000 | Consensus P2P |

---

## Client Comparison (Consensus Layer)

| Client | Language | RAM Usage | Notes |
|---|---|---|---|
| **prysm-pulse** | Go | ~4–6 GB | Most common for PulseChain; Prysm fork |
| **lighthouse-pulse** | Rust | ~3–5 GB | Efficient, Lighthouse fork |
| **Nimbus** | Nim | ~1–2 GB | Most lightweight; runs on low-power hardware |
| **Teku** | Java | ~4–8 GB | Enterprise-grade; higher RAM floor |
| **Lodestar** | TypeScript | ~4–6 GB | Good for JS ecosystem |

Nimbus is the only consensus client officially documented as capable of running on Raspberry Pi class hardware (though validators on such hardware are not recommended).

---

## Node Types Compared

| Type | Syncs | Stores | Hardware | Use case |
|---|---|---|---|---|
| **Full node** | From checkpoint or genesis | Recent state only (pruned) | Standard specs above | General participation, RPC |
| **Archive node** | Genesis | All historical state | 12+ TB SSD | Historical queries, analytics |
| **Validator** | Full node + validator client | Same as full node | Full node + UPS + uptime | Staking rewards |
| **Light client** | Headers only | Minimal | Very low spec | Verification only, no P2P duties |

---

## Recommended Home Hardware Examples

### Budget (Minimum viable validator)
- **CPU:** Intel Core i5-10400 / AMD Ryzen 5 5600
- **RAM:** 16 GB DDR4
- **SSD:** 2 TB NVMe (e.g., Samsung 980)
- **Network:** Gigabit home fibre, wired

### Reliable (Recommended validator)
- **CPU:** Intel Core i7-12700 / AMD Ryzen 7 5700X
- **RAM:** 32 GB DDR4
- **SSD:** 2 TB NVMe (e.g., Samsung 980 Pro or WD Black SN850)
- **Network:** Gigabit fibre, wired, with 4G LTE failover
- **Power:** UPS (e.g., APC Back-UPS 1500VA)

### Server / Bare Metal
- **CPU:** Intel Xeon E-2300 / AMD EPYC 7003 (4+ cores)
- **RAM:** 32–64 GB ECC
- **SSD:** 4 TB NVMe RAID-1 or enterprise NVMe
- **Network:** Dedicated 1 Gbps with SLA

---

## Cloud / VPS Considerations

Running a validator on a VPS is **not recommended** for solo staking. Reasons:
- Centralization risk (many validators on same provider = correlated failures)
- VPS providers can throttle disk I/O, causing missed attestations
- You do not physically control your signing keys

If using cloud for a **non-validating full node** (e.g., for RPC):

| Provider | Instance type | Notes |
|---|---|---|
| Hetzner | AX41-NVMe or AX52 | 64 GB RAM, 2×512 GB NVMe; good value |
| OVH | Advance-3 or Rise-3 | NVMe options, EU datacentres |
| AWS | i3.large or m6i.2xlarge + EBS gp3 | Higher cost; gp3 needs 3000+ IOPS |

Hetzner Helsinki is used by the official PulseChain RPC nodes (as seen in checkpoint.pulsechain.com status API).

---

## Key Operational Notes

- **Inactivity penalties:** A validator that goes offline does not get slashed immediately but loses rewards proportional to downtime. During a mass-offline event (>1/3 of stake offline), an "inactivity leak" begins, draining stake rapidly.
- **Slashing:** Caused by double-voting or surround-voting — happens if you run the same validator keys on two machines simultaneously. Never run duplicate validator instances.
- **Client diversity:** Using a minority client (Lighthouse, Nimbus, Teku) improves network health and reduces your slashing risk if the majority client has a bug.
- **Monitoring:** Set up Prometheus + Grafana alerts for disk space, CPU load, and missed attestations. The official install scripts at [tdslaine/install_pulse_node](https://github.com/tdslaine/install_pulse_node) include monitoring setup.

---

## Quick Reference Summary

| Scenario | CPU | RAM | Storage | Network |
|---|---|---|---|---|
| Full node (min) | 4 cores | 16 GB | 2 TB SSD | 25 Mbps |
| Full node (rec) | 4–8 cores @ 2.8 GHz | 32 GB | 2 TB NVMe | 100 Mbps |
| Validator (min) | 4 cores | 16 GB | 2 TB SSD + UPS | 25 Mbps |
| Validator (rec) | 4–8 cores @ 2.8 GHz | 32 GB | 2–4 TB NVMe + UPS | 100 Mbps |
| Archive node | 8+ cores | 64 GB | 16+ TB NVMe | 100 Mbps |

---

## References

- go-ethereum hardware docs (go-pulse is a fork): https://geth.ethereum.org/docs/getting-started/hardware-requirements
- Prysm system requirements: https://prysm.offchainlabs.com/docs/install/install-with-docker
- Teku system requirements: https://docs.teku.consensys.io/get-started/system-requirements
- Nimbus hardware guide: https://nimbus.guide/hardware.html
- Ethereum.org run-a-node guide: https://ethereum.org/en/run-a-node/
- PulseChain checkpoint sync: https://checkpoint.pulsechain.com
- Community node installer: https://github.com/tdslaine/install_pulse_node
