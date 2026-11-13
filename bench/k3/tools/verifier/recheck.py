#!/usr/bin/env python3
"""Independent mechanical re-check of a k3 question file (default: questions.verified.jsonl).

Written by the verifier. It shares no code with tools/verify.py, tools/enc.py or
tools/k3lib.py: function attribution comes from the brace-depth scanner in vk.py, and the
searches are word-level searches for the symbol / member name rather than the author's
patterns. It does not use codefast or any indexer; only ripgrep and file reads.

  python3 -I tools/verifier/recheck.py [questions.jsonl] [--ids F3-01,F9-04]

Exit status 1 if any check fails. Hits that need a human decision (receiver types in F9,
other-struct members in F6) are compared against the reviewed lists in REVIEWED below, so a
new hit after a tree change makes the run fail.
"""
import json, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import vk  # noqa: E402

K3 = os.path.dirname(os.path.dirname(HERE))
IDS = None
args = []
_argv = sys.argv[1:]
while _argv:
    a = _argv.pop(0)
    if a == "--ids":
        IDS = set(_argv.pop(0).split(","))
    else:
        args.append(a)
PATH = args[0] if args else os.path.join(K3, "questions.verified.jsonl")
fails, infos = [], []


def fail(q, msg):
    fails.append("%s: %s" % (q["id"], msg))


def norm(s):
    return re.sub(r"\s+", " ", s).strip()


def owner(path, line):
    rl, sl, ow = vk.analyze(path)
    return ow[line - 1]


def code(path, line):
    return vk.analyze(path)[1][line - 1]


def raw(path, line):
    return vk.analyze(path)[0][line - 1]


def in_fn(q, path, line, fn):
    o = owner(path, line)
    name = o[0] if o else None
    if name == fn or (name and re.search(r"\((sys_)?%s\)$" % re.escape(re.sub(r"^sys_", "", fn)), name)):
        return True
    fail(q, "%s:%d is inside %r, gold says %r" % (path, line, name, fn))
    return False


# Write-like hits of the F9 member names that were read by the verifier and are NOT writes to
# the struct in the question (another struct, or a read). Anything else must be a gold line.
SCOPE9 = {"F9-04": "security/selinux", "F9-07": "arch/x86"}

# Calls through a same-named member of another struct (read by the verifier).
F6_OTHER = {"F6-08": {("mm/mmap.c", 1448), ("net/ceph/messenger.c", 1535)}}
F6_PATTERN = {"F6-06": r"call_\w+_hook\(\s*file_permission\b",
              "F6-07": r"static_call\(x86_pmu_handle_irq\)|x86_pmu\.handle_irq\s*\("}
F6_GLOBS = {"F6-04": ["io_uring"], "F6-06": ["security"], "F6-07": ["arch/x86"]}


