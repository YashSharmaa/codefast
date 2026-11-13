#!/usr/bin/env python3
"""Score a tool's call edges against a call-graph oracle (bench/oracles/FORMAT.md).

    score.py --oracle bench/oracles/gitleaks --tool tool_edges.jsonl [--out score.json]

Tool input: JSON Lines, one call edge per line:

    {"caller": {"file": "cmd/root.go", "line": 132, "name": "initConfig"},
     "callee": {"file": "config/config.go", "line": 88, "name": "Translate"}}

`file` is repository-relative. `line` (any line inside the function, normally
its declaration line) and `name` are each optional, but one of them is needed.
Nothing here is specific to a tool: the rules are the ones in FORMAT.md
section "Scoring", implemented once.
"""
import argparse
import collections
import json
import math
import os
import sys


def wilson(k, n, z=1.959964):
    if n == 0:
        return None
    p = k / n
    d = 1 + z * z / n
    c = (p + z * z / (2 * n)) / d
    h = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
    return [round(max(0.0, c - h), 4), round(min(1.0, c + h), 4)]


def load_jsonl(path):
    """Rows of a JSON Lines file. The larger oracle files are stored gzip-compressed, as
    `<name>.jsonl.gz` next to where `<name>.jsonl` would be."""
    if not os.path.exists(path) and os.path.exists(path + ".gz"):
        import gzip
        with gzip.open(path + ".gz", "rt") as fh:
            return [json.loads(l) for l in fh if l.strip()]
    with open(path) as fh:
        return [json.loads(l) for l in fh if l.strip()]


def bare(name):
    """Last component of a possibly qualified name: `(*T).M`, `T.M`, `T::M`, `pkg.F` -> M / F."""
    for sep in ("::", "."):
        if sep in name:
            name = name.rsplit(sep, 1)[1]
    return name.strip("()* ")


