"""Selectors for "which symbol named in a bug report should a card carry", development tasks only.

Every selector picks at most one identifier of the report that the index resolves to exactly one
definition. Scored on whether that definition's file is a file the real fix changed, and on
whether the definition is the gold function itself.
The task file comes from make_i_tasks.py, and CODEFAST_BIN names the binary.
usage: sel.py
"""
import collections
import json
import os
import re
import subprocess
from pathlib import Path

R = str(Path(__file__).resolve().parents[2])
BIN = os.environ.get("CODEFAST_BIN", f"{R}/target/release/codefast")
held = set(json.load(open(f"{R}/bench/tier2/tasks.json"))["held_out"]["I"])
tasks = [t for t in json.load(open(f"{R}/target/ab/i/tasks.json")) if t["id"] not in held]

TOK = re.compile(r"[A-Za-z_][A-Za-z0-9_]{2,}")
HEAD = re.compile(r"^\S+\.\w+:\d+")
QUOTED = re.compile(r"[`'\"]([A-Za-z_][A-Za-z0-9_]{2,})[`'\"]")
CALLED = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]{2,})\s*\(")
DOTTED = re.compile(r"\b[A-Za-z_][A-Za-z0-9_]*\.([A-Za-z_][A-Za-z0-9_]{2,})\b")

score = collections.Counter()
fires = collections.Counter()
detail = []
for t in tasks:
    env = dict(os.environ, CODEFAST_INDEX_DIR=f"{R}/target/bench-index/ck-{t['id']}", CODEFAST_THREADS="4")
    text = t["issue"]
    gold = set(t["gold_files"])
    goldfn = {g.rsplit(":", 1)[-1] for g in (t.get("gold_functions") or [])}
    title = text.splitlines()[0] if text.splitlines() else ""

    order = list(dict.fromkeys(TOK.findall(text)))
    counts = collections.Counter(TOK.findall(text))
    quoted = list(dict.fromkeys(QUOTED.findall(text)))
    called = list(dict.fromkeys(CALLED.findall(text)))
    dotted = list(dict.fromkeys(DOTTED.findall(text)))
    in_title = list(dict.fromkeys(TOK.findall(title)))

    # Resolve every candidate once.
    uniq = {}
    for tok in order:
        if tok.islower() and "_" not in tok and len(tok) < 8:
            continue
        p = subprocess.run([BIN, "--root", t["base"], "--budget", "0", "search", tok],
                           capture_output=True, text=True, env=env)
        hits = [l for l in p.stdout.splitlines() if HEAD.match(l) and l.rstrip().endswith(tok)]
        if len(hits) == 1:
            uniq[tok] = hits[0].split(":")[0]

    def first(cands):
        for k in cands:
            if k in uniq:
                return k
        return None

    SEL = {
        "first unique, text order": first(order),
        "first unique in the title": first(in_title),
        "first unique that is quoted": first(quoted),
        "first unique that is called like f(": first(called),
        "first unique after a dot (x.f)": first(dotted),
        "most mentioned unique": first(sorted(uniq, key=lambda k: -counts[k])),
        "quoted, else called, else text order": first(quoted) or first(called) or first(order),
        "called, else quoted, else text order": first(called) or first(quoted) or first(order),
    }
    for name, k in SEL.items():
        if k is None:
            continue
        fires[name] += 1
        score[(name, "file")] += uniq[k] in gold
        score[(name, "fn")] += k in goldfn
    detail.append((t["id"], {n: k for n, k in SEL.items() if k}))

print(f"{len(tasks)} development reports, the ranker's own figures are file@1 0.36 and file@3 0.79\n")
print(f"{'selector':42s} {'fires':>5s} {'file right':>11s} {'is the gold fn':>15s}")
for name in SEL:
    f = fires[name]
    if not f:
        print(f"{name:42s} {0:5d}")
        continue
    print(f"{name:42s} {f:5d} {score[(name,'file')]:4d} = {score[(name,'file')]/f:.2f} {score[(name,'fn')]:7d} = {score[(name,'fn')]/f:.2f}")
print()
for d in detail:
    print(d[0], {k[:18]: v for k, v in d[1].items()})
