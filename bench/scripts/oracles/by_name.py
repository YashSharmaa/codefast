"""How good are the matches by name? A call on a receiver the index cannot type is offered
under every method of that name. This compares those candidates with the call graph a
compiler or type checker built.

usage: codefast --root REPO export-edges --loose --tests > edges.jsonl
       python3 bench/scripts/oracles/by_name.py --oracle bench/oracles/syft --tool edges.jsonl
"""
import argparse, collections, gzip, json, os


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--oracle", required=True)
    ap.add_argument("--tool", required=True)
    a = ap.parse_args()
    p = os.path.join(a.oracle, "edges.jsonl")
    lines = open(p) if os.path.exists(p) else gzip.open(p + ".gz", "rt")
    truth = collections.defaultdict(set)  # (caller file, site line) -> {(callee file, callee name)}
    for l in lines:
        e = json.loads(l)
        truth[(e["caller"]["file"], e["site_line"])].add((e["callee"]["file"], e["callee"]["name"].split(".")[-1]))
    sure, loose = collections.defaultdict(set), collections.defaultdict(set)
    for l in open(a.tool):
        e = json.loads(l)
        key = (e["caller"]["file"], e["site_line"])
        (loose if e["kind"] == "name" else sure)[key].add((e["callee"]["file"], e["callee"]["name"].split(".")[-1]))
    pairs = sum(len(v) for v in loose.values())
    confirmed = sum(1 for k, v in loose.items() for t in v if t in truth.get(k, ()))
    print(f"candidates by name: {pairs} at {len(loose)} call sites")
    print(f"  the oracle confirms {confirmed} of them")
    print(f"sure edges: {sum(len(v) for v in sure.values())} (score.py has their precision and recall)")


main()
