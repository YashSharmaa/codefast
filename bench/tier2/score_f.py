"""Family F, read the way the published comparison reads it: file recall at 1 and at 10, and tokens.

Rows come from `live.py score --out FILE`. Tokens are counted as the paper counts them
(arXiv 2611.07286: everything a session processed, cached or not, plus what it wrote) and as
the rest of this benchmark counts them (`warm`, priced by the size of every request).

The first arm of the first file is the one the others are set against. Only the tasks that
every arm ran are counted, so `--arms` also decides how many tasks there are.

usage: python3 -I bench/tier2/score_f.py ROWS.jsonl [MORE.jsonl ...] [--arms "control;ranking card"]
       ROWS.jsonl:plugin=NAME renames an arm of that file, for two builds that ran as the same arm
"""
import argparse
import collections
import json
import random
import statistics

PUBLISHED = [
    ("published, no map (Haiku 4.5, 152 tasks)", 0.572, 0.717, 1_008_163),
    ("published, best map (Hybrid)", 0.526, 0.829, 610_910),
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("rows", nargs="+", help="FILE or FILE:plugin=NAME to rename an arm of that file")
    ap.add_argument("--arms", default="", help="only these arms, separated by a semicolon")
    a = ap.parse_args()
    only = [x.strip() for x in a.arms.split(";") if x.strip()]
    rows = []
    for spec in a.rows:
        path, _, ren = spec.partition(":")
        ren = dict(x.split("=") for x in ren.split(",") if x)
        for l in open(path):
            r = json.loads(l)
            if r.get("family") != "F":
                continue
            r["arm"] = ren.get(r["arm"], r["arm"])
            if not only or r["arm"] in only:
                rows.append(r)
    by = collections.defaultdict(lambda: collections.defaultdict(list))
    for r in rows:
        by[r["arm"]][r["id"]].append(r)
    arms = list(by)
    ids = sorted(set.intersection(*(set(by[x]) for x in arms)))
    mean = lambda xs: sum(xs) / len(xs) if xs else 0.0
    hit = lambda r, k: 1.0 if r.get("rank") and r["rank"] <= k else 0.0
    total = lambda r: r["processed"] + r["output"]
    print(f"{len(ids)} tasks that every arm ran")
    print(f"{'arm':44s} {'fR@1':>6s} {'fR@5':>6s} {'fR@10':>6s} {'tokens':>10s} {'priced':>8s} {'requests':>8s} {'answered':>8s}")
    for label, r1, r10, tok in PUBLISHED:
        print(f"{label:44s} {r1:6.3f} {'':6s} {r10:6.3f} {tok:10,d}")
    base = None
    for arm in arms:
        per = {t: by[arm][t] for t in ids}
        f = lambda fn: mean([mean([fn(r) for r in per[t]]) for t in ids])
        line = (f(lambda r: hit(r, 1)), f(lambda r: hit(r, 5)), f(lambda r: hit(r, 10)), f(total), f(lambda r: r["warm"]), f(lambda r: r["requests"]), f(lambda r: r.get("answered") or 0))
        print(f"{arm:44s} {line[0]:6.3f} {line[1]:6.3f} {line[2]:6.3f} {line[3]:10,.0f} {line[4]:8,.0f} {line[5]:8.1f} {line[6]:8.1f}")
        if base is None:
            base = (arm, line)
        else:
            b = base[1]
            print(f"{'  against ' + base[0]:44s} {line[0] - b[0]:+6.3f} {line[1] - b[1]:+6.3f} {line[2] - b[2]:+6.3f} {line[3] / b[3] - 1:+10.0%} {line[4] / b[4] - 1:+8.0%}")
    if len(arms) >= 2:
        # Task by task against the first arm: how often the file moved into, and out of, the ten.
        a0 = arms[0]
        for arm in arms[1:]:
            win = sum(1 for t in ids if mean([hit(r, 10) for r in by[arm][t]]) > mean([hit(r, 10) for r in by[a0][t]]))
            loss = sum(1 for t in ids if mean([hit(r, 10) for r in by[arm][t]]) < mean([hit(r, 10) for r in by[a0][t]]))
            fewer = sum(1 for t in ids if mean([total(r) for r in by[arm][t]]) < mean([total(r) for r in by[a0][t]]))
            random.seed(3)
            d10, dtok = [], []
            for _ in range(10000):
                s = [random.choice(ids) for _ in ids]
                g = lambda arm_, fn: mean([mean([fn(r) for r in by[arm_][t]]) for t in s])
                d10.append(g(arm, lambda r: hit(r, 10)) - g(a0, lambda r: hit(r, 10)))
                dtok.append(g(arm, total) / g(a0, total) - 1)
            d10.sort()
            dtok.sort()
            print(f"{arm} against {a0}: in the ten where it was not for {win} tasks, out of it for {loss}, fewer tokens in {fewer} of {len(ids)}")
            print(f"  fR@10 difference {d10[250]:+.3f} to {d10[9750]:+.3f} at 95%, tokens {dtok[250]:+.0%} to {dtok[9750]:+.0%}")


if __name__ == "__main__":
    main()
