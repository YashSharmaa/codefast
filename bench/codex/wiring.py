# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Does the plugin work inside Codex? Checked with the real `codex` binary and no model.

`codefast setup codex --hooks --apply` is run in a sandbox home, and `codex exec` is then run
there against a stand-in for the model (`mock_model.py`), which answers from a script and keeps
every request Codex sends. So what is checked is what a model behind Codex would have been
given: whether Codex fired each hook, whether the hook's text is in the next request, whether
the MCP tools and the command line answer inside Codex's sandbox, and how many bytes each part
of the plugin adds to a request. What a model would do with it is not measured here: that takes
a model, and `bench/tier2/live.py` measures it in Claude Code.

Nothing outside `target/codex-wiring` is written, your own `~/.codex` is not read, and no
request leaves the machine (the provider of the sandbox is `127.0.0.1`).

usage: python3 -I bench/codex/wiring.py [--codefast target/release/codefast] [--codex codex]
                                        [--model NAME] [--out bench/results/codex-wiring.json]
"""
import argparse
import datetime
import json
import os
import shutil
import socket
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from mock_model import StandIn, offered  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
WORK = ROOT / "target/codex-wiring"

STORE = """package store

import "errors"

// Store keeps values by key.
type Store struct {
	items map[string]string
	log   *Log
}

// Save checks the key, writes the value and records the write.
func (s *Store) Save(key, value string) error {
	if err := s.validate(key); err != nil {
		return err
	}
	s.write(key, value)
	s.log.Record("save", key)
	return nil
}

// validate rejects an empty key.
func (s *Store) validate(key string) error {
	if key == "" {
		return errors.New("empty key")
	}
	return nil
}

// write puts the value in place.
func (s *Store) write(key, value string) {
	s.items[key] = value
}
"""

LOG = """package store

// Log remembers what was done.
type Log struct {
	lines []string
}

