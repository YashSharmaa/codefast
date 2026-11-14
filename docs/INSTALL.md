# Installing codefast

codefast is one binary. A coding harness uses it in up to four ways:

- **the skill**: a short guide (`skills/codefast/SKILL.md`) that tells the agent to run the
  `codefast` command in its shell. This is the main path and costs about 140 tokens per session.
- **the MCP server**: the same binary started as `codefast mcp`, for harnesses that prefer tools.
- **hooks**: optional. They hand the agent facts at the moment they matter: where a symbol named
  in a prompt is defined and how widely it is used, what a search did not print, what an edit
  left behind, and what has changed so far after a compaction. A hook with nothing new to say
  prints nothing.
- **the rules block**: optional, sixteen lines in a project's `AGENTS.md` that say which command
  answers which question. It is always in the agent's context, where a skill has to be picked
  up first, and it reaches agents that have nothing else (`codefast setup agents-md`, section 2).

`docs/HARNESSES.md` says what each harness gets and which file it reads.

Every entry below carries one of these marks:

| Mark | Meaning |
|---|---|
| **tested in a sandbox** | Run on 2026-11-13 with the named version, against throwaway home and configuration directories: by the scripts in `tests/packaging/manifests/`, and once more by hand from a clone-like copy of this repository, command by command as this file gives them. No session was started and no model was called, so nothing here was observed inside a live agent session. |
| **validated against a schema** | The files that harness reads pass the published JSON Schema of their format (`tests/packaging/manifests/validate.py`). The harness itself was not run. |
| **documented only** | Taken from the harness's own documentation, read on 2026-11-12 and 2026-11-13. Not run. |

## 1. The binary

Install the binary first. The skill tells the agent to type `codefast`, so the command has to be
on `PATH`, and every configuration snippet in this file starts `codefast` by name.

The one exception is the Claude Code plugin: it puts a launcher, `bin/codefast`, on the `PATH` of
Claude Code's Bash tool, and the launcher finds a codefast binary or fetches the one for your
platform the first time it runs. Plugins for the other harnesses start their MCP server through
the same launcher, so their tools work without this step, but their skill does not.

```sh
curl -fsSL https://raw.githubusercontent.com/YashSharmaa/codefast/main/install.sh | sh
```

The script downloads the release asset for your platform (macOS and Linux, arm64 and x86-64),
checks its SHA-256 against the `SHA256SUMS` file of the same release, and installs
`~/.local/bin/codefast`. It never uses sudo and never edits a shell profile. It runs the new
binary once before putting it in place, so a download that cannot run on your machine never
replaces a codefast that works.

```sh
sh install.sh --dry-run                 # print every action, download and write nothing
sh install.sh --version 0.3.1           # a specific release instead of the latest
sh install.sh --bin-dir /opt/tools/bin  # another directory
```

`CODEFAST_VERSION`, `CODEFAST_INSTALL_DIR` and `CODEFAST_RELEASE_BASE` set the same things from
the environment. *Tested in a sandbox* under sh (bash 3.2 in POSIX mode), dash and BusyBox 1.36
against a local directory standing in for the release server (`CODEFAST_RELEASE_BASE=file://DIR`).
The `curl` line above was run once as written on 2026-11-13, in a sandbox on macOS arm64, against
release v0.1.0: it downloaded the asset, checked it and installed a working binary. The
`install-check` workflow repeats that on Linux x86-64, Linux arm64 and macOS after every release.

From source: `cargo install --git https://github.com/YashSharmaa/codefast` (run once as written on
2026-11-13), or `cargo build --release` in a checkout (the binary is `target/release/codefast`,
which the launcher of that checkout finds by itself). On Windows, build from source: neither `install.sh` nor the launcher supports
Windows, and the npm package below is not published yet.

With Node.js, once the npm packages are published (`npm/` holds them, and nothing is published yet):

```sh
npx -y @yashsharmaa/codefast@0.3.1 mcp        # or: npm install -g @yashsharmaa/codefast
```

### The launcher

`bin/codefast` looks for the binary in this order and runs the first one it finds:

1. `$CODEFAST_BIN`.
2. its cache, `${CODEFAST_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/codefast}/<version>/codefast`.
3. another `codefast` on `PATH` (never itself).
4. `target/release/codefast`, when it sits in a source checkout.
5. the release asset, downloaded into the cache and checked against its SHA-256.

`CODEFAST_NO_DOWNLOAD=1` forbids step 5. A downloaded binary is run once before it enters the
cache, and one that cannot run on this machine is discarded. After a failed download the launcher
does not try again for ten minutes, so an offline machine pays for one timeout only. It writes
nothing to stdout. *Tested in a sandbox* (134 checks under sh, dash and bash in POSIX mode, 133
under BusyBox).

Not every harness passes these variables on. Claude Code starts a plugin's MCP server and hooks
with your environment. Codex starts a plugin's MCP server with `HOME`, `PATH`, `TMPDIR`, `TERM`
and its own `PLUGIN_ROOT` and `PLUGIN_DATA`, and with nothing else from your shell (seen with
codex-cli 0.164.0). Gemini CLI documents the same for extensions. `CODEFAST_BIN`,
`CODEFAST_CACHE_DIR`, `CODEFAST_NO_DOWNLOAD` and proxy settings do not reach the launcher there,
so put `codefast` on `PATH`: steps 2 and 3 need nothing but `HOME` and `PATH`.

## 2. One command for every harness

```sh
codefast setup                              # what it would do for the harnesses found here; changes nothing
codefast setup --all --apply                # register with every harness found on this machine
codefast setup claude-code codex --hooks --apply
codefast setup --harness claude-code --scope project --apply
codefast setup agents-md --apply            # the rules block in this repository's AGENTS.md
codefast doctor                             # or: codefast doctor --json
codefast doctor context                     # what the harness loads into every request
codefast doctor sessions                    # where the tokens of this project's sessions went
```

