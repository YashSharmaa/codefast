#!/usr/bin/env python3
"""Independent verifier's seeded sample (seed 4051977 unless overridden).

    sample.py <oracle_dir> <repo_checkout> [--seed N] [--edges 40] [--functions 15] [--part edges|functions]

Part 1: N edges drawn uniformly at random from edges.jsonl, each shown with
the caller declaration, two lines of context and the call-site line (caret
under site_col), and the callee declaration. Macro rows also print the
`#define` of the macro written at the site.
Part 2: M functions, stratified by size (a third each of 3-10, 11-40 and
41-150 lines; top-level functions only), printed in full. Beside each line:
the oracle's edges and unresolved sites for that line, and `?name` for every
`name(` token whose name is defined somewhere in the analysed files but has no
oracle row on that line (a pointer for the human reader, not a verdict).
"""
import argparse, collections, json, os, random, re, subprocess

ap = argparse.ArgumentParser()
ap.add_argument("oracle"); ap.add_argument("repo")
ap.add_argument("--seed", type=int, default=4051977)
ap.add_argument("--edges", type=int, default=40)
ap.add_argument("--functions", type=int, default=15)
ap.add_argument("--variant", default="")
ap.add_argument("--part", default="both")
a = ap.parse_args()
jl = lambda n: [json.loads(l) for l in open(os.path.join(a.oracle, n)) if l.strip()]
edges, funcs, unres = jl(f"edges{a.variant}.jsonl"), jl(f"functions{a.variant}.jsonl"), jl(f"unresolved_sites{a.variant}.jsonl")
_src = {}
def lines(f):
    if f not in _src:
        _src[f] = open(os.path.join(a.repo, f), "rb").read().decode("utf-8", "replace").split("\n")
    return _src[f]
def show(f, n):
    ls = lines(f)
    return ls[n - 1].rstrip().replace("\t", " ") if 0 < n <= len(ls) else "<out of range>"

rng = random.Random(a.seed)
pick_e = rng.sample(edges, min(a.edges, len(edges)))
rng2 = random.Random(a.seed + 1)
pool = [f for f in funcs if not f.get("nested_in") and f.get("kind") not in ("class", "variable")]
strata = [(3, 10), (11, 40), (41, 150)]
pick_f = []
for i, (lo, hi) in enumerate(strata):
    p = [f for f in pool if lo <= f["end_line"] - f["line"] + 1 <= hi]
    k = a.functions // 3 + (1 if i < a.functions % 3 else 0)
    pick_f += rng2.sample(p, min(k, len(p)))

if a.part in ("both", "edges"):
    print(f"# PART 1: {len(pick_e)} of {len(edges)} edge rows, seed {a.seed}")
    for i, e in enumerate(pick_e, 1):
        c, t = e["caller"], e["callee"]
        extra = {k: v for k, v in e.items() if k not in ("caller", "callee", "site_line", "site_col", "kind", "rta_reachable")}
        print(f"\n[E{i}] {e['kind']} {json.dumps(extra) if extra else ''}")
        print(f"  caller {c['file']}:{c['line']} {c.get('receiver', '')} {c['name']}")
        print(f"      | {show(c['file'], c['line'])[:160]}")
        sf = e.get("site_file", c["file"])
        print(f"  site   {sf}:{e['site_line']}:{e['site_col']}")
        for n in (e["site_line"] - 2, e["site_line"] - 1):
            print(f"      . {show(sf, n)[:200]}")
        raw = lines(sf)[e["site_line"] - 1] if 0 < e["site_line"] <= len(lines(sf)) else ""
        pad = len(raw.encode()[:e["site_col"] - 1].decode("utf-8", "replace"))
        print(f"      | {raw.replace(chr(9), ' ').rstrip()[:240]}")
        print(f"      | {' ' * pad}^")
        print(f"      . {show(sf, e['site_line'] + 1)[:200]}")
        print(f"  callee {t['file']}:{t['line']} {t.get('receiver', '')} {t['name']}")
        print(f"      | {show(t['file'], t['line'])[:160]}")
        if e["kind"] == "macro":
            try:
                out = subprocess.run(["grep", "-rnE", "--include=*.h", "--include=*.c", rf"#\s*define\s+{re.escape(e['via_macro'])}\b", a.repo],
                                     capture_output=True, text=True).stdout.splitlines()[:3]
                for o in out:
                    fn, ln, _ = o.replace(a.repo.rstrip("/") + "/", "").split(":", 2)
                    print(f"  macro  {fn}:{ln}")
                    for n in range(int(ln), int(ln) + 4):
                        s = show(fn, n); print(f"      | {s[:200]}")
                        if not s.endswith("\\"): break
            except Exception as ex:
                print("  macro lookup failed", ex)

if a.part in ("both", "functions"):
    by_caller = collections.defaultdict(list)
    for e in edges: by_caller[(e["caller"]["file"], e["caller"]["line"], e["caller"]["name"])].append(e)
    un_by = collections.defaultdict(list)
    for u in unres: un_by[(u["caller"]["file"], u["caller"]["line"], u["caller"]["name"])].append(u)
    defined = collections.defaultdict(list)
    for f in funcs: defined[f["name"]].append(f)
    CALL = re.compile(r"([A-Za-z_$][\w$]*)\s*(?:<[^<>()]*>)?\s*\(")
    print(f"\n\n# PART 2: {len(pick_f)} functions (strata {strata}), seed {a.seed + 1}")
    for i, f in enumerate(pick_f, 1):
        key = (f["file"], f["line"], f["name"])
        es = by_caller.get(key, [])
        by_line, un_line = collections.defaultdict(list), collections.defaultdict(list)
        for e in es: by_line[e["site_line"]].append(e)
        for u in un_by.get(key, []): un_line[u["site_line"]].append(u)
        print(f"\n=== [F{i}] {f['file']}:{f['line']}-{f['end_line']} {f.get('receiver_or_class', '')} {f['name']} judged={f.get('judged')} edges={len(es)} unresolved={len(un_by.get(key, []))}")
        for n in range(f.get("start_line", f["line"]), f["end_line"] + 1):
            text = show(f["file"], n)
            notes = []
            for e in sorted(by_line.get(n, []), key=lambda e: e["site_col"]):
                t = e["callee"]
                tag = e["kind"][:3] + (f"/{e['candidates']}" if e.get("candidates") else "") + ("/closure" if e.get("callee_closure") else "") + (f"/{e['via_macro']}" if e.get("via_macro") else "")
                notes.append(f"{t.get('receiver', '') + '.' if t.get('receiver') else ''}{t['name']}@{os.path.basename(t['file'])}:{t['line']}[{tag}]")
            for u in un_line.get(n, []): notes.append(f"UNRES[{u['kind']}:{u.get('callee_expr', '')}]")
            have = {e["callee"]["name"] for e in by_line.get(n, [])} | {e.get("via_macro") for e in by_line.get(n, [])}
            for m in CALL.finditer(text.split("//")[0]):
                nm = m.group(1)
                if nm in defined and nm not in have and not (n == f["line"] and nm == f["name"]): notes.append(f"?{nm}")
            print(f"{n:5d} | {text[:150]:<50}" + (f"   <-- {'; '.join(notes)}" if notes else ""))
        off = [e for e in es if "site_file" in e]
        if off: print("  rows with site in another file:", len(off))
