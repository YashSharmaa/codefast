#!/bin/sh
# Installs this repository as a plugin into the coding harnesses found on this machine, using
# throwaway configuration directories, and checks what each harness loaded.
#
#   tests/packaging/manifests/harness_sandbox_test.sh /path/to/a/built/codefast [WORK_DIR]
#
# Nothing of yours is touched: every harness runs under `env -i` with HOME, CLAUDE_CONFIG_DIR
# and CODEX_HOME inside WORK_DIR, so it sees no credentials and no existing configuration.
# No model is called and no session is started: the checks use each harness's own plugin
# commands, `claude mcp list`, and the Codex app server's listing requests.
# The repository is staged as a git repository of its own (the files git would track) and
# served from 127.0.0.1, so the harnesses clone it the way they would clone it from a forge.
#
# Environment: CLAUDE_BIN and CODEX_BIN name the harness binaries (default: `claude` and
# `codex` on PATH). A harness that is not installed is skipped.

set -u

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P) || exit 2
repo=$(CDPATH='' cd -- "$here/../../.." && pwd -P) || exit 2
real=${1:-}
if [ -z "$real" ] || [ ! -x "$real" ]; then
    echo "usage: $0 /path/to/codefast [WORK_DIR]" >&2
    exit 2
fi
case $real in
/*) ;;
*) real=$(pwd)/$real ;;
esac
work=${2:-}
if [ -z "$work" ]; then
    work=$(mktemp -d "${TMPDIR:-/tmp}/codefast-harness-test.XXXXXX") || exit 2
fi
mkdir -p "$work" || exit 2
work=$(CDPATH='' cd -- "$work" && pwd -P) || exit 2
rm -rf "$work/h"
work=$work/h
mkdir -p "$work/home" "$work/tmp" "$work/proj" "$work/www" "$work/stage"

claude_bin=${CLAUDE_BIN:-$(command -v claude 2>/dev/null || true)}
codex_bin=${CODEX_BIN:-$(command -v codex 2>/dev/null || true)}
tool_path=$(dirname "$(command -v node 2>/dev/null || echo /usr/bin/node)"):$(dirname "$(command -v git)"):$(dirname "$(command -v python3)"):/usr/bin:/bin:/usr/sbin:/sbin

pass=0
fail=0
ok() {
    pass=$((pass + 1))
    printf 'ok    %s\n' "$1"
}
bad() {
    fail=$((fail + 1))
    printf 'FAIL  %s\n' "$1"
    [ $# -gt 1 ] && printf '      %s\n' "$2"
}
check() {
    name=$1
    shift
    if "$@"; then ok "$name"; else bad "$name" "$(head -c 600 "$work/out" 2>/dev/null)"; fi
}
has() { grep -F -q -- "$2" "$1"; }

# Runs a command with nothing in its environment but the sandbox.
clean() {
    env -i HOME="$work/home" TMPDIR="$work/tmp" TERM=dumb PATH="$tool_path" \
        GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null \
        DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 "$@"
}

# ----- stage the repository as its own git repository, and serve it -----
if git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    (cd "$repo" && git ls-files -co --exclude-standard -z | tar --null -T - -cf -) | tar -xf - -C "$work/stage"
else
    (cd "$repo" && tar -cf - --exclude ./target --exclude ./.git .) | tar -xf - -C "$work/stage"
fi
(
    cd "$work/stage" &&
        clean git init -q -b main . &&
        clean git add -A &&
        clean git -c user.name=sandbox -c user.email=sandbox@example.invalid commit -q -m "staged copy" &&
        clean git clone -q --bare "$work/stage" "$work/www/codefast.git"
) || {
    echo "could not stage the repository" >&2
    exit 2
}
clean python3 -I "$here/git_http_server.py" "$work/www" >"$work/port" 2>"$work/server.err" &
# The server prints its port, then its own process id: the id of the background job above is
# only that of the shell that started it.
stop_server() {
    server=$(sed -n 2p "$work/port" 2>/dev/null)
    [ -n "$server" ] && kill "$server" 2>/dev/null
}
trap stop_server EXIT
trap 'exit 130' HUP INT TERM
tries=0
while [ "$(wc -l <"$work/port" | tr -d ' ')" -lt 2 ] && [ "$tries" -lt 50 ]; do
    sleep 0.1 2>/dev/null || sleep 1
    tries=$((tries + 1))
done
port=$(head -1 "$work/port")
url=http://127.0.0.1:$port/codefast.git
echo "# staged $(cd "$work/stage" && clean git ls-files | wc -l | tr -d ' ') files, served at $url"

# A local "release" for the launcher, so that nothing is fetched from the network.
version=$(sed -n 's/^version=//p' "$repo/bin/codefast" | head -1)
target=$(clean sh "$repo/install.sh" --dry-run | sed -n 's/^platform: *//p')
asset=codefast-$target.tar.gz
mkdir -p "$work/rel/download/v$version" "$work/relstage"
cp "$real" "$work/relstage/codefast"
(cd "$work/relstage" && tar -czf "$work/rel/download/v$version/$asset" codefast)
(cd "$work/rel/download/v$version" && { sha256sum "$asset" 2>/dev/null || shasum -a 256 "$asset"; } >SHA256SUMS)

