#!/usr/bin/env python3
"""C call-graph oracle from clang's AST, one translation unit at a time.

Input: a compile_commands.json produced by a real build (build_compdb.sh).
Each translation unit is parsed by libclang (the same clang that compiled it)
with its exact compile flags. From the AST we take

  * every function definition located in a repository file, and
  * every CallExpr whose callee clang resolved to a FunctionDecl ("direct
    call"), resolved to the callee's *definition*: inside the translation unit
    via clang (static functions, header inlines, same-file functions), across
    translation units by external-linkage name, disambiguated by the recorded
    link commands when a name is defined more than once.

Calls through a function pointer are not resolved by a compiler. They are
listed in unresolved_sites.jsonl, their callers are marked judged=false, and a
clearly-labelled heuristic candidate set (address-taken functions with the
same canonical type) is written to a separate file.

Output format: bench/oracles/FORMAT.md.
"""
import argparse
import collections
import ctypes
import json
import multiprocessing
import os
import re
import subprocess
import sys

from clang import cindex
from clang.cindex import CursorKind

sys.setrecursionlimit(20000)

DROP_WITH_ARG = {"-o", "-MF", "-MT", "-MQ"}
DROP = {"-c", "-MMD", "-MD", "-MP", "-MM"}

_state = {}


def _init(libclang, build_root, git_files, sdk):
    cindex.Config.set_library_file(libclang)
    lib = cindex.conf.lib
    for fn in (lib.clang_getFileLocation, lib.clang_getSpellingLocation):
        fn.argtypes = [cindex.SourceLocation, ctypes.POINTER(cindex.c_object_p),
                       ctypes.POINTER(ctypes.c_uint), ctypes.POINTER(ctypes.c_uint),
                       ctypes.POINTER(ctypes.c_uint)]
        fn.restype = None
    _state.update(index=cindex.Index.create(), lib=lib, root=build_root, git=git_files, sdk=sdk,
                  relcache={})


def _rel(path):
    """Repository-relative path of a file clang saw, or None if it is not a
    file of the pinned commit (system header, build product, generated file)."""
    c = _state["relcache"]
    if path in c:
        return c[path]
    r = None
    if path:
        real = os.path.realpath(path)
        root = _state["root"]
        if real.startswith(root + os.sep):
            cand = os.path.relpath(real, root)
            if cand in _state["git"]:
                r = cand
            else:
                _state.setdefault("nongit", set()).add(cand)
    c[path] = r
    return r


def _loc(getter, loc):
    f, l, col, off = cindex.c_object_p(), ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint()
    getter(loc, ctypes.byref(f), ctypes.byref(l), ctypes.byref(col), ctypes.byref(off))
    name = cindex.File(f).name if f else None
    return name, l.value, col.value


def _clean(text):
    """clang spells unnamed types with the absolute path of the build tree;
    make such strings repository-relative so the output is machine-independent."""
    return text.replace(_state["root"] + os.sep, "")


def file_loc(loc):
    """Where the token is written in a file: the macro-argument position for a
    token passed as a macro argument, the expansion point for a token that
    comes from a macro body."""
    return _loc(_state["lib"].clang_getFileLocation, loc)


_lines = {}


def token_at(path, line, col):
    """Identifier written in the file at (line, col), looking through leading
    parentheses. libclang exposes no true spelling location
    (clang_getSpellingLocation returns the file location), so "does this token
    come from a macro body" is decided by reading the source: a name that is
    not literally written at its file location was produced by a macro."""
    if path not in _lines:
        try:
            with open(path, "rb") as fh:
                _lines[path] = fh.read().decode("latin-1").split("\n")
        except OSError:
            _lines[path] = []
    ls = _lines[path]
    if not (0 < line <= len(ls)):
        return ""
    text = ls[line - 1][col - 1:].lstrip("(* \t")
    n = 0
    while n < len(text) and (text[n].isalnum() or text[n] == "_"):
        n += 1
    return text[:n]


def clean_args(arguments, src):
    out, skip = [], False
    for a in arguments[1:]:
        if skip:
            skip = False
            continue
        if a in DROP_WITH_ARG:
            skip = True
            continue
        if a in DROP or a == src or os.path.basename(a) == os.path.basename(src) and not a.startswith("-"):
            continue
        out.append(a)
    return out


