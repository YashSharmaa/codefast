#!/usr/bin/env python3
"""Helper for F9 (struct field writers). NOT an indexing tool under test.

  fieldw.py FIELD [rg-glob...]

Lists every textual write-like use of a member named FIELD in the tree:
  x->FIELD = / x.FIELD = / op= / ++ / -- / WRITE_ONCE(x->FIELD / designated initialiser .FIELD = /
  address-of (&x->FIELD) / xchg, cmpxchg, atomic*, set_bit etc. taking &x->FIELD
together with the enclosing function (brace-at-column-0 heuristic). The receiver type of each
hit was then checked by hand in the source; this script only produces the candidate list.
"""
import re, subprocess, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import enc
f = sys.argv[1]
globs = sys.argv[2:] or ["*.c", "*.h"]
pat = (r"(?:(?:->|\.)\s*%s\s*(?:=[^=]|\+=|-=|\|=|&=|\^=|<<=|>>=|\+\+|--)"
       r"|(?:\+\+|--)\s*[\w>.\-\[\]()]*(?:->|\.)%s\b"
       r"|&\s*[\w>.\-\[\]()]*(?:->|\.)%s\b"
       r"|WRITE_ONCE\(\s*[\w>.\-\[\]()]*(?:->|\.)%s\s*,)") % (f, f, f, f)
args = ["rg", "-n", "--no-heading", "-e", pat]
for g in globs:
    args += ["-g", g]
args += ["-g", "!tools", "-g", "!Documentation", "."]
out = subprocess.run(args, executable=enc.rg_bin(), cwd=enc.LINUX, capture_output=True, text=True,
                     stdin=subprocess.DEVNULL).stdout
for row in sorted(out.splitlines()):
    path, ln, text = row.split(":", 2)
    path = path[2:] if path.startswith("./") else path
    e = enc.enclosing(path, int(ln)) if path.endswith((".c", ".h")) else None
    print("%s:%s: [%s] %s" % (path, ln, enc.fname(e[1]) if e else "<file scope>", text.strip()[:150]))
