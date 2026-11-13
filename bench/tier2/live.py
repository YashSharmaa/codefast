# /// script
# requires-python = ">=3.12"
# dependencies = ["tiktoken"]
# ///
"""Live agent experiment: real, headless Claude Code sessions with and without the codefast plugin.

Every run is one `claude -p` session in a private copy of the task's repository. The prompt is
the same in every arm: an arm differs only in what is installed (nothing, the plugin as a user
would have it, the plugin with its hooks switched off, ...). The session's own event stream
gives the cost: requests, tokens by kind, list-price dollars, every tool call and every hook
that fired. The outcome is checked afterwards in the run's copy (tests or compiler).

    live.py run   --tag T --arms control,plugin --model haiku [--dev | --d N --r N | --ids a,b] [--jobs 4]
    live.py verify --tag T                         check again the outcome of runs already made
    live.py score --tag T [--out FILE]
    live.py show  --tag T --id ID --arm ARM        one run, step by step

Tasks come from make_d_tasks.py (D: make failing tests pass), make_r_tasks.py (R: carry a
rename through its call sites, Go) and bench/k3 (K: a question about the Linux kernel, answered
in the shared kernel checkout and scored against the verified answer). Development tasks
(DEV_IDS) are never reported.

Environment: CLAUDE_BIN (default ~/.local/bin/claude), CLAUDE_CONFIG_DIR (which login to use,
required), CODEFAST_BIN (the binary under test, default target/release/codefast).
"""
import argparse, concurrent.futures, hashlib, json, os, re, shutil, subprocess, sys, time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
AB = ROOT / "target/ab"
LIVE = ROOT / "target/live"
SYFT = ROOT / "bench/repos/syft"
LINUX = ROOT / "bench/repos/linux"
LINUX_INDEX = ROOT / "target/bench-index/final-linux"
SYFT_INDEX = ROOT / "target/bench-index/live-syft"
DEV_IDS = {"django__django-18850", "tobymao__sqlglot-4519", "R-Collection-Add"}
W_READ, W_WRITE, W_OUT = 0.1, 1.25, 5.0
# Variables of the session this script may be started from: a child session must not inherit them.
SCRUB = [
    "CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN", "CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_STOP_HOOK_BLOCK_CAP", "CLAUDE_PID", "CLAUDE_EFFORT", "CLAUDE_CODE_EXECPATH",
]
# Left out of every arm: the web, other machines, and anything that outlives the session.
DENY = "WebSearch,WebFetch,CronCreate,CronDelete,CronList,RemoteTrigger,PushNotification,Workflow,DesignSync,EnterWorktree,ExitWorktree,SendMessage"
ARMS = {
    "control": dict(plugin=False),
    "plugin": dict(plugin=True),
    # The plugin without its MCP server: hooks, the skill and the command on PATH.
    "lean": dict(plugin=True, lean=True),
    "nohooks": dict(plugin=True, env={"CODEFAST_HOOKS": "off"}),
    "rules": dict(plugin=True, rules=True),
    # The ceiling of any tool that finds the place to change: the arm is told the file and the
    # function of the real fix, for nothing, by a hook of its own (see LIVE.md).
    "oracle": dict(plugin=False, oracle=True),
    # The same, and the code of that function with its callers and its tests: everything a
    # perfect push could hand over before the session starts.
    "oracle-context": dict(plugin=False, oracle=True, oracle_context=True),
    # How a prompt card gives the uses of a symbol (an experiment, see LIVE.md).
    "uses-count": dict(plugin=True, env={"CODEFAST_CARD_USES": "count"}),
    "uses-list": dict(plugin=True, env={"CODEFAST_CARD_USES": "list"}),
}
BUDGET = {"haiku": 2.5, "sonnet": 8.0, "opus": 20.0}


def load(name):
    p = AB / name / "tasks.json"
    return json.load(open(p)) if p.exists() else []


def file_hash(repo, files):
    h = hashlib.sha256()
    for f in sorted(files):
        p = Path(repo) / f
        h.update(f.encode())
        h.update(p.read_bytes() if p.exists() else b"<missing>")
    return h.hexdigest()


def k_tasks(key="kernel_questions_round2"):
    """The kernel questions of the pilot's second round, one of each family first."""
    ids = json.load(open(ROOT / "bench/tier2/tasks.json")).get(key, [])
    qs = {}
    for line in open(ROOT / "bench/k3/questions.verified.jsonl"):
        q = json.loads(line)
        qs[q["id"]] = q
    order = ids[0::2] + ids[1::2]
    return [dict(id="K-" + i, family="K", repo_name="torvalds/linux", question=qs[i]["question"], qid=i) for i in order]


def n_tasks():
    """Who-calls-this questions on syft (make_n_tasks.py), and sessions of four of them."""
    n = load("n")
    long = [dict(id=f"L-{i // 4 + 1}", family="L", repo_name="anchore/syft", parts=n[i : i + 4]) for i in range(0, len(n) - 3, 4)]
    return n, long


def held_out(name):
    """Tasks that no build was looked at on before it was final (`tasks.json`, key `held_out`)."""
    return json.load(open(ROOT / "bench/tier2/tasks.json")).get("held_out", {}).get(name, [])