def process_tu(cmd):
    os.chdir(cmd["directory"])
    args = clean_args(cmd["arguments"], cmd["file"]) + ["-isysroot", _state["sdk"]]
    tu = _state["index"].parse(cmd["file"], args=args)
    res = {
        "file": cmd["file"], "output": cmd.get("output"), "functions": {}, "edges": [], "pending": [],
        "indirect": [], "addr_taken": [], "errors": [], "includes": set(), "external_calls": 0,
    }
    for d in tu.diagnostics:
        if d.severity >= cindex.Diagnostic.Error:
            res["errors"].append(f"{d.location.file}:{d.location.line}: {d.spelling}")
    main_rel = _rel(cmd["file"])
    if main_rel:
        res["includes"].add(main_rel)
    for inc in tu.get_includes():
        r = _rel(inc.include.name)
        if r:
            res["includes"].add(r)

    def func_record(c):
        f, name_line, name_col = file_loc(c.location)
        rel = _rel(f)
        if rel is None:
            return None
        ext = c.extent
        _, start_line, _ = file_loc(ext.start)
        _, end_line, _ = file_loc(ext.end)
        key = (rel, start_line, c.spelling)
        rec = res["functions"].get(key)
        if rec is None:
            rec = {
                "file": rel, "name": c.spelling, "line": start_line, "end_line": end_line,
                "receiver_or_class": "", "kind": "function",
                "linkage": "internal" if c.linkage == cindex.LinkageKind.INTERNAL else "external",
            }
            if name_line != start_line:
                rec["name_line"] = name_line
            if token_at(f, name_line, name_col) != c.spelling:
                rec["from_macro"] = True
            res["functions"][key] = rec
        return rec

    def endpoint(rec):
        return {"file": rec["file"], "name": rec["name"], "line": rec["line"]}

    def handle_call(ch, caller):
        ref = ch.referenced
        f, sline, scol = file_loc(ch.location)
        if ref is None or ref.kind != CursorKind.FUNCTION_DECL:
            # clang_getCursorReferenced does not look through a parenthesised
            # callee, so `(f)(x)` comes back with no referenced declaration
            # although it is a direct call of f. Look through parentheses and
            # implicit casts ourselves; anything else (`(*fp)(x)`, `a->b(x)`,
            # `(c ? f : g)(x)`) stays an indirect call.
            inner = next(iter(ch.get_children()), None)
            while inner is not None and inner.kind in (CursorKind.UNEXPOSED_EXPR, CursorKind.PAREN_EXPR):
                inner = next(iter(inner.get_children()), None)
            if (inner is not None and inner.kind == CursorKind.DECL_REF_EXPR and inner.referenced is not None
                    and inner.referenced.kind == CursorKind.FUNCTION_DECL):
                ref = inner.referenced
                f, sline, scol = file_loc(inner.location)
        srel = _rel(f)
        site = {"site_line": sline, "site_col": scol}
        if srel != caller["file"]:
            site["site_file"] = srel
        if ref is not None and ref.kind == CursorKind.FUNCTION_DECL:
            tok = token_at(f, sline, scol)
            kind = "static"
            if tok != ref.spelling:
                # The callee's name is not written at the call site: the call
                # is produced by expanding the macro named there.
                kind = "macro"
                site["via_macro"] = tok
            d = ref.get_definition()
            if d is not None:
                drec = func_record(d)
                if drec is None:
                    res["external_calls"] += 1
                    return
                e = {"caller": endpoint(caller), "callee": endpoint(drec), **site, "kind": kind}
                res["edges"].append(e)
            else:
                df, dl, _ = file_loc(ref.location)
                res["pending"].append({
                    "caller": endpoint(caller), **site, "kind": kind, "name": ref.spelling,
                    "decl_file": _rel(df), "decl_line": dl,
                    "internal": ref.linkage == cindex.LinkageKind.INTERNAL,
                })
        else:
            first = next(iter(ch.get_children()), None)
            ctype = ""
            if first is not None:
                t = first.type.get_canonical()
                if t.kind == cindex.TypeKind.POINTER:
                    t = t.get_pointee()
                ctype = _clean(t.spelling)
            info = {"caller": endpoint(caller), **site, "kind": "funcptr",
                    "callee_expr": ref.spelling if ref is not None else "",
                    "callee_decl_kind": ref.kind.name if ref is not None else "",
                    "fn_type": ctype}
            if ref is not None and ref.kind == CursorKind.FIELD_DECL and ref.semantic_parent is not None:
                info["struct"] = _clean(ref.semantic_parent.spelling)
            res["indirect"].append(info)
            caller["_indirect"] = caller.get("_indirect", 0) + 1

    def visit(cur, caller, callee_pos):
        """callee_pos: the first child of `cur` is the callee of a call (the
        callee is a call's first child, possibly wrapped in implicit casts and
        parentheses). A function named there is being called; a function named
        anywhere else is having its address taken."""
        first = True
        for ch in cur.get_children():
            k = ch.kind
            in_callee = callee_pos and first
            first = False
            if k == CursorKind.FUNCTION_DECL:
                if ch.is_definition():
                    rec = func_record(ch)
                    if rec is not None:
                        visit(ch, rec, False)
            elif k == CursorKind.CALL_EXPR:
                if caller is not None:
                    handle_call(ch, caller)
                visit(ch, caller, True)
            elif k == CursorKind.DECL_REF_EXPR:
                ref = ch.referenced
                if ref is not None and ref.kind == CursorKind.FUNCTION_DECL and not in_callee:
                    d = ref.get_definition()
                    drec = func_record(d) if d is not None else None
                    res["addr_taken"].append({
                        "name": ref.spelling, "fn_type": _clean(ref.type.get_canonical().spelling),
                        "def": [drec["file"], drec["line"]] if drec else None,
                        "internal": ref.linkage == cindex.LinkageKind.INTERNAL,
                    })
            else:
                visit(ch, caller, in_callee and k in (CursorKind.UNEXPOSED_EXPR, CursorKind.PAREN_EXPR))

    for top in tu.cursor.get_children():
        f = top.location.file
        if f is None or _rel(f.name) is None:
            continue
        if top.kind == CursorKind.FUNCTION_DECL:
            if top.is_definition():
                rec = func_record(top)
                if rec is not None:
                    visit(top, rec, False)
        else:
            visit(top, None, False)
    for rec in res["functions"].values():
        rec["indirect_call_sites"] = rec.pop("_indirect", 0)
    res["functions"] = list(res["functions"].values())
    res["includes"] = sorted(res["includes"])
    res["nongit"] = sorted(_state.get("nongit", ()))
    return res


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--build-root", required=True, help="scratch copy the build ran in")
    ap.add_argument("--out", required=True)
    ap.add_argument("--name", default="")
    ap.add_argument("--libclang", required=True)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--exclude-prefix", action="append", default=[],
                    help="repository-relative path prefix to drop from the oracle (e.g. tests/)")
    args = ap.parse_args()

    root = os.path.realpath(args.build_root)
    git_files = set(open(os.path.join(root, ".oracle-git-files")).read().split("\n")) - {""}
    commit = open(os.path.join(root, ".oracle-commit")).read().strip()
    sdk = subprocess.check_output(["xcrun", "--show-sdk-path"], text=True).strip()
    cmds = json.load(open(os.path.join(root, "compile_commands.json")))
    links = json.load(open(os.path.join(root, "link_commands.json")))

    with multiprocessing.Pool(args.jobs, initializer=_init, initargs=(args.libclang, root, git_files, sdk)) as pool:
        results = pool.map(process_tu, cmds, chunksize=1)

    excluded = lambda p: any(p.startswith(x) for x in args.exclude_prefix)

    # ---- merge functions ----------------------------------------------------
    funcs = {}
    analysed, nongit, tu_errors = set(), set(), {}
    obj_of_def = collections.defaultdict(set)   # (file, line, name) -> objects defining it
    ext_defs = collections.defaultdict(set)     # name -> {(file, line)}
    for r in results:
        analysed.update(r["includes"])
        nongit.update(r["nongit"])
        if r["errors"]:
            tu_errors[os.path.relpath(r["file"], root)] = r["errors"][:5]
        for f in r["functions"]:
            key = (f["file"], f["line"], f["name"])
            old = funcs.get(key)
            if old is None:
                funcs[key] = f
            else:
                old["indirect_call_sites"] = max(old["indirect_call_sites"], f["indirect_call_sites"])
                old["end_line"] = max(old["end_line"], f["end_line"])
            obj_of_def[key].add(r["output"])
            if f["linkage"] == "external":
                ext_defs[f["name"]].add((f["file"], f["line"]))

    # ---- link units -----------------------------------------------------------
    # object -> set of binaries it is linked into directly. Objects that only
    # reach a binary through a static archive (the deps) are not in any link
    # command we recorded and are treated as available to every binary.
    binaries_of = collections.defaultdict(set)
    for l in links:
        for inp in l["inputs"]:
            if inp.endswith(".o"):
                binaries_of[inp].add(l["output"])

    def resolve_extern(name, caller_obj):
        cands = sorted(ext_defs.get(name, ()))
        if len(cands) <= 1:
            return cands, None
        my_bins = binaries_of.get(caller_obj, set())
        keep = []
        for c in cands:
            objs = obj_of_def[(c[0], c[1], name)]
            direct = set().union(*(binaries_of.get(o, set()) for o in objs)) if objs else set()
            in_archive = any(o not in binaries_of for o in objs)
            if not my_bins or in_archive or (direct & my_bins):
                keep.append(c)
        return (keep or cands), "link-variant"

    # ---- edges ----------------------------------------------------------------
    edges = {}
    counts = collections.Counter()

    def add_edge(e):
        k = (e["caller"]["file"], e["caller"]["line"], e["caller"]["name"], e.get("site_file", ""), e["site_line"],
             e["site_col"], e["callee"]["file"], e["callee"]["line"], e["callee"]["name"], e["kind"])
        if k not in edges:
            edges[k] = e

    unresolved_decl = collections.Counter()
    for r in results:
        counts["external_calls"] += r["external_calls"]
        for e in r["edges"]:
            add_edge(e)
        for p in r["pending"]:
            cands, why = ([], None) if p["internal"] else resolve_extern(p["name"], r["output"])
            if not cands:
                if p["decl_file"] and not p["name"].startswith("__builtin_"):
                    counts["calls_to_repo_declared_but_undefined"] += 1
                    unresolved_decl[p["name"]] += 1
                else:
                    counts["external_calls"] += 1
                continue
            for (cf, cl) in cands:
                e = {"caller": p["caller"], "callee": {"file": cf, "name": p["name"], "line": cl},
                     "site_line": p["site_line"], "site_col": p["site_col"], "kind": p["kind"]}
                for opt in ("site_file", "via_macro"):
                    if opt in p:
                        e[opt] = p[opt]
                if why and len(cands) > 1:
                    e["candidates"] = len(cands)
                    e["candidate_reason"] = why
                add_edge(e)

    # ---- indirect sites + heuristic candidates --------------------------------
    indirect = {}
    for r in results:
        for s in r["indirect"]:
            k = (s["caller"]["file"], s["caller"]["line"], s.get("site_file", ""), s["site_line"], s["site_col"])
            indirect.setdefault(k, s)
    taken_by_type = collections.defaultdict(set)
    for r in results:
        for a in r["addr_taken"]:
            if a["def"]:
                defs = [tuple(a["def"])]
            elif not a["internal"]:
                defs = sorted(ext_defs.get(a["name"], ()))
            else:
                defs = []
            for d in defs:
                taken_by_type[a["fn_type"]].add((d[0], d[1], a["name"]))
    cand_edges = []
    for s in indirect.values():
        cands = sorted(taken_by_type.get(s["fn_type"], ()))
        s["signature_candidates"] = len(cands)
        for (cf, cl, name) in cands:
            e = {"caller": s["caller"], "callee": {"file": cf, "name": name, "line": cl},
                 "site_line": s["site_line"], "site_col": s["site_col"], "kind": "funcptr",
                 "candidates": len(cands), "resolution": "signature-match"}
            if "site_file" in s:
                e["site_file"] = s["site_file"]
            cand_edges.append(e)

    # ---- scope filter, flags, write ---------------------------------------------
    in_scope = lambda p: p in analysed and not excluded(p)
    # A function whose body contains a conditional-compilation directive has
    # source text this build configuration never compiled. The key cannot say
    # what that text calls, so the function is not judged: an edge a tool
    # reports from it (read off the inactive branch) is unjudged, not wrong.
    cond_re = re.compile(r"^\s*#\s*(if|ifdef|ifndef|elif|else)\b")
    src_lines = {}

    def has_conditional(f):
        if f["file"] not in src_lines:
            with open(os.path.join(root, f["file"]), "rb") as fh:
                src_lines[f["file"]] = fh.read().decode("latin-1").split("\n")
        return any(cond_re.match(x) for x in src_lines[f["file"]][f["line"] - 1:f["end_line"]])

    flist = []
    for f in funcs.values():
        if not in_scope(f["file"]):
            continue
        f["judged"] = f["indirect_call_sites"] == 0
        if has_conditional(f):
            f["judged"] = False
            f["unjudged_reasons"] = ["conditional-compilation"]
        flist.append(f)
    flist.sort(key=lambda f: (f["file"], f["line"], f["name"]))
    ekey = lambda e: (e["caller"]["file"], e["site_line"], e["site_col"], e["callee"]["file"], e["callee"]["line"], e["kind"])
    elist = sorted((e for e in edges.values() if in_scope(e["caller"]["file"]) and in_scope(e["callee"]["file"])), key=ekey)
    cand_edges = sorted((e for e in cand_edges if in_scope(e["caller"]["file"]) and in_scope(e["callee"]["file"])), key=ekey)
    ulist = sorted((s for s in indirect.values() if in_scope(s["caller"]["file"])),
                   key=lambda s: (s["caller"]["file"], s["site_line"], s["site_col"]))

    scope_files = sorted(p for p in analysed if not excluded(p))
    not_compiled = sorted(p for p in git_files if p.endswith((".c", ".h")) and p not in analysed and not excluded(p))
    by_kind = collections.Counter(e["kind"] for e in elist)
    by_top = collections.Counter(p.split("/")[0] for p in scope_files)
    clang_version = subprocess.check_output([cmds[0]["arguments"][0], "--version"], text=True).splitlines()[0]
    meta = {
        "repo": args.name, "commit": commit, "language": "c",
        "oracle": "clang AST (libclang): CallExpr with a FunctionDecl callee, per translation unit",
        "clang_version": clang_version, "libclang": args.libclang,
        "python_clang_bindings": getattr(cindex, "__version__", "21.1.7"),
        "platform": " ".join(os.uname()[i] for i in (0, 2, 4)),
        "build": "make -C src MALLOC=libc redis-server redis-cli redis-benchmark (macOS default configuration, no TLS, no jemalloc)",
        "tests_included": False,
        "caller_key": "enclosing function definition",
        "translation_units": len(cmds),
        "translation_units_with_errors": tu_errors,
        # sorted, and without xxhash's per-build cache directory name, so that
        # meta.json does not depend on `make -j` ordering or on the build path
        "link_units": sorted({re.sub(r"/cachedObjs/[0-9a-f]+/", "/cachedObjs/*/", os.path.relpath(l["output"], root))
                              for l in links}),
        "analysed_files": scope_files,
        "excluded_prefixes": args.exclude_prefix,
        "build_generated_files_seen": sorted(nongit),
        "source_files_not_compiled_in_this_configuration": not_compiled,
        "repo_declared_functions_without_definition_in_build": dict(unresolved_decl.most_common()),
        "counts": {
            "analysed_files": len(scope_files), "analysed_files_by_top_dir": dict(by_top),
            "translation_units": len(cmds),
            "functions": len(flist), "functions_judged": sum(1 for f in flist if f["judged"]),
            "functions_with_conditional_compilation": sum(1 for f in flist if f.get("unjudged_reasons")),
            "edges": len(elist), "edges_by_kind": dict(by_kind),
            "edges_with_link_variants": sum(1 for e in elist if e.get("candidates", 1) > 1),
            "indirect_call_sites": len(ulist),
            "indirect_call_sites_with_signature_candidates": sum(1 for s in ulist if s["signature_candidates"]),
            "funcptr_candidate_edges": len(cand_edges),
            "calls_to_external_functions": counts["external_calls"],
            "calls_to_repo_declared_but_undefined": counts["calls_to_repo_declared_but_undefined"],
        },
    }
    os.makedirs(args.out, exist_ok=True)

    def jl(name, rows):
        with open(os.path.join(args.out, name), "w") as fh:
            for row in rows:
                fh.write(json.dumps(row) + "\n")

    jl("edges.jsonl", elist)
    jl("functions.jsonl", flist)
    jl("unresolved_sites.jsonl", ulist)
    jl("edges.funcptr_candidates.jsonl", cand_edges)
    with open(os.path.join(args.out, "meta.json"), "w") as fh:
        json.dump(meta, fh, indent=1)
        fh.write("\n")
    print(json.dumps(meta["counts"]), file=sys.stderr)
    if tu_errors:
        print(f"WARNING: {len(tu_errors)} translation units had clang errors", file=sys.stderr)


if __name__ == "__main__":
    main()
