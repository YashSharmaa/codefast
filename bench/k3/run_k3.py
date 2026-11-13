# /// script
# requires-python = ">=3.12"
# dependencies = ["tiktoken"]
# ///
"""K3: per-question token efficiency on the Linux kernel, no model in the loop.

For every question each arm runs a fixed, documented command sequence. We record what a
harness would put in the model's context (the commands' output, cut at the harness's 30,000
character limit), count tokens with o200k_base, and check the gold answer against that text.

Arms
  codefast   one codefast call per question.
  rg-expert  the cheapest grep / read sequence that can expose the answer, written the way a
             careful engineer would: definition-shaped regexes, `git grep -p` for enclosing
             functions, reading exactly the definition instead of the whole file. Where the
             procedure needs knowledge only the answer provides (which function to read next on
             a call path), it is given that knowledge, so this arm is a LOWER bound on what a
             grep-based agent pays.
"""
import argparse, json, os, re, subprocess, sys, time
from pathlib import Path
import tiktoken

ROOT = Path(__file__).resolve().parents[2]
LINUX = ROOT / "bench/repos/linux"
CF = Path(os.environ.get("CODEFAST_BIN", ROOT / "target/release/codefast"))
ENC = tiktoken.get_encoding("o200k_base")
HARNESS_CAP = 30_000  # characters of tool output a harness keeps
RG = os.environ.get("RG", "/opt/homebrew/bin/rg")  # a real ripgrep binary, not a shell function


def ntok(s: str) -> int:
    return len(ENC.encode(s, disallowed_special=()))


def sh(cmd: str, timeout=120):
    t0 = time.time()
    try:
        real = re.sub(r"(^|[;&|(]\s*)rg ", lambda m: m.group(1) + RG + " ", cmd)
        p = subprocess.run(real, shell=True, cwd=LINUX, capture_output=True, timeout=timeout, stdin=subprocess.DEVNULL)
        out = p.stdout.decode("utf-8", "replace")
        rc = p.returncode
        if not out and p.stderr:
            out = p.stderr.decode("utf-8", "replace")[:500]
    except subprocess.TimeoutExpired:
        out, rc = "(timeout)", -1
    dt = time.time() - t0
    truncated = len(out) > HARNESS_CAP
    return {"cmd": cmd, "out": out[:HARNESS_CAP], "truncated": truncated, "secs": dt, "rc": rc}


def cf(args: str):
    return sh(f"{CF} --root {LINUX} {args}")


def q(s: str) -> str:
    return "'" + s.replace("'", "'\\''") + "'"


# ----------------------------------------------------------------------------- scoring

def collapse(s: str) -> str:
    return re.sub(r"\s+", " ", s)


def has_loc(text: str, file: str, line: int, tol: int = 2) -> bool:
    """The text shows `file` together with a line number within `tol` of `line`."""
    if file not in text:
        return False
    for m in re.finditer(re.escape(file) + r"[:\-](\d+)", text):
        if abs(int(m.group(1)) - line) <= tol:
            return True
    # one row per hit with the file and the line as separate columns (cscope, ctags, GNU Global
    # and graph tools print `file symbol line`, `name line file` or `file 534-563`)
    for row in text.splitlines():
        if file in row:
            for m in re.finditer(r"(?<![\w.])(\d+)(?![\w.])", row.replace(file, " ")):
                if abs(int(m.group(1)) - line) <= tol:
                    return True
    # grouped output: a file header line, then rows that start with a line number
    idx = text.find(file)
    while idx != -1:
        seg = text[idx: idx + 6000]
        for m in re.finditer(r"(?m)^\s*(\d+)[\t :\-]", seg):
            if abs(int(m.group(1)) - line) <= tol:
                return True
        idx = text.find(file, idx + 1)
    return False


def has_word(text: str, w: str) -> bool:
    return re.search(r"(?<![A-Za-z0-9_])" + re.escape(w) + r"(?![A-Za-z0-9_])", text) is not None


