"""A stand-in for the model behind Codex. No model is called.

Codex is pointed at this server as its model provider (`wire_api = "responses"`). Every request
Codex sends is kept, so that what Codex really puts in front of a model can be read afterwards:
the text of the hooks, the definitions of the tools, the results of the calls. What the "model"
answers comes from a script, a list of turns, each a list of steps:

    {"shell": "grep -rn validate ."}                    a command in Codex's shell tool
    {"mcp": "search", "arguments": {"queries": [...]}}  a call to a tool of the codefast server
    {"patch": "*** Begin Patch\\n..."}                   an edit with Codex's patch tool
    {"js": "text(JSON.stringify(ALL_TOOLS.map(t => t.name)))"}   code for Codex's `exec` tool
    {"say": "text"}                                     a message, which ends the turn

A step is written the way the request offers the tool. Codex 0.164 gives its default model one
`exec` tool that runs JavaScript, with the shell, the patch tool and every MCP tool as
functions of a `tools` object; older model entries get them as separate tools. After the last
turn of the script every request is answered with a closing message.

`wiring.py` starts this in a thread. On its own:

    python3 -I bench/codex/mock_model.py --port 18431 --script SCRIPT.json --log DIR
"""
import argparse
import gzip
import http.server
import json
import pathlib
import threading


def offered(body):
    """Every tool of a request: name -> (its JSON, the namespace it is in or None).

    Codex 0.164 sends the tools of its default model as an `additional_tools` item of the
    input, grouped in namespaces, and those of other models in `tools`.
    """
    out = {}

    def walk(t, space=None):
        if not isinstance(t, dict):
            return
        if isinstance(t.get("tools"), list):
            for u in t["tools"]:
                walk(u, t.get("name"))
        elif t.get("name"):
            out[(space, t["name"])] = t

    for t in body.get("tools") or []:
        walk(t)
    for item in body.get("input") or []:
        if isinstance(item, dict) and item.get("type") == "additional_tools":
            for t in item.get("tools") or []:
                walk(t)
    return out


def render(step, body, call_id):
    """The output item for one step of the script, or None where the request has no such tool."""
    tools = offered(body)
    find = lambda name: next(((space, t) for (space, n), t in tools.items() if n == name), None)
    code_mode = any(n == "exec" and t.get("type") == "custom" for (_, n), t in tools.items())

    def custom(name, text):
        return {"type": "custom_tool_call", "call_id": call_id, "name": name, "input": text}

    def function(name, arguments, space=None):
        item = {"type": "function_call", "call_id": call_id, "name": name, "arguments": json.dumps(arguments)}
        if space and space != "functions":
            item["namespace"] = space
        return item

    if "say" in step:
        return {"type": "message", "role": "assistant", "id": "msg_" + call_id, "content": [{"type": "output_text", "text": step["say"]}]}
    if "js" in step:
        return custom("exec", step["js"]) if code_mode else None
    if "shell" in step:
        if code_mode:
            return custom("exec", f"const r = await tools.exec_command({json.dumps({'cmd': step['shell']})});\ntext(r.output);")
        hit = find("exec_command")
        return function("exec_command", {"cmd": step["shell"]}, hit[0]) if hit else None
    if "patch" in step:
        if code_mode:
            return custom("exec", f"const r = await tools.apply_patch({json.dumps(step['patch'])});\ntext(typeof r === 'string' ? r : JSON.stringify(r));")
        hit = find("apply_patch")
        if not hit:
            return None
        return custom("apply_patch", step["patch"]) if hit[1].get("type") == "custom" else function("apply_patch", {"input": step["patch"]}, hit[0])
    if "mcp" in step:
        if code_mode:
            name = "mcp__codefast__" + step["mcp"]
            return custom("exec", f"const r = await tools.{name}({json.dumps(step.get('arguments', {}))});\ntext(r.content.map(c => c.text).join(''));")
        hit = next(((space, t) for (space, n), t in tools.items() if n == step["mcp"] and space and "codefast" in space), None)
        return function(step["mcp"], step.get("arguments", {}), hit[0]) if hit else None
    raise ValueError(f"a step the stand-in does not know: {step}")


class StandIn(http.server.ThreadingHTTPServer):
    """Answers from `script`, keeps every request in `requests` (and in `log`, where one is given)."""

    daemon_threads = True

    def __init__(self, port, script, log=None):
        super().__init__(("127.0.0.1", port), Handler)
        self.script = script
        self.log = pathlib.Path(log) if log else None
        self.requests = []
        self.skipped = []
        self.turn = 0
        self.lock = threading.Lock()
        if self.log:
            self.log.mkdir(parents=True, exist_ok=True)

    def start(self):
        threading.Thread(target=self.serve_forever, daemon=True).start()
        return self


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format, *args):  # noqa: A002
        del format, args

    def standin(self) -> StandIn:
        assert isinstance(self.server, StandIn)
        return self.server

    def keep(self, body, size):
        srv = self.standin()
        with srv.lock:
            rec = {"n": len(srv.requests) + 1, "method": self.command, "path": self.path, "bytes": size, "body": body}
            srv.requests.append(rec)
        if srv.log:
            (srv.log / f"req-{rec['n']:03d}.json").write_text(json.dumps(rec, indent=1))
        return rec["n"]

    def send(self, code, payload, ctype="application/json"):
        self.send_response(code)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        self.keep(None, 0)
        self.send(200, json.dumps({"object": "list", "data": [], "models": []}).encode())

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("content-length") or 0))
        data = gzip.decompress(raw) if (self.headers.get("content-encoding") or "").lower() == "gzip" else raw
        try:
            body = json.loads(data)
        except ValueError:
            body = {"unparsed": data[:2000].decode("utf-8", "replace")}
        n = self.keep(body, len(raw))
        if not self.path.split("?")[0].endswith("/responses"):
            self.send(404, b"{}")
            return
        srv = self.standin()
        items = []
        while not items:
            with srv.lock:
                turn = srv.turn
                srv.turn += 1
            steps = srv.script[turn] if turn < len(srv.script) else [{"say": "That is all."}]
            for i, step in enumerate(steps):
                item = render(step, body, f"call_{n}_{i}")
                if item is None:
                    # The request does not offer the tool for this step: note it and go on with the next.
                    srv.skipped.append(step)
                else:
                    items.append(item)
        events = [{"type": "response.created", "response": {"id": f"resp_{n}"}}]
        events += [{"type": "response.output_item.done", "item": item} for item in items]
        usage = {"input_tokens": 0, "input_tokens_details": None, "output_tokens": 0, "output_tokens_details": None, "total_tokens": 0}
        events.append({"type": "response.completed", "response": {"id": f"resp_{n}", "usage": usage}})
        self.send(200, "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode(), "text/event-stream")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=18431)
    ap.add_argument("--script", required=True)
    ap.add_argument("--log", required=True)
    a = ap.parse_args()
    srv = StandIn(a.port, json.load(open(a.script)), a.log)
    print("ready", a.port, flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
