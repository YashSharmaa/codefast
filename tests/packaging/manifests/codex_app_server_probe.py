#!/usr/bin/env python3
"""Ask a local Codex what it loaded from its plugins, without starting a model session.

Drives `codex app-server` over stdio (JSON-RPC, one message per line) and prints one JSON
object with the answers to hooks/list, skills/list, mcpServerStatus/list and, when a plugin
is named, plugin/read. Those requests only read configuration and start MCP servers; none of
them talks to a model.

    codex_app_server_probe.py CODEX_BINARY CWD [PLUGIN_NAME MARKETPLACE_ROOT]

The caller supplies the environment (CODEX_HOME and so on); run it under `env -i`.
"""

import json
import subprocess
import sys
import threading

codex, cwd = sys.argv[1], sys.argv[2]
plugin = sys.argv[3:5]
proc = subprocess.Popen([codex, "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=cwd, text=True)
threading.Thread(target=lambda: [None for _ in proc.stderr], daemon=True).start()


def send(message):
    proc.stdin.write(json.dumps(message) + "\n")
    proc.stdin.flush()


def call(request_id, method, params):
    send({"id": request_id, "method": method, "params": params})
    for line in proc.stdout:
        try:
            reply = json.loads(line)
        except ValueError:
            continue
        if reply.get("id") == request_id and ("result" in reply or "error" in reply):
            return reply.get("result", {"error": reply.get("error")})
    return {"error": "the app server closed its output"}


timer = threading.Timer(90, proc.kill)
timer.start()
out = {}
try:
    out["initialize"] = call(1, "initialize", {"clientInfo": {"name": "packaging-probe", "version": "0"}})
    send({"method": "initialized"})
    out["hooks"] = call(2, "hooks/list", {"cwds": [cwd]})
    out["skills"] = call(3, "skills/list", {"cwds": [cwd], "forceReload": True})
    out["mcp"] = call(4, "mcpServerStatus/list", {})
    if len(plugin) == 2:
        marketplace = plugin[1].rstrip("/") + "/.agents/plugins/marketplace.json"
        out["plugin"] = call(5, "plugin/read", {"pluginName": plugin[0], "marketplacePath": marketplace})
finally:
    timer.cancel()
    proc.kill()
json.dump(out, sys.stdout, indent=1)
print()