def choose(a):
    d, r, i, e = load("d"), load("r") + load("r2"), load("i"), load("e")
    k = k_tasks() + k_tasks("kernel_questions_held_out")
    n, long = n_tasks()
    n2 = load("n2") + load("nz")
    # Sessions of four on the second repository, from its held-out caller questions.
    held_n = [t for t in n2 if t["id"] in held_out("N")]
    long2 = [dict(id=f"L-gogit-{j // 4 + 1}", family="L", repo_name="go-git/go-git", parts=held_n[j : j + 4]) for j in range(0, len(held_n) - 3, 4)]
    if a.ids or a.held:
        want = a.ids.split(",") if a.ids else [x for fam in a.held.split(",") for x in held_out(fam)]
        by = {t["id"]: t for t in d + r + k + n + long + i + n2 + e + long2}
        return [by[x] for x in want]
    order = lambda ts: sorted(ts, key=lambda t: hashlib.sha256(t["id"].encode()).hexdigest())
    if a.dev:
        return [t for t in order(d + r) if t["id"] in DEV_IDS]
    # Alternate the repositories of family D so that one of them does not fill the list.
    by = {}
    for t in order(d):
        by.setdefault(t["repo_name"], []).append(t)
    mixed, groups = [], list(by.values())
    while any(groups):
        for g in groups:
            if g:
                mixed.append(g.pop(0))
    return [t for t in mixed if t["id"] not in DEV_IDS][: a.d] + [t for t in order(r) if t["id"] not in DEV_IDS][: a.r] + k[: a.k] + n[: a.n] + long[: a.l]


ABOUT = {
    "anchore/syft": "syft, a Go project that lists the software packages inside container images and file systems",
    "go-git/go-git": "go-git, an implementation of Git written in Go",
    "colinhacks/zod": "zod, a TypeScript library for describing data and checking that it has the shape it should",
}


def repo_of(t):
    """The checkout a question about a Go project is asked in, and where its shared index is kept."""
    d = Path(t.get("repo_dir") or SYFT)
    return d, (SYFT_INDEX if d == SYFT else ROOT / "target/bench-index" / f"live-{d.name}")


def n_question(t):
    if not t["receiver"]:
        return (f"Which functions call the function {t['name']} defined in {t['file']}? Something else in this project is called {t['name']} too: "
                f"calls of that do not count. Calls from test code do count.")
    return (f"Which functions call the method {t['name']} of {t['receiver']} (defined in {t['file']})? Other types have a method called {t['name']} too: "
            f"calls of those do not count. Calls from test code do count.")


def prompt_for(t, repo):
    if t["family"] == "E":
        return f"""You are in a checkout of {ABOUT[t['repo_name']]}. It is in the current directory ({repo}).

A run of the program ended with this error:

    {t['message']}

Which function creates this error, and which functions call that function? Direct callers only. Callers in test code do not have to be listed.

Rules: do not change any file and do not read anything outside {repo}. End with the complete list: first the function that creates the error, then one line per calling function, each with the file it is in."""
    if t["family"] in ("N", "L"):
        if t["family"] == "N":
            ask = f"Answer this question about the code:\n\n{n_question(t)}"
            end = "End with the complete list: one line per calling function, with the file it is in."
        else:
            qs = "\n".join(f"{i + 1}. {n_question(p)}" for i, p in enumerate(t["parts"]))
            ask = f"Answer these {len(t['parts'])} questions about the code, one after the other:\n\n{qs}"
            end = "End with the complete lists: under a heading `## Question K` for each question, one line per calling function, with the file it is in."
        about = ABOUT[t["repo_name"]]
        return f"""You are in a checkout of {about}. It is in the current directory ({repo}).

{ask}

Rules: do not change any file and do not read anything outside {repo}. {end}"""
    if t["family"] == "K":
        return f"""You are in a checkout of the Linux kernel source, in the current directory ({repo}).

Answer this question about the code:

{t['question']}

Rules: do not change any file and do not read anything outside {repo}. Be exact and complete: name every function and every file path the question asks for, and give line numbers where they identify a place. End with the answer itself, not with an account of how you found it."""
    if t["family"] == "I":
        how = t["how"].replace("{repo}", str(repo))
        return f"""You are working in a checkout of the {t['repo_name']} project, in the current directory ({repo}).

This issue was reported against it:

--- issue ---
{t['issue'][:6000]}
--- end of issue ---

Fix it in the source code. The tests that the project added for this change are not in the tree: your fix is checked against them afterwards.

How tests of this project are run: `{how}`

Rules: work only inside {repo}. Do not read, list or run anything outside it, apart from the Python interpreter named in the command. Do not install anything. When you are done, say in one or two sentences what you changed."""
    if t["family"] == "D":
        cmd = t["test_cmd"].replace("{repo}", str(repo))
        return f"""You are working in a checkout of the {t['repo_name']} project, in the current directory ({repo}).

This issue was reported against it:

--- issue ---
{t['issue'][:6000]}
--- end of issue ---

The tests that check the fix are already in the tree, and they fail. Change the source code so that they pass.

Failing tests: {', '.join(t['f2p'][:8])}
How to run them: `{cmd}`

Rules: work only inside {repo}. Do not read, list or run anything outside it, apart from the Python interpreter named in the command. Do not edit these test files: {', '.join(t['test_files'])}. Do not install anything. Stop as soon as the command passes, and say in one or two sentences what you changed."""
    cmd = t["check_cmd"].replace("{repo}", str(repo))
    about = "syft, a tool that lists the software packages inside container images and file systems" if t["repo_name"] == "anchore/syft" else ABOUT[t["repo_name"]]
    return f"""You are working in a Go project: {about}. It is in the current directory ({repo}).

Rename the method {t['name']} of {t['receiver']} (defined in {t['file']}) to {t['new_name']}: the definition and every place that calls it, tests included. Other types have a method called {t['name']} too: those are different methods and must stay as they are.

How to check: `{cmd}` (the first run takes about a minute). It has to pass when you are done.

Rules: work only inside {repo}. Do not read, list or run anything outside it. Change nothing but the name of this one method and the calls of it. Stop as soon as the check passes, and say in one or two sentences what you changed."""


