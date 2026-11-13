# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Agent experiment, family P: carry a method rename through a Python code base.

No compiler says what is left behind here. The method has overrides in subclasses, which are
renamed with it, and namesakes in unrelated classes, which must stay as they are. The task is
done when the project's own test suite passes again (the tests that already failed before the
change are left out of the command).

Candidates are found with Python's `ast` module, not with codefast: a method of a class of the
package, used through `.name` 6 to 40 times in at least 3 files, that at least one unrelated
class also defines. A candidate is kept when a reference solution exists: the rename is made by
rope (an independent refactoring library, with its class-hierarchy option), and the test suite
passes on the result. That also tells how many lines have to change.

usage: python bench/tier2/make_p_tasks.py [--n 10] [--tree PATH]
       (run with the interpreter of target/ab/d/env-sqlglot, which has rope and pytest)
"""
import argparse, ast, collections, difflib, hashlib, json, os, shutil, subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
AB = ROOT / "target/ab/p"
TREES = ROOT / "bench/repos/_trees"
PY = ROOT / "target/ab/d/env-sqlglot/bin/python"
PKG = "sqlglot"


def sh(cmd, cwd, timeout=1800):
    e = dict(os.environ, PYTHONPATH=str(cwd), PYTHONDONTWRITEBYTECODE="1")
    try:
        p = subprocess.run(cmd, cwd=cwd, shell=True, capture_output=True, text=True, timeout=timeout, env=e)
    except subprocess.TimeoutExpired:
        return 124, "timed out"
    return p.returncode, p.stdout + p.stderr


def failing(tree):
    """Ids of the tests that fail in `tree`, and how many ran."""
    xml = tree / ".p-junit.xml"
    sh(f"nice -n 10 {PY} -m pytest -q -p no:cacheprovider --junitxml={xml} tests", tree)
    import xml.etree.ElementTree as ET

    bad, ran = set(), 0
    if xml.exists():
        for tc in ET.parse(xml).getroot().iter("testcase"):
            ran += 1
            if tc.find("failure") is not None or tc.find("error") is not None:
                bad.add(node_id(tc.get("classname") or "", tc.get("name") or ""))
        xml.unlink()
    return bad, ran


def node_id(classname, name):
    """The pytest id of a junit test case: `tests.test_x.TestY` + `test_z` is
    `tests/test_x.py::TestY::test_z`."""
    parts = classname.split(".")
    cut = next((i for i, p in enumerate(parts) if p[:1].isupper()), len(parts))
    path = "/".join(parts[:cut]) + ".py"
    return "::".join([path] + parts[cut:] + [name])


def scan(tree):
    """Classes of the package (name -> bases, methods with their place) and every `.attr` use."""
    classes, uses, strings = {}, collections.defaultdict(list), collections.Counter()
    dup = set()
    for p in sorted(tree.rglob("*.py")):
        rel = str(p.relative_to(tree))
        if not (rel.startswith(PKG + "/") or rel.startswith("tests/")):
            continue
        try:
            mod = ast.parse(p.read_text())
        except SyntaxError:
            continue
        for node in ast.walk(mod):
            if isinstance(node, ast.Attribute):
                uses[node.attr].append((rel, node.lineno))
            elif isinstance(node, ast.Constant) and isinstance(node.value, str) and node.value.isidentifier():
                strings[node.value] += 1
            elif isinstance(node, ast.ClassDef) and rel.startswith(PKG + "/"):
                bases = [b.id if isinstance(b, ast.Name) else b.attr for b in node.bases if isinstance(b, (ast.Name, ast.Attribute))]
                methods = {f.name: f.lineno for f in node.body if isinstance(f, (ast.FunctionDef, ast.AsyncFunctionDef))}
                if node.name in classes:
                    dup.add(node.name)
                classes[node.name] = dict(file=rel, line=node.lineno, bases=bases, methods=methods)
    return classes, uses, strings, dup


def family(classes, root):
    """`root` and every class below it."""
    out, frontier = {root}, [root]
    while frontier:
        c = frontier.pop()
        for name, info in classes.items():
            if c in info["bases"] and name not in out:
                out.add(name)
                frontier.append(name)
    return out


def candidates(tree):
    classes, uses, strings, dup = scan(tree)
    out = []
    for cname, info in classes.items():
        if cname in dup:
            continue
        for m, line in info["methods"].items():
            if m.startswith("__") or len(m) < 5 or strings[m]:
                continue  # dunders, and names also reached through strings (getattr): not a plain rename
            # Only the topmost class that defines it: the rename is of the whole hierarchy.
            if any(m in classes[b]["methods"] for b in ancestors(classes, cname)):
                continue
            fam = family(classes, cname)
            if fam & dup:
                continue
            definers = sorted(c for c in fam if m in classes[c]["methods"])
            rivals = sorted(c for c in classes if c not in fam and m in classes[c]["methods"])
            sites = uses.get(m, [])
            files = {f for f, _ in sites}
            if 6 <= len(sites) <= 40 and len(files) >= 3 and rivals and len(definers) <= 6:
                out.append(dict(cls=cname, name=m, file=info["file"], line=line, uses=len(sites), files=len(files), overrides=[c for c in definers if c != cname], rivals=rivals))
    out.sort(key=lambda c: hashlib.sha256(f"{c['cls']}.{c['name']}".encode()).hexdigest())
    return out, classes


def ancestors(classes, name):
    out, frontier = set(), list(classes.get(name, {}).get("bases", []))
    while frontier:
        b = frontier.pop()
        if b in classes and b not in out:
            out.add(b)
            frontier.extend(classes[b]["bases"])
    return out


def rope_rename(tree, c, new):
    from rope.base.project import Project
    from rope.refactor.rename import Rename

    proj = Project(str(tree), ropefolder=None)
    try:
        res = proj.get_resource(c["file"])
        text = res.read()
        lines = text.split("\n")
        col = lines[c["line"] - 1].index("def " + c["name"]) + 4
        offset = sum(len(l) + 1 for l in lines[: c["line"] - 1]) + col
        changes = Rename(proj, res, offset).get_changes(new, in_hierarchy=True, docs=False)
        proj.do(changes)
    finally:
        proj.close()


def changed_lines(a, b):
    n, files = 0, 0
    for p in sorted(a.rglob("*.py")):
        q = b / p.relative_to(a)
        x, y = p.read_text(), q.read_text() if q.exists() else ""
        if x != y:
            files += 1
            n += sum(1 for l in difflib.unified_diff(x.split("\n"), y.split("\n"), lineterm="", n=0) if l.startswith("+") and not l.startswith("+++"))
    return n, files


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=10)
    ap.add_argument("--tree", default="")
    ap.add_argument("--max-tries", type=int, default=30)
    a = ap.parse_args()
    src = Path(a.tree) if a.tree else sorted(TREES.glob("tobymao__sqlglot@*"))[-1]
    AB.mkdir(parents=True, exist_ok=True)
    base = AB / "base"
    if base.exists():
        shutil.rmtree(base)
    shutil.copytree(src, base, symlinks=True, ignore=shutil.ignore_patterns(".git", ".codefast", "__pycache__", "sqlglotrs"))
    before, ran = failing(base)
    print(f"tree {src.name}: {ran} tests, {len(before)} fail before any change", flush=True)
    deselect = " ".join(f"--deselect '{t}'" for t in sorted(before))
    cands, classes = candidates(base)
    print(len(cands), "candidates", flush=True)
    tasks = []
    for c in cands[: a.max_tries]:
        if len(tasks) >= a.n:
            break
        new = c["name"] + "_step"
        gold = AB / "gold"
        if gold.exists():
            shutil.rmtree(gold)
        shutil.copytree(base, gold, symlinks=True)
        try:
            rope_rename(gold, c, new)
        except Exception as e:  # rope gives up on some constructs: not a usable task
            print(f"skip {c['cls']}.{c['name']}: rope: {type(e).__name__}: {str(e)[:120]}", flush=True)
            continue
        lines, files = changed_lines(base, gold)
        after, ran2 = failing(gold)
        ok = ran2 == ran and after <= before and lines >= 5
        print(f"{'valid  ' if ok else 'INVALID'} P-{c['cls']}-{c['name']}: {lines} lines in {files} files, overrides {c['overrides']}, rivals {len(c['rivals'])}, new failures {len(after - before)}", flush=True)
        if not ok:
            continue
        fam = sorted(family(classes, c["cls"]))
        tasks.append(dict(
            id=f"P-{c['cls']}-{c['name']}", family="P", repo_name="tobymao/sqlglot", base=str(base), cls=c["cls"], name=c["name"], new_name=new, file=c["file"], line=c["line"],
            uses=c["uses"], overrides=c["overrides"], rivals=c["rivals"][:8], gold_lines=lines, gold_files=files, hierarchy=fam,
            check_cmd=f"cd {{repo}} && PYTHONPATH={{repo}} {PY} -m pytest -q -x -p no:cacheprovider tests {deselect}".strip(),
        ))
    shutil.rmtree(AB / "gold", ignore_errors=True)
    json.dump(tasks, open(AB / "tasks.json", "w"), indent=1)
    print(len(tasks), "tasks written to", AB / "tasks.json")


if __name__ == "__main__":
    main()
