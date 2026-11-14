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

    live.py run   --tag T --arms control,plugin --model haiku [--dev | --d N --r N --w N | --ids a,b | --held W] [--jobs 4]
    live.py verify --tag T                         check again the outcome of runs already made
    live.py score --tag T [--out FILE]
    live.py show  --tag T --id ID --arm ARM        one run, step by step

Tasks come from make_d_tasks.py (D: make failing tests pass), make_i_tasks.py (I: the same
issues without their tests), make_r_tasks.py (R: carry a rename through its call sites, Go),
make_n_tasks.py (N and L: who calls this), make_e_tasks.py (E: where does this error come
from), make_w_tasks.py (W: write a function whose body was taken out) and bench/k3 (K: a
question about the Linux kernel, answered in the shared kernel checkout and scored against the
verified answer). Development tasks (DEV_IDS) are never reported.

Environment: CLAUDE_BIN (default ~/.local/bin/claude), CLAUDE_CONFIG_DIR (which login to use,
required), CODEFAST_BIN (the binary under test, default target/release/codefast).
"""
import argparse, concurrent.futures, hashlib, json, os, re, shutil, subprocess, sys, threading, time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
AB = ROOT / "target/ab"
LIVE = ROOT / "target/live"
SYFT = ROOT / "bench/repos/syft"
LINUX = ROOT / "bench/repos/linux"
NEW = ROOT / "bench/tier2/new"
STAGED = Path(os.environ.get("BENCH_STAGED_DIR") or ROOT / "bench/tier2/staged")
# The indexes of the shared trees. An index belongs to the version of codefast that wrote it,
# and a build that finds another version's index rebuilds it. Two builds that are measured
# side by side therefore each get directories of their own: BENCH_INDEX_SUFFIX names them.
INDEX_SUFFIX = os.environ.get("BENCH_INDEX_SUFFIX", "")
LINUX_INDEX = ROOT / ("target/bench-index/final-linux" + INDEX_SUFFIX)
SYFT_INDEX = ROOT / ("target/bench-index/live-syft" + INDEX_SUFFIX)
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
    # The condition of the treatment arms of the published kernel comparison (arXiv 2611.07286):
    # the tools are there and a sentence of the system prompt says that they must be used for
    # searching, and grep must not. The sentence is the paper's, with the tools named.
    "steer": dict(plugin=True, steer=True),
    # The ceiling of any tool that finds the place to change: the arm is told the file and the
    # function of the real fix, for nothing, by a hook of its own (see LIVE.md).
    "oracle": dict(plugin=False, oracle=True),
    # The same, and the code of that function with its callers and its tests: everything a
    # perfect push could hand over before the session starts.
    "oracle-context": dict(plugin=False, oracle=True, oracle_context=True),
    # A staged task with every message in a session of its own: what starting fresh for each
    # job costs, next to one conversation that carries all of them.
    "control-fresh": dict(plugin=False, fresh=True),
    "plugin-fresh": dict(plugin=True, fresh=True),
    # And with the list of what the uncommitted work defines, at the start of each session
    # (an experiment, see LIVE.md).
    "plugin-fresh-recap": dict(plugin=True, fresh=True, env={"CODEFAST_START_RECAP": "1"}),
    # A conversation with a smaller window for automatic compaction. Claude Haiku 5.5 costs five
    # times as much a token in a request whose prompt is over 100,000 tokens. The harness
    # compacts 33,000 tokens under the window: at 67,000 with the smallest window it takes
    # (100,000), and at 92,000 with a window of 125,000.
    "control-ac": dict(plugin=False, env={"CLAUDE_CODE_AUTO_COMPACT_WINDOW": "100000"}),
    "plugin-ac": dict(plugin=True, env={"CLAUDE_CODE_AUTO_COMPACT_WINDOW": "100000"}),
    "control-ac125": dict(plugin=False, env={"CLAUDE_CODE_AUTO_COMPACT_WINDOW": "125000"}),
    "plugin-ac125": dict(plugin=True, env={"CLAUDE_CODE_AUTO_COMPACT_WINDOW": "125000"}),
    # How a prompt card gives the uses of a symbol (an experiment, see LIVE.md).
    "uses-count": dict(plugin=True, env={"CODEFAST_CARD_USES": "count"}),
    "uses-list": dict(plugin=True, env={"CODEFAST_CARD_USES": "list"}),
}
# The last one is the model of the published comparison on LinuxFLBench (arXiv 2611.07286), with
# the dollar cap of its harness.
BUDGET = {"haiku": 2.5, "sonnet": 8.0, "opus": 20.0, "claude-haiku-4-5-20251001": 1.0}


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


def g_tasks():
    """Projects to create from nothing (`bench/tier2/new`): each is a specification, hidden tests
    written against the interface it gives, and a reference implementation that shows those
    tests can be passed. A session gets the specification and an empty directory."""
    out = []
    for p in sorted(NEW.glob("*/task.json")):
        t = json.load(open(p))
        t.update(dir=str(p.parent), repo_name="a new project")
        out.append(t)
    return out


def s_tasks():
    """A project built and then changed in one conversation (`bench/tier2/staged`): the first
    message asks for the project, each later one for a change to it. The hidden tests are of
    the state after the last message, with a set for every state on the way."""
    out = []
    for p in sorted(STAGED.glob("*/task.json")):
        t = json.load(open(p))
        t.update(dir=str(p.parent), repo_name="a new project", stage_files=[str(x) for x in sorted((p.parent / "stages").glob("*.md"))])
        out.append(t)
    return out


def choose(a):
    d, r, i, e, w, c, f = load("d"), load("r") + load("r2"), load("i"), load("e"), load("w") + load("w2"), load("c"), load("f") + load("fdev") + load("f2")
    g = g_tasks() + s_tasks()
    k = k_tasks() + k_tasks("kernel_questions_held_out") + k_tasks("kernel_questions_held_out_2")
    n, long = n_tasks()
    n2 = load("n2") + load("nz")
    # Sessions of four on the second repository, from its held-out caller questions.
    held_n = [t for t in n2 if t["id"] in held_out("N")]
    long2 = [dict(id=f"L-gogit-{j // 4 + 1}", family="L", repo_name="go-git/go-git", parts=held_n[j : j + 4]) for j in range(0, len(held_n) - 3, 4)]
    if a.ids or a.held:
        # The new projects carry their split themselves: G is the held-out half, Gdev the other.
        split = {fam + suffix: [t["id"] for t in g if t["family"] == fam and t["split"] == which] for fam in ("G", "S") for suffix, which in (("", "held"), ("dev", "dev"))}
        want = a.ids.split(",") if a.ids else [x for fam in a.held.split(",") for x in (split[fam] if fam in split else held_out(fam))]
        by = {t["id"]: t for t in d + r + k + n + long + i + n2 + e + long2 + w + c + f + g}
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
    return [t for t in mixed if t["id"] not in DEV_IDS][: a.d] + [t for t in order(r) if t["id"] not in DEV_IDS][: a.r] + k[: a.k] + n[: a.n] + long[: a.l] + w[: a.w]


ABOUT = {
    "anchore/syft": "syft, a Go project that lists the software packages inside container images and file systems",
    "go-git/go-git": "go-git, an implementation of Git written in Go",
    "colinhacks/zod": "zod, a TypeScript library for describing data and checking that it has the shape it should",
    "zricethezav/gitleaks": "gitleaks, a Go tool that finds secrets committed to a git repository",
}


def repo_of(t):
    """The checkout a question about a Go project is asked in, and where its shared index is kept."""
    d = Path(t.get("repo_dir") or SYFT)
    return d, (SYFT_INDEX if d == SYFT else ROOT / "target/bench-index" / f"live-{d.name}{INDEX_SUFFIX}")


def n_question(t):
    if not t["receiver"]:
        return (f"Which functions call the function {t['name']} defined in {t['file']}? Something else in this project is called {t['name']} too: "
                f"calls of that do not count. Calls from test code do count.")
    return (f"Which functions call the method {t['name']} of {t['receiver']} (defined in {t['file']})? Other types have a method called {t['name']} too: "
            f"calls of those do not count. Calls from test code do count.")


def stage_prompts(t, repo):
    """The messages of a staged task, as a user would send them one after the other."""
    first = prompt_for(dict(t, family="G"), repo)
    later = [
        f"""Next change to the project.

