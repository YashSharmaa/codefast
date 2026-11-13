#!/usr/bin/env python3
"""Mechanical invariants over 100% of an oracle's rows (independent verifier).

    invariants.py <oracle_dir> <repo_checkout> [--variant .with_tests] [--show N]

Checks that need no judgement, on every row:
  I1  every caller / non-closure callee is a record of functions.jsonl
  I2  the call site lies inside the caller's span (same file unless site_file)
  I3  the token written at (site_line, site_col) is the callee's name
      (static rows; macro rows must NOT have it and must have via_macro there)
  I4  the callee's declaration header contains its name
  I5  no duplicate rows; (file, line, name) unique in functions.jsonl
  I6  both files are in meta.analysed_files
  I7  `candidates` equals the number of distinct targets sharing the site
  I8  judged == (no unresolved site charged to the function or its outermost)
  I9  every unresolved site lies inside its caller's span
Exit status 1 if any invariant has violations.
"""
import argparse, collections, json, os, re, sys

ap = argparse.ArgumentParser()
ap.add_argument("oracle"); ap.add_argument("repo")
ap.add_argument("--variant", default=""); ap.add_argument("--show", type=int, default=8)
ap.add_argument("--edges-file", default=None)
a = ap.parse_args()
jl = lambda n: [json.loads(l) for l in open(os.path.join(a.oracle, n)) if l.strip()]
meta = json.load(open(os.path.join(a.oracle, f"meta{a.variant}.json")))
edges = jl(a.edges_file or f"edges{a.variant}.jsonl")
funcs = jl(f"functions{a.variant}.jsonl")
unres = jl(f"unresolved_sites{a.variant}.jsonl")
analysed = set(meta["analysed_files"])
lang = meta["language"]
_src = {}
def lines(f):
    if f not in _src:
        try: _src[f] = open(os.path.join(a.repo, f), "rb").read().decode("utf-8", "replace").split("\n")
        except OSError: _src[f] = []
    return _src[f]
def blines(f):
    k = ("b", f)
    if k not in _src:
        try: _src[k] = open(os.path.join(a.repo, f), "rb").read().split(b"\n")
        except OSError: _src[k] = []
    return _src[k]
IDENT = re.compile(rb"[A-Za-z_$#][\w$]*")
def tok(f, line, col):
    ls = blines(f)
    if not (0 < line <= len(ls)): return None
    m = IDENT.match(ls[line - 1], col - 1)
    return m.group(0).decode() if m else ls[line - 1][col - 1:col + 7].decode("utf-8", "replace")

viol = collections.defaultdict(list)
notes = collections.Counter()
by_id = {}
for f in funcs:
    k = (f["file"], f["line"], f["name"])
    if k in by_id: viol["I5 duplicate function id"].append(k)
    by_id[k] = f
    if f["end_line"] < f["line"]: viol["I5 end_line < line"].append(k)
    if f["file"] not in analysed: viol["I6 function file not analysed"].append(k)
seen = set()
sites = collections.defaultdict(set)
for e in edges:
    c, t = e["caller"], e["callee"]
    ck, tk = (c["file"], c["line"], c["name"]), (t["file"], t["line"], t["name"])
    row = json.dumps(e, sort_keys=True)
    if row in seen: viol["I5 duplicate edge row"].append(e)
    seen.add(row)
    cf = by_id.get(ck)
    if cf is None: viol["I1 caller not in functions.jsonl"].append(e); continue
    if not e.get("callee_closure") and tk not in by_id: viol["I1 callee not in functions.jsonl"].append(e)
    if c["file"] not in analysed or t["file"] not in analysed: viol["I6 edge file not analysed"].append(e)
    sf = e.get("site_file", c["file"])
    if "site_file" not in e and not (cf.get("start_line", cf["line"]) <= e["site_line"] <= cf["end_line"]):
        viol["I2 site outside caller span"].append(e)
    w = tok(sf, e["site_line"], e["site_col"])
    name = t["name"]
    if e["kind"] == "macro":
        if w == name: viol["I3 macro row but callee name is written at the site"].append(e)
        if w != e.get("via_macro"): viol["I3 via_macro is not the token at the site"].append(e)
    elif e.get("callee_closure"):
        pass
    elif e.get("implicit"):
        pass
    elif e["kind"] == "static":
        alts = {name} | {x["name"] for x in e.get("callee_alt", ())}
        if e.get("via") == "new" and t.get("receiver"): alts.add(t["receiver"])
        if w not in alts:
            if lang == "typescript":
                # import/export aliases (`export { _null as null }`) and const
                # aliases (`const objectType = ZodObject.create`) are legitimate;
                # they are checked against the symbol table by ts/crosscheck.mjs
                notes["static rows whose site token is an alias of the callee name"] += 1
            else:
                viol["I3 static row: token at site is not the callee name"].append((w, e))
    else:
        # dynamic: the method name is written at the site for interface calls
        if e["kind"] == "interface" and w != name:
            viol["I3 interface row: token at site is not the method name"].append((w, e))
    if not e.get("callee_closure") and tk in by_id:
        tf = by_id[tk]
        ls = lines(t["file"])
        hdr = "\n".join(ls[tf.get("start_line", tf["line"]) - 1: max(tf["line"], tf.get("name_line", tf["line"])) + (2 if lang == "c" else 0)])
        nm = name if name not in ("constructor", "default") else ""
        if nm and nm not in hdr and not tf.get("from_macro"):
            viol["I4 callee name not on its declaration header"].append(e)
    if e["kind"] in ("interface", "funcptr") or e.get("candidates"):
        sites[(ck, sf, e["site_line"], e["site_col"], e["kind"] if lang != "go" else "")].add(tk)
for e in edges:
    if e["kind"] in ("interface", "funcptr") or e.get("candidates"):
        c = e["caller"]; ck = (c["file"], c["line"], c["name"])
        n = len(sites[(ck, e.get("site_file", c["file"]), e["site_line"], e["site_col"], e["kind"] if lang != "go" else "")])
        if e.get("candidates") != n: viol["I7 candidates != distinct targets at the site"].append((n, e))
charged = collections.Counter()
for u in unres:
    c = u["caller"]; ck = (c["file"], c["line"], c["name"])
    f = by_id.get(ck)
    if f is None:
        if c["name"] != "<module>": viol["I9 unresolved site caller unknown"].append(u)
        continue
    charged[ck] += 1
    if "site_file" not in u and not (f.get("start_line", f["line"]) <= u["site_line"] <= f["end_line"]):
        viol["I9 unresolved site outside caller span"].append(u)
for k, f in by_id.items():
    if f.get("kind") in ("class", "variable"): continue
    o = f.get("nested_in")
    ok = (f["file"], o["line"], o["name"]) if o else k
    reasons = f.get("unjudged_reasons")
    expect = charged[ok] == 0 and not reasons
    if f.get("judged") != expect: viol["I8 judged flag inconsistent with unresolved sites"].append((k, f.get("judged"), charged[ok]))

print(f"{meta['repo']}{a.variant}: {len(edges)} edge rows, {len(funcs)} function records, {len(unres)} unresolved sites")
bad = 0
for k in sorted(viol):
    v = viol[k]; bad += len(v)
    print(f"  VIOLATION {k}: {len(v)}")
    for x in v[:a.show]: print("     ", json.dumps(x)[:400])
for k, v in sorted(notes.items()): print(f"  note: {k}: {v}")
if not bad: print("  all invariants hold")
sys.exit(1 if bad else 0)