def sh(cmd, cwd=None, env=None, timeout=3600, inp=None):
    p = subprocess.run(cmd, cwd=cwd, env=env, shell=isinstance(cmd, str), capture_output=True, text=True, timeout=timeout, input=inp)
    return p.returncode, p.stdout + p.stderr


def plugin_dir(tag, lean=False):
    """The plugin as a harness receives it: the files a marketplace checkout would hold. The lean
    variant declares no MCP server."""
    dst = LIVE / tag / ("plugin-lean" if lean else "plugin")
    if dst.exists():
        shutil.rmtree(dst)
    dst.mkdir(parents=True)
    for part in (".claude-plugin", "hooks", "skills", "bin"):
        shutil.copytree(ROOT / part, dst / part, symlinks=True)
    if lean:
        manifest = dst / ".claude-plugin/plugin.json"
        m = json.load(open(manifest))
        m.pop("mcpServers", None)
        json.dump(m, open(manifest, "w"), indent=2)
    return dst


def rules_block(binary):
    scratch = LIVE / "rules-project"
    if scratch.exists():
        shutil.rmtree(scratch)
    scratch.mkdir(parents=True)
    subprocess.run(["git", "init", "-q", "."], cwd=scratch, check=True)
    subprocess.run([str(binary), "setup", "agents-md", "--apply"], capture_output=True, text=True, cwd=scratch)
    return (scratch / "AGENTS.md").read_text()


def prepare(t, arm, rep, a, binary):
    spec = ARMS[arm]
    run = LIVE / a.tag / t["id"] / (arm if rep == 0 else f"{arm}.{rep}")
    repo = run / "repo"
    if run.exists():
        shutil.rmtree(run)
    run.mkdir(parents=True)
    if t["family"] == "K":
        # Nothing is changed, so every run reads the one kernel checkout and its one index.
        row = dict(id=t["id"], family="K", arm=arm, rep=rep, repo=str(LINUX), run=str(run), repo_name=t["repo_name"], model=a.model, qid=t["qid"], tree_state=tree_state(LINUX))
        (run / "prompt.txt").write_text(prompt_for(t, LINUX))
        json.dump(row, open(run / "task.json", "w"), indent=1)
        return row
    if t["family"] in ("N", "L", "E"):
        parts = t["parts"] if t["family"] == "L" else [t]
        where, index = repo_of(parts[0])
        row = dict(id=t["id"], family=t["family"], arm=arm, rep=rep, repo=str(where), run=str(run), repo_name=t["repo_name"], model=a.model, tree_state=tree_state(where),
                   index=str(index), parts=[dict(gold=p["gold"], wrong=p["wrong"]) for p in parts])
        (run / "prompt.txt").write_text(prompt_for(t, where))
        json.dump(row, open(run / "task.json", "w"), indent=1)
        return row
    src = repo_of(t)[0] if t["family"] == "R" else t["base"]
    shutil.copytree(src, repo, symlinks=True, ignore=shutil.ignore_patterns(".git", ".codefast", "__pycache__"))
    if spec.get("rules"):
        with open(repo / "CLAUDE.md", "a") as f:
            f.write("\n" + rules_block(binary))
    if spec.get("oracle"):
        where = ", ".join(t.get("gold_files") or [])
        which = ", ".join(f.rsplit(":", 1)[-1] for f in (t.get("gold_functions") or []))
        card = f"codefast: the change this asks for belongs in {where}" + (f", in {which}." if which else ".")
        if spec.get("oracle_context"):
            # And what codefast knows about each of those functions: its body, what it calls,
            # who calls it and the tests that reach it.
            e = dict(os.environ, CODEFAST_INDEX_DIR=str(run / "oracle-index"))
            parts = []
            for g in (t.get("gold_functions") or [])[:2]:
                p = subprocess.run([str(binary), "--root", str(repo), "context", g.rsplit(":", 1)[-1], "--budget", "700"], capture_output=True, text=True, env=e)
                if p.returncode == 0 and p.stdout.lstrip().startswith("no symbol"):
                    continue
                if p.returncode == 0 and p.stdout.strip():
                    parts.append(p.stdout.strip())
            if parts:
                card += "\n\n" + "\n\n".join(parts)
        (repo / ".claude").mkdir(parents=True, exist_ok=True)
        hook = repo / ".claude/oracle.py"
        hook.write_text(
            "import json, sys\n"
            "sys.stdin.read()\n"
            f"card = {card!r}\n"
            "print(json.dumps({'hookSpecificOutput': {'hookEventName': 'UserPromptSubmit', 'additionalContext': card}}))\n"
        )
        json.dump(
            {"hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": f"python3 {hook}"}]}]}},
            open(repo / ".claude/settings.json", "w"),
            indent=1,
        )
    git = ["git", "-c", "user.name=bench", "-c", "user.email=bench@localhost", "-c", "commit.gpgsign=false"]
    subprocess.run(git + ["init", "-q", "."], cwd=repo, check=True)
    (repo / ".git/info/exclude").write_text(".codefast/\n")
    subprocess.run(git + ["add", "-A"], cwd=repo, check=True, capture_output=True)
    subprocess.run(git + ["commit", "-q", "-m", "base"], cwd=repo, check=True, capture_output=True)
    row = dict(id=t["id"], family=t["family"], arm=arm, rep=rep, repo=str(repo), run=str(run), repo_name=t["repo_name"], model=a.model)
    if t["family"] == "D":
        row.update(check=t["test_cmd"].replace("{repo}", str(repo)), test_files=t["test_files"], tests_hash=file_hash(repo, t["test_files"]))
    elif t["family"] == "I":
        row.update(check=t["test_cmd"].replace("{repo}", str(repo)), test_files=t["test_files"], tests_from=t["tests_from"], gold_files=t["gold_files"])
    else:
        row.update(check=t["check_cmd"].replace("{repo}", str(repo)), name=t["name"], new_name=t["new_name"], base=t["base"])
    if spec["plugin"]:
        # The steady state of a project that has the plugin: its index exists.
        e = dict(os.environ, CODEFAST_NO_REFRESH="")
        subprocess.run([str(binary), "--root", str(repo), "index"], capture_output=True, text=True, env=e)
    (run / "prompt.txt").write_text(prompt_for(t, repo))
    json.dump(row, open(run / "task.json", "w"), indent=1)
    return row