{Path(f).read_text().strip()}

Keep everything that this does not change. Update your own tests and run them until they pass. When you are done, say in one or two sentences what you changed."""
        for f in t["stage_files"][1:]
    ]
    return [first] + later


def prompt_for(t, repo):
    if t["family"] == "S":
        return stage_prompts(t, repo)[0]
    if t["family"] == "G":
        return f"""You are in a new, empty project directory ({repo}). The only file in it is SPEC.md.

Build the project that SPEC.md describes: write the code, write tests of your own, and run them until they pass.

When you are done, the project is checked by other tests, written against exactly the interface SPEC.md gives.

Rules: work only inside {repo}. Do not read, list or run anything outside it, apart from the language tools the specification names. Do not install anything and do not use the network. When you are done, say in two or three sentences what you built."""
    if t["family"] == "E":
        return f"""You are in a checkout of {ABOUT[t['repo_name']]}. It is in the current directory ({repo}).

A run of the program ended with this error:

    {t['message']}

Which function creates this error, and which functions call that function? Direct callers only. Callers in test code do not have to be listed.

Rules: do not change any file and do not read anything outside {repo}. End with the complete list: first the function that creates the error, then one line per calling function, each with the file it is in."""
    if t["family"] == "F":
        # The kernel wording is that of the published harness. Another project gets the same
        # sentences with its own name in them.
        kernel = t["repo_name"] == "torvalds/linux"
        what = f"the Linux kernel source at version {t['version']}" if kernel else f"{t['repo_name']} at commit {t['version']}"
        return f"""You are in a checkout of {what}, in the current directory ({repo}).