(cd "$work/proj" && clean git init -q . && printf 'int add(int a, int b) { return a + b; }\nint main(void) { return add(1, 2); }\n' >main.c)

# ----- Claude Code -----
if [ -n "$claude_bin" ] && [ -x "$claude_bin" ]; then
    cc="$work/claude"
    mkdir -p "$cc"
    claude() { (cd "$work/proj" && clean CLAUDE_CONFIG_DIR="$cc" "$@" >"$work/out" 2>&1); }
    echo "# Claude Code: $(clean CLAUDE_CONFIG_DIR="$cc" "$claude_bin" --version 2>/dev/null)"

    claude "$claude_bin" plugin validate "$work/stage/.claude-plugin/plugin.json" --strict
    check "claude plugin validate --strict (plugin manifest, hooks file, MCP entry)" has "$work/out" "Validation passed"
    claude "$claude_bin" plugin validate "$work/stage/.claude-plugin/plugin.json" --json
    check "claude plugin validate: no errors and no warnings" python3 -I -c "
import json, sys
d = json.load(open('$work/out'))
bad = d['manifest']['errors'] + d['manifest']['warnings'] + [x for c in d['contents'] for x in c['errors'] + c['warnings']]
sys.exit(1 if bad or not d['success'] else 0)"
    claude "$claude_bin" plugin validate "$work/stage" --strict
    check "claude plugin validate --strict (marketplace manifest)" has "$work/out" "Validation passed"

    claude "$claude_bin" plugin install codefast --marketplace "$url"
    check "claude plugin install codefast --marketplace <git url>" has "$work/out" "Successfully installed plugin: codefast@codefast"
    claude "$claude_bin" plugin list
    check "claude plugin list: codefast@codefast is enabled" has "$work/out" "enabled"
    claude "$claude_bin" plugin details codefast
    check "claude plugin details: one skill" has "$work/out" "Skills (1)  codefast"
    check "claude plugin details: the four hook events" has "$work/out" "Hooks (4)  SessionStart, PreToolUse, PostToolUse, PostToolUseFailure"
    check "claude plugin details: one MCP server" has "$work/out" "MCP servers (1)  codefast"
    cached=$(find "$cc/plugins/cache" -path '*/bin/codefast' -type f 2>/dev/null | head -1)
    check "the launcher in Claude Code's plugin cache is executable" [ -x "${cached:-/nonexistent}" ]

    (cd "$work/proj" && clean CLAUDE_CONFIG_DIR="$cc" CODEFAST_BIN="$real" "$claude_bin" mcp list >"$work/out" 2>&1)
    check "claude mcp list: the plugin's server connects (binary named by CODEFAST_BIN)" has "$work/out" "bin/codefast mcp - ✔ Connected"
    rm -rf "$work/home/.cache"
    (cd "$work/proj" && clean CLAUDE_CONFIG_DIR="$cc" CODEFAST_RELEASE_BASE="file://$work/rel" "$claude_bin" mcp list >"$work/out" 2>&1)
    check "claude mcp list: connects when the launcher has to fetch the binary first" has "$work/out" "bin/codefast mcp - ✔ Connected"
    check "the fetched binary is in the sandbox cache" [ -x "$work/home/.cache/codefast/$version/codefast" ]

    # What Claude Code runs for the hooks, by hand: the command from the hooks file with the
    # plugin root filled in, started in the sandbox project with a payload that names it, as
    # Claude Code does. (Started anywhere else, or with a payload that names no directory, a
    # hook answers for the repository around the directory it runs in: this one, when the
    # script is run from a checkout.)
    plugin_root=${cached%/bin/codefast}
    (cd "$work/proj" && clean "$real" index >"$work/out" 2>&1)
    for event in session-start pre-tool-use post-tool-use; do
        line=$(python3 -I -c "
import json
d = json.load(open('$plugin_root/hooks/claude-code.json'))
for groups in d['hooks'].values():
    for g in groups:
        for h in g['hooks']:
            if h['command'].endswith('hook $event'):
                print(h['command'])
" | head -1)
        case $event in
        session-start) payload='{"session_id":"sandbox","cwd":"'$work/proj'","hook_event_name":"SessionStart","source":"startup"}' ;;
        pre-tool-use) payload='{"session_id":"sandbox","cwd":"'$work/proj'","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}' ;;
        *) payload='{"session_id":"sandbox","cwd":"'$work/proj'","hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"grep -rn add ."}}' ;;
        esac
        (cd "$work/proj" && printf '%s' "$payload" | clean CLAUDE_PLUGIN_ROOT="$plugin_root" CLAUDE_PROJECT_DIR="$work/proj" CODEFAST_BIN="$real" sh -c "$line" >"$work/out" 2>"$work/err")
        code=$?
        check "hook command for $event runs from the cache and exits 0" [ "$code" = 0 ]
        check "hook command for $event keeps stderr empty" [ ! -s "$work/err" ]
        case $event in
        session-start) check "hook session-start answers for the sandbox project (one file, two symbols)" has "$work/out" '(1 files, 2 symbols)' ;;
        pre-tool-use) check "hook pre-tool-use has nothing to add to an unrelated command" [ ! -s "$work/out" ] ;;
        *) check "hook post-tool-use names the definition a search was about" has "$work/out" 'add is defined at main.c:1' ;;
        esac
    done
    check "the hooks kept their session notes in the sandbox project's index" [ -d "$work/proj/.codefast/sessions" ]
