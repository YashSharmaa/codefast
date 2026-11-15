"""A recorded conversation of family S or G, replayed through the hooks of one build. No model.

The files of the conversation are rebuilt in a scratch directory from the recorded Write and
Edit calls (and from here documents that write a file), and every prompt, read, edit and shell
command is handed to `codefast hook` the way the harness handed it. What the hooks then say is
what that build would have put in front of the model in this conversation.

Recorded shell commands are never run. An edit whose file a shell command had changed cannot
be rebuilt and is counted as such. Nothing is written outside the scratch directory, which is
made new for every replay.

usage: replay_conversation.py BINARY RUN_DIR SCRATCH_DIR [--fresh] [--show]

  RUN_DIR      a run that `live.py` recorded, for example target/live/TAG/S-py-2/plugin
  SCRATCH_DIR  a directory to make. If it exists it must be an earlier replay's
  --fresh      every message in a session of its own, as the `-fresh` arms run them
  --show       print every note with the message it came in

Run it with two builds on the same run to see what a change to the hooks does to a whole
conversation before any session is paid for."""
import collections
import json
import os
import re
import shutil
import subprocess
import sys

MARK = ".replay-scratch"


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if len(args) != 3:
        sys.exit(__doc__)
    binary, run, scratch = (os.path.abspath(a.rstrip("/")) for a in args)
    fresh, show = "--fresh" in sys.argv, "--show" in sys.argv
    if scratch == run or scratch.startswith(run + os.sep) or run.startswith(scratch + os.sep):
        sys.exit("the scratch directory must lie apart from the recorded run")
    if os.path.exists(scratch):
        if not os.path.exists(os.path.join(scratch, MARK)):
            sys.exit(f"{scratch} exists and is not an earlier replay: name a directory to make")
        shutil.rmtree(scratch)
    repo, bin_dir = os.path.join(scratch, "repo"), os.path.join(scratch, "bin")
    os.makedirs(repo)
    os.makedirs(bin_dir)
    open(os.path.join(scratch, MARK), "w").close()
    subprocess.run(["git", "init", "-q", repo], check=True)
    # `codefast` on PATH, as it is for an agent's shell: the hooks then name the command so.
    os.symlink(binary, os.path.join(bin_dir, "codefast"))
    env = dict(os.environ, PATH=bin_dir + ":/usr/bin:/bin", CODEFAST_HOOK_BUDGET_MS="5000", CODEFAST_COST_NOTES="off", CLAUDECODE="1")
    for name in ("CODEFAST_INDEX_DIR", "CODEFAST_ROOT", "CODEFAST_HOOKS", "CODEFAST_NO_REFRESH"):
        env.pop(name, None)
    recorded = run + "/repo"
    said, size, lines = collections.Counter(), collections.Counter(), []
    count = collections.Counter()
    session = ["replay-1"]

    def local(path):
        """Where a recorded path lies in the scratch copy, or nothing for a path outside the
        recorded project: such a path is neither written nor read."""
        if path.startswith(recorded + "/"):
            out = repo + path[len(recorded):]
        elif path and not os.path.isabs(path):
            out = os.path.join(repo, path)
        else:
            return None
        out = os.path.normpath(out)
        return out if out.startswith(repo + "/") else None

    def hook(event, payload):
        payload = dict(payload, session_id=session[0], cwd=repo)
        r = subprocess.run([os.path.join(bin_dir, "codefast"), "hook", event], input=json.dumps(payload).encode(), capture_output=True, env=env, cwd=repo, timeout=60)
        out = r.stdout.decode("utf-8", "replace").strip()
        if not out:
            return ""
        try:
            return json.loads(out).get("hookSpecificOutput", {}).get("additionalContext", "")
        except ValueError:
            return out

    def kind_of(event, text):
        if event == "UserPromptSubmit":
            if "since the last edits" in text:
                return "for a prompt: a finding that was held"
            if "rename" in text and ("NEW_NAME" in text or "does this rename" in text or "renames is one command" in text):
                return "for a prompt: rename command"
            return "for a prompt: card"
        if event == "PreToolUse":
            return "outline hint before a read"
        if "nothing in this repository defines" in text:
            return "after an edit or a run: member that does not exist"
        if "is no longer defined" in text or "now takes" in text:
            return "after an edit: uses left behind"
        return "note after a search"

    def note(event, text, where):
        if not text:
            return
        kind = "start" if event == "SessionStart" else kind_of(event, text)
        said[kind] += 1
        size[kind] += len(text)
        lines.append((where, kind, " ".join(text.split())[:210]))

    def prompt(k):
        # In a session the index follows the edits in the background while the model works.
        # Here nothing waits, so it is brought up to date before each prompt.
        subprocess.run([os.path.join(bin_dir, "codefast"), "--root", repo, "index"], capture_output=True, env=env, cwd=repo, timeout=300)
        if fresh and k > 0:
            session[0] = f"replay-{k + 1}"
            note("SessionStart", hook("session-start", {"hook_event_name": "SessionStart", "source": "startup"}), "start")
        note("UserPromptSubmit", hook("user-prompt-submit", {"hook_event_name": "UserPromptSubmit", "prompt": prompts[k]}), f"message {k + 1}")

    prompts = json.load(open(run + "/prompts.json")) if os.path.exists(run + "/prompts.json") else [open(run + "/prompt.txt").read()]
    stage = 0
    note("SessionStart", hook("session-start", {"hook_event_name": "SessionStart", "source": "startup"}), "start")
    prompt(0)
    for line in open(run + "/run.jsonl", errors="replace"):
        try:
            e = json.loads(line)
        except ValueError:
            continue
        if e.get("type") == "bench":
            stage = e["stage"]
            if stage < len(prompts):
                prompt(stage)
            continue
        if e.get("type") != "assistant":
            continue
        for c in e["message"].get("content", []):
            if c.get("type") != "tool_use":
                continue
            name, i, where = c["name"], dict(c.get("input") or {}), f"message {stage + 1}"
            if name in ("Write", "Edit", "Read"):
                i["file_path"] = local(i.get("file_path") or "")
                if not i["file_path"]:
                    count["outside"] += 1
                    continue
            if name == "Read":
                note("PreToolUse", hook("pre-tool-use", {"hook_event_name": "PreToolUse", "tool_name": "Read", "tool_input": i}), where)
            elif name == "Write":
                os.makedirs(os.path.dirname(i["file_path"]), exist_ok=True)
                open(i["file_path"], "w").write(i.get("content") or "")
                count["rebuilt"] += 1
                note("PostToolUse", hook("post-tool-use", {"hook_event_name": "PostToolUse", "tool_name": "Write", "tool_input": i, "tool_response": {"filePath": i["file_path"]}}), where)
            elif name == "Edit":
                try:
                    text = open(i["file_path"]).read()
                except OSError:
                    text = None
                old, new = i.get("old_string") or "", i.get("new_string") or ""
                if text is None or not old or old not in text:
                    count["missed"] += 1
                    continue
                open(i["file_path"], "w").write(text.replace(old, new) if i.get("replace_all") else text.replace(old, new, 1))
                count["rebuilt"] += 1
                note("PostToolUse", hook("post-tool-use", {"hook_event_name": "PostToolUse", "tool_name": "Edit", "tool_input": i, "tool_response": {"filePath": i["file_path"]}}), where)
            elif name == "Bash":
                command = i.get("command") or ""
                # A here document that writes a file is applied as text. Nothing of the command is run.
                for m in re.finditer(r"cat\s*>\s*([^\s<>]+)\s*<<-?\s*(['\"]?)(\w+)\2\n(.*?)\n\3(?=\n|$)", command, re.S):
                    path = local(m.group(1).strip("'\""))
                    if not path or "$" in path:
                        continue
                    os.makedirs(os.path.dirname(path), exist_ok=True)
                    open(path, "w").write(m.group(4) + "\n")
                    count["rebuilt"] += 1
                command = command.replace(recorded, repo)
                note("PostToolUse", hook("post-tool-use", {"hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {"command": command}, "tool_response": {"stdout": "", "stderr": ""}}), where)
    task, arm = os.path.basename(os.path.dirname(run)), os.path.basename(run)
    print(f"{task} {arm}: {count['rebuilt']} writes and edits rebuilt, {count['missed']} edits not rebuilt (a shell command had changed their file), {count['outside']} calls outside the project left out")
    for kind, n in sorted(said.items()):
        if kind != "start":
            print(f"   {n:3} {size[kind]:6,} B  {kind}")
    print(f"   said to the model after the start: {sum(v for k, v in size.items() if k != 'start'):,} B in {sum(v for k, v in said.items() if k != 'start')} notes")
    if show:
        for where, kind, text in lines:
            if kind != "start":
                print(f"      [{where}] {kind}: {text}")


if __name__ == "__main__":
    main()
