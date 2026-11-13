#!/usr/bin/env python3
"""Shared machinery for building and verifying bench/k3/questions.jsonl.

Nothing here is an indexing tool: it only (a) runs the literal rg / sed commands recorded
as evidence, (b) reads source lines by number, and (c) finds the enclosing function of a
line with a brace-at-column-0 heuristic (tools/enc.py). Every gold answer was additionally
read by hand in the source while the set was being authored.
"""
import os, re, subprocess, sys, shutil

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import enc  # noqa: E402

LINUX = enc.LINUX
RG_BIN = enc.rg_bin()

# The evidence commands are plain `rg ...` / `sed ...` lines. They are run through a bash
# function that execs RG_BIN under the name rg, so that a ripgrep given by $RG_BIN is used
# even when another `rg` is on PATH.
# -j4 keeps the tree-wide searches polite on a shared machine; it does not change results.
_PREAMBLE = 'rg() { ( exec -a rg "$RG_BIN" -j4 "$@" ); }\n'


def run(cmd):
    env = dict(os.environ, RG_BIN=RG_BIN, LC_ALL="C")
    p = subprocess.run(["bash", "-c", _PREAMBLE + cmd], cwd=LINUX, env=env,
                       capture_output=True, text=True, errors="replace",
                       stdin=subprocess.DEVNULL)
    return p.stdout


def norm(s):
    return re.sub(r"\s+", " ", s).strip()


def src(path, line):
    return enc.lines_of(path)[line - 1]


def loc(path, line, expect=None):
    """'path:line: text' for a source line, asserting that it contains `expect`."""
    text = src(path, line)
    if expect is not None and norm(expect) not in norm(text):
        raise AssertionError("%s:%d does not contain %r: %r" % (path, line, expect, text))
    return "%s:%d: %s" % (path, line, norm(text))


def parse_loc(s):
    m = re.match(r"^([^:]+):(\d+): ?(.*)$", s)
    return m.group(1), int(m.group(2)), m.group(3)


def output_has(out, cmd, path, line):
    """True when the command output shows source line path:line."""
    text = norm(src(path, line))
    for o in out.split("\n"):
        if o.startswith("%s:%d:" % (path, line)) or o.startswith("%s-%d-" % (path, line)):
            return True
        # single-file rg prints 'line:text'
        if (o.startswith("%d:" % line) or o.startswith("%d-" % line)) and (" " + path) in (cmd + " "):
            if norm(o.split(":", 1)[1] if o.startswith("%d:" % line) else o.split("-", 1)[1]) == text:
                return True
    if cmd.lstrip().startswith("sed "):
        return text in [norm(o) for o in out.split("\n")]
    return False


def hits(out):
    """Parse 'path:line:text' rows of a multi-file rg -n output."""
    rows = []
    for o in out.split("\n"):
        m = re.match(r"^([^:\s][^:]*):(\d+):(.*)$", o)
        if m:
            rows.append((m.group(1), int(m.group(2)), m.group(3)))
    return rows


def enclosing_name(path, line):
    if not path.endswith((".c", ".h")):
        return None
    e = enc.enclosing(path, line)
    return enc.fname(e[1]) if e else None


def enclosing_header(path, line):
    e = enc.enclosing(path, line)
    return (e[0], e[1]) if e else None


def is_comment(text):
    t = text.strip()
    return t.startswith(("*", "/*", "//"))


def body_tokens(path, line):
    """identifier( tokens in the body of the function enclosing path:line (comments and
    string literals removed). Returns (header_line, open_brace_line, close_line, [tokens])."""
    e = enc.enclosing(path, line)
    L = enc.lines_of(path)
    body = "\n".join(L[e[2]:e[3]])
    body = re.sub(r"/\*.*?\*/", " ", body, flags=re.S)
    body = re.sub(r"//[^\n]*", " ", body)
    body = re.sub(r'"(\\.|[^"\\])*"', '""', body)
    seen = []
    for m in re.finditer(r"((?:\w+(?:->|\.))*)(\w+)\s*\(", body):
        if m.group(2) in enc.KW:
            continue
        full = m.group(1) + m.group(2)
        if full not in seen:
            seen.append(full)
    return e[0], e[2], e[3], seen


def field_write_cmd(field, paths=""):
    f = field
    cls = r"[\w>.\-\[\]()]*"
    return ("rg -n"
            " -e '(->|\\.)\\s*%s\\s*(=($|[^=])|\\+=|-=|\\|=|&=|\\^=|<<=|>>=|\\+\\+|--)'"
            " -e '(\\+\\+|--)\\s*%s(->|\\.)%s\\b'"
            " -e '&\\s*%s(->|\\.)%s\\b'"
            " -e '(WRITE_ONCE|rcu_assign_pointer)\\(\\s*%s(->|\\.)%s\\s*,'"
            " -g '*.[chS]' -g '!tools' -g '!Documentation'%s"
            % (f, cls, f, cls, f, cls, f, (" " + paths) if paths else ""))