// Record appends one line to the log.
func (l *Log) Record(op, key string) {
	l.lines = append(l.lines, op+" "+key)
}
"""

PATCH = """*** Begin Patch
*** Update File: store.go
@@
-func (s *Store) validate(key string) error {
+func (s *Store) check(key string) error {
*** End Patch
"""

ASK = "What does the function Store.Save call?"
RENAME = "Rename the method validate of Store to check."


def run(cmd, env, cwd=None, timeout=180):
    return subprocess.run([str(c) for c in cmd], env=env, cwd=cwd, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=timeout)


def make_repo(repo, env):
    repo.mkdir(parents=True)
    (repo / "store.go").write_text(STORE)
    (repo / "log.go").write_text(LOG)
    big = ["package store", ""]
    for i in range(150):
        big += [f"// step{i:03d} is one of many.", f"func step{i:03d}(x int) int {{", f"\treturn x + {i}", "}", ""]
    (repo / "big.go").write_text("\n".join(big))
    who = ["-c", "user.name=wiring", "-c", "user.email=wiring@example.invalid", "-c", "commit.gpgsign=false"]
    for cmd in (["git", "init", "-q", "."], ["git", "add", "."], ["git", *who, "commit", "-q", "-m", "a small repository for the check"]):
        p = run(cmd, env, cwd=repo)
        if p.returncode:
            sys.exit(f"{' '.join(cmd)}: {p.stderr}")


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def messages(body, role):
    """The texts of every message of one role in a request, with the place of each in the input."""
    out = []
    for i, item in enumerate(body.get("input") or []):
        if item.get("type") == "message" and item.get("role") == role:
            out += [(i, c.get("text") or "") for c in item.get("content") or [] if isinstance(c, dict)]
    return out


def tool_outputs(body):
    out = []
    for item in body.get("input") or []:
        if item.get("type") in ("custom_tool_call_output", "function_call_output"):
            o = item.get("output")
            out.append(o if isinstance(o, str) else "".join(c.get("text") or "" for c in o or [] if isinstance(c, dict)))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--codefast", default=os.environ.get("CODEFAST_BIN") or str(ROOT / "target/release/codefast"))
    ap.add_argument("--codex", default="codex")
    ap.add_argument("--model", help="a model name for Codex instead of its default: how tools are offered depends on it")
    ap.add_argument("--out")
    a = ap.parse_args()
    codex = shutil.which(a.codex)
    if not codex:
        sys.exit(f"{a.codex} is not on PATH")
    if not Path(a.codefast).is_file():
        sys.exit(f"{a.codefast}: no such file (cargo build --release, or --codefast PATH)")

    if WORK.exists():
        shutil.rmtree(WORK)
    home, bindir, repo = WORK / "home", WORK / "bin", WORK / "repo"
    (home / ".codex").mkdir(parents=True)
    bindir.mkdir()
    (WORK / "tmp").mkdir()
    shutil.copy2(a.codefast, bindir / "codefast")
    port = free_port()
    (home / ".codex/config.toml").write_text(f'model_provider = "standin"\n\n[model_providers.standin]\nname = "standin"\nbase_url = "http://127.0.0.1:{port}/v1"\nwire_api = "responses"\n')
    # Only what a shell would have, so that nothing of the caller's own harness reaches a hook.
    env = {"HOME": str(home), "CODEX_HOME": str(home / ".codex"), "PATH": f"{bindir}:{os.environ.get('PATH', '/usr/bin:/bin')}", "TMPDIR": str(WORK / "tmp"), "TERM": "dumb", "LANG": "en_US.UTF-8"}
    make_repo(repo, env)

    versions = {"codex": run([codex, "--version"], env).stdout.strip(), "codefast": run([bindir / "codefast", "--version"], env).stdout.strip()}
    setup = run([bindir / "codefast", "setup", "codex", "--hooks", "--apply"], env, cwd=repo)
    if setup.returncode:
        sys.exit(f"codefast setup codex: {setup.stdout}{setup.stderr}")

    def session(name, prompt, script, mode):
        srv = StandIn(port, script, WORK / "log" / name).start()
        try:
            cmd = [codex, "exec", "--json", "--skip-git-repo-check", "--dangerously-bypass-hook-trust", "-s", mode, "-C", repo]
            p = run(cmd + (["-m", a.model] if a.model else []) + [prompt], env)
        finally:
            srv.shutdown()
            srv.server_close()
        (WORK / "log" / name / "events.jsonl").write_text(p.stdout)
        (WORK / "log" / name / "stderr.txt").write_text(p.stderr)
        posts = [r["body"] for r in srv.requests if r["method"] == "POST" and r["body"] and r["path"].split("?")[0].endswith("/responses")]
        if p.returncode or not posts:
            sys.exit(f"codex exec ({name}) ended with status {p.returncode} after {len(posts)} requests:\n{p.stderr[-2000:]}")
        return posts, srv.skipped

    checks = []

    def check(what, ok, detail="", offered=True):
        if not offered:
            # Not a failure of the plugin: this model entry of Codex has no such tool to call.
            checks.append({"what": what, "ok": None, "detail": detail})
            print(f"  --  {what}\n      {detail}")
            return
        checks.append({"what": what, "ok": bool(ok)} | ({} if ok or not detail else {"detail": detail}))
        print(f"  {'ok' if ok else 'NO'}  {what}" + (f"\n      {detail}" if detail and not ok else ""))

    def note(body, *words):
        """The developer message of a request that holds every one of `words`, or ''."""
        return next((t for _, t in messages(body, "developer") if all(w in t for w in words)), "")

    print(f"{versions['codex']}, {versions['codefast']}, model {a.model or '(the default of this Codex)'}")

    # 1. A repository that has no index yet.
    print("A session in a repository that has no index:")
    first, _ = session("no-index", ASK, [[{"say": "Done."}]], "read-only")
    check("the start hook builds the index, though the session's sandbox is read-only", (repo / ".codefast/base.cfseg").is_file())
    check("and the prompt of that same session already gets its card", note(first[0], "what Store.Save calls"), "no card in the first request")

    # 2. A question, answered through the MCP server and through the command line.
    print("A question (read-only sandbox):")
    asked, skipped = session("ask", ASK, [[{"mcp": "search", "arguments": {"queries": ["Store.Save"]}}], [{"shell": "codefast callees Store.Save"}], [{"say": "Done."}]], "read-only")
    body = asked[0]
    start = note(body, "codefast has an index of this repository")
    check("SessionStart fires, and its line is in the first request", start, "no line about the index among the developer messages")
    card = note(body, "what Store.Save calls", "Store.validate", "Store.write", "Log.Record")
    check("UserPromptSubmit fires, and the card with the answer is in the first request", card, "no card naming the three callees")
    at = lambda text: next((i for i, t in messages(body, "developer") + messages(body, "user") if t == text), -1)
    check("the card comes after the prompt it belongs to", card and at(card) > at(ASK), f"prompt at {at(ASK)}, card at {at(card)}")
    listing = next((line for _, t in messages(body, "developer") if "<skills_instructions>" in t for line in t.splitlines() if line.startswith("- codefast:")), "")
    check("the skill is in the list of skills", listing, "no `- codefast:` line in the skills message")
    tools = offered(body)
    mcp_defs = {n: t for (space, n), t in tools.items() if "codefast" in (space or "") or "codefast" in n}
    code_mode = any(n == "exec" and t.get("type") == "custom" for (_, n), t in tools.items())
    outs = tool_outputs(asked[-1])
    if skipped:
        check("a tool of the MCP server answers (`search`)", None, "not offered directly: this model entry reaches the server's tools through its tool search", offered=False)
    else:
        check("a tool of the MCP server answers (`search`)", outs and "store.go" in outs[0] and "func (s *Store) Save" in outs[0], (outs or ["no tool output"])[0][:300])
    check("the command line answers inside the read-only sandbox (`codefast callees`)", outs and all(w in outs[-1] for w in ("Store.validate", "Store.write", "Log.Record")), (outs or ["no tool output"])[-1][:300])

    # 3. An edit, with the hooks around the tools.
    print("A rename (workspace-write sandbox):")
    edit, skipped = session("edit", RENAME, [[{"shell": "grep -rn Record ."}], [{"shell": "cat big.go"}], [{"patch": PATCH}], [{"say": "Done."}]], "workspace-write")
    check("UserPromptSubmit: the card names the rename command", note(edit[0], "codefast rename Store.validate"), "no rename card")
    # A name the card of this prompt has not already placed: what a session was told once is not said again.
    check("PostToolUse after a search for a name: where it is defined and who calls it", len(edit) > 1 and note(edit[1], "Record (log.go:", "Save"), "nothing after the grep")
    check("PreToolUse before a long file is printed whole: its length and the outline command", len(edit) > 2 and note(edit[2], "big.go has", "codefast show big.go"), "nothing before the cat")
    if skipped:
        check("PostToolUse after an edit: the use it left behind", None, "not offered: this model entry has no patch tool", offered=False)
    else:
        check("PostToolUse after an edit: the use it left behind", note(edit[-1], "is no longer defined in store.go", "Save"), "nothing after the patch")

    # 4. The command line under the profile of a read-only session, after a change the index has not seen.
    run(["git", "checkout", "-q", "--", "."], env, cwd=repo)
    run([bindir / "codefast", "status"], env, cwd=repo)  # outside the sandbox: the index is up to date again
    print("The command line under Codex's read-only profile:")
    (repo / "store.go").write_text(STORE + "\n// probe is new since the index was written.\nfunc (s *Store) probe() error {\n\treturn s.validate(\"\")\n}\n")
    before = sorted(f.name for f in (repo / ".codefast").iterdir())
    p = run([codex, "sandbox", "--permission-profile", ":read-only", "-C", repo, "--", bindir / "codefast", "callers", "Store.validate"], env, cwd=repo)
    check("it answers with the change in it", "probe" in p.stdout and "Save" in p.stdout, (p.stdout + p.stderr)[-300:])
    check("and wrote nothing into the index, which the profile forbids", sorted(f.name for f in (repo / ".codefast").iterdir()) == before)
    (repo / "store.go").write_text(STORE)

    sizes = {
        "skill_listing": len(listing.encode()),
        "session_line": len(start.encode()),
        "card_for_the_question": len(card.encode()),
        "mcp_tool_definitions": sum(len(json.dumps(t)) for t in mcp_defs.values()),
        "request_without_them": len(json.dumps(body)) - len(listing.encode()) - len(start.encode()) - len(card.encode()) - sum(len(json.dumps(t)) for t in mcp_defs.values()),
    }
    how = "listed in the request" if mcp_defs else ("not in the request: Codex gives this model one `exec` tool and the server's tools are functions it can call there (`tools.mcp__codefast__search`), found through `ALL_TOOLS`" if code_mode else "not in the request: this model entry finds them through its tool search")
    print("Bytes of a request that are the plugin's:")
    print(f"  {sizes['skill_listing']:6,d}  the skill's line in the list of skills (every request)")
    print(f"  {sizes['session_line']:6,d}  the line of the start hook (every request)")
    print(f"  {sizes['mcp_tool_definitions']:6,d}  the definitions of the five MCP tools ({how})")
    print(f"  {sizes['card_for_the_question']:6,d}  the card for this prompt (from the prompt on)")
    print(f"  {sizes['request_without_them']:6,d}  the rest of the first request, which is Codex's own")
    failed = [c for c in checks if c["ok"] is False]
    ran = [c for c in checks if c["ok"] is not None]
    print(f"{len(ran) - len(failed)} of {len(ran)} checks hold" + (f", {len(checks) - len(ran)} not offered to this model entry" if len(ran) < len(checks) else "") + (", the requests are under target/codex-wiring/log" if failed else ""))
    if a.out:
        result = {"date": datetime.date.today().isoformat(), "versions": versions, "model": asked[0].get("model"), "tools": how, "checks": checks, "bytes": sizes}
        Path(a.out).write_text(json.dumps(result, indent=1) + "\n")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
