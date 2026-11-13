#!/bin/sh
# Exercises bin/codefast (the launcher) and install.sh against a throwaway HOME, with a local
# directory standing in for the release server. Nothing outside the scratch directory is read
# or written, and no network is used.
#
#   tests/packaging/manifests/launcher_test.sh /path/to/a/built/codefast
#
# Environment:
#   CODEFAST_TEST_SH    shell that runs the two scripts (default: sh); try dash and busybox sh
#   CODEFAST_TEST_WORK  scratch directory (default: a fresh one under ${TMPDIR:-/tmp}, removed
#                       when every check passes)
#   CODEFAST_TEST_URL_PREFIX  what turns an absolute directory into a URL (default: file://).
#                       BusyBox has no curl and its wget cannot read file:// URLs: there, serve
#                       the filesystem (`httpd -p 127.0.0.1:8099 -h /`) and pass
#                       http://127.0.0.1:8099
#
# The binary may be any executable that answers `--version`; the MCP check compares what the
# launcher prints with what the binary prints, so a stub works too.

set -u

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P) || exit 2
repo=$(CDPATH='' cd -- "$here/../../.." && pwd -P) || exit 2
real=${1:-}
if [ -z "$real" ] || [ ! -x "$real" ]; then
    echo "usage: $0 /path/to/codefast" >&2
    exit 2
fi
case $real in
/*) ;;
*) real=$(pwd)/$real ;;
esac
sh_under_test=${CODEFAST_TEST_SH:-sh}

keep=1
work=${CODEFAST_TEST_WORK:-}
if [ -z "$work" ]; then
    work=$(mktemp -d "${TMPDIR:-/tmp}/codefast-launcher-test.XXXXXX") || exit 2
    keep=0
fi
mkdir -p "$work" || exit 2
work=$(CDPATH='' cd -- "$work" && pwd -P) || exit 2
rm -rf "$work/t"
mkdir -p "$work/t/tmp"
work=$work/t

# The tools the scripts need, without whatever else is on the caller's PATH.
base_path=/usr/bin:/bin:/usr/sbin:/sbin

pass=0
fail=0
ok() {
    pass=$((pass + 1))
    printf 'ok    %s\n' "$1"
}
bad() {
    fail=$((fail + 1))
    printf 'FAIL  %s\n' "$1"
    shift
    for line in "$@"; do printf '      %s\n' "$line"; done
}
# check NAME CONDITION...: CONDITION is a command; the check passes when it succeeds.
check() {
    name=$1
    shift
    if "$@"; then ok "$name"; else bad "$name" "command failed: $*" "stderr: $(cat "$work/err" 2>/dev/null | head -5)"; fi
}
empty() { [ ! -s "$1" ]; }
has() { grep -F -q -- "$2" "$1"; }
code_is() { [ "$code" = "$1" ]; }

launcher_version=$(sed -n 's/^version=//p' "$repo/bin/codefast" | head -1)
expect_version=$("$real" --version) || exit 2

# Runs a command with a scrubbed environment; stdout, stderr and the exit code are kept.
# usage: run_in HOME [VAR=VALUE...] COMMAND...
run_in() {
    run_home=$1
    shift
    env -i HOME="$run_home" TMPDIR="$work/tmp" PATH="${test_path:-$base_path}" "$@" >"$work/out" 2>"$work/err" </dev/null
    code=$?
}

# A plugin install: the launcher next to a manifest, and no source tree around it.
new_plugin() {
    rm -rf "$1"
    mkdir -p "$1/bin"
    cp "$repo/bin/codefast" "$1/bin/codefast"
    chmod 755 "$1/bin/codefast"
    printf '{"name":"codefast"}\n' >"$1/plugin.json"
}
new_home() {
    rm -rf "$1"
    mkdir -p "$1"
}

# ----- a local "release server" -----
target=$(env -i PATH="$base_path" HOME="$work/nohome" "$sh_under_test" "$repo/install.sh" --dry-run | sed -n 's/^platform: *//p')
if [ -z "$target" ]; then
    echo "install.sh --dry-run did not name a platform" >&2
    exit 2
