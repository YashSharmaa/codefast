# Installing codefast

codefast is one binary. A coding harness uses it in up to four ways:

- **the skill**: a short guide (`skills/codefast/SKILL.md`) that tells the agent to run the
  `codefast` command in its shell. This is the main path and costs about 140 tokens per session.
- **the MCP server**: the same binary started as `codefast mcp`, for harnesses that prefer tools.
- **hooks**: optional. They add a line of context after a search or before a long file read.
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
sh install.sh --version 0.1.1           # a specific release instead of the latest
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
npx -y @yashsharmaa/codefast@0.1.1 mcp        # or: npm install -g @yashsharmaa/codefast
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
```

`codefast setup [TARGET ...] [OPTIONS]` registers codefast with coding harnesses: the MCP server
and the skill, and the hooks when asked. It runs the harness's own command where there is one
(`claude mcp add`, `codex mcp add`, `gemini mcp add`, `qwen mcp add`, `copilot mcp add`,
`amp mcp add`, `cline mcp add`, `droid mcp add`, `code --add-mcp`) and that program is on `PATH`.
Otherwise it edits the configuration file the harness documents, keeping its comments, order
and formatting. `docs/HARNESSES.md` lists the file for every harness and scope.

It changes something only when a target is named (or `--all`) **and** either `--apply` is given
or its output goes to a terminal. In every other case, an agent's shell or a script among them,
it prints the plan and ends with "Dry run: nothing was changed". `--print` asks for the plan
whatever the circumstances.

| Target | What it is |
|---|---|
| `claude-code`, `codex`, `gemini-cli`, `cursor`, `copilot` (VS Code), `copilot-cli`, `opencode`, `qwen-code`, `goose`, `amp`, `cline`, `kilo-code`, `droid`, `crush`, `zed`, `kiro`, `junie`, `deepagents`, `pi`, `devin`, `antigravity`, `dsh`, `aider` | A harness. Also given as `--harness ID`, several are fine, and common names work too (`claude`, `gemini`, `vscode`, `kilo`, `qwen`, `factory`, `windsurf`, `dcode`, `agy`). |
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
Codex app server listed the skill, the server's five tools and the three hooks without warnings.
For every other harness setup writes the file format that harness documents, and its own tests
read the result back. None of those files has been loaded by the harness itself, and the
commands of `gemini`, `qwen`, `copilot`, `amp`, `cline`, `droid` and `code` were only exercised
with stand-in programs that record how they were called.

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
forms (the app server's `hooks/list` reports the three hooks without warnings), and each command
line was run by hand with a payload in the shape Codex documents. No hook was fired by Codex
itself, because that needs a model session.

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
`"enabled": true`, which OpenCode documents as still supported.

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

MCP server only: `devin mcp add codefast -- codefast mcp`.

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

### Antigravity

Plugin (Agent Plugins format): skill and MCP server. Manifests **validated against a schema**,
command **documented only**.

```sh
agy plugin install https://github.com/YashSharmaa/codefast
```

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
Code runs hooks with your environment. Whether Codex does was not verified, and there `/hooks`
disables a hook in any case.

What they add: at the start of a session, two lines saying that the repository is indexed (the
index is built in the background when there is none). After a `grep`, `rg`, `find -name` or the
harness's own search for one known symbol, where it is defined and who calls it. Before a
whole-file read of a source file longer than 400 lines, one line naming the outline command.
Each is said once per session.

| Harness | How the hooks are registered |
|---|---|
| Claude Code, with the plugin | Automatically (`hooks/claude-code.json`). **Tested in a sandbox**: `claude plugin details` lists the four events, and each command line was run by hand from the plugin cache with a payload in the shape Claude Code documents. |
| Claude Code, without the plugin | `codefast setup claude-code --hooks --apply` writes them into `~/.claude/settings.json` (`.claude/settings.json` with `--scope project`), next to hooks of your own. **Tested in a sandbox** the same way. With the plugin installed setup leaves them out: both copies would run. |
| Codex, with or without the plugin | `codefast setup codex --hooks --apply`, or merge `hooks/codex.json` into `~/.codex/hooks.json` by hand. Then trust them in `/hooks`. **Tested in a sandbox** (see the Codex section). |
| Others | Setup registers no hooks. `codefast hook` also understands the payloads of Cursor (`sessionStart`, `postToolUse`) and Gemini CLI (`SessionStart`, `AfterTool`), built from their documentation. Nothing in this repository registers those, and neither harness was run. |

In a real session of Claude Code 2.1.323 (headless, the plugin loaded with `--plugin-dir`, 2026-11-13)
the session-start hook and the hook after a search fired, and their text reached the model. No
hook has yet been fired by Codex itself.

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