This bug was reported against that version:

--- bug report ---
{t['report']}
--- end of bug report ---

Find the source files that would have to be changed to fix it. Do not fix it.

Rules: do not change any file and do not read anything outside {repo}. End with a JSON object of this form and nothing after it:
{{"files": ["up to 10 file paths relative to the {'kernel' if kernel else 'repository'} root, the most likely first"], "functions": ["up to 10 function names, the most likely first"]}}"""
    if t["family"] == "C":
        which = f"the method {t['name']} of {t['receiver']}" if t["receiver"] else f"the function {t['name']}"
        return f"""You are in a checkout of {ABOUT[t['repo_name']]}. It is in the current directory ({repo}).

Answer this question about the code:

Which functions of this repository does {which} (defined in {t['file']}) call directly? Calls into the Go standard library and into other modules do not have to be listed.

Rules: do not change any file and do not read anything outside {repo}. End with the complete list: one line per called function, with the file it is defined in."""
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
    if t["family"] == "W":
        cmd = t["check_cmd"].replace("{repo}", str(repo))
        which = (t["owner"] + "." if t["owner"] else "") + t["name"]
        return f"""You are working in a Go project: {ABOUT[t['repo_name']]}. It is in the current directory ({repo}).

The body of {which}, in {t['file']}, has been taken out and replaced with `panic("not implemented")`. Write the body again, so that it does what the rest of the project expects of it.

How to check: `{cmd}` (the first run takes about a minute). It has to pass when you are done.

