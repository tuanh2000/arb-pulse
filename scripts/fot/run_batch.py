#!/usr/bin/env python3
"""Run the FOT prober over a worklist concurrently.

Worklist lines: <tokenOut>|<pair>|<base>|<baseSlot>
Writes results.tsv: token<TAB>pair<TAB>verdict<TAB>requested<TAB>received<TAB>bps<TAB>err
"""
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from probe import probe

HERE = Path(__file__).parent
WORKLIST = HERE / "worklist.txt"
RESULTS = HERE / "results.tsv"
WORKERS = 8


def run_one(line):
    token, pair, base, slot = line.split("|")
    for attempt in range(3):
        try:
            v, rq, rc, bps, err = probe(pair, base, token, int(slot))
            return (token, pair, v, rq, rc, bps, err)
        except Exception as e:  # noqa: BLE001
            last = str(e)
    return (token, pair, "ERROR", 0, 0, 0, last[:60])


def main():
    lines = [l.strip() for l in WORKLIST.read_text().splitlines() if l.strip()]
    done = 0
    counts = {"CLEAN": 0, "FOT": 0, "ERROR": 0}
    with RESULTS.open("w") as out, ThreadPoolExecutor(max_workers=WORKERS) as ex:
        for r in ex.map(run_one, lines):
            token, pair, v, rq, rc, bps, err = r
            counts[v if v in counts else "ERROR"] = counts.get(v, 0) + 1
            out.write(f"{token}\t{pair}\t{v}\t{rq}\t{rc}\t{bps}\t{err}\n")
            done += 1
            if done % 100 == 0:
                out.flush()
                print(
                    f"{done}/{len(lines)}  CLEAN={counts['CLEAN']} "
                    f"FOT={counts['FOT']} ERROR={counts['ERROR']}",
                    flush=True,
                )
    print(
        f"DONE {done}/{len(lines)}  CLEAN={counts['CLEAN']} "
        f"FOT={counts['FOT']} ERROR={counts['ERROR']}",
        flush=True,
    )


if __name__ == "__main__":
    main()
