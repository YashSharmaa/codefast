#!/usr/bin/env python3
"""Second derivation of the C oracle's direct-call edges, from clang's own
call-graph builder instead of the libclang cursor walk in c/extract.py.

    c_crosscheck.py <oracle_dir> <build_root> [--jobs 4] [--cache FILE] [--show N]

For every translation unit of compile_commands.json this runs
`clang --analyze -Xclang -analyzer-checker=debug.DumpCallGraph` (shallow mode,
so no path-sensitive analysis worth mentioning) with the unit's own flags and
parses "Function: f calls: a b c". clang::CallGraph records one entry per
CallExpr with a direct callee, macro expansions included, and knows nothing
about files, links or macros-vs-text, which is what extract.py adds.

Compared, for callers defined in the unit's main .c file:
  * the multiset of (caller, callee name) call sites per unit  (site-level)
and for all callers:
  * the set of (caller name, callee name) pairs                 (name-level)
"""
import argparse, collections, json, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

ap = argparse.ArgumentParser()
ap.add_argument("oracle"); ap.add_argument("build_root")
ap.add_argument("--jobs", type=int, default=4); ap.add_argument("--cache"); ap.add_argument("--show", type=int, default=12)
a = ap.parse_args()
root = os.path.realpath(a.build_root)
cmds = json.load(open(os.path.join(root, "compile_commands.json")))
sdk = subprocess.check_output(["xcrun", "--show-sdk-path"], text=True).strip()
DROP_ARG, DROP = {"-o", "-MF", "-MT", "-MQ"}, {"-c", "-MMD", "-MD", "-MP", "-MM", "-flto"}

def run(c):
    out, skip = [], False
    for x in c["arguments"][1:]:
        if skip: skip = False; continue
        if x in DROP_ARG: skip = True; continue
        if x in DROP: continue
        out.append(x)
    cmd = [c["arguments"][0]] + out + ["-isysroot", sdk, "--analyze", "-o", "/dev/null", "-w",
           "-Xclang", "-analyzer-checker=debug.DumpCallGraph",
           "-Xclang", "-analyzer-config", "-Xclang", "mode=shallow",
           "-Xclang", "-analyzer-config", "-Xclang", "max-nodes=1"]
    r = subprocess.run(cmd, cwd=c["directory"], capture_output=True, text=True, errors="replace")
    g = {}
    for line in r.stderr.splitlines():
        if line.startswith("  Function: ") and " calls: " in line + " ":
            name, _, rest = line[len("  Function: "):].partition(" calls:")
            if name.strip() != "< root >":
                g[name.strip()] = rest.split()
    return os.path.relpath(c["file"], root), g, r.returncode

if a.cache and os.path.exists(a.cache):
    graphs = json.load(open(a.cache))
else:
    with ThreadPoolExecutor(a.jobs) as ex:
        res = list(ex.map(run, cmds))
    bad = [f for f, g, rc in res if rc != 0 or not g]
    if bad: print("units with no call graph:", bad, file=sys.stderr)
    graphs = [[f, g] for f, g, rc in res]
    if a.cache: json.dump(graphs, open(a.cache, "w"))

jl = lambda n: [json.loads(l) for l in open(os.path.join(a.oracle, n)) if l.strip()]
E, F = jl("edges.jsonl"), jl("functions.jsonl")
meta = json.load(open(os.path.join(a.oracle, "meta.json")))
defined = collections.defaultdict(set)
for f in F: defined[f["name"]].add(f["file"])
in_scope_tu = lambda f: f in set(meta["analysed_files"])

# ---- site-level, callers defined in the unit's main file ----------------------
orc_sites = collections.Counter()      # (tu file, caller, callee name) -> distinct sites
seen = set()
for e in E:
    k = (e["caller"]["file"], e["caller"]["name"], e["callee"]["name"], e.get("site_file", ""), e["site_line"], e["site_col"])
    if k in seen: continue           # link variants: one site, two definitions
    seen.add(k)
    orc_sites[k[:3]] += 1
main_callers = collections.defaultdict(set)
for f in F: main_callers[f["file"]].add(f["name"])
ana_sites = collections.Counter()
ana_pairs, n_units, n_calls = set(), 0, 0
for tu, g in graphs:
    if not in_scope_tu(tu): continue
    n_units += 1
    for caller, callees in g.items():
        for cal in callees:
            if cal not in defined: continue       # libc, builtins, declared-but-not-built
            n_calls += 1
            ana_pairs.add((caller, cal))
            if caller in main_callers.get(tu, ()): ana_sites[(tu, caller, cal)] += 1
tus = {tu for tu, _ in graphs}
more_ana = {k: (ana_sites[k], orc_sites.get(k, 0)) for k in ana_sites if ana_sites[k] > orc_sites.get(k, 0)}
more_orc = {k: (ana_sites.get(k, 0), orc_sites[k]) for k in orc_sites if k[0] in tus and orc_sites[k] > ana_sites.get(k, 0)}
tot_ana = sum(ana_sites.values()); tot_orc = sum(v for k, v in orc_sites.items() if k[0] in tus)
print(f"units: {n_units}; analyzer call sites to repo-defined names: {n_calls}")
print(f"site-level (callers in the unit's .c file): analyzer {tot_ana} sites, oracle {tot_orc} sites")
print(f"  (unit, caller, callee) keys where the analyzer has MORE sites than the oracle: {len(more_ana)} (missing from oracle: {sum(x - y for x, y in more_ana.values())} sites)")
for k in sorted(more_ana)[:a.show]: print("     ", k, "analyzer/oracle", more_ana[k])
print(f"  keys where the oracle has MORE sites than the analyzer: {len(more_orc)} (extra in oracle: {sum(y - x for x, y in more_orc.values())} sites)")
for k in sorted(more_orc)[:a.show]: print("     ", k, "analyzer/oracle", more_orc[k])
# ---- name-level, all callers ---------------------------------------------------
orc_pairs = {(e["caller"]["name"], e["callee"]["name"]) for e in E}
callers_known = set(defined)
ana_pairs = {p for p in ana_pairs if p[0] in callers_known}
print(f"name-level pairs: analyzer {len(ana_pairs)}, oracle {len(orc_pairs)}, analyzer-only {len(ana_pairs - orc_pairs)}, oracle-only {len(orc_pairs - ana_pairs)}")
for p in sorted(ana_pairs - orc_pairs)[:a.show]: print("      analyzer-only", p, sorted(defined[p[1]])[:3])
for p in sorted(orc_pairs - ana_pairs)[:a.show]: print("      oracle-only", p)
