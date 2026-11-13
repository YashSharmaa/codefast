#!/usr/bin/env python3
"""Helper used while deriving gold answers (NOT an indexing tool under test).

  enc.py callers NAME [rg-glob...]   list every textual occurrence of NAME( in the tree
                                     with the enclosing function header found by a
                                     brace-at-column-0 heuristic (kernel coding style)
  enc.py at FILE LINE                print the enclosing function header of FILE:LINE
  enc.py body FILE LINE              print the body of the function whose header/brace
                                     encloses FILE:LINE, with line numbers
  enc.py calls FILE LINE             list identifier( tokens inside that function body

Every answer derived with this helper was afterwards re-read in the source by hand.
"""
import re, subprocess, sys, os
LINUX = os.environ.get("LINUX", os.path.join(os.path.dirname(os.path.abspath(__file__)), *([".."] * (3 if "/verifier/" in __file__ else 2)), "repos", "linux"))
_cache = {}
def lines_of(path):
    if path not in _cache:
        with open(os.path.join(LINUX, path), errors="replace") as f:
            _cache[path] = f.read().split("\n")
    return _cache[path]

def enclosing(path, line):
    """Return (header_line_no, header_text, open_brace_line, close_brace_line) or None."""
    L = lines_of(path)
    i = line - 1
    # find the opening brace at column 0 at or before the line
    j = i
    while j >= 0 and not L[j].startswith("{"):
        if L[j].startswith("}") and j != i:
            return None  # we are between functions
        j -= 1
    if j < 0:
        return None
    # closing brace
    k = j
    while k < len(L) and not L[k].startswith("}"):
        k += 1
    if k < i:
        return None
    # header: walk back over continuation lines to the first col-0 non-blank line
    h = j - 1
    while h > 0 and (L[h].startswith((" ", "\t")) or L[h] == ""):
        h -= 1
    hdr = " ".join(x.strip() for x in L[h:j])
    return (h + 1, hdr, j + 1, k + 1)

def fname(hdr):
    m = re.match(r"((?:COMPAT_)?SYSCALL_DEFINE\d|DEFINE_\w+|\w+_INITCALL|BPF_CALL_\d)\((\w+)", hdr)
    if m:
        return "%s(%s)" % (m.group(1), m.group(2))
    m = re.search(r"(\w+)\s*\(", hdr)
    return m.group(1) if m else hdr

def rg_bin():
    """ripgrep binary: $RG_BIN if set, else rg on PATH."""
    import shutil
    for c in (os.environ.get("RG_BIN"), shutil.which("rg")):
        if c and os.access(c, os.X_OK):
            return c
    raise SystemExit("no rg binary found; set RG_BIN")

def cmd_callers(name, globs):
    args = ["rg", "-n", "--no-heading", r"\b%s\s*\(" % re.escape(name)]
    for g in globs:
        args += ["-g", g]
    args.append(".")
    out = subprocess.run(args, executable=rg_bin(), cwd=LINUX, capture_output=True, text=True,
                         stdin=subprocess.DEVNULL).stdout
    for row in sorted(out.splitlines()):
        path, ln, text = row.split(":", 2)
        path = path[2:] if path.startswith("./") else path
        e = None
        if path.endswith((".c", ".h")):
            e = enclosing(path, int(ln))
        tag = fname(e[1]) if e else "<file scope>"
        print("%s:%s: [%s] %s" % (path, ln, tag, text.strip()))

def cmd_at(path, line):
    e = enclosing(path, line)
    print(e)

def cmd_body(path, line):
    e = enclosing(path, line)
    if not e:
        # maybe the line is the header itself: search forward for brace
        L = lines_of(path)
        j = line - 1
        while not L[j].startswith("{"):
            j += 1
        e = enclosing(path, j + 1)
    L = lines_of(path)
    for n in range(e[0], e[3] + 1):
        print("%d\t%s" % (n, L[n - 1]))
    return e

KW = {"if", "for", "while", "switch", "return", "sizeof", "typeof", "__typeof__", "defined", "do", "else", "case", "alignof", "_Alignof", "offsetof", "__builtin_expect"}
def cmd_calls(path, line):
    e = enclosing(path, line)
    if not e:
        L = lines_of(path)
        j = line - 1
        while not L[j].startswith("{"):
            j += 1
        e = enclosing(path, j + 1)
    L = lines_of(path)
    body = "\n".join(L[e[2]:e[3]])
    body = re.sub(r"/\*.*?\*/", " ", body, flags=re.S)
    body = re.sub(r"//[^\n]*", " ", body)
    body = re.sub(r'"(\\.|[^"\\])*"', '""', body)
    seen = []
    for m in re.finditer(r"((?:\w+(?:->|\.))*)(\w+)\s*\(", body):
        nm = m.group(2)
        if nm in KW:
            continue
        full = m.group(1) + nm
        if full not in seen:
            seen.append(full)
    print("%s:%d-%d %s" % (path, e[0], e[3], fname(e[1])))
    print("lines:", e[3] - e[0] + 1)
    for s in seen:
        print("  ", s)

if __name__ == "__main__":
    c = sys.argv[1]
    if c == "callers":
        cmd_callers(sys.argv[2], sys.argv[3:])
    elif c == "at":
        cmd_at(sys.argv[2], int(sys.argv[3]))
    elif c == "body":
        cmd_body(sys.argv[2], int(sys.argv[3]))
    elif c == "calls":
        cmd_calls(sys.argv[2], int(sys.argv[3]))