def tree_state(repo):
    """What git says differs in a work tree (a kernel checkout on a file system that ignores
    case is never clean, so the state is compared, not expected to be empty)."""
    rc, out = sh("git status --porcelain --untracked-files=no", cwd=repo)
    return hashlib.sha256(out.encode()).hexdigest()[:16]


def final_text(run):
    text = ""
    for e in events(Path(run) / "run.jsonl"):
        if e.get("type") == "result":
            text = e.get("result") or ""
    return text


def verify_k(row):
    """The answer's score against the verified one, from 0 to 1."""
    import importlib.util

    spec = importlib.util.spec_from_file_location("score_ab", ROOT / "bench/tier2/score_ab.py")
    mod = importlib.util.module_from_spec(spec)
    argv, sys.argv = sys.argv, [sys.argv[0]]
    try:
        spec.loader.exec_module(mod)
    finally:
        sys.argv = argv
    k3 = mod.load_k3()
    qn = next(json.loads(l) for l in open(ROOT / "bench/k3/questions.verified.jsonl") if json.loads(l)["id"] == row["qid"])
    return float(mod.score_k(k3, qn, final_text(row["run"])))


def named(text, who):
    """Whether an answer names a function together with its file."""
    base = who["file"].rsplit("/", 1)[-1]
    return bool(re.search(r"(?<![A-Za-z0-9_])" + re.escape(who["name"]) + r"(?![A-Za-z0-9_])", text)) and base in text


def score_callers(text, part):
    """F1 of an answer against the compiler's list of callers: found callers against all of
    them, and against the callers of other types' methods of the same name it also lists."""
    found = sum(1 for g in part["gold"] if named(text, g))
    wrong = sum(1 for w in part["wrong"] if named(text, w))
    if not found:
        return 0.0
    recall, precision = found / len(part["gold"]), found / (found + wrong)
    return 2 * recall * precision / (recall + precision)


def verify_n(row):
    text = final_text(row["run"])
    if row["family"] in ("N", "E"):
        return score_callers(text, row["parts"][0])
    # One section per question. An answer that has no headings is read as a whole for each.
    cuts = [m.start() for m in re.finditer(r"(?m)^#+\s*Question\s+\d+", text)]
    sections = [text[a:b] for a, b in zip(cuts, cuts[1:] + [len(text)])] if len(cuts) == len(row["parts"]) else [text] * len(row["parts"])
    return sum(score_callers(s, p) for s, p in zip(sections, row["parts"])) / len(row["parts"])


def verify(row):
    if row["family"] in ("N", "L", "E"):
        s = verify_n(row)
        row["kscore"] = s
        return s >= 0.9, row.get("tree_state") in (None, tree_state(row["repo"]))
    if row["family"] == "K":
        s = verify_k(row)
        row["kscore"] = s
        # The tree is shared: a run that changed it would spoil the others.
        return s >= 0.999, row.get("tree_state") in (None, tree_state(row["repo"]))
    if row["family"] == "I":
        # The tests of the real fix come into the tree now, over whatever the agent left there.
        for f in row["test_files"]:
            dst = Path(row["repo"]) / f
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(Path(row["tests_from"]) / f, dst)
    try:
        rc, _ = sh(row["check"], timeout=1800)
        ok = rc == 0
    except Exception:
        ok = False
    if row["family"] == "I":
        return ok, True
    if row["family"] == "D":
        return ok, file_hash(row["repo"], row["test_files"]) == row["tests_hash"]
    defs = lambda root: sorted(
        (str(p.relative_to(root)), m.group(0))
        for p in Path(root).rglob("*.go")
        for m in re.finditer(r"^func \([^)]*\) \w+", p.read_text(errors="replace"), re.M)
        if m.group(0).endswith((" " + row["name"], " " + row["new_name"]))
    )
    return ok, defs(row["repo"]) == defs(row["base"])


