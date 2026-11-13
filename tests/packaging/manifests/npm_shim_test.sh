#!/bin/sh
# Exercises npm/codefast/bin/codefast.js (the npm wrapper) in a throwaway node_modules layout,
# with a built codefast standing in for the platform package. Nothing is installed from or
# published to a registry, and no network is used.
#
#   tests/packaging/manifests/npm_shim_test.sh /path/to/a/built/codefast
#
# Environment: CODEFAST_TEST_WORK names the scratch directory (default: a fresh one under
# ${TMPDIR:-/tmp}, removed when every check passes).

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
node=$(command -v node 2>/dev/null) || {
    echo "node is not installed: skipped"
    exit 0
}

keep=1
work=${CODEFAST_TEST_WORK:-}
if [ -z "$work" ]; then
    work=$(mktemp -d "${TMPDIR:-/tmp}/codefast-npm-test.XXXXXX") || exit 2
    keep=0
fi
mkdir -p "$work" || exit 2
work=$(CDPATH='' cd -- "$work" && pwd -P) || exit 2
rm -rf "$work/n"
work=$work/n
mkdir -p "$work"

pass=0
fail=0
ok() {
    pass=$((pass + 1))
    printf 'ok    %s\n' "$1"
}
bad() {
    fail=$((fail + 1))
    printf 'FAIL  %s\n' "$1"
    printf '      stdout: %s\n' "$(head -c 300 "$work/out" 2>/dev/null)"
    printf '      stderr: %s\n' "$(head -c 300 "$work/err" 2>/dev/null)"
}
check() {
    name=$1
    shift
    if "$@"; then ok "$name"; else bad "$name"; fi
}
empty() { [ ! -s "$1" ]; }
has() { grep -F -q -- "$2" "$1"; }
code_is() { [ "$code" = "$1" ]; }

scope=$(sed -n 's/^ *"name": *"\(@[^/]*\)\/codefast",*$/\1/p' "$repo/npm/codefast/package.json" | head -1)
platform=$("$node" -p 'process.platform + "-" + process.arch')
exe=codefast
case $platform in win32-*) exe=codefast.exe ;; esac
expect_version=$("$real" --version) || exit 2
node_dir=$(dirname "$node")

# An install as npm lays it out: the wrapper, its platform package next to it, and the link
# that `npm install -g` puts on PATH.
layout() {
    rm -rf "$1"
    mkdir -p "$1/node_modules/$scope/codefast/bin" "$1/bin"
    cp "$repo/npm/codefast/package.json" "$1/node_modules/$scope/codefast/"
    cp "$repo/npm/codefast/bin/codefast.js" "$1/node_modules/$scope/codefast/bin/"
    chmod 755 "$1/node_modules/$scope/codefast/bin/codefast.js"
    ln -s "../node_modules/$scope/codefast/bin/codefast.js" "$1/bin/codefast"
}
with_platform_package() {
    mkdir -p "$1/node_modules/$scope/codefast-$platform/bin"
    cp "$repo/npm/platforms/$platform/package.json" "$1/node_modules/$scope/codefast-$platform/" 2>/dev/null ||
        printf '{"name":"%s/codefast-%s","version":"0"}\n' "$scope" "$platform" >"$1/node_modules/$scope/codefast-$platform/package.json"
    cp "$2" "$1/node_modules/$scope/codefast-$platform/bin/$exe"
    chmod 755 "$1/node_modules/$scope/codefast-$platform/bin/$exe"
}
# usage: run_in DIR [VAR=VALUE...] COMMAND...
run_in() {
    run_dir=$1
    shift
    env -i HOME="$work/home" PATH="$run_dir/bin:$node_dir:/usr/bin:/bin" "$@" >"$work/out" 2>"$work/err" </dev/null
    code=$?
}

echo "# node $("$node" --version)    platform package: $scope/codefast-$platform"

layout "$work/a"
with_platform_package "$work/a" "$real"
run_in "$work/a" codefast --version
check "the wrapper runs the binary of the platform package" [ "$(cat "$work/out")" = "$expect_version" ]
check "the wrapper adds nothing on stderr" empty "$work/err"

cat >"$work/exit7" <<'EOF'
#!/bin/sh
for a in "$@"; do printf '[%s]\n' "$a"; done
exit 7
EOF
chmod 755 "$work/exit7"
run_in "$work/a" CODEFAST_BIN="$work/exit7" codefast grep -F "a  b" ''
check "CODEFAST_BIN: the named program runs, arguments intact" [ "$(cat "$work/out")" = "$(printf '[grep]\n[-F]\n[a  b]\n[]')" ]
check "the exit status passes through" code_is 7

request='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}'
printf '%s\n' "$request" | env -i HOME="$work/home" PATH=/usr/bin:/bin "$real" mcp --root "$work" >"$work/direct.out" 2>/dev/null
printf '%s\n' "$request" | env -i HOME="$work/home" PATH="$work/a/bin:$node_dir:/usr/bin:/bin" codefast mcp --root "$work" >"$work/via.out" 2>"$work/err"
check "mcp: stdout through the wrapper is byte-identical to the binary's" cmp -s "$work/direct.out" "$work/via.out"

# CODEFAST_BIN naming the wrapper itself is what `export CODEFAST_BIN=$(which codefast)` gives
# after an npm install. The wrapper must not start itself again and again.
"$node" -e '
const { spawnSync } = require("node:child_process");
const [command, bin, out, err] = process.argv.slice(1);
const r = spawnSync(command, ["--version"], { env: { HOME: process.env.HOME, PATH: process.env.PATH, CODEFAST_BIN: bin }, timeout: 15000, killSignal: "SIGKILL" });
require("node:fs").writeFileSync(out, r.stdout || "");
require("node:fs").writeFileSync(err, r.stderr || "");
process.exit(r.error || r.signal ? 99 : r.status);
' "$work/a/bin/codefast" "$work/a/bin/codefast" "$work/out" "$work/err"
code=$?
# Should the chain exist after all, stop it before going on.
for _ in 1 2 3 4 5; do
    pkill -9 -f "$work/a/bin/codefast" 2>/dev/null || break
    sleep 0.2 2>/dev/null || sleep 1
done
check "CODEFAST_BIN naming the wrapper itself: ends (no endless chain of processes)" [ "$code" != 99 ]
check "CODEFAST_BIN naming the wrapper itself: the platform binary runs" [ "$(cat "$work/out")" = "$expect_version" ]

layout "$work/b"
run_in "$work/b" codefast --version
check "no platform package: exit 127" code_is 127
check "no platform package: stdout empty" empty "$work/out"
check "no platform package: names the package that is missing" has "$work/err" "$scope/codefast-$platform"

run_in "$work/b" CODEFAST_BIN="$work/nope" codefast --version
check "CODEFAST_BIN that does not exist: exit 126" code_is 126
check "CODEFAST_BIN that does not exist: stdout empty" empty "$work/out"

echo
echo "$pass passed, $fail failed"
if [ "$fail" = 0 ]; then
    [ "$keep" = 0 ] && rm -rf "${work%/n}"
    exit 0
fi
echo "scratch directory kept: $work"
exit 1
