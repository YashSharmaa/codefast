"""What a bug report names exactly, on development tasks only.

For each report: the file paths it names that exist in the repository, and the identifiers it
names that resolve to one definition. Then how often those point at a file the real fix changed.
The task file comes from make_i_tasks.py, and CODEFAST_BIN names the binary.
usage: exact.py
"""
import collections
import importlib.util
import json
import os
import re
import subprocess
from pathlib import Path
import sys

R = str(Path(__file__).resolve().parents[2])
BIN = os.environ.get("CODEFAST_BIN", f"{R}/target/release/codefast")
held = set(json.load(open(f"{R}/bench/tier2/tasks.json"))["held_out"]["I"])
tasks = [t for t in json.load(open(f"{R}/target/ab/i/tasks.json")) if t["id"] not in held]

spec = importlib.util.spec_from_file_location("live", f"{R}/bench/tier2/live.py")
live = importlib.util.module_from_spec(spec)
argv, sys.argv = sys.argv, [sys.argv[0]]
spec.loader.exec_module(live)
sys.argv = argv

TOK = re.compile(r"[A-Za-z_][A-Za-z0-9_]{2,}")
HEAD = re.compile(r"^\S+\.\w+:\d+")
PATH = re.compile(r"[\w./-]+\.(?:py|go|rs|c|h|cc|cpp|ts|tsx|js|java)\b")
c = collections.Counter()
rows = []
for t in tasks:
    env = dict(os.environ, CODEFAST_INDEX_DIR=f"{R}/target/bench-index/ck-{t['id']}", CODEFAST_THREADS="4")
    if not os.path.isdir(env["CODEFAST_INDEX_DIR"]):
        subprocess.run([BIN, "--root", t["base"], "index"], capture_output=True, env=env)
    text = t["issue"]
    gold = set(t["gold_files"])
    goldfn = {g.rsplit(":", 1)[-1] for g in (t.get("gold_functions") or [])}

    # Paths the report names that exist in the tree.
    named = []
    for p in dict.fromkeys(PATH.findall(text)):
        p = p.lstrip("./")
        if os.path.isfile(os.path.join(t["base"], p)):
            named.append(p)
    # Identifiers the report names that the index resolves to exactly one definition.
    uniq = []
    toks = [k for k in dict.fromkeys(TOK.findall(text)) if not (k.islower() and "_" not in k and len(k) < 8)]
    for tok in toks:
        p = subprocess.run([BIN, "--root", t["base"], "--budget", "0", "search", tok],
                           capture_output=True, text=True, env=env)
        # `path:lines name` heads each definition; indented lines are its code.
        hits = [l for l in p.stdout.splitlines() if HEAD.match(l) and l.rstrip().endswith(tok)]
        if len(hits) == 1:
            uniq.append((tok, hits[0].split(":")[0]))

    c["reports"] += 1
    c["names an existing path"] += bool(named)
    c["that path is a gold file"] += any(p in gold for p in named)
    c["first named path is gold"] += bool(named) and named[0] in gold
    c["names a unique symbol"] += bool(uniq)
    c["a unique symbol is in a gold file"] += any(f in gold for _, f in uniq)
    c["a unique symbol IS a gold function"] += any(n in goldfn for n, _ in uniq)
    c["first unique symbol is in a gold file"] += bool(uniq) and uniq[0][1] in gold
    rows.append((t["id"], len(named), len(uniq), sorted(gold)[:1], named[:2], uniq[:2]))

print(f"{len(tasks)} development reports")
for k, v in c.items():
    print(f"  {k:38s} {v}")
print()
for r in rows:
    print(r)
