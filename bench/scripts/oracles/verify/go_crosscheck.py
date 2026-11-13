#!/usr/bin/env python3
"""Compare the Go oracle's static rows with the AST-derived edges of
go/crosscheck (go/ast + go/types, no SSA).

    go_crosscheck.py <oracle_dir> <ast_edges.jsonl> [--variant .with_tests] [--show N]

Site-level comparison on (caller file, caller line, site line, site col,
callee file, callee line). Reports, with examples:
  * AST static edges missing from the oracle (oracle recall of static edges)
  * oracle static rows the AST walk does not have, split by explanation
  * dynamic call sites the AST sees that the oracle neither resolved nor
    listed as unresolved
"""
import argparse, collections, json, os
ap = argparse.ArgumentParser()
ap.add_argument("oracle"); ap.add_argument("ast"); ap.add_argument("--variant", default=""); ap.add_argument("--show", type=int, default=6)
a = ap.parse_args()
jl = lambda p: [json.loads(l) for l in open(p) if l.strip()]
E = jl(os.path.join(a.oracle, f"edges{a.variant}.jsonl"))
U = jl(os.path.join(a.oracle, f"unresolved_sites{a.variant}.jsonl"))
F = jl(os.path.join(a.oracle, f"functions{a.variant}.jsonl"))
meta = json.load(open(os.path.join(a.oracle, f"meta{a.variant}.json")))
analysed = set(meta["analysed_files"])
A = [r for r in jl(a.ast)]
key = lambda c, sl, sc, t: (c["file"], c["line"], sl, sc, t["file"], t["line"])
ast_static = {}
ast_dyn = {}
init_level = 0
for r in A:
    if r["caller"] is None:
        init_level += 1; continue
    if r["caller"]["file"] not in analysed: continue
    if r.get("dynamic"):
        ast_dyn[(r["caller"]["file"], r["caller"]["line"], r["site_line"], r["site_col"])] = r
    elif r["callee"]["file"] in analysed:
        ast_static[key(r["caller"], r["site_line"], r["site_col"], r["callee"])] = r
orc_static = {key(e["caller"], e["site_line"], e["site_col"], e["callee"]): e for e in E if e["kind"] == "static"}
missing = sorted(set(ast_static) - set(orc_static))
extra = sorted(set(orc_static) - set(ast_static))
print(f"AST static edges (site-level, in scope): {len(ast_static)};  oracle static rows: {len(orc_static)};  AST calls at package-init level (not edges): {init_level}")
print(f"AST static edges missing from the oracle: {len(missing)}")
by = collections.Counter()
for k in missing:
    r = ast_static[k]
    why = "caller generic" if r.get("caller_generic") else ("in func literal" if r.get("in_func_lit") else "plain")
    by[why] += 1
print("   by context:", dict(by))
for k in missing[:a.show]: print("   ", json.dumps(ast_static[k]))
print(f"oracle static rows not in the AST walk: {len(extra)}")
ast_dyn_sites = set(ast_dyn)
cat = collections.defaultdict(list)
for k in extra:
    e = orc_static[k]
    site = (e["caller"]["file"], e["caller"]["line"], e["site_line"], e["site_col"])
    if e.get("callee_closure"): c = "callee is a func literal (SSA resolves a local closure; no types.Func)"
    elif e.get("implicit"): c = "implicit (no call expression)"
    elif site in ast_dyn_sites and ast_dyn[site]["dynamic"] == "funcvalue": c = "AST sees a call of a func value here (method value / local func var bound once)"
    elif site in ast_dyn_sites: c = "AST sees an interface or type-parameter method call here (resolved per instantiation)"
    else: c = "UNEXPLAINED"
    cat[c].append(e)
for c, v in sorted(cat.items(), key=lambda kv: -len(kv[1])):
    print(f"   {len(v):6d}  {c}")
    if "closure" not in c:
        for e in v[:a.show]: print("        ", json.dumps(e)[:330])
# dynamic site coverage
orc_dyn_sites = {(e["caller"]["file"], e["caller"]["line"], e["site_line"], e["site_col"]) for e in E if e["kind"] != "static"}
orc_dyn_sites |= {(u["caller"]["file"], u["caller"]["line"], u["site_line"], u["site_col"]) for u in U}
orc_static_sites = {(e["caller"]["file"], e["caller"]["line"], e["site_line"], e["site_col"]) for e in E if e["kind"] == "static"}
uncovered = sorted(ast_dyn_sites - orc_dyn_sites - orc_static_sites)
print(f"AST dynamic call sites: {len(ast_dyn_sites)}; with oracle edges or listed unresolved: {len(ast_dyn_sites) - len(uncovered)}; with neither (all targets external, or a gap): {len(uncovered)}")
judged = {(f["file"], f["line"]): f for f in F}
kinds = collections.Counter(ast_dyn[k]["dynamic"] for k in uncovered)
print("   by kind:", dict(kinds))
# multi-target static sites
st = collections.defaultdict(set)
for e in E:
    if e["kind"] == "static": st[(e["caller"]["file"], e["caller"]["line"], e["site_line"], e["site_col"])].add((e["callee"]["file"], e["callee"]["line"]))
multi = {k: v for k, v in st.items() if len(v) > 1}
print(f"static sites with more than one target (generic instantiations): {len(multi)} sites, {sum(len(v) for v in multi.values())} rows")
for k in sorted(multi)[:a.show]: print("   ", k, sorted(multi[k]))