fi
asset=codefast-$target.tar.gz
rel=$work/rel
mkdir -p "$rel/download/v$launcher_version" "$rel/latest/download" "$work/stage"
cp "$real" "$work/stage/codefast"
(cd "$work/stage" && tar -czf "$rel/download/v$launcher_version/$asset" codefast) || exit 2
digest_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1"
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1"
    else
        openssl dgst -sha256 -r "$1"
    fi | sed 's/[^0-9a-f].*//'
}
good_sum=$(digest_of "$rel/download/v$launcher_version/$asset")
printf '%s  %s\n' "$good_sum" "$asset" >"$rel/download/v$launcher_version/SHA256SUMS"
cp "$rel/download/v$launcher_version/$asset" "$rel/download/v$launcher_version/SHA256SUMS" "$rel/latest/download/"
url_prefix=${CODEFAST_TEST_URL_PREFIX:-file://}
base=$url_prefix$rel

echo "# shell under test: $sh_under_test    platform: $target    launcher version: $launcher_version"

# ----- launcher: 1. explicit override -----
new_plugin "$work/p1"
new_home "$work/h1"
run_in "$work/h1" CODEFAST_BIN="$real" CODEFAST_NO_DOWNLOAD=1 "$sh_under_test" "$work/p1/bin/codefast" --version
check "CODEFAST_BIN runs the named binary" [ "$(cat "$work/out")" = "$expect_version" ]
check "CODEFAST_BIN: nothing on stderr" empty "$work/err"

run_in "$work/h1" CODEFAST_BIN="$work/nope" "$sh_under_test" "$work/p1/bin/codefast" --version
check "CODEFAST_BIN that does not exist: exit 126" code_is 126
check "CODEFAST_BIN that does not exist: stdout empty" empty "$work/out"
check "CODEFAST_BIN that does not exist: says why" has "$work/err" "not an executable file"

run_in "$work/h1" CODEFAST_BIN="$work/p1/bin/codefast" "$sh_under_test" "$work/p1/bin/codefast" --version
check "CODEFAST_BIN naming the launcher itself is refused" code_is 126
check "CODEFAST_BIN naming the launcher: says why" has "$work/err" "launcher script"

# ----- launcher: 5. download, then 2. cache -----
new_plugin "$work/p2"
new_home "$work/h2"
run_in "$work/h2" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$work/p2/bin/codefast" --version
check "download: exit 0" code_is 0
check "download: stdout is exactly the binary's output" [ "$(cat "$work/out")" = "$expect_version" ]
check "download: progress went to stderr" has "$work/err" "fetching codefast"
check "download: binary is in the cache" [ -x "$work/h2/.cache/codefast/$launcher_version/codefast" ]
check "download: no temporary directory left behind" [ -z "$(find "$work/h2/.cache/codefast" -name '.tmp.*')" ]

run_in "$work/h2" CODEFAST_RELEASE_BASE="$url_prefix$work/nowhere" "$sh_under_test" "$work/p2/bin/codefast" --version
check "cache: second run needs no download" [ "$(cat "$work/out")" = "$expect_version" ]
check "cache: second run is silent on stderr" empty "$work/err"

new_home "$work/h2b"
run_in "$work/h2b" XDG_CACHE_HOME="$work/h2b/xdg" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$work/p2/bin/codefast" --version
check "XDG_CACHE_HOME is honoured" [ -x "$work/h2b/xdg/codefast/$launcher_version/codefast" ]
new_home "$work/h2c"
run_in "$work/h2c" CODEFAST_CACHE_DIR="$work/h2c/own" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$work/p2/bin/codefast" --version
check "CODEFAST_CACHE_DIR is honoured" [ -x "$work/h2c/own/$launcher_version/codefast" ]

# ----- launcher: stdout stays clean for the MCP server, even on the run that downloads -----
request='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}'
new_home "$work/h3"
printf '%s\n' "$request" | env -i HOME="$work/h3" PATH="$base_path" "$real" mcp --root "$work" >"$work/direct.out" 2>/dev/null
printf '%s\n' "$request" | env -i HOME="$work/h3" PATH="$base_path" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$work/p2/bin/codefast" mcp --root "$work" >"$work/via.out" 2>"$work/err"
check "mcp: launcher output is byte-identical to the binary's (first run, with download)" cmp -s "$work/direct.out" "$work/via.out"
check "mcp: the download was announced on stderr only" has "$work/err" "fetching codefast"
printf '%s\n' "$request" | env -i HOME="$work/h3" PATH="$base_path" "$sh_under_test" "$work/p2/bin/codefast" mcp --root "$work" >"$work/via2.out" 2>"$work/err"
check "mcp: byte-identical again from the cache" cmp -s "$work/direct.out" "$work/via2.out"

# ----- launcher: tells the MCP server where the plugin lives -----
cat >"$work/printenv-bin" <<'EOF'
#!/bin/sh
printf 'root=%s\n' "${CODEFAST_PLUGIN_ROOT:-unset}"
EOF
chmod 755 "$work/printenv-bin"
run_in "$work/h1" CODEFAST_BIN="$work/printenv-bin" "$sh_under_test" "$work/p2/bin/codefast" mcp
check "mcp: CODEFAST_PLUGIN_ROOT is the plugin directory" [ "$(cat "$work/out")" = "root=$work/p2" ]
run_in "$work/h1" CODEFAST_BIN="$work/printenv-bin" "$sh_under_test" "$work/p2/bin/codefast" search x
check "other commands: CODEFAST_PLUGIN_ROOT is left alone" [ "$(cat "$work/out")" = "root=unset" ]
run_in "$work/h1" CODEFAST_BIN="$work/printenv-bin" CODEFAST_PLUGIN_ROOT=/elsewhere "$sh_under_test" "$work/p2/bin/codefast" mcp
check "mcp: an existing CODEFAST_PLUGIN_ROOT is kept" [ "$(cat "$work/out")" = "root=/elsewhere" ]
mkdir -p "$work/links"
ln -s "$work/p2/bin/codefast" "$work/links/codefast"
run_in "$work/h1" CODEFAST_BIN="$work/printenv-bin" "$sh_under_test" "$work/links/codefast" mcp
check "mcp: the plugin directory is found through a symlink" [ "$(cat "$work/out")" = "root=$work/p2" ]

# ----- launcher: checksum failures -----
new_plugin "$work/p4"
new_home "$work/h4"
mkdir -p "$work/badrel/download/v$launcher_version"
cp "$rel/download/v$launcher_version/$asset" "$work/badrel/download/v$launcher_version/"
printf '%s  %s\n' "0000000000000000000000000000000000000000000000000000000000000000" "$asset" >"$work/badrel/download/v$launcher_version/SHA256SUMS"
run_in "$work/h4" CODEFAST_RELEASE_BASE="$url_prefix$work/badrel" "$sh_under_test" "$work/p4/bin/codefast" --version
check "wrong checksum: exit 127" code_is 127
check "wrong checksum: stdout empty" empty "$work/out"
check "wrong checksum: reported" has "$work/err" "checksum mismatch"
check "wrong checksum: nothing installed" [ ! -e "$work/h4/.cache/codefast/$launcher_version/codefast" ]
check "wrong checksum: no temporary directory left behind" [ -z "$(find "$work/h4/.cache/codefast" -name '.tmp.*')" ]
run_in "$work/h4" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$work/p4/bin/codefast" --version
check "after a failure the next call fails fast" has "$work/err" "not retrying yet"
check "after a failure the next call exits 127" code_is 127
rm -f "$work/h4/.cache/codefast/$launcher_version/.failed"
run_in "$work/h4" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$work/p4/bin/codefast" --version
check "removing the marker lets the download run again" [ "$(cat "$work/out")" = "$expect_version" ]

new_home "$work/h4b"
printf 'not a checksum file\n' >"$work/badrel/download/v$launcher_version/SHA256SUMS"
run_in "$work/h4b" CODEFAST_RELEASE_BASE="$url_prefix$work/badrel" "$sh_under_test" "$work/p4/bin/codefast" --version
check "checksum file without the asset: refused" has "$work/err" "no entry for"
check "checksum file without the asset: nothing installed" [ ! -e "$work/h4b/.cache/codefast/$launcher_version/codefast" ]

new_home "$work/h4c"
run_in "$work/h4c" CODEFAST_RELEASE_BASE="$url_prefix$work/nowhere" "$sh_under_test" "$work/p4/bin/codefast" --version
check "missing asset: exit 127" code_is 127
check "missing asset: names the URL" has "$work/err" "could not download $url_prefix$work/nowhere/download/v$launcher_version/$asset"
check "missing asset: tells how to get a binary" has "$work/err" "CODEFAST_BIN"

# ----- launcher: digests pinned next to the launcher win over the published ones -----
new_plugin "$work/p5"
new_home "$work/h5"
printf '%s  %s\n' "$good_sum" "$asset" >"$work/p5/bin/SHA256SUMS"
run_in "$work/h5" CODEFAST_RELEASE_BASE="$url_prefix$work/badrel" "$sh_under_test" "$work/p5/bin/codefast" --version
check "pinned digest: accepted although the published file is wrong" [ "$(cat "$work/out")" = "$expect_version" ]
new_home "$work/h5b"
printf '%s  %s\n' "1111111111111111111111111111111111111111111111111111111111111111" "$asset" >"$work/p5/bin/SHA256SUMS"
run_in "$work/h5b" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$work/p5/bin/codefast" --version
check "pinned digest: a mismatch is refused although the published file matches" has "$work/err" "checksum mismatch"
check "pinned digest mismatch: nothing installed" [ ! -e "$work/h5b/.cache/codefast/$launcher_version/codefast" ]

# ----- launcher: a download that cannot run here is refused, not cached -----
new_plugin "$work/p5c"
new_home "$work/h5c"
mkdir -p "$work/alienrel/download/v$launcher_version" "$work/alienrel/latest/download" "$work/alienstage"
printf '\000\001not a program\n' >"$work/alienstage/codefast"
(cd "$work/alienstage" && tar -czf "$work/alienrel/download/v$launcher_version/$asset" codefast) || exit 2
printf '%s  %s\n' "$(digest_of "$work/alienrel/download/v$launcher_version/$asset")" "$asset" >"$work/alienrel/download/v$launcher_version/SHA256SUMS"
cp "$work/alienrel/download/v$launcher_version/$asset" "$work/alienrel/download/v$launcher_version/SHA256SUMS" "$work/alienrel/latest/download/"
run_in "$work/h5c" CODEFAST_RELEASE_BASE="$url_prefix$work/alienrel" "$sh_under_test" "$work/p5c/bin/codefast" --version
check "download that cannot run: exit 127" code_is 127
check "download that cannot run: stdout empty" empty "$work/out"
check "download that cannot run: reported" has "$work/err" "does not run on this machine"
check "download that cannot run: not put into the cache" [ ! -e "$work/h5c/.cache/codefast/$launcher_version/codefast" ]
mkdir -p "$work/goodbin"
cp "$real" "$work/goodbin/codefast"
test_path=$work/goodbin:$base_path
run_in "$work/h5c" "$sh_under_test" "$work/p5c/bin/codefast" --version
check "download that cannot run: a codefast on PATH is still reached afterwards" [ "$(cat "$work/out")" = "$expect_version" ]
test_path=

# ----- launcher: an unpublished copy never goes to the network -----
# Until the repository has a public home its URLs carry a stand-in owner, which is somebody
# else's account. The launcher must not fetch from it. These checks apply only while the
# stand-in is there (its name is put together here so that replacing it leaves this file alone).
standin=OWN
standin=${standin}ER
unpublished=0
grep -q "^release_base=.*/$standin/" "$repo/bin/codefast" && unpublished=1
mkdir -p "$work/nonet"
for tool in curl wget; do
    printf '#!/bin/sh\necho "$0 $*" >>"%s/nonet/calls"\nexit 22\n' "$work" >"$work/nonet/$tool"
    chmod 755 "$work/nonet/$tool"
done
if [ "$unpublished" = 1 ]; then
    new_plugin "$work/p5d"
    new_home "$work/h5d"
    test_path=$work/nonet:$base_path
    run_in "$work/h5d" "$sh_under_test" "$work/p5d/bin/codefast" --version
    check "unpublished copy, no binary: exit 127" code_is 127
    check "unpublished copy, no binary: stdout empty" empty "$work/out"
    check "unpublished copy, no binary: says there is no release" has "$work/err" "no published release"
    check "unpublished copy, no binary: does not send anyone to the stand-in URL" [ -z "$(grep -F "/$standin/" "$work/err")" ]
    run_in "$work/h5d" "$sh_under_test" "$work/p5d/bin/codefast" hook session-start
    sleep 1
    check "unpublished copy, hook session-start: exit 0" code_is 0
    check "unpublished copy, hook session-start: silent" empty "$work/err"
    check "unpublished copy: neither curl nor wget was started" [ ! -e "$work/nonet/calls" ]
    check "unpublished copy: nothing written under HOME" [ -z "$(ls -A "$work/h5d")" ]
    test_path=
else
    echo "note  the launcher's release URL is real: the unpublished-copy checks do not apply"
fi

# ----- launcher: 3. PATH -----
new_plugin "$work/p6"
new_home "$work/h6"
mkdir -p "$work/pathbin"
cp "$real" "$work/pathbin/codefast"
test_path=$work/p6/bin:$work/links:$work/pathbin:$base_path
run_in "$work/h6" CODEFAST_NO_DOWNLOAD=1 codefast --version
check "PATH: started by name, the launcher skips itself and other launchers and runs the real binary" [ "$(cat "$work/out")" = "$expect_version" ]
check "PATH: silent on stderr" empty "$work/err"
run_in "$work/h6" CODEFAST_NO_DOWNLOAD=1 "$sh_under_test" "$work/p6/bin/codefast" --version
check "PATH: the same under the shell under test" [ "$(cat "$work/out")" = "$expect_version" ]
test_path=$work/p6/bin:$base_path
run_in "$work/h6" CODEFAST_NO_DOWNLOAD=1 "$sh_under_test" "$work/p6/bin/codefast" --version
check "PATH with only the launcher: no loop, exit 127" code_is 127
check "PATH with only the launcher: explains" has "$work/err" "CODEFAST_NO_DOWNLOAD=1 forbids"
mkdir -p "$work/cwdtrap"
cp "$work/printenv-bin" "$work/cwdtrap/codefast"
test_path=$work/p6/bin::.:$base_path
(cd "$work/cwdtrap" && env -i HOME="$work/h6" PATH="$test_path" CODEFAST_NO_DOWNLOAD=1 "$sh_under_test" "$work/p6/bin/codefast" --version >"$work/out" 2>"$work/err")
check "PATH: a codefast in the working directory is not picked up through relative entries" empty "$work/out"
test_path=

# ----- launcher: 4. local build in a source checkout -----
rm -rf "$work/src"
mkdir -p "$work/src/bin" "$work/src/target/release"
cp "$repo/bin/codefast" "$work/src/bin/codefast"
: >"$work/src/Cargo.toml"
cp "$real" "$work/src/target/release/codefast"
new_home "$work/h7"
run_in "$work/h7" CODEFAST_NO_DOWNLOAD=1 "$sh_under_test" "$work/src/bin/codefast" --version
check "source checkout: target/release/codefast is used" [ "$(cat "$work/out")" = "$expect_version" ]
rm "$work/src/Cargo.toml"
run_in "$work/h7" CODEFAST_NO_DOWNLOAD=1 "$sh_under_test" "$work/src/bin/codefast" --version
check "without Cargo.toml a target/ directory is not trusted" code_is 127

# ----- launcher: hooks never fail and never speak -----
new_plugin "$work/p8"
new_home "$work/h8"
run_in "$work/h8" CODEFAST_HOOKS=off CODEFAST_BIN="$work/nope" "$sh_under_test" "$work/p8/bin/codefast" hook session-start
check "CODEFAST_HOOKS=off: exit 0" code_is 0
check "CODEFAST_HOOKS=off: no output" empty "$work/out"
check "CODEFAST_HOOKS=off: no stderr" empty "$work/err"
# A harness reads a hook's stdout and stderr through pipes and waits until both are closed.
# The background download must not keep them open, or the hook only "returns" when the
# download ends. The downloader here is a stand-in that takes five seconds and then fails.
mkdir -p "$work/slownet"
for tool in curl wget; do
    printf '#!/bin/sh\nsleep 5\nexit 22\n' >"$work/slownet/$tool"
    chmod 755 "$work/slownet/$tool"
done
new_home "$work/h8s"
started=$(date +%s)
piped=$(env -i HOME="$work/h8s" TMPDIR="$work/tmp" PATH="$work/slownet:$base_path" CODEFAST_RELEASE_BASE="http://127.0.0.1:9/none" "$sh_under_test" "$work/p8/bin/codefast" hook session-start </dev/null 2>&1)
code=$?
elapsed=$(($(date +%s) - started))
check "hook session-start with a slow download: exit 0" code_is 0
check "hook session-start with a slow download: its output pipes close at once, not when the download ends (took ${elapsed}s)" [ "$elapsed" -lt 4 ]
check "hook session-start with a slow download: nothing came through the pipes" [ -z "$piped" ]

for value in OFF 0 false No disabled; do
    run_in "$work/h8" CODEFAST_HOOKS="$value" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$work/p8/bin/codefast" hook session-start
    sleep 1
    check "CODEFAST_HOOKS=$value: exit 0" code_is 0
    check "CODEFAST_HOOKS=$value: no output" empty "$work/out"
    check "CODEFAST_HOOKS=$value: no stderr" empty "$work/err"
    check "CODEFAST_HOOKS=$value: no background download" [ ! -e "$work/h8/.cache/codefast/$launcher_version/codefast" ]
done
run_in "$work/h8" CODEFAST_BIN="$work/nope" "$sh_under_test" "$work/p8/bin/codefast" hook pre-tool-use
check "hook with a broken CODEFAST_BIN: exit 0" code_is 0
check "hook with a broken CODEFAST_BIN: silent" empty "$work/err"
run_in "$work/h8" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$work/p8/bin/codefast" hook pre-tool-use
check "hook pre-tool-use without a binary: exit 0" code_is 0
check "hook pre-tool-use without a binary: no output" empty "$work/out"
check "hook pre-tool-use without a binary: no stderr" empty "$work/err"
check "hook pre-tool-use without a binary: no download" [ ! -e "$work/h8/.cache/codefast/$launcher_version/codefast" ]
run_in "$work/h8" CODEFAST_RELEASE_BASE="$base" CODEFAST_NO_DOWNLOAD=1 "$sh_under_test" "$work/p8/bin/codefast" hook session-start
sleep 1
check "hook session-start with CODEFAST_NO_DOWNLOAD=1: no download" [ ! -e "$work/h8/.cache/codefast/$launcher_version/codefast" ]
run_in "$work/h8" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$work/p8/bin/codefast" hook session-start
check "hook session-start without a binary: exit 0" code_is 0
check "hook session-start without a binary: no output" empty "$work/out"
check "hook session-start without a binary: no stderr" empty "$work/err"
tries=0
while [ ! -x "$work/h8/.cache/codefast/$launcher_version/codefast" ] && [ "$tries" -lt 50 ]; do
    sleep 0.2 2>/dev/null || sleep 1
    tries=$((tries + 1))
done
check "hook session-start: the binary arrives in the background" [ -x "$work/h8/.cache/codefast/$launcher_version/codefast" ]
cat >"$work/failing-bin" <<'EOF'
#!/bin/sh
echo '{"hookSpecificOutput":{}}'
exit 3
EOF
chmod 755 "$work/failing-bin"
run_in "$work/h8" CODEFAST_BIN="$work/failing-bin" "$sh_under_test" "$work/p8/bin/codefast" hook post-tool-use
check "hook: a failing binary still gives exit 0" code_is 0
check "hook: the binary's stdout is passed through" [ "$(cat "$work/out")" = '{"hookSpecificOutput":{}}' ]
printf '{"hook_event_name":"SessionStart"}\n' >"$work/payload"
cat >"$work/cat-bin" <<'EOF'
#!/bin/sh
cat
EOF
chmod 755 "$work/cat-bin"
env -i HOME="$work/h8" PATH="$base_path" CODEFAST_BIN="$work/cat-bin" "$sh_under_test" "$work/p8/bin/codefast" hook session-start <"$work/payload" >"$work/out" 2>"$work/err"
check "hook: stdin reaches the binary" cmp -s "$work/payload" "$work/out"

# ----- install.sh -----
new_home "$work/h9"
run_in "$work/h9" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$repo/install.sh" --dry-run
check "install --dry-run: exit 0" code_is 0
check "install --dry-run: names the asset URL" has "$work/out" "would download: $base/latest/download/$asset"
check "install --dry-run: names the checksum URL" has "$work/out" "would download: $base/latest/download/SHA256SUMS"
check "install --dry-run: names the destination" has "$work/out" "would install:  $work/h9/.local/bin/codefast"
check "install --dry-run: writes nothing" [ -z "$(ls -A "$work/h9")" ]

run_in "$work/h9" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$repo/install.sh"
check "install: exit 0" code_is 0
check "install: binary in ~/.local/bin" [ -x "$work/h9/.local/bin/codefast" ]
check "install: the installed binary runs" [ "$("$work/h9/.local/bin/codefast" --version)" = "$expect_version" ]
check "install: reports the checksum" has "$work/out" "checksum ok ($good_sum)"
check "install: says PATH is missing the directory" has "$work/out" "is not on your PATH"
check "install: touched nothing else in HOME" [ "$(cd "$work/h9" && find . -type f | sort | tr '\n' ' ')" = "./.local/bin/codefast " ]

run_in "$work/h9" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$repo/install.sh" --version "v$launcher_version" --bin-dir "$work/h9/custom dir"
check "install --version --bin-dir: exit 0" code_is 0
check "install --bin-dir: binary in the named directory (with a space in it)" [ -x "$work/h9/custom dir/codefast" ]
check "install --version: used the versioned URL" has "$work/out" "downloading $base/download/v$launcher_version/$asset"

run_in "$work/h9" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$repo/install.sh" --dry-run --bin-dir "$work/h9/custom dir"
check "install --dry-run over an existing binary: says replace" has "$work/out" "would replace:  $work/h9/custom dir/codefast"

new_home "$work/h10"
run_in "$work/h10" CODEFAST_RELEASE_BASE="$url_prefix$work/badrel" "$sh_under_test" "$repo/install.sh" --version "$launcher_version"
check "install with a checksum file that lacks the asset: non-zero exit" [ "$code" != 0 ]
check "install with a bad checksum file: nothing installed" [ ! -e "$work/h10/.local/bin/codefast" ]
printf '%s  %s\n' "0000000000000000000000000000000000000000000000000000000000000000" "$asset" >"$work/badrel/download/v$launcher_version/SHA256SUMS"
run_in "$work/h10" CODEFAST_RELEASE_BASE="$url_prefix$work/badrel" "$sh_under_test" "$repo/install.sh" --version "$launcher_version"
check "install with a wrong checksum: reported" has "$work/err" "checksum mismatch"
check "install with a wrong checksum: nothing installed" [ ! -e "$work/h10/.local/bin/codefast" ]

mkdir -p "$work/h10/ro"
chmod 555 "$work/h10/ro"
run_in "$work/h10" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$repo/install.sh" --bin-dir "$work/h10/ro/bin"
if [ "$(id -u)" = 0 ]; then
    ok "install into an unwritable directory (skipped: running as root)"
else
    check "install into an unwritable directory: fails without sudo" [ "$code" != 0 ]
    check "install into an unwritable directory: suggests --bin-dir" has "$work/err" "does not use sudo"
fi
chmod 755 "$work/h10/ro"

# A release whose binary cannot run here must not cost the user the codefast they have.
new_home "$work/h10b"
run_in "$work/h10b" CODEFAST_RELEASE_BASE="$base" "$sh_under_test" "$repo/install.sh"
check "install (before the next check): a working codefast is in place" [ "$("$work/h10b/.local/bin/codefast" --version 2>/dev/null)" = "$expect_version" ]
run_in "$work/h10b" CODEFAST_RELEASE_BASE="$url_prefix$work/alienrel" "$sh_under_test" "$repo/install.sh"
check "install of a binary that cannot run: non-zero exit" [ "$code" != 0 ]
check "install of a binary that cannot run: reported" has "$work/err" "does not run on this machine"
check "install of a binary that cannot run: the codefast that was there still works" [ "$("$work/h10b/.local/bin/codefast" --version 2>/dev/null)" = "$expect_version" ]
check "install of a binary that cannot run: nothing else left in the directory" [ "$(ls -A "$work/h10b/.local/bin")" = codefast ]

if [ "$unpublished" = 1 ]; then
    new_home "$work/h10c"
    rm -f "$work/nonet/calls"
    test_path=$work/nonet:$base_path
    run_in "$work/h10c" "$sh_under_test" "$repo/install.sh"
    check "install from an unpublished copy: non-zero exit" [ "$code" != 0 ]
    check "install from an unpublished copy: says there is no release" has "$work/err" "no published release"
    check "install from an unpublished copy: neither curl nor wget was started" [ ! -e "$work/nonet/calls" ]
    check "install from an unpublished copy: nothing written under HOME" [ -z "$(ls -A "$work/h10c")" ]
    run_in "$work/h10c" "$sh_under_test" "$repo/install.sh" --dry-run
    check "install --dry-run from an unpublished copy: still prints the plan" has "$work/out" "would install:  $work/h10c/.local/bin/codefast"
    check "install --dry-run from an unpublished copy: says a real run would stop" has "$work/out" "no published release"
    test_path=
fi

run_in "$work/h10" "$sh_under_test" "$repo/install.sh" --no-such-flag
check "install: an unknown flag is an error" code_is 2

# ----- the installed binary satisfies the launcher through PATH -----
new_plugin "$work/p11"
test_path=$work/p11/bin:$work/h9/.local/bin:$base_path
run_in "$work/h9" CODEFAST_RELEASE_BASE="$url_prefix$work/nowhere" codefast --version
check "launcher + install.sh: the installed binary is found on PATH, no download" [ "$(cat "$work/out")" = "$expect_version" ]
test_path=

echo
echo "$pass passed, $fail failed"
if [ "$fail" = 0 ]; then
    [ "$keep" = 0 ] && rm -rf "${work%/t}"
    exit 0
fi
echo "scratch directory kept: $work"
exit 1
