#!/usr/bin/env python3
"""Cross-check a codefast Go oracle against the output of repowise-bench's
G4 Go oracle (graph/experiments/g4-oracle-anchored/oracle) run on the same
checkout. Usage: compare_with_reference.py <oracle_dir> <reference.jsonl>"""
import collections
import json
import sys

odir, refpath = sys.argv[1], sys.argv[2]
ref, hdr, reach = set(), {}, set()
for line in open(refpath):
    o = json.loads(line)
    if o.get("_header"):
        hdr = o
    elif o.get("_reachable"):
        reach = {tuple(x) for x in o["funcs"]}
    elif o["caller_decl_file"] and o["callee_file"]:
        ref.add((o["caller_decl_file"], o["caller_decl_line"], o["callee_file"], o["callee_line"]))
analysed = set(hdr["analysed_files"])
ref = {k for k in ref if k[0] in analysed and k[2] in analysed}
edges = [json.loads(l) for l in open(odir + "/edges.jsonl")]
key = lambda e: (e["caller"]["file"], e["caller"]["line"], e["callee"]["file"], e["callee"]["line"])
mine_all = {key(e) for e in edges}
mine_rta = {key(e) for e in edges if e["rta_reachable"]}
by = {}
for e in edges:
    by.setdefault(key(e), e)
print(f"reference edges (in scope): {len(ref)}  reference judged functions: {len(reach)}")
print(f"ours: {len(mine_all)} function-level edges, {len(mine_rta)} with an RTA-reachable caller")
print(f"reference edges missing from ours: {len(ref - mine_all)}")
for k in sorted(ref - mine_all)[:20]:
    print("   ", k)
extra = mine_rta - ref
print(f"ours (RTA-reachable) not in reference: {len(extra)}")
print("   by kind:", dict(collections.Counter(by[k]["kind"] for k in extra)))
funcs = [json.loads(l) for l in open(odir + "/functions.jsonl")]
mine_reach = {(f["file"], f["line"]) for f in funcs if f["rta_reachable"]}
print(f"RTA-reachable functions: ours {len(mine_reach)}, reference {len(reach)}, "
      f"symmetric difference {len(mine_reach ^ reach)}")