`codefast setup [TARGET ...] [OPTIONS]` registers codefast with coding harnesses: the MCP server
and the skill, and the hooks when asked. It runs the harness's own command where there is one
(`claude mcp add`, `codex mcp add`, `gemini mcp add`, `qwen mcp add`, `copilot mcp add`,
`amp mcp add`, `cline mcp add`, `droid mcp add`, `devin mcp add`, `openclaw mcp add`,
`vibe mcp add`, `openhands mcp add`, `auggie mcp add`, `codebuddy mcp add`, `code --add-mcp`) and
that program is on `PATH`.
Otherwise it edits the configuration file the harness documents, keeping its comments, order
and formatting. `docs/HARNESSES.md` lists the file for every harness and scope.

It changes something only when a target is named (or `--all`) **and** either `--apply` is given
or its output goes to a terminal. In every other case, an agent's shell or a script among them,
it prints the plan and ends with "Dry run: nothing was changed". `--print` asks for the plan
whatever the circumstances.

| Target | What it is |
|---|---|
| `claude-code`, `codex`, `gemini-cli`, `cursor`, `copilot` (VS Code), `copilot-cli`, `opencode`, `qwen-code`, `goose`, `amp`, `cline`, `kilo-code`, `droid`, `crush`, `zed`, `kiro`, `junie`, `deepagents`, `pi`, `devin`, `antigravity`, `dsh`, `aider`, `hermes`, `openclaw`, `mistral-vibe`, `continue`, `openhands`, `warp`, `auggie`, `codebuddy`, `kimi-code` | A harness. Also given as `--harness ID`, several are fine, and common names work too (`claude`, `gemini`, `vscode`, `kilo`, `qwen`, `factory`, `windsurf`, `dcode`, `agy`, `vibe`, `cn`, `augment`, `kimi`). |
| `--all` | Every harness found on this machine: its program is on `PATH`, or its configuration directory exists. `codefast setup --list` shows which. |
| `agents-md` | The short rules block (between `<!-- codefast:begin -->` and `<!-- codefast:end -->`) in the project's `AGENTS.md`, and in `CLAUDE.md` or `GEMINI.md` where they exist and do not import `AGENTS.md`. `--file PATH` names another file. |
| `mcp-json` | Prints the standard `mcpServers` snippet for any other MCP client. Writes nothing. |

| Option | Meaning |
|---|---|
| `--scope user\|project` | The user's own configuration (the default), or files inside this repository that a team shares. User-scope entries name this binary by its absolute path. Project-scope entries name plain `codefast`, so everyone who uses them needs it on `PATH`. Choose one of the two for a harness: with the server registered at both, Claude Code reports "conflicting scopes" in `claude mcp list`. |
| `--with mcp,skill,hooks` | Which parts to register. The default is `mcp,skill`. |
| `--hooks` | Adds the hooks to the parts. Setup registers hooks for Claude Code and Codex only. |
| `--print`, `--apply` | See above. Together they are an error. |
| `--force` | Replace a server entry or a skill named `codefast` that setup did not write. Without it such an entry is left alone and reported as `refused`, and a run that was to apply ends with status 1. |
| `--bin PATH` | The command to register instead of this binary. |
| `--list` | The harnesses setup knows, and whether each is found here. |

Each line of the report carries the part (`mcp`, `skill`, `hooks`, `rules`, `plugin`) and one of
these words: `plan` (would be done), `done`, `ok` (already in place), `skip` (the harness has no
such thing at this scope), `refused` (see `--force`), `manual` (something for you to do by hand,
with the text to use), `print` (something to read) and `failed`.

What else it takes care of:

- What a plugin brings is not registered a second time. With the codefast plugin installed in
  Claude Code, setup leaves the skill, the server and the hooks to it. With the plugin installed
  in Codex it leaves the skill to it and still registers the server and the hooks: Codex loads
  no hooks from the plugin, and it starts the plugin's server in the plugin's own directory,
  where every call has to name the repository (see the Codex section). A plugin that Claude Code
  installed for one project (`--scope project`) counts in that project only.
- An older registration of its own is brought up to date where it stands: only the command
  changes, and arguments after `mcp` (a `--root DIR` you added), an environment, approved tools
  or a switch that turns the server off stay as they are.
- A file it cannot parse is left untouched and reported as `manual`.

Look at the plan before `--all --apply`. A harness counts as found when a program with its name
is on `PATH`, and setup then runs that program: another tool of the same name (`goose` the
database migration tool, `zed` of ZFS) would be taken for the harness. Naming the targets
yourself avoids that.

`codefast setup help` prints the options. `codefast setup --help` prints only the generic
command-line summary.

