"""Finish the gortex index JSON: add LSP facts and re-measure the store after the daemon stopped."""
import json
import sys

src, dst, lsp, started, copy, ib, db = sys.argv[1:8]
with open(src) as fh:
    d = json.load(fh)
d["index_bytes_daemon_running"] = d.get("index_bytes")
d["index_bytes"] = int(ib)
d["index_disk_bytes"] = int(db)
d.update({
    "lsp_mode": lsp,
    "lsp_servers_started": [s for s in started.split(",") if s],
    "indexed_copy": copy,
    "note": "wall_seconds is `gortex track --wait` against an already running daemon; "
            "index_bytes is the SQLite store after `gortex daemon stop`",
})
with open(dst, "w") as fh:
    json.dump(d, fh, indent=2)
    fh.write("\n")