def score(qn, text: str):
    g, fam = qn["gold"], qn["family"]
    if fam == "F1":
        ok = has_loc(text, g["file"], g["line"])
        return float(ok), {"loc": ok}
    if fam == "F2":
        c = collapse(text)
        hits = [collapse(s) in c for s in g["must_contain"]]
        return sum(hits) / len(hits), {"found": sum(hits), "of": len(hits)}
    if fam == "F3":
        items = g["callers"]
        hits = [has_word(text, c["function"]) and c["file"] in text for c in items]
        return sum(hits) / len(hits), {"found": sum(hits), "of": len(hits)}
    if fam == "F4":
        items = g["callees"]
        hits = [has_word(text, c) for c in items]
        return (sum(hits) / len(hits) if items else 1.0), {"found": sum(hits), "of": len(hits)}
    if fam == "F5":
        fn = has_word(text, g["function"])
        tab = has_word(text, g["table"]) or has_loc(text, g["file"], g["line"])
        mem = has_word(text, g["member"])
        return float(fn and tab and mem), {"function": fn, "table_or_loc": tab, "member": mem}
    if fam == "F6":
        items = g["dispatch_sites"]
        hits = [has_word(text, c["function"]) and c["file"] in text for c in items]
        return sum(hits) / len(hits), {"found": sum(hits), "of": len(hits)}
    if fam == "F7":
        path = g["path"]
        hits = [has_word(text, f) for f in path]
        return sum(hits) / len(hits), {"found": sum(hits), "of": len(hits)}
    if fam == "F8":
        fn = has_word(text, g["function"]) or any(has_word(text, a) for a in g.get("also_accept_function", []) if isinstance(a, str))
        f = g["file"] in text
        return float(fn and f), {"function": fn, "file": f}
    if fam == "F9":
        items = g["writers"]
        hits = [has_word(text, c["function"]) and c["file"] in text for c in items]
        return sum(hits) / len(hits), {"found": sum(hits), "of": len(hits)}
    raise ValueError(fam)


# ----------------------------------------------------------------------------- helpers

def literal_fragment(fmt: str) -> str:
    """Longest literal run of a printf format: what survives in the log line."""
    parts = re.split(r"%[-+ #0-9.*]*(?:hh|h|ll|l|z|t|j|L)?[a-zA-Z](?:[A-Za-z0-9]*)?|\\n|\\t", fmt)
    parts = [p.strip() for p in parts if p.strip()]
    return max(parts, key=len) if parts else fmt


def def_regex(sym: str) -> str:
    s = re.escape(sym)
    return f"-e '^(struct|union|enum) {s}\\s*\\{{' -e '^#\\s*define\\s+{s}\\b' -e '^[A-Za-z_].*\\b{s}\\s*\\('"


def macro_fallback(sym: str):
    for prefix, pat in (("compat_sys_", r"COMPAT_SYSCALL_DEFINE\d\({}\b"), ("sys_", r"SYSCALL_DEFINE\d\({}\b"), ("dev_attr_", r"DEVICE_ATTR\w*\({}\b"), ("trace_", r"(TRACE_EVENT|DEFINE_EVENT)\(.*\b{}\b")):
        if sym.startswith(prefix):
            return "rg -n --no-heading " + q(pat.format(re.escape(sym[len(prefix):]))) + " -g '*.[ch]' ."
    return f"rg -n --no-heading -w {q(sym)} -g '*.[ch]' ."


def read_def(file: str, line: int, kind: str) -> str:
    # Read from the definition to its closing brace at column 0: exactly the definition.
    if kind == "sig":
        return f"sed -n '{line},{line + 11}p' {file}"
    return f"sed -n '{line},/^}}/p' {file}"


# ----------------------------------------------------------------------------- arms

def arm_codefast(qn, intents):
    fam, sym, g = qn["family"], qn["symbol"], qn["gold"]
    if fam == "F1":
        return [cf(f"find {q(sym)}")]  # where is it: one line per definition
    if fam == "F2":
        return [cf(f"search {q(sym)}")]  # signature / members: the definition itself
    if fam == "F3":
        return [cf(f"callers {q(sym)}")]
    if fam == "F4":
        return [cf(f"callees {q(sym)}")]
    if fam == "F5":
        if g["direction"] == "table->function":
            scope = intents.get(qn["id"], {}).get("scope", "")
            return [cf(f"grep {q(chr(92) + '.' + g['member'] + chr(92) + 's*=')} -p {q(scope)}")]
        return [cf(f"search {q(sym)}")]
    if fam == "F6":
        return [cf(f"callers {q(sym)}")]
    if fam == "F7":
        a, b = [s.strip() for s in sym.split("->")]
        return [cf(f"path {q(a)} {q(b)}")]
    if fam == "F8":
        return [cf(f"grep -F {q(literal_fragment(g['format']))}")]
    if fam == "F9":
        return [cf(f"refs {q(sym)} --only write")]
    raise ValueError(fam)