else
    echo "# Claude Code: not installed, skipped"
fi

# ----- Codex -----
if [ -n "$codex_bin" ] && [ -x "$codex_bin" ]; then
    cx="$work/codex"
    mkdir -p "$cx" "$work/codex-bin"
    # Codex starts a plugin's MCP server with a short list of variables (HOME, PATH, TMPDIR and
    # its own PLUGIN_ROOT and PLUGIN_DATA): CODEFAST_BIN or CODEFAST_RELEASE_BASE set here would
    # never reach the launcher. So codefast is put on PATH, which is what docs/INSTALL.md tells
    # Codex users to do, and the launcher's cache is emptied so that nothing left by the Claude
    # Code checks above can stand in for it.
    cp "$real" "$work/codex-bin/codefast"
    rm -rf "$work/home/.cache"
    codex() { (cd "$work/proj" && clean PATH="$work/codex-bin:$tool_path" CODEX_HOME="$cx" "$@" >"$work/out" 2>"$work/err"); }
    echo "# Codex: $(clean CODEX_HOME="$cx" "$codex_bin" --version 2>/dev/null)"

    codex "$codex_bin" plugin marketplace add "$url" --json
    check "codex plugin marketplace add <git url>" has "$work/out" '"marketplaceName": "codefast"'
    codex "$codex_bin" plugin add codefast@codefast --json
    check "codex plugin add codefast@codefast" has "$work/out" '"pluginId": "codefast@codefast"'
    check "codex installed version $version" has "$work/out" "\"version\": \"$version\""
    codex "$codex_bin" plugin list
    check "codex plugin list: installed, enabled" has "$work/out" "installed, enabled"
    codex "$codex_bin" mcp list --json
    check "codex mcp list: the launcher in the plugin cache, argument mcp" has "$work/out" "/bin/codefast"
    check "codex gives the server PLUGIN_ROOT" has "$work/out" '"PLUGIN_ROOT"'
    cached=$(find "$cx/plugins/cache" -path '*/bin/codefast' -type f 2>/dev/null | head -1)
    check "the launcher in Codex's plugin cache is executable" [ -x "${cached:-/nonexistent}" ]

    cp "$repo/hooks/codex.json" "$cx/hooks.json"
    codex python3 -I "$here/codex_app_server_probe.py" "$codex_bin" "$work/proj" codefast "$cx/.tmp/marketplaces/codefast"
    cp "$work/out" "$work/codex-probe.json"
    probe() {
        python3 -I -c "
import json, sys
d = json.load(open('$work/codex-probe.json'))
hooks = [h for e in d['hooks']['data'] for h in e['hooks']]
problems = [x for e in d['hooks']['data'] for x in e['errors'] + e['warnings']]
skills = [k['name'] for e in d['skills']['data'] for k in e['skills']]
servers = {s['name']: sorted((s.get('tools') or {}).keys()) for s in d['mcp'].get('data', [])}
plugin = d.get('plugin', {}).get('plugin', d.get('plugin', {}))
$1"
    }
    check "codex app server: the skill is listed as codefast:codefast" probe "sys.exit(0 if 'codefast:codefast' in skills else 1)"
    check "codex app server: the MCP server started through the launcher (codefast on PATH) and listed its tools" probe "sys.exit(0 if servers.get('codefast') else 1)"
    check "the launcher fetched nothing for Codex" [ ! -e "$work/home/.cache/codefast" ]
    check "codex app server: plugin/read reports the MCP server and the skill" probe "sys.exit(0 if plugin.get('mcpServers') == ['codefast'] and [s['name'] for s in plugin.get('skills', [])] == ['codefast:codefast'] else 1)"
    check "codex app server: plugin/read shows the display name from extensions.com.openai" probe "sys.exit(0 if plugin.get('summary', {}).get('interface', {}).get('displayName') == 'codefast' else 1)"
    check "codex app server: hooks/codex.json as a config-layer hooks.json gives three hooks, no errors" probe "sys.exit(0 if sorted(h['eventName'] for h in hooks) == ['postToolUse', 'preToolUse', 'sessionStart'] and not problems and all(h.get('source') == 'user' for h in hooks) else 1)"
    if probe "sys.exit(0 if plugin.get('hooks') == [] else 1)"; then
        echo "note  codex loads no hooks from this plugin (expected: Codex skips hooks for the Agent Plugins manifest)"
    else
        echo "note  codex now reports plugin hooks: docs/HARNESSES.md says it does not, update it"
    fi

    # `codefast setup codex` registers the server in config.toml even when the plugin is
    # installed, because Codex starts the plugin's own server in the plugin's directory. That
    # only helps as long as Codex uses the configured server in place of the plugin's.
    codex "$codex_bin" mcp add codefast -- "$work/codex-bin/codefast" mcp
    check "codex mcp add codefast next to the plugin" [ "$?" = 0 ]
    codex "$codex_bin" mcp list --json
    check "codex then runs one server named codefast: the configured one, started in the project" python3 -I -c "
import json, sys
servers = [s for s in json.load(open('$work/out')) if s['name'] == 'codefast']
t = servers[0]['transport'] if len(servers) == 1 else {}
sys.exit(0 if t.get('command') == '$work/codex-bin/codefast' and not t.get('cwd') and not t.get('env') else 1)"
else
    echo "# Codex: not installed, skipped"
fi

echo
echo "$pass passed, $fail failed"
if [ "$fail" != 0 ]; then
    echo "scratch directory kept: $work"
    exit 1
fi