def session(row, a, binary, plug):
    spec = ARMS[row["arm"]]
    run = Path(row["run"])
    env = {k: v for k, v in os.environ.items() if k not in SCRUB}
    env["CLAUDE_CONFIG_DIR"] = a.config_dir
    env["CODEFAST_BIN"] = str(binary)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    env.pop("CODEFAST_HOOKS", None)
    env.pop("CODEFAST_INDEX_DIR", None)
    if row["family"] == "K" and spec["plugin"]:
        env["CODEFAST_INDEX_DIR"] = str(LINUX_INDEX)
    if row["family"] in ("N", "L", "E") and spec["plugin"]:
        env["CODEFAST_INDEX_DIR"] = row.get("index") or str(SYFT_INDEX)
    env.update(spec.get("env", {}))
    cmd = [
        a.claude, "-p", "--model", a.model, "--output-format", "stream-json", "--verbose", "--include-hook-events",
        "--no-session-persistence", "--setting-sources", "project", "--dangerously-skip-permissions",
        "--disallowedTools", DENY, "--max-turns", str(a.max_turns), "--max-budget-usd", str(a.budget or BUDGET[a.model]), "--no-chrome",
    ]
    # The account's cloud connectors (mail, documents) have no place in a benchmark run. Strict
    # mode would also drop the plugin's own server, so it is used only where no plugin is loaded.
    env["ENABLE_CLAUDEAI_MCP_SERVERS"] = "false"
    if spec["plugin"]:
        cmd += ["--plugin-dir", str(plug) + ("-lean" if spec.get("lean") else "")]
    else:
        cmd += ["--strict-mcp-config"]
    t0 = time.time()
    with open(run / "run.jsonl", "w") as out, open(run / "err.txt", "w") as err:
        try:
            p = subprocess.run(cmd, cwd=row["repo"], env=env, stdin=open(run / "prompt.txt"), stdout=out, stderr=err, timeout=a.timeout)
            rc = p.returncode
        except subprocess.TimeoutExpired:
            rc = 124
    secs = round(time.time() - t0)
    try:
        ok, clean = verify(row)
    except Exception as e:  # the session is on disk: `live.py verify` can check it again
        print(f"  check failed for {row['id']} {row['arm']}: {type(e).__name__}: {e}", flush=True)
        ok, clean = False, False
    res = dict(row, exit=rc, seconds=secs, passed=ok, clean=clean)
    json.dump(res, open(run / "outcome.json", "w"), indent=1)
    return res


def events(path):
    out = []
    for line in open(path, errors="replace"):
        line = line.strip()
        if line.startswith("{"):
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    return out


def is_codefast(cmd):
    return bool(re.search(r"(^|[\s;&|(`$])(\S*/)?codefast(\s|$)", cmd))


def parse(path):
    """What one session did: requests, tokens, tools, hooks."""
    ev = events(path)
    firsts = []  # (cache read, cache write, uncached input) of the session's first request
    sizes = []  # per request of the main thread: (tokens of prompt, tokens of output)
    early = 0  # characters hooks added before the first request
    r: dict[str, Any] = dict(requests=0, sub_requests=0, tools={}, cmds=[], cf_cli=0, cf_mcp=0, hooks={}, hook_bytes=0, result_chars={}, init={}, limits={}, final=None, usage={}, cost=0.0, turns=0, denials=0)
    seen, names = set(), {}
    for e in ev:
        t, s = e.get("type"), e.get("subtype")
        if t == "system" and s == "init":
            r["init"] = dict(tools=len(e.get("tools", [])), mcp=[m["name"] for m in e.get("mcp_servers", [])], plugins=[p["name"] for p in e.get("plugins", []) if not p["name"].startswith("cc-plugin-")],
                             skills=len(e.get("skills", [])), model=e.get("model"))
        elif t == "system" and s == "hook_response":
            out = e.get("output") or e.get("stdout") or ""
            try:
                ctx = json.loads(out).get("hookSpecificOutput", {}).get("additionalContext", "") if out.strip().startswith("{") else out
            except json.JSONDecodeError:
                ctx = out
            if ctx:
                k = e.get("hook_event", "?")
                r["hooks"][k] = r["hooks"].get(k, 0) + 1
                r["hook_bytes"] += len(ctx)
                if not sizes:
                    early += len(ctx)
        elif t == "rate_limit_event":
            info = e.get("rate_limit_info", {})
            for w, v in (info.get("unifiedWindows") or {}).items():
                r["limits"][w] = v.get("utilization")
            r["limits"]["status"] = info.get("status")
        elif t == "assistant":
            m = e["message"]
            if m.get("id") not in seen:
                seen.add(m.get("id"))
                r["requests"] += 1
                if e.get("parent_tool_use_id"):
                    r["sub_requests"] += 1
                else:
                    u = m.get("usage", {})
                    if not firsts:
                        firsts.append((u.get("cache_read_input_tokens", 0), u.get("cache_creation_input_tokens", 0), u.get("input_tokens", 0)))
                    sizes.append([u.get("cache_read_input_tokens", 0) + u.get("cache_creation_input_tokens", 0) + u.get("input_tokens", 0), u.get("output_tokens", 0)])
            elif sizes and not e.get("parent_tool_use_id"):
                # Later parts of the same reply: the output count grows, the prompt is the same.
                sizes[-1][1] = max(sizes[-1][1], m.get("usage", {}).get("output_tokens", 0))
            for c in m.get("content", []):
                if c.get("type") == "tool_use":
                    n = c["name"]
                    names[c["id"]] = n
                    r["tools"][n] = r["tools"].get(n, 0) + 1
                    if n == "Bash":
                        cmd = c["input"].get("command", "")
                        r["cmds"].append(cmd)
                        r["cf_cli"] += is_codefast(cmd)
                    elif n.startswith("mcp__") and "codefast" in n:
                        r["cf_mcp"] += 1
        elif t == "user":
            content = e["message"].get("content")
            for c in content if isinstance(content, list) else []:
                if isinstance(c, dict) and c.get("type") == "tool_result":
                    n = names.get(c.get("tool_use_id"), "?")
                    body = c.get("content")
                    size = len(body) if isinstance(body, str) else sum(len(x.get("text", "")) for x in body or [] if isinstance(x, dict))
                    r["result_chars"][n] = r["result_chars"].get(n, 0) + size
        elif t == "result":
            u = e.get("usage", {})
            r["usage"] = dict(input=u.get("input_tokens", 0), cache_read=u.get("cache_read_input_tokens", 0), cache_write=u.get("cache_creation_input_tokens", 0), output=u.get("output_tokens", 0))
            r["cost"] = e.get("total_cost_usd", 0.0)
            r["turns"] = e.get("num_turns", 0)
            r["final"] = e.get("subtype")
            r["denials"] = len(e.get("permission_denials") or [])
            # Sub-agents are billed on top of the main thread: modelUsage has the whole session.
            mu = e.get("modelUsage") or {}
            if mu:
                r["usage"] = dict(input=sum(v.get("inputTokens", 0) for v in mu.values()), cache_read=sum(v.get("cacheReadInputTokens", 0) for v in mu.values()),
                                  cache_write=sum(v.get("cacheCreationInputTokens", 0) for v in mu.values()), output=sum(v.get("outputTokens", 0) for v in mu.values()))
    r["first"] = firsts[0] if firsts else (0, 0, 0)
    r["sizes"] = sizes
    r["early_chars"] = early
    return r


