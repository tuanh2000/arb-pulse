#!/usr/bin/env python3
"""Probe the non-WPLS-paired tokens. Worklist: <token>|<pair>|<base>.
Discovers each base's balance slot on demand (using the pair as a known holder),
then runs the standard probe. Appends to results.tsv.
"""
from pathlib import Path

from probe import discover_slot, probe

HERE = Path(__file__).parent
RAW = HERE / "worklist88.raw"
RESULTS = HERE / "results.tsv"

# Known base slots (avoid rediscovery).
SLOT_CACHE = {"0xa1077a294dde1b09bb078844df40758a5d0f9a27": 3}


def main():
    lines = [l.strip() for l in RAW.read_text().splitlines() if l.strip()]
    counts = {"CLEAN": 0, "FOT": 0, "ERROR": 0, "NOSLOT": 0}
    with RESULTS.open("a") as out:
        for line in lines:
            token, pair, base = line.split("|")
            slot = SLOT_CACHE.get(base)
            if slot is None:
                slot = discover_slot(base, pair)
                SLOT_CACHE[base] = slot
            if slot is None:
                counts["NOSLOT"] += 1
                out.write(f"{token}\t{pair}\tNOSLOT\t0\t0\t0\tbase={base}\n")
                continue
            try:
                v, rq, rc, bps, err = probe(pair, base, token, slot)
            except Exception as e:  # noqa: BLE001
                v, rq, rc, bps, err = "ERROR", 0, 0, 0, str(e)[:60]
            counts[v if v in counts else "ERROR"] = counts.get(v, 0) + 1
            out.write(f"{token}\t{pair}\t{v}\t{rq}\t{rc}\t{bps}\t{err}\n")
    print(f"88-batch: {counts}", flush=True)


if __name__ == "__main__":
    main()