class Oracle:
    def __init__(self, odir, suffix="", extra_edge_files=()):
        self.meta = json.load(open(os.path.join(odir, f"meta{suffix}.json")))
        self.analysed = set(self.meta["analysed_files"])
        self.functions = load_jsonl(os.path.join(odir, f"functions{suffix}.jsonl"))
        self.edges = load_jsonl(os.path.join(odir, f"edges{suffix}.jsonl"))
        for f in extra_edge_files:
            self.edges += load_jsonl(f)
        self.by_file = collections.defaultdict(list)
        self.by_id = {}
        for f in self.functions:
            fid = (f["file"], f["line"], f["name"])
            f["_id"] = fid
            self.by_id[fid] = f
            self.by_file[f["file"]].append(f)
        for fs in self.by_file.values():
            fs.sort(key=lambda f: (f.get("start_line", f["line"]), -f["end_line"]))

    def outermost(self, f):
        """Caller key: the outermost named function a function is written in."""
        n = f.get("nested_in")
        if n:
            return self.by_id.get((f["file"], n["line"], n["name"]), f)
        return f

    def resolve(self, ep, role):
        """Map a tool endpoint to a set of oracle record ids (FORMAT.md, Scoring step 2).

        Empty set = unmapped. More than one id = ambiguous. The rules are
        position first: a line, when given, decides; a name only narrows.
        There is no fallback from a line that maps nowhere to a file-wide
        name search.
        """
        fs = self.by_file.get(ep.get("file"), ())
        name = bare(ep["name"]) if ep.get("name") else None
        line = ep.get("line")
        start = lambda f: f.get("start_line", f["line"])

        def innermost(cands):
            # latest start, then earliest end; exact ties stay ambiguous
            if not cands:
                return set()
            best = max((start(f), -f["end_line"]) for f in cands)
            return {f["_id"] for f in cands if (start(f), -f["end_line"]) == best}

        if line is None:
            named = [f for f in fs if f["name"] == name] if name else []
            if role == "caller":
                named = [f for f in named if f.get("kind") != "class"]
            recv = ep.get("receiver")
            if recv and len(named) > 1:
                narrowed = [f for f in named if f.get("receiver_or_class", "").lstrip("*") == recv.lstrip("*")]
                named = narrowed or named
            return {f["_id"] for f in named}
        inside = [f for f in fs if start(f) <= line <= f["end_line"]]
        if role == "caller":
            # a class is not a caller: code in a class body outside any method
            # belongs to no function
            inside = [f for f in inside if f.get("kind") != "class"]
            return innermost([f for f in inside if f["name"] == name] if name else []) or innermost(inside)
        header = [f for f in inside if line <= max(f["line"], f.get("name_line", f["line"]))]
        if header:
            return {f["_id"] for f in ([f for f in header if f["name"] == name] or header)} if len(header) > 1 else {header[0]["_id"]}
        # a line inside a body: only a name can say which containing function is meant
        return innermost([f for f in inside if f["name"] == name]) if name else set()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--oracle", required=True, help="oracle directory, e.g. bench/oracles/gitleaks")
    ap.add_argument("--tool", required=True, help="tool edges, JSON Lines")
    ap.add_argument("--variant", default="", help="oracle file suffix, e.g. .with_tests")
    ap.add_argument("--scope-prefix", action="append", default=[],
                    help="restrict both sides to files under this prefix (repeatable)")
    ap.add_argument("--include-variable-endpoints", action="store_true",
                    help="TypeScript: also score edges whose caller or callee is a module-level variable "
                         "(initialiser code, and calls of factory-produced constructors)")
    ap.add_argument("--with-funcptr-candidates", action="store_true",
                    help="C: load edges.funcptr_candidates.jsonl (heuristic) into the candidate tier")
    ap.add_argument("--repowise-compat", action="store_true",
                    help="reproduce the repowise-bench G4 population: Go edges restricted to RTA-reachable "
                         "callers, closure callees kept in the denominator, one pooled recall")
    ap.add_argument("--out", help="write the score as JSON")
    ap.add_argument("--dump", help="directory for missed.jsonl / contradicted.jsonl / unmapped.jsonl")
    args = ap.parse_args()

    extra = []
    if args.with_funcptr_candidates:
        extra.append(os.path.join(args.oracle, "edges.funcptr_candidates.jsonl"))
    o = Oracle(args.oracle, args.variant, extra)
    in_scope = lambda p: p in o.analysed and (not args.scope_prefix or any(p.startswith(x) for x in args.scope_prefix))

    # ---- oracle tiers -------------------------------------------------------
    def ep_id(ep):
        return (ep["file"], ep["line"], ep["name"])

    must, cand = {}, {}             # (caller_id, callee_id) -> example edge
    alt_of = {}                     # (caller_id, alt_callee_id) -> canonical pair
    sites = collections.defaultdict(set)  # dynamic site -> {pair}
    skipped = collections.Counter()
    withheld_compat = set()
    for e in o.edges:
        if not (in_scope(e["caller"]["file"]) and in_scope(e["callee"]["file"])):
            skipped["out_of_scope"] += 1
            continue
        if (e.get("caller_kind") == "variable" or e.get("callee_kind") == "variable") and not args.include_variable_endpoints:
            skipped["variable_endpoint"] += 1
            continue
        if args.repowise_compat and "rta_reachable" in e and not e["rta_reachable"]:
            skipped["not_rta_reachable"] += 1
            continue
        if e.get("callee_closure") and not args.repowise_compat:
            skipped["closure_callee"] += 1
            continue
        if args.repowise_compat and e.get("resolution"):
            # class-hierarchy fold, const-initializer: the reference oracle has
            # no such edge and withholds the caller instead.
            skipped["beyond_reference_method"] += 1
            withheld_compat.add(ep_id(e["caller"]))
            continue
        pair = (ep_id(e["caller"]), ep_id(e["callee"]))
        # candidate tier: every row that carries a `candidates` field (dynamic
        # dispatch, C link variants, Go generic instantiations)
        dynamic = e["kind"] in ("interface", "funcptr") or "candidates" in e
        if dynamic and not args.repowise_compat:
            cand.setdefault(pair, e)
            sites[(pair[0], e.get("site_file", ""), e["site_line"], e["site_col"])].add(pair)
        else:
            must.setdefault(pair, e)
        for a in e.get("callee_alt", ()):
            alt_of[(pair[0], ep_id(a))] = pair
    for pair in list(cand):
        if pair in must:
            del cand[pair]

    judged = {f["_id"] for f in o.functions if f.get("judged", True)}
    if args.repowise_compat:
        judged = {f["_id"] for f in o.functions if f.get("judged", True) and f.get("rta_reachable", True)} - withheld_compat

    # ---- tool edges ---------------------------------------------------------
    tool_rows = load_jsonl(args.tool)
    buckets = collections.Counter()
    tool_pairs = {}     # representative pair -> row
    pair_judged = {}    # representative pair -> every possible caller is judged
    unmapped, ambiguous = [], 0
    unmapped_strict = set()
    is_var = lambda i: o.by_id[i].get("kind") == "variable"
    for row in tool_rows:
        c, t = row["caller"], row["callee"]
        if not (in_scope(c.get("file")) and in_scope(t.get("file"))):
            buckets["out_of_scope"] += 1
            continue
        cids = {o.outermost(o.by_id[i])["_id"] for i in o.resolve(c, "caller")}
        tids = o.resolve(t, "callee")
        if not args.include_variable_endpoints:
            # module-level variables are not functions: drop them from an
            # ambiguous endpoint; an endpoint that is only a variable drops the row
            if (cids and all(is_var(i) for i in cids)) or (tids and all(is_var(i) for i in tids)):
                buckets["variable_endpoint"] += 1
                continue
            cids = {i for i in cids if not is_var(i)}
            tids = {i for i in tids if not is_var(i)}
        if not cids:
            buckets["unmapped_caller"] += 1
            unmapped.append({"why": "caller", **row})
            continue
        all_judged = all(i in judged for i in cids)
        if not tids:
            buckets["unmapped_callee"] += 1
            unmapped.append({"why": "callee", "caller_judged": all_judged, **row})
            if all_judged:
                unmapped_strict.add((min(cids), t.get("file"), t.get("line"), t.get("name")))
            continue
        pairs = sorted({alt_of.get((a, b), (a, b)) for a in cids for b in tids})
        if len(pairs) > 1:
            ambiguous += 1
        # one row stands for exactly one pair: the smallest matching must pair,
        # else the smallest matching candidate pair, else the smallest pair
        rep_pair = next((p for p in pairs if p in must), None) or next((p for p in pairs if p in cand), None) or pairs[0]
        tool_pairs.setdefault(rep_pair, row)
        pair_judged[rep_pair] = pair_judged.get(rep_pair, True) and all_judged

    matched_must = {p for p in tool_pairs if p in must}
    matched_cand = {p for p in tool_pairs if p in cand}
    outside = [p for p in tool_pairs if p not in must and p not in cand]
    contradicted = [p for p in outside if pair_judged[p]]
    unjudged = [p for p in outside if not pair_judged[p]]
    unmapped_callee_judged = len(unmapped_strict)

    n_match = len(matched_must) + len(matched_cand)
    covered_sites = sum(1 for s, ps in sites.items() if ps & tool_pairs.keys())

    def rate(k, n):
        return {"value": round(k / n, 4) if n else None, "k": k, "n": n, "ci95": wilson(k, n)}

    def tier(e):
        return e["kind"] if e["kind"] in ("interface", "funcptr") else e.get("candidate_reason", "candidate")

    cand_by_kind = {}
    for kind in sorted({tier(e) for e in cand.values()}):
        ps = {p for p, e in cand.items() if tier(e) == kind}
        cand_by_kind[kind] = rate(len(ps & matched_cand), len(ps))

    score = {
        "oracle": {k: o.meta.get(k) for k in ("repo", "commit", "language", "oracle", "tests_included")},
        "options": {"variant": args.variant, "scope_prefix": args.scope_prefix,
                    "include_variable_endpoints": args.include_variable_endpoints,
                    "with_funcptr_candidates": args.with_funcptr_candidates,
                    "repowise_compat": args.repowise_compat},
        "oracle_edges": {"must": len(must), "candidate": len(cand), "dynamic_sites": len(sites),
                         "excluded": dict(skipped)},
        "tool_edges": {"rows": len(tool_rows), "distinct_function_level": len(tool_pairs),
                       "ambiguous_endpoint_rows": ambiguous, **dict(buckets)},
        "matched_must": len(matched_must), "matched_candidate": len(matched_cand),
        "contradicted": len(contradicted), "unjudged": len(unjudged),
        "unmapped_callee_distinct_with_judged_caller": unmapped_callee_judged,
        "precision": rate(n_match, n_match + len(contradicted)),
        "precision_strict": rate(n_match, n_match + len(contradicted) + unmapped_callee_judged),
        "recall_static": rate(len(matched_must), len(must)),
        "recall_candidate_edges": rate(len(matched_cand), len(cand)),
        "recall_candidate_edges_by_kind": cand_by_kind,
        "recall_dynamic_sites": rate(covered_sites, len(sites)),
        "recall_all_edges": rate(n_match, len(must) + len(cand)),
        "unjudged_share": round(len(unjudged) / len(tool_pairs), 4) if tool_pairs else None,
    }
    print(json.dumps(score, indent=1))
    if args.out:
        with open(args.out, "w") as fh:
            json.dump(score, fh, indent=1)
            fh.write("\n")
    if args.dump:
        os.makedirs(args.dump, exist_ok=True)

        def dump(name, rows):
            with open(os.path.join(args.dump, name), "w") as fh:
                for r in rows:
                    fh.write(json.dumps(r) + "\n")

        dump("missed_static.jsonl", [must[p] for p in sorted(must) if p not in matched_must])
        dump("missed_candidate.jsonl", [cand[p] for p in sorted(cand) if p not in matched_cand])
        dump("contradicted.jsonl", [tool_pairs[p] for p in contradicted])
        dump("unjudged.jsonl", [tool_pairs[p] for p in unjudged])
        dump("unmapped.jsonl", unmapped)
    return 0


if __name__ == "__main__":
    sys.exit(main())