def cleanup_projects(config_dir):
    """A session leaves an empty memory folder per working directory in the account's projects
    folder. Remove those of this script's runs, and only when they hold nothing else."""
    base = Path(config_dir) / "projects"
    prefix = str(LIVE).replace("/", "-")
    for d in base.glob(prefix + "-*"):
        files = [p for p in d.rglob("*") if p.is_file()]
        if not files:
            shutil.rmtree(d)


def cmd_run(a):
    assert a.config_dir, "set CLAUDE_CONFIG_DIR: which login the sessions use is never left to chance"
    arms = a.arms.split(",")
    for arm in arms:
        assert arm in ARMS, f"unknown arm {arm}"
    (LIVE / a.tag).mkdir(parents=True, exist_ok=True)
    binary = LIVE / a.tag / "codefast"
    if not binary.exists() or a.refresh_bin:
        shutil.copy2(a.codefast, binary)
    plug = plugin_dir(a.tag)
    plugin_dir(a.tag, lean=True)
    tasks = choose(a)
    todo = []
    # Repetitions are interleaved, so that a change in the account's speed or cache during the
    # batch falls on all arms alike.
    for rep in range(a.reps):
        for t in tasks:
            for arm in arms:
                done = LIVE / a.tag / t["id"] / (arm if rep == 0 else f"{arm}.{rep}") / "outcome.json"
                if a.resume and done.exists():
                    continue
                todo.append((t, arm, rep))
    print(f"{len(tasks)} tasks x {len(arms)} arms x {a.reps} repetitions, {len(todo)} runs to do, model {a.model}, {a.jobs} at a time", flush=True)
    if any(t["family"] == "K" for t, _, _ in todo) and any(ARMS[arm]["plugin"] for _, arm, _ in todo):
        e = dict(os.environ, CODEFAST_INDEX_DIR=str(LINUX_INDEX))
        subprocess.run([str(binary), "--root", str(LINUX), "status"], capture_output=True, text=True, env=e)
    if any(ARMS[arm]["plugin"] for _, arm, _ in todo):
        for where, index in {repo_of((t.get("parts") or [t])[0]) for t, _, _ in todo if t["family"] in ("N", "L", "E")}:
            e = dict(os.environ, CODEFAST_INDEX_DIR=str(index))
            subprocess.run([str(binary), "--root", str(where), "index"], capture_output=True, text=True, env=e)
    for where, check in {(repo_of(t)[0], t["check_cmd"]) for t, _, _ in todo if t["family"] == "R"}:
        rc, out = sh(check.replace("{repo}", str(where)), cwd=where)
        print(f"  compiler cache warmed for {where.name}" if rc == 0 else "  WARM FAILED: " + out[-300:], flush=True)
    rows = [prepare(t, arm, rep, a, binary) for t, arm, rep in todo]
    stop = False

    def one(row):
        if stop:
            return None
        return session(row, a, binary, plug)

    with concurrent.futures.ThreadPoolExecutor(a.jobs) as ex:
        for res in ex.map(one, rows):
            if res is None:
                continue
            r = parse(Path(res["run"]) / "run.jsonl")
            lim = r["limits"]
            worst = max([v for k, v in lim.items() if k != "status" and isinstance(v, (int, float))] or [0])
            print(f"  {res['id']:28} {res['arm']:8} exit={res['exit']} passed={res['passed']} clean={res['clean']} req={r['requests']:3d} ${r['cost']:.3f} {res['seconds']}s limits={lim}", flush=True)
            if lim.get("status") not in (None, "allowed") or worst >= a.limit_stop:
                print(f"  STOPPING: account usage window at {worst:.2f} (status {lim.get('status')})", flush=True)
                stop = True
    cleanup_projects(a.config_dir)


def cmd_verify(a):
    """Check the outcome of every finished session of a tag again (after a change to a checker,
    or when the batch stopped before it could)."""
    for task in sorted((LIVE / a.tag).glob("*/*/task.json")):
        row = json.load(open(task))
        run = task.parent
        row.setdefault("run", str(run))
        if not (run / "run.jsonl").exists() or (run / "outcome.json").exists() and not a.force:
            continue
        ok, clean = verify(row)
        old = json.load(open(run / "outcome.json")) if (run / "outcome.json").exists() else {}
        res = dict(row, exit=old.get("exit", 0), seconds=old.get("seconds", 0), passed=ok, clean=clean)
        json.dump(res, open(run / "outcome.json", "w"), indent=1)
        print(f"  {row['id']:28} {row['arm']:8} passed={ok} clean={clean}", flush=True)


