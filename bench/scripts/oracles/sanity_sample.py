#!/usr/bin/env python3
"""Draw the seeded hand-check sample for one oracle and print it with source.

    sanity_sample.py <oracle_dir> <repo_checkout> [--seed N] [--edges 20] [--functions 10]

Part 1: N random edges, each shown as caller declaration line, call-site line
(with a caret under site_col) and callee declaration line.
Part 2: M random functions (8-45 lines, at least one oracle edge or call-like
token), each printed in full next to every oracle edge and unresolved site the
oracle has for it, plus a text-only cross-check: every `name(` token in the body
whose name is a function defined in the analysed files, and whether the oracle
has an edge to that name on that line. The cross-check knows nothing about
types; it exists to point a human reader at lines worth a second look.
"""
import argparse
import collections
import json
import os
import random
import re

ap = argparse.ArgumentParser()
ap.add_argument("oracle")
ap.add_argument("repo")
ap.add_argument("--seed", type=int, default=20261112)
ap.add_argument("--edges", type=int, default=20)
ap.add_argument("--functions", type=int, default=10)
ap.add_argument("--variant", default="")
a = ap.parse_args()

jl = lambda n: [json.loads(l) for l in open(os.path.join(a.oracle, n))]
edges = jl(f"edges{a.variant}.jsonl")
funcs = jl(f"functions{a.variant}.jsonl")
unres = jl(f"unresolved_sites{a.variant}.jsonl")
rng = random.Random(a.seed)
_src = {}


def lines(f):
    if f not in _src:
        _src[f] = open(os.path.join(a.repo, f), encoding="utf-8", errors="replace").read().split("\n")
    return _src[f]


def show(f, n):
    ls = lines(f)
    return ls[n - 1].rstrip() if 0 < n <= len(ls) else "<out of range>"


print(f"# PART 1: {a.edges} random edges of {len(edges)} (seed {a.seed})")
for i, e in enumerate(rng.sample(edges, min(a.edges, len(edges))), 1):
    c, t = e["caller"], e["callee"]
    extra = {k: v for k, v in e.items() if k not in ("caller", "callee", "site_line", "site_col", "kind")}
    print(f"\n[{i}] kind={e['kind']} {json.dumps(extra) if extra else ''}")
    print(f"  caller {c['file']}:{c['line']} {c.get('receiver', '')} {c['name']}")
    print(f"      | {show(c['file'], c['line'])[:150]}")
    sf = e.get("site_file", c["file"])
    print(f"  site   {sf}:{e['site_line']}:{e['site_col']}")
    s = show(sf, e["site_line"])
    print(f"      | {s[:170]}")
    print(f"      | {' ' * (len(s[:e['site_col'] - 1].expandtabs(8)) if False else e['site_col'] - 1)}^")
    print(f"  callee {t['file']}:{t['line']} {t.get('receiver', '')} {t['name']}")
    print(f"      | {show(t['file'], t['line'])[:150]}")

by_caller = collections.defaultdict(list)
for e in edges:
    by_caller[(e["caller"]["file"], e["caller"]["line"], e["caller"]["name"])].append(e)
un_by_caller = collections.defaultdict(list)
for u in unres:
    un_by_caller[(u["caller"]["file"], u["caller"]["line"], u["caller"]["name"])].append(u)
defined = collections.defaultdict(list)
for f in funcs:
    defined[f["name"]].append(f)

pool = [f for f in funcs if 8 <= f["end_line"] - f["line"] + 1 <= 45 and not f.get("nested_in") and f.get("kind") != "class"]
print(f"\n\n# PART 2: {a.functions} random functions of {len(pool)} eligible (8-45 lines, top level)")
CALL = re.compile(r"([A-Za-z_$][\w$]*)\s*(?:<[^<>()]*>)?\s*\(")
for i, f in enumerate(rng.sample(pool, min(a.functions, len(pool))), 1):
    key = (f["file"], f["line"], f["name"])
    es = sorted(by_caller.get(key, []), key=lambda e: (e["site_line"], e["site_col"]))
    print(f"\n=== [{i}] {f['file']}:{f['line']}-{f['end_line']} {f.get('receiver_or_class', '')} {f['name']} judged={f.get('judged')}")
    by_line = collections.defaultdict(list)
    for e in es:
        by_line[e["site_line"]].append(e)
    un_line = collections.defaultdict(list)
    for u in un_by_caller.get(key, []):
        un_line[u["site_line"]].append(u)
    for n in range(f["line"], f["end_line"] + 1):
        text = show(f["file"], n)
        notes = []
        for e in by_line.get(n, []):
            t = e["callee"]
            tag = e["kind"] + (f"/{e['candidates']}" if e.get("candidates") else "") + ("/closure" if e.get("callee_closure") else "")
            notes.append(f"{t['name']}@{os.path.basename(t['file'])}:{t['line']}[{tag}]")
        for u in un_line.get(n, []):
            notes.append(f"UNRESOLVED[{u['kind']}]")
        have = {e["callee"]["name"] for e in by_line.get(n, [])} | {e.get("via_macro") for e in by_line.get(n, [])}
        code = text.split("//")[0]
        if n > f["line"] or True:
            for m in CALL.finditer(code):
                nm = m.group(1)
                if nm in defined and nm not in have and not (n == f["line"] and nm == f["name"]):
                    notes.append(f"?text:{nm}")
        print(f"{n:5d} | {text[:140]:<60}" + (f"   <-- {'; '.join(notes)}" if notes else ""))
    off = [e for e in es if not (f["line"] <= e["site_line"] <= f["end_line"])]
    if off:
        print("  edges outside the span:", off)