def arm_rg_expert(qn, intents):
    fam, sym, g = qn["family"], qn["symbol"], qn["gold"]
    calls = []
    if fam in ("F1", "F2", "F4"):
        c1 = sh(f"rg -n --no-heading {def_regex(sym)} -g '*.[ch]' .")
        calls.append(c1)
        if not has_loc(c1["out"], g["file"], g["line"]):
            calls.append(sh(macro_fallback(sym)))
        if fam == "F2":
            is_struct = bool(re.search(r"(?m)^(struct|union|enum) " + re.escape(sym), sh(f"sed -n '{g['line']}p' {g['file']}")["out"]))
            calls.append(sh(read_def(g["file"], g["line"], "body" if is_struct else "sig")))
        if fam == "F4":
            calls.append(sh(read_def(g["file"], g["line"], "body")))
        return calls
    if fam == "F3":
        return [sh(f"git grep -n -p -w {q(sym)} -- '*.c' '*.h'")]
    if fam == "F5":
        if g["direction"] == "table->function":
            scope = intents.get(qn["id"], {}).get("scope", ".")
            return [sh(f"git grep -n -p -E {q(chr(92) + '.' + g['member'] + '[[:space:]]*=')} -- {q(scope)}")]
        return [sh(f"git grep -n -p -w {q(sym)} -- '*.c' '*.h'")]
    if fam == "F6":
        member = g["slot"].split(".")[-1]
        return [sh(f"git grep -n -p -w {q(sym)} -- '*.c' '*.h'"), sh(f"git grep -n -p -e {q('->' + member + '(')} -- '*.c' '*.h'")]
    if fam == "F7":
        path = g["path"]
        alt = "|".join(re.escape(f) for f in path[:-1])
        c1 = sh(f"rg -n --no-heading {q('^[A-Za-z_].*' + chr(92) + 'b(' + alt + ')' + chr(92) + 's*' + chr(92) + '(')} -g '*.c' .")
        calls.append(c1)
        for h in g.get("hops", []):
            m = re.match(r"(.+):(\d+)$", h.get("site", ""))
            if not m:
                continue
            file, site = m.group(1), int(m.group(2))
            # read the calling function: from its definition line (found by the grep above) to its end
            dl = None
            for mm in re.finditer(r"(?m)^\./" + re.escape(file) + r":(\d+):.*\b" + re.escape(h["from"]) + r"\s*\(", c1["out"]):
                n = int(mm.group(1))
                if n <= site and (dl is None or n > dl):
                    dl = n
            if dl is None:
                dl = max(1, site - 40)
            calls.append(sh(read_def(file, dl, "body")))
        return calls
    if fam == "F8":
        return [sh(f"git grep -n -p -F {q(literal_fragment(g['format']))}")]
    if fam == "F9":
        field = g["field"]
        pat = f"(->|\\.){field}[[:space:]]*(=[^=]|\\+=|-=|\\|=|&=|\\^=|<<=|>>=|\\+\\+|--)"
        return [sh(f"git grep -n -p -E {q(pat)} -- '*.c' '*.h'")]
    raise ValueError(fam)


# Other tools, each asked through its own closest documented command (bench/scripts/competitors).
# `nl` marks the tools whose locate command takes a question in plain language.
QDIR = ROOT / "bench/scripts/competitors"
TOOLS = {"ripwire": True, "codebase-memory-mcp": True, "codegraph": True, "gortex": True, "semcode": False, "cscope": False, "global": False, "ctags": False, "tilth": False}


def tq(tool, kind, arg):
    return sh(f"{QDIR}/query_{tool}.sh {LINUX} {kind} {q(arg)}", timeout=300)