def rows_of(tag):
    rows = []
    for out in sorted((LIVE / tag).glob("*/*/outcome.json")):
        res = json.load(open(out))
        r = parse(out.parent / "run.jsonl")
        u = r["usage"] or dict(input=0, cache_read=0, cache_write=0, output=0)
        row = dict(tag=tag, id=res["id"], family=res["family"], arm=res["arm"], model=res["model"], requests=r["requests"], sub_requests=r["sub_requests"], **u)
        row["processed"] = u["input"] + u["cache_read"] + u["cache_write"]
        row["weighted"] = round(u["input"] + W_READ * u["cache_read"] + W_WRITE * u["cache_write"] + W_OUT * u["output"])
        row["cost"] = r["cost"]
        row["first"] = r["first"]
        prompt = (out.parent / "prompt.txt").read_text() if (out.parent / "prompt.txt").exists() else ""
        row["warm"] = warm_cost(r["sizes"], len(prompt) + r["early_chars"], u["output"])
        row["tool_calls"] = sum(r["tools"].values())
        row["tools"] = r["tools"]
        row["cf_cli"], row["cf_mcp"] = r["cf_cli"], r["cf_mcp"]
        row["hooks"], row["hook_bytes"] = r["hooks"], r["hook_bytes"]
        row["read_chars"] = r["result_chars"].get("Read", 0)
        row["bash_chars"] = r["result_chars"].get("Bash", 0)
        row["result_chars"] = sum(r["result_chars"].values())
        row["final"], row["exit"], row["seconds"] = r["final"], res["exit"], res["seconds"]
        row["passed"], row["clean"] = res["passed"], res["clean"]
        row["score"] = res["kscore"] if "kscore" in res else (1.0 if res["passed"] and res["clean"] else 0.0)
        row["violations"] = (["control arm used the tool"] if res["arm"] == "control" and (r["cf_cli"] or r["cf_mcp"]) else []) + (["ended early: " + str(r["final"])] if r["final"] != "success" else [])
        rows.append(row)
    return rows


def warm_cost(sizes, new_chars, total_output):
    """Weighted tokens of a session priced from the size of each request alone, as if the cache
    always held what an earlier request had already sent: what the harness itself sends at the
    start of every session is read from the cache (it is there for anyone who started a session
    within the hour), what is new in a request is written once, and what a previous request of
    the session held is read. The billed figure (`weighted`) depends on what happened to be in
    the cache when the run started, which says when the run ran and nothing about the arm.

    `sizes` are (prompt tokens, output tokens) per request of the main thread, `new_chars` the
    characters of the task prompt and of what hooks added before the first request."""
    if not sizes:
        return 0
    new = min(sizes[0][0], round(new_chars / 3.0))
    cost = W_READ * (sizes[0][0] - new) + W_WRITE * new
    for prev, cur in zip(sizes, sizes[1:]):
        grown = max(0, cur[0] - prev[0])
        cost += W_READ * (cur[0] - grown) + W_WRITE * grown
    return round(cost + W_OUT * max(total_output, sum(s[1] for s in sizes)))


def cmd_score(a):
    rows = rows_of(a.tag)
    for r in rows if not a.quiet else []:
        print(f"{r['id']:28} {r['arm']:8} score={r['score']:.0f} req={r['requests']:3d} cost=${r['cost']:.3f} weighted={r['weighted']/1000:6.1f}k read={r['read_chars']/1000:5.0f}kc bash={r['bash_chars']/1000:5.0f}kc cf={r['cf_cli']}+{r['cf_mcp']} hooks={r['hooks'] or ''} {r['violations'] or ''}")
    if a.out:
        with open(a.out, "w") as fo:
            for r in rows:
                fo.write(json.dumps(r) + "\n")
    report(rows)


NUMERIC = ("requests", "sub_requests", "input", "cache_read", "cache_write", "output", "processed", "weighted", "warm", "cost", "tool_calls", "cf_cli", "cf_mcp", "hook_bytes",
           "read_chars", "bash_chars", "result_chars", "seconds", "score")


def per_task(rows):
    """One row per task and arm: the mean over its repetitions."""
    groups = {}
    for r in rows:
        groups.setdefault((r["id"], r["arm"]), []).append(r)
    out = []
    for (tid, arm), v in sorted(groups.items()):
        m = dict(v[0])
        for k in NUMERIC:
            m[k] = sum(x[k] for x in v) / len(v)
        m["hooks"] = {"n": sum(sum(x["hooks"].values()) for x in v) / len(v)}
        m["reps"] = len(v)
        m["used"] = sum(1 for x in v if x["cf_cli"] + x["cf_mcp"]) / len(v)
        out.append(m)
    return out


def report(rows):
    runs = len(rows)
    rows = per_task(rows)
    print(f"\n{runs} runs, {len(rows)} task and arm pairs (each the mean of its repetitions)")
    fams = sorted(set(r["family"] for r in rows))
    arms = sorted(set(r["arm"] for r in rows), key=lambda x: (x != "control", x))
    print()
    print(f"{'family':6} {'arm':8} {'n':>3} {'solved':>6} {'requests':>8} {'cost $':>7} {'weighted':>9} {'warm':>8} {'out tok':>8} {'result kc':>9} {'cf calls':>8} {'adopt':>6} {'hooks':>6}")
    for fam in fams + ["all"]:
        for arm in arms:
            v = [r for r in rows if (fam == "all" or r["family"] == fam) and r["arm"] == arm]
            if not v:
                continue
            n = len(v)
            m = lambda k: sum(x[k] for x in v) / n
            cf = lambda x: x["cf_cli"] + x["cf_mcp"]
            print(f"{fam:6} {arm:8} {n:3d} {m('score'):6.2f} {m('requests'):8.1f} {m('cost'):7.3f} {m('weighted')/1000:8.1f}k {m('warm')/1000:7.1f}k {m('output'):8.0f} {m('result_chars')/1000:9.1f} {sum(cf(x) for x in v)/n:8.1f} {m('used'):6.2f} {sum(sum(x['hooks'].values()) for x in v)/n:6.1f}")
    if "control" not in arms:
        return
    by = {}
    for r in rows:
        by.setdefault(r["id"], {})[r["arm"]] = r
    for arm in arms:
        if arm == "control":
            continue
        for name, keep in (("all pairs", lambda p: True), ("solved by both", lambda p: p["control"]["score"] == 1.0 and p[arm]["score"] == 1.0)):  # every repetition solved
            for fam in fams + ["all"]:
                pairs = [p for p in by.values() if "control" in p and arm in p and (fam == "all" or p["control"]["family"] == fam) and keep(p)]
                if not pairs:
                    continue
                tot = lambda x, k: sum(p[x][k] for p in pairs)
                ratio = lambda k: tot(arm, k) / max(tot("control", k), 1e-9)
                wins = sum(1 for p in pairs if p[arm]["cost"] < p["control"]["cost"])
                wins = sum(1 for p in pairs if p[arm]["warm"] < p["control"]["warm"])
                print(f"{arm:8} vs control  {name:15} {fam:4} n={len(pairs):2d}  warm = {ratio('warm'):.2f}  cost = {ratio('cost'):.2f}  requests = {ratio('requests'):.2f}  weighted = {ratio('weighted'):.2f}  "
                      f"output = {ratio('output'):.2f}  tool results = {ratio('result_chars'):.2f}  cheaper in {wins}/{len(pairs)}")


