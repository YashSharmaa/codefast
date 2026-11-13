"""Write one index-measurement JSON object. Called by measure() in _common.sh (run with python3 -I)."""
import json
import platform
import sys
import time

(out, tool, repo, version, idx, status, wall, rss, ib, db, user, sys_s, jobs, files, cmd, tree, extra, xenv) = sys.argv[1:19]


def num(x, f):
    try:
        return f(x)
    except Exception:
        return None


t_l, t_tree = num(rss, int), num(tree, int)
cands = [(v, n) for v, n in ((t_l, "time_l"), (t_tree, "tree_sampled")) if v is not None]
best = max(cands) if cands else (None, None)
d = {
    "tool": tool,
    "version": version,
    "repo": repo,
    "repo_git_files": num(files, int),
    "command": cmd.strip(),
    "extra_env": xenv,
    "threads_requested": jobs if jobs == "uncapped" else num(jobs, int),
    "wall_seconds": num(wall, float),
    "user_seconds": num(user, float),
    "sys_seconds": num(sys_s, float),
    "peak_rss_bytes": best[0],
    "peak_rss_source": best[1],
    "peak_rss_bytes_time_l": t_l,
    "peak_rss_bytes_tree_sampled": t_tree,
    "index_bytes": num(ib, int),
    "index_disk_bytes": num(db, int),
    "index_paths": [p for p in idx.split(",") if p],
    "exit_status": num(status, int),
    "host": platform.platform(),
    "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
}
if extra:
    try:
        d.update(json.loads(extra))
    except Exception as e:  # keep the measurement even if the extra blob is malformed
        d["extra_json_error"] = str(e)
with open(out, "w") as fh:
    json.dump(d, fh, indent=2)
    fh.write("\n")
