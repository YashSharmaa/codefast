#!/usr/bin/env python3
"""Turn the wrapper log into compile_commands.json (+ link_commands.json)."""
import json
import os
import sys

log, build_root, out = sys.argv[1], os.path.realpath(sys.argv[2]), sys.argv[3]
SRC_EXT = (".c", ".cc", ".cpp", ".cxx", ".m")
compile_cmds, link_cmds, seen = [], [], set()
for line in open(log):
    rec = json.loads(line)
    args, cwd = rec["arguments"], os.path.realpath(rec["directory"])
    srcs = [a for a in args[1:] if a.endswith(SRC_EXT) and not a.startswith("-")]
    if "-c" in args and srcs:
        for s in srcs:
            f = os.path.realpath(os.path.join(cwd, s))
            out_o = args[args.index("-o") + 1] if "-o" in args else os.path.splitext(os.path.basename(s))[0] + ".o"
            key = (f, tuple(args))
            if key in seen or os.path.basename(f) == "foo.c":  # foo.c: the Makefile's C11 probe
                continue
            seen.add(key)
            compile_cmds.append({"directory": cwd, "file": f, "arguments": args,
                                 "output": os.path.realpath(os.path.join(cwd, out_o))})
    elif "-o" in args and any(a.endswith((".o", ".a")) for a in args):
        link_cmds.append({"directory": cwd, "output": os.path.realpath(os.path.join(cwd, args[args.index("-o") + 1])),
                          "inputs": [os.path.realpath(os.path.join(cwd, a)) for a in args[1:] if a.endswith((".o", ".a"))]})
json.dump(compile_cmds, open(out, "w"), indent=1)
json.dump(link_cmds, open(os.path.join(os.path.dirname(out), "link_commands.json"), "w"), indent=1)
print(f"{len(compile_cmds)} compile commands, {len(link_cmds)} link commands -> {out}")