Rules: work only inside {repo}. Do not read, list or run anything outside it. Do not install anything. Change nothing but the body of this one function, and change no test file and no file under a testdata directory. Stop as soon as the check passes, and say in one or two sentences what you wrote."""
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


STEER = (
    "This repository is indexed by codefast. Its MCP tools (search, show, refs, path, grep) answer questions about the code: `search` takes a symbol, a path or "
    "free text such as a bug report, `show` prints one definition or the outline of a file, `refs` lists the uses of a symbol, `path` finds how one function "
    "reaches another, `grep` searches text and says which function each hit is in. For searching and navigating code, you MUST use these MCP tools. Do NOT use "
    "the built-in Grep or Glob tools, or shell search commands (grep, rg, ag, find -name), to search code."
)
_RULES = {}


def rules_block(binary):
    """The paragraph `codefast setup agents-md` writes, made once per binary: two sessions
    that start together must not both rebuild the scratch project it is read from."""
    if str(binary) not in _RULES:
        _RULES[str(binary)] = _rules_block(binary)
    return _RULES[str(binary)]


def _rules_block(binary):
    scratch = LIVE / "rules-project"
    if scratch.exists():
        shutil.rmtree(scratch)
    scratch.mkdir(parents=True)
    subprocess.run(["git", "init", "-q", "."], cwd=scratch, check=True)
    subprocess.run([str(binary), "setup", "agents-md", "--apply"], capture_output=True, text=True, cwd=scratch)
    return (scratch / "AGENTS.md").read_text()


def f_index(tree):
    """Where the index of one kernel version is kept."""
    return ROOT / "target/bench-index" / f"lfl-{Path(tree).name}{INDEX_SUFFIX}"


def ranked_files(text, tree):
    """The files a session ended with: the `files` list of the last JSON object of its answer,
    as paths relative to the kernel root."""
    best = None
    for m in re.finditer(r"\{", text):
        depth = 0
        for j in range(m.start(), len(text)):
            if text[j] == "{":
                depth += 1
            elif text[j] == "}":
                depth -= 1
                if depth == 0:
                    try:
                        obj = json.loads(text[m.start() : j + 1])
                    except Exception:
                        obj = None
                    if isinstance(obj, dict) and isinstance(obj.get("files"), list):
                        best = obj["files"]
                    break
    out = []
    for f in best or []:
        if not isinstance(f, str):
            continue
        f = f.strip().replace(str(tree) + "/", "")
        for prefix in ("./", "a/", "b/", "/"):
            if f.startswith(prefix):
                f = f[len(prefix):]
        f = f.split(":")[0]
        if f and f not in out:
            out.append(f)
    return out[:10]


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
    if t["family"] == "F":
        tree = Path(t["repo_dir"])
        # A kernel tree of this family is an exported directory inside this repository. To
        # codefast, as to git, the project of a directory is the work tree around it, which
        # would be codefast itself: its command and its server would index that, into the
        # index directory of this tree, and answer about the wrong code. A real kernel checkout
        # is a work tree of its own, so each of these is made one (an empty repository, nothing
        # added to it).
        if not (tree / ".git").exists():
            subprocess.run(["git", "init", "-q", "."], cwd=tree, check=True)
        row = dict(id=t["id"], family="F", arm=arm, rep=rep, repo=str(tree), run=str(run), repo_name=t["repo_name"], model=a.model, version=t["version"],
                   gold_files=t["gold_files"], index=str(f_index(tree)), started=time.time())
        (run / "prompt.txt").write_text(prompt_for(t, tree))
        json.dump(row, open(run / "task.json", "w"), indent=1)
        return row
    if t["family"] in ("N", "L", "E", "C"):
        parts = t["parts"] if t["family"] == "L" else [t]
        where, index = repo_of(parts[0])
        row = dict(id=t["id"], family=t["family"], arm=arm, rep=rep, repo=str(where), run=str(run), repo_name=t["repo_name"], model=a.model, tree_state=tree_state(where),
                   index=str(index), parts=[dict(gold=p["gold"], wrong=p["wrong"]) for p in parts])
        (run / "prompt.txt").write_text(prompt_for(t, where))
        json.dump(row, open(run / "task.json", "w"), indent=1)
        return row
    if t["family"] in ("G", "S"):
        # Nothing but the specification, in a work tree of its own (see family F for why).
        repo.mkdir()
        shutil.copy2(t["stage_files"][0] if t["family"] == "S" else Path(t["dir"]) / "SPEC.md", repo / "SPEC.md")
        git = ["git", "-c", "user.name=bench", "-c", "user.email=bench@localhost", "-c", "commit.gpgsign=false"]
        subprocess.run(git + ["init", "-q", "."], cwd=repo, check=True)
        (repo / ".git/info/exclude").write_text(".codefast/\n")
        subprocess.run(git + ["add", "-A"], cwd=repo, check=True, capture_output=True)
        subprocess.run(git + ["commit", "-q", "-m", "specification"], cwd=repo, check=True, capture_output=True)
        row = dict(id=t["id"], family=t["family"], arm=arm, rep=rep, repo=str(repo), run=str(run), repo_name=t["repo_name"], model=a.model, language=t["language"],
                   check=t["check_cmd"], hidden=str(Path(t["dir"]) / "hidden"), tests=t["tests"], check_timeout=t.get("timeout", 300))
        if t["family"] == "S":
            row["checkpoints"] = str(Path(t["dir"]) / "checkpoints")
            json.dump(stage_prompts(t, repo), open(run / "prompts.json", "w"), indent=1)
        if spec["plugin"]:
            e = dict(os.environ, CODEFAST_NO_REFRESH="")
            subprocess.run([str(binary), "--root", str(repo), "index"], capture_output=True, text=True, env=e)
        (run / "prompt.txt").write_text(prompt_for(t, repo))
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
    elif t["family"] == "W":
        row.update(check=t["check_cmd"].replace("{repo}", str(repo)), file=t["file"], name=t["name"], owner=t["owner"],
                   removed_lines=t["removed_lines"], covered=t["covered"], tests_hash=test_tree_hash(repo), base=t["base"])
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


def score_listed(text, part):
    """F1 of a list that names each function with its file on one line, which is what a
    question about callees asks for. A name and a file that merely both occur somewhere in the
    answer do not count: the file of the function asked about is in every answer, and a callee
    often has a namesake there."""
    def on_a_line(who):
        base = who["file"].rsplit("/", 1)[-1]
        name = re.compile(r"(?<![A-Za-z0-9_])" + re.escape(who["name"]) + r"(?![A-Za-z0-9_])")
        return any(base in line and name.search(line) for line in text.splitlines())

    found = sum(1 for g in part["gold"] if on_a_line(g))
    wrong = sum(1 for w in part["wrong"] if on_a_line(w) and not any(g["name"] == w["name"] and g["file"].rsplit("/", 1)[-1] == w["file"].rsplit("/", 1)[-1] for g in part["gold"]))
    if not found:
        return 0.0
    recall, precision = found / len(part["gold"]), found / (found + wrong)
    return 2 * recall * precision / (recall + precision)


def verify_n(row):
    text = final_text(row["run"])
    if row["family"] == "C":
        return score_listed(text, row["parts"][0])
    if row["family"] in ("N", "E"):
        return score_callers(text, row["parts"][0])
    # One section per question. An answer that has no headings is read as a whole for each.
    cuts = [m.start() for m in re.finditer(r"(?m)^#+\s*Question\s+\d+", text)]
    sections = [text[a:b] for a, b in zip(cuts, cuts[1:] + [len(text)])] if len(cuts) == len(row["parts"]) else [text] * len(row["parts"])
    return sum(score_callers(s, p) for s, p in zip(sections, row["parts"])) / len(row["parts"])


def test_tree_hash(repo):
    """Every test file and every file of test data, so an edit to one of them is visible."""
    h = hashlib.sha256()
    for p in sorted(Path(repo).rglob("*")):
        rel = str(p.relative_to(repo))
        if p.is_file() and (rel.endswith("_test.go") or rel.startswith("testdata/") or "/testdata/" in rel):
            h.update(rel.encode())
            h.update(p.read_bytes())
    return h.hexdigest()


def hidden_passed(language, hidden, out):
    """How many of the hidden tests the output of the check reports as passed. Tests a session
    wrote for itself are run by some of these commands as well, and are not counted."""
    hidden = Path(hidden)
    if language == "python":
        # `unittest discover -s tests_hidden` runs the hidden tests and nothing else.
        ran = re.search(r"^Ran (\d+) tests?", out, re.M)
        bad = re.search(r"^FAILED \(([^)]*)\)", out, re.M)
        lost = sum(int(n) for n in re.findall(r"(?:failures|errors|skipped|unexpected successes)=(\d+)", bad.group(1))) if bad else 0
        return max(0, int(ran.group(1)) - lost) if ran else 0
    if language == "go":
        names = {m for p in hidden.rglob("*_test.go") for m in re.findall(r"^func (Test\w+)\(", p.read_text(), re.M)}
        return len({m for m in re.findall(r"^--- PASS: (\w+)", out, re.M) if m in names})
    if language == "rust":
        # One test binary per file under tests/: count the passes of the hidden ones.
        n, mine = 0, False
        for line in out.splitlines():
            m = re.match(r"\s*Running (\S+)", line)
            if m:
                mine = Path(m.group(1)).name.startswith("hidden")
            elif line.lstrip().startswith("Doc-tests"):
                mine = False
            elif mine and re.match(r"test \S+ \.\.\. ok\b", line):
                n += 1
        return n
    if language == "typescript":
        # `node --test tests_hidden/` runs the hidden tests alone, and reports in TAP when it
        # is not on a terminal ("# pass 31") and in its own format when it is.
        m = re.search(r"^(?:#|\u2139) pass (\d+)", out, re.M)
        return int(m.group(1)) if m else 0
    raise ValueError(language)


def verify_g(row):
    """The hidden tests come into a copy of what the session built, and are run there."""
    run = Path(row["run"])
    graded = run / "graded"
    if graded.exists():
        shutil.rmtree(graded)
    shutil.copytree(row["repo"], graded, symlinks=True, ignore=shutil.ignore_patterns(".git", ".codefast", "__pycache__", "target", "node_modules"))
    shutil.copytree(row["hidden"], graded, dirs_exist_ok=True)
    cmd = row["check"]
    if row["language"] == "go" and " -v" not in cmd:
        cmd += " -v -count=1"
    if row["language"] == "rust" and "--no-fail-fast" not in cmd:
        # Without it cargo stops at the first test binary that has a failure, and the tests of
        # the binaries after it are never run.
        cmd += " --no-fail-fast"
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1", CARGO_TARGET_DIR=str(run / "cargo-target"), CARGO_BUILD_JOBS="2", NO_COLOR="1", GOFLAGS="-mod=mod")
    try:
        # One stream, in the order it was written: cargo names the test binary it starts on
        # stderr and the binary prints its tests on stdout.
        p = subprocess.run(["nice", "-n", "10", "/bin/sh", "-c", cmd], cwd=graded, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=row.get("check_timeout", 300), env=env)
        rc, out = p.returncode, p.stdout
    except subprocess.TimeoutExpired as e:
        rc, out = 124, (e.stdout or b"").decode(errors="replace") if isinstance(e.stdout, bytes) else (e.stdout or "")
    (run / "check.txt").write_text(out[-40000:])
    shutil.rmtree(run / "cargo-target", ignore_errors=True)
    got = min(hidden_passed(row["language"], row["hidden"], out), row["tests"])
    row["tests_passed"] = got
    row["kscore"] = round(got / row["tests"], 3)
    return rc == 0 and got == row["tests"], True


def verify_s(row):
    """The last state against the hidden tests, and every state on the way against the tests
    that hold for it: how far the conversation got, and where it lost something."""
    run = Path(row["run"])
    stages = []
    for snap in sorted(run.glob("stage-*")):
        k = int(snap.name.split("-")[1])
        tests = Path(row["checkpoints"]) / f"{k:02d}"
        if not tests.is_dir():
            continue  # the last state is graded below, against the final tests
        part = dict(row, repo=str(snap), hidden=str(tests), run=str(run / f"graded-{k:02d}"), tests=10**6)
        Path(part["run"]).mkdir(exist_ok=True)
        verify_g(part)
        stages.append(part["tests_passed"])
        shutil.rmtree(part["run"], ignore_errors=True)
    row["stage_passed"] = stages
    row["stages_done"] = len(list(run.glob("stage-*")))
    return verify_g(row)


def verify(row):
    if row["family"] == "G":
        return verify_g(row)
    if row["family"] == "S":
        return verify_s(row)
    if row["family"] in ("N", "L", "E", "C"):
        s = verify_n(row)
        row["kscore"] = s
        return s >= 0.9, row.get("tree_state") in (None, tree_state(row["repo"]))
    if row["family"] == "F":
        files = ranked_files(final_text(row["run"]), row["repo"])
        ranks = [files.index(g) + 1 for g in row["gold_files"] if g in files]
        row["rank"] = min(ranks) if ranks else None
        row["answered"] = len(files)
        row["kscore"] = 1.0 if ranks else 0.0
        # The tree is shared by every run of that version: a file newer than this run began
        # would be a file a session wrote.
        newer = sh(["find", row["repo"], "-type", "f", "-newermt", time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(row.get("started", 0))), "-print", "-quit"])[1].strip()
        return bool(ranks), not newer
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
    if row["family"] == "W":
        # The tests are the judge, so a run that edited them or their data does not count.
        return ok, test_tree_hash(row["repo"]) == row["tests_hash"]
    defs = lambda root: sorted(
        (str(p.relative_to(root)), m.group(0))
        for p in Path(root).rglob("*.go")
        for m in re.finditer(r"^func \([^)]*\) \w+", p.read_text(errors="replace"), re.M)
        if m.group(0).endswith((" " + row["name"], " " + row["new_name"]))
    )
    return ok, defs(row["repo"]) == defs(row["base"])


SNAPSHOT_SKIP = shutil.ignore_patterns(".git", ".codefast", "__pycache__", "target", "node_modules")


def staged_session(cmd, row, env, run, timeout):
    """One conversation that is sent the messages of a staged task one after the other: the
    next goes in when the reply to the last has ended. The project as it stands after each
    reply is kept, so that every state on the way can be graded."""
    prompts = json.load(open(run / "prompts.json"))
    message = lambda text: json.dumps({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]}}) + "\n"
    with open(run / "run.jsonl", "w") as out, open(run / "err.txt", "w") as err:
        p = subprocess.Popen(cmd + ["--input-format", "stream-json"], cwd=row["repo"], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err, text=True, bufsize=1)
        late_start = time.time()
        late = threading.Timer(timeout, p.kill)
        late.start()
        try:
            p.stdin.write(message(prompts[0]))
            p.stdin.flush()
            k = 0
            for line in p.stdout:
                out.write(line)
                out.flush()
                # The keys of an event come in no fixed order.
                if '"type":"result"' not in line:
                    continue
                try:
                    ended = json.loads(line)
                except ValueError:
                    continue
                if ended.get("type") != "result":
                    continue
                k += 1
                out.write(json.dumps({"type": "bench", "subtype": "stage_done", "stage": k, "seconds": round(time.time() - late_start)}) + "\n")
                out.flush()
                shutil.copytree(row["repo"], run / f"stage-{k:02d}", symlinks=True, ignore=SNAPSHOT_SKIP)
                if k < len(prompts) and ended.get("subtype") == "success":
                    p.stdin.write(message(prompts[k]))
                    p.stdin.flush()
                else:
                    # The last message is answered, or the session hit a limit: nothing more to send.
                    p.stdin.close()
            return p.wait()
        except (BrokenPipeError, OSError):
            p.kill()
            return 125
        finally:
            late.cancel()


def fresh_sessions(cmd, row, env, run, timeout):
    """The messages of a staged task, each in a session of its own in the same directory: the
    project is what the next session finds, and the conversation is not. A message after the
    first says that the project exists, as a user starting a new session on it would."""
    prompts = json.load(open(run / "prompts.json"))
    deadline = time.time() + timeout
    rc = 0
    with open(run / "run.jsonl", "w") as out, open(run / "err.txt", "w") as err:
        for k, text in enumerate(prompts, 1):
            if k > 1:
                text = f"You are in a project directory ({row['repo']}). SPEC.md describes the first version of the project, and it has been changed since: the code is what counts.\n\n" + text
            left = deadline - time.time()
            if left <= 0:
                return 124
            try:
                p = subprocess.run(cmd, cwd=row["repo"], env=env, input=text, stdout=subprocess.PIPE, stderr=err, text=True, timeout=left)
            except subprocess.TimeoutExpired:
                return 124
            out.write(p.stdout)
            out.write(json.dumps({"type": "bench", "subtype": "session_end", "stage": k}) + "\n")
            out.write(json.dumps({"type": "bench", "subtype": "stage_done", "stage": k}) + "\n")
            out.flush()
            shutil.copytree(row["repo"], run / f"stage-{k:02d}", symlinks=True, ignore=SNAPSHOT_SKIP)
            rc = p.returncode
            ended = [json.loads(l) for l in p.stdout.splitlines() if '"type":"result"' in l]
            if rc != 0 or not ended or ended[-1].get("subtype") != "success":
                break
    return rc


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
    if row["family"] == "F" and spec["plugin"]:
        env["CODEFAST_INDEX_DIR"] = row["index"]
        # The kernel trees of this family are plain directories inside this repository, not
        # checkouts of their own. Left to itself codefast takes the git work tree around the
        # working directory for the project, which here is codefast: its server would index
        # that into the directory named above and answer about the wrong code. A real kernel
        # checkout is its own work tree, and this says so for these.
        env["CODEFAST_ROOT"] = row["repo"]
    if row["family"] in ("N", "L", "E", "C") and spec["plugin"]:
        env["CODEFAST_INDEX_DIR"] = row.get("index") or str(SYFT_INDEX)
    env.update(spec.get("env", {}))
    cmd = [
        a.claude, "-p", "--model", a.model, "--output-format", "stream-json", "--verbose", "--include-hook-events",
        "--no-session-persistence", "--setting-sources", "project", "--dangerously-skip-permissions",
        "--disallowedTools", DENY, "--max-turns", str(a.max_turns), "--max-budget-usd", str(a.budget or BUDGET[a.model]), "--no-chrome",
    ]
    if a.effort:
        cmd += ["--effort", a.effort]
    if a.tools:
        # The built-in tools a published harness pins. What a plugin brings is not in this list
        # and stays available.
        cmd += ["--tools", a.tools]
    if spec.get("steer"):
        cmd += ["--append-system-prompt", STEER]
    if spec.get("rules") and row["family"] in ("K", "F"):
        # These families run in a shared tree, where no file can be written for one arm: the
        # rules block that `codefast setup agents-md` writes goes in with the system prompt.
        cmd += ["--append-system-prompt", rules_block(binary)]
    # The account's cloud connectors (mail, documents) have no place in a benchmark run. Strict
    # mode would also drop the plugin's own server, so it is used only where no plugin is loaded.
    env["ENABLE_CLAUDEAI_MCP_SERVERS"] = "false"
    if spec["plugin"]:
        cmd += ["--plugin-dir", str(plug) + ("-lean" if spec.get("lean") else "")]
    else:
        cmd += ["--strict-mcp-config"]
    t0 = time.time()
    if row["family"] == "S" and spec.get("fresh"):
        rc = fresh_sessions(cmd, row, env, run, a.timeout)
    elif row["family"] == "S":
        rc = staged_session(cmd, row, env, run, a.timeout)
    else:
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
    r: dict[str, Any] = dict(requests=0, sub_requests=0, tools={}, cmds=[], cf_cli=0, cf_mcp=0, hooks={}, hook_bytes=0, result_chars={}, init={}, limits={}, final=None, usage={}, cost=0.0, turns=0, denials=0,
                             thinking=None, touched=[])
    seen, names = set(), {}
    earlier, earlier_cost = dict(input=0, cache_read=0, cache_write=0, output=0), 0.0
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
                    r["touched"].append(json.dumps(c.get("input") or {}))
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
            # Of the output, what was thinking: not shown, and billed like the rest. A
            # conversation of several messages ends each with a result of its own, which
            # reports that message's tokens (`modelUsage` below is of the whole session).
            t = (u.get("output_tokens_details") or {}).get("thinking_tokens")
            if t is not None:
                r["thinking"] = (r["thinking"] or 0) + t
            r["messages"] = r.get("messages", 0) + 1
            r["turns"] += e.get("num_turns", 0)
            r["final"] = e.get("subtype")
            r["denials"] = len(e.get("permission_denials") or [])
            # Sub-agents are billed on top of the main thread: modelUsage has the whole session.
            mu = e.get("modelUsage") or {}
            if mu:
                r["usage"] = dict(input=sum(v.get("inputTokens", 0) for v in mu.values()), cache_read=sum(v.get("cacheReadInputTokens", 0) for v in mu.values()),
                                  cache_write=sum(v.get("cacheCreationInputTokens", 0) for v in mu.values()), output=sum(v.get("outputTokens", 0) for v in mu.values()))
            r["cost"] = earlier_cost + e.get("total_cost_usd", 0.0)
        elif t == "bench" and s == "session_end":
            # A run of several sessions (a staged task with a session per message): what the
            # sessions before this point used is kept, and the next one counts from nothing.
            for k in earlier:
                earlier[k] += r["usage"].get(k, 0)
            earlier_cost, r["usage"] = r["cost"], {}
    if any(earlier.values()):
        r["usage"] = {k: earlier[k] + r["usage"].get(k, 0) for k in earlier}
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
        # One index per kernel version, written by this batch's binary before any session.
        for tree in sorted({t["repo_dir"] for t, _, _ in todo if t["family"] == "F"}):
            e = dict(os.environ, CODEFAST_INDEX_DIR=str(f_index(tree)))
            subprocess.run([str(binary), "--root", tree, "status"], capture_output=True, text=True, env=e)
        for where, index in {repo_of((t.get("parts") or [t])[0]) for t, _, _ in todo if t["family"] in ("N", "L", "E", "C")}:
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
        # The day the session ended, which is when its outcome was written.
        day = time.strftime("%Y-%m-%d", time.localtime(out.stat().st_mtime))
        row = dict(tag=tag, date=day, id=res["id"], family=res["family"], arm=res["arm"], model=res["model"], requests=r["requests"], sub_requests=r["sub_requests"], **u)
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
        if res["family"] == "F":
            row["rank"], row["answered"] = res.get("rank"), res.get("answered")
        row["violations"] = (["control arm used the tool"] if res["arm"] == "control" and (r["cf_cli"] or r["cf_mcp"]) else []) + (["ended early: " + str(r["final"])] if r["final"] != "success" else [])
        row["thinking"] = r["thinking"]
        if res["family"] in ("G", "S"):
            row["tests_passed"], row["tests"] = res.get("tests_passed"), res.get("tests")
            # The hidden tests and the reference live in this repository: a session that went
            # looking for them is not a session that built the project.
            if any("tier2/new" in t or "tier2/staged" in t for t in r["touched"]):
                row["violations"].append("looked at the task's own files")
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


def cmd_check_new(a):
    """The grader and the tasks, checked against each other: the reference implementation of a
    task, graded as a session's work would be, passes every hidden test, and an empty project
    passes none."""
    want = set(a.ids.split(",")) if a.ids else None
    bad = 0
    for t in g_tasks() + s_tasks():
        if want and t["id"] not in want:
            continue
        run = LIVE / "check-new" / t["id"]
        if run.exists():
            shutil.rmtree(run)
        run.mkdir(parents=True)
        row = dict(run=str(run), repo=str(Path(t["dir"]) / "reference"), language=t["language"], check=t["check_cmd"], hidden=str(Path(t["dir"]) / "hidden"), tests=t["tests"], check_timeout=t.get("timeout", 300))
        ok, _ = verify_g(row)
        (run / "empty").mkdir()
        (run / "empty/SPEC.md").write_text("nothing built")
        none = dict(row, run=str(run / "none"), repo=str(run / "empty"))
        Path(none["run"]).mkdir()
        ok0, _ = verify_g(none)
        fine = ok and not ok0 and none["tests_passed"] == 0
        bad += not fine
        print(f"{t['id']:10} {t['language']:10} {t['split']:4} reference {row['tests_passed']:2d}/{t['tests']:2d}  empty project {none['tests_passed']}/{t['tests']}  {'ok' if fine else 'NOT OK'}  {t['title']}")
    sys.exit(1 if bad else 0)


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
    p.add_argument("--held", default="", help="held-out sets of tasks.json, by name: K,K2,C,F,F2,D,I,N,R,E,L,NZ,W,W2, and G (new projects, held out) or Gdev (the ones to develop on)")
    p.add_argument("--d", type=int, default=16)
    p.add_argument("--r", type=int, default=8)
    p.add_argument("--k", type=int, default=0, help="kernel questions")
    p.add_argument("--n", type=int, default=0, help="who-calls-this questions on syft")
    p.add_argument("--l", type=int, default=0, help="sessions of four such questions")
    p.add_argument("--w", type=int, default=0, help="code-writing tasks: write a removed function body")
    p.add_argument("--jobs", type=int, default=4)
    p.add_argument("--reps", type=int, default=1, help="runs per task and arm")
    p.add_argument("--resume", action="store_true", help="skip runs that already have an outcome")
    p.add_argument("--refresh-bin", action="store_true")
    p.add_argument("--max-turns", type=int, default=120)
    p.add_argument("--effort", default="", help="effort level of the session, as the harness takes it (the published kernel comparison used low)")
    p.add_argument("--tools", default="", help="pin the built-in tools, comma separated (the published kernel comparison used Read,Edit,Write,Bash,Grep,Glob)")
    p.add_argument("--budget", type=float, default=0.0, help="dollar cap per run at list prices (default by model)")
    p.add_argument("--timeout", type=int, default=3600)
    p.add_argument("--limit-stop", type=float, default=0.6, help="stop the batch when an account usage window is this full")
    p.add_argument("--claude", default=os.environ.get("CLAUDE_BIN", str(Path.home() / ".local/bin/claude")))
    p.add_argument("--config-dir", default=os.environ.get("CLAUDE_CONFIG_DIR", ""))
    p.add_argument("--codefast", default=os.environ.get("CODEFAST_BIN", str(ROOT / "target/release/codefast")))
    p = sub.add_parser("check-new", help="grade the reference implementation of every new-project task: each must pass all of its hidden tests")
    p.add_argument("--ids", default="")
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
    {"run": cmd_run, "verify": cmd_verify, "score": cmd_score, "show": cmd_show, "check-new": cmd_check_new}[a.cmd](a)


if __name__ == "__main__":
    main()
