#!/usr/bin/env python3
"""Re-check bench/k3/questions.jsonl against the pinned kernel tree.

  python3 tools/verify.py            # fast: schema, cited source lines, gold-vs-source checks
  python3 tools/verify.py --run      # also re-run every evidence command (about 40 s)

Environment: LINUX=/path/to/linux (default bench/repos/linux),
RG_BIN=/path/to/rg if ripgrep is not on PATH.

This uses only rg / sed / reading lines / a brace-at-column-0 enclosing-function heuristic.
It does not use codefast or any other indexer.
"""
import json, os, re, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from k3lib import (run, src, norm, hits, output_has, enclosing_name, body_tokens, parse_loc)

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUN = "--run" in sys.argv
errors = []


def err(q, msg):
    errors.append("%s: %s" % (q["id"], msg))


def in_function(q, path, line, fn):
    got = enclosing_name(path, line)
    if got == fn:
        return True
    # macro-defined functions: SYSCALL_DEFINEn(name) / DEFINE_IDTENTRY_*(name)
    if got and re.search(r"\((sys_)?%s\)$" % re.escape(re.sub(r"^sys_", "", fn)), got):
        return True
    err(q, "%s:%d is inside %r, expected %r" % (path, line, got, fn))
    return False


def check_gold(q):
    g, fam = q["gold"], q["family"]
    if fam == "F1":
        text = src(g["file"], g["line"])
        name = re.sub(r"^sys_", "", q["symbol"])
        if name not in text:
            err(q, "gold line does not mention %s" % name)
    elif fam == "F2":
        text = norm(" ".join(src(g["file"], n) for n in range(g["line"], g["line"] + 20)))
        for m in g["must_contain"]:
            if norm(m) not in text:
                err(q, "must_contain %r not found from %s:%d" % (m, g["file"], g["line"]))
    elif fam == "F3":
        n = 0
        for c in g["callers"]:
            for l in c["lines"]:
                n += 1
                in_function(q, c["file"], l, c["function"])
                if not re.search(r"\b%s\s*\(" % re.escape(q["symbol"]), src(c["file"], l)):
                    err(q, "%s:%d has no call of %s" % (c["file"], l, q["symbol"]))
        if not (3 <= n <= 25):
            err(q, "call site count %d outside 3..25" % n)
    elif fam == "F4":
        hdr, brace, close, toks = body_tokens(g["file"], g["line"] + (g["end_line"] - g["line"]) // 2)
        want = set(g["callees"]) | set(g["macros"]) | set(i["expr"] for i in g["indirect"])
        if hdr != g["line"] or close != g["end_line"]:
            err(q, "function extent %d-%d differs from gold %d-%d" % (hdr, close, g["line"], g["end_line"]))
        if set(toks) != want:
            err(q, "body tokens differ: body-only=%s gold-only=%s"
                % (sorted(set(toks) - want), sorted(want - set(toks))))
        if not (20 <= g["function_lines"] <= 80):
            err(q, "function length %d outside 20..80" % g["function_lines"])
    elif fam == "F5":
        text = src(g["file"], g["line"])
        if g["member"] not in text:
            err(q, "registration line lacks member %s" % g["member"])
        fpath, fline = g["function_def"].rsplit(":", 1)
        if g["function"] not in src(fpath, int(fline)):
            err(q, "function_def line lacks %s" % g["function"])
    elif fam == "F6":
        for s in g["dispatch_sites"] + g.get("other_sites_same_slot", []):
            in_function(q, s["file"], s["line"], s["function"])
        rpath, rline = g["registration"].rsplit(":", 1)
        if q["symbol"] not in src(rpath, int(rline)):
            err(q, "registration line lacks %s" % q["symbol"])
    elif fam == "F7":
        p = g["path"]
        if not (3 <= len(p) - 1 <= 6) or len(g["hops"]) != len(p) - 1:
            err(q, "bad hop count")
        for (a, b), h in zip(zip(p, p[1:]), g["hops"]):
            if (h["from"], h["to"]) != (a, b):
                err(q, "hop order mismatch at %s->%s" % (a, b))
            spath, sline = h["site"].rsplit(":", 1)
            in_function(q, spath, int(sline), a)
            text = src(spath, int(sline))
            if h["kind"] == "call":
                if not re.search(r"\b%s\s*\(" % re.escape(b), text):
                    err(q, "site %s does not call %s" % (h["site"], b))
            else:
                rpath, rline = h["registration"].rsplit(":", 1)
                if b not in src(rpath, int(rline)):
                    err(q, "registration %s lacks %s" % (h["registration"], b))
    elif fam == "F8":
        in_function(q, g["file"], g["line"], g["function"])
        if norm(q["symbol"]) not in norm(src(g["file"], g["line"])):
            err(q, "gold line lacks the string")
    elif fam == "F9":
        n = 0
        for w in g["writers"]:
            for l in w["lines"]:
                in_function(q, w["file"], l, w["function"])
                if not re.search(r"(->|\.)%s\b" % re.escape(g["field"]), src(w["file"], l)):
                    err(q, "%s:%d does not touch .%s" % (w["file"], l, g["field"]))
            n += 1
        if not (2 <= n <= 15):
            err(q, "writer count %d outside 2..15" % n)


def check_evidence(q):
    if q["family"] in ("F3", "F9"):
        # every gold call-site / writer line must be one of the classified search hits
        key = "callers" if q["family"] == "F3" else "writers"
        shown = {parse_loc(s)[:2] for s in q["evidence"][0]["lines"]}
        gold = {(c["file"], l) for c in q["gold"][key] for l in c["lines"]}
        if gold != shown:
            err(q, "gold lines and evidence[0] lines differ: %s" % sorted(gold ^ shown))
    for e in q["evidence"]:
        for s in e["lines"]:
            path, line, text = parse_loc(s)
            if norm(src(path, line)) != text:
                err(q, "evidence line changed: %s:%d" % (path, line))
        if RUN and not e["cmd"].startswith("sed -n '<line>p'"):
            out = run(e["cmd"])
            for s in e["lines"]:
                path, line, _ = parse_loc(s)
                if not output_has(out, e["cmd"], path, line):
                    err(q, "command does not show %s:%d: %s" % (path, line, e["cmd"]))
            if "output" in e and out.strip() != e["output"]:
                err(q, "command output %r != recorded %r" % (out.strip(), e["output"]))
            if "total_hits" in e and len(hits(out)) != e["total_hits"]:
                err(q, "hit count %d != recorded %d" % (len(hits(out)), e["total_hits"]))
            if "other_hits" in e:
                known = {parse_loc(s)[:2] for s in e["lines"]} | {
                    (o["loc"].rsplit(":", 1)[0], int(o["loc"].rsplit(":", 1)[1])) for o in e["other_hits"]}
                got = {(p, l) for p, l, _ in hits(out)}
                if got != known:
                    err(q, "hit set changed: +%s -%s" % (sorted(got - known), sorted(known - got)))


def main():
    path = os.path.join(HERE, "questions.jsonl")
    qs = [json.loads(l) for l in open(path)]
    fam = {}
    for q in qs:
        for k in ("id", "family", "question", "symbol", "gold", "evidence", "difficulty", "notes"):
            if k not in q:
                err(q, "missing key %s" % k)
        if q["difficulty"] not in ("easy", "medium", "hard"):
            err(q, "bad difficulty")
        fam[q["family"]] = fam.get(q["family"], 0) + 1
        try:
            check_gold(q)
            check_evidence(q)
        except Exception as ex:  # keep going, report everything
            err(q, "exception %r" % ex)
    if len(qs) != 72 or sorted(fam.items()) != [("F%d" % i, 8) for i in range(1, 10)]:
        errors.append("expected 72 questions, 8 per family; got %d %s" % (len(qs), fam))
    for e in errors:
        print("FAIL", e)
    print("%d questions, %d problems%s" % (len(qs), len(errors), "" if RUN else " (commands not re-run; use --run)"))
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