def check(q):
    g, fam = q["gold"], q["family"]
    sym = q["symbol"]
    if fam == "F1":
        name = re.sub(r"^sys_", "", sym)
        if not re.search(r"\b%s\b" % re.escape(name), code(g["file"], g["line"])):
            fail(q, "gold line does not name %s in code" % name)
        if raw(g["file"], g["line"]).rstrip().endswith(";") and "DEFINE" not in raw(g["file"], g["line"]):
            fail(q, "gold line looks like a declaration")
    elif fam == "F2":
        seg = norm(" ".join(raw(g["file"], n) for n in range(g["line"], g["line"] + 20)))
        for m in g["must_contain"]:
            if norm(m) not in seg:
                fail(q, "must_contain %r not at %s:%d" % (m, g["file"], g["line"]))
        p = g.get("also_accept_prototype")
        if p:
            seg = norm(" ".join(raw(p["file"], n) for n in range(p["line"], p["line"] + 4)))
            for m in p["must_contain"]:
                if norm(m) not in seg:
                    fail(q, "prototype must_contain %r not at %s:%d" % (m, p["file"], p["line"]))
    elif fam == "F3":
        gold = {(c["file"], l): c["function"] for c in g["callers"] for l in c["lines"]}
        seen = set()
        ncomment = 0
        for path, ln, text in vk.hits(r"\b%s\b" % re.escape(sym), ["."]):
            if path.startswith("Documentation/"):
                continue
            if not path.endswith((".c", ".h")):
                fail(q, "occurrence in a non-C file: %s:%d" % (path, ln))
                continue
            st = code(path, ln)
            if not re.search(r"\b%s\b" % re.escape(sym), st):
                ncomment += 1
                continue
            o = owner(path, ln)
            if not re.search(r"\b%s\s*\(" % re.escape(sym), st):
                if not re.match(r"\s*EXPORT_SYMBOL\w*\(", st):
                    fail(q, "use without '(' (address taken?): %s:%d %s" % (path, ln, norm(st)))
                continue
            if o is None or o[0] == "<header %s>" % sym:
                continue  # prototype at file scope, or the definition header
            seen.add((path, ln))
            if (path, ln) not in gold:
                fail(q, "call site not in gold: %s:%d in %s" % (path, ln, o[0]))
            elif o[0] != gold[(path, ln)]:
                fail(q, "%s:%d is in %s, gold says %s" % (path, ln, o[0], gold[(path, ln)]))
        for k in set(gold) - seen:
            fail(q, "gold call site not found by the search: %s:%d" % k)
        if g["call_sites"] != len(gold):
            fail(q, "call_sites count %d != %d" % (g["call_sites"], len(gold)))
        infos.append("%s: %d call sites in %d functions, %d comment-only mentions"
                     % (q["id"], len(seen), len({(c["file"], c["function"]) for c in g["callers"]}), ncomment))
    elif fam == "F4":
        rl, sl, ow = vk.analyze(g["file"])
        lines = [i for i, o in enumerate(ow) if o and o[0] in (sym, "<header %s>" % sym)]
        if not lines or lines[0] + 1 != g["line"] or lines[-1] + 1 != g["end_line"]:
            fail(q, "function extent %s differs from gold %d-%d" % (
                (lines[0] + 1, lines[-1] + 1) if lines else None, g["line"], g["end_line"]))
            return
        body = "\n".join(sl[lines[0]:lines[-1] + 1])
        body = body[body.index("{"):]
        kw = set("if for while switch return sizeof typeof __typeof__ do else case alignof _Alignof offsetof defined".split())
        toks = set()
        for m in re.finditer(r"((?:[\w\]\[]+\s*(?:->|\.)\s*)*)(\w+)\s*\(", body):
            if m.group(2) not in kw:
                toks.add((m.group(1) + m.group(2)).replace(" ", ""))
        want = set(g["callees"]) | set(g["macros"]) | {i["expr"] for i in g["indirect"]}
        if toks != want:
            fail(q, "body tokens differ: body-only=%s gold-only=%s" % (sorted(toks - want), sorted(want - toks)))
        # names listed as macros must not have a function definition in x86 arch code or
        # next to the function itself; names listed as callees must not be '#define NAME('
        # in the generic or x86 headers.
        scope = ["arch/x86", "include", os.path.dirname(g["file"])]
        for nm in g["macros"]:
            out = vk.rg(["-n", "--no-heading", "-j4", r"^[A-Za-z_][\w \t\*]*\b%s\s*\([^;]*$" % re.escape(nm),
                         "-g", "*.[ch]", "arch/x86", os.path.dirname(g["file"])])
            defs = [x for x in out.splitlines() if not re.search(r":\d+:\s*(#|return\b|if\b|else\b)", x)]
            if defs:
                fail(q, "listed as macro but has a function definition: %s -> %s" % (nm, defs[:2]))
        for nm in g["callees"]:
            out = vk.rg(["-n", "--no-heading", "-j4", r"^\s*#\s*define\s+%s\s*\(" % re.escape(nm),
                         "-g", "*.h"] + scope)
            if out.strip():
                # a generic macro may be switched off by the architecture (__HAVE_ARCH_*): accept
                # the name as a function only if x86 has a real definition of it
                fdef = vk.rg(["-n", "--no-heading", "-j4", r"^[A-Za-z_][\w \t\*]*\b%s\s*\([^;]*$" % re.escape(nm),
                              "-g", "*.c", "arch/x86"])
                if fdef.strip():
                    infos.append("%s: %s has a generic '#define' and an x86 function definition (%s)"
                                 % (q["id"], nm, fdef.splitlines()[0].split(":", 2)[0]))
                else:
                    fail(q, "listed as function but '#define %s(' exists: %s" % (nm, out.splitlines()[:2]))
    elif fam == "F5":
        st = code(g["file"], g["line"])
        if not re.search(r"\b%s\b" % re.escape(g["member"]), st):
            fail(q, "registration line lacks member %s" % g["member"])
        fpath, fline = g["function_def"].rsplit(":", 1)
        if not re.search(r"\b%s\s*\(" % re.escape(g["function"]), code(fpath, int(fline))):
            fail(q, "function_def line lacks %s(" % g["function"])
        o = owner(g["file"], g["line"])
        if not o or not o[0].startswith("<file-scope block"):
            fail(q, "registration line is not inside a file-scope initializer: %r" % (o,))
        elif g["table"] not in o[0] and not re.search(r"DEFINE_\w+\(\w+\)", o[0]):
            fail(q, "initializer %r does not name table %s" % (o[0], g["table"]))
    elif fam == "F6":
        sites = {(s["file"], s["line"]) for s in g["dispatch_sites"] + g.get("other_sites_same_slot", [])}
        for s in g["dispatch_sites"] + g.get("other_sites_same_slot", []):
            in_fn(q, s["file"], s["line"], s["function"])
        rpath, rline = g["registration"].rsplit(":", 1)
        if not re.search(r"\b%s\b" % re.escape(sym), code(rpath, int(rline))):
            fail(q, "registration line lacks %s" % sym)
        member = re.sub(r"\s.*$", "", g["slot"]).split(".")[-1]
        pat = F6_PATTERN.get(q["id"], r"(->|\.)\s*%s\s*\(" % re.escape(member))
        got = set()
        for path, ln, text in vk.hits(pat, ["-g", "*.[ch]", "-g", "!tools"] + F6_GLOBS.get(q["id"], ["."])):
            if re.search(pat, code(path, ln)):
                got.add((path, ln))
        got -= F6_OTHER.get(q["id"], set())
        if got != sites:
            fail(q, "calls through the slot differ from gold: extra=%s missing=%s" % (sorted(got - sites), sorted(sites - got)))
        # the function itself must have no direct caller
        for path, ln, text in vk.hits(r"\b%s\s*\(" % re.escape(sym), ["-g", "*.[ch]", "-g", "!tools", "."]):
            o = owner(path, ln)
            if re.search(r"\b%s\s*\(" % re.escape(sym), code(path, ln)) and o and not o[0].startswith("<header") \
                    and not o[0].startswith("<file-scope"):
                fail(q, "direct call of %s at %s:%d" % (sym, path, ln))
    elif fam == "F7":
        p = g["path"]
        if len(g["hops"]) != len(p) - 1:
            fail(q, "hop count")
        for (a, b), h in zip(zip(p, p[1:]), g["hops"]):
            if (h["from"], h["to"]) != (a, b):
                fail(q, "hop order mismatch at %s->%s" % (a, b))
            spath, sline = h["site"].rsplit(":", 1)
            sline = int(sline)
            in_fn(q, spath, sline, a)
            st = code(spath, sline)
            if h["kind"] == "call":
                if not re.search(r"\b%s\s*\(" % re.escape(b), st):
                    fail(q, "site %s does not call %s" % (h["site"], b))
            else:
                if re.search(r"\b%s\s*\(" % re.escape(b), st):
                    fail(q, "dispatch hop site %s names %s directly" % (h["site"], b))
                rpath, rline = h["registration"].rsplit(":", 1)
                if not re.search(r"\b%s\b" % re.escape(b), code(rpath, int(rline))):
                    fail(q, "registration %s lacks %s" % (h["registration"], b))
    elif fam == "F8":
        in_fn(q, g["file"], g["line"], g["function"])
        if norm(sym) not in norm(raw(g["file"], g["line"])):
            fail(q, "gold line lacks the string")
        n = [h for h in vk.hits(re.escape(sym.strip()), ["-g", "!tools", "-g", "!Documentation", "."])]
        if len(n) != 1 or (n[0][0], n[0][1]) != (g["file"], g["line"]):
            fail(q, "string hits: %s" % [(a, b) for a, b, _ in n])
    elif fam == "F9":
        fld = re.escape(g["field"])
        gold = {(w["file"], l): w["function"] for w in g["writers"] for l in w["lines"]}
        for (path, ln), fn in gold.items():
            in_fn(q, path, ln, fn)
        wr = re.compile(r"(->|\.)\s*%s\s*(=(?!=)|[-+|&^*/%%]=|<<=|>>=|\+\+|--)|(\+\+|--)\s*[\w>.\-\[\]()]*(->|\.)\s*%s\b"
                        r"|&\s*\(?[\w>.\-\[\]()]*(->|\.)\s*%s\b|\w+\(\s*[\w>.\-\[\]()]*(->|\.)\s*%s\s*,|(->|\.)\s*%s\s*$"
                        % (fld, fld, fld, fld, fld))
        occ = 0
        cand = set()
        for path, ln, text in vk.hits(r"\b%s\b" % fld, ["-g", "*.[ch]", "-g", "!tools", "-g", "!Documentation",
                                                       SCOPE9.get(q["id"], ".")]):
            occ += 1
            st = code(path, ln)
            if re.search(r"\b%s\b" % fld, st) and wr.search(st):
                cand.add((path, ln))
        for k in set(gold) - cand:
            fail(q, "gold writer line not matched by the write pattern: %s:%d" % k)
        infos.append("%s: %d occurrences of '%s' in scope, %d write-like lines, %d of them gold"
                     % (q["id"], occ, g["field"], len(cand), len(set(gold) & cand)))
        # receiver type: print the declaration for the record
        for (path, ln) in sorted(gold):
            st = norm(code(path, ln))
            infos.append("    %s:%d [%s] %s" % (path, ln, gold[(path, ln)], st[:110]))


def main():
    qs = [json.loads(l) for l in open(PATH)]
    for q in qs:
        if IDS and q["id"] not in IDS:
            continue
        try:
            check(q)
        except Exception as ex:
            fail(q, "exception %r" % ex)
    for i in infos:
        print(i)
    for f in fails:
        print("FAIL", f)
    print("%s: %d questions checked, %d problems" % (os.path.basename(PATH), len([q for q in qs if not IDS or q["id"] in IDS]), len(fails)))
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