`codefast doctor [--json]` reports, and never writes: the binary and whether `codefast` on `PATH`
is this binary (or a plugin's launcher or the npm wrapper that runs it), the index of the
repository you are in (location, size, when it was built, how far behind the working tree it
is), and the rules block in `AGENTS.md`, `CLAUDE.md` and `GEMINI.md`. For each harness found, it
also reports the plugin, the MCP entry, the skill and the hooks at user and project scope, with
the file that holds each. A harness that is found and not registered is listed with the command
that registers it. When a plugin and a hand-made entry register the same server or hooks in Claude
Code, it says so (`twice` in the text, `registered_twice` in the JSON), and when the only server
Codex has is the plugin's, which needs `root` with every call, it says that too
(`plugin_server_needs_root`). The JSON form is one object with the members `binary`, `index`,
`rules`, `harnesses` and `not_found`.

**Tested in a sandbox** with Claude Code 2.1.323 and codex-cli 0.164.0: every command above, the
dry runs, `--apply` running the real `claude mcp add` and `codex mcp add`, project scope, hooks,
and `doctor` before and after. Then `claude mcp list` reported the server as connected and the
Codex app server listed the skill, the server's five tools and the hooks without warnings.
For every other harness setup writes the file format that harness documents, and its own tests
read the result back. None of those files has been loaded by the harness itself, and the
commands of `gemini`, `qwen`, `copilot`, `amp`, `cline`, `droid`, `devin`, `openclaw`, `vibe`,
`openhands`, `auggie`, `codebuddy` and `code` were only exercised with stand-in programs that
record how they were called.

## 3. Harness by harness

Each section says what that harness gets from this repository as a plugin or an extension,
where it has such a thing, and gives the registration of the MCP server and the skill by hand.
For the second part `codefast setup ID --apply` does the same in one step (the ids are in
section 2, and `docs/HARNESSES.md` lists the file it writes for each harness and scope). It needs
the binary on `PATH` or wherever you run it from, and nothing published.

### Claude Code

Plugin: skill, `codefast` on the Bash tool's `PATH`, MCP server, hooks.
**Tested in a sandbox** with Claude Code 2.1.322 and 2.1.323 (`claude plugin validate --strict`,
install from a git URL, `claude plugin list`, `claude plugin details`, `claude mcp list`). The
command below was also run once as written on 2026-11-13 with 2.1.323, in a sandbox: it cloned
this repository from GitHub, the launcher fetched the v0.1.0 binary by itself, and
`claude mcp list` reported the plugin's server as connected.

```sh
claude plugin install codefast --marketplace YashSharmaa/codefast
```

The same in two steps, `claude plugin marketplace add YashSharmaa/codefast` and then
`claude plugin install codefast@codefast`, also **tested in a sandbox**. Inside a session the two
are `/plugin marketplace add YashSharmaa/codefast` and `/plugin install codefast@codefast` (not run:
no session was started). Add `--scope project` to the install to record the plugin in the
repository's `.claude/settings.json`: Claude Code then loads it in that repository only, and
`codefast doctor` and `codefast setup` treat it the same way. The plugin is for Claude Code
itself: claude.ai and Cowork do not install a plugin that has a `bin/` directory.

What Claude Code shows after the install (`claude plugin details codefast`): one skill, the hook
events `SessionStart`, `PreToolUse`, `PostToolUse` and `PostToolUseFailure`, and one MCP server,
which `claude mcp list` reports as connected once the launcher finds a binary: on `PATH`, in its
cache, or after its first download. With no binary anywhere the server is listed as failed and
the hooks stay silent.

MCP server only (**tested in a sandbox**), which is what `codefast setup claude-code` runs, with
the binary's full path in place of the bare name:

```sh
claude mcp add --transport stdio --scope user codefast -- codefast mcp
```

### OpenAI Codex CLI

Plugin: skill (listed as `codefast:codefast`) and MCP server. **Tested in a sandbox** with codex-cli 0.164.0
(`codex plugin marketplace add`, `codex plugin add`, `codex plugin list`, `codex mcp list`, then
the app server's `skills/list`, `mcpServerStatus/list` and `plugin/read`).

```sh
codex plugin marketplace add YashSharmaa/codefast
codex plugin add codefast@codefast
```

Have `codefast` on `PATH` first (section 1). The skill needs it there, and so does the plugin's
MCP server in practice: Codex starts it with `HOME` and `PATH` but without the other variables of
your shell, so the launcher finds the binary on `PATH` or has to download it.

Then register the server and, if you want them, the hooks (**tested in a sandbox**, with the
plugin installed and without it):

```sh
codefast setup codex --hooks --apply
```

Why the server once more: Codex starts a plugin's server in the plugin's own directory, so that
server cannot know your project and answers only when the model passes the `root` of the
repository with every call. The entry that `codefast setup codex` writes (it runs
`codex mcp add codefast -- /path/to/codefast mcp`) is started in your project and needs no
`root`, and Codex uses it in place of the plugin's server of the same name: with both present,
codex-cli 0.164.0 started only the configured one. `codefast doctor` tells you when the plugin's
server is the only one there is.

Why the hooks this way: Codex does not load hooks from a plugin that uses the Agent Plugins
manifest, so they are registered in a configuration layer. Setup writes `~/.codex/hooks.json`
(`<repo>/.codex/hooks.json` with `--scope project`) and leaves handlers of your own in that file
alone. By hand, merge the `hooks` object of [`hooks/codex.json`](../hooks/codex.json) into the
same file. It is what setup writes at project scope. Either way Codex runs new hooks only after
you review and trust them once, with `/hooks` in a session. Codex's own parser accepts both
forms (the app server's `hooks/list` reports the hooks without warnings).

**Tested with codex-cli 0.164.0 itself, without a model.** `bench/codex/wiring.py` runs
`codefast setup codex --hooks --apply` in a sandbox home and then `codex exec` against a local
stand-in for the model, which answers from a script and keeps every request Codex sends. What
those requests show:

- Codex fires `SessionStart`, `UserPromptSubmit`, `PreToolUse` and `PostToolUse` (after a shell
  command and after `apply_patch`), and the text of each hook is in the next request as a
  developer message. The card for a prompt comes right after the prompt.
- The payloads have the documented shape. The shell is reported as `Bash` with the command as
  one string, also when the model reaches it through Codex's `exec` tool.
- Hooks run outside the session's sandbox and with the environment Codex was started in. So the
  start hook builds a missing index in a read-only session, and `CODEFAST_HOOKS=off` reaches
  them.
- The command line answers inside the read-only sandbox, also about a file that changed after
  the index was written. It cannot store the refreshed part there and works it out again per
  call.
- The five MCP tools answer. For the default model of this Codex they are not in the request at
  all: Codex gives that model one `exec` tool that runs JavaScript, and the server's tools are
  functions it can call there (`tools.mcp__codefast__search`). For a model entry that lists
  tools one by one (`-m gpt-5`), their definitions are about 4,600 bytes of every request.
- What the plugin adds to every request is the skill's line in the list of skills (620 bytes)
  and the line of the start hook (168 bytes).

What a model in Codex then does with the cards was not measured: that needs a model session,
and every figure in `bench/tier2/LIVE.md` is from Claude Code.

MCP server only, by hand (**tested in a sandbox**):

```sh
codex mcp add codefast -- codefast mcp
```

### Gemini CLI

Extension: MCP server (started with `--root ${workspacePath}`), the skill, and `GEMINI.md` as
context. No hooks. **Documented only**. The manifest fields were checked against the extension
loader's source.

```sh
gemini extensions install https://github.com/YashSharmaa/codefast --auto-update
```

### Cursor

Plugin (Agent Plugins format): skill and MCP server. The manifests are **validated against a
schema** (Agent Plugins 1.0.0). The install steps are **documented only**. Install from the
Cursor marketplace once listed, or locally:

```sh
git clone https://github.com/YashSharmaa/codefast ~/.cursor/plugins/local/codefast
```

and run "Developer: Reload Window". MCP server only, in `.cursor/mcp.json` or `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "codefast": { "type": "stdio", "command": "codefast", "args": ["mcp", "--root", "${workspaceFolder}"] }
  }
}
```

### GitHub Copilot CLI and VS Code

Plugin (Agent Plugins format): skill and MCP server. Manifests **validated against a schema**,
commands **documented only**.

```sh
copilot plugin install YashSharmaa/codefast
```

VS Code: run "Chat: Install Plugin from Source" and give the repository URL. Plugins installed
with the Copilot CLI are found too. MCP server only: `copilot mcp add codefast -- codefast mcp`,
or by hand in `~/.copilot/mcp-config.json`, which the Copilot CLI and VS Code share:

```json
{
  "mcpServers": {
    "codefast": { "type": "stdio", "command": "codefast", "args": ["mcp"], "tools": ["*"] }
  }
}
```

VS Code's older `.vscode/mcp.json` uses the key `servers` instead of `mcpServers` and accepts
`"args": ["mcp", "--root", "${workspaceFolder}"]`.

### OpenCode

**Documented only.** MCP server, then the skill:

```sh
opencode mcp add codefast -- codefast mcp
npx skills add YashSharmaa/codefast -a opencode
```

Or in `opencode.json`: `{ "mcp": { "servers": { "codefast": { "type": "local", "command": ["codefast", "mcp"] } } } }`.
`codefast setup opencode --apply` edits that file itself and never runs `opencode mcp add`. In a
file that does not use the `mcp.servers` layout yet it writes the older `mcp.codefast` form with
`"enabled": true`, which OpenCode documents as still supported. For the rules block, OpenCode V2
reads `AGENTS.md` only and does not fall back to `CLAUDE.md` (its instructions page, read on
2026-11-13), so `codefast setup agents-md` reaches it through `AGENTS.md`.

### Qwen Code

Extension (it loads the Agent Plugins manifest natively): skill and MCP server. Manifests
**validated against a schema**, command **documented only**.

```sh
qwen extensions install YashSharmaa/codefast
```

### Goose

**Documented only.** The MCP server is a Goose extension. Add to `~/.config/goose/config.yaml`:

```yaml
extensions:
  codefast:
    name: codefast
    type: stdio
    cmd: codefast
    args: [mcp]
    enabled: true
    timeout: 300
```

The skill: `goose plugin install https://github.com/YashSharmaa/codefast.git` (Goose takes the skill from
a plugin and ignores its MCP server).

### Amp

**Documented only.**

```sh
amp mcp add codefast -- codefast mcp
amp skill add YashSharmaa/codefast
```

### Cline

**Documented only.**

```sh
cline mcp add codefast --yes -- codefast mcp
npx skills add YashSharmaa/codefast -a cline
```

### Kilo Code

**Documented only.** In `~/.config/kilo/kilo.jsonc` or the project's `kilo.jsonc`:

```jsonc
{ "mcp": { "codefast": { "type": "local", "command": ["codefast", "mcp"], "enabled": true } } }
```

The skill: `npx skills add YashSharmaa/codefast -a kilo`.

### Factory Droid

Plugin, translated by Droid from the Claude Code layout. **Documented only.**

```sh
droid plugin marketplace add YashSharmaa/codefast
droid plugin install codefast@codefast --scope user
```

### Crush

**Documented only.** In `~/.config/crush/crushrc` or `./.crushrc`:

```sh
mcp add codefast --command codefast --args mcp
```

The skill: `npx skills add YashSharmaa/codefast -a crush`.

### Pi

Pi has no MCP client. It uses the skill and the command. **Documented only.**

```sh
pi install git:github.com/YashSharmaa/codefast
```

### Zed

**Documented only.** In `settings.json`:

```json
{ "context_servers": { "codefast": { "command": "codefast", "args": ["mcp"], "env": {} } } }
```

### Devin Desktop

Plugin (Devin reads the Claude Code layout): skill, MCP server, and this repository's `AGENTS.md`
as a rule. **Documented only.**

```sh
devin plugins install YashSharmaa/codefast
```

MCP server only (**documented only**, Devin CLI documentation read on 2026-11-13, not run):

```sh
devin mcp add -s user codefast -- codefast mcp
```

or by hand in `~/.config/devin/mcp_config.json`, or `.devin/mcp_config.json` for the project.
Devin Desktop names the same three files (with `.devin/mcp_config.local.json`) in its own
documentation:

```json
{ "mcpServers": { "codefast": { "command": "codefast", "args": ["mcp"] } } }
```

`codefast setup devin` runs the command when `devin` is on `PATH` and writes the file otherwise.
`devin mcp list` shows the result.

### Junie

**Documented only.** In `.junie/mcp/mcp.json` (project or user):

```json
{ "mcpServers": { "codefast": { "command": "codefast", "args": ["mcp"] } } }
```

The skill: copy `skills/codefast` to `.junie/skills/codefast`, or `npx skills add YashSharmaa/codefast -a junie`.

### Deep Agents Code

**Documented only.** In `<project>/.mcp.json` or `~/.deepagents/.mcp.json`:

```json
{ "mcpServers": { "codefast": { "command": "codefast", "args": ["mcp"] } } }
```

The skill: `npx skills add YashSharmaa/codefast -a deepagents -g -y`.

### DeepSeek Harness (dsh)

**Documented only.** Settings, MCP: add a stdio server with command `codefast` and argument `mcp`.
The skill is found in `.agents/skills/codefast` (`npx skills add YashSharmaa/codefast`).

### Kiro

Power (Kiro Powers follow the Agent Plugins format): skill and MCP server. Manifests **validated
against a schema**, steps **documented only**. Install the Power from
`https://github.com/YashSharmaa/codefast`. MCP server only, in `.kiro/settings/mcp.json`:

```json
{ "mcpServers": { "codefast": { "command": "codefast", "args": ["mcp"] } } }
```

The skill: `codefast setup kiro` copies it to `~/.kiro/skills/codefast` (or `.kiro/skills/codefast`
with `--scope project`). Kiro's skills page (read on 2026-11-13, not run) lists `~/.kiro/skills`
for the IDE only: Kiro CLI reads the project's `.kiro/skills`.

### Antigravity

Plugin (Agent Plugins format): skill and MCP server. Manifests **validated against a schema**,
command **documented only**.

```sh
agy plugin install https://github.com/YashSharmaa/codefast
```

MCP server only (**documented only**, read on 2026-11-13, not run): Antigravity, both the IDE and
the CLI, reads `~/.gemini/config/mcp_config.json` and the workspace's `.agents/mcp_config.json`:

```json
{ "mcpServers": { "codefast": { "command": "codefast", "args": ["mcp"] } } }
```

`codefast setup antigravity` writes that entry, and the skill into `~/.gemini/config/skills` (or
`.agents/skills` with `--scope project`). Check it with `/mcp` in a session.

### Aider

Aider has no skills, plugins or MCP. Give it the instruction block and let it ask you to run
the command:

```sh
codefast setup aider --apply
```

This puts the rules block into the project's `AGENTS.md` and adds `read: [AGENTS.md]` to
`.aider.conf.yml` (it keeps a `read:` entry that is already there). By hand:
`codefast setup agents-md --apply`, and in `.aider.conf.yml`: `read: AGENTS.md`. The two commands
are **tested in a sandbox** as far as the files they write. Aider itself was not run
(**documented only**).

### Hermes Agent

**Documented only** (Hermes documentation read on 2026-11-13, not run). Hermes keeps MCP servers
in `~/.hermes/config.yaml` (`$HERMES_HOME/config.yaml`) and has no command that adds an arbitrary
one, so `codefast setup hermes` edits that file:

```yaml
mcp_servers:
  codefast:
    command: codefast
    args: [mcp]
```

Check it with `hermes mcp test codefast`, and send `/reload-mcp` to a session that is already
running. There is no project file for servers. The skill goes into `~/.hermes/skills/codefast`.
A repository's `.agents/skills` is read only after `hermes skills trust` in it, and
`~/.agents/skills` only when it is listed under `skills.external_dirs`.

### OpenClaw

**Documented only** (read on 2026-11-13, not run). Servers live under `mcp.servers` in
`~/.openclaw/openclaw.json`, and there is no file per repository:

```sh
openclaw mcp add codefast --command codefast --arg mcp
openclaw mcp doctor codefast --probe
```

`codefast setup openclaw` runs the first command when `openclaw` is on `PATH` and otherwise adds
`"codefast": {"command": ..., "args": ["mcp"]}` under `mcp.servers` itself. That file is JSON5. A
file written with JSON5 syntax beyond comments and trailing commas (unquoted keys, single quotes)
is left alone and reported as `manual`. The skill goes into `~/.agents/skills`, which OpenClaw
reads.

### Mistral Vibe

**Documented only** (read on 2026-11-13, not run).

```sh
vibe mcp add codefast --transport stdio --command codefast --arg mcp
```

That writes a table to `~/.vibe/config.toml` (`$VIBE_HOME`). The same by hand, or in the
project's `.vibe/config.toml`, which Vibe reads only in a folder you have trusted:

```toml
[[mcp_servers]]
name = "codefast"
transport = "stdio"
command = "codefast"
args = ["mcp"]
```

Check it with `/mcp` in a session. The skill goes into `.agents/skills`, which Vibe reads at both
scopes (the project's only in a trusted folder).

### Continue

**Documented only** (read on 2026-11-13, not run). Continue uses MCP tools in agent mode only. For
a project, `codefast setup continue --scope project` writes `.continue/mcpServers/mcp.json`, a
file in the JSON format of other clients, which Continue picks up from that folder:

```json
{ "mcpServers": { "codefast": { "command": "codefast", "args": ["mcp"] } } }
```

For the user, add an item to the `mcpServers` list of `~/.continue/config.yaml`, which the IDE
extensions and `cn` share. Setup prints it and does not edit that list:

```yaml
mcpServers:
  - name: codefast
    command: codefast
    args: [mcp]
```

Continue's documentation names no skills directory, so setup copies no skill.

### OpenHands CLI

**Documented only** (read on 2026-11-13, not run). The OpenHands CLI says of itself that it is no
longer actively maintained.

```sh
openhands mcp add codefast --transport stdio codefast -- mcp
openhands mcp list
```

or by hand in `~/.openhands/mcp.json`, in the `mcpServers` format. There is no project file. The
skill goes into `.agents/skills`.

### Warp

**Documented only** (read on 2026-11-13, not run). Warp reads `~/.warp/.mcp.json` and the
project's `.warp/.mcp.json` in the `mcpServers` format, and also the files of Claude Code and
Codex (`~/.claude.json`, `.mcp.json`, `~/.codex/config.toml`, `.codex/config.toml`), so a server
registered for those is listed too. Warp starts its own global servers by itself. Those from
other agents' files need "Auto-spawn servers from third-party agents", and every project server
has to be switched on in the MCP servers page, once per session. There is no shell command for
it. The skill goes into `.agents/skills`.

### Auggie (Augment CLI)

**Documented only** (read on 2026-11-13, not run).

```sh
auggie mcp add codefast --command codefast --args "mcp"
auggie mcp list --json
```

That writes `~/.augment/settings.json`. Add `--project` for `.augment/settings.json`. The skill
goes into `.agents/skills`.

### CodeBuddy Code

**Documented only** (Tencent's documentation read on 2026-11-13, not run).

```sh
codebuddy mcp add --scope user codefast -- codefast mcp
codebuddy mcp get codefast
```

User servers are read from the first of `~/.codebuddy/.mcp.json`, `~/.codebuddy/mcp.json` and
`~/.codebuddy.json` that exists, project servers from `.mcp.json`. The entry carries
`"type": "stdio"`. The skill goes into `~/.codebuddy/skills/codefast` or `.codebuddy/skills/codefast`.

### Kimi Code CLI

**Documented only** (read on 2026-11-13, not run). Kimi Code documents no shell command for MCP
servers (`/mcp-config` in a session edits them). Add the server to `~/.kimi-code/mcp.json`
(`$KIMI_CODE_HOME`) or the project's `.kimi-code/mcp.json`:

```json
{ "mcpServers": { "codefast": { "command": "codefast", "args": ["mcp"] } } }
```

The project file is read only in a trusted folder. A headless run needs
`KIMI_CODE_TRUST_WORKSPACE=1` for that. Check it with `/mcp` in a session. The skill goes into
`.agents/skills`.

### JetBrains AI Assistant

**Documented only** (read on 2026-11-13, not run). AI Assistant (not Junie) keeps its MCP servers
inside the IDE, and no file for them is documented, so there is no setup target. Run
`codefast setup mcp-json`, then paste what it prints under Settings, Tools, AI Assistant, Model
Context Protocol (MCP), with Add, and choose global or project level there.

### Any other agent with Agent Skills

```sh
npx skills add YashSharmaa/codefast          # -g for all projects, -a AGENT for one agent
```

or copy `skills/codefast` into the directory the agent reads (`.agents/skills/` in most of them).
**Tested in a sandbox** with skills 1.8.1 and a local copy of this repository in place of
`YashSharmaa/codefast`: it finds the one skill, and `-a opencode`, `-a cline` and `-a kilo` put it in
`.agents/skills/codefast`, `-a crush` in `.crush/skills/codefast`, `-a junie` in
`.junie/skills/codefast`, and `-a deepagents -g -y` in `~/.agents/skills/codefast`. The agents
themselves were not run. The skill also passes the reference validator of the Agent Skills
specification (`skills-ref` 0.1.1).

### Any other MCP client

```json
{ "mcpServers": { "codefast": { "command": "codefast", "args": ["mcp"] } } }
```

`codefast setup mcp-json` prints this snippet with the binary's full path.

The server takes the repository from, in order: the `root` argument of a tool call, `--root DIR`
on its command line, `CODEFAST_ROOT`, `CLAUDE_PROJECT_DIR`, and the directory it was started in.
Whichever it is, a directory inside a git work tree (or inside a directory that already has a
codefast index) means that whole repository, as it does for the command line, so a session
started in a subdirectory does not get an index of its own. Three cases are refused, with an
error that tells the model what to pass:

- the home directory, or a directory above it, unless it is a git work tree or has an index
  (`codefast index`, run there once, opts a directory in).
- a working directory that is not inside a git work tree, when nothing else names the repository.
- the plugin's own directory, which is where Agent Plugins clients such as Codex start a
  plugin's server. The server then says in its instructions that every call needs `root`.

So an entry like the one above works as it is in a harness that starts the server in the
project (Claude Code and Codex do, **tested in a sandbox**). Where a harness starts it somewhere
else and has a variable for the workspace, name it: `"args": ["mcp", "--root", "${workspaceFolder}"]`
in Cursor and VS Code. `codefast setup` writes the plain form and keeps a `--root` that you add.

Claude Code and Codex connect to the server and list its five tools (`search`, `show`, `refs`,
`path`, `grep`). For troubleshooting in Claude Code, `MCP_PROTOCOL_NEGOTIATION=legacy` makes it
open with the older handshake instead of the stateless revision 2026-07-28. For the MCP
Registry, [`server.json`](../server.json) is **validated against a schema** (2025-12-11).

## 4. Hooks

Hooks are optional: codefast works without them. They only add context, never block or rewrite a
tool call, and `CODEFAST_HOOKS=off` turns them off. The variable has to reach the hook: Claude
Code and Codex both run hooks with your environment (Codex seen with codex-cli 0.164.0), and in
Codex `/hooks` disables a hook as well.

What they add, each said once per session:

- At the start of a session, one line saying that the repository is indexed (the index is built
  in the background when there is none).
- For a prompt that names a symbol the index can pin down: where it is defined, the sentence its
  author wrote about it, and how it is used. A definition of up to 25 lines comes whole, with
  its line numbers. Where the function is stored in a table or handed over as a callback, the
  card says where, and which calls reach it through that table. Its direct call sites are
  listed when nothing else in the repository has its name, and counted otherwise, with the
  command that prints each one. At most two symbols, and the failing test when the prompt
  names one.
- For a prompt that asks one of three questions about a symbol it names, the answer itself, as
  the command for it prints it: what a function calls ("What does `vfs_get_tree()` call?"), how
  one function reaches another ("How does `submit_bio()` get to `blk_mq_submit_bio()`?", with
  the place of every hop), and where a member is written ("Which functions set the `mnt_ns`
  field of `struct mount`?"). The question is read from the shape of the sentence, in a prompt
  of up to 2,000 bytes. A search for a call path that takes more than half a second of work is
  given up, and a path that is not found is not mentioned.
- Where a name has several definitions (one per architecture, one per driver) and a word of
  the same sentence is a directory of exactly one of them, the card is about that one and says
  how it was picked: "of 22 definitions of `copy_thread`, the one whose path has `x86`, a word
  of the prompt". Without such a word there is no card. For a slot of an operations table, the
  same words pick its bindings: "of its 104 bindings, under `ext4`: `ext4_file_read_iter`".
- For a prompt that quotes a message (an error text, a log line): the line that writes it, the
  function around that line and who calls it. Only when the words stand in one string literal
  of the source and in no other file.
- After a `grep`, `rg`, `find -name` or the harness's own search for one known symbol: where it
  is defined and how it is used. When the search itself printed the definition, only what it
  did not print (what the function is for, who calls it), and nothing when there is no such
  news.
- After an edit (the harness's edit tools, or a shell command that may have written files): the
  places that still use something the edit removed, renamed or gave another signature, and
  calls of a member that nothing in the repository defines. An edit that leaves nothing behind
  adds nothing.
- Before a whole-file read of a source file longer than 400 lines, one line naming the outline
  command.
- After a compaction, the definitions the uncommitted changes added, removed and altered.

Once a week per project, Claude Code also shows you (not the model) one line when plugins and
skills that nothing in the project matches load more than about 1,500 tokens into every
request.

The index that hooks keep up to date is refreshed in the background on half of the machine's
cores. `CODEFAST_THREADS=N` sets the number of threads for every codefast command, the
background refresh included.

### What the harness itself loads

`codefast doctor context` reads the configuration of Claude Code and of Codex and prints what
each part puts into every request: plugins, skills, sub-agents, instruction files and the memory
index. It sets them against what the project contains, names the parts nothing in the project
matches, and prints the setting that switches each off (per project where the harness has such
a switch). It changes nothing.

For Claude Code the estimate was compared with the harness's own `/context` on one real
configuration (7 plugins, 43 skills, 3 loaded files): the total was 4% higher. That comparison
found two things, which the report now handles: an `AGENTS.md` is not loaded unless an
instruction file imports it, and the skills of the signed-in claude.ai account are loaded too.
For Codex the figures rest on its documentation, since Codex has no command that shows them.
The parts that are codefast's own were read from a real request (`bench/codex/wiring.py`, see
the Codex section).

| Harness | How the hooks are registered |
|---|---|
| Claude Code, with the plugin | Automatically (`hooks/claude-code.json`). **Tested in a sandbox**: `claude plugin details` lists the four events, and each command line was run by hand from the plugin cache with a payload in the shape Claude Code documents. |
| Claude Code, without the plugin | `codefast setup claude-code --hooks --apply` writes them into `~/.claude/settings.json` (`.claude/settings.json` with `--scope project`), next to hooks of your own. **Tested in a sandbox** the same way. With the plugin installed setup leaves them out: both copies would run. |
| Codex, with or without the plugin | `codefast setup codex --hooks --apply`, or merge `hooks/codex.json` into `~/.codex/hooks.json` by hand. Then trust them in `/hooks`. **Tested with Codex itself** (see the Codex section). |
| Others | Setup registers no hooks. `codefast hook` also understands the payloads of Cursor (`sessionStart`, `postToolUse`) and Gemini CLI (`SessionStart`, `AfterTool`), built from their documentation. Nothing in this repository registers those, and neither harness was run. |

In a real session of Claude Code 2.1.323 (headless, the plugin loaded with `--plugin-dir`, 2026-11-13)
the session-start hook and the hook after a search fired, and their text reached the model. In
codex-cli 0.164.0 every hook fired and its text was in the next request (2026-11-14, with a
stand-in for the model: `bench/codex/wiring.py`).

### Where the tokens of past sessions went

`codefast doctor sessions` reads the transcripts Claude Code and Codex keep on this machine and
reports where the tokens of this project's sessions went and which habits cost the most, one
report per harness that has sessions here, Claude Code first. Nothing is run or changed, nothing
leaves the machine, and no model is called.

| Harness | Where | Which sessions are the project's |
|---|---|---|
| Claude Code | `$CLAUDE_CONFIG_DIR/projects/`, `~/.claude/projects/` when the variable is unset | those in the directory named after the project's path |
| Codex | `$CODEX_HOME/sessions/`, `~/.codex/sessions/` when the variable is unset | those whose `session_meta` names the project's directory, or a directory inside it, as `cwd` |

Every figure is in weighted tokens: one uncached input token of the request's own model, with
each kind of token weighed as that model's published price weighs it (Anthropic's price list,
read 2026-11-15). Output is 5 and a cache write 1.25 (2 for the one-hour cache) for every
model. A cache read is 0.1, except 0.05 on Claude Opus 5.5 and Sonnet 5.5 and 0.025 on Claude
Fable 5.2 and 5.1 and Mythos 5.2 and 5.1. Claude Haiku 5.5 steps up: a request whose prompt
(cache reads and writes included) is over 100,000 tokens pays every kind of token five times as
much. A model the table does not name, every Codex model among them, is weighed 0.1, 1.25, 2
and 5. The line under the title names the weights in play. Dollars are not printed, since
prices change. The report gives the split of the main thread
and of the sub-agents, then each finding with its cost and share, largest first, and the one
thing to do about it. A finding under 1% of the total is left out.

| Finding | What it counts |
|---|---|
| the start | the context of the first request of each stretch between compactions, read again by every later request of the stretch (`doctor context` shows what it holds) |
| cold restarts | a request that read under 20% of the previous context from the cache, by cause: a model switch, more than an hour idle, more than five minutes idle, other. Codex has no one-hour cache to tell apart, so for it the idle cause is five minutes. Where Claude Code recorded its own reason for the miss, `--session` prints it next to the cause, and the report says when the two differ for more than a quarter of the restarts |
| compactions | manual and automatic ones, the conversation size at each, and the manual ones under 100k tokens. A compaction costs three things: a request reads the whole conversation, it writes the summary as output, and the first request after it sends what is left as new. The transcript does not record that first request, so its part is an estimate from the size of the summary. What the next request sent is measured. Codex records neither what started a compaction nor its sizes: the context of the requests either side of it stands in for them |
| thinking | thinking tokens as a share of the output, where the transcript records them. For Codex these are `reasoning_output_tokens`, taken as a part of `output_tokens` |
| carried context | what compacting earlier would have done, tried at seven lengths from 150k to 800k tokens (`--carry N` tries one). At each, the context drops to what the session's own compactions left (its start and 50k tokens without one) and grows again as it did, which counts the compactions more and what each request then costs less. A compaction costs what the session's own did (without one: a summary of 6k tokens and 50k sent as new). The report names the longest length that gains at least four fifths of what the best one gains, since every compaction loses some of the conversation, and the best one beside it. An estimate: it takes the work after a compaction to go as it did |
| cold restarts in sub-agents | the same for each sub-agent that has a transcript of its own, which is a conversation of its own. A sub-agent of Claude Code writes to the five-minute cache: one that waits longer than that on a command sends its whole conversation again when it comes back |
| carried context in sub-agents | what compacting earlier would have done in the sub-agents' own conversations, tried and reported as for the main thread |
| price step | for Claude Haiku 5.5: the requests whose prompt was over 100k tokens, what they cost, and what they would have cost at the lower price. For these sessions an earlier compaction is tried at 100k, and reported here |
| repeated reads | a file read again within a stretch that came back word for word as before. For Codex: a call with the same tool and arguments that gave back the same output |

It also lists the tools whose results weighed most (Bash and Codex's shell by the program they
ran, MCP tools by server) and the three heaviest single results. `--days N` sets the window (default 14 days,
by when a transcript last changed), `--session ID` breaks one session down request by request,
`--all` gives one line per project and harness and a total per harness, and `--json` prints the
same numbers as JSON, one object per harness. Counts that an older harness version did not
write, such as thinking tokens, are reported as not recorded rather than as zero.

In Claude Code three notes go to you and not to the model, so they cost no tokens. Their figures
are those of the model the conversation runs on. Each is said once per occasion, and
`CODEFAST_COST_NOTES=off` turns them off:

- When a conversation is so long that a compaction would pay for itself within ten requests:
  what every request now pays to read it again, and after how many requests a `/compact` has
  paid for itself. With the one-hour cache that is from 230,000 tokens at the default prices,
  from 370,000 on Claude Opus 5.5 and Sonnet 5.5 and from 660,000 on Claude Fable 5.2, where a
  cache read costs less. It comes back each time the conversation has grown by another 100,000
  tokens. `CODEFAST_CARRY=N` sets another length.
- When a conversation on Claude Haiku 5.5 comes within a fifth of 100,000 tokens, and again when
  it is over: that a request whose prompt is over that length pays five times the price for
  every token of it.
- When you come back to a conversation of 60,000 tokens or more after its prompt cache has run
  out (an hour, or five minutes where the transcript shows the five-minute cache), or resume
  such a conversation: that the next request sends all of it again as new input. That is 20
  times the price of a warm request at the default prices, 40 times on Claude Opus 5.5 and
  Sonnet 5.5 and 80 times on Claude Fable 5.2 (12, 25 and 50 with the five-minute cache). On
  resuming, the harness's own estimate in dollars is passed on where it gives one. `/clear` or a
  new session avoids it when the next job does not need the history.

`codefast doctor sessions` gives the same figures after the fact, for every session of the
project.

## 5. Release assets

The launcher and `install.sh` expect a release `vX.Y.Z` to carry:

| Asset | Content |
|---|---|
| `codefast-aarch64-apple-darwin.tar.gz`, `codefast-x86_64-apple-darwin.tar.gz`, `codefast-aarch64-unknown-linux-musl.tar.gz`, `codefast-x86_64-unknown-linux-musl.tar.gz` | The binary, named `codefast`, at the top of the archive. |
| `SHA256SUMS` | One `sha256sum` line per asset. |

They are fetched from `https://github.com/YashSharmaa/codefast/releases/download/vX.Y.Z/` (the launcher,
for its own version) and `.../releases/latest/download/` (`install.sh`). When the plugin itself
contains `bin/SHA256SUMS`, the launcher trusts that file instead of the published one, which pins
the binaries to the plugin's content. The npm platform packages carry the same binaries, one each.

Gemini CLI can install a release archive instead of cloning the repository. If you publish one,
name it `{platform}.{arch}.codefast.tar.gz` (for example `darwin.arm64.codefast.tar.gz`) and put
`gemini-extension.json`, `GEMINI.md`, `skills/` and the real binary as `bin/codefast` at its top.

Once the repository has a GitHub release, Gemini CLI installs from the latest release rather
than from a clone. Without archives of its own it takes the source archive of the tag, which
has the same content. A release with exactly one asset is the exception: Gemini CLI then takes
that asset for the extension, whatever it is (`findReleaseAsset` in its
`packages/cli/src/config/extensions/github.ts`, read on 2026-11-13). So never leave a release
with a single asset, for example `SHA256SUMS` alone while the binaries are still uploading:
create the release as a draft and publish it when every asset is there.
