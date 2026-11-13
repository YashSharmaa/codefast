#!/usr/bin/env python3
"""recv.py FILE LINE VAR : show the enclosing function header and every line in that function
(up to LINE) that declares VAR, so the receiver's type can be read off. Manual-check helper."""
import sys, os, re
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import enc
path, line, var = sys.argv[1], int(sys.argv[2]), sys.argv[3]
e = enc.enclosing(path, line)
L = enc.lines_of(path)
print("%s:%d  %s" % (path, line, L[line-1].strip()))
print("   hdr@%d: %s" % (e[0], e[1][:200]))
for n in range(e[2], line):
    s = L[n-1]
    if re.search(r"\b(struct|const|unsigned|int|long|u\d+|\w+_t)\b[^;=()]*[\s*]%s\b\s*(=|;|,)" % re.escape(var), s):
        print("   decl@%d: %s" % (n, s.strip()))
