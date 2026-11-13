"""Read `gortex query symbol NAME --format json` on stdin; print the graph ids of callables named exactly NAME."""
import json
import sys

name = sys.argv[1]
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(0)
for r in data.get("results", []):
    if r.get("name") == name and r.get("kind") in ("function", "method", "constructor", "macro"):
        print(r.get("id", ""))