def cmd_show(a):
    run = LIVE / a.tag / a.id / a.arm  # ARM, or ARM.N for a repetition
    names = {}
    n = 0
    seen = set()
    for e in events(run / "run.jsonl"):
        t, s = e.get("type"), e.get("subtype")
        if t == "system" and s == "hook_response":
            out = e.get("output") or ""
            try:
                ctx = json.loads(out).get("hookSpecificOutput", {}).get("additionalContext", "") if out.strip().startswith("{") else out
            except json.JSONDecodeError:
                ctx = out
            if ctx:
                print(f"      HOOK {e.get('hook_name')}: {ctx[:a.width]!r}")
        elif t == "assistant":
            m = e["message"]
            if m.get("id") not in seen:
                seen.add(m.get("id"))
                n += 1
                u = m.get("usage", {})
                print(f"[{n}] in={u.get('input_tokens', 0)}+{u.get('cache_read_input_tokens', 0)}r+{u.get('cache_creation_input_tokens', 0)}w{' (sub-agent)' if e.get('parent_tool_use_id') else ''}")
            for c in m.get("content", []):
                if c.get("type") == "tool_use":
                    names[c["id"]] = c["name"]
                    print(f"    {c['name']}: {json.dumps(c['input'])[:a.width]}")
                elif c.get("type") == "text" and c["text"].strip():
                    print(f"    say: {c['text'].strip()[:a.width]!r}")
        elif t == "user":
            content = e["message"].get("content")
            for c in content if isinstance(content, list) else []:
                if isinstance(c, dict) and c.get("type") == "tool_result":
                    body = c.get("content")
                    text = body if isinstance(body, str) else " ".join(x.get("text", "") for x in body or [] if isinstance(x, dict))
                    print(f"      -> {names.get(c.get('tool_use_id'), '?')} {len(text)} chars: {text[:a.width // 2]!r}")
        elif t == "result":
            print(f"RESULT {e.get('subtype')} turns={e.get('num_turns')} cost=${e.get('total_cost_usd', 0):.3f}")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("run")
    p.add_argument("--tag", required=True)
    p.add_argument("--arms", default="control,plugin")
    p.add_argument("--model", default="haiku", choices=sorted(BUDGET))
    p.add_argument("--dev", action="store_true")
    p.add_argument("--ids", default="")
    p.add_argument("--held", default="", help="held-out sets of tasks.json, by name: K,D,I,N,R")
    p.add_argument("--d", type=int, default=16)
    p.add_argument("--r", type=int, default=8)
    p.add_argument("--k", type=int, default=0, help="kernel questions")
    p.add_argument("--n", type=int, default=0, help="who-calls-this questions on syft")
    p.add_argument("--l", type=int, default=0, help="sessions of four such questions")
    p.add_argument("--jobs", type=int, default=4)
    p.add_argument("--reps", type=int, default=1, help="runs per task and arm")
    p.add_argument("--resume", action="store_true", help="skip runs that already have an outcome")
    p.add_argument("--refresh-bin", action="store_true")
    p.add_argument("--max-turns", type=int, default=120)
    p.add_argument("--budget", type=float, default=0.0, help="dollar cap per run at list prices (default by model)")
    p.add_argument("--timeout", type=int, default=3600)
    p.add_argument("--limit-stop", type=float, default=0.6, help="stop the batch when an account usage window is this full")
    p.add_argument("--claude", default=os.environ.get("CLAUDE_BIN", str(Path.home() / ".local/bin/claude")))
    p.add_argument("--config-dir", default=os.environ.get("CLAUDE_CONFIG_DIR", ""))
    p.add_argument("--codefast", default=os.environ.get("CODEFAST_BIN", str(ROOT / "target/release/codefast")))
    p = sub.add_parser("verify")
    p.add_argument("--tag", required=True)
    p.add_argument("--force", action="store_true", help="also runs that have an outcome")
    p = sub.add_parser("score")
    p.add_argument("--tag", required=True)
    p.add_argument("--out", default="")
    p.add_argument("--quiet", action="store_true", help="only the summary tables")
    p = sub.add_parser("show")
    p.add_argument("--tag", required=True)
    p.add_argument("--id", required=True)
    p.add_argument("--arm", required=True)
    p.add_argument("--width", type=int, default=220)
    a = ap.parse_args()
    {"run": cmd_run, "verify": cmd_verify, "score": cmd_score, "show": cmd_show}[a.cmd](a)


if __name__ == "__main__":
    main()
