#!/usr/bin/env python3
"""Self-test of score.py: feed each oracle back to the scorer as if it were a
tool. Expected: precision 1.0 and recall 1.0 with full endpoints, and a
predictable drop when a known fraction of edges is removed or corrupted.

    selftest_score.py bench/oracles/gitleaks [more oracle dirs...]
"""
import json
import os
import random
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))


def run(odir, rows, *extra, hashseed=None):
    with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as fh:
        for r in rows:
            fh.write(json.dumps(r) + "\n")
    env = dict(os.environ)
    if hashseed is not None:
        env["PYTHONHASHSEED"] = str(hashseed)
    try:
        out = subprocess.check_output([sys.executable, os.path.join(HERE, "score.py"), "--oracle", odir,
                                       "--tool", fh.name, *extra], text=True, env=env)
    finally:
        os.unlink(fh.name)
    return json.loads(out)


ok = True
for odir in sys.argv[1:]:
    edges = [json.loads(l) for l in open(os.path.join(odir, "edges.jsonl"))]
    funcs = [json.loads(l) for l in open(os.path.join(odir, "functions.jsonl"))]
    scored = [e for e in edges if not e.get("callee_closure") and "variable" not in (e.get("caller_kind"), e.get("callee_kind"))]
    full = [{"caller": e["caller"], "callee": e["callee"]} for e in scored]
    s = run(odir, full)
    exact = (s["precision"]["value"] == 1.0 and s["recall_static"]["value"] == 1.0
             and s["recall_all_edges"]["value"] == 1.0 and s["contradicted"] == 0 and s["unjudged"] == 0)
    print(f"{odir}: identity  P={s['precision']['value']} R_static={s['recall_static']['value']} "
          f"R_all={s['recall_all_edges']['value']} must={s['oracle_edges']['must']} cand={s['oracle_edges']['candidate']}"
          f"  {'OK' if exact else 'FAIL'}")
    ok &= exact

    # Lines only, shifted to the last line of each function: span containment must still map.
    span = {(f["file"], f["line"], f["name"]): f["end_line"] for f in funcs}
    shifted = [{"caller": {"file": e["caller"]["file"], "line": span.get((e["caller"]["file"], e["caller"]["line"], e["caller"]["name"]), e["caller"]["line"])},
                "callee": {"file": e["callee"]["file"], "line": e["callee"]["line"]}} for e in scored]
    s2 = run(odir, shifted)
    good = s2["recall_static"]["value"] is not None and s2["recall_static"]["value"] >= 0.99 and s2["precision"]["value"] >= 0.99
    print(f"{odir}: line-only, caller at end_line  P={s2['precision']['value']} R_static={s2['recall_static']['value']}  {'OK' if good else 'FAIL'}")
    ok &= good

    # Drop half the edges and add wrong ones from judged callers: recall ~0.5, precision < 1.
    rng = random.Random(7)
    pairs = sorted({(json.dumps(r["caller"], sort_keys=True), json.dumps(r["callee"], sort_keys=True)) for r in full})
    keep = [p for p in pairs if rng.random() < 0.5]
    judged = [f for f in funcs if f.get("judged") and f.get("kind") not in ("class", "variable")]
    targets = [f for f in funcs if f.get("kind") != "variable"]
    have = set(pairs)
    wrong = []
    while len(wrong) < 200:
        a, b = rng.choice(judged), rng.choice(targets)
        ca = {"file": a["file"], "name": a["name"], "line": a["line"]}
        cb = {"file": b["file"], "name": b["name"], "line": b["line"]}
        if a.get("nested_in"):
            continue
        if (json.dumps(ca, sort_keys=True), json.dumps(cb, sort_keys=True)) not in have:
            wrong.append({"caller": ca, "callee": cb})
    rows = [{"caller": json.loads(a), "callee": json.loads(b)} for a, b in keep] + wrong
    s3 = run(odir, rows)
    r = s3["recall_all_edges"]["value"]
    good = 0.4 < r < 0.6 and s3["contradicted"] >= 190
    print(f"{odir}: half dropped + 200 wrong  P={s3['precision']['value']} R_all={r} contradicted={s3['contradicted']}  {'OK' if good else 'FAIL'}")
    ok &= good

    # Names only (no lines): endpoints become ambiguous wherever a file has two
    # functions of one name. The score must not depend on set iteration order.
    names = [{"caller": {"file": e["caller"]["file"], "name": e["caller"]["name"]},
              "callee": {"file": e["callee"]["file"], "name": e["callee"]["name"]}} for e in scored]
    a, b = run(odir, names, hashseed=1), run(odir, names, hashseed=4242)
    good = a == b and a["precision"]["value"] is not None and a["precision"]["value"] >= 0.99
    print(f"{odir}: names only, two hash seeds  identical={a == b} P={a['precision']['value']} R_static={a['recall_static']['value']} "
          f"ambiguous_rows={a['tool_edges']['ambiguous_endpoint_rows']}  {'OK' if good else 'FAIL'}")
    ok &= good

    # Position first: a line that lies in no function maps nowhere, even with a
    # name that exists elsewhere in the file; a callee line in the middle of a
    # body with no name maps nowhere.
    top = [f for f in funcs if not f.get("nested_in") and f.get("kind") not in ("class", "variable") and f["end_line"] - f["line"] >= 4]
    covered = {}
    for f in funcs:
        covered.setdefault(f["file"], set()).update(range(f.get("start_line", f["line"]), f["end_line"] + 1))
    probe = []
    for f in top:
        free = next((n for n in range(1, 10 ** 6) if n not in covered[f["file"]]), None)
        probe.append({"caller": {"file": f["file"], "line": free, "name": f["name"]},
                      "callee": {"file": f["file"], "line": f["line"], "name": f["name"]}})
        probe.append({"caller": {"file": f["file"], "line": f["line"]},
                      "callee": {"file": f["file"], "line": f["line"] + 2}})
        if len(probe) >= 100:
            break
    s4 = run(odir, probe)
    te = s4["tool_edges"]
    inner = sum(1 for f in funcs if f.get("nested_in") or f.get("kind") in ("class", "variable"))
    good = te.get("unmapped_caller", 0) == len(probe) // 2 and (te.get("unmapped_callee", 0) == len(probe) // 2 or inner > 0)
    print(f"{odir}: position-first probes  unmapped_caller={te.get('unmapped_caller', 0)}/{len(probe) // 2} "
          f"unmapped_callee={te.get('unmapped_callee', 0)}/{len(probe) // 2}  {'OK' if good else 'FAIL'}")
    ok &= good
sys.exit(0 if ok else 1)