def arm_tool(tool):
    nl = TOOLS[tool]

    def arm(qn, intents):
        fam, sym, g = qn["family"], qn["symbol"], qn["gold"]
        if fam in ("F1", "F2"):
            return [tq(tool, "def", sym)]
        if fam in ("F3", "F6"):
            return [tq(tool, "callers", sym)]
        if fam == "F4":
            return [tq(tool, "callees", sym)]
        if fam == "F5":
            if g["direction"] == "table->function":
                # no tool has an ops-table query: ask in words, or search for the member name
                return [tq(tool, "locate", qn["question"] if nl else g["member"])]
            return [tq(tool, "callers", sym)]
        if fam == "F7":
            a, b = [s.strip() for s in sym.split("->")]
            return [tq(tool, "callees", a), tq(tool, "callers", b)]
        if fam == "F8":
            return [tq(tool, "locate", literal_fragment(g["format"]))]
        if fam == "F9":
            return [tq(tool, "locate", qn["question"] if nl else g["field"])]
        raise ValueError(fam)

    return arm


ARMS = {"codefast": arm_codefast, "rg-expert": arm_rg_expert}
ARMS.update({t: arm_tool(t) for t in TOOLS})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--questions", default=str(Path(__file__).parent / "questions.jsonl"))
    ap.add_argument("--arms", default="codefast,rg-expert")
    ap.add_argument("--out", default=str(ROOT / "bench/results/k3.jsonl"))
    ap.add_argument("--only", default="")
    a = ap.parse_args()
    intents = json.load(open(Path(__file__).parent / "intents.json"))
    qs = [json.loads(l) for l in open(a.questions)]
    if a.only:
        qs = [x for x in qs if x["id"].startswith(a.only)]
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    rows = []
    with open(a.out, "w") as fo, open(a.out + ".outputs", "w") as raw:
        for qn in qs:
            for arm in a.arms.split(","):
                calls = ARMS[arm](qn, intents)
                # exit status 3 from a tool wrapper: the tool has no such query
                unsupported = any(c.get("rc") == 3 for c in calls)
                text = "\n".join(c["out"] for c in calls if c.get("rc") != 3)
                s, detail = score(qn, text) if text.strip() else (0.0, {})
                row = {
                    "id": qn["id"], "family": qn["family"], "arm": arm, "score": round(s, 4), "detail": detail,
                    "calls": len(calls), "tokens": sum(ntok(c["out"]) for c in calls), "bytes": sum(len(c["out"]) for c in calls),
                    "cmd_tokens": sum(ntok(c["cmd"].replace(str(CF), "codefast").replace(f"--root {LINUX} ", "")) for c in calls),
                    "truncated": any(c["truncated"] for c in calls), "secs": round(sum(c["secs"] for c in calls), 3),
                    "cmds": [c["cmd"].replace(str(CF), "codefast").replace(f"--root {LINUX} ", "").replace(str(QDIR) + "/", "").replace(str(LINUX) + " ", "") for c in calls],
                    "unsupported": unsupported,
                }
                rows.append(row)
                fo.write(json.dumps(row) + "\n")
                fo.flush()
                raw.write(json.dumps({"id": qn["id"], "arm": arm, "calls": [{"cmd": c["cmd"], "rc": c.get("rc"), "out": c["out"]} for c in calls]}) + "\n")
                raw.flush()
                print(f"{row['id']} {arm:10} score={row['score']:.2f} calls={row['calls']} tokens={row['tokens']:6d} secs={row['secs']:.2f} {'TRUNC' if row['truncated'] else ''}", flush=True)
    # summary
    import statistics as st
    fams = sorted({r["family"] for r in rows})
    arms = a.arms.split(",")
    print("\nfamily   " + "".join(f"| {arm:>34} " for arm in arms))
    print("         " + "".join(f"| {'score':>7} {'tok(med)':>9} {'tok(mean)':>9} {'calls':>5} " for _ in arms))
    for fam in fams + ["ALL"]:
        line = f"{fam:8} "
        for arm in arms:
            rs = [r for r in rows if r["arm"] == arm and (fam == "ALL" or r["family"] == fam)]
            if not rs:
                continue
            line += f"| {st.mean(r['score'] for r in rs):7.3f} {st.median(r['tokens'] for r in rs):9.0f} {st.mean(r['tokens'] for r in rs):9.0f} {st.mean(r['calls'] for r in rs):5.2f} "
            if fam == "ALL" and any(r.get("unsupported") for r in rs):
                line += f"(no such query for {sum(1 for r in rs if r.get('unsupported'))}) "
        print(line)


if __name__ == "__main__":
    main()
